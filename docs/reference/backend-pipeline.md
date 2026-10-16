# 后端编译管线契约

> 状态：**[active]**（2026-10-11 起）。本文是后端的**现行契约**：覆盖 v24 落地后的
> 合法化两相化、值描述单一来源、能力缺口与观测台账（v23 的收口记录见
> [`archive/backend-v23-plan.md`](../archive/backend-v23-plan.md)；v24 逐批证据见
> [`plans/backend-v24-plan.md`](../plans/backend-v24-plan.md)）。
>
> 术语：本文的"后端"= IR 之后的全部阶段（合法化 / 指令选择 / 寄存器分配 / 帧布局 /
> 发射 / 装载）。ISA-DSL 语法、生成的指令编码与汇编器见 [`isa-dsl.md`](isa-dsl.md)；
> 调用约定数据与引擎见 [`calling-conventions.md`](calling-conventions.md)；
> **能力缺口台账**见 [`backend-gaps.md`](backend-gaps.md)。
>
> 三条"别绕过"的纪律：① 编译只有**一个入口**（§1）；② 合法化只有**一个改写者**
> （物化相，§3）；③ 值只有**一份描述**（§4）。每节都有一条守卫钉着，改代码就同时改本节。

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

诊断开关**不可能静默失效**：`pipeline::trace::knob("FORGE_TRACE_*")` 直接按名字问环境变量
（**任何名字都生效**），`pipeline::trace::KNOBS` 是**唯一登记处**，只回答"我们有哪些观测
开关、各自看什么"。忘登记的名字：debug 下 `debug_assert!` **当场 panic**，release 下照常
生效但 `eprintln` 一次警告。

为什么值得一节：旧实现维护一份开关白名单，**名字漏进白名单 ⇒ 该开关恒为假、静默失效**
——`FORGE_TRACE_AGG` 就这样死了几个月（"文档承诺可见、实际看不见"），M4 落地时又抓到第二个
同类 `FORGE_TRACE_M4`（调用点在、白名单里没有），而白名单里的 `FORGE_TRACE_ISEL` 一处调用点
都没有（死条目）。下表与源码调用点由守卫 `trace_knobs_are_bijective` 钉成**双向相等**
（多一行、少一行都红）。

| 开关 | 看什么 |
| --- | --- |
| `FORGE_TRACE_ABI` | 调用约定规划与落点回填（abi_setup / compiler） |
| `FORGE_TRACE_AGG` | 聚合展开与合法化（legalize 各 pass / 计划相 / 物化相） |
| `FORGE_TRACE_ALLOC` | 寄存器分配主循环（regalloc_bt） |
| `FORGE_TRACE_EMIT` | 机器码发射（emission） |
| `FORGE_TRACE_EXPIRE` | 活跃区间到期处理（regalloc_bt） |
| `FORGE_TRACE_LOWER` | 指令选择（lowering；每指令一条） |
| `FORGE_TRACE_M4` | 绑定折入与多类槽回填类诊断（compiler） |
| `FORGE_TRACE_REGALLOC` | 寄存器分配编排（regalloc_bt 入口） |
| `FORGE_TRACE_SPILL` | 溢出与回填（emission / regalloc_bt） |
| `FORGE_TRACE_STACK` | 栈槽与帧布局（lowering） |
| `FORGE_TRACE_VCODE` | 虚拟指令流（compiler） |

性能：每个开关一个 `OnceLock` 缓存（首次查询后零开销）——regalloc/lowering/emission 的
**每指令热路径**上都有查询，实测裸 `std::env::var_os` 会占 codegen 的 7–18%。

## 8. 相关账本与下一步

* 能力缺口：[`backend-gaps.md`](backend-gaps.md)（21 条，含逐条决策证据）。
* v24 逐批证据与删除清单：[`plans/backend-v24-plan.md`](../plans/backend-v24-plan.md)。
* v23 收口记录（历史）：[`archive/backend-v23-plan.md`](../archive/backend-v23-plan.md)。
