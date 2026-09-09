# 覆盖率工作流（cargo-llvm-cov）

目标：用 [cargo-llvm-cov](https://crates.io/crates/cargo-llvm-cov)（LLVM 源码级覆盖率，
`-C instrument-coverage`）量化测试盲区，针对低覆盖模块补测试。

> ⚠️ 历史背景：本机（Windows x86_64-msvc）2026-07 曾因 LLVM binary-id 关联失败而
> 无法产出 workspace 覆盖率——完整诊断与本地替代方案见
> [`docs/archive/coverage-history.md`](../archive/coverage-history.md)。
> **现行覆盖率在 CI（Linux）跑**：`.github/workflows/ci.yml` 的 Coverage job 使用
> `cargo llvm-cov` + `llvm-tools-preview`，范围 = workspace 纯库（exclude
> forge-rustc/forge-codegen/forge-tests——执行面由 test jobs 覆盖）。

## 工作流

```bash
# 安装
cargo +stable install cargo-llvm-cov --locked

# 全量：跑测试 + 输出摘要
cargo llvm-cov --workspace --exclude forge-rustc

# HTML 报告（target/llvm-cov/html/index.html）
cargo llvm-cov --workspace --exclude forge-rustc --html

# CI 门禁示例（行覆盖 < 80% 时退出码 1）
cargo llvm-cov --workspace --exclude forge-rustc --fail-under-lines 80

# 仅测试不报告（多次合并）
cargo llvm-cov clean --workspace
cargo llvm-cov --workspace --exclude forge-rustc --no-report
cargo llvm-cov report --html
```

注意：首次运行会用 `-C instrument-coverage` 全量重编译（10-20 分钟）；
默认忽略 `tests/` 目录与 `*_tests.rs`；`cfg(coverage)` 可用于排除工具代码。
