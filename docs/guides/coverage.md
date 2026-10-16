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

## 覆盖率下限棘轮（v26 M3 起）

CI 的 `coverage` 作业在生成 `lcov.info` 之后跑判定程序，**低于下限即失败**：

```bash
# 判定程序住在仓库里（纯 awk，无 shell 引号依赖）；本地同一条命令可复跑
awk -f docs/performance/coverage_floor.awk docs/performance/coverage_floor.txt lcov.info
```

- 下限写在 [`docs/performance/coverage_floor.txt`](../performance/coverage_floor.txt)（一个数，**只许上调**）；
- 判据：lcov 里全部 `LF:`/`LH:` 求和 ⇒ 行覆盖率 = `LH/LF`；
- **2026-10-11 实测基线**：**81.16%**（LF 59678 / LH 48436；
  `cargo llvm-cov --no-clean --workspace --exclude forge-rustc --exclude forge-codegen
  --exclude forge-tests --lcov`），当轮下限钉 **80**（留 ~1.2pp 跨平台余量）⇒ 下次实测若升高，
  把下限跟着上调（棘轮方向单一）。
- 白名单口径同 `ci.yml` 的「Generate coverage」：**只报纯库**（执行面由 JIT 矩阵 / asm 三档 /
  forge-rustc e2e 覆盖，它们不计入这个数）。

## CI 门禁示例（行覆盖 < 80% 时退出码 1）

```bash
cargo llvm-cov --workspace --exclude forge-rustc --fail-under-lines 80

# 仅测试不报告（多次合并）
cargo llvm-cov clean --workspace
cargo llvm-cov --workspace --exclude forge-rustc --no-report
cargo llvm-cov report --html
```

注意：首次运行会用 `-C instrument-coverage` 全量重编译（10-20 分钟）；
默认忽略 `tests/` 目录与 `*_tests.rs`；`cfg(coverage)` 可用于排除工具代码。
