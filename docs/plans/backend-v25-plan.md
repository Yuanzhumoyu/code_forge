# 后端 v25 里程碑方案：一次分类 · 诊断分层 · 模块收敛

> **状态：[progress]**（2026-10-11 立项）。本文是**活账本**：每个里程碑落地时在 §6 追加
> 批次记录（含实测数字、命令、偏差），任何"说得出但没做到"的东西必须如实写在这里。
> 立项时的代码基线 = v24 收口提交 `cc9236ce`；**行号随迭代漂移，以符号名为准**。
> 上一轮方案（v24，已收口归档）见 [`docs/archive/backend-v24-plan.md`](../archive/backend-v24-plan.md)；
> 现行契约 [`reference/backend-pipeline.md`](../reference/backend-pipeline.md)、
> 能力缺口台账 [`reference/backend-gaps.md`](../reference/backend-gaps.md)。

前提纪律（沿用既有约定）：

- **一个任务一次提交**（不按批次拆 PR）；**里程碑未完成不提交**，工作累积在工作区。
- 每个里程碑以仓库全量门禁（`cargo test --workspace --exclude forge-rustc`、JIT 两矩阵、
  asm 三档棘轮、clippy `-D warnings`、fmt）+ 该里程碑**新增守卫**为验收；**删除清单逐条清零**才算完成。
- 破坏性更新允许，但 **C ABI 互操作不可破坏**（见 §5）。
- 守卫必须精确（白名单条目**必须被命中**）；预算/棘轮只许按实测**下调**，不许为放过而上调。

## 1. 立项证据（只读侦察实测，2026-10-11）

| # | 证据 | 数字 / 位置 |
| --- | --- | --- |
| E1 | **计划相是死数据** | `AbiEdges` 共 8 处引用，只有 1 处**写**（`legalize/mod.rs` 的 `ctx.edges = Some(edges)`），**没有任何消费者**（全仓 grep 无读点） |
| E2 | **分类重复计算** | 落点探针 5 处调用：`agg_param.rs:51`、`agg_call.rs:32/92/512`、`plan.rs:220/240/264/287`；两个探针（`common.rs:47/94`）**每次查询各自跑一遍引擎规划 + 注册表读锁** |
| E3 | **落点类型重复** | `AggArgLoc` 22 处引用；`forge-abi-model` 已有 `Placement`（`Reg`/`RegPair`/`RegGroup`/`Stack`/`StackByVal`/`Indirect` + 消费者访问器 `as_reg`/`indirect_ptr`/`stack_offset`/`is_stack_byval`/`reg_refs`）。`AggArgLoc` 是它的**三值手抄投影**（`common.rs:58–74`） |
| E4 | **计划内五种枚举无对外价值** | `ParamPlan/RetPlan/ArgPlan/CallResultPlan/VaArgPlan` 共 38 处引用，构造与打印都在 `plan.rs` 内 |
| E5 | **错误语义混杂** | 全仓 `Unsupported` 代码站点 **127**；其中标了 `[cap:ID]` 的 **20**；后端**未标 ~33**（`agg_call`5/`agg_param`6/`materialize`9/`agg_const`4/`gep`4/`agg_store`2/`va_expand`2/`common`1/`compiler`1/`conv_registry`1/`sig_view`1/`jit`1）；`Internal` 已用 59 处；`Unimplemented` 只剩 **3 处**（`machine/frame.rs` 的 trait 默认实现） |
| E6 | **诊断不可机读** | 11 个 trace 开关全是 `eprintln!` 人读文本；而 JIT / asm / E2E 已有 `FORGE_JIT_EVENTS`/`FORGE_ASM_EVENTS`/`FORGE_E2E_EVENTS` 落盘事件（`forge-tests/src/lib.rs:116–130`）——**codegen 阶段是唯一没有机读出口的** |
| E7 | **大文件** | `compiler.rs` 1467 / `regalloc_bt.rs` 1453 / `va_expand.rs` 1079 / `lowering.rs` 799 / `agg_call.rs` 688 / `materialize.rs` 640 / `agg_param.rs` 599 行 |
| E8 | **外部设计参考** | ClangIR ABI Lowering（"分类单一来源 + rewriter 只消费分类"，并有 `classification-attr` 注入式测试驱动）；LLVM GlobalISel IRTranslator（ABI lowering 作为独立 late pass）；Cranelift `ABIMachineSpec`/`abi_impl`（每目标 spec × 通用实现） |

结论：v24 的"两相化"只做完了**改写**那一半（单遍物化），**分类**那一半仍散在 5 个调用点，
且计划相产出的 `AbiEdges` 无人消费 ⇒ 这就是"无冗余"的下一个主战场。

## 2. 里程碑

### M1 —— 一次分类，处处消费：`AbiEdges` 成为唯一的落点来源

**问题**：同一个 ABI 事实被算 ≥2 次（E1/E2），并用我方手抄类型表达（E3/E4）。

**改法**（数据流：计划相[分类] → 各 pass[只消费]）：

- 新增 `pipeline/legalize/classify.rs`——**唯一**的合法化分类入口，只在这里问引擎：
  - `classify_signature(machine, conv, store, params: &[TypeId], rets: &[TypeId]) -> Result<Classified, IrError>`：
    **一次** `plan_for_shapes`（把该函数**全部**聚合形参 + 返回类型合成一个 `CallRequest`），
    一次注册表读锁，一次 `frame_layout_info`（拿 `shift`）；返回 `Classified { args: Vec<ArgLoc>, ret: RetLoc, shift: i32 }`。
  - `classify_call_args(machine, conv, store, tys: &[TypeId]) -> Result<Vec<ArgLoc>, IrError>`：
    **每个调用点一次**（同样把该调用点的聚合实参类型合成一个请求）。
  - 形状一律走 `TyView::from_ir_type`（保留 C1 修复的不变量：成员必须摊开，否则 HFA 被当普通聚合 ⇒ 静默错值）。
- `legalize/plan.rs` 重写为**计划载体**（不再自带枚举）：`params: Vec<ArgLoc>`、`ret: RetLoc`、
  `call_args: Vec<CallArgEdge { inst, op_idx, val, loc: ArgLoc }>`、
  `call_results: Vec<CallResultEdge { inst, val, ret: RetLoc }>`、
  `va_args: Vec<VaArgEdge { inst, val, rule: forge_abi_model::VaAggregateArgRule }>`；
  `actionable()` / `summary()` 改为按新载荷统计与打印。
- 消费者改为**读计划**：`agg_param` 读 `edges.params[i]`；`agg_call` 读 `edges.call_args/call_results`
  （按 `(inst, op_idx)` 匹配）；`materialize` 读计划里的 `ret`/结果落点；`va_expand` 读 `edges.va_args`。
  计划缺失 ⇒ `IrError::Internal("合法化计划缺失（PlanPass 未跑）")`（驱动恒把 `PlanPass` 排首位，已由
  `driver_has_the_documented_pass_order` 钉住）。
- **删除清单**（判据，全部 `rg` 归零）：`AggArgLoc`、`agg_arg_place`、`agg_ret_is_sret`、
  五种计划枚举名；`plan_for_shapes` 在 `crates/backend/forge-codegen/src` 只允许出现在
  `legalize/classify.rs` 与 `pipeline/abi_setup.rs`。
- **新增守卫**：
  1. `classification_has_one_entry`（源码级）：`plan_for_shapes` 只许出现在上述两文件（白名单条目必须被命中）；
  2. `edges_drive_the_rewrite`（源码级）：`agg_param.rs`/`agg_call.rs`/`materialize.rs` 必须引用 `ctx.edges`；
  3. `one_engine_plan_per_function`（测试级）：在 `abi_target::plan_for_shapes` 加 `#[cfg(test)]` 计数原子，
     编译一个含 ≥3 个聚合形参的函数后断言**计数 == 1**（回归"按类型逐次探针"）；
  4. 行为等价：先用临时对拍测试断言"新计划载荷 == 旧探针输出"（在现有 JIT 用例的函数上），再删旧探针。
- **验收**：删除清单归零；四条守卫绿；全量门禁 + 两矩阵 + asm 三档绿（win64/sysv64/aapcs64/lp64d 的聚合用例已覆盖）。
- **风险**：中高（触及全部落点决策）。缓解：专职窗口；对拍先行；不改"决策是什么"，只改"决策从哪来"。

### M2 —— 错误分类学：失败语义分层 + 变体去冗余

**问题**：`Unsupported` 一名三用（能力缺口 / 畸形 IR / 使用错误），`Unimplemented` 与它语义重叠（E5）。

**改法**：冻结四分类与映射，全后端（`forge-codegen` + `forge-isa-runtime`）逐条归类：

| 语义 | 变体 | 判据 |
| --- | --- | --- |
| 这台机器/这份约定/这份谱**做不了** | `IrError::Unsupported` + **`[cap:ID]`** | 用户换约定/补规则/换 ISA 就能解 |
| **畸形 IR / 内部不变量**被破坏 | `IrError::Internal` | 字段越界、常量池缺项、索引缺失、结构不符 |
| **约定/注册表数据错或缺失** | `IrError::Semantic` | 约定未注册、约定把聚合判成标量栈落点 |
| 发射/分配等阶段自己的失败 | 现成变体（`Emit`/`RegAlloc`/`Verify`…） | 保持 |

- 具体动作：~30 处畸形 IR 站点 → `Internal`；~3 处能力站点补 `[cap:ID]`（同时进台账）；3 处使用/数据站点 → `Semantic`；
  **删除 `IrError::Unimplemented`**（3 处 trait 默认实现 → 新增 `[cap:FRAME_EMIT_EPILOGUE_JUMP]`/`[cap:FRAME_SPILL_LOAD]`/`[cap:FRAME_SPILL_STORE]`：
  "这份谱没覆盖该帧操作"本就该在缺口台账里可见）。
- **构造器下沉（通用性）**：把 `gaps::capability` 从 `forge-codegen` 移到 **`forge-ir`**
  （`IrError::capability(id, msg)` + `IrError::gap_id()`）——`forge-isa-runtime` 也需要它能构造缺口错误；
  `pipeline::gaps` 删除，21 处调用点机械迁移（破坏性允许）。
- **删除清单**：`Unimplemented`（含 `Display` 臂）全仓归零；`pipeline::gaps` 归零；
  后端未标 `Unsupported` 归零（白名单必须被命中且写明理由）。
- **新增守卫**：
  1. `unsupported_is_always_a_capability`：扫 `crates/backend/**` + `crates/foundation/forge-isa-runtime/**` 的
     非注释 `Unsupported(`，必须经 `IrError::capability`；白名单条目（构造器自身）必须命中；
  2. `no_unimplemented_variant`：全仓 `Unimplemented` == 0；
  3. 既有 `capability_gaps_guard` 自动强制台账补齐新 ID（双向相等），台账 §3 逐条写理由。
- **范围外（写进台账为 `待做`）**：`dsl/codegen` 里 66 处**生成物消息**（要把 `[cap:SPEC_*]` 穿进生成器模板，另一条线）。
- **假设**：`IrError` 不进二进制格式——实施首步核对 `docs/reference/binary-format.md`；若进则升 `IR_FORMAT_VERSION` 并改版本表。
- **风险**：低（机械 + 破坏性公开枚举变更）。

### M3 —— 观测事件化：codegen 也能被 CI 机读

**问题**：codegen 是唯一没有结构化事件出口的阶段（E6）。

**改法**：

- 新增 `pipeline/events.rs`：`pub static EVENTS: [EventSpec; N]`（名字 + 说明）+ `emit(name, fields)`
  → 当 `FORGE_CODEGEN_EVENTS=<path>` 时**追加一行 JSON**；写失败静默（沿用 `FORGE_JIT_EVENTS` 的既有约定：
  可见性辅助不得影响编译结论）。不引入新依赖：自带最小 JSON 写出（转义 + 固定字段序 ⇒ 输出确定性）。
- 首批事件（与现有 summary 打印点一一对应，人读文本保留给 `trace::knob`）：
  `COMPILE-START`、`COMPILE-DONE`（各阶段毫秒：blocks/vreg/lower/peep/regalloc/frame/emit）、
  `CAPABILITY-GAP`（`id` + 函数名 + 阶段）、`LEGALIZE-PLAN`（各类边界计数）、
  `ALLOC-SUMMARY`（溢出数/寄存器数）、`EMIT-SUMMARY`（字节数/重定位数）。
- **新增守卫** `event_names_are_bijective`（照 `trace_knobs_are_bijective` 的三方双向相等）：
  源码 `events::emit("…")` ↔ `EVENTS` 表 ↔ `reference/backend-pipeline.md` 的事件表。
- **测试**：集成测试把 `FORGE_CODEGEN_EVENTS` 指向临时文件、走 `CompileSession` 编译一个小模块，
  逐行断言事件名与字段（固定字段序 ⇒ 可整行比较，无需 JSON 解析依赖）。
- **风险**：低。

### M4 —— 人体工学：管线大文件按职责切分 + 规模棘轮

**问题**：四个文件 >1000 行（E7），读改成本高、评审难。

**改法**（纯**逐字搬移**，不改行为；`pub(crate)` 可见性原样保留）：

- `compiler.rs` → `compiler/`：`mod.rs`（`CompileSession` + 入口）、`state.rs`（`CompileState` + 单函数管线）、
  `bind.rs`（`inst_ops` ↔ `OperandBind` 回填）、`tests.rs`（现有 `alloc_integration_tests`）
- `regalloc_bt.rs` → `regalloc/`：`mod.rs`（`BacktrackingAllocator`）、`state.rs`（`BtState`）、
  `spill.rs`（`build_spill_edits` + 溢出/回填）、`tests.rs`
- `va_expand.rs` → `va/`：`mod.rs`（`expand_va`/`AbiSlots` 入口）、`site.rs`（`va_arg_site`/`expand_arg`）、
  `emit.rs`（`mk_*` 辅助）、`slots.rs`（`plan_abi_slots`/`frontend_locals_depth`）、`tests.rs`
- `legalize/agg_call.rs` → `legalize/agg_call/{mod.rs, args.rs, results.rs}`
- **新增守卫** `file_size_ratchet`（`crates/backend/forge-codegen/tests/file_size_ratchet.rs`）：
  签入 `(path, max_lines)` 表（`src/pipeline/**` 每个文件必须在表内或 ≤ 缺省 700），实测 > 上限即红；
  **只许按实测下调**（与 asm 计数棘轮同一纪律）。切分后目标：全部 ≤600。
- **新增守卫** `pipeline_layering`：`pipeline/legalize/**` 与 `pipeline/{regalloc,va,emission,frame}*` 不得
  `use crate::pipeline::{compiler,session}`（叶方向；白名单条目必须命中）。
- **风险**：低但 diff 大；排在 M1–M3 之后（代码稳定后再搬），每步以全量门禁验证。

### M5 —— 契约回写 + 账本工程化

- `docs/reference/backend-pipeline.md` 补四节：**分类与改写分层**（M1，含数据流图与"计划缺失即内部错误"）、
  **错误分类学**（M2 映射表 + 构造器位置）、**事件表**（M3，守卫钉住）、**模块边界与规模棘轮**（M4）。
- 守卫扩展：现有 `pipeline_stages_match_doc` 之外，事件表与棘轮表同样由守卫钉住。
- 账本 §6 逐里程碑回写（实测数字、偏差、删除清单进度）；`CHANGELOG.md` 每里程碑一条；
  **v25 收口后把账本归档到 `docs/archive/`**（同 v23/v24 做法）并全量更新引用与文档地图。

## 3. 顺序与窗口

`M1`（最大风险，单独窗口）→ `M2` → `M3` → `M4` → `M5`。

M1 成功判据不达标（含"删除清单未清零"）即**停止推进**：如实记录偏差并重估，不得为推进而放宽判据或上调守卫预算。

## 4. 交付物与提交

1. 活账本 `docs/plans/backend-v25-plan.md`（方案原文 + §6 逐里程碑回写）——第一笔提交。
2. 代码：`legalize/classify.rs` + `plan.rs` 重写（M1）、错误分层 + `forge-ir` 构造器（M2）、
   `pipeline/events.rs`（M3）、模块切分（M4）、契约回写（M5）。
3. `CHANGELOG.md`：每里程碑一条 `### Changed (日期)`（含实测数字与命令）。
4. **每里程碑一次提交**（单 subject，`git commit -F target/tmp/*.txt`）。

## 5. 假设与不做清单

- **C ABI 不可破坏**：小聚合仍走寄存器档（win64/sysv64 ≤8B/≤16B、aapcs64 HFA、lp64d ≤2×XLEN）；
  M1 只改"决策从哪来"，**不改决策内容**（用对拍测试钉住）。
- **`forge-abi` 的规则/绑定数据面不动**（分类引擎本身不改；M1 只是"问一次、问全"）。
- **`dsl/codegen` 的 66 处生成物消息不在 v25 范围**（台账记 `待做` + 前置条件）。
- **能力缺口三条 `待做`（v24 台账遗留）不在 v25 范围**：`AGG_LOAD_USE`、`VA_ARG_VECTOR_MEM`、
  `VA_ARG_AGG_RULE`（后者前置条件 = 该 ISA 的聚合变参真执行通道）——v25 保证它们**仍被台账钉住**，不静默消失。
- `IrError` 不进二进制格式（实施首步核对；若进则升 `IR_FORMAT_VERSION`）。
- 假设既有 `place_addr_of_slot` / `Places` 写一次 / `shadow_agg` 三处 v24 契约不变（M1 只增消费）。

## 6. 批次记录（逐里程碑回写）

（立项时为空；每里程碑落地后在此追加：做了什么、实测数字、删除清单进度、偏差与下一批。）

## 7. 外部参考

- [ClangIR ABI Lowering Design Document](https://clang.llvm.org/docs/CIR/ABILowering.html)——
  **分类单一来源**（`ABIInfo` 在库里，rewriter 只消费分类结果）、"ABI lowering 作为独立 pass"、
  以及 `classification-attr` 注入式测试驱动（M1 守卫 4 的灵感）。
- [LLVM GlobalISel IRTranslator](https://llvm.org/docs/GlobalISel/IRTranslator.html)——ABI lowering 的
  位置与"输入高层签名、输出 ABI 化签名"的契约。
- [cranelift_codegen::machinst::abi_impl::ABIMachineSpec](https://docs.rs/cranelift-codegen/0.69.0/cranelift_codegen/machinst/abi_impl/trait.ABIMachineSpec.html)——
  每目标 spec × 通用实现的分层（与本仓 `forge-abi` 的规则/绑定/引擎三层同形，v25 不动这一层）。
