# cmdsift 测试工程（C 语言）

故意保留若干 error / warning 的 C 工程，用于验证 cmdsift 对编译输出的分类采集能力。

## 预期的错误与警告

实测（gcc 11.4.0 + make 4.3，`-Wall -Wextra -std=c99`）：

| 文件 | 类型 | 触发的诊断 |
|------|------|-----------|
| `src/core/calc.c` | error | `'nonexistent_symbol' undeclared`（编译阶段） |
| `src/core/calc.c` | warning | `unused variable 'result'`（-Wunused-variable） |
| `src/core/calc.c` | warning | `control reaches end of non-void function`（-Wreturn-type） |
| `src/core/utils.c` | warning | `implicit declaration of function 'printf'`（-Wimplicit-function-declaration） |
| `src/core/utils.c` | warning | `incompatible implicit declaration of built-in function 'printf'`（-Wbuiltin-declaration-mismatch） |
| `src/main.c` | error | `passing argument 2 of 'calc_add' makes integer from pointer`（-Wint-conversion，归为 error） |
| `src/main.c` | warning | `unused variable 'unused_value'`（-Wunused-variable） |

> 注：`calc.c` 的编译错误会使 make 在 `Error 1` 中止，链接阶段不会执行；
> cmdsift 实测分类约 **2 errors + 6 warnings**。

## 在容器中编译

本机无 gcc/make，需在 Docker 容器中编译（挂载点 `/workspace` 对应宿主 `test/c-project` 的上级）：

```bash
# 进入容器交互式 shell（在容器内执行）
cd /workspace/test/c-project && make

# 或直接用 cmdsift 采集分类结果（-C 指定工作目录）
cmdsift -C /workspace/test/c-project 'make'
```

