# docs 文档索引

仓库文档按**职能**分类存放。每篇文档头带状态标记；读前先确认其状态与你的目的匹配。

> 状态图例：`[active]` 现行维护 · `[progress]` 进行中（含未完成待办）·
> `[archive]` 已完成/已过时/纯历史（ARCHIVED 头，仅供参考，代码为准）。
> 分类说明与文档地图的 Agent 版见仓库根 `CLAUDE.md` → Documentation Map。

## reference/ — 现行规范与设计参考（读代码前先看）

| 文件 | 内容 | 状态 |
| --- | --- | --- |
| `reference/isa-dsl.md` | ISA-DSL **v15** 语法规范（唯一 DSL 语法，逐项对代码核对） | active |
| `reference/aarch64-encoding-ref.md` | A64 整数核心指令编码参考（isa 表/golden 测试依据，尾节含 arm64_v12 实现状态） | active |
| `reference/imm_str.md` | ImmStr 不可变字符串类型设计（已实现，forge-ir 代码注释引用） | active |

## forge-ir/ — forge-ir 工作流（活跃）

| 文件 | 内容 | 状态 |
| --- | --- | --- |
| `forge-ir/README.md` | 组说明：历轮计划已归档、如何继续迭代 | active |
| `forge-ir/backlog.md` | 仍未关闭待办速览（逐项标出处归档档） | active |
| `archive/forge-ir/` | 历轮计划/执行/审计记录全集（roadmap/checklist/audit 等 7 篇，均 ARCHIVED） | archive |

## plans/ — 专项改进方案（有未完成工作）

| 文件 | 内容 | 状态 |
| --- | --- | --- |
| `plans/forge-rustc-vec_push-plan.md` | forge-rustc vec 族方案（历史修复链 + **§8 2026-09-10 复核**：E1 假设未复现、失败面已 fail-closed；5 用例仍 FLAKY，见 `tests/e2e.rs`；§9 修复方案） | progress |

## performance/ — 基准与优化

| 文件 | 内容 |
| --- | --- |
| `performance/BENCHMARKS.md` | 基准运行/框架（原仓库根） |
| `performance/OPTIMIZATION.md` | 编译管线热点分析优化清单（原仓库根） |
| `performance/bench_baseline.md` | criterion 基准基线 + 历轮优化实测（含少量记录节） |
| `performance/codegen_stage_profile.md` | codegen stage 占比表（被 BENCHMARKS/bench_baseline 引用） |

## guides/ — 工具与方法

| 文件 | 内容 |
| --- | --- |
| `guides/coverage.md` | cargo-llvm-cov 覆盖率工作流（CI Coverage job 现行方法） |
| `guides/lint.md` | markdownlint 检查命令/豁免形式/存量基线（写文档后自查） |

## archive/ — 历史归档（⚠️ 先看 ARCHIVED 头）

| 文件 | 内容（归档原因） |
| --- | --- |
| `archive/roadmap-status.md` | v14-forge-ir-redesign 路线图交接（2026-08；自述 2026-09 大幅过期，开放项移交 forge-rustc WORKAROUNDS/README） |
| `archive/ymm-abi-plan.md` | YMM ABI（>128 位向量传参）方案（**2026-09-10 核查后归档**：S1-S5/D2-D6 全部落地；残余移交 WORKAROUNDS WA-37 与 CLAUDE.md SIMD 矩阵） |
| `archive/hir-shrink-plan.md` | HIR/mini_c 收缩方案（**2026-09-10 归档**：点 1/2/3 落地、点 5 前提不成立而删除、点 4 决定不做；收益结论下调） |
| `archive/isa-dsl-v12-roadmap.md` | ISA-DSL v12 历史（现行规范 = reference/isa-dsl v15） |
| `archive/asm-dec-generic-design-v2.md` | 汇编器/解码器 v2 设计提案（v13 已落地，v15 演进） |
| `archive/clippy-fixes.md` | 2026-07 clippy 清零单次记录 |
| `archive/coverage-history.md` | Windows 本地覆盖率接入诊断 + 审查驱动补测交付（2026-07/08） |
| `archive/forge-ir/` | forge-ir 历轮计划/backlog/执行/审计记录 7 篇（452 收敛基线，2026-08 停更） |
| `archive/forge-ir/README.md` | 归档组说明与阅读指引 |

## 根目录与 crate 文档（不在 docs/ 下）

- `README.md` — 项目主页（架构总览）
- `CLAUDE.md` — Agent/协作者约定（含本文档地图）
- `CHANGELOG.md` — 更新日志（Keep a Changelog）
- `crates/*/README.md`、`crates/tools/forge-rustc/WORKAROUNDS.md` — 随 crate 的源码级文档
