# 后端编译管线契约

> 状态：**[active]**（2026-10-11 起；v25 收口时回写）。本文是后端的**现行契约**：编译入口、
> 管线阶段、合法化两相、**一次分类处处消费**、值描述单一来源、能力缺口台账、**错误分类学**、
> **模块边界与规模棘轮**、观测开关与事件表（v23/v24 的收口记录见
> [`archive/backend-v23-plan.md`](../archive/backend-v23-plan.md) 与
> [`archive/backend-v24-plan.md`](../archive/backend-v24-plan.md)，均已归档）。
>
> 术语：本文的"后端"= IR 之后的全部阶段（合法化 / 指令选择 / 寄存器分配 / 帧布局 /
> 发射 / 装载）。ISA-DSL 语法、生成的指令编码与汇编器见 [`isa-dsl.md`](isa-dsl.md)；
> 调用约定数据与引擎见 [`calling-conventions.md`](calling-conventions.md)；
> **能力缺口台账**见 [`backend-gaps.md`](backend-gaps.md)。
>
> 四条"别绕过"的纪律：① 编译只有**一个入口**（§1）；② 合法化只有**一个改写者**
> （物化相，§3）；③ ABI 落点只有**一个来源**（计划相，§6）；④ 值只有**一份描述**（§4）。
> 每节都有一条守卫钉着，改代码就同时改本节。

## 1. 编译入口：只有一份实现

宿主面**只有一个编译类型** `forge_codegen::CompileSession`；编译阶段只有一处实现
（`pipeline/compiler.rs` 的单函数管线 + `pipeline/session.rs` 的模块编排）。

| 入口 | 用途 | 说明 |
| --- | --- | --- |
| `CompileSession::new(machine)` | 单函数 | `compile(&Function)` / `compile_with_alloc(&Function)` |
| `CompileSession::for_module(machine, &Module)` | 单函数或整模块 | 从模块取签名表装上（§6） |
| `CompileSession::with_module_sigs(ModuleSigs)` | 显式装表 | 表由 `Module::signature_table()` 给出 |
| `CompileSession::with_opt_level(level)` | 前置 IR 优化 | 不设置 = 不优化；**没有**"绕过优化"的旁路入口 |
| `CompileSession::add_function(name, sig, build_fn)` | 单函数便捷 | 与 `compile` 同源，认会话上的签名表 |
| `CompileSession::compile_module(&Module)` | 整模块 | 返回 `CompiledModule`：全局数据段与双符号 + 逐函数（≥8 个函数走 scoped 并行）；函数按 `FuncRef` 序 |

`CompiledModule` 是**纯数据且类型无关**（机器码 + 符号 + 数据段，不含机器实例、不含可执行内存）：
`isa()` / `function_count` / `function(name)` / `iter_functions` / `globals` / `data_segments` /
`into_parts`。`isa()` 是目标 ISA 名标签——`JitLoader::install` 拿它与自己的机器核对，
**不匹配即 fail-closed**（把 x86 的码装进 riscv 的装载器会"装载成功"然后跑出任意行为）。

**装载（JIT）** 在 `JitLoader`（`runtime/jit.rs`）：`new` / `install(CompiledModule)` /
`get_fn::<F>` / `register_external` / `lookup_symbol` / `set_symbol_resolver`。
`JitLoader::add_function` 与 `JitLoader::compile_module` 是**薄委托**（编译一律经
`CompileSession`，装载串行按 `FuncRef` 序）——保留在本类型上是为了既有宿主的调用面
不必被迫重写；编译实现不在这里。

## 2. 管线阶段

下表由守卫 `pipeline_stages_match_doc` 与 `pipeline::STAGES` **集合相等**钉住
（改了阶段就同时改表：多一个、少一个都红）。

| 阶段 | 做什么 |
| --- | --- |
| `legalize` | 合法化（**两相**：计划相只读算计划 → 物化相按指令序单遍改写，见 §3） |
| `isel` | 指令选择（lowering：生成物的 `lower_inst` + 宿主的辅助发射） |
| `regalloc` | 寄存器分配（活跃区间 + 溢出与回填） |
| `frame` | 帧布局（槽位/对齐/帧尺寸/ABI 槽） |
| `emit` | 发射（序言 / 指令 / 尾声 / fixup 解析） |
| `jit` | 装载（JIT：可执行内存 + 符号解析） |

每个阶段的诊断开关见 §7；观测开关**不可能静默失效**（那是这一节存在的理由）。

## 3. 合法化：两相 + pass 清单

聚合合法化从"每个 pass 就地改写 IR 并各自重写使用处"改成**两相**（v24 M1）：

* **计划相**（`pipeline/legalize/plan.rs`，pass 名 `plan`）：**只读地**算出整函数的
  ABI 边界计划（形参 / 返回 / 调用实参 / 调用结果 / `va_arg` 五类落点，**完整描述**，
  不需要动的也以 `Keep` 入计划）。落点一律问 `forge-abi` 的探针（`agg_arg_place` /
  `agg_ret_is_sret` / 变参形状的 `arg_rules`）——**管线里没有约定名分支**。
  守卫 `plan_is_read_only`：跑完计划相后 IR 的指令/值/常量数**逐项相等**（决策不改写）。
* **物化相**（`pipeline/legalize/materialize.rs`，pass 名 `materialize`）：按指令序**单遍**
  处理，逐块读快照 → 为每条指令算出要插入的生成指令 → 拼成新的 `inst_order` **一次写回**
  ⇒ 生成指令**天生排在消费者之前**。守卫 `materialize_is_idempotent`（第二遍零改动）与
  `materialized_load_precedes_its_consumer`（逐块核对"每条 `Copy` 的操作数定义排在它之前"）。

**位置的唯一表示**是 `legalize::agg_expand::Places`（`Value → Place::{Mem(addr), Segs(值)}`，
**写一次**：同值二次登记在 debug 下 panic）。它是 pass 之间**唯一**的"值住哪"通道。

pass 清单与**合并次序**（下表与 `LegalizeDriver::with_aggregates().pass_names()` 相等，
由 `legalize_passes_match_doc` 钉住）：

| pass | 职责 |
| --- | --- |
| `plan` | 计划相（只读） |
| `va` | 变参展开（要等 ABI 槽排定） |
| `agg-param` | 聚合形参：建槽/登记位置（**先于**存储与实参） |
| `agg-store` | 聚合字面量 store → 打包常量 store |
| `agg-call` | 调用侧：sret 形参 / 结果拆段 / 实参落点 |
| `agg-leftover` | 大聚合 `load` 值传播（**最后**于实参之后） |
| `materialize` | **物化相**：按指令序单遍改写（唯一的改写者） |
| `gep` | GEP 展开 |

**入口按三批跑**（`with_aggregates_except_va` → 排 ABI 槽 → `va_only` → `finalize`），
理由：ABI 槽必须看到聚合族新建的 `Alloca`；`Va` 自己也会登记位置（它把聚合实参拷进
自己的帧槽）⇒ 物化必须等它。分批清单由 `driver_has_the_documented_pass_order` 钉住。

**聚合与变参的表示契约**（对外可见的部分）：

* **C ABI 是硬的**：小聚合仍走寄存器档（win64/sysv64 ≤8B/≤16B、aapcs64 HFA、lp64d ≤2×XLEN）
  ——两相化只改**内部表示**，不改 ABI。
* **`>8B` 大聚合**的落点由约定数据决定，两端共用同一份计划：`ByRef`（指针指向调用方副本，
  win64 与 aapcs64/lp64d 的 24B 形参）、`StackByVal`（整块进传出区，sysv64）、
  `Registers`（拆段）。落点探针失败/未覆盖 ⇒ **fail-closed**（`[cap:AGG_*]`，见 §5）。
* **被重映射的值仍代表聚合**时（by-ref 形参重建签名后旧值被全局替换成 `ptr`），
  "这个 `ptr` 代表 T 型聚合"由 `LegalizeCtx::shadow_agg` 交给物化相（**唯一**这类事实交接）。
* **变参**：`va_arg(聚合)` 的形状必须由约定声明（`va_list` 形状的 `aggregate_arg` +
  `indirect` 阈值）；未声明 ⇒ `[cap:VA_ARG_AGG_RULE]`（现状：aapcs64/lp64d 未声明，
  台账 §3.1 有实测与前置条件）。段值档的嵌套聚合字段没有地址 ⇒ 由
  `memoryize_from_segs` 开槽写入（`off % 8 != 0` 用移位拼接）。

## 4. 值描述：单一来源

"值的类 / 位宽 / IR 类型"只有**一份描述** `forge_isa_runtime::ctx::ValDesc { class, bits, ty }`，
只有两个产生点：

* 写：`LowerCtx::set_value_desc(xreg, ty)` —— 类与位宽都**从 IR 类型推导**（唯一推导处）；
* 读：访问器 `desc_of` / `is_fp_value` / `is_vector_value` / `value_bytes` / `type_bits_of`
  ——表本身**私有**，生成物**不再依赖任何"值→类型"旁表**。

守卫 `only_one_value_desc_table`：`LowerCtx` 里值描述表**恰好一张**、值类型是 `ValDesc`、
表不得 `pub`，且旧的平行表名在全仓生产代码里零出现。**别用 `ValDesc::bytes()` 代替
`value_bytes()`**：前者由位宽向上取整，对"存储尺寸 ≠ 标量位宽"的类型（如 80 位扩展浮点
存 16 字节）会给出不同答案。

## 5. 能力缺口：台账 + 双向守卫

"这台机器 / 这份约定 / 这份谱做不了什么"不是散在代码里的字符串，而是**可机读的集合**：

* 代码侧：`pipeline::gaps::capability(ID, msg)` ⇒ 失败信息以 **`[cap:ID]`** 开头
  （`gap_id(&IrError)` 取 ID）；**边界**：畸形 IR / 使用错误 / 约定数据自相矛盾**不进**台账；
* 文档侧：[`backend-gaps.md`](backend-gaps.md) —— 每行 = 缺口 / 决策（已实现·数据层·
  不做·待做）/ 钉住它的守卫或数据条件；
* 守卫 `capability_gaps_guard`：两侧**双向逐一相等**，每个 ID 在源码里恰好一处。

## 6. 模块签名表与擦除入口

调用指令只带一个 `FuncRef`，而"被调方是不是变参"只写在**被调方的签名**里——所以编译
**整个模块**的宿主必须把这份事实交给编译器：类型 `forge_ir::ModuleSigs`（逐项
`FuncSigEntry { variadic, fixed_args }`，由 `Module::signature_table()` 产出，按
`FuncRef::index()` 对齐）；传递 `CompileSession::with_module_sigs` / `for_module` →
`LowerCtx::conv.module_sigs`；读口子 `LowerCtx::conv.variadic_of(FuncRef)`。
两条纪律：① 表**只从模块取**，别在宿主侧另抄一份；② 单函数入口也要带上已知的表。

生成的 ISA 模块导出 `ERASED` 面：`ErasedTargetMachine::compile(&Function)` 按 ISA 名查
`forge_isa_runtime::register_pipeline` 登记过的工厂；**未注册 ⇒ `IrError::Unsupported`**
（fail-closed，不 panic），消息点名 ISA 与"需要宿主注册"。宿主侧登记在
`forge_codegen::pipeline_hooks::ensure_registered()`（进程内一次），`CompileSession::new`
会调它。

## 7. 观测开关（诊断面台账）

**一张表管两侧**（v26 M1）：后端与 `forge-rustc` 的 `FORGE_TRACE_*` 都登记在
`forge_isa_runtime::diagnostics::KNOBS`（本表 ↔ 该台账 ↔ 源码字面量三方双向相等，守卫
`trace_knobs_are_bijective`）；下表共 **42** 项。

诊断开关**不可能静默失效**：`forge_isa_runtime::diagnostics::knob("FORGE_TRACE_*")` 直接按
名字问环境变量（**任何名字都生效**；`forge-rustc` 侧写 `trace_enabled("后缀")`，同源转发；
`pipeline::trace` 只是转发层），`diagnostics::KNOBS` 是**唯一登记处**，只回答"我们有哪些观测
开关、各自看什么"。忘登记的名字：debug 下 `debug_assert!` **当场 panic**，release 下照常
生效但 `eprintln` 一次警告。

为什么值得一节：旧实现维护一份开关白名单，**名字漏进白名单 ⇒ 该开关恒为假、静默失效**
——`FORGE_TRACE_AGG` 就这样死了几个月（"文档承诺可见、实际看不见"），M4 落地时又抓到第二个
同类 `FORGE_TRACE_M4`（调用点在、白名单里没有），而白名单里的 `FORGE_TRACE_ISEL` 一处调用点
都没有（死条目）。下表与源码字面量、与 `KNOBS` 台账由守卫 `trace_knobs_are_bijective` 钉成
**三方双向相等**（多一行、少一行都红）；v26 M1 起 `forge-rustc` 的 27 项也在同一张表里。

| 开关 | 看什么 |
| --- | --- |
| `FORGE_TRACE_ABI` | 调用约定规划与落点回填（forge-codegen abi_setup/compiler；forge-rustc FnAbi 形态） |
| `FORGE_TRACE_AGG` | 聚合展开与合法化（legalize 各 pass / 计划相 / 物化相） |
| `FORGE_TRACE_ALLOC` | 寄存器分配主循环（regalloc_bt） |
| `FORGE_TRACE_CALLCLOBBER` | 调用点的约定级破坏集合并（pipeline/clobbers） |
| `FORGE_TRACE_ARGS` | 调用实参打包明细（forge-rustc mod.rs） |
| `FORGE_TRACE_ASSERT` | 编译期断言与 const 求值路径（forge-rustc） |
| `FORGE_TRACE_BC` | 机器码字节 dump（forge-rustc compile.rs；配 llvm-objdump 对照） |
| `FORGE_TRACE_BLOCK` | 基本块 lowering 边界（forge-rustc mod.rs） |
| `FORGE_TRACE_CALL` | 直接调用：callee 签名与实参打包（forge-rustc mod.rs） |
| `FORGE_TRACE_CAST` | 类型转换 lowering（forge-rustc） |
| `FORGE_TRACE_CGU` | codegen unit 划分（forge-rustc） |
| `FORGE_TRACE_CODE_HEX` | 函数机器码 hex dump（forge-codegen compiler；`FN_SIZE` 的细节档） |
| `FORGE_TRACE_CONST` | 常量求值（forge-rustc const_eval.rs） |
| `FORGE_TRACE_DISCR` | 判别式（discriminant）取值与布局（forge-rustc） |
| `FORGE_TRACE_DW` | DWARF 调试信息生成（forge-rustc dwarf.rs） |
| `FORGE_TRACE_EMIT` | 机器码发射（emission；`[machine.text]` 相关诊断也归此） |
| `FORGE_TRACE_EXPIRE` | 活跃区间到期处理（regalloc_bt） |
| `FORGE_TRACE_FIELD` | 字段偏移计算（forge-rustc mod.rs） |
| `FORGE_TRACE_FN` | 每个函数的 lowering 开始（forge-rustc mod.rs） |
| `FORGE_TRACE_FN_SIZE` | 每个函数的入口/体积摘要（forge-codegen compiler） |
| `FORGE_TRACE_GLOBAL` | 全局与重定位符号解析（forge-rustc） |
| `FORGE_TRACE_IR` | IR 构建期 dump（forge-ir builder —— 该层不允许依赖运行时，故实现在本 crate 内） |
| `FORGE_TRACE_LIVE` | 活区间全量 dump（forge-codegen liverange；调 regalloc 用） |
| `FORGE_TRACE_LOAD` | load lowering（forge-rustc） |
| `FORGE_TRACE_LOC` | 源码位置/行号映射（forge-rustc mod.rs） |
| `FORGE_TRACE_LOWER` | 指令选择（lowering；每指令一条） |
| `FORGE_TRACE_M4` | 绑定折入与多类槽回填类诊断（compiler） |
| `FORGE_TRACE_META` | 实例元数据（forge-rustc） |
| `FORGE_TRACE_MIR` | 每条 MIR statement（forge-rustc mod.rs） |
| `FORGE_TRACE_PAIR` | ScalarPair 判定与偏移（forge-rustc） |
| `FORGE_TRACE_PLACE` | place 投影 lowering（forge-rustc place.rs） |
| `FORGE_TRACE_REGALLOC` | 寄存器分配编排（regalloc_bt 入口） |
| `FORGE_TRACE_SLOT` | 栈槽分配（forge-rustc mod.rs） |
| `FORGE_TRACE_SPILL` | 溢出与回填（emission / regalloc_bt） |
| `FORGE_TRACE_STACK` | 栈槽与帧布局（lowering） |
| `FORGE_TRACE_TIMING` | 编译阶段计时（`[codegen 名字] total/lower/…/emit`；基准用；原名 `CF_CODEGEN_TIMING`） |
| `FORGE_TRACE_STMT` | MIR statement 级跟踪（forge-rustc） |
| `FORGE_TRACE_STORE` | store lowering（forge-rustc） |
| `FORGE_TRACE_SUBSTS` | 泛型替换解析（FnDef → instance，forge-rustc mod.rs） |
| `FORGE_TRACE_SYM` | 符号名计算（forge-rustc mod.rs） |
| `FORGE_TRACE_TERM` | 终结符 lowering（forge-rustc mod.rs） |
| `FORGE_TRACE_VAR` | 局部变量与调试变量（forge-rustc mod.rs） |
| `FORGE_TRACE_VCODE` | 虚拟指令流（compiler） |

性能：每个开关一个 `OnceLock` 缓存（首次查询后零开销）——regalloc/lowering/emission 的
**每指令热路径**上都有查询，实测裸 `std::env::var_os` 会占 codegen 的 7–18%。

## 8. 一次分类，处处消费

ABI 落点**只有一个来源**：合法化计划相（`legalize/plan.rs`）。它只读地算出 `AbiEdges`，
物化与各 `pass` **只消费**它。

```text
                 ┌──────────────── 唯一分类处（只读）────────────────┐
   IR + 约定 ──► │ legalize/classify.rs  Classifier                 │ ──► AbiEdges
                 │   · 唯一问 forge-abi 引擎的通道（plan_for_shapes）│     · params / ret
                 │   · 按类型去重（同一类型只问一次）                │     · call_args / call_results
                 │   · 返回规范类型 ArgLoc/RetLoc/Placement          │     · shift / plans（诊断）
                 └──────────────────────────────────────────────────┘
                                        │ 只读
                    agg-param ─ agg-store ─ agg-call ─ agg-leftover ─ materialize ─ gep
```

契约要点：

* **一个入口**：问引擎只经 `Classifier`（`plan_for_shapes` 只允许出现在 `classify.rs`、
  `abi_setup.rs`、`abi_target.rs`（引擎入口本身）三处）。
* **规范类型**：计划里装的是 `forge-abi-model` 的 `ArgLoc`/`RetLoc`/`Placement`——本仓**不再有**
  自己的"计划枚举"或落点投影（旧实现的手抄枚举与两个单类型探针已删除，守卫钉住零出现）。
* **生产者签名里没有机器与约定名**：`expand_large_agg_params` / `expand_sret_return` /
  `expand_large_agg_call_results` / `expand_agg_call_args` 只拿 `&AbiEdges`——规则由**类型系统**
  保证（想再问一次引擎，得先把机器传回去）。计划缺失 ⇒ `IrError::Internal`（`PlanPass` 恒排首位）。
* **按值查，不按序号查**：`agg-call` 批次里 `expand_large_agg_call_results` 会往实参表**前插**
  sret 缓冲指针，之后的 `op_idx` 比计划记录时大 1 ⇒ 计划按**值句柄**查（`call_arg_of(inst, val)`）。
* **不登记 `va_arg`**：那一条的规则是**约定数据**（`VaInfo::arg_rules`），`va` pass 本来就在手边
  并自己 fail-closed——计划只描述**物化相要改写的边界**。

守卫 = `classification_single_entry`（三条：唯一入口双向相等、生产者必须引用计划且只有计划相建
分类器、旧符号零出现）。

## 9. 错误分类学：失败语义分层

`IrError::Unsupported` 曾经一名三用（能力缺口 / 畸形 IR / 使用错误），三者对用户的意义完全不同。
现在**冻结四分类**，`forge-codegen` 与 `forge-isa-runtime` 的 `src/` 里不许出现裸 `Unsupported(`：

| 类别 | 变体 | 判据（谁来解） |
| --- | --- | --- |
| `capability` | `IrError::Unsupported` + **`[cap:ID]`**（`IrError::capability(id, msg)`） | 换约定 / 补规则 / 换 ISA 就能解 ⇒ 进 [`backend-gaps.md`](backend-gaps.md) 台账 |
| `internal` | `IrError::Internal` | 畸形 IR / 内部不变量被破坏 ⇒ 报 bug 或修输入 |
| `semantic` | `IrError::Semantic` | 使用或数据错误（约定未注册、装载 ISA 不匹配、约定数据自相矛盾） |
| `other` | 现成变体（`Emit`/`RegAlloc`/`Verify`/`UnknownOpcode`…） | 各阶段自己的失败，保持原样 |

* 构造器住在 **`forge-ir`**（`IrError::capability` / `IrError::gap_id`）——`forge-isa-runtime`
  的 trait 默认实现也要能构造缺口错误（"这份谱没覆盖这个帧操作"就是缺口，不是"未实现"）。
* **缺口 ID 一律以字面量写在构造点**：台账守卫靠字面量识别失败点，把 ID 变成变量会让台账行被判成
  **死行**（实测撞到过）。
* `IrError::Unimplemented` 已删除（与 `Unsupported` 语义重叠：第二套说法）；全仓零出现。

守卫 = `error_taxonomy`（后端 + 运行时的 `src/` 无裸 `Unsupported(`；全仓无 `Unimplemented`）
加 `capability_gaps_guard`（台账与源码缺口集合双向相等）。

## 9b. 标量批：宽值的多寄存器表示（v27 M5）

**触发**：函数里出现 `bits > machine.int_widths().max()` 的**整数标量**（值或块参数）。
`CompileSession` 只在"变参/聚合"时为函数建可变副本，因此用
`legalize::needs_scalar_legalize()` 单独判断并补建（此前普通函数**从不跑 legalize**，
`wide_split` 形同虚设 —— v27 M5 实测踩了十几轮才定位）。

**表示**：一个宽值 = **n 个原生宽 SSA 值**（肢，`n = ceil(bits/W)`，下标 0 = 最低肢）。
`n = 2` 即 i128 形态；`i100`（非字宽整数倍）靠**顶肢规范化**处理；`i256`（4 肢）同样适用。
isel 因此**永远看不到** 16 字节标量（它没有多寄存器表示，`bitcast [i64;2] ↔ i128` 也没有
lowering，实测 `[cap:SPEC_LOWERING_RULE_MISSING]`）。

**pass 顺序**：`scalar() = [wide_split, int_width]`。
`wide_split` 只产出原生宽值；`int_width` 的 R1/R2 再处理 `i3/i7` 等非原生**窄**位宽
（`i1` 是 bool，不参与升位）。

**覆盖**：`Iconst`（≤128 走整数池、>128 走 `Big` 取肢）、`Iadd/Isub`（进位链 `icmp Ult`）、
`Band/Bor/Bxor/Bnot`、`Select`、`Icmp`（n 肢字典序，**标志走 i64 通道**）、`Ireduce`、
`Sextend/Uextend`、`Load/Store`（n 次窄存取）、`Ishl/Ushr/Sshr`（常量 → O(n) 快路径；
运行期 → O(n²) select 链）、`Imul`（子肢梳状，`s = W/2`）。

**未覆盖 ⇒ 明确 fail-closed（`WIDE_INT_SHAPE`，单一触发点）**：宽值**块参数（phi）**、
`Div/Rem`、函数级宽形参/宽返回（边界由前端用指针/byref/sret 处理，IR 里不该出现）。

**两条实测踩过的坑（都写进代码注释）**：
① 不动点循环必须记"已处理指令"，否则重复处理 ⇒ 死循环；
② `store` 的肢改写是**就地**的（原指令改成"存最后一个肢"）⇒ **不能**再把它 Nop 掉
（否则高位肢根本没写进内存；症状是所有"从内存读 i128 再算"的用例错值）。

## 10. 模块边界与规模棘轮

大文件不是风格问题，是**读改成本**：立项时 `compiler.rs` 1616 行、`regalloc_bt.rs` 1544、
`va_expand.rs` 1113、`agg_call.rs` 690。v25 M4 按职责切成目录——**模块路径不变**
（`pipeline/compiler/mod.rs` 取代 `pipeline/compiler.rs`），所以调用点零改动。

| 目录 | 各文件职责 |
| --- | --- |
| `pipeline/compiler/` | `mod.rs` 会话入口与模块编排；`state.rs` `CompileState` + 单函数管线；`alloc_integration_tests.rs` 集成用例 |
| `pipeline/regalloc_bt/` | `mod.rs` 分配器与 `Default`；`state.rs` `BtState` 状态机；`spill.rs` 溢出编辑；`tests.rs` / `clobber_map_tests.rs` 用例 |
| `pipeline/va_expand/` | `mod.rs` `AbiSlots` 与模块文档；`slots.rs` ABI 槽规划；`expand.rs` 展开主体；`tests.rs` 用例 |
| `pipeline/legalize/agg_call/` | `mod.rs` sret 形参入口；`args.rs` 实参展开；`results.rs` 结果拆段与宽聚合返回 |

两条纪律：

* **叶子方向**：`legalize/**`、`va_expand/**`、`regalloc_bt/**` 只能被驱动调用，**不得**依赖
  `compiler`/`session`——`pass` 的次序与状态只由驱动持有（v23 那四个缺陷同源）。
  `emission`/`frame_layout`/`lowering` 例外：它们按参数拿 `&CompileState`（不是反向拥有）。
* **规模棘轮**（只许按实测**下调**，要放行更大的文件必须进表并写明理由）：

| 文件 | 上限（行） | 为什么允许这么长 |
| --- | --- | --- |
| `pipeline/regalloc_bt/state.rs` | 944 | `BtState`：13 字段 + 17 方法构成一台状态机，切开会把"谁改了哪个字段"拆散 |
| `pipeline/lowering.rs` | 835 | 生成物 lowering 的宿主侧辅助，与谱面规则一一对应 |
| `pipeline/legalize/materialize.rs` | 657 | 物化相：按 IR 形态分段单遍改写，切开反而难对照 |
| `pipeline/va_expand/expand.rs` | 621 | `expand_start`/`expand_arg` 共享局部不变量（游标推进次序） |
| `pipeline/compiler/mod.rs` | 640 | 会话入口 + 模块编排（state 与测试已切出；+宽值批调用点 v27 M6-84、+溢出事件 v27 M6-87） |
| `pipeline/compiler/alloc_integration_tests.rs` | 690 | 集成用例（测试长不构成阅读负担，与实现同源；+栈探测正反用例 v27 M6-78） |

缺省上限 **700** 行；上表与守卫里的预算表由 `file_size_ratchet` 钉成**双向相等**（改动其一就红）。

**测试专属模块的识别**（给两条扫描型守卫用）：`#[cfg(test)] mod X;` 声明出来的文件整份跳过——
M4 把测试搬成独立文件后，文件内不再有 `#[cfg(test)]` 可供"截断扫描"。识别方式是**向上找声明它的
`mod.rs`**，看那条 `mod X;` 是否带 `#[cfg(test)]`（比按文件名猜精确，也盖得住改名）。

## 11. 事件表（codegen 的机读出口）

诊断开关给的是**人读**的散文；CI 要的是**机读**的一行 JSON。出口 = 环境变量
**`FORGE_CODEGEN_EVENTS=<文件>`**（追加写；未设置 ⇒ 零开销返回；**写失败静默**——观测不得
影响编译结论，与 `FORGE_JIT_EVENTS`/`FORGE_ASM_EVENTS`/`FORGE_E2E_EVENTS` 同约定）。
字段序固定（调用点给的顺序即输出顺序）⇒ 输出确定，测试可整行比对。

为什么值得一节：JIT / asm / E2E 三个面早就有落盘事件，**codegen 曾经是唯一没有机读出口的
阶段**——CI 只能靠正则去啃散文。下表与 [`events::EVENTS`] 台账、源码调用点由守卫
`event_names_are_bijective` 钉成**三方双向相等**（比 v24 M4 的两方多钉一层文档）。

| 事件 | 什么时候发 / 带什么字段 |
| --- | --- |
| `COMPILE-START` | 开始编译一个函数（`fn`） |
| `COMPILE-DONE` | 函数编译成功：`total_us` + 各阶段微秒（`blocks_us`/`vreg_us`/`lower_us`/`peep_us`/`regalloc_us`/`frame_us`/`emit_us`）+ `code_bytes` |
| `CAPABILITY-GAP` | 编译失败：`gap` = 缺口 ID（非缺口错误时为空串）+ `fn` |
| `LEGALIZE-PLAN` | 合法化计划相完成：`actionable`（可动边界条数）、`plans`（问引擎次数） |
| `ALLOC-SUMMARY` | 寄存器分配完成：`assigned`、`spills`、`edits` |
| `SPILL-VALUE` | 溢出决策（逐值）：`vreg`、`offset`（有符号帧偏移）、`size`——v27 M6-87 起，P4 事件化 |
| `SPILL-EDIT` | 逐指令溢出编辑：`inst`（vcode 全局序号）、`kind`（reload/bind/store）、`offset`、`scratch`、`width`——v27 M6-90 起，P4 事件化 |
| `EMIT-SUMMARY` | 发射完成：`code_bytes`、`relocs` |

## 12. 事件通道（四个出口，一处实现）

四个通道**共用一处落盘实现**（`forge_isa_runtime::diagnostics`：唯一的环境变量查表 + 开文件 +
追加 + **写失败静默**），各自只决定**行的格式**。未登记的通道名 fail-closed（debug 下当场 panic、
release 下警告一次并放弃写入）。下表与 `diagnostics::CHANNELS`、源码调用点由守卫
`event_channels_are_bijective` 钉成**三方双向相等**。

| 通道 | 环境变量 | 格式与用途 |
| --- | --- | --- |
| `codegen` | `FORGE_CODEGEN_EVENTS` | **JSON 行**（`{"event":…}`）：编译管线事件（见上表） |
| `jit` | `FORGE_JIT_EVENTS` | 文本 `<事件> <用例>`：JIT 矩阵可见性（skip 与真跑过在日志里同形） |
| `asm` | `FORGE_ASM_EVENTS` | 文本 `<事件> <说明>`：汇编语料测试台（解析桶/棘轮/SKIP 原因） |
| `e2e` | `FORGE_E2E_EVENTS` | 文本 `[<事件>] <用例> <细节>`：forge-rustc e2e（RETRY/KNOWN/AV 探针） |

`forge-rustc` 的编译期跟踪开关（`FORGE_TRACE_*`）用**同一张**开关台账（§7 的 `KNOBS` 下沉到
`forge-isa-runtime::diagnostics`），因此"漏登记导致开关静默失效"这一类问题在两侧都不再可能。

## 13. v27 增量：宿主伪 op / 运行时注入 / 内联汇编

v27（`docs/plans/forge-rustc-v27-plan.md`）往管线里加了三处**宿主侧**能力。它们都不是
新的机器能力，而是"把编译期已经确定的东西变成 IR 或数据"——这里记契约，避免下次改它们时
再摸一遍。

### 13.1 内联汇编（`Opcode::InlineAsm`，v27 M2.5/S1）

* **IR 形态**：`operands = 0`、`results = 0`、`side_effect = true`、`type_rule = "none"`；
  汇编文本放在**唯一一个** `Immediate::String` 里（`builder::inline_asm(&str)`）。
* **展开时机**：降级期（`pipeline/inline_asm.rs::expand_inline_asm`）。把文本交给
  **本 ISA 的汇编器**（`CompileState::asm_parse`，由 `CompileState::new` 从
  `machine.assembler()` clone），得到 `Vec<I>` 后连同与 vcode 平行的
  `inst_ops`/`inst_clobbers` 推进当前虚拟块。
* **文本从哪里取**：`ctx.type_store`（`CompileState::new` 早已放进去的 `TypeStore` 快照）
  的 `lookup_str` ⇒ **不需要**额外的字符串表通道。
* **fail-closed**：本 ISA 无汇编器 / 文本解析失败 / `InlineAsm` 没带 `String` 立即数 ⇒
  明确 `Unsupported`；`forge-opt` 的 `const_fold` 对它是**穷尽 match** 的一条
  （`InlineAsm => Ok(None)`，不透明文本永不折叠）。
* **边界（S1 口径）**：只放行**无占位符的纯文本**。寄存器/`const`/`sym` 约束、标签、
  `options(noreturn)` 由 `forge-rustc` 侧继续 `[cap:INLINE_ASM]`——它们的瓶颈在
  **IR 缺"操作数 ↔ 物理寄存器"的表达**（S2+），不是文本能否拿到。

### 13.2 运行时注入（alloc / TLS）

后端会在**首个 CGU 对象**里注入运行时函数（`writer.add_function`），它们都是普通 IR 函数：

| 运行时 | 何时注入 | 关键契约 |
| --- | --- | --- |
| `__rust_alloc` 一族（`alloc_runtime.rs`） | `tcx.crates(())` 里**没有** `std` 时 | 链 `std` 时 std 自带那一族，再注入就是 LNK2005（`WA-02`） |
| `__forge_tls_addr`（`tls_runtime.rs`） | 计划里**真有**线程局部静态时 | 每静态两个隐藏数据符号（初值模板 / 索引槽）；`TlsAlloc` 索引 + 1 存槽（0 = 未分配）；每静态独占一个索引、无析构 |

两条共同纪律：**按需**（不产出死代码）、**坐标固定**（注入点在首个 CGU，符号名确定性，
降级侧不查表）。

### 13.3 `abort()`

`core::intrinsics::abort()` ⇒ `ExitProcess(3)`（无 `asm!` 可用时的唯一正确出口）；
其后的 `unreachable()` 由调用点（`Call` 终结符的 `target: None` 分支）发。

## 14. 相关账本与下一步

* 能力缺口：[`backend-gaps.md`](backend-gaps.md)（含逐条决策证据）。
* v25 逐批证据与删除清单：[`archive/backend-v25-plan.md`](../archive/backend-v25-plan.md)（已归档）。
* v24 逐批证据：[`archive/backend-v24-plan.md`](../archive/backend-v24-plan.md)（已归档）；
  v23 收口记录：[`archive/backend-v23-plan.md`](../archive/backend-v23-plan.md)（已归档）。
