/*
 * =====================================================
 * Copyright © hk. 2022-2025. All rights reserved.
 * File name  : command.rs
 * Author     : 苏木
 * Date       : 2026-08-02
 * Description: 完成标记与 shell 命令包装
 * ======================================================
 */

// 编译完成标记，用于检测命令执行结束
pub const BUILD_MARKER: &str = "___MCP_BUILD_DONE___";

/**
 * @brief 构造包含完成标记的 shell 命令
 *
 * 如果指定了工作目录，先 cd 到该目录；
 * 编译命令的标准输出和标准错误合并后，追加完成标记（含退出码）。
 *
 * 命令结构: (cd <cwd> && <buildCmd>); echo "<marker>:$?"
 *
 * [()] 括号创建子 shell，cd 仅影响子 shell 工作目录，不污染父 shell
 *     cd 成功: 子 shell 工作目录切换到 cwd，继续执行 buildCmd
 *     cd 失败: 子 shell 工作目录不变，跳过 buildCmd，退出码为 cd 的非零值
 *
 * [&&] 逻辑与短路
 *     左侧成功(exit 0): 继续执行右侧 buildCmd
 *     左侧失败(exit ≠0): 跳过右侧 buildCmd，子 shell 退出码为左侧值
 *
 * [;] 分号无条件连接，无论子 shell 成功或失败，echo 始终执行
 *
 * [$?] 捕获上一条命令（即子 shell）的退出码
 *     cd 成功 + buildCmd 成功 → "$?:0"
 *     cd 成功 + buildCmd 失败 → "$?:buildCmd 退出码"
 *     cd 失败               → "$?:cd 退出码(非零)"
 *
 * 这是从流式输出通道外部可靠取得命令退出码的途径：
 * 管道/PTY 只有字节流，没有"命令完成"事件，只能由 shell 自己打印出来。
 *
 * @param command  编译命令
 * @param cwd      工作目录（可选）
 * @param marker   完成标记字符串
 * @return 完整的 shell 命令
 */
pub fn build_remote_command(command: &str, cwd: Option<&str>, marker: &str) -> String {
    let build_cmd = format!("{command} 2>&1");
    match cwd {
        Some(dir) => format!("(cd {dir} && {build_cmd}); echo \"{marker}:$?\""),
        None => format!("{build_cmd}; echo \"{marker}:$?\""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_cwd() {
        assert_eq!(
            build_remote_command("make -j8", None, BUILD_MARKER),
            "make -j8 2>&1; echo \"___MCP_BUILD_DONE___:$?\""
        );
    }

    #[test]
    fn with_cwd() {
        assert_eq!(
            build_remote_command("make", Some("/srv/build"), BUILD_MARKER),
            "(cd /srv/build && make 2>&1); echo \"___MCP_BUILD_DONE___:$?\""
        );
    }
}
