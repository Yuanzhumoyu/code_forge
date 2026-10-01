# 变参（varargs）方案 [progress]

> 状态：**win64 栈式变参已端到端跑通**（2026-10-01）。已落地：**D6 + V0/V1 + V2/V3 + V4（win64：
> 整数类 + `f64` + `f32` 提升）**——调用方按被调方语义把未命名实参发到栈上（浮点按类分派），
> 被调方用 `va_start` 物化 `va_list` 对象、`va_arg` 取值并**原地推进**。仍缺：`%al`/`va_meta`
> 的写入、**寄存器保存区**（sysv64/lp64d/aapcs64 的 V3 剩余部分，这类约定点 `va_start` 会明确
> fail-closed）、更窄浮点与向量的取值能力，以及**任何前端产出变参签名**。
> 相关背景：`docs/reference/calling-conventions.md`、`docs/plans/calling-convention-redesign-plan.md` A6。

## 1. 现状：**规划（plan）与调用方/被调方发射都通了，寄存器保存区与前端还是空的**

| 层 | 现状 | 证据（按符号名，行号随迭代漂移） |
| --- | --- | --- |
| 规则数据 | `hidden = { va_meta_pool, va_len_pool, va_list, va_list_size, va_list_align }`、`variadic_stack_only` 都在 | `forge-abi/src/rules.rs` 的 `Hidden` / `VaListKind` |
| 引擎 | 变参签名能规划：**未命名实参**按 `variadic_stack_only` 决定"只走栈"还是"继续用寄存器"；产出 `AbiPlan::va_area`（kind/size/align/stack_only）与 `hidden.va_meta`/`va_len` | `forge-abi/src/engine.rs`（`unnamed` 分支、`va_area` 构造） |
| 黄金快照 | 四份内置约定 × 变参语料已钉住（见 §2 表） | `forge-abi/tests/golden/*.plan.txt` 的 `## va_*` 段 |
| CLI | `forge-isa abi plan <谱> --conv <名> --sig "…" --variadic <命名数>` 能打印变参计划 | `forge-isa/src/abi.rs` |
| **发射（调用方）** | **已落地**（V1）：未命名实参按被调方的变参语义发（win64 走栈），由 `LowerCtx::module_sigs` 提供的"被调方是变参/命名几个"驱动；**整模块编译的宿主由 `FunctionCompiler::for_module` 自动装表**（接线缺口见 §5 V1） | `jit.rs::test_jit_variadic_unnamed_args_go_to_stack`、`abi_target_real::call_site_variadic_hint_decides_unnamed_argument_placement` |
| **发射（被调方）** | **win64 已落地**（V2）：`va_start` 取未命名实参区地址（`LeaRbpOff rbp+48`），后续 `load`/`gep` 走既有路径；**寄存器保存区形态明确 fail-closed**（消息点名 V3） | `jit.rs::test_jit_va_start_reads_unnamed_stack_args`、`lowering.rs::gen_va_start_lowering` |
| **发射（`va_meta`/`va_len`）** | **零消费**：`hidden.va_meta` / `hidden.va_len` 在 `crates/…/src` 里没有任何读者 | 全仓 grep：命中只在引擎、测试与黄金文件 |
| **前端** | **零产出**：`FunctionSignature::variadic` 在生产代码里没有设置者（测试与二进制格式往返测试除外） | `forge-rustc` / `mini_c` 里没有 `variadic` |
| IR | 签名能表达 `variadic`（二进制格式也往返） | `forge-ir/src/ir/types.rs`、`forge-ir/src/binary/types.rs` |

一句话：**调用方与 win64 被调方两条发射路径都通了；"寄存器保存区 + 提升 + `va_arg`"与前端产出还没做。**

## 2. 四份内置约定的 `va_list` 形态（本仓库内置数据）

这张表是**代码里的数据**（`forge-abi/src/builtin.rs` 的四份规则），不是抄 psABI 手册——
与官方定本逐条核对列为 §4 的待办 D3。守卫 `invariants.rs::va_shapes_match_the_documented_table`
拿引擎实际算出的计划与这张表逐格对照。

| 约定 | `VaListKind` | size | align | 未命名实参 | `va_meta` 寄存器 |
| --- | --- | --- | --- | --- | --- |
| `win64` | `Win64Stack` | 8 | 8 | 只走栈 | — |
| `sysv64` | `SysvRegSave` | 24 | 8 | 继续用寄存器 | `RAX` |
| `aapcs64` | `Aapcs64Struct` | 32 | 8 | 只走栈 | — |
| `lp64d` | `RiscvSaveArea` | 24 | 8 | 只走栈 | — |

## 3. 缺口（按链路，越靠前越阻塞）

```text
前端（写 variadic 签名 / va_start / va_arg）      ← 缺
   │
IR（签名能表达；缺"取下一个实参"的 op 形态）        ← 缺
   │
引擎（✔ 已能算：va_area / va_meta / 未命名实参落点）
   │
发射：调用方（✔ 未命名实参按 plan 发；缺 %al 写入）  ← 部分
发射：被调方（物化寄存器保存区 / 建 va_list）        ← 缺
```

三个具体缺口：

1. **调用方**：调用一个**外部**变参函数（如 `printf`）时，要按约定设 `va_meta`（SysV 的 `%al`
   = 用到的向量寄存器个数）并把未命名实参按 plan 传出去。引擎已给出 `va_meta` 与落点，
   **但调用点根本拿不到"被调方是变参"这条信息**（见 D6）——这是 V1 的前置阻塞点，
   排在发射代码之前。
2. **被调方**：函数自己是变参时，要在序言把"寄存器参数区"物化成 `va_area`
   （SysV 保存区 / riscv 保存区 / AAPCS64 结构；Win64 例外——`va_list` 就是栈指针）。
3. **`va_arg`**：从 `va_list` 取下一个实参，且要做**默认实参提升**（`f32`→`f64`、窄整型→
   `int`）。这是唯一需要"按类型推进游标"的部分，也是与 A6 的"按成员赋值"同源的难点。

## 4. 需要拍板的决策点

| # | 决策 | 选项 | 影响 |
| --- | --- | --- | --- |
| D1 | **先服务哪种场景** | ① 调用外部变参函数（调用方）② 自己被当变参调用（被调方） | ①只动发射、能立刻用 JIT 验（造一个自己的变参 callee）；②要 `va_start` 的 IR 形态，牵动前端 |
| D2 ✅（2026-10-01 裁定） | **`va_arg` 怎么表达** | **IR 内建 op `VaArg`**（拿 `va_list` 对象的指针、原地推进、返回提升后的值）；配套把 `va_start` 的语义收紧为"**物化 `va_list` 对象并返回其地址**" | 不选"前端展开"：游标的推进语义属于 ABI，摊给每个前端等于把约定抄 N 份；代价是 `VaStart` 语义**破坏性**变更（见 §5 的契约裁定） |
| D3 | **以哪份 psABI 定本为准** | SysV / Win64 / AAPCS64 / RISC-V 各自官方定本 | §2 的 size/align 现在是"数据里写的"，需逐条核对（尤其 `sysv64` 的 24 字节保存区与 `%al` 语义） |
| D4 | **先做哪台机器** | 建议 **win64**（`va_list` = 栈指针，不需要寄存器保存区、不需要 `%al`） | 最小的可用切片；riscv/sysv 的保存区留到第二期 |
| D5 ✅（2026-10-01 裁定，同日更正） | **提升规则放哪** | **放在 `va_arg` 的语义里，不放约定数据**：C 的默认实参提升是**语言层**规则（`float` → `double`），不是 ABI 事实——LLVM 就是这么分工的（`va_arg` 指令的语义规定"小于 double 的浮点按 double 读再截断"，clang 在**调用点**做提升）。我们照抄这个分工：调用点（前端）提升实参，读取方（`va_arg` 发射）"取 f64 再窄回"，**窄回这一步的能力**由 ISA 按角色申报（`{ role = "fpr_narrow", bits = 32 }`） | 原先写"进 `AbiRules` 的 `va_promote`"是**过度数据化**：四份内置约定都是 double，加一个没人会改的键不符合本仓库"没有消费者不加键"的纪律。**触发条件**：若某份约定的变参浮点提升在 psABI 层面**不是** double，再加数据键 |
| D6 ✅ | **调用点怎么知道"被调方是变参、命名了几个"** | **已落地**：宿主把**模块级签名表**（`FuncRef` → `(variadic, 命名个数)`）塞进 `LowerCtx::module_sigs`，生成物在调用点查表后传给 `plan_call`（`CallPlanner` 入参多一位 `variadic`） | 不动 IR；单函数编译（表缺席）按非变参处理，与旧行为一致 |
| D7 ✅（更正） | **IR/管线怎么表达"未命名实参"** | **不需要改 IR**：沿用 LLVM 形态即可——变参**签名只列命名参数**，`Call` 可以带比形参更多的实参（`declare @printf(ptr, ...)`；`Call` 本来就不校验个数） | 我一开始把它判成"必须先动 IR"，原因是**测试里把被调方建模错了**（3 个形参全列进签名又标 variadic ⇒ `fixed_count == args.len()` ⇒ 未命名实参不出现、用例假绿） |

### D6/D7 的实测经过与证据（2026-09-30）

**先把结论说清**：`plan_call` 原来只吃形状，没有"被调方是变参"这条信息，而 win64 的
`variadic_stack_only = true` 会把未命名实参从寄存器改判到栈——**这是真的缺口（D6）**；
但我随后把它写成"还要动 IR（D7）"，那是**误判**：IR 的 LLVM 形态本来就能表达，问题出在
我那次测试**把被调方建模错了**。

| 建模 | `fixed_count` | 第 2/3 个实参 | 结果 |
| --- | --- | --- | --- |
| 错：3 个形参全列进签名 + `variadic = true` | 3 = 实参个数 | 寄存器 | 用例**假绿**（变参提示无操作） |
| 对：签名只列命名参数（1 个）+ `variadic = true` | 1 | **栈** | 用例真绿：传出区 = shadow(32) + 2×8 = 48 |

守卫 `crates/backend/forge-codegen/tests/abi_target_real.rs::call_site_variadic_hint_decides_unnamed_argument_placement`
钉三件事：① 不给提示 ⇒ 按非变参发（`Reg(RDX)/Reg(R8)`）；② 给提示 ⇒ 未命名实参走栈
（`Stack/Stack`）；③ 调用点（给提示）与被调方（`.variadic(1)`）**落点一致**。
`jit.rs::test_jit_variadic_unnamed_args_go_to_stack` 再加两条可观察的发射决策：
装表 ⇒ `stack_arg_bytes == 48`，不装 ⇒ `0`，并 JIT 真跑一遍（不崩、返回命名参数值）。

## 5. 建议的分期（每期独立可验证）

- **V0 ✅（D6 + D7）**：调用点通过 `LowerCtx::module_sigs`（`FuncRef` → `(variadic, 命名个数)`）
  知道被调方是变参，`CallPlanner::plan_call` / `abi_target::plan_for_shapes` 多一位
  `variadic: Option<(bool, u32)>`，引擎据此按 `variadic_stack_only` 改判未命名实参。
  IR **未改动**（LLVM 形态本来就够用）。
- **V1 ✅（win64）**：调用方把未命名实参按被调方语义发出（win64 走栈；传出区随
  `__cl` 自动增长——`max_stack_arg_bytes` 是按**调用点布局**算的）。**验收口径**：V1 那一版
  只能验**落点与帧尺寸**（守卫 + JIT 各一条），因为当时被调方还读不出多余实参；V2 起改成
  **算出来的值**（见下）。仍缺：`va_meta`（SysV 的 `%al`）的写入。
- **V2 ✅（被调方，win64 栈式）**：2026-09-30 落了数据面、2026-10-01 把发射接通——
  ① **IR**：新 op `VaStart`（0 操作数 → 指针；`ops.toml` 一行 + builder `va_start()`），语义
  "取未命名实参区的地址"；
  ② **计划镜像**：`CallLayout.va: Option<VaInfo>`（`VaKind`/`size`/`align`/`stack_only`），由
  `AbiPlan::va_area` 逐条 match 折过来（`VaKind::supports_frame_addr_va_start()` 是"能不能
  直接用帧内栈地址实现"的判据：只有 `win64_stack` 为真）；
  ③ **发射**：`lowering.rs::gen_va_start_lowering` 按角色 `frame_addr`（x86 = `LEA_RBP_OFF`）
  发一条 `lea dst, [fp + 未命名区起点]`——与 `frame_set`/`frame_alloc`/`callee_save` 同族：
  **谱只申报能力，序列由生成器发**（`isa/x86_v12.toml` 里没有 `VaStart` 的 `[[lowering]]` 规则，
  这是刻意的）；其余（`load`/`gep`/算术）全走既有规则——**不需要**新的 IR 形状或多值路径。
  寄存器保存区形态与"该约定不支持变参"都**明确 fail-closed**
  （消息分别点名 V3 与"va_list 未声明"）。
  **验收**：`jit.rs::test_jit_va_start_reads_unnamed_stack_args` 真跑 `4*10+7 = 47`
  （`va_start` → `load` → `gep` → `load`，两个未命名实参各读一槽）。
  **落地时抓到的两个真缺口**（都是"看起来对、跑起来错"的那类，记在这里以免回潮）：
  1. **未命名区起点要 `first_arg_offset + shadow_bytes`**（win64 = 16 + 32 = 48）。引擎侧不变量是
     `Stack.offset(k) = first_arg_offset + shadow_bytes + k×slot`：`first_arg_offset`
     （= `first_offset_slots × slot`）是"帧基址 → 调用方 sp"的距离，而调用方把第 0 个栈槽写在
     `sp + shadow_bytes` 处。只取 `first_arg_offset` 会读到 shadow 区里的垃圾
     （实测 `va` 落在 shadow 内，算出 1.4e15 量级的地址值）。
  2. **`JitCompiler::compile_module` 从未装模块签名表**：`with_module_sigs` 的**唯一**调用者是
     V1 的测试本身，整模块编译的宿主全都没接 ⇒ 调用点按非变参发（三个实参全进 RCX/RDX/R8），
     被调方读 shadow 区。修法是**让模块自述这张表**：`forge_ir::Module::signature_table()`
     （下标 = `FuncRef::index()`，读**每个函数自己的**类型上下文——`add_function` 不把签名并进
     模块 store）+ `FunctionCompiler::for_module(machine, &module)`（宿主侧默认写法），
     `JitCompiler::compile_module`、`forge-tests` 的矩阵/QEMU 逐函数编译路径都改用它。
  **仍缺**：`forge-tests` 矩阵的夹具（已登记为缺口 `("VaStart", "…方案 V2")`）；矩阵的
  `CaseKind::Module` 目前没有变参用例（JIT 用例在 `forge-codegen` 里）。
- **V3（寄存器保存区，sysv64/lp64d）**：按 `va_area.size/align` 在帧内开槽并把参数寄存器存进去；
  校验 `hidden.va_meta` 的写入。
- **V4（`va_arg`）**：按 D2 的裁定实现取参 + D5 的提升规则；这是唯一必须动 IR/前端的一期。
- **V5 ✅（2026-09-30）**：`forge-isa abi check <谱>` 为每份约定多打一行 `ℹ 变参 …`，把"这台机器
  × 这份约定的变参处于什么状态"说清楚：`win64` 栈式（va_list = 栈指针，调用方一侧可用）；
  `sysv64`/`lp64d`/`aapcs64` 需要**寄存器保存区** ⇒ 如实报"发射侧尚未物化（V2/V3）"。
  **三种自相矛盾是硬错**（影响退出码）：① `va_list = "win64_stack"` 却
  `variadic_stack_only = false`（未命名实参可能进寄存器，而 va_list 只指向栈）；
  ② 声明了形态却没给合法的 `va_list_size`/`va_list_align`（要 > 0 且 size 是 align 的整数倍）；
  ③ `va_meta_pool`/`va_len_pool` 点到的池在这台机器上解析不动。
  "需要保存区"这类**已知状态只进 `ℹ`**、不算缺口——缺口语义保持"这台机器做不了"，否则
  `--strict` 会把"发射尚未实现"混进机器能力账。守卫：`cli_tests::abi_check_reports_the_variadic_state`
  与 `abi.rs` 的 5 条 `variadic_report_*` 单测（矛盾 / 尺寸 / 池名 / 保存区 / 不支持）。

验收口径沿用本仓库惯例：每期跑 `fmt` + `clippy -D warnings` + workspace 全量（含三条 JIT
矩阵），生成物形状/落点用守卫钉住，行为用 JIT 真跑。

### V3/V4 的契约已裁定（2026-10-01）：**`va_list` 物化 + `va_arg`**

V2 落地时把 `VaStart` 定义成"**取未命名实参区的地址**"——那只对 **win64 栈式**成立。往寄存器
保存区（`sysv64`/`lp64d`/`aapcs64`）走时这个语义就**不通用**了：那些约定的 `va_list` 是一个
**多字段对象**（sysv64 = `{gp_offset, fp_offset, overflow_arg_area, reg_save_area}`），不是
一个地址。所以先把契约收紧再往下写——**这是破坏性改动**（改 `VaStart` 的语义：win64 要多一个
帧槽与一次 store），换来一份**对所有约定同形**的模型（也才谈得上"足够通用"）：

| 概念 | 通用契约 |
| --- | --- |
| `va_start` | **物化本约定的 `va_list` 对象**（放帧槽）并返回**该对象的地址**；win64 的对象 = 1 个指针字段（指向未命名区）⇒ 与今天的取值同源，只是多一次 store |
| `va_arg(ap, ty)` | 读 `ap` 指向的对象 → 取一个 `ty` → **原地推进** → 返回**提升后**的值（IR 内建 op，见 D2 裁定） |
| `va_copy` / `va_end` | `va_copy` = 按 `va_list_size` 的对象拷贝；`va_end` = 空操作（都不建 op，前端不做即可） |

为此**计划面还缺三样数据**（今天只有 `hidden.va_list` 形态 + `va_list_size`/`va_list_align`）：

1. **`va_list` 的字段布局**（每个 kind 的字段名/偏移/宽度）——物化对象时要写这些字段；
2. **寄存器保存区**（`[{ reg, offset }]` + 区大小/对齐 + `%al` 这类元信息）——序言要把参数
   寄存器存进去，`reg_save_area` 指向它；
3. **提升规则**（`f32` → `f64`、`i8`/`i16` → 32 位槽等）——D5 裁定放**约定数据**。

发射侧还差一个**通用机制**：让管线**不依赖前端**就能要一段帧字节（`va_list` 对象与保存区都由
生成器管）。今天帧字节只有 `max_stack_bytes`（由前端 `mem.stack_addr` 抬出来的），所以这件事要
和 V3 一起设计，**不要**在 `va_start` 里塞特例。

修订后的分期：**V3** = 上面 1 + 2 + 帧槽机制 + 序言物化（win64 的"1 字段对象"当第一块试金石：
验收件 `test_jit_va_arg_reads_unnamed_stack_args` 读回 47）；**V4** = `VaArg` op
加上提升规则（可选再做 `va_copy`）。

**V3 的第一片已落地（2026-10-01）**：**对象物化**——管线按 `va_area.size/align` 给 `VaStart`
分配帧槽（与 `StackAddr` 同一条通路：设 `ctx.current_offset`、抬 `max_stack_bytes`），生成器
`lea` 出对象地址、把未命名区地址写进字段 0（win64 的形状）、返回对象地址。契约从
"返回未命名区地址"改成"**返回对象地址**"（破坏性，按上表裁定执行）；
`test_jit_va_arg_reads_unnamed_stack_args`（当时还叫 `…_va_start_…`）改成**穿过对象**读回两个实参（实跑 47）。
**仍是 ⬜**：寄存器保存区（`sysv64`/`lp64d`/`aapcs64` 的序言物化）、`VaArg` op 与提升规则
（V4）、以及"按 vreg 基址读写"的三个能力角色（`ptr_load`/`ptr_store`/`add_imm`——`VaArg`
的游标推进要用，见下一段）。

**V4 需要的新能力（已申报）**：win64 的 `va_arg` 序列 = 从对象取游标 → 从游标取值 →
游标 += 槽宽 → 写回对象。x86 谱里"值基址"的内存访问靠 **Reg 槽**（`MOV_R_MEM` 宽度自动、
`STORE_MEM_R` 宽度自动），"寄存器 += 常量"= `ADD_R_IMM32`——三条**都不是 ABI 能力**
（`abi_view::role_capability` 对它们返回 `None`），只是"生成器要发一段序列"要的能力。

**V4 的第一片已落地（2026-10-01）**：`va_arg(ap, ty)`（IR op 早已在 `ops.toml`/文本层/
二进制格式里，这一片补上 builder 与发射）——生成器按上面三个角色发
"取游标 → 取值 → 游标 += 槽宽 → 写回"，x86 谱把 `ptr_load`/`ptr_store`/`add_imm` 打到
`MOV_R_MEM`/`STORE_MEM_R`/`ADD_R_IMM32` 上（这三条本来就有、只是没有角色）。
`test_jit_va_arg_reads_unnamed_stack_args` 两次 `va_arg` 取回 `4`/`7` 算 `47`。

**V4 的第二片已落地（2026-10-01）**：**浮点（f64）未命名实参两端打通**——被调方取值按结果类
分派到 `{ role = "ptr_load", class = "fpr" }`（x86 = `MOVSD_R_MEM`，mem → XMM），**调用方**
写栈也按类分派到 `{ role = "stack_arg_store", class = "fpr" }`（x86 = `MOVSD_MR`）：只做被调方
那一半是不够的——调用方拿整数 store 搬 XMM 会把值写坏。缺 fpr 版 store ⇒ 浮点栈实参**明确
Unsupported**（不再静默错值）；第 5+ 个**命名**浮点实参走的同一条路径也因此归位。
验收 `test_jit_va_arg_reads_unnamed_float_args`（`4.5 + 7.0 → 11`）。

**V4 的第三片已落地（2026-10-01）**：**`f32` 的默认提升**——`va_arg(ap, f32)` 取 `f64` 再**窄回**
（能力角色 `{ role = "fpr_narrow", bits = 32 }`，x86 = `CVTSD2SS`）。这是 D5 那个裁定的落地形态：
规则在 `va_arg` 的语义里（与 LLVM 同分工），能力在 ISA 的角色声明里。只按 f32 读 4 字节会读到
promoted double 的低半（静默错值）。验收 `test_jit_va_arg_narrows_promoted_f32`
（`2.5 + 3.25 → 5`）。**仍缺**：更窄的浮点（f16 之类）与**向量**结果的取值能力——运行期按结果
类型 fail-closed。

**V3 的第二片已落地（2026-10-01）：`va_list` 字段布局 + 寄存器保存区的**计划面数据**。
`AbiPlan::va_area` 现在带 `fields`（对象字段：名/偏移/宽）与 `save`（保存区：大小/对齐/槽表
`{寄存器, 区内偏移, 字节数}`），`CallLayout.va` 有对应的运行时中立镜像，黄金快照与两条守卫
（`va_object_layout_matches_the_psabi_numbers`、
`abi_target_real::call_layout_mirrors_the_variadic_shape`）把数字钉住：

- win64：`cursor@0+8`，**没有**保存区（未命名实参只在栈上）；
- sysv64：`gp_offset@0+4 / fp_offset@4+4 / overflow_arg_area@8+8 / reg_save_area@16+8`，
  保存区 176/16 = 6 GP（8 字节槽）+ 8 XMM（**16 字节槽**）；
- aapcs64：5 字段（`__stack`/`__gr_top`/`__vr_top`/`__gr_offs`/`__vr_offs`），保存区 192/16 = 8 X + 8 V；
- lp64d：`area@0+8`，保存区 128/8 = 8 X + 8 F。

**踩到的真问题（已修）**：浮点槽宽**不能取"寄存器类宽"**——psABI 规定的是保存区槽宽。按绑定
宽度算会得到 arm64 `128`（应 `192`：V 槽恒 16 字节）、riscv `96`（应 `128`：FP 槽 = XLEN）。
现在槽宽按**形态**给（与字段布局同一类 psABI 事实）：`sysv_reg_save`/`aapcs64_struct` → 16、
`riscv_save_area` → `slot_bytes`。**已知偏差**：arm64 的绑定把 V 建模成 64 位 ⇒ 序言只会写低
64 位，向量/HFA 变参要 128 位存取（arm64 变参落地时处理）。

**仍缺（发射侧）**：`va_arg` 的分支式取值（游标超界 ⇒ 改取溢出区）。这一片的其余三件已经落地
（见下），所以 sysv64 的"**对象物化 + 保存区 spill**"现在真跑得通了。

**V3 的第三片已落地（2026-10-01）：sysv64 的发射侧**（保存区 spill + `va_start` 物化对象）：

- **管线预留保存区**：`CallLayout.va.save` 有区就按 `align`/`size` 在 locals 区之后开一段帧字节，
  偏移落在 `LowerCtx::va_save_off` / `AllocResult.va_save_off`（与 `VaStart` 的对象槽同一条通路
  ⇒ 不重叠）。实测（sysv64）：`size=176 align=16 depth=176 → 帧基址-240`。
- **序言 spill**：`move_args` 在收参**之前**按槽表把参数寄存器存进去，**按槽的寄存器类分派**
  （GP 用 `stack_arg_store`、FP 用 `{ …, class = "fpr" }`——两个角色都已存在）。
- **`va_start` 物化对象**（sysv64 一族）：`lea` 对象地址、`lea` 溢出区、`lea` 保存区，再用新角色
  `gpr_imm` 把宿主算好的 `gp_offset|fp_offset` 打包值装进寄存器，最后三条帧相对 store 写字段。
  "已用掉几个参数寄存器 / 溢出区从哪开始"由**宿主**按 plan 预先算好（`VaListInit`），
  因为生成物是各约定通用的、算不出来。
- **验收（真跑）**：`test_jit_va_start_materializes_the_sysv64_register_save_area`——调用方（也用
  sysv64）传 `(0, 4, 7)`，被调方用 IR 算术读出 `gp_offset`（=8）与 `reg_save_area`，从
  `[reg_save+8]` / `[+16]` 取回 4 / 7 算出 47。**这条路验证了三件事**：序言 spill 的槽序/偏移、
  `gp_offset` 的初值、`reg_save_area` 指向保存区。
- **`va_arg` 仍未接**（保存区形态）⇒ 对 sysv64 的 `va_arg` 继续 fail-closed。下一片的形态已核实
  （见本节末尾「下一片」）。

**V3 的第四片已落地（2026-10-01）：保存区形态的 `va_arg` 走 IR 展开**（sysv64 真跑两条）：

- **在管线里展开、不在生成器里发序列**：条件取值（游标未超上限 ⇒ 取保存区，否则取溢出区，各自
  推进）在 IR 层只需 `Icmp`/`Select`/`Iadd`/`Uextend`/`Ireduce`/`Load`/`Store`/`Fload`/`Fptrunc`
  ——**全都有现成降级**；生成器要发同样的事得新增 `cmp`/`cmov`/`add_rr`/`and` 四个能力角色。
- **布局与宽度一律来自数据**（这一条是评审时被点出来的，已改）：字段偏移与宽度取自 plan 的
  `va.fields`（不是 sysv64 的常量），游标按**它自己的宽度**读写（`u32` 就 4 字节、`u64` 就 8 字节），
  槽宽取自 `va.save.slots[0].size`、上限/步长取自 `va.init`。换一台机器/换一份约定只要还是
  "主游标 + 次游标 + 溢出指针 + 保存区指针"这一族，**不改代码**；不是这一族的形态（aapcs64 的
  gr/vr 计数、riscv 的分界）明确报错说"该形态不在本族"，而不是按 sysv64 的偏移瞎算。
- **验收（真跑，两条支都覆盖）**：
  `test_jit_va_arg_sysv64_reads_the_register_save_area`（两个未命名实参都在寄存器 ⇒ 47）与
  `test_jit_va_arg_sysv64_overflows_to_the_stack_arg_area`（第 6 个超出 GP 池 ⇒ 必须改取溢出区
  并推进溢出游标 ⇒ 42）。

**仍然存在的"为某一个开的洞"（下一步要拆掉）**：`va_start` 物化对象仍在**生成器**里，于是留下两处
x86 专属：① `VaInit.offsets` 把 `gp_offset|fp_offset` **打包**成一个 u64（因为生成器只有 8 字节的
帧相对 store），② 为此新增的能力角色 `gpr_imm`。**通用做法**：把 `va_start` 也搬进同一个 IR 展开
——IR 里每个字段按**自己的宽度** store（`Store` 的类型就是字段类型），常量用 `Iconst`、帧内地址用
现成的 `StackAddr`（偏移由展开自己分配，与 `Opcode::VaStart` 现在那条管线通路合一）。那样
`gpr_imm` 与打包都会消失，保存区物化对**任意宽度**的 ISA 都成立。触发条件：arm64/riscv 的保存区
落地时（它们的 `init` 也要一并数据化）。

**展开形态**（本族的整数类；偏移/宽度按 plan 的 `va.fields` 代入）：

```text
gp      = load <游标宽> [ap + 主游标偏移]
in_reg  = icmp ult gp, 上限
p_reg   = reg_save + uextend(gp)
p_src   = select in_reg, p_reg, overflow
v       = load <ty> [p_src]
gp_out  = select in_reg, gp + 步长, gp
ov_out  = select in_reg, overflow, overflow + 槽宽
store <游标宽> [ap + 主游标偏移] = ireduce(gp_out)
store <指针宽> [ap + 溢出偏移]   = ov_out
```

浮点类用**次游标**字段（`fp_offset`/`fp_limit`/`fp_step`），取值走 `Fload`；`f32` 先取 `f64`
再 `Fptrunc`。字段数不足本族（aapcs64/riscv）⇒ 明确报错"该形态不在本族"。

在此之前 aapcs64/riscv 的 `init` 未算 ⇒ 它们的 `va_start` 也继续 fail-closed。

**顺带修掉一个真 bug（sysv64 的栈实参落点）**：`sysv64` 内置规则的 `first_offset_slots` 写的是
**1**，而本实现的被调方**总是 push 帧指针** ⇒ 从 `rbp` 看第一个栈实参在 `[rbp + 16]`（返回地址 +
保存的 fp 各一槽），win64/aapcs64/lp64d 本来就都是 2，只有这一份漏了。实测证据：
`test_jit_sysv64_seventh_integer_arg_comes_from_the_stack` 在修前读到的是**返回地址**
（`2001632886933`），修后是 `7`；`sysv64.plan.txt` 黄金快照的 `first_arg_off` 8 → 16、
栈实参偏移整体 +8（逐个核对过）。**影响面**：任何走栈的 sysv64 形参（第 7 个整数起、超 FP 池的
浮点）此前都读错——与变参无关，是这条路径此前**没有用例**。

## 6. 为什么现在**不做**（触发条件）

与 A6 的两条"评估后不做"同源：**没有消费者**。

- 没有任何前端会产出变参签名 ⇒ V2–V4 写完也没有真实输入；
- 发射侧**基本零消费**（V1 已把调用方那一半接上，但那半也只有"没有前端签名"时才静默不生效）
  ⇒ 继续往下写只是"为将来的路预埋"，而本仓库的纪律是"先有证据再动手"
  （参见 A6 ⑤ 的按成员赋值、`callee_pop` 两条）。

**触发条件**（出现任一即可开工）：① 有宿主需要在 JIT 里调用外部变参函数（例如 `printf`
一类运行时入口）；② 有前端（`mini_c` 或 forge-rustc 的外部声明）开始产出 `variadic` 签名；
③ 有人要接一份"被调方变参"的 ABI 兼容测试。

在此之前，本方案的 §1–§2 就是**当前事实的索引**，`invariants.rs` 的变参守卫保证它不漂移。
