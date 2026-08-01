/*
 * =====================================================
 * Copyright © hk. 2022-2025. All rights reserved.
 * File name  : sanitize.rs
 * Author     : 苏木
 * Date       : 2026-08-02
 * Description: 终端输出清洗，剥离 ANSI 转义序列与控制字符
 * ======================================================
 */

use regex::Regex;
use std::sync::LazyLock;

// ANSI CSI 序列：ESC[ + 参数 + 字母（SGR 颜色、光标控制等）
static CSI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\x1b\\[[0-9;]*[A-Za-z]").unwrap());

// ANSI OSC 序列：ESC] ... BEL（如窗口标题）
static OSC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("\x1b\\][^\x07]*\x07").unwrap());

// 其他 ESC 开头的 ANSI 序列（[^\[] 匹配除 [ 外的字符，避免与 CSI 重复）
static OTHER_ANSI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b[^\[]][0-9;]*[A-Za-z]").unwrap());

// 除 \n \t 之外的控制字符（0x00-0x08, 0x0B-0x0C, 0x0E-0x1F）
static CONTROL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("[\x00-\x08\x0B\x0C\x0E-\x1F]").unwrap());

/**
 * @brief 清洗编译输出中的控制字符，防止终端显示错乱与分类正则失效
 *
 * gcc 在 TTY 下会输出彩色的 warning:/error: 前缀
 * （如 "\x1b[0;32mwarning:\x1b[0m" 中 warning 后是 ESC 而非冒号），
 * 会破坏基于冒号的分类正则，因此分类前必须清洗。
 *
 * 清洗策略：
 *   1. \r\n → \n（CRLF 归一化为 LF）
 *   2. 孤立的 \r（无 \n 跟随）→ \n（视为换行）
 *   3. 移除 ANSI CSI 序列（\x1b[...m, \x1b[...A/B/C/D 等）
 *   4. 移除其他 ANSI 序列（ESC]...BEL 等）
 *   5. 移除其他控制字符（保留 \n 和 \t）
 *
 * @param raw  原始输出字符串
 * @return 清洗后的安全字符串，可安全打印到终端
 */
pub fn sanitize(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let without_csi = CSI_RE.replace_all(&normalized, "");
    let without_osc = OSC_RE.replace_all(&without_csi, "");
    let without_ansi = OTHER_ANSI_RE.replace_all(&without_osc, "");
    CONTROL_RE.replace_all(&without_ansi, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_color() {
        assert_eq!(sanitize("\x1b[0;32mwarning:\x1b[0m x"), "warning: x");
    }

    #[test]
    fn normalizes_crlf_and_lone_cr() {
        assert_eq!(sanitize("a\r\nb\rc"), "a\nb\nc");
    }

    #[test]
    fn strips_osc_and_control_chars() {
        assert_eq!(sanitize("\x1b]0;title\x07ok\x07done"), "okdone");
        assert_eq!(sanitize("a\x00\x08b"), "ab");
    }

    #[test]
    fn keeps_tab_and_newline() {
        assert_eq!(sanitize("a\tb\nc"), "a\tb\nc");
    }
}
