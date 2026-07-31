#!/bin/bash
# * =====================================================
# * Copyright © hk. 2022-2025. All rights reserved.
# * File name  : build.sh
# * Author     : 苏木
# * Date       : 2026-07-31
# * Description: cmdsift 项目构建脚本
# * ======================================================
##
# 脚本和工程路径
# ========================================================
SCRIPT_NAME=${0#*/}
SCRIPT_CURRENT_PATH=${0%/*}
SCRIPT_ABSOLUTE_PATH=`cd $(dirname ${0}); pwd`

# 工程根目录 (脚本所在目录即为工程根目录)
PROJECT_ROOT="${SCRIPT_ABSOLUTE_PATH}"

# 构建产物目录
TARGET_DIR="${PROJECT_ROOT}/target"

# 可执行文件名 (取自 Cargo.toml 的 [package].name)
BINARY_NAME="cmdsift"

# ========================================================
# 颜色和日志标识
# ========================================================
step() {
    echo -e "\e[96m➤  $@\e[0m"
}

warning(){
    echo -n "⚠️  "
    echo -e "\e[33m$@\e[0m"
}

error() {
    echo -n "❌ "
    echo -e "\e[31m$@\e[0m"
}

success() {
    echo -n "✅ "
    echo -e "\e[32m$@\e[0m"
}

info() {
    echo -ne "\e[32mℹ️ [INFO]\e[0m"
    echo -e "\e[0m$@\e[0m"
}

# sudo 密码配置
SUDO_PASSWORD="000000"

# 带命令回显的执行函数
# 回显和错误信息输出到 stderr, 不干扰管道和重定向
# 支持 sudo 自动提权: 当首个参数为 sudo 时, 自动判断 root 权限并处理
#
# 注意: 不要将管道输入接到 execute 上（如 echo data | execute sudo tee file），
#       因为 execute 内部通过 echo password | sudo -S 传递密码，如果外部也通过管道传入数据，
#       execute 的 stdin 会被外层管道占据，导致两种问题:
#         1. 若用 (echo password; cat) 转发 stdin，非管道调用时 cat 会因等待终端输入而永久阻塞
#         2. 若用 echo password | sudo -S，外部管道的数据无法传给实际命令（如 tee 写入的是密码而非数据）
#       正确做法: 在调用侧避免管道进 execute，改用临时文件中转或 sudo bash -c "echo > file"
execute() {
    printf '\e[95m[CMD] %s\e[0m\n' "$*" >&2

    if [ "$1" = "sudo" ]; then
        shift
        if [ "$(id -u)" -eq 0 ]; then
            printf '\e[33m[SUDO] Already root, skip sudo\e[0m\n' >&2
            "$@"
        else
            printf '\e[33m[SUDO] Auto elevating privileges\e[0m\n' >&2
            echo "$SUDO_PASSWORD" | sudo -S "$@" 2>&1
        fi
    else
        "$@"
    fi
    local ret=$?
    if [ $ret -ne 0 ]; then
        printf '\e[31m❌ Command failed (exit code: %d): %s\e[0m\n' "$ret" "$*" >&2
        return $ret
    fi
    return 0
}

# 目录切换函数定义
cdi() {
    if command -v pushd &>/dev/null; then
        # 压栈并切换
        pushd $1 >/dev/null || return 1
    else
        cd $1
    fi
}

cdo() {
    if command -v popd &>/dev/null; then
        # 弹出并恢复
        popd >/dev/null || return 1
    else
        cd -
    fi
}

# ========================================================
# 检查 cargo 是否安装
check_cargo() {
    if ! command -v cargo &>/dev/null; then
        error "cargo not found! Please install Rust toolchain first."
        error "  install via: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        return 1
    fi
    return 0
}

# ========================================================
# 编译项目
do_build() {
    step "building project..."

    check_cargo || return 1

    cdi "${PROJECT_ROOT}" || return 1

    # 执行 release 编译 (生产环境推荐)
    execute cargo build --release
    local ret=$?

    cdo

    if [ ${ret} -eq 0 ]; then
        local bin="${TARGET_DIR}/release/${BINARY_NAME}"
        if [ -f "${bin}" ]; then
            success "build success: ${bin}"
        else
            success "build success."
        fi
    else
        error "build failed!"
        return 1
    fi

    return 0
}

# ========================================================
# 运行项目
do_run() {
    step "running project..."

    check_cargo || return 1

    cdi "${PROJECT_ROOT}" || return 1

    local bin="${TARGET_DIR}/release/${BINARY_NAME}"

    # 若 release 产物不存在, 则先编译
    if [ ! -f "${bin}" ]; then
        warning "release binary not found, building first..."
        execute cargo build --release
        local ret=$?
        if [ ${ret} -ne 0 ]; then
            error "build failed, cannot run."
            cdo
            return 1
        fi
    fi

    # 直接运行编译后的可执行文件
    execute "${bin}"
    local ret=$?

    cdo

    if [ ${ret} -eq 0 ]; then
        success "run finished."
    else
        warning "program exited with code: ${ret}"
    fi

    return ${ret}
}

# ========================================================
# 清理构建产物
do_clean() {
    step "cleaning build artifacts..."

    check_cargo || return 1

    cdi "${PROJECT_ROOT}" || return 1

    execute cargo clean
    local ret=$?

    cdo

    if [ ${ret} -eq 0 ]; then
        success "clean done. (${TARGET_DIR} removed)"
    else
        error "clean failed!"
        return 1
    fi

    return 0
}

# ========================================================
# 显示帮助信息
do_help() {
    cat <<EOF
=================================================
           cmdsift build script
=================================================
Usage: ${SCRIPT_NAME} [option]

Options:
  -b    Build project (release mode)
  -r    Run project (build first if needed)
  -c    Clean build artifacts (cargo clean)
  -h    Show this help message

Examples:
  ${SCRIPT_NAME} -b        # Build the project
  ${SCRIPT_NAME} -r        # Run the project
  ${SCRIPT_NAME} -c        # Clean build artifacts
  ${SCRIPT_NAME} -b -r     # Build and run
=================================================
EOF
}

# ========================================================
# 参数解析
ACTION_BUILD=0
ACTION_RUN=0
ACTION_CLEAN=0

while getopts "brch" opt; do
    case ${opt} in
        b)
            ACTION_BUILD=1
            ;;
        r)
            ACTION_RUN=1
            ;;
        c)
            ACTION_CLEAN=1
            ;;
        h)
            do_help
            exit 0
            ;;
        ?)
            error "unknown option: -${OPTARG}"
            do_help
            exit 1
            ;;
    esac
done

# ========================================================
# 打印菜单
do_echo_menu() {
    echo "================================================="
    echo -e "           cmdsift build script"
    echo "================================================="
    echo -e "PROJECT_ROOT        :${PROJECT_ROOT}"
    echo -e "BINARY_NAME         :${BINARY_NAME}"
    echo -e "TARGET_DIR          :${TARGET_DIR}"
    echo -e "SCRIPT_ABSOLUTE_PATH:${SCRIPT_ABSOLUTE_PATH}"
    echo -e "SHELL_PARAM         :($# total)arg=$*"
    echo ""
    echo "================================================="
}

# ========================================================
# 主流程
main() {
    do_echo_menu "$@"

    # 无参数时显示帮助
    if [ $# -eq 0 ]; then
        do_help
        exit 0
    fi

    # 清理操作优先执行 (清理后通常不需要再编译)
    if [ ${ACTION_CLEAN} -eq 1 ]; then
        do_clean || exit 1
        # 若同时指定了 clean 和其他操作, 清理后不再继续
        if [ ${ACTION_BUILD} -eq 0 ] && [ ${ACTION_RUN} -eq 0 ]; then
            exit 0
        fi
    fi

    # 编译
    if [ ${ACTION_BUILD} -eq 1 ]; then
        do_build || exit 1
    fi

    # 运行
    if [ ${ACTION_RUN} -eq 1 ]; then
        do_run || exit 1
    fi

    exit 0
}

main "$@"
