## 一、 项目介绍

### 1. 概述

cmdsift 是一个编译命令执行与结果采集工具，使用 Rust 编写。它将"发送命令 + 轮询等待 + 检测完成 + 分类输出"封装为一次 CLI 调用：在本地执行编译命令并等待完成，自动分类采集输出中的错误、警告和常规信息，以结构化格式打印，便于人或 AI 进行后续分析。

### 2. 核心能力

- 执行任意编译命令（如 `make -j8`、`./build.sh`），支持指定工作目录
- 自动剥离 ANSI 转义序列与控制字符，避免终端显示错乱与分类正则失效
- 按正则规则将输出行分类为 error / warning / info，输出编号列表与统计摘要
- 支持超时控制与轮询间隔配置，超时自动杀死子进程
- 完整编译日志自动落盘，日志每一行都带 `[YYYY-MM-DD HH:MM:SS] ` 时间戳前缀，便于时序分析

### 3. 用法

```text
cmdsift [选项] <命令>
cmdsift [选项] -- <命令> [参数...]
```

常用选项：

- `-C, --cwd <目录>`：切换到该目录后再执行编译命令
- `-m, --max-wait <毫秒>`：最大等待时间（默认 600000，即 10 分钟）
- `-p, --poll-interval <毫秒>`：轮询间隔（默认 2000）
- `--no-classify`：关闭输出分类，仅返回输出尾部 50 行
- `-h, --help`：显示帮助
- `-V, --version`：显示版本

示例：

```bash
cmdsift 'make -j8'
cmdsift -C /srv/kernel -m 1800000 './build.sh alpha -a -c'
cmdsift --no-classify -- make -j8
```

### 日志文件

- 默认写入**待编译项目所在目录**的 `log/` 下 `<YYYYMMDD_HHMMSS>.log`（文件名时间戳为编译开始时刻）：
  - 使用 `-C <目录>` 时，写入 `<目录>/log/`，与项目目录保持一致
  - 未使用 `-C` 时，写入**当前目录**的 `log/`
  - 可用 `-L, --log-file <目录>` 指定目录（优先于上述默认规则），`--no-log-file` 关闭
- 编译过程中边采集边写入（约每 2 秒批量落盘一次），记录完整编译输出
- 日志每一行都带有 `[YYYY-MM-DD HH:MM:SS] ` 时间戳前缀（取该行落盘时刻），便于分析各阶段耗时；stdout 上的结构化摘要统计（状态/错误/警告列表）不带时间戳

### 4. 退出码

- 编译命令的退出码（透传，便于脚本串联）
- `124`：超时
- `2`：参数错误
- `126`：命令启动失败

## 二、 构建与版本管理

### 1. 编译

项目提供两种编译方式：直接使用 `cargo` 命令，或使用项目自带的 `build.sh` 脚本。

#### 1.1 使用 cargo 编译

```bash
cargo build --release
```

编译产物位于 `target/release/cmdsift`。

#### 1.2 使用 build.sh 编译

```bash
./build.sh -b
```

`build.sh` 支持以下参数：

- `-b`：编译项目（release 模式）
- `-r`：运行项目（不存在则先编译）
- `-c`：清理构建产物
- `-h`：显示帮助

组合示例：

```bash
./build.sh -b -r   # 编译并运行
```

### 2. 清理构建产物

#### 2.1 使用 cargo 清理

```bash
cargo clean
```

该命令会移除整个 `target/` 目录。

#### 2.2 使用 build.sh 清理

```bash
./build.sh -c
```

### 3. 更新版本号

版本号集中维护在 `Cargo.toml` 的 `[package].version` 字段，源码通过 `env!("CARGO_PKG_VERSION")` 宏在编译期动态读取，无需修改源文件。

#### 3.1 修改版本号

编辑 `Cargo.toml`，将 `version` 改为目标值：

```toml
[package]
name = "cmdsift"
version = "1.0.0"
edition = "2024"
```

#### 3.2 同步 Cargo.lock

修改 `Cargo.toml` 后，`Cargo.lock` 不会自动更新，需手动刷新本包的锁版本：

```bash
cargo update -p cmdsift
```

`-p cmdsift` 仅更新 cmdsift 自身的锁版本，不会连带升级其他依赖，避免引入不必要的依赖版本变动。

#### 3.3 验证版本号

重新编译后，通过 `-V` 选项验证：

```bash
cargo build --release
./target/release/cmdsift -V
# 输出: cmdsift 1.0.0
```

---
*本文档由 markdowncli 技能辅助生成*
