# 项目 v26 里程碑方案：审查收口（诊断单一来源 · 无消费者面清理 · 单命令门禁 · 能力补齐）

> **状态：[progress]**（2026-10-11 立项）。本文是**活账本**：每个里程碑落地时在 §8 追加批次记录
> （含实测数字、命令、偏差），任何"说得出但没做到"的东西必须如实写在这里。
> 基线 = v25 收口提交 `574eb82c`；**行号随迭代漂移，以符号名为准**。
> 上一轮方案（v25，已收口归档）见 [`docs/archive/backend-v25-plan.md`](../archive/backend-v25-plan.md)；
> 现行契约 [`reference/backend-pipeline.md`](../reference/backend-pipeline.md)、
> 能力缺口台账 [`reference/backend-gaps.md`](../reference/backend-gaps.md)。

前提纪律（沿用既有约定）：

- **一个任务一次提交**（不按批次拆 PR）；**里程碑未完成不提交**，工作累积在工作区。
- 每个里程碑以全量门禁（M3 起 = `cargo forge gate`：fmt / clippy×2 / workspace tests /
  `forge-codegen --lib --all-features` / 守卫清单 / JIT 两矩阵 / asm 三档 / markdownlint）+ 该里程碑
  **新增守卫**为验收；**删除清单逐条清零**才算完成。
- 守卫必须精确（白名单条目**必须被命中**）；预算/棘轮只许按实测**下调**，不许为放过而上调。
- 破坏性更新允许（已批准两项：前端线收缩降级为示例、删 `isel_strategy` 并升 `IR_FORMAT_VERSION`）；
  **C ABI 互操作不可破坏**。

## 1. 立项证据（本次只读审查实测，2026-10-11）

| # | 证据 | 数字 / 位置 |
| --- | --- | --- |
| E1 | **DSL 错误码台账漂移且无守卫** | 源码发出 **29** 个 `DSL-*`/`LINT-*` 码；`docs/reference/isa-dsl-errors.md` 列 **31** 个。源码有而文档无：`LINT-ASM-AMBIG`、`LINT-FORM-FIELD-UNUSED`；文档有而源码无：**`DSL-ABI`**（`[abi]` 节 v20 A5-3 已整节删除）、`DSL-SLOT`。全仓**没有任何守卫**钉这两侧（`rg isa-dsl-errors crates/frontend/forge-isa-dsl/tests` = 0） |
| E2 | **事件出口四份实现** | `forge-codegen/src/pipeline/events.rs`（新，JSON 行 + 台账）、`forge-codegen/src/lib.rs::jit_event`、`forge-tests` 的 asm 事件与 e2e 事件各写一遍 `OpenOptions+append+writeln`；四个 env（`FORGE_CODEGEN_EVENTS`/`FORGE_JIT_EVENTS`/`FORGE_ASM_EVENTS`/`FORGE_E2E_EVENTS`）无统一 schema 文档 |
| E3 | **开关台账两套** | `pipeline::trace::KNOBS`（11 项，有双向守卫）↔ `forge-rustc/src/trace.rs::trace_enabled`（自带名字集合，无守卫）；v24 M4 明确把后者留在范围外 |
| E4 | **`forge-mem` 只有 1 处使用** | 全仓 `forge_mem` 引用 = `forge-codegen/src/runtime/jit.rs:34`（`ExecutableMemory`、`MemError`）+ 一条 Cargo 依赖；其 `CpuFeatures`/`host_cpu.rs` **零外部使用者**（`forge-isa-runtime/src/cpu.rs` 已是现行主机探测实现） |
| E5 | **`isel_strategy` 无消费者** | 引用面：`forge-ir` src 4 文件（含 `ir/isel_strategy.rs` 类型本体）、`forge-ir` tests 3 文件、`forge-isa-runtime/src/prelude.rs` 重导出、`forge-opt` 4 处**纯透传拷贝**（`ipa/{func_specialize,inline,lto}.rs`、`loops/loop_unroll.rs` 的注释）；后端**零读点**（backlog 第 2 条记为"仍需决策"）。`IR_FORMAT_VERSION = 3`（`forge-ir/src/binary/format.rs:26`） |
| E6 | **前端线只服务一个示例** | `forge-grammar`(7110 行 src) 仅被 `forge-hir` 依赖；`forge-hir`(3533)+`forge-hir-macro`(835) 仅被 `examples/mini_c` 使用（`mini_c/src/atoms.rs` 等）；`forge_grammar::`/`forge_hir::` 在 `crates/` 其他位置的引用 = 0；`forge-grammar/src/lib.rs:47` 的"语义层零外部调用"注释**已停更**（那些类型当前不存在）；`grammars/asm_v11.lx` 只被 `forge-grammar` 自己的两个测试引用 |
| E7 | **门禁只能手抄** | 无 `xtask`、无脚本；全套命令只写在 `CLAUDE.md`（fmt / clippy×2 / workspace / lib / 12 条守卫 / 两矩阵 / asm 三档 / markdownlint）。既有 CLI `cargo-forge`（clap，`cmd/{build,init,run,doctor,backend}.rs`）已具备加子命令的形状 |
| E8 | **CI 缺口** | CI 无 markdownlint（命中 0）；`forge-codegen` 的 `parallel` 与根 `--all-features` 只被 clippy 覆盖、**没有被执行**（workspace 跑默认 features）；覆盖率 job `fail_ci_if_error: false`（无下限）；`FORGE_CODEGEN_EVENTS` 未在 CI 落盘 |
| E9 | **能力/性能待办** | 台账三条 `待做`：`AGG_LOAD_USE`、`VA_ARG_AGG_RULE`、`VA_ARG_VECTOR_MEM`；谱侧 `dsl/codegen` 的 66 处发射型 `Unsupported` 无 `[cap:ID]`（v25 M2 明确留作另一条线）；`OPTIMIZATION.md` 两项 P1（`codegen_mem` 451.9 µs / `codegen_big_loop` 1.14 ms）标"需分析" |

## 2. 里程碑

### M1 诊断单一来源：错误码台账 + 事件 sink + 开关归并

**问题**：E1/E2/E3——三处诊断面各写各的，且其中之一已经漂移（`DSL-ABI` 文档还在教一个已删除的节）。

- **错误码台账**：新增 `docs/reference/isa-dsl-errors.md` ↔ 源码的**双向守卫**（扫
  `crates/frontend/forge-isa-dsl/src` 的 `"DSL-*"`/`"LINT-*"` 字面量；文档侧只认全大写 `XXX-YYY` 形态，
  排除 `dsl-tutorial` 这类链接文本误命中）。修 4 处漂移：删文档里的 `DSL-ABI`（节已不存在）与 `DSL-SLOT`
  （无源码点时删，有则补源）；补 `LINT-ASM-AMBIG`、`LINT-FORM-FIELD-UNUSED` 两行。
- **事件 sink 下沉（通用性）**：新增 `forge-isa-runtime::diagnostics`（leaf 层，`cpu.rs` 已在此）：
  - `CHANNELS` 注册表（`codegen`/`jit`/`asm`/`e2e`，每条含 env 名与用途）+ `emit(channel, name, fields)`：
    **唯一**的 JSON 转义/落盘实现（写失败静默，沿用既有约定）；
  - `forge-codegen/src/pipeline/events.rs` 改为**薄转发**（保留 `EVENTS` 台账与 `event_names_are_bijective`）；
  - `forge-codegen/src/lib.rs::jit_event` 与 `forge-tests` 的两处事件写入改为调用同一 sink（删掉各自的
    `OpenOptions`/`writeln` 复制）。
  - 守卫：`CHANNELS` ↔ 四个 env 名 ↔ 契约文档的通道表**三方双向**。
- **开关台账归并**：`pipeline::trace::KNOBS` 下沉到同一 leaf（`diagnostics::KNOBS`），`forge-rustc/src/trace.rs`
  的名字进**同一张表**（`knob()` 语义不变：未登记名字 debug panic、release 警告一次）；`trace_knobs_are_bijective`
  的扫描面扩到 `crates/tools/forge-rustc`。
- **删除清单**：`jit_event` 的独立实现、forge-tests 的两处事件写入函数体、`forge-rustc` 自带的开关名字集合
  （`rg 'fn trace_enabled' crates/tools/forge-rustc/src == 0`）。

### M2 无消费者面清理（无冗余）

**问题**：E4/E5/E6 + 34 处 `#[allow(dead_code)]`。

- **`forge-mem` 折叠并删除**：`ExecutableMemory`/`MemError` 移入 `forge-isa-runtime::mem`
  （`jit.rs` 改一处 `use`）；删 `CpuFeatures`/`host_cpu.rs`（与 `cpu.rs` 重复且无使用者）；
  删 crate、workspace 成员、`forge-codegen` 的依赖边、根 `Cargo.toml` 的 `forge-mem` 条目。
- **删 `isel_strategy`（已批准）**：删 `ir/isel_strategy.rs`、`Instruction` 字段与
  `isel_strategy()/set_isel_strategy()/clear_isel_strategy()`、`binary/funcs.rs` 的读写、`verify.rs` 两处、
  `forge-isa-runtime/src/prelude.rs` 重导出、`forge-opt` 的 4 处透传拷贝（**它们的存在理由只是保住这个字段**）；
  **`IR_FORMAT_VERSION` 3 → 4** + `docs/reference/binary-format.md` 版本历史；删 `tests/isel_strategy.rs`，
  改 `binary_functions.rs`/`tombstone_semantics.rs` 的引用。删除清单：`rg 'isel_strategy|IselStrategy' crates/ == 0`。
- **v11 遗留与停更注释**：删 `forge-grammar/grammars/asm_v11.lx` 与引用它的两个测试
  （`asm_grammar_test.rs`/`ast_lowering_test.rs` 收敛到 `ir.lx`）；改 `forge-grammar/src/lib.rs:47` 的停更注释。
- **前端线降级为示例（已批准）**：`crates/frontend/forge-hir`、`crates/frontend/forge-hir-macro`、
  `examples/mini_c` 用 `git mv` 移入 `examples/hir-demo/`（自带 `Cargo.toml` + README：「示例 / 非产品 /
  不进门禁」），根 `Cargo.toml` 加 `exclude`；`forge-grammar` 保留为成员（CST/AST 核心 + `ir.lx` 测试），
  账本写明"其剩余消费者只有该示例；示例若再移除，它按无消费者面删除"。
- **`#[allow(dead_code)]` 审计（34 处 src）**：逐条"用起来或删掉"；生成物发射器的条目保留并在守卫/注释里写明理由。

### M3 单命令门禁 + CI 补齐（人体工学 / 验证）

**问题**：E7/E8——全套门禁只写在文档里，CI 少两道闸。

- **`cargo forge gate`**（复用既有 clap CLI 与 `cmd/` 模式，不新开 xtask 层）：
  一条命令依次跑 **fmt → clippy(workspace + 文本层门) → workspace tests → `forge-codegen --all-features` →
  守卫清单 → JIT 两矩阵 → asm 三档 → markdownlint**，**逐项打印**（名称/命令/耗时/退出码），
  失败即汇总非零退出；开关 `--subset linux|windows|quick`、`--guards`（只跑守卫）、`--no-docs`、`--json`。
  守卫清单 = CLI 里**一张注册表**（新增守卫只加一行）；markdownlint 缺失时明确 SKIP 并提示安装。
- **CI 补齐**：新增 markdownlint job（pin `markdownlint-cli2` 版本、读仓库 `.markdownlint.json`、覆盖
  `docs/**` + `CLAUDE.md`/`CHANGELOG.md`/`README.md`）；Windows job 增 `cargo test -p forge-codegen --all-features`
  （覆盖 `parallel` + `jit`），Linux job 增 `cargo check --workspace --all-features`；Windows job 设
  `FORGE_CODEGEN_EVENTS` 且用 `if: always()` 步骤打印（同 e2e 既有模式）；覆盖率加**下限棘轮**
  （先实测钉一次，此后只许上调）。
- **文档**：`docs/guides/lint.md` 补 CI 命令；`CLAUDE.md` 的 Build Commands 改为指向 `cargo forge gate`。

### M4 能力补齐（功能）

**问题**：E9。

- **谱侧能力台账（`[cap:SPEC_*]`）**：枚举 `forge-isa-dsl/src/dsl/codegen` 的 66 处发射型 `Unsupported`，
  按 v25 M2 的四分类逐条判定"这份谱做不了"的那一类，改成发射 `[cap:SPEC_<名>]`；`capability_gaps_guard`
  的扫描面扩到生成器源（扫 `[cap:SPEC_` 字面量），台账加对应行、每条写清决策与数据条件。
- **后端三条 `待做` 缺口**：`AGG_LOAD_USE`（大聚合 `load` 值用于 store/extractvalue 之外的使用点）、
  `VA_ARG_AGG_RULE`（aapcs64/lp64d 声明 + 该 ISA 的聚合变参真执行通道）、`VA_ARG_VECTOR_MEM`（mem→向量取值）。
  逐条**落地或明确降级为不做**并写回台账——决策必须离开"待做"，或写明不可做的数据条件。
- **性能两项只做分析**（`codegen_mem` 451.9 µs / `codegen_big_loop` 1.14 ms「需分析」）：产出结论写回
  `docs/performance/OPTIMIZATION.md` + 账本，**不承诺实施**。

### M5 契约回写 + 账本工程化

- 文档：`docs/reference/isa-dsl-errors.md`（台账 + 守卫指针 + 状态头日期）、
  `docs/reference/backend-pipeline.md`（事件表扩到四通道 + 新增"诊断出口"一节）、
  `docs/reference/binary-format.md`（格式版本 4 与历史表）、`docs/guides/lint.md`、`README.md`/`CLAUDE.md`
  （依赖图去掉 `forge-mem`；前端线降级说明；`cargo forge gate` 为唯一门禁入口）。
- 账本 §8 逐里程碑回写（实测数字、偏差、踩坑）；`CHANGELOG.md` 每里程碑一条；**收口后归档到 `docs/archive/`**
  并全量更新引用与文档地图（同 v23/v24/v25）。

## 3. 顺序与窗口

`M1`（诊断面，风险低、面广）→ `M2`（破坏性删除，单独窗口：格式版本升 + 前端线搬运）→ `M3`（门禁）→
`M4`（能力，逐条可裁剪）→ `M5`（收口）。

M2 若中途发现"某个面其实还有消费者"（守卫/`rg` 反证），**停下并如实记录**，不为了删而放宽判据。

## 4. 交付物与提交

1. 活账本 `docs/plans/project-v26-plan.md`（首次提交）。
2. 代码/配置：`forge-isa-runtime::diagnostics`（M1）、`ExecutableMemory` 折叠与 `isel_strategy` 删除（M2）、
   `cargo-forge` 的 `cmd/gate.rs` + CI（M3）、`[cap:SPEC_*]`（M4）、文档回写（M5）。
3. 新增守卫：`error_codes_match_doc`、`event_channels_are_bijective`、`knobs_are_bijective`（扩面）、
   `capability_gaps_guard`（扩面到谱侧）。
4. 每里程碑一次提交（单 subject 长标题，`git commit -F target/tmp/*.txt`）。

## 5. 成功判据（每里程碑）

- 全量门禁（= `cargo forge gate`，M3 起单命令）逐项绿：fmt、clippy×2、workspace tests、
  `forge-codegen --lib --all-features`、12+ 条守卫、JIT 两矩阵（197/3/0 · 136/64/0）、asm 三档（3/1/1）、
  markdownlint 0 issues。
- 该里程碑的删除清单归零（`rg` 判据逐条列出并复核）；新增守卫在 CI 里会跑到。
- 账本 §8 与 CHANGELOG 已回写；改动文档 markdownlint 干净。

## 6. 假设与不做清单

- **不做**：Windows SEH / ARM EH codegen、statepoint（长期无近计划）；不新增 ISA；不改 `forge-abi` 的
  规则/绑定数据模型；不动 C ABI 互操作行为。
- **假设**：`examples/hir-demo/` 脱离工作区后仍可独立 `cargo run`（保留其 `Cargo.toml` 与 path 依赖指向
  workspace 内的 `forge-grammar`/`forge-ir`）；CI 的 `markdownlint-cli2` 与工具链版本在实现时以仓库现状为准；
  `IR_FORMAT_VERSION` 升版只影响二进制接口（`check_binary_compat` 的黄金与版本表同步改）。
- **风险**：M2 的格式升版会改变二进制 IR 兼容面（仓库允许破坏性更新，但必须同步 `binary-format.md` 与相关测试）；
  前端线搬运会让历史路径变化（用 `git mv` 保留追溯）。

## 7. 外部参考

- [matklad/cargo-xtask](https://github.com/matklad/cargo-xtask/) 与
  [cargo-xtask-example](https://github.com/nickgerace/cargo-xtask-example)：把"构建/门禁自动化"做成一条命令的
  通行做法——本仓**复用既有 `cargo-forge`**（少一层 xtask crate，"无冗余"），只借它的"单入口 + 逐项报告"形态。
- [rust-lang/measureme](https://github.com/rust-lang/measureme) 与 rustc 的 `SelfProfiler`
  （[`SelfProfiler` 文档](https://doc.rust-lang.org/stable/nightly-rustc/rustc_data_structures/profiling/struct.SelfProfiler.html)）：
  编译器诊断/性能数据以**结构化行**落盘的成型做法——M1 的事件 sink 对齐这一思路（统一转义与通道注册表、
  名字由守卫钉住、文档化 schema）。

## 8. 批次记录（逐里程碑回写）

### M2 无消费者面清理 —— **一次落地**（2026-10-11，工作区）

**破坏性变更（有意）**：`forge-mem` crate 删除；`Instruction::isel_strategy` 删除且
`IR_FORMAT_VERSION` **3 → 4**；前端线（`forge-hir`/`forge-hir-macro`/`mini_c`）移出工作区。

① `forge-mem` 折叠（E4 收口）

- `ExecutableMemory`/`MemError` 用 `git mv` 搬进 `forge_isa_runtime::mem`（保留历史）；
  `forge-codegen/src/runtime/jit.rs` 改一处 `use`；删 crate + 依赖边 + workspace 成员。
- **`CpuFeatures` 只留一个函数**：`detect()`（含一套 CPUID/xgetbv 检测）**零调用者**，只有
  `host_arch_name()` 被 mem 的 GDB 描述符用来写 ELF `e_machine` ⇒ 把它并入 `cpu.rs`
  （主机探测本来就住那儿），其余 191 行删掉。
- 根 facade 的 `code_forge::mem` 路径**保持不变**（`pub use forge_isa_runtime::mem;`）⇒
  `forge-tests`/`benches`/`examples` 的 7 处调用点零改动。

② 删 `isel_strategy`（E5 收口）

- 删除面：`Instruction` 私有字段 + 三个访问器 + `ir/isel_strategy.rs` 类型本体 + 二进制读写，
  以及 `verify.rs` 的墓碑校验项、`forge-isa-runtime` prelude 重导出、**`forge-opt` 三处真透传
  代码块**（先前侦察只看到注释，实现里其实各有一段 `if let Some(strategy) = … set_isel_strategy`）。
- `IR_FORMAT_VERSION` 3 → 4 + `format.rs` 版本史 + `docs/reference/binary-format.md`（标题/状态头/
  FUNCS 字段清单）。
- 测试面：删专用 `tests/isel_strategy.rs`；`binary_functions.rs`/`tombstone_semantics.rs` 去掉
  相关断言；**`mini_inst` 夹具少一个字段**——两个负向夹具原先靠手写字节布局，字段一删就错位
  （实测 `Option 判别位应为 0/1，实测 4`），已同步。

③ v11 遗留与"停更注释"（E6 的一半）

- 删 `forge-grammar/grammars/asm_v11.lx` 与只测它的 `tests/asm_grammar_test.rs`；
  `ast_lowering_test.rs` 收敛到 `ir.lx`（补回因此丢掉的 `use`）。
- **立项判断修正（如实记录）**：E6 说 `forge-grammar/src/lib.rs:47` 的注释"已停更"——实读是
  **准确的**（写明语义层已删及其原因）⇒ **不动**。审查结论以代码为准，此处以代码为准。

④ 前端线降级为示例（已批准）

- `crates/frontend/forge-hir` → `examples/hir-demo/hir`、`forge-hir-macro` → `examples/hir-demo/hir-macro`、
  `examples/mini_c` → `examples/hir-demo/`（均 `git mv`）；路径依赖改写；根 `Cargo.toml` 去掉三个成员
  并 `exclude = ["examples/hir-demo"]`；新增 README 写明**它不再有 CI 覆盖、会静默失效**以及重新纳入的两条路。
- `forge-grammar`（CST/AST/visit + `ir.lx` 测试）**仍是成员**；账本记明：它的剩余消费者只有该示例，
  示例若再移除，它按"无消费者面"删除。

⑤ `allow(dead_code)` 审计（34 处，E6 的剩余项）

用**证据**而不是判断：先把 34 处属性全部摘掉，让 rustc/clippy 指认，再分类回填。结果：

- **17 处是多余的**——它们**发射进生成物**（`dsl/codegen/asm.rs` 10 + `dsl/codegen/mod.rs` 4 +
  `lib.rs` 3），而生成物整体带 `#[allow(warnings, clippy::all)]`（证据：`gen_file.rs:274`）⇒ 直接删；
  删后全量 clippy `-D warnings` 仍零告警（这就是判据）。
- **1 处是真死代码**：`forge-ir` 的 `#[cfg(debug_assertions)] fn debug_check_shape`（文档自己写着
  "不在发射路径上强制"）⇒ 删（类型规则的强制点在 verifier；builder 的宽松是有意的，5 条测试依赖它）。
- **2 处只在 test 目标里被用**（`lexer.rs::matches_pattern`、`Reader::header`）：摘掉后 `lib` 目标报
  never used、测试目标编译失败——**两个目标同时成立**才是真相 ⇒ 升级为精确写法
  `#[cfg_attr(not(test), allow(dead_code))]` 并写明理由（无条件 allow 会连测试目标一起豁免，掩盖真死代码）。
- 其余 14 处保留（含 3 处模块级 `#![cfg_attr(not(test), …)]`，那是"测试支持模块"的既有惯用法）。

**验收（同批实测）**：`cargo fmt --all -- --check` / `cargo clippy --workspace --exclude forge-rustc
--all-targets --all-features -D warnings` / `cargo clippy -p forge-ir --no-default-features --lib
-D warnings`（文本层门）/ `cargo check -p forge-rustc` / `cargo test --workspace --exclude forge-rustc`
全部 exit 0；`forge-codegen --lib --all-features` **2834 passed**；守卫全绿（9 条后端、`read_path_budget`
与 `error_codes_match_doc`）；JIT 两矩阵 **197/3/0** 与 **136/64/0**；asm 三档 **3/1/1**；改动文档
markdownlint 0 issues。

**删除清单（本里程碑）**：

- crate：`forge-mem`（含 `src/host_cpu.rs` 与 `CpuFeatures::detect` 一套 CPUID/xgetbv）；
- IR：`isel_strategy` 字段 + 三访问器 + 类型本体文件 + 二进制读写 + verifier 校验项 + prelude 重导出，
  以及 `forge-opt` 三处透传块（判据：`rg 'isel_strategy|IselStrategy' crates/` 只剩 `format.rs` 的版本史注释）；
- 语法/测试：`grammars/asm_v11.lx` + `tests/asm_grammar_test.rs`；`forge-ir/tests/isel_strategy.rs`；
- 死代码：`debug_check_shape`（1 项）+ 17 处多余的发射用 `allow(dead_code)`；
- 工作区成员：`forge-hir`/`forge-hir-macro`/`mini_c`（3 个，改为 `exclude`）。

### M1 诊断单一来源 —— **一次落地**（2026-10-11，工作区）

**做了什么**（三块，都在"承诺与实现分叉"这一类上）：

① DSL 错误码台账（E1 收口，且比立项时的判断更深一层）：

立项时只看到"两侧集合不等"（源码 29 / 文档 31）。落地时查明**根因在实现**：`section_code(kind)`
的映射表**缺 `operand`/`operand_slots`/`machine` 三个 kind** ⇒ `[operand.<名>]` 与 `[machine]`
的诊断一律落到 `DSL-OTHER`，而文档恰写着"出现 `DSL-OTHER` 就说明消息缺前缀、应报 bug"——
判据被反用。文档承诺的 `DSL-SLOT`/`DSL-ABI` 因此**从来没被发出过**。

- `diag.rs` 补三个映射（`operand`/`operand_slots` → `DSL-SLOT`；`machine` → **`DSL-MACHINE`**，
  旧名 `DSL-ABI` 指 `[machine]` 属误导，按"码名与节名对齐"改名）。
- `isa-dsl-errors.md`：`DSL-ABI` 行改 `DSL-MACHINE`；被划掉的 `DSL-EMIT` 行写回现实
  （该码仍活着：`machine.text`/`emit` 两个 kind 都映射它）；`LINT` 行补 `LINT-ASM-AMBIG`、
  `LINT-FORM-FIELD-UNUSED`（源码有、文档漏）。
- **新守卫** `crates/frontend/forge-isa-dsl/tests/error_codes_match_doc.rs`：源码字面量 ↔
  文档 **§2 表**码集合双向相等（只扫该小节：实测全文正则会误命中 `dsl-tutorial` 这类链接文本）。
- **诊断矩阵如实抓到 2 例**（`diag_matrix_tests.rs` 钉每例的码与精确行）：它们此前把"缺映射"
  写成了契约（`DSL-OTHER`）⇒ 期望码改正为 `DSL-SLOT`/`DSL-MACHINE`；改名只记进 CHANGELOG
  （代码串里不留旧码名，否则守卫会把它当发射点）。
- 实测终态：源码 **31** 码 == 文档 §2 **31** 码；`cargo test -p forge-isa-dsl` 全绿（301 lib + 12 个二进制）。

② 事件 sink 下沉（E2 收口）：

四个 `FORGE_*_EVENTS` 通道原先各写一遍 `OpenOptions+append+writeln`（环境变量名也散在四处）。
新增 **`forge_isa_runtime::diagnostics`**（叶层）：

- `CHANNELS` 注册表（`codegen`/`jit`/`asm`/`e2e` + 各自 env + 用途）+ `emit`（JSON 行，唯一转义）
  与 `emit_text`（逐字文本，三个文本通道的既有格式**不动**：CI 的 `Select-String` 依赖
  `MATRIX-SKIP`/`RETRY`/`ASM-EXEC-SKIP` 这些 token）+ **写失败静默**（四个通道一致）
  与未登记通道 fail-closed（debug panic / release 警告一次）。
- `pipeline/events.rs`、`lib.rs::jit_event`、`forge-tests/src/asm/report.rs::emit_event`、
  `forge-rustc/tests/e2e.rs::record_event` 全部改**薄转发**（只决定行的格式）。
- 根 facade 补 `pub use forge_isa_runtime;`（测试面经它用同一 sink）。
- **新守卫** `event_channels_are_bijective`（`events_guard.rs` 第 3 条）：`CHANNELS` ↔ 源码调用点
  ↔ 契约文档 §12 的通道表**三方双向**。落地时踩到两个真坑并如实修掉：① 通道表原先放在
  "事件表"小节里，被 `event_names_are_bijective` 的整节解析吞掉 ⇒ 独立成 §12；
  ② 守卫扫源码时**自己被命中**（它写出了 `diagnostics::emit(` 这个标记串）⇒ 自跳过 +
  跨行解析（rustfmt 折行，本仓守卫的常客）。

③ 开关台账归并（E3 收口）：

- `KNOBS`/`knob` 从 `forge-codegen::pipeline::trace` 下沉到同一 leaf，**两侧并成一张表**：
  后端 11 项 + 直读项（`FN_SIZE`/`CODE_HEX`/`LIVE`/`DW`）+ `forge-rustc` 的 27 项 = **41 项**；
  新增 `trace(suffix)`（`forge-rustc` 的 `trace_enabled("SLOT")` 写法同源转发）。
- **5 处绕过台账的直读**（`FORGE_TRACE_FN_SIZE`/`CODE_HEX`/`LIVE`/`DW`，以及 `forge-ir` 内
  因层依赖必须自持的 `FORGE_TRACE_IR`）里，前四个改走 API；`IR` 登记进表并在"看什么"里写明
  为什么实现在本 crate。
- `forge-rustc/src/trace.rs` 的 13 行名字表**删掉**（第二份清单必然漂移），改为指向契约文档。
- 守卫 `trace_knobs_are_bijective` 重写为**真三方**：源码字面量（全 `crates/**/src`，
  跳过台账定义文件、跳过注释、在第一个 `#[cfg(test)]` 处截断）↔ `diagnostics::KNOBS` ↔
  文档 §7 表（扩到 41 行）。
- **连带发现并修掉文档重号**：v25 M5 插入 `## 6/7/8` 时没顺延既有的 `## 6 模块签名表`/`## 7 观测开关`
  ⇒ 同号并存（守卫按关键词定位所以没红）。现按物理顺序重排为 1–13。

**验收（同批实测）**：`cargo fmt --all -- --check` / `cargo clippy --workspace --exclude forge-rustc
--all-targets --all-features -D warnings` / `cargo check -p forge-rustc` / `cargo test --workspace
--exclude forge-rustc` 全部 exit 0；`forge-codegen --lib --all-features` **2834 passed**（比 M1 前少 2：
`trace.rs` 的 3 条台账单测搬进 `diagnostics.rs` 并合成 1 条跨 crate 用例，属预期）；新增/改动守卫
`error_codes_match_doc` 1、`events_guard` 3、`trace_knobs` 2 全绿，其余守卫不回归；JIT 两矩阵
**197/3/0** 与 **136/64/0**；asm 三档 **3/1/1**；改动文档 markdownlint 0 issues。

**删除清单（本里程碑）**：

- `pipeline/trace.rs` 的 `Knob`/`KNOBS`/`knob`/`warn_unregistered` 实现（改薄转发）；
- `lib.rs::jit_event`、`forge-tests/src/asm/report.rs::emit_event`、`forge-rustc/tests/e2e.rs::record_event`
  各自的 `OpenOptions`/`writeln` 实现（改薄转发）；
- `forge-rustc/src/trace.rs` 的 13 行名字表（第二份清单）；
- 4 处直读环境变量的开关写法；旧码名 `DSL-ABI`（代码串零出现；改名只记 CHANGELOG）。
