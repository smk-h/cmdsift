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
mod runner;
mod sanitize;

use std::env;
use std::process;
use std::time::Duration;

use classify::{BuildCollector, TAIL_LINES, collect_output, format_build_result, tail_lines};
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
  -h, --help                 显示本帮助
  -V, --version              显示版本

输出:
  分类模式（默认）：构建状态 + 统计摘要 + 编号错误列表 + 编号警告列表
  非分类模式：构建状态 + 输出尾部 {TAIL_LINES} 行

退出码:
  编译命令的退出码；超时 {EXIT_TIMEOUT}；参数错误 {EXIT_USAGE}；启动失败 {EXIT_SPAWN}

示例:
  cmdsift 'make -j8'
  cmdsift -C /srv/kernel -m 1800000 './build.sh alpha -a -c'
  cmdsift --no-classify -- make -j8"
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

    let outcome = match run_build(
        &cli.command,
        cli.cwd.as_deref(),
        Duration::from_millis(cli.max_wait_ms),
        Duration::from_millis(cli.poll_interval_ms),
    ) {
        Ok(outcome) => outcome,
        Err(spawn_error) => {
            eprintln!("cmdsift: failed to spawn command: {spawn_error}");
            process::exit(EXIT_SPAWN);
        }
    };

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
