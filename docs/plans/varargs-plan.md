# 变参（varargs）方案 [progress]

> 状态：**部分落地**（2026-09-30）。已落地：**D6 + V1 的调用方一侧**——调用点通过宿主的
> **模块级签名表**知道被调方是变参、命名几个，从而把**未命名实参**按约定发到栈上
>（`CallPlanner::plan_call` 多一位 `variadic`；守卫见 §5 的 V1）。仍缺：`%al`/`va_meta` 的
> 写入、被调方 `va_area` 物化、`va_arg`，以及**任何前端产出变参签名**。
> 相关背景：`docs/reference/calling-conventions.md`、`docs/plans/calling-convention-redesign-plan.md` A6。

## 1. 现状：**规划（plan）已经能做，发射与前端是空的**

| 层 | 现状 | 证据（按符号名，行号随迭代漂移） |
| --- | --- | --- |
| 规则数据 | `hidden = { va_meta_pool, va_len_pool, va_list, va_list_size, va_list_align }`、`variadic_stack_only` 都在 | `forge-abi/src/rules.rs` 的 `Hidden` / `VaListKind` |
| 引擎 | 变参签名能规划：**未命名实参**按 `variadic_stack_only` 决定"只走栈"还是"继续用寄存器"；产出 `AbiPlan::va_area`（kind/size/align/stack_only）与 `hidden.va_meta`/`va_len` | `forge-abi/src/engine.rs`（`unnamed` 分支、`va_area` 构造） |
| 黄金快照 | 四份内置约定 × 变参语料已钉住（见 §2 表） | `forge-abi/tests/golden/*.plan.txt` 的 `## va_*` 段 |
| CLI | `forge-isa abi plan <谱> --conv <名> --sig "…" --variadic <命名数>` 能打印变参计划 | `forge-isa/src/abi.rs` |
| **发射（调用方）** | **已落地一部分**（V1）：未命名实参按被调方的变参语义发（win64 走栈），由 `LowerCtx::module_sigs` 提供的"被调方是变参/命名几个"驱动 | `jit.rs::test_jit_variadic_unnamed_args_go_to_stack`、`abi_target_real::call_site_variadic_hint_decides_unnamed_argument_placement` |
| **发射（`va_meta`/`va_area`）** | **零消费**：`hidden.va_meta` / `hidden.va_len` / `va_area` 在 `crates/…/src` 里没有任何读者 | 全仓 grep：命中只在引擎、测试与黄金文件 |
| **前端** | **零产出**：`FunctionSignature::variadic` 在生产代码里没有设置者（测试与二进制格式往返测试除外） | `forge-rustc` / `mini_c` 里没有 `variadic` |
| IR | 签名能表达 `variadic`（二进制格式也往返） | `forge-ir/src/ir/types.rs`、`forge-ir/src/binary/types.rs` |

一句话：**"计划"这一半是完整的，"发射 + 前端"这一半完全没有。**

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
| D2 | **`va_arg` 怎么表达** | ① IR 内建 op（`va_arg(va_list, ty)`）② 前端展开成 load + 游标运算 ③ 提供库函数 | 决定要不要动 `forge-ir` 的 opcode 表与文本/二进制格式 |
| D3 | **以哪份 psABI 定本为准** | SysV / Win64 / AAPCS64 / RISC-V 各自官方定本 | §2 的 size/align 现在是"数据里写的"，需逐条核对（尤其 `sysv64` 的 24 字节保存区与 `%al` 语义） |
| D4 | **先做哪台机器** | 建议 **win64**（`va_list` = 栈指针，不需要寄存器保存区、不需要 `%al`） | 最小的可用切片；riscv/sysv 的保存区留到第二期 |
| D5 | **提升规则放哪** | 规则数据（`when = { kind = "float", size_le = 4 } → f64`）还是引擎内置 | 影响"足够通用"：数据化更好，但要设计键 |
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

- **V0 ✅（D6 + D7）**：调用点通过 `LowerCtx::module_sigs`（JIT 在 `compile_module` 里按
  `FuncRef` 填 `(variadic, 命名个数)`）知道被调方是变参，`CallPlanner::plan_call` /
  `abi_target::plan_for_shapes` 多一位 `variadic: Option<(bool, u32)>`，引擎据此按
  `variadic_stack_only` 改判未命名实参。IR **未改动**（LLVM 形态本来就够用）。
- **V1 ✅（部分，win64）**：调用方把未命名实参按被调方语义发出（win64 走栈；传出区随
  `__cl` 自动增长——`max_stack_arg_bytes` 是按**调用点布局**算的）。**验收口径**：因为被调方
  还读不出多余实参，验收是**落点与帧尺寸**（守卫 + JIT 各一条，见上），而不是"算出来的值"。
  仍缺：`va_meta`（SysV 的 `%al`）的写入。
- **V2 🚧（被调方，win64：数据面已落地，发射待做）**：2026-09-30 落地了两半——
  ① **IR**：新 op `VaStart`（0 操作数 → 指针；`ops.toml` 一行 + builder `va_start()`），语义
  "取未命名实参区的地址"；
  ② **计划镜像**：`CallLayout.va: Option<VaInfo>`（`VaKind`/`size`/`align`/`stack_only`），由
  `AbiPlan::va_area` 逐条 match 折过来（`VaKind::supports_frame_addr_va_start()` 是"能不能
  直接用帧内栈地址实现"的判据：只有 `win64_stack` 为真）。
  守卫 `abi_target_real::call_layout_mirrors_the_variadic_shape`（win64 栈式 8/8 / sysv64 保存区
  24/8 / 非变参不带 va）与 `forge-ir` 的 opcode 表守卫。
  **仍缺**：生成器侧的 `VaStart` 发射（`isa/x86_v12.toml` 现在**没有** `VaStart` 的 lowering
  规则，所以任何函数用它都会因"没有规则"而 fail-closed——**这是当前的诚实状态**）；以及
  `forge-tests` 矩阵的夹具（已登记为缺口 `("VaStart", "…方案 V2")`）。
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
