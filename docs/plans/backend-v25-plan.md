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

### M2 错误分类学 —— **一次落地**（M2 收口，2026-10-11，工作区）

**前提核对**：`IrError` **不进二进制格式**（`docs/reference/binary-format.md` 只在 API 签名里
提到它）⇒ 删变体**不需要**升 `IR_FORMAT_VERSION` ✔。

**做了什么**：

- **构造器下沉（通用性）**：`capability`/`gap_id` 从 `forge-codegen::pipeline::gaps` 移到
  **`forge-ir`**（`IrError::capability(id, msg)` / `IrError::gap_id()`）——`forge-isa-runtime`
  的 trait 默认实现也要能构造缺口错误。`pipeline/gaps.rs` 删除，**21 处调用点**机械迁移；
  自测随之下沉（`capability_errors_are_marked` 现在住在 `forge-ir`，并多加了两条"不该伪装成
  缺口"的断言：`Internal`/`Semantic` 的 `gap_id()` 都是 `None`）。
- **四分类落地**（41 处）：**畸形 IR / 内部不变量 ⇒ `Internal`（33 处）**（字段越界、常量池缺项、
  索引缺失、段值不足、IR 结构损坏…）；**使用/数据错误 ⇒ `Semantic`（5 处）**（约定未注册、
  JIT 装载 ISA 不匹配、管线未注册/工厂不匹配、约定把聚合判成标量栈落点）；**能力缺口 ⇒
  `capability(ID)`（3 处）**。
- **删掉第二套说法**：`IrError::Unimplemented` 变体 + `Display` 臂删除，全仓**零出现**；它的
  3 处调用点是 `TargetFrameLowering` 的 trait 默认实现（"这份谱没覆盖这个帧操作"）⇒ 转为能力缺口
  `FRAME_EMIT_EPILOGUE_JUMP` / `FRAME_SPILL_LOAD` / `FRAME_SPILL_STORE`（进台账：可搜索、可豁免、
  可追踪"哪份谱没接"）。
- **新增 5 条台账行**：`FRAME_*`×3、`SPEC_LOWERING_RULE_MISSING`（这份谱没有该 op 的降低规则——
  最经典的能力缺口之一）、`VECTOR_TIER_UNSUPPORTED`（宿主能力表里没有该宽向量档位，与
  `SIMD_WIDE_VECTOR_TIER`（**谱**声明的档位表）分工写明）。台账守卫的扫描面扩到
  `forge-isa-runtime/src`（两侧的缺口集合现在一起管）。
- **新增守卫** [`error_taxonomy`](../../crates/backend/forge-codegen/tests/error_taxonomy.rs)（2 条）：
  `unsupported_is_always_a_capability`（后端 + 运行时 **`src/`** 里不允许裸 `Unsupported(`，
  白名单条目必须被命中——`tests/` 允许构造负例）；`second_variant_for_not_supported_is_gone`
  （全仓无 `Unimplemented`）。
- **守卫抓出一处 M3 期的错误标注**（如实记录）：`va_expand` 里"宿主 plan 缺字段"的失败点
  借用了 `VA_ARG_VECTOR_MEM`（向量缺口）的 ID ⇒ 它其实是**输入/数据不一致**，改为 `Internal`；
  该 ID 现在只剩真正的失败点（向量结果缺 mem→向量搬移能力），台账那行的语义不再被稀释。

**实测**：`forge-codegen --lib` **2834 passed**（少的那 1 条随 `gaps.rs` 迁到 `forge-ir`）；
workspace 全量 exit 0；`error_taxonomy` 2 passed；`capability_gaps` 1 passed（含新扫描面）；
其余守卫与矩阵见下（同批实测）。

**下一步**：M3 观测事件化。

### M1 一次分类处处消费 —— 批次 3：**计划驱动改写 + 删五种枚举**（M1 收口，2026-10-11，工作区）

**做了什么**：

- **计划载荷换成规范类型**（删五种枚举 `ParamPlan`/`RetPlan`/`ArgPlan`/`CallResultPlan`/`VaArgPlan`
  及其三张边结构）：`AbiEdges { plans, shift, params: Vec<ParamEdge>, ret: Option<RetLoc>,
  call_args: Vec<CallArgEdge>, call_results: Vec<CallResultEdge> }`——里面装的是
  `forge-abi-model` 的 `ArgLoc` / `RetLoc`，本仓不再有自己的"计划枚举"。
  **稀疏载荷**取代"与形参一一对齐的表"（`ParamEdge{index, loc}` + `param_of(i)`）：
  对齐表要读者自己判"这一项是不是聚合"，稀疏表连序号都显式。
- **生产者改为只读计划**，而且**签名上就拿不到机器与约定名**——规则由类型系统保证：
  - `expand_large_agg_params(func, edges, agg_slots, shadow_agg)`（原 `machine, conv` 退场）；
  - `expand_sret_return(func, edges)`：`edges.ret` 是 `RetLoc::Indirect` 才算 sret；
  - `expand_large_agg_call_results(func, edges, agg_slots)`；
  - `expand_agg_call_args(func, edges, agg_slots, shadow_agg)`。
  驱动侧缺计划 ⇒ `IrError::Internal("合法化计划缺失（PlanPass 未跑）")`（`PlanPass` 恒排首位，
  已由 `driver_has_the_documented_pass_order` 钉住）。
- **按值查而不是按序号查**（实测踩到的坑，写进 `CallArgEdge` 文档）：`agg-call` 批次里
  `expand_large_agg_call_results` 会往 call 的实参表**前插** sret 缓冲指针，之后
  `expand_agg_call_args` 看到的 `op_idx` 比计划记录时**大 1** ⇒ 按序号查必然错位；
  值句柄不受插入影响（且"一次分类"保证同一值只有一种落点）。
- **两处"判据重复"收进计划相**（同一判据只有一处）：
  - "既不是 sret 也不是寄存器档" ⇒ 计划相报 `[cap:AGG_RET_PLACEMENT]`，pass 侧改为
    `IrError::Internal`（计划与 pass 不一致才是内部错误）；
  - "约定把聚合判成标量栈落点" ⇒ 计划相拒绝（数据错误），pass 侧同改为 `Internal`。
- **删掉两处冗余面**（都是"只在摘要里被读"的装饰）：
  - `AbiEdges.va_args`：`va_arg(聚合)` 的规则是**约定数据**（`VaInfo::arg_rules`），
    `va` pass 本来就在手边并自己 fail-closed ⇒ 计划不做"规则的第三份转录"
    （顺带 `plan_abi_edges` 少一个入参）；
  - `Classifier::ret_is_sret`：计划相直接看 `RetLoc`，布尔包装是重复面。
- **新增守卫** `edges_drive_the_rewrite`（第三条）：`Classifier::new` 只许出现在计划相
  （与定义处 `classify.rs` 及其单测）；`plan.rs`/`classify.rs`/`agg_param.rs`/`agg_call.rs`/`mod.rs`
  **必须**引用 `AbiEdges`——否则计划又沦为装饰。**两向都红**。
- **计划相单测更新**：`plan_is_read_only` 改按 `param_of(0)` / `Option<RetLoc>` 断言；
  新增 `one_plan_per_distinct_type`（形参与返回同型但各查一次 ⇒ `plans == 2`，把"引擎调用次数"
  变成可观测量）。

**实测**：`forge-codegen --lib --all-features` **2835 passed / 0 failed**；
`classification_single_entry` 3 passed；全量门禁见下（同批实测）。

**M1 删除清单**：手抄落点枚举与两个单类型探针 **清零**（守卫 `hand_copied_placement_is_gone` 钉住）；
五种计划枚举 **清零**；`plan_for_shapes` 只出现在三处白名单（守卫钉住）。

**下一步**：M2 错误分类学。

### M1 一次分类处处消费 —— 批次 2：**五处调用点全切 + 删除清单清零 + 两条守卫**（2026-10-11，工作区）

**做了什么**：

- `agg_param`（1 处）、`agg_call`（3 处：sret 探测 / call 结果 / 实参落点）全部切到 `Classifier`；
  `plan.rs` 两处 match 改为按 `forge-abi-model` 的 `Placement` 分流（`is_by_ref` / `StackByVal` /
  `Stack`（数据错误）/ 其余 ⇒ 寄存器档）。
- **分类器返回拥有权**（`arg_place -> Placement`、`ret_place -> RetLoc`）：`&mut self` 借出的引用
  会让同一个 `match` 里再问 `shift()` 冲突（实测 `E0502`），改为克隆（每形参/实参一次，代价可忽略）
  ⇒ 调用点不必关心借用次序。**这条写进了 `classify.rs` 的文档**。
- **删除清单清零**：`common.rs` 删除手抄落点枚举 `AggArgLoc` 与两个单类型探针
  （共 92 行）；全仓生产代码零引用（守卫钉住）。
- **新增守卫** [`classification_single_entry`](../../crates/backend/forge-codegen/tests/classification_single_entry.rs)：
  1. `classification_has_one_entry`：`plan_for_shapes` 只允许出现在
     `legalize/classify.rs` / `abi_setup.rs` / **`abi_target.rs`（引擎入口本身）**——**双向相等**
     （未列出的文件 = 分类分叉；列出的没出现 = 白名单死条目）；
  2. `hand_copied_placement_is_gone`：`AggArgLoc` / `agg_arg_place` / `agg_ret_is_sret`
     在全仓（含注释）零出现——守卫文件自跳过。
- **守卫当场抓到两处批次 1 未发现的遗漏**（如实记录）：
  1. `compiler.rs` 的 `#[cfg(test)]` 里有一个**很有价值**的 C1 不变量测试
     （aapcs64 的 24B **HFA** 必须走寄存器档、同尺寸非 HFA 必须按引用，"两者答案必须不同"
     才说明形状带了成员）仍在用已删类型 ⇒ **迁到分类器**（断言意图不变）；
  2. `abi_target.rs` 是管线的**引擎入口**（`impl AbiTarget` 的工厂闭包），不是消费者 ⇒ 进白名单并写明理由。
- **引用纪律**：5 处注释/文档里残留的旧符号名一并改写（`agg_param`、`classify`、`plan`、
  `forge-abi/src/rules.rs`、`read_path_budget` 的说明串）——`read_path_budget` 实测**无变化**
  （预算表不必改）。

**实测**：`forge-codegen --lib --all-features` **2834 passed / 0 failed**；
`classification_single_entry` 2 passed；`read_path_budget` 2 passed；
`clippy -p forge-codegen --all-targets --all-features -D warnings` 干净。

**下一批（批次 3，M1 收口）**：让生产者**读 `ctx.edges`**（而不是各自建分类器）⇒ 计划相成为
唯一的分类处、"一次分类"名副其实（现状：计划相与两个 pass 各自分类 ⇒ 同一类型一次编译最多算 3 次）；
同时把计划载荷里的五种枚举（`ParamPlan`/`RetPlan`/`ArgPlan`/`CallResultPlan`/`VaArgPlan`）
换成 `forge-abi-model` 的 `ArgLoc`/`RetLoc`（那 38 处引用是它们唯一的栖身地）；补
`edges_drive_the_rewrite` 守卫；跑全量门禁 + 两矩阵 + asm 三档 ⇒ M1 一次提交。

### M1 一次分类处处消费 —— 批次 1：**分类器落地 + 计划相接线**（2026-10-11，工作区）

**做了什么**：

- 新增 [`legalize/classify.rs`](../../crates/backend/forge-codegen/src/pipeline/legalize/classify.rs)：
  `Classifier`——**唯一**允许问引擎的通道。要点：
  - 返回 **`forge-abi-model` 的规范类型**（`ArgLoc` / `RetLoc` / `Placement`），不再手抄落点枚举；
    另配两个口径函数：`is_by_ref`（`Placement::Indirect`）与 `scalar_stack_is_a_data_error`
    （"约定把聚合判成**标量**栈落点" = 数据错误，**不是**能力缺口——v24 M3 的边界分类）；
  - **按类型去重**：同一类型在一个编译单元里只问一次引擎（旧实现每个使用点各问一次，每次重跑
    `plan_for_shapes` + 注册表读锁）；`plans()` 暴露引擎调用次数供守卫/诊断；
  - 形状一律走 `TyView::from_ir_type`（保留 C1 修复的不变量：成员必须摊开，否则 HFA 被当普通聚合）；
  - **请求形状与旧探针一致**（单类型请求）⇒ **决策内容不变**，本批只改"从哪来、问几次"；
    "整签名一次问全"（更贴合真实落点，尤其前面已有栈实参时的 `StackByVal.offset`）留作
    后续批次，届时以全量门禁 + 对拍为证据单独评估。
- `plan.rs` 的 `plan_abi_edges` 全部改走分类器（形参 / 返回 / 调用实参 / 调用结果四处），
  文档里的探针引用改指 `Classifier`。
- 两条单测：`classification_is_deduplicated`（同类型两次查询 ⇒ 引擎只调用一次；
  win64 的 24B 聚合实参是 `ByRef`）与 `ret_classification_matches_abi_model`（24B 返回是 sret）。

**实测**：`forge-codegen --lib --all-features` **2834 passed / 0 failed**（新增 2 条）。

**下一批（批次 2）**：把 `agg_param`（1 处）与 `agg_call`（3 处）也切到分类器 ⇒ 删除
`common.rs` 的两个探针与 `AggArgLoc`（删除清单清零）⇒ 加四条守卫。

### 立项（2026-10-11）

先落盘账本（本文），实现自下一轮开始；侦察证据见 §1，删除清单与守卫见 §2。

## 7. 外部参考

- [ClangIR ABI Lowering Design Document](https://clang.llvm.org/docs/CIR/ABILowering.html)——
  **分类单一来源**（`ABIInfo` 在库里，rewriter 只消费分类结果）、"ABI lowering 作为独立 pass"、
  以及 `classification-attr` 注入式测试驱动（M1 守卫 4 的灵感）。
- [LLVM GlobalISel IRTranslator](https://llvm.org/docs/GlobalISel/IRTranslator.html)——ABI lowering 的
  位置与"输入高层签名、输出 ABI 化签名"的契约。
- [cranelift_codegen::machinst::abi_impl::ABIMachineSpec](https://docs.rs/cranelift-codegen/0.69.0/cranelift_codegen/machinst/abi_impl/trait.ABIMachineSpec.html)——
  每目标 spec × 通用实现的分层（与本仓 `forge-abi` 的规则/绑定/引擎三层同形，v25 不动这一层）。
