/*
 * =====================================================
 * Copyright © hk. 2022-2025. All rights reserved.
 * File name  : log.rs
 * Author     : 苏木
 * Date       : 2026-08-03
 * Description: 增量日志写盘（方案A增强版）
 *              输出流 → 按行累积到内存缓冲（保留半行拼接）
 *                    → 每 2 秒（即每次 drain 后）把"完整行"批量落盘一次
 *                    → 编译结束收尾 flush 残余半行
 * ======================================================
 */

use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::Local;

use crate::command::BUILD_MARKER;
use crate::sanitize::sanitize;

// 日志文件名时间戳格式：YYYYMMDD_HHMMSS（本地时间，编译开始时刻）
pub const LOG_TIMESTAMP_FMT: &str = "%Y%m%d_%H%M%S";
// 日志行时间戳格式：[YYYY-MM-DD HH:MM:SS]（本地时间，写盘时刻）
pub const LOG_LINE_TIMESTAMP_FMT: &str = "%Y-%m-%d %H:%M:%S";

/**
 * @brief 增量日志写入器
 *
 * 设计目标（方案A增强版）：
 *   1. 写入频率与输出规模解耦：无论编译输出多少行，写盘次数 ≈ 编译时长/2s
 *   2. 批量 flush 按行进行，保证行完整：缓冲到"最近的完整行"为止再写，
 *      避免 2s 边界把一行截成两半（破坏行级可读性）
 *   3. 完成标记（___MCP_BUILD_DONE___:exitcode）不会进入日志：
 *      遇到标记立即截断，标记之后的内容不再写盘
 *   4. 落盘是附属能力：创建/写入失败只打印一行 stderr 警告并降级为不写，
 *      不 panic、不影响编译结果与退出码
 */
pub struct LogWriter {
    // 底层文件缓冲写入器；写失败后置 None 禁用，避免反复刷错误
    file: Option<BufWriter<fs::File>>,
    // 日志文件完整路径
    path: PathBuf,
    // 已切分、尚未落盘的完整行（原始数据，flush 时统一 sanitize 后写入）
    pending: String,
    // 未遇到换行的残余半行（原始数据，等待后续块拼接补齐）
    partial: String,
    // 是否已遇到完成标记（命中后停止写盘）
    done: bool,
}

impl LogWriter {
    /**
     * @brief 创建日志文件并返回写入器
     *
     * 目标目录不存在时自动递归创建（幂等）；创建/打开失败仅提示并返回 None。
     *
     * @param log_dir 日志目录（None 时用默认目录 "log"，相对进程当前工作目录）
     * @return 成功返回 LogWriter；失败返回 None
     */
    pub fn create(log_dir: Option<&str>) -> Option<LogWriter> {
        let dir = log_dir.unwrap_or("log");
        // create_dir_all 在目录已存在时不报错，幂等
        if let Err(error) = fs::create_dir_all(dir) {
            eprintln!("cmdsift: failed to create log dir {dir}: {error}");
            return None;
        }
        let timestamp = Local::now().format(LOG_TIMESTAMP_FMT).to_string();
        let file_name = format!("{timestamp}.log");
        let path = Path::new(dir).join(file_name);

        match fs::File::create(&path) {
            Ok(file) => Some(LogWriter {
                file: Some(BufWriter::new(file)),
                path,
                pending: String::new(),
                partial: String::new(),
                done: false,
            }),
            Err(error) => {
                eprintln!(
                    "cmdsift: failed to create log file {}: {error}",
                    path.display()
                );
                None
            }
        }
    }

    /**
     * @brief 返回日志文件完整路径
     */
    pub fn path(&self) -> &Path {
        &self.path
    }

    /**
     * @brief 追加一块原始输出
     *
     * 按 \n 切出完整行累积到 pending，未遇到换行的半行暂存 partial 等待拼接；
     * 命中完成标记时截断到标记之前并停止后续写盘。
     *
     * @param chunk 本次采集到的原始输出增量块
     */
    pub fn write_chunk(&mut self, chunk: &str) {
        if self.done || self.file.is_none() {
            return;
        }
        self.partial.push_str(chunk);
        // 循环取出所有完整行（含行尾 \n）
        while let Some(idx) = self.partial.find('\n') {
            let complete: String = self.partial.drain(..=idx).collect();
            if let Some(marker_pos) = complete.find(BUILD_MARKER) {
                // 完成标记所在行：只保留标记之前的内容，之后不再写盘
                self.pending.push_str(&complete[..marker_pos]);
                self.done = true;
                break;
            }
            self.pending.push_str(&complete);
        }
    }

    /**
     * @brief 批量落盘：把 pending 中累积的完整行整体 sanitize 后写入文件
     *
     * 每 2 秒（即每次轮询 drain 后）调用一次，写盘次数与输出规模解耦。
     * 写入文件时给每一行加上时间戳前缀 `[YYYY-MM-DD HH:MM:SS] `
     * （时间戳取本批落盘时刻，同一批共享，避免逐行取时间戳的开销）。
     * 写入失败则禁用文件写入并返回错误（不影响编译主流程）。
     */
    pub fn flush(&mut self) -> io::Result<()> {
        if self.file.is_none() || self.pending.is_empty() {
            return Ok(());
        }
        let clean = sanitize(&self.pending);
        // 为日志每一行加上时间戳前缀 [YYYY-MM-DD HH:MM:SS]
        let timestamp = Local::now().format(LOG_LINE_TIMESTAMP_FMT).to_string();
        let prefix = format!("[{timestamp}] ");
        let timed: String = clean
            .lines()
            .map(|line| format!("{prefix}{line}\n"))
            .collect();
        let result = {
            let file = self.file.as_mut().expect("file checked");
            file.write_all(timed.as_bytes()).and_then(|_| file.flush())
        };
        match result {
            Ok(()) => {
                self.pending.clear();
                Ok(())
            }
            Err(error) => {
                eprintln!(
                    "cmdsift: failed to write log file {}: {error}",
                    self.path.display()
                );
                self.file = None;
                Err(error)
            }
        }
    }

    /**
     * @brief 收尾：把残余半行补齐为完整行写入，然后 flush 文件
     *
     * 编译结束时调用；半行即使不完整也按行写入（补 \n），保证日志不丢尾部内容。
     */
    pub fn finish(&mut self) {
        if !self.partial.is_empty() {
            // 补齐为完整行，统一走 write_chunk 的按行/marker 处理
            self.write_chunk("\n");
        }
        let _ = self.flush();
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush();
        }
    }
}

// 测试模块声明为 pub(crate)，供 main.rs / runner.rs 的测试复用时间戳断言辅助
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use regex::Regex;
    use std::fs;

    // 每个测试用独立子目录，避免并发测试下秒级文件名互相冲突
    fn unique_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cmdsift_log_{name}_{}", std::process::id()))
    }

    // 测试辅助：去除每行的时间戳前缀，返回纯日志内容
    pub fn strip_timestamps(content: &str) -> String {
        let re = Regex::new(r"^\[\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\] ").unwrap();
        content
            .lines()
            .map(|line| re.replace(line, "").into_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    // 测试辅助：断言每行都带有 [YYYY-MM-DD HH:MM:SS] 时间戳前缀
    pub fn assert_all_lines_timestamped(content: &str) {
        let re = Regex::new(r"^\[\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\] ").unwrap();
        assert!(!content.is_empty(), "log content should not be empty");
        for line in content.lines() {
            assert!(
                re.is_match(line),
                "line missing timestamp prefix: {line:?}"
            );
        }
    }

    #[test]
    fn writes_complete_lines_on_flush() {
        let dir = unique_dir("flush");
        let _ = fs::remove_dir_all(&dir);
        let mut writer = LogWriter::create(dir.to_str()).expect("create writer");

        // 文件名形如 YYYYMMDD_HHMMSS.log
        let name = writer
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(
            name.len() == "YYYYMMDD_HHMMSS.log".len() && name.ends_with(".log"),
            "unexpected log file name: {name}"
        );

        writer.write_chunk("line1\nline2\n");
        writer.flush().expect("flush");
        let content = fs::read_to_string(writer.path()).unwrap();
        assert_all_lines_timestamped(&content);
        assert_eq!(strip_timestamps(&content), "line1\nline2");

        writer.write_chunk("line3\n");
        writer.finish();
        let content = fs::read_to_string(writer.path()).unwrap();
        assert_all_lines_timestamped(&content);
        assert_eq!(strip_timestamps(&content), "line1\nline2\nline3");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn joins_partial_lines_across_chunks() {
        let dir = unique_dir("partial");
        let _ = fs::remove_dir_all(&dir);
        let mut writer = LogWriter::create(dir.to_str()).expect("create writer");

        writer.write_chunk("hel");
        writer.write_chunk("lo\nwor");
        writer.write_chunk("ld\n");
        writer.flush().expect("flush");
        let content = fs::read_to_string(writer.path()).unwrap();
        assert_all_lines_timestamped(&content);
        assert_eq!(strip_timestamps(&content), "hello\nworld");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sanitizes_before_writing() {
        let dir = unique_dir("sanitize");
        let _ = fs::remove_dir_all(&dir);
        let mut writer = LogWriter::create(dir.to_str()).expect("create writer");

        writer.write_chunk("\x1b[0;32mwarning:\x1b[0m x\r\nnext\n");
        writer.flush().expect("flush");
        let content = fs::read_to_string(writer.path()).unwrap();
        assert_all_lines_timestamped(&content);
        assert_eq!(strip_timestamps(&content), "warning: x\nnext");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn truncates_at_build_marker() {
        let dir = unique_dir("marker");
        let _ = fs::remove_dir_all(&dir);
        let mut writer = LogWriter::create(dir.to_str()).expect("create writer");

        writer.write_chunk("compiling...\n___MCP_BUILD_DONE___:0\n");
        writer.finish();
        let content = fs::read_to_string(writer.path()).unwrap();
        assert_all_lines_timestamped(&content);
        assert_eq!(strip_timestamps(&content), "compiling...");
        assert!(!content.contains(BUILD_MARKER));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn truncates_marker_split_across_chunks() {
        let dir = unique_dir("marker_split");
        let _ = fs::remove_dir_all(&dir);
        let mut writer = LogWriter::create(dir.to_str()).expect("create writer");

        writer.write_chunk("hello\n___MCP_BU");
        writer.write_chunk("ILD_DONE___:0\n");
        writer.finish();
        let content = fs::read_to_string(writer.path()).unwrap();
        assert_all_lines_timestamped(&content);
        assert_eq!(strip_timestamps(&content), "hello");
        assert!(!content.contains(BUILD_MARKER));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn flushes_residual_partial_line_on_finish() {
        let dir = unique_dir("finish_partial");
        let _ = fs::remove_dir_all(&dir);
        let mut writer = LogWriter::create(dir.to_str()).expect("create writer");

        writer.write_chunk("line1\nline2-no-newline");
        writer.finish();
        let content = fs::read_to_string(writer.path()).unwrap();
        assert_all_lines_timestamped(&content);
        assert_eq!(strip_timestamps(&content), "line1\nline2-no-newline");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn uses_default_log_dir_when_none() {
        let mut writer = LogWriter::create(None).expect("create in default dir");
        assert_eq!(writer.path().parent().unwrap(), Path::new("log"));
        writer.write_chunk("hello\n");
        writer.finish();
        let content = fs::read_to_string(writer.path()).unwrap();
        assert_all_lines_timestamped(&content);
        assert_eq!(strip_timestamps(&content), "hello");
        fs::remove_file(writer.path()).unwrap();
        // 清理测试创建的 log 目录（仅当为空时）
        let _ = fs::remove_dir("log");
    }

    #[test]
    fn creates_missing_dir() {
        let dir = std::env::temp_dir().join(format!(
            "cmdsift_log_missing_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let writer = LogWriter::create(dir.to_str()).expect("create missing dir");
        assert!(writer.path().exists());
        fs::remove_file(writer.path()).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
