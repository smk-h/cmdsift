/*
 * =====================================================
 * Copyright © hk. 2022-2025. All rights reserved.
 * File name  : classify.rs
 * Author     : 苏木
 * Date       : 2026-08-02
 * Description: 编译输出行分类、采集与结构化结果格式化
 * ======================================================
 */

use regex::Regex;
use std::sync::LazyLock;

// 非分类模式下返回的尾部行数
pub const TAIL_LINES: usize = 50;

/**
 * @brief 取字符串最后 N 行（不足 N 行则返回全部；空串返回占位提示）
 *
 * @param text  完整输出文本
 * @param n     保留的尾部行数
 * @return 尾部 N 行文本，超长时带截断行数前缀
 */
pub fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() <= n {
        return if text.is_empty() {
            "(no output)".to_string()
        } else {
            text.to_string()
        };
    }
    format!(
        "...(truncated {} lines)\n{}",
        lines.len() - n,
        lines[lines.len() - n..].join("\n")
    )
}

// 输出行分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildCategory {
    Error,
    Warning,
    Info,
}

// 编译输出分类采集队列（info 只计数不存储，避免内存膨胀）
#[derive(Debug, Default)]
pub struct BuildCollector {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub info_count: usize,
}

/*
 * 错误模式匹配规则：
 *   error: / fatal: / [ERROR] — 通用编译错误/致命错误前缀（含日志格式 [ERROR]）
 *   undefined reference       — 链接阶段：未定义引用（ld 经典报错）
 *   No rule to make           — make：无规则可生成目标（缺少 Makefile 目标）
 *   make[N]: *** / make: ***  — make：严重构建失败标记
 *   cannot find / can't find  — 编译/链接：找不到文件或符号（如 "cannot find -lfoo"）
 *   collect2: error           — GCC 包装器调用 ld 失败
 *   ld returned               — ld 非零退出（链接阶段失败汇总）
 *   failed: / failed          — 构建步骤显式失败（如 "make: *** [all] Error 2"）
 *   no such file or directory — 文件系统：找不到源文件或头文件
 */
static ERROR_RES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\berror\b[:\s\]]",
        r"(?i)\bfatal\b[:\s]",
        r"(?i)undefined reference",
        r"(?i)No rule to make",
        r"(?i)make(?:\[.+\])?: \*\*\*",
        r"(?i)cannot find",
        r"(?i)can'?t\s+find",
        r"(?i)collect2: error",
        r"(?i)ld returned",
        r"(?i)\bfailed\b[:\s\]]",
        r"(?i)no such file or directory",
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).unwrap())
    .collect()
});

/*
 * 警告模式匹配规则：
 *   warning: / [WARNING] — 通用编译警告前缀（含日志格式 [WARNING]）
 *   warn: / [WARN]       — 简写形式的警告日志
 *   deprecated           — 弃用 API 提示
 *   [-W...]              — GCC/Clang 警告标识（如 "[-Wreturn-type]"、"[-Wimplicit]"）
 *                          注意：匹配 [-W 方括号形式，避免误判命令行中的 -Wall/-Wextra 等编译选项
 */
static WARNING_RES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\bwarning\b[:\s\]]",
        r"(?i)\bwarn\b[:\s\]]",
        r"(?i)\bdeprecated\b",
        r"(?i)\[-W[a-z]",
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).unwrap())
    .collect()
});

/**
 * @brief 编译输出行分类器
 *
 * 根据关键字匹配将单行输出归类为 error / warning / info。
 * 优先匹配 error（更严重），其次 warning，其余归入 info。
 *
 * @param line  待分类的单行输出
 * @return 分类结果
 */
pub fn classify_line(line: &str) -> BuildCategory {
    if ERROR_RES.iter().any(|regex| regex.is_match(line)) {
        return BuildCategory::Error;
    }
    if WARNING_RES.iter().any(|regex| regex.is_match(line)) {
        return BuildCategory::Warning;
    }
    BuildCategory::Info
}

/**
 * @brief 对编译输出按行分类并填入采集队列
 *
 * 遍历输出的每一行，调用 classify_line 进行分类，
 * 按类别追加到采集器中各自对应的数组（info 只计数）。
 *
 * @param collector   采集队列
 * @param raw_output  去除了完成标记的完整编译输出
 */
pub fn collect_output(collector: &mut BuildCollector, raw_output: &str) {
    for line in raw_output.split('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match classify_line(trimmed) {
            BuildCategory::Error => collector.errors.push(trimmed.to_string()),
            BuildCategory::Warning => collector.warnings.push(trimmed.to_string()),
            BuildCategory::Info => collector.info_count += 1,
        }
    }
}

/**
 * @brief 格式化分类采集结果为结构化文本
 *
 * 按优先级组织输出：
 *   1. 构建状态摘要（成功/失败、退出码、统计）
 *   2. 错误列表（带编号）
 *   3. 警告列表（带编号）
 *
 * @param collector  分类采集队列
 * @param exit_code  编译退出码
 * @return 结构化的编译结果文本
 */
pub fn format_build_result(collector: &BuildCollector, exit_code: i32) -> String {
    let status_label = if exit_code == 0 {
        "BUILD SUCCESS"
    } else {
        "BUILD FAILED"
    };
    let mut parts: Vec<String> = Vec::new();

    // 状态摘要
    parts.push(format!("{status_label} (exit code: {exit_code})"));
    parts.push(format!(
        "Summary: {} error(s), {} warning(s), {} info line(s)",
        collector.errors.len(),
        collector.warnings.len(),
        collector.info_count
    ));

    // 错误列表
    parts.push(String::new());
    parts.push(format!("=== ERRORS ({}) ===", collector.errors.len()));
    if collector.errors.is_empty() {
        parts.push("(none)".to_string());
    } else {
        for (index, error) in collector.errors.iter().enumerate() {
            parts.push(format!("[E{}] {error}", index + 1));
        }
    }

    // 警告列表
    parts.push(String::new());
    parts.push(format!("=== WARNINGS ({}) ===", collector.warnings.len()));
    if collector.warnings.is_empty() {
        parts.push("(none)".to_string());
    } else {
        for (index, warning) in collector.warnings.iter().enumerate() {
            parts.push(format!("[W{}] {warning}", index + 1));
        }
    }

    // 跳过完整构建日志（体量巨大，多数场景下 errors + warnings 足以分析问题）
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_gcc_error() {
        assert_eq!(
            classify_line("main.c:10:5: error: expected ';'"),
            BuildCategory::Error
        );
    }

    #[test]
    fn classifies_link_and_make_errors() {
        assert_eq!(
            classify_line("ld: undefined reference to `foo'"),
            BuildCategory::Error
        );
        assert_eq!(
            classify_line("make[2]: *** [subdir/foo.o] Error 1"),
            BuildCategory::Error
        );
        assert_eq!(
            classify_line("collect2: error: ld returned 1 exit status"),
            BuildCategory::Error
        );
    }

    #[test]
    fn classifies_warnings() {
        assert_eq!(
            classify_line("main.c:3:7: warning: unused variable [-Wunused-variable]"),
            BuildCategory::Warning
        );
        assert_eq!(
            classify_line("note: 'strdup' is deprecated"),
            BuildCategory::Warning
        );
    }

    #[test]
    fn does_not_treat_cflags_as_warning() {
        // 命令行中的 -Wall/-Wextra 不是 [-W 方括号形式，应归入 info
        assert_eq!(
            classify_line("gcc -Wall -Wextra -c main.c"),
            BuildCategory::Info
        );
    }

    #[test]
    fn classifies_plain_lines_as_info() {
        assert_eq!(classify_line("Compiling main.c ..."), BuildCategory::Info);
    }

    #[test]
    fn collects_and_formats() {
        let mut collector = BuildCollector::default();
        collect_output(
            &mut collector,
            "gcc -c main.c\nmain.c:1: error: boom\nmain.c:2: warning: hmm\n\n",
        );
        assert_eq!(collector.errors.len(), 1);
        assert_eq!(collector.warnings.len(), 1);
        assert_eq!(collector.info_count, 1);
        let formatted = format_build_result(&collector, 1);
        assert!(formatted.starts_with("BUILD FAILED (exit code: 1)"));
        assert!(formatted.contains("[E1] main.c:1: error: boom"));
        assert!(formatted.contains("[W1] main.c:2: warning: hmm"));
    }

    #[test]
    fn tail_lines_behaviour() {
        assert_eq!(tail_lines("", TAIL_LINES), "(no output)");
        assert_eq!(tail_lines("a\nb", TAIL_LINES), "a\nb");
        let long: String = (1..=60).map(|i| format!("line{i}\n")).collect();
        let tailed = tail_lines(long.trim_end(), 50);
        assert!(tailed.starts_with("...(truncated 10 lines)\n"));
        assert!(tailed.ends_with("line60"));
        assert!(!tailed.contains("line10\n"));
    }
}
