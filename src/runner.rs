/*
 * =====================================================
 * Copyright © hk. 2022-2025. All rights reserved.
 * File name  : runner.rs
 * Author     : 苏木
 * Date       : 2026-08-02
 * Description: 本地编译执行引擎
 *              （包装完成标记 → 增量轮询输出 → 检测标记取退出码 → 超时兜底）
 * ======================================================
 */

use regex::Regex;
use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::command::{BUILD_MARKER, build_remote_command};
use crate::log::LogWriter;

// 子进程已退出但标记未匹配时的残余输出宽限时间
const EXIT_GRACE: Duration = Duration::from_millis(500);

// 收尾阶段等待采集线程耗尽管道残余数据的最长时间
const DRAIN_GRACE: Duration = Duration::from_millis(300);

// 一次编译执行的产出
#[derive(Debug)]
pub struct BuildOutcome {
    // 编译退出码；None 表示超时（未在 max_wait 内检测到完成标记）
    pub exit_code: Option<i32>,
    // 剥离完成标记后的完整合并输出（stdout + stderr）
    pub output: String,
}

/**
 * @brief 启动一个输出采集线程，持续把流数据追加进共享缓冲区
 *
 * 后台线程不间断收集，主循环通过 drain 增量取走。
 *
 * @param reader  子进程管道读端（stdout 或 stderr）
 * @param buffer  共享输出缓冲区
 * @return 采集线程句柄（随管道 EOF 自然结束）
 */
fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    buffer: Arc<Mutex<String>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break, // EOF：子进程关闭了对端
                Ok(bytes_read) => {
                    let text = String::from_utf8_lossy(&chunk[..bytes_read]);
                    buffer.lock().unwrap().push_str(&text);
                }
                Err(_) => break,
            }
        }
    })
}

/**
 * @brief 取走共享缓冲区当前内容并清空
 *
 * @param buffer  共享输出缓冲区
 * @param output  累积输出，缓冲区内容追加到其尾部
 */
fn drain_buffer(buffer: &Mutex<String>, output: &mut String) {
    let mut guard = buffer.lock().unwrap();
    output.push_str(&guard);
    guard.clear();
}

/**
 * @brief 有界等待两个采集线程结束（或超时放弃）
 *
 * 子进程退出/被杀后管道即 EOF，采集线程会很快结束；
 * 仅当孙进程继承并持有管道写端时才可能迟迟不结束，此时超时放弃。
 *
 * @param readers    stdout / stderr 采集线程句柄
 * @param max_grace  最长等待时间
 */
fn wait_readers(readers: &(JoinHandle<()>, JoinHandle<()>), max_grace: Duration) {
    let deadline = Instant::now() + max_grace;
    while Instant::now() < deadline {
        if readers.0.is_finished() && readers.1.is_finished() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/**
 * @brief 在输出中匹配完成标记；命中则截掉标记及其后的尾部内容，返回退出码
 *
 * @param marker_regex  完成标记正则（标记:退出码）
 * @param output        累积输出，命中时被截断到标记之前
 * @return 解析出的退出码；未命中返回 None
 */
fn match_marker(marker_regex: &Regex, output: &mut String) -> Option<i32> {
    let captures = marker_regex.captures(output)?;
    let marker_start = captures.get(0)?.start();
    let exit_code = captures.get(1)?.as_str().parse::<i32>().ok()?;
    output.truncate(marker_start);
    *output = output.trim_end().to_string();
    Some(exit_code)
}

/**
 * @brief 在本地执行编译命令，轮询等待完成
 *
 * 流程：
 *   1. 构造 shell 命令（含工作目录切换和完成标记）
 *   2. spawn `sh -c`，启动 stdout/stderr 采集线程
 *   3. 轮询缓冲区，检测完成标记，增量读取输出
 *   4. 解析退出码，剥离完成标记
 *   5. 超时则杀死子进程并返回已收集的部分输出
 *
 * @param command        编译命令（如 "make -j8"、"./build.sh"）
 * @param cwd            工作目录（可选，子 shell 切换到该目录后再执行命令）
 * @param max_wait       最大等待时间（默认 600000ms 即 10 分钟）
 * @param poll_interval  轮询间隔（默认 2000ms）
 * @return 编译产出（退出码与合并输出）；spawn 失败返回 io::Error
 */
pub fn run_build(
    command: &str,
    cwd: Option<&str>,
    max_wait: Duration,
    poll_interval: Duration,
    mut log: Option<&mut LogWriter>,
) -> io::Result<BuildOutcome> {
    // ── 步骤 1：构造 shell 命令 ──
    // full_command 形如：
    //   (cd <cwd> && <command> 2>&1); echo "___MCP_BUILD_DONE___:$?"
    // 或（无 cwd）：
    //   <command> 2>&1; echo "___MCP_BUILD_DONE___:$?"
    let full_command = build_remote_command(command, cwd, BUILD_MARKER);

    // ── 步骤 2：spawn 并启动采集线程 ──
    // stdout、stderr 双管道各自一个采集线程汇入同一缓冲区，
    // 形成单一合并数据流（cd 失败等外壳错误信息也能被采集分类）
    let mut child: Child = Command::new("sh")
        .arg("-c")
        .arg(&full_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let buffer = Arc::new(Mutex::new(String::new()));
    let child_stdout = child.stdout.take().expect("stdout piped");
    let child_stderr = child.stderr.take().expect("stderr piped");
    // 采集线程随管道 EOF 自然结束；仅保留句柄到函数结束，避免孙进程持有管道时的边缘挂起
    let reader_threads = (
        spawn_reader(child_stdout, Arc::clone(&buffer)),
        spawn_reader(child_stderr, Arc::clone(&buffer)),
    );

    // ── 步骤 3/4：轮询缓冲区，检测完成标记 ──
    let marker_regex = Regex::new(&format!("{}:(\\d+)", regex::escape(BUILD_MARKER))).unwrap();
    let deadline = Instant::now() + max_wait;
    let mut all_output = String::new();
    let mut exit_code: Option<i32> = None;
    // 已写入日志的字节位置（增量写盘：只写新采集到的部分）
    let mut last_log_len = 0usize;

    loop {
        thread::sleep(poll_interval);
        drain_buffer(&buffer, &mut all_output);

        // 增量写盘：先把新采集内容落盘（方案A增强版，每 2s 一次）。
        // 在 match_marker 之前写入，确保 LogWriter 能看到完整的完成标记并自行截断。
        if let Some(writer) = log.as_deref_mut() {
            last_log_len = last_log_len.min(all_output.len());
            writer.write_chunk(&all_output[last_log_len..]);
            let _ = writer.flush();
            last_log_len = all_output.len();
        }

        if let Some(code) = match_marker(&marker_regex, &mut all_output) {
            exit_code = Some(code);
            break;
        }

        // 兜底：子进程已退出但标记未出现（如 sh 被信号杀死、输出通道异常），
        // 宽限收取残余输出后仍无标记，则以子进程退出码兜底，避免干等到超时
        if let Some(status) = child.try_wait()? {
            thread::sleep(EXIT_GRACE);
            drain_buffer(&buffer, &mut all_output);
            if let Some(code) = match_marker(&marker_regex, &mut all_output) {
                exit_code = Some(code);
            } else {
                exit_code = Some(status.code().unwrap_or(-1));
            }
            break;
        }

        if Instant::now() >= deadline {
            // 超时后杀死子进程，避免残留进程
            let _ = child.kill();
            break;
        }
    }

    // 回收子进程，避免僵尸进程
    let _ = child.wait();

    // ── 收尾：耗尽管道残余数据 ──
    // 完成标记经 stdout 到达时，stderr 线程的文本可能尚未写入缓冲区
    // （双管道流间无序，PTY 单流无此问题），子进程退出后管道即 EOF，
    // 有界等待采集线程结束后做最后一次 drain，避免丢失尾部的错误/警告行
    wait_readers(&reader_threads, DRAIN_GRACE);
    drain_buffer(&buffer, &mut all_output);
    all_output = all_output.trim_end().to_string();

    // ── 收尾：把收尾阶段采集到的残余内容写入日志并 flush 半行 ──
    if let Some(writer) = log.as_deref_mut() {
        last_log_len = last_log_len.min(all_output.len());
        writer.write_chunk(&all_output[last_log_len..]);
        writer.finish();
    }

    // 显式 detach 采集线程句柄（线程已随管道 EOF 结束或即将结束）
    drop(reader_threads);

    Ok(BuildOutcome {
        exit_code,
        output: all_output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::log::tests::{assert_all_lines_timestamped, strip_timestamps};
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn captures_output_and_success_code() {
        let outcome = run_build(
            "echo hello",
            None,
            Duration::from_secs(10),
            Duration::from_millis(50),
            None,
        )
        .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(outcome.output, "hello");
    }

    #[test]
    fn propagates_failure_exit_code() {
        let outcome = run_build(
            "echo oops; exit 3",
            None,
            Duration::from_secs(10),
            Duration::from_millis(50),
            None,
        )
        .unwrap();
        assert_eq!(outcome.exit_code, Some(3));
        assert!(outcome.output.contains("oops"));
        assert!(!outcome.output.contains(BUILD_MARKER));
    }

    #[test]
    fn merges_stderr_into_output() {
        let outcome = run_build(
            "echo to-stderr >&2",
            None,
            Duration::from_secs(10),
            Duration::from_millis(50),
            None,
        )
        .unwrap();
        assert!(outcome.output.contains("to-stderr"));
    }

    #[test]
    fn honors_cwd() {
        let outcome = run_build(
            "pwd",
            Some("/tmp"),
            Duration::from_secs(10),
            Duration::from_millis(50),
            None,
        )
        .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(outcome.output, "/tmp");
    }

    #[test]
    fn bad_cwd_fails_with_nonzero_code() {
        let outcome = run_build(
            "echo never-runs",
            Some("/nonexistent-dir-xyz"),
            Duration::from_secs(10),
            Duration::from_millis(50),
            None,
        )
        .unwrap();
        assert_ne!(outcome.exit_code, Some(0));
        assert!(!outcome.output.contains("never-runs"));
    }

    #[test]
    fn times_out_and_kills_child() {
        let start = Instant::now();
        let outcome = run_build(
            "sleep 30",
            None,
            Duration::from_millis(600),
            Duration::from_millis(100),
            None,
        )
        .unwrap();
        assert_eq!(outcome.exit_code, None);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    fn unique_log_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cmdsift_runner_{name}_{}", std::process::id()))
    }

    #[test]
    fn writes_incrementally_and_strips_marker() {
        let dir = unique_log_dir("incremental");
        let _ = fs::remove_dir_all(&dir);
        let mut writer = LogWriter::create(dir.to_str()).expect("create writer");
        let log_path = writer.path().to_path_buf();

        let outcome = run_build(
            "echo line1; echo line2",
            None,
            Duration::from_secs(10),
            Duration::from_millis(50),
            Some(&mut writer),
        )
        .unwrap();

        assert_eq!(outcome.exit_code, Some(0));
        let content = fs::read_to_string(&log_path).unwrap();
        // 日志每一行都应带时间戳前缀
        assert_all_lines_timestamped(&content);
        assert_eq!(strip_timestamps(&content), "line1\nline2");
        assert!(!content.contains(BUILD_MARKER));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn does_not_write_log_when_disabled() {
        let dir = unique_log_dir("disabled");
        let _ = fs::remove_dir_all(&dir);

        let outcome = run_build(
            "echo nothing-logged",
            None,
            Duration::from_secs(10),
            Duration::from_millis(50),
            None,
        )
        .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        // 未启用日志写入器，目录应保持为空
        assert!(!dir.exists() || fs::read_dir(&dir).unwrap().next().is_none());
        // 目录可能不存在，清理时容忍 NotFound
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
    }
}
