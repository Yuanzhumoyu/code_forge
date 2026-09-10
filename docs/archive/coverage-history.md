# 覆盖率接入诊断与补测记录（2026-07/08 历史）

## ⚠️ ARCHIVED（2026-09）

> 本文记录 2026-07-31 在本机（Windows msvc）接入 cargo-llvm-cov 的失败诊断与
> 改用**审查驱动补测**的交付记录。Windows 本地覆盖率路径已定论不可用；
> 现行覆盖率在 CI（Linux）用 llvm-cov 运行（见 `docs/guides/coverage.md`）。
> 本文为历史记录，仅供参考；代码现状以仓库代码与现行文档为准，不再维护。

## cargo-llvm-cov 接入与工具链诊断（2026-07-31，两次尝试）

**结论：本环境（Windows x86_64-msvc + rustc 1.95/1.96 nightly/stable）无法产出
workspace 覆盖率报告**——即使重装 `llvm-tools-preview` 组件后复现，测试仍可正常运行。

### 重装组件后的复试验证（第二次尝试，2026-07-31）

用户重装 `llvm-tools-preview` 后全面复测，**问题全部复现**：

| 方案 | 结果 |
| --- | --- |
| `cargo llvm-cov` 默认 wrapper（组件检查通过） | 测试全跑，报告 files 仍仅 2（bitflags 依赖 11%），**workspace crate 0 个** |
| 手动 RUSTFLAGS（nightly 1.96 + LLVM 22.1.0 匹配） | `llvm-profdata show` 确认 profdata 有真实计数（Total count 50843713），但 `llvm-cov report`/`export` 全部 segment 计数 0（binary-id 关联失败） |
| 旧 nightly 2026-01-22（rustc 1.95 + LLVM 21.1.8） | 同样 0% |
| `--no-rustc-wrapper` + 手动 `LLVM_PROFILE_FILE` | 测试跑但不写 profraw（0 个文件） |

**组件重装确认有效**（rustup 组件检查通过、llvm-profdata/llvm-cov 可执行、LLVM 版本与
rustc 完全匹配 22.1.0/22.1.2），**但 coverage 数据关联仍失败**。

**根因**（wrapper 路径）：`wrapper.rs` 按 `--crate-name` 匹配 workspace crate 名单决定
是否加 `-C instrument-coverage`，本环境匹配失败（workspace crate 未被 instrument，
报告里只有依赖 bitflags 的残余计数）。
**根因**（手动路径）：rustc 1.87+ 默认启用 binary-id（`llvm-profdata show --binary-ids`
为空、`llvm-cov export` 全部 segment 0），profraw 的 counter 无法关联到 PE 二进制的
instrumentation——Windows 上 LLVM binary-id 的已知薄弱区。

### 环境

- `cargo-llvm-cov 0.8.7`（`cargo +stable install cargo-llvm-cov --locked`）
- nightly + `llvm-tools-preview` 组件已装
- rustup 自动安装组件失败（`component download failed`）→ 用环境变量指定路径绕过：

```bash
export LLVM_COV="$RUSTUP_HOME/toolchains/nightly-x86_64-pc-windows-msvc/lib/rustlib/x86_64-pc-windows-msvc/bin/llvm-cov.exe"
export LLVM_PROFDATA=".../llvm-profdata.exe"
```

### 验证过的四种方案（全部失败）

| 方案 | 结果 |
| --- | --- |
| `cargo llvm-cov --workspace`（默认 RUSTC_WRAPPER） | 测试全跑，但报告 `files` 仅 2（bitflags 依赖），workspace crate 不进报告 |
| `cargo llvm-cov --no-rustc-wrapper` | `files: 0`（Windows 下 LLVM_PROFILE_FILE 未传递） |
| 手动 `RUSTFLAGS="-C instrument-coverage"` + `LLVM_PROFILE_FILE` + `llvm-profdata merge` + `llvm-cov report`（nightly） | profraw 3.2MB 有数据、merge 成功，但 `report`/`show` 全部 0% 计数 |
| 同上（stable toolchain） | 同样 0%（TOTAL 87134 行全 missed） |

**根因**：instrument 二进制的 counter 与 profraw 记录错位（llvm-cov 读出的行计数全 0）。
README 声称 `x86_64-pc-windows-msvc` 受支持，但 rustc 1.96 时代实际不工作（疑似
`-Z coverage-options` 的 binary-id 在 Windows 的问题）。详见 rust-lang/rust 的
coverage 相关 issue。

### 后续建议

- 跟踪 rust-lang/rust 的 Windows coverage/binary-id 修复（`x86_64-pc-windows-msvc`
  在 rustc 1.87+ 的 coverage binary-id 回归）；修复后可按 docs/guides/coverage.md 重试
- 若需覆盖率，可在 Linux/macOS CI 上运行 `cargo llvm-cov`（该环境 LLVM coverage 稳定）
- 本地开发用**审查驱动补测**（见下文）

## 审查驱动补测试（2026-07-31，本环境实际交付）

覆盖率数值不可用后，改为**代码审查驱动**补测，针对 forge-ir 的薄弱路径：

| 测试 | 覆盖路径 | 发现的 bug |
| --- | --- | --- |
| `roundtrip_stack_addr` | forge 扩展 `stack_addr` 的 parse↔display round-trip | **display 丢失 offset 立即数**（输出 `stack_addr` 空）→ 已修 |
| `roundtrip_copy` | 扩展 `copy` 指令 round-trip | — |
| `roundtrip_call_indirect` | `call i32 %ptr(...)`（CallIndirect）round-trip | **display 丢失函数指针**（输出 `call i32(...)`）→ 已修 |
| `roundtrip_fconst_inline` | 浮点常量内联（`fadd double 1.5, double 2.5`） | — |
| `errors_undefined_block` | `br label %missing` 语义错误 | — |
| `errors_undefined_function` | `call i32 @missing(...)` 语义错误 | — |
| `test_is_default` | `DataLayout::is_default()` 新方法 | — |

**display.rs 两个 round-trip 真 bug 修复**（round-trip helper 此前不比较
immediates，故未暴露）：

1. `CallIndirect`：`call <retty> %ptr(<args>)`——callee 是 operands[0]，原实现只处理
   `Immediate::Func`（Call 直调），间接调用缺 `%ptr`
2. `StackAddr`/`GlobalAddr`/`Alloca`：立即数（offset/大小）未输出——现按
   `Immediate::Int/Uint/Const` 输出（如 `stack_addr -4`）

**验证**（2026-08-08 实测）：forge-ir 239 lib + 89 display_llvm + 24 ir_parser_llvm 全过；
`cargo test --workspace --exclude forge-rustc` 48 组 ok、0 FAILED。
