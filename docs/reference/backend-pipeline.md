# 后端编译管线契约

> 状态：**[active]**（2026-10-11 起）。本文件**随** `docs/archive/backend-v23-plan.md`（v23 已收口归档）的里程碑
> 回写：当前覆盖 M1（编译入口），M2–M6 落地时分节补齐（账本见该方案 §7）。
>
> 术语：本文的"后端"= IR 之后的全部阶段（合法化 / 指令选择 / 寄存器分配 / 帧布局 /
> 发射 / 装载）。ISA-DSL 语法、生成的指令编码与汇编器见 `reference/isa-dsl.md`；
> 调用约定数据与引擎见 `reference/calling-conventions.md`。

## 1. 编译入口：只有一份实现

宿主面**只有一个编译类型** `forge_codegen::CompileSession`；编译阶段只有一处实现
（`pipeline/compiler.rs` 的单函数管线 + `pipeline/session.rs` 的模块编排）。

| 入口 | 用途 | 说明 |
| --- | --- | --- |
| `CompileSession::new(machine)` | 单函数 | `compile(&Function)` / `compile_with_alloc(&Function)` |
| `CompileSession::for_module(machine, &Module)` | 单函数或整模块 | 从模块取签名表装上（§2） |
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

## 2. 模块签名表（变参 D6）

调用指令只带一个 `FuncRef`，而"被调方是不是变参"只写在**被调方的签名**里——所以
编译**整个模块**的宿主必须把这份事实交给编译器：

- 类型：`forge_ir::ModuleSigs`（逐项 `FuncSigEntry { variadic, fixed_args }`），
  由 `Module::signature_table()` 产出，**按 `FuncRef::index()` 对齐**；
- 传递：`CompileSession::with_module_sigs` / `for_module` → `LowerCtx::conv.module_sigs`
  （`Option<Arc<ModuleSigs>>`，按函数传递 O(1)）；
- 读口子：`LowerCtx::conv.variadic_of(FuncRef) -> Option<(bool, u32)>`——生成物在调用点
  只写一行（查表与越界兜底收在访问器里，越界 = `None` = 按非变参处理）。

两条纪律：① 表**只从模块取**，别在宿主侧另抄一份；② 单函数入口（`add_function`）
也要带上已知的表，否则跨函数变参调用会按非变参规划落点（漏写调用的元信息寄存器、
浮点实参落位错）。

## 3. 擦除入口（Registry / 插件）

生成的 ISA 模块导出 `ERASED` 面：`ErasedTargetMachine::compile(&Function)` 按 ISA 名查
`forge_isa_runtime::register_pipeline` 登记过的工厂；**未注册 ⇒ `IrError::Unsupported`**
（fail-closed，不 panic），消息点名 ISA 与"需要宿主注册"。

宿主侧登记在 `forge_codegen::pipeline_hooks::ensure_registered()`（进程内一次：三个发行
后端 x86_64 / riscv64 / arm64 的工厂闭包 + 调用点布局钩子）。`CompileSession::new` 会调它，
因此"忘记注册"在本仓内部不可达。

## 4. 观测开关（诊断面台账，v24 M4）

诊断开关**不可能静默失效**：`pipeline::trace::knob("FORGE_TRACE_*")` 直接按名字问环境变量
（**任何名字都生效**），`pipeline::trace::KNOBS` 是**唯一登记处**，只回答"我们有哪些观测
开关、各自看什么"。忘登记的名字：debug 下 `debug_assert!` **当场 panic**，release 下照常
生效但 `eprintln` 一次警告。

为什么值得一节：旧实现维护一份 `TRACE_KEYS` 白名单，**名字漏进白名单 ⇒ 该开关恒为假、
静默失效**——`FORGE_TRACE_AGG` 就这样死了几个月（"文档承诺可见、实际看不见"），M4 落地时
又抓到第二个同类 `FORGE_TRACE_M4`（调用点在、白名单里没有），而白名单里的
`FORGE_TRACE_ISEL` 一处调用点都没有（死条目）。下表与源码调用点由守卫
`trace_knobs_are_bijective` 钉成**双向相等**（多一行、少一行都红）。

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

## 5. 未完成项（指向方案）

M2–M6 的契约变化（单一 ABI 数据模型、能力数据化、单一操作数模型、分配/发射契约、
合法化单一驱动）见 `docs/archive/backend-v23-plan.md` §5（v23 已收口归档）；v24 的契约
回写（合法化两相化 / 值描述单一来源 / 缺口与观测台账 / 阶段清单守卫）见
`docs/plans/backend-v24-plan.md`，**M5 落地时在本文件补齐并改为 v24 契约**。
