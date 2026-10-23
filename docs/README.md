# docs 文档索引

仓库文档按**职能**分类存放。每篇文档头带状态标记；读前先确认其状态与你的目的匹配。

> 状态图例：`[active]` 现行维护 · `[progress]` 进行中（含未完成待办）·
> `[archive]` 已完成/已过时/纯历史（ARCHIVED 头，仅供参考，代码为准）。
> 分类说明与文档地图的 Agent 版见仓库根 `CLAUDE.md` → Documentation Map。

## reference/ — 现行规范与设计参考（读代码前先看）

| 文件 | 内容 | 状态 |
| --- | --- | --- |
| `reference/isa-dsl.md` | **ISA-DSL v22 语法规范（现行唯一语法）**：操作数声明与字段绑定就地写在 `asm` 里（`{名字:槽[:角色]}` / `{@字段:槽[:角色]}`）、键速查表（机器校验）、逐节语法、生成代码契约、`forge-isa` CLI、生成期自测 | active |
| `reference/isa-dsl-errors.md` | ISA-DSL 错误码目录（诊断格式、节级错误码、常见修法；含多文件组合与 `parts` 的加载期/编译期错误） | active |
| `reference/aarch64-encoding-ref.md` | A64 整数核心指令编码参考（isa 表/golden 测试依据，尾节含 arm64 实现状态） | active |
| `reference/imm_str.md` | ImmStr 不可变字符串类型设计（已实现，forge-ir 代码注释引用） | active |
| `reference/calling-conventions.md` | **调用约定层（`forge-abi`，v20 A1/A2）**：三层数据（规则/绑定/计划）+ 通用引擎、IR 侧 `CallConvId`（`Builtin`/`Named`/`Index`，未注册即 fail-closed）、内置四份约定与参考绑定、`AbiTarget` 能力视图、`AbiHooks`、`forge-isa abi list\|check\|plan`、已知缺口与"加自己的约定" | active |
| `reference/binary-format.md` | forge-ir 二进制格式（IR bitcode **v2**：段体压缩 + 段表 `raw_len`）规范：容器/段/原语/各段细节/压缩/确定性/版本策略/验证基线/消费者/已知限制 | active |
| `reference/backend-pipeline.md` | **后端编译管线契约**（**v24 现行**）：编译入口（`CompileSession`/`CompiledModule`/`JitLoader`）、**管线阶段表**（由守卫与 `pipeline::STAGES` 集合相等钉住）、**合法化两相**（计划只读 + 物化单遍）与 pass 合并次序、聚合/变参表示契约、**值描述单一来源**、能力缺口与观测开关的台账指针 | active |
| `reference/backend-gaps.md` | **后端能力缺口台账**（v24 M3 建）：逐行给出"谁在什么条件下撞到 / 决策（已实现·数据层·不做·待做）/ 钉住它的守卫或数据条件"（条数以台账本身为准）；失败信息带 `[cap:ID]`，守卫 `capability_gaps_guard` 与源码缺口集合**双向逐一相等** | active |
| `reference/ir-int-semantics.md` | **IR 整数/浮点位宽语义（v27 M5 建）**：整数是"无符号位图 + 任意位宽"（符号性由 opcode 表达）、逐运算语义（含移位量 ≥ 位宽的定值行为）、常量按位宽规范化、浮点按 **(阶码, 尾数)** 建模（符号恒 1 位，覆盖 f16/bf16/f8 变体/自定义格式）、**补位规则 R1–R4**（精确 → 就近升位 → 分字展开 → 死规则）与规模公式 | active |
| `reference/constants.md` | **常量层（v28 建）**：`BitValue`（整数与浮点合并为位模式 + 位宽，种类由使用点决定）、`ConstId` 四元枚举（`Bits/Big/Vector/Agg`，tag 铺满 ⇒ `raw`/`from_raw` 全域）、`ConstantPool` 四条 interner 通道（无预置槽、端序进键）、读取视图与 `as_i64`/`as_big` 的**原位宽补码**语义、`AsConst` 常量糖、失败模式与回归网 | active |

## plans/ — 进行中的专项方案（活账本）

| 文件 | 内容 | 状态 |
| --- | --- | --- |
| [`plans/forge-rustc-v27-plan.md`](plans/forge-rustc-v27-plan.md) | **forge-rustc v27：完整 rustc 后端**（M1 正确性清零 / M2 `global_asm`+`asm!`+unwind+TLS / M3 `std` 可用性 / M4 第二目标 + 对象层 / M5 契约与门禁）；含立项实测证据 E1–E11 | progress |
| —（已归档） | 项目 v26 的账本已收口：见 [`archive/project-v26-plan.md`](archive/project-v26-plan.md)（M1–M5 全部落地；现行入口 = `reference/backend-pipeline.md` + `reference/backend-gaps.md`） | archive |
| ~~`plans/project-v26-plan.md`~~（原文，已移走） | **项目 v26：审查收口**（M1 诊断单一来源：DSL 错误码台账 + 双向守卫、事件 sink 下沉 `forge-isa-runtime::diagnostics`、开关台账归并 `forge-rustc`；M2 无消费者面清理：删 `forge-mem`/`isel_strategy`（`IR_FORMAT_VERSION` 3→4）/`asm_v11.lx`，前端线降级为 `examples/hir-demo`；M3 `cargo forge gate` 单命令门禁 + CI 补齐；M4 `[cap:SPEC_*]` 谱侧台账与能力补齐；M5 契约回写）；含审查证据、删除清单、守卫与判据 | progress |
| —（已归档） | 后端 v25 的账本已收口：见 [`archive/backend-v25-plan.md`](archive/backend-v25-plan.md)（M1–M5 全部落地；现行入口 = `reference/backend-pipeline.md`） | archive |
| —（已归档） | 后端 v24 的账本已收口：见 [`archive/backend-v24-plan.md`](archive/backend-v24-plan.md)（M1–M5 全部落地；现行入口 = `reference/backend-pipeline.md` + `reference/backend-gaps.md`） | archive |
| —（已归档） | 后端 v23 的账本已收口：见 [`archive/backend-v23-plan.md`](archive/backend-v23-plan.md) | archive |

## forge-ir/ — forge-ir 工作流（活跃）

| 文件 | 内容 | 状态 |
| --- | --- | --- |
| `forge-ir/README.md` | 组说明：历轮计划已归档、如何继续迭代 | active |
| `forge-ir/backlog.md` | 仍未关闭待办速览（逐项标出处归档档） | active |
| `archive/forge-ir/` | 历轮计划/执行/审计记录全集（roadmap/checklist/audit 等 7 篇，均 ARCHIVED） | archive |

## performance/ — 基准与优化

| 文件 | 内容 |
| --- | --- |
| `performance/BENCHMARKS.md` | 基准运行/框架（原仓库根） |
| `performance/OPTIMIZATION.md` | 编译管线热点分析优化清单（原仓库根） |
| `performance/bench_baseline.md` | criterion 基准基线 + 历轮优化实测（含少量记录节） |
| `performance/codegen_stage_profile.md` | codegen stage 占比表（被 BENCHMARKS/bench_baseline 引用） |
| `performance/generated_compile_profile.md` | 生成物编译画像（**2026-09-24**：分段体积 + 编译时间归属 + v19 V7「表化」的度量结论与重启触发条件） |

## guides/ — 工具与方法

| 文件 | 内容 |
| --- | --- |
| `guides/isa-dsl-tutorial.md` | ISA-DSL 教程：30 分钟接入一个小 ISA（骨架 → 操作数槽 → 指令 → 模板 → 生成期自测 → CLI 自查） |
| `guides/coverage.md` | cargo-llvm-cov 覆盖率工作流（CI Coverage job 现行方法） |
| `guides/lint.md` | markdownlint 检查命令/豁免形式/存量基线（写文档后自查） |
| `guides/testing-notes.md` | **测试与夹具笔记**（v26 M5 从 `CLAUDE.md` 搬出的约 300 行：JIT 矩阵/汇编三档/生成物落盘/谱内向量/工具链 lint 档/宽度元数据/…） |

## archive/ — 历史归档（⚠️ 先看 ARCHIVED 头）

| 文件 | 内容（归档原因） |
| --- | --- |
| `archive/calling-convention-redesign-plan.md` | 调用约定重设计 v20 A1–A8（**2026-10-08 归档**：A1–A8 全部落地 —— A7 异域钩子 = `forge-abi/tests/exotic_hooks.rs`、A8 文档一致性收口；现行参考 = `reference/calling-conventions.md`） |
| `archive/forge-rustc-v27-milestones.md` | forge-rustc v27 的**逐轮记录与旧施工图**（**2026-10-19 归档**：原活账本 3200+ 行，状态与历史混在一起 —— 状态留 `plans/forge-rustc-v27-plan.md`、决策留 `reference/backend-gaps.md`；本文只作历史证据） |
| `archive/forge-rustc-vec_push-plan.md` | forge-rustc vec 族方案（**2026-10-11 归档**：§4 验收 4/4 达成 —— `e2e.rs` 103 条用例全部 `known_failure: false`、CI job `forge-rustc (e2e, Windows)` 绿；现行入口 = `tests/e2e.rs` 用例表 + 该 CI job） |
| `archive/forge-ir-v3-plan.md` | forge-ir v3 改进方案（**2026-10-08 归档**：S1–S8 **全部收口** —— S3 三余项已落地、**S7 余项清零**、S8 经拍板只做二进制序列化且已落地；未关闭项在 `forge-ir/backlog.md`） |
| `archive/isa-dsl-variant-field-values.md` | 按变体给字段值（**2026-10-08 归档**：工作**早已落地** —— 键名是 `fields_variant`、`rev8` 正在用；本文留设计取舍与教训） |
| `archive/varargs-plan.md` | 变参（varargs）方案（**2026-10-08 归档**：功能面**已完成** —— V0–V5 落地、SysV `%al` 写入 V7 已做、四份 `va_list` 形状齐备且由守卫钉住；前端产出变参签名是明写的**非目标**） |
| `archive/isa-dsl-v21-redesign-plan.md` | ISA-DSL v21 重设计方案（**2026-10-08 归档**：v21 已收口，W1–W11 + W6 + W6+ 全部落地；现行规范 = `reference/isa-dsl.md`） |
| `archive/isa-dsl-v21-execution-checklist.md` | ISA-DSL v21 执行清单（**2026-10-11 归档**：W1–W11 + W6+ 与 §14 最终验收全部完成；只作历史证据，现行语法 = `reference/isa-dsl.md`） |
| `archive/forge-isa-dsl-v19-plan.md` | forge-isa-dsl v19 改进方案（**2026-10-08 归档**：可独立接入、谱内向量、lint、参数化变体 四块招牌切片按代码事实核对落地；现行面 = `reference/isa-dsl.md` + `CLAUDE.md` 速查） |
| `archive/forge-ir-binary-serialization-plan.md` | forge-ir 二进制序列化执行方案 B1–B5（**2026-10-08 归档**：`to_binary`/`from_binary`/`IR_FORMAT_VERSION`/`tests/binary_module.rs` 均在；现行规范 = `reference/binary-format.md`） |
| `archive/forge-ir-s8-design.md` | forge-ir S8 可选项设计（**2026-10-08 归档**：三个候选里**只做二进制序列化**，MemorySSA-lite 与 crate 拆分经拍板不做） |
| `archive/roadmap-status.md` | v14-forge-ir-redesign 路线图交接（2026-08；自述 2026-09 大幅过期，开放项移交 forge-rustc WORKAROUNDS/README） |
| `archive/ymm-abi-plan.md` | YMM ABI（>128 位向量传参）方案（**2026-09-10 核查后归档**：S1-S5/D2-D6 全部落地；残余移交 WORKAROUNDS WA-37 与 CLAUDE.md SIMD 矩阵） |
| `archive/hir-shrink-plan.md` | HIR/mini_c 收缩方案（**2026-09-10 归档**：点 1/2/3 落地、点 5 前提不成立而删除、点 4 决定不做；收益结论下调） |
| `archive/isa-dsl-v12-v17.md` | ISA-DSL v12–v17 语法史 + v18 删除/改名总表（**2026-09-21 归档**：现行规范 = `reference/isa-dsl.md` v18） |
| `archive/isa-dsl-v12-roadmap.md` | ISA-DSL v12 历史（现行规范 = `reference/isa-dsl.md` v18） |
| `archive/forge-dsl-v18-plan.md` | forge-dsl v18 执行方案（**2026-09-23 归档**：S0–S10 全部落地或经度量判定不做；S0 立项基线、S8/S9/S10 的实测数字仍是 `performance/bench_baseline.md` 与 CHANGELOG 的证据出处；现行方案 = `archive/forge-isa-dsl-v19-plan.md`（该方案亦已归档，现行面 = `reference/isa-dsl.md`）） |
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
