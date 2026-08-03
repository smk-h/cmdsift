/*
 * =====================================================
 * Copyright © hk. 2022-2025. All rights reserved.
 * File name  : main.rs
 * Author     : 苏木
 * Date       : 2026-08-02
 * Description: cmdsift — 执行编译命令并返回结构化编译结果
 *
 *   将"发送命令 + 轮询等待 + 检测完成 + 分类输出"封装为一次 CLI 调用：
 *   在本地执行编译命令并等待完成，自动分类采集输出中的错误、警告和常规信息，
 *   以结构化格式打印，便于人或 AI 进行后续分析。
 * ======================================================
 */

#![forbid(unsafe_code)]

mod classify;
mod command;
mod log;
mod runner;
mod sanitize;

use std::env;
use std::process;
use std::time::Duration;

use classify::{BuildCollector, TAIL_LINES, collect_output, format_build_result, tail_lines};
use log::LogWriter;
use runner::run_build;
use sanitize::sanitize;

// 默认最大等待时间（毫秒）：10 分钟
const DEFAULT_MAX_WAIT_MS: u64 = 600_000;
// 默认轮询间隔（毫秒）
const DEFAULT_POLL_INTERVAL_MS: u64 = 2_000;
// 超时退出码（与 coreutils timeout 一致）
const EXIT_TIMEOUT: i32 = 124;
// 参数错误退出码
const EXIT_USAGE: i32 = 2;
// 命令启动失败退出码
const EXIT_SPAWN: i32 = 126;

// 日志文件名时间戳格式：YYYYMMDD_HHMMSS（本地时间），定义于 log.rs

// 命令行参数
struct Cli {
    // 编译命令字符串
    command: String,
    // 工作目录（可选）
    cwd: Option<String>,
    // 最大等待时间（毫秒）
    max_wait_ms: u64,
    // 轮询间隔（毫秒）
    poll_interval_ms: u64,
    // 是否对输出进行分类采集（默认 true）
    classify: bool,
    // 是否将完整编译输出写入日志文件（默认 true，--no-log-file 关闭）
    log_enabled: bool,
    // 日志文件所在目录（None 时用当前工作目录）
    log_dir: Option<String>,
}

// 参数解析结果
enum Parsed {
    Run(Cli),
    Help,
    Version,
}

/**
 * @brief 打印帮助信息到标准输出
 */
fn print_help() {
    println!(
        "cmdsift — 执行编译命令并返回结构化编译结果（错误/警告分类采集）

用法:
  cmdsift [选项] <命令>
  cmdsift [选项] -- <命令> [参数...]

参数:
  <命令>                     要执行的编译命令（如 'make -j8'、'./build.sh'）
                             含复杂引号/管道时，建议用单引号包裹为单个字符串

选项:
  -C, --cwd <目录>           切换到该目录后再执行编译命令
  -m, --max-wait <毫秒>      最大等待时间（默认 {DEFAULT_MAX_WAIT_MS}，即 10 分钟）
  -p, --poll-interval <毫秒> 轮询间隔（默认 {DEFAULT_POLL_INTERVAL_MS}）
      --no-classify          关闭输出分类，仅返回输出尾部 {TAIL_LINES} 行
  -L, --log-file <目录>      完整输出日志写入该目录（默认当前目录下的 log/）
      --no-log-file          不写日志文件（默认每次调用都会写）
  -h, --help                 显示本帮助
  -V, --version              显示版本

输出:
  分类模式（默认）：构建状态 + 统计摘要 + 编号错误列表 + 编号警告列表
  非分类模式：构建状态 + 输出尾部 {TAIL_LINES} 行
  日志文件：默认写入当前目录的 log/ 下 <YYYYMMDD_HHMMSS>.log，
            编译过程中边采集边写入（约每 2 秒批量落盘一次），记录完整编译输出

退出码:
  编译命令的退出码；超时 {EXIT_TIMEOUT}；参数错误 {EXIT_USAGE}；启动失败 {EXIT_SPAWN}

示例:
  cmdsift 'make -j8'
  cmdsift -C /srv/kernel -m 1800000 './build.sh alpha -a -c'
  cmdsift -L /var/log 'make -j8'
  cmdsift --no-classify -- make -j8
  cmdsift --no-log-file 'make -j8'"
    );
}

/**
 * @brief 从参数迭代器中取一个选项值（支持 `--opt value` 与 `--opt=value` 两种形式）
 *
 * @param args          参数迭代器
 * @param inline_value  `--opt=value` 形式中已拆出的内联值
 * @param option_name   选项名（用于错误提示）
 * @return 选项值；缺失时返回错误信息
 */
fn take_value(
    args: &mut std::vec::IntoIter<String>,
    inline_value: Option<&str>,
    option_name: &str,
) -> Result<String, String> {
    if let Some(value) = inline_value {
        return Ok(value.to_string());
    }
    args.next()
        .ok_or_else(|| format!("option {option_name} requires a value"))
}

/**
 * @brief 解析毫秒数选项
 *
 * @param raw_value    原始字符串值
 * @param option_name  选项名（用于错误提示）
 * @return 毫秒数；非非负整数时返回错误信息
 */
fn parse_ms(raw_value: String, option_name: &str) -> Result<u64, String> {
    raw_value.parse::<u64>().map_err(|_| {
        format!("option {option_name} expects a non-negative integer, got '{raw_value}'")
    })
}

/**
 * @brief 打开增量日志写入器
 *
 * 落盘是附属能力：创建/写入失败仅打印一行 stderr 警告，不 panic、不影响退出码，
 * 确保编译结果始终能正常返回（编译结果优先于日志留存）。
 *
 * 文件名以本地时间命名（编译开始时刻，YYYYMMDD_HHMMSS.log），
 * 默认写入当前工作目录下的 log/ 子目录；可用 --log-file 指定其它目录。
 *
 * @param log_dir   日志目录（None 时用默认目录 "log"）
 * @return 成功时返回日志写入器（含已打开的文件）；失败返回 None
 */
fn open_log_writer(log_dir: Option<&str>) -> Option<LogWriter> {
    LogWriter::create(log_dir)
}

/**
 * @brief 解析命令行参数
 *
 * @return 解析结果（运行参数 / 帮助 / 版本）；参数非法时返回错误信息
 */
fn parse_args() -> Result<Parsed, String> {
    let argv: Vec<String> = env::args().skip(1).collect();
    let mut args = argv.into_iter();

    let mut cwd: Option<String> = None;
    let mut max_wait_ms = DEFAULT_MAX_WAIT_MS;
    let mut poll_interval_ms = DEFAULT_POLL_INTERVAL_MS;
    let mut classify = true;
    // 完整编译输出默认落盘到当前目录的 log/ 子目录，--no-log-file 关闭
    let mut log_enabled = true;
    let mut log_dir: Option<String> = None;
    let mut command_parts: Vec<String> = Vec::new();

    while let Some(arg) = args.next() {
        // `--` 之后的内容整体作为命令
        if arg == "--" {
            command_parts.extend(args.by_ref());
            break;
        }

        // 拆分 --opt=value 形式
        let (flag, inline_value) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value)),
            _ => (arg.as_str(), None),
        };

        match flag {
            "-h" | "--help" => return Ok(Parsed::Help),
            "-V" | "--version" => return Ok(Parsed::Version),
            "--no-classify" => classify = false,
            "--no-log-file" => log_enabled = false,
            "-L" | "--log-file" => {
                log_dir = Some(take_value(&mut args, inline_value, "--log-file")?);
            }
            "-C" | "--cwd" => {
                cwd = Some(take_value(&mut args, inline_value, "--cwd")?);
            }
            "-m" | "--max-wait" => {
                let raw_value = take_value(&mut args, inline_value, "--max-wait")?;
                max_wait_ms = parse_ms(raw_value, "--max-wait")?;
            }
            "-p" | "--poll-interval" => {
                let raw_value = take_value(&mut args, inline_value, "--poll-interval")?;
                poll_interval_ms = parse_ms(raw_value, "--poll-interval")?;
            }
            _ if flag.starts_with('-') && flag.len() > 1 => {
                return Err(format!("unknown option: {flag}"));
            }
            _ => command_parts.push(arg),
        }
    }

    if command_parts.is_empty() {
        return Err("missing command".to_string());
    }

    Ok(Parsed::Run(Cli {
        command: command_parts.join(" "),
        cwd,
        max_wait_ms,
        poll_interval_ms,
        classify,
        log_enabled,
        log_dir,
    }))
}

/**
 * @brief 程序入口：解析参数 → 执行编译 → 分类输出 → 透传退出码
 */
fn main() {
    let cli = match parse_args() {
        Ok(Parsed::Run(cli)) => cli,
        Ok(Parsed::Help) => {
            print_help();
            return;
        }
        Ok(Parsed::Version) => {
            println!("cmdsift {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Err(error_message) => {
            eprintln!("cmdsift: {error_message}");
            eprintln!("try 'cmdsift --help' for more information");
            process::exit(EXIT_USAGE);
        }
    };

    // ── 日志写盘（方案A增强版：边采集边写）──
    // 编译开始时创建日志文件，采集线程每 2s 批量写一次完整行；
    // 落盘失败不影响主流程与退出码。路径提示走 stderr，不污染 stdout。
    let mut log_writer: Option<LogWriter> = None;
    if cli.log_enabled {
        log_writer = open_log_writer(cli.log_dir.as_deref());
    }

    let outcome = match run_build(
        &cli.command,
        cli.cwd.as_deref(),
        Duration::from_millis(cli.max_wait_ms),
        Duration::from_millis(cli.poll_interval_ms),
        log_writer.as_mut(),
    ) {
        Ok(outcome) => outcome,
        Err(spawn_error) => {
            eprintln!("cmdsift: failed to spawn command: {spawn_error}");
            process::exit(EXIT_SPAWN);
        }
    };

    // run_build 返回时日志已边采边写完并完成收尾；此处仅提示路径
    if let Some(writer) = log_writer {
        eprintln!("cmdsift: log saved: {}", writer.path().display());
    }

    // ── 超时/完成，统一格式化输出 ──
    // exit_code 为 None 表示超时（未检测到完成标记），否则为实际退出码
    let timed_out = outcome.exit_code.is_none();
    // 超时时用 -1 作为退出码占位符，统一为 i32 类型便于下游使用
    let resolved_exit_code = outcome.exit_code.unwrap_or(-1);
    let header = if timed_out {
        format!("Build timed out after {}ms.", cli.max_wait_ms)
    } else if resolved_exit_code == 0 {
        "BUILD SUCCESS (exit code: 0)".to_string()
    } else {
        format!("BUILD FAILED (exit code: {resolved_exit_code})")
    };

    // 剥离 ANSI 控制序列（gcc 在 TTY 下输出的彩色 warning:/error: 前缀会破坏
    // 分类正则，如 "\x1b[0;32mwarning:\x1b[0m" 中 warning 后是 ESC 而非冒号）。
    // 在标记剥离之后统一清洗，避免破坏 ___MCP_BUILD_DONE___ 检测。
    let output = sanitize(&outcome.output);

    if cli.classify {
        let mut collector = BuildCollector::default();
        collect_output(&mut collector, &output);
        let formatted = format_build_result(&collector, resolved_exit_code);
        if timed_out {
            println!(
                "{header}\nPartial: {} error(s), {} warning(s).\n\n{formatted}",
                collector.errors.len(),
                collector.warnings.len()
            );
        } else {
            println!("{formatted}");
        }
    } else {
        let tail = tail_lines(&output, TAIL_LINES);
        if timed_out {
            println!("{header}\n\nPartial output:\n{tail}");
        } else {
            println!("{header}\n\n{tail}");
        }
    }

    // CLI 附加约定：以编译命令的退出码作为进程退出码，便于脚本串联
    if timed_out {
        process::exit(EXIT_TIMEOUT);
    }
    process::exit(resolved_exit_code.clamp(0, 255));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    // 用唯一的临时文件名避免并行测试冲突；测试后清理
    #[test]
    fn open_log_writer_writes_content_to_given_dir() {
        let dir = std::env::temp_dir();
        let content = "main.c:1: error: boom\nCompiling...\n";
        let mut writer = open_log_writer(dir.to_str())
            .expect("should open log writer in temp dir");

        // 文件名形如 YYYYMMDD_HHMMSS.log
        let name = writer.path().file_name().unwrap().to_string_lossy().to_string();
        assert!(
            name.len() == "YYYYMMDD_HHMMSS.log".len()
                && name.ends_with(".log"),
            "unexpected log file name: {name}"
        );

        writer.write_chunk(content);
        writer.finish();
        let written = fs::read_to_string(writer.path()).unwrap();
        assert_eq!(written, content);
        fs::remove_file(writer.path()).unwrap();
    }

    #[test]
    fn open_log_writer_uses_default_dir_when_none() {
        // log_dir = None 时应写入默认目录 "log"
        let mut writer = open_log_writer(None).expect("should write to default log dir");
        assert_eq!(writer.path().parent().unwrap(), Path::new("log"));
        assert!(writer.path().exists());
        writer.write_chunk("hello\n");
        writer.finish();
        assert_eq!(fs::read_to_string(writer.path()).unwrap(), "hello\n");
        fs::remove_file(writer.path()).unwrap();
        // 清理测试创建的 log 目录（仅当为空时）
        let _ = fs::remove_dir("log");
    }

    #[test]
    fn open_log_writer_creates_missing_dir() {
        // 指定目录不存在时应自动创建（create_dir_all）
        let dir = std::env::temp_dir().join("cmdsift_test_subdir");
        let _ = fs::remove_dir_all(&dir); // 确保起始为干净状态
        let mut writer = open_log_writer(dir.to_str())
            .expect("should create missing dir and write file");
        assert!(writer.path().exists());
        writer.write_chunk("auto-create\n");
        writer.finish();
        assert_eq!(fs::read_to_string(writer.path()).unwrap(), "auto-create\n");
        fs::remove_file(writer.path()).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
