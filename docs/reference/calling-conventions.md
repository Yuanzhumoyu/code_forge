# 调用约定层（forge-abi）

> **状态：active**（v20 A1/A2，2026-09-24）。代码为准：`crates/foundation/forge-abi`、
> `forge-ir` 的 `CallConvId`、`forge-codegen` 的 `pipeline::conv_registry`。
> 本文只讲**已经落地**的东西；A3–A7（管线按 plan 发射、删谱里 `[abi]`）的设计与进度见
> [`docs/plans/calling-convention-redesign-plan.md`](../plans/calling-convention-redesign-plan.md)。

调用约定在这套编译器里是**三层数据 + 一个通用引擎**，不是散在各处的 if-else：

```text
使用者的数据                          平台无关
┌────────────────────────┐
│ AbiRules   约定规则     │  win64 / sysv64 / aapcs64 / lp64d / 你自己的
└───────────┬────────────┘
            │  ┌──────────────────────────┐
            ├─►│ AbiBinding (ISA, 约定)   │  池名 → 具体寄存器（这台机器）
            │  └──────────────────────────┘
            │  ┌──────────────────────────┐
            ├─►│ AbiTarget  ISA 能力视图  │  有哪些寄存器/宽度/能力（谱说了算）
            │  └──────────────────────────┘
            ▼
      forge_abi::plan_fn
            │
            ▼
      AbiPlan（纯数据：每个实参/形参的落点、返回值、栈布局、callee-saved、hidden 槽）
```

IR 侧只做一件事——**声明用哪份约定**（`CallConvId`），并把名字交给宿主注册表解析：

```text
FunctionSignature.calling_convention : CallConvId
   = Builtin(ConvName)  |  Named(ImmStr)  |  Index(u32)
        │  forge_codegen::pipeline::conv_registry::resolve()   ← 宿主数据，**未注册即报错**
        ▼
   ctx.call_conv_name（注册表键："win64"/"my_conv"/"cc42"）
        │  forge_abi::AbiRegistry::rules(name) / binding(isa, name)
        ▼
   AbiRules + AbiBinding → plan_fn → AbiPlan（A3 按它发射调用点/入口/序尾声）
```

**一条铁律**：引擎只认识注册表里注册过的约定名。没注册 ⇒ 明确报错，绝不退回
`Default`——旧设计里 IR 的 `CallConv::Default` 就是被这条"静默兜底"变成死值的。

## 为什么要有这一层（旧设计的硬伤）

四条都有代码级证据，不是风格意见：

| 现象 | 证据（随迭代漂移，以符号名为准） |
| --- | --- |
| IR 里的调用约定是**死值** | `CallConv` 曾有 16 个变体 + `Custom(u32)`，全仓只有一处**写**它（`pipeline/compiler.rs` 里设 `ctx.call_conv`），**没有任何地方读**；连前端都没人填它（`forge-rustc` 从不引用这个类型） |
| 同一份约定在每个 ISA 里各写一遍、且各写各的 | x86 谱有 `push`/`pop`/`stack_arg_load`/`stack_arg_store`/`frame_addr`/`wide_vec_*` 角色，arm64 谱一个都没有 |
| 换 ISA 就错 | 间接结果指针（sret）被硬编码成"**首 int 参数槽**"= Windows x64 的 RCX；AAPCS64 其实是 **x8**，RISC-V 是 **a0** |
| 变参无处可写 | IR 有 `va_arg`、有 `variadic` 标志，但没有"未命名实参放哪、`va_list` 什么形状、`%al`/`LEN` 谁填"的模型 |
| 未知约定被折成不可还原的占位值 | 文本层曾把 `amdgpu_cs_chain` 这类未知名字编码成 `Custom(len ^ 0x8000_0000)`——既打印不回原样，也没有任何代码读它 |

本层把"约定"从 ISA 里拿出来、把"能力"从约定里拿出来，于是：

- 同一份 `win64` 规则能被**任何**"寄存器编号与 Windows x64 一致"的 ISA 复用（换机器只换绑定）；
- 同一台 x86 机器能同时挂 `win64` 与 `sysv64` 两份约定（差别只在数据）；
- ISA 只申报"我有没有这个能力"，缺了就在**规划期**报 `CapabilityGap`，而不是生成期炸。

## ① AbiRules —— 约定本身（平台无关）

TOML 数据，可继承（`parent = "c"`，整字段覆写，不是深合并）。写错的键直接报错
（`deny_unknown_fields`）。

### 键总览

| 键 | 含义 |
| --- | --- |
| `name` / `parent` / `note` | 名字、父约定、人类可读备注（出处/已知偏差） |
| `position` | `by_class`（int/float 各自计数）或 `by_position`（共享位置游标；Windows x64） |
| `int_pool` / `float_pool` / `vector_pool` | 参数池的**抽象名**（默认 `int` / `float` / `float`） |
| `stack_align` / `frame_padding` / `red_zone` / `shadow_bytes` | 调用点对齐、帧填充、红区、shadow space |
| `stack` | `slot_bytes` / `first_offset_slots`（被调方第一个栈参数相对 frame_base 的槽数）/ `right_to_left` |
| `classify` | **参数位**分类规则（顺序即优先级，首条命中者胜） |
| `ret_classify` | **返回位**分类规则（非空即**独占**，不落回参数位规则） |
| `fallback` | 没命中时的兜底（缺省 `stack {}`） |
| `callee_saved` | `mechanism`（`none`/`push`/`store_to_frame`）/ `pools` / `includes_fp` / `includes_link` |
| `callee_pop` | `none` / `sum_stack_args`（stdcall）/ `fixed = 4`（thiscall） |
| `hidden` | sret / context / 变参元信息槽 + `va_list` 形态与尺寸 |
| `extensions` | `widen_to_bits`（AArch64 ≥32）/ `callee_ignores_upper_bits`（x86） |
| `variadic_stack_only` | 未命名实参**只能走栈**（Win64/AAPCS64/RISC-V） |
| `tail_calls` | `allowed` / `must_match_stack` |

### 分类规则：`when → do`

`when` 只看**类型形状**（族名/字节大小/同质浮点成员数）：

| `when` 键 | 判据 |
| --- | --- |
| `kind` | `int`/`ptr`/`float`/`vector`/`aggregate`/`other`/`scalar`（`scalar` = 非聚合非向量） |
| `size_le` / `size_gt` | 字节大小（**上下界都是闭/开区间，注意 `size_gt = N` 不含 N**） |
| `hfa_max` | "同质**浮点**聚合且成员数 ≤ N"（`{f32,f32}` 是 HFA；`{i64,i64}` **不是**——见下） |

`do` 五种动作：

| `do` | 含义 |
| --- | --- |
| `direct = { pool = "int" }` | 从池里取 1 个槽 |
| `direct = { pool = "float", slots = 2 }` | 取 2 个**连续**槽（不够就整块走栈，绝不取一半） |
| `direct = { pool = "float", slots = "hfa" }` | 槽数**由类型决定**（HFA 成员数：`{f32}` 占 1、`{f32,f32,f32,f32}` 占 4） |
| `pair = { lo = "int", hi = "float" }` | 两个池各取一槽（`(int,float)` 拆分） |
| `indirect = { via = "caller_stack_copy" }` | 调用方在自己栈上做副本、传指针（LLVM 的 `byval`） |
| `indirect = { via = "hidden_sret" }` | 隐藏的间接结果指针（占 hidden 槽） |
| `stack = { align = 8 }` | 强制走栈 |
| `ignore` | 不传递（例如返回值的"无"） |

三条容易写错、这里已经钉住的点：

1. **`slots` 不能写死**：AAPCS64 的 `struct {f32}` 占 **1** 个浮点槽、`struct {f32;f32;f32;f32}`
   占 **4** 个。写死 `slots = 4` 会让单成员 HFA 白吃 3 个寄存器、把后面的参数挤到栈上；
   写死 `2` 更错。所以有 `slots = "hfa"`（`SlotsSpec::Named`，未知名字在 `validate` 期报错）。
2. **HFA 只认浮点成员**：`{i64,i64}` 是"16 字节整数聚合"，该走 2 个整数槽
   （RISC-V 的 a0:a1），**不是** HFA。把整数聚合当 HFA 会让它进浮点寄存器 —— 典型错值。
3. **返回位与参数位可以不同**（`ret_classify`）：AAPCS64 的 >16B 聚合**当参数**是
   byval 指针、**当返回**是 x8 的 sret；Win64 同理（参数 byref、返回 RCX sret）。
   只写一份 `classify` 会把"返回值就是这个指针"当成事实。

### 返回位为什么还要**独立池**

返回寄存器与参数寄存器不是同一批：Win64 的标量返回在 `RAX`，参数却从 `RCX` 起；
SysV 返回 `RAX`/`RAX:RDX`，参数从 `RDI` 起。所以内置绑定给出 `ret_int` / `ret_float`，
`ret_classify` 用它们；RISC-V/AAPCS64 的 `a0`/`x0` 恰好既是首个参数又是返回寄存器，
但**仍然是两套位置序列**（引擎里是两个独立的池游标，一个 2 槽返回不会把第一个参数挤走）。

## ② AbiBinding —— (ISA, 约定) 的寄存器绑定

```toml
isa = "x86_64_v12"      # 与 [meta].name 一致
conv = "win64"          # 与 AbiRules::name 一致

[pools]
int = ["RCX", "RDX", "R8", "R9"]        # 名字，或该 ISA 的寄存器号（整数）
float = ["XMM0", "XMM1", "XMM2", "XMM3"]
cs_gpr = ["RBX", "RDI", "RSI", "R12", "R13", "R14", "R15"]
ret_int = ["RAX"]
ret_float = ["XMM0"]
```

- 池是**懒解析**的：没被任何签名用到的池可以不写；一旦规则要求它而绑定没有 ⇒
  `MissingPool`（消息点名"哪个约定引用了哪个池"）。
- 名字解析不到 ⇒ `UnresolvedReg`（带池名、第几项、选择子）。**不猜**。
- 整数选择子是**宿主寄存器号**（`TargetRegInfo` 的语义）。用 `forge-isa abi`
  （没有宿主，按 `abi_view` 的"GPR 区 + FP 区"编号）检查时，**优先用名字**。
- 帧指针不进 `cs_gpr`：x86 的 RBP、riscv 的 X8、arm64 的 X29 由帧件保存，
  规则侧用 `callee_saved.includes_fp` / `includes_link` 表达"它也被保存"（否则算两遍）。

## IR 侧：`CallConvId`（声明用哪份约定）

`forge-ir` 只声明**用哪份**约定，不定义任何一份：

```rust
pub enum CallConvId {
    Builtin(ConvName),   // c / sysv64 / win64 / aapcs64 / lp64d
    Named(ImmStr),       // 使用者注册的名字（开放集合，如 "my_conv"）
    Index(u32),          // 数值约定（LLVM `cc N`；注册键是 "cc<N>"）
}
```

- **默认 = `Builtin(C)`**；文本层对 `c` **不写关键字**（与 LLVM 一致），其余按 LLVM 拼写
  （`win64cc`/`fastcc`/`cc 42`）或规范名（`sysv64`/`aapcs64`/`lp64d`）。
- **文本表只有一份**（`CallConvId::to_text` / `from_text` 互为逆）：不会出现"解析成一个、
  打印成另一个"的往返漂移；未知标识符 → `Named`（**原样保留**，不再折成
  `Custom(len ^ 0x8000_0000)` 这类无法还原的占位值）。
- **未注册即 fail-closed**：`forge_codegen::pipeline::conv_registry::resolve()` 在编译入口
  （`CompileState::new`）把标识解析成注册表键，查不到就报 `Unsupported` 并列出已注册的名字。
  宿主用 `register_rules_toml` / `register_binding_toml` 加自己的约定。
- 解析结果落在 `LowerCtx::call_conv_name`——**A3 就用这个名字查 `AbiRules`/`AbiBinding`**
  发射调用点/入口/序尾声。旧实现"只写不读"的那个字段，现在是一条活路径。
- 二进制格式跟着升到 **`IR_FORMAT_VERSION = 3`**：签名体里那 1 字节判别值改成 tag
  （`0..=4` = 内置、`5` = 命名（字符串表下标）、`6` = 数值（varint））。`c` 仍是 **1 字节**
  ——与旧格式同宽，字节偏移类的手工测试不受影响。破坏性更新、**无兼容读取**：
  旧流在头部就报版本不符。

## ③ AbiPlan —— 引擎产物

`plan_fn(target, rules, binding, sig, hooks)` 产出**调用方与被调方共用**的一份纯数据：

| 字段 | 用途 |
| --- | --- |
| `args[i].place` | 每个实参/形参的落点：`reg` / `pair` / `group`（≥3 连续槽）/ `stack` / `indirect` |
| `ret` | `reg` / `pair` / `indirect`（sret）/ `void` |
| `stack` | 对齐、槽单位、shadow、`first_arg_offset`（被调方视角）、`arg_area_bytes`、`byval_area_bytes`、红区、帧填充；`caller_offset(k)` 给调用方视角 |
| `hidden` | `sret` / `context` / `va_meta`（SysV `%al`）/ `va_len` |
| `callee_saved` | 机制 + 有序寄存器表 + `includes_fp` / `includes_link` |
| `clobbers` | 可分配寄存器 − callee-saved − 固定用途（调用方要假设被破坏的部分） |
| `va_area` | 变参形态、尺寸、对齐、是否只走栈 |
| `widen_to_bits` | 形参/实参至少扩到多少位 |

`AbiPlan::to_text()` 是**确定性**渲染（`forge-abi` 的黄金快照与 `forge-isa abi plan`
都用它）；同一输入两次必须逐字节相同。

两个坐标系的区别（踩过）：

- `Placement::Stack.offset` = **被调方**视角（相对 `frame_base`，已含
  `first_offset_slots × slot_bytes`）；
- `Placement::Indirect.at` = **调用方 byval 临时区**里的偏移（`byval_area_bytes` 那么大，
  从 0 起）。副本是调用方**帧内**的临时量，跟"第几个栈参数"无关；混进传出参数区会让
  后面的栈参数与副本抢同一段内存。

## 内置约定（四份 + 一个抽象基类）

| 约定 | 位置计数 | 参数寄存器（内置绑定） | 返回寄存器 | 栈/shadow/红区 | 宽返回（sret） | callee-saved 机制 | 变参 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `c` | by_class | ——（抽象基类，无绑定） | —— | 16 / 0 / 无 | 需自己声明 | 无 | 不支持（未声明 `va_list`） |
| `win64` | **by_position** | RCX-R9 / XMM0-3 | RAX / XMM0 | 16 / **32** / 无 | RCX（`int` 池 0 槽） | `push` | 未命名实参走栈；`va_list` = 栈指针 |
| `sysv64` | by_class | RDI,RSI,RDX,RCX,R8,R9 / XMM0-7 | RAX:RDX / XMM0:XMM1 | 16 / 0 / **128** | RDI | `push` | 未命名实参继续用寄存器；`%al` 报向量寄存器数 |
| `aapcs64` | by_class | X0-X7 / **缺**（谱里没有 FPR 组） | X0:X1 | 16 / 0 / 无 | **X8（独立池）** | `store_to_frame` | 未命名实参走栈；形参 ≥32 位 |
| `lp64d` | by_class | X10-X17 / F10-F17 | X10:X11 / F10:F11 | 16 / 0 / 无 | X10（`int` 池 0 槽） | `store_to_frame` | 未命名实参走栈 |

`c` 是给别的约定继承的抽象基类（`parent = "c"` 给出"通用 C 家族"的分类兜底），
**它自己没有绑定**——直接拿 `c` 规划会明确报"缺绑定"，这是刻意设计的 fail-closed。

## 声明属性（`byval`/`sret`/`inreg`/`zeroext`/`signext`/`align`）

前端在形参/返回值上写下的"按什么传"，经两条路进引擎，**真的改变规划**（不是装饰）：

```text
IR：Function.param_attrs[i] / ret_attrs（ParamAttributes）
        │  forge_codegen::pipeline::sig_view::decl_attrs()   ← A2b：这些字段以前没人读
        ▼
引擎：forge_abi::DeclAttrs（输入）
        │  plan_fn 把声明折进分类与落点
        ▼
plan：AbiPlan（产物；`ext`/`align`/`Indirect{..}` 都是可断言的结果）
```

| 声明 | 对 plan 的影响 |
| --- | --- |
| `byval(N)` | 该形参变 `Indirect{ ptr, on_stack: true }`（调用方栈上 N 字节副本），副本进 `byval_area_bytes`；指针按 int 池传 |
| `sret` | 该形参占约定声明的 **hidden sret 槽**（x86 RCX/RDI、AAPCS64 **x8**、riscv a0），并记进 `hidden.sret`；它**不再**按普通参数分类 |
| `inreg` | 分类说要走栈时再试一次寄存器池；**池空就仍走栈**（不硬凑） |
| `zeroext` / `signext` | 落点带 `Extension`（两个都写时 `signext` 胜，与 LLVM 一致） |
| `align(N)` | 栈落点对齐抬到 `N`（与类型自然对齐取大）；`0` = 未声明 |

类型投影（`sig_view::ty_view`）只摊开、不猜测：大小/对齐一律取自 `TypeStore`
（**带填充的结构体必须用权威大小**，按成员裸和会少算），摊不开的类型（可扩展向量）落到
`TyKind::Other` 交给规则兜底。

`AbiPlan::to_text()` 不打印"输入属性"——它只打印**产物**（`ext=ZeroExt`、`indirect
ptr=RDX at=Some(0) on_stack=true`、`align=32` 这些），因此"属性生效了没有"在快照里一眼可见。

## ISA 侧：能力视图（AbiTarget）

引擎只问"能不能"（`AbiTarget`），不问"约定了什么"：

```rust
pub trait AbiTarget {
    fn isa_name(&self) -> &str;
    fn reg_count(&self) -> u32;
    fn reg_name(&self, index: u32) -> Option<String>;
    fn reg_class_name(&self, index: u32) -> String;
    fn reg_index(&self, name: &str) -> Option<u32>;
    fn reg_width(&self, index: u32) -> u8;
    fn pinned(&self, index: u32) -> bool;          // sp/fp/zero/platform
    fn allocatable(&self) -> Vec<u32>;
    fn spill_scratch(&self) -> Vec<u32>;
    fn link_reg(&self) -> Option<u32>;             // ra / lr
    fn cap(&self, cap: Capability) -> Option<u16>; // 能力 → 可编码位宽
}
```

没有宿主后端时（CLI、测试），能力视图由**谱本身**给出：
`forge_isa_dsl::abi_view::inspect(谱)` 读 `[reg.*]` / `[abi]` / `[[instructions]].roles`
建出一份 `MachineView`：

```text
GPR 区 = 地址类组的成员，号 0..n_gpr-1          （x86: RAX..R15 = 0..15）
FP  区 = 主 FPR 组的成员，号 n_gpr..n_gpr+n_fp-1 （x86: XMM0..XMM15 = 16..31）
别名（EAX/W0/ZMM0…）解析到同一物理号；落到两区之外的别名（x86 的 ZMM16-31）不入表
```

编号与 `TargetRegInfo::num_gp_regs` / `num_fp_regs` 对齐；固定用途寄存器来自
`[abi].reserved` + `[abi.frame]` 的 sp/fp；链接寄存器来自 `[abi].call_ret_reg`；
能力由 `[[instructions]].roles` 折算（`gpr_mov`/`fpr_mov`/`vec_mov`/`frame_alloc`
+`frame_free`→`sp_adjust`/`stack_arg_*`/`frame_addr`/`wide_vec_*`→`wide_vec_move`/
`call`/`call_indirect`/`ret`）。

**无宽度语义的角色按地址宽折算**（`roles = ["gpr_mov"]` → x86 记 `gpr_mov@64`）；
需要精确宽度就按 v18 S9 的写法声明 `{ role = "fpr_mov", bits = 32 }`。

## 数据表达不了的：AbiHooks

某些约定是**语言专属**的（Swift 的 `self`/`error`、Go 的 context 寄存器与 GC 安全点、
闭包 env、按语言规则改分类）。这类走宿主钩子，**不**塞进引擎：

```rust
pub trait AbiHooks: Send + Sync {
    /// `dir` 区分参数位/返回位（同一个类型可以两样）。
    fn classify(&self, rules: &AbiRules, ty: &TyView, dir: ClassDir) -> Option<ClassAction> { None }
    /// plan 出锅后的最后调整（加上下文寄存器、改 callee-saved…）。
    fn adjust_plan(&self, plan: &mut AbiPlan) {}
}
```

`AbiRegistry::insert_hooks("swift", Box::new(...))` 按约定名挂。

## CLI：`forge-isa abi`

```text
forge-isa abi list [--json]                    列出内置约定与绑定（含关键事实）
forge-isa abi check <谱.toml>... [--conv <名>] [--strict] [--json]
forge-isa abi plan  <谱.toml> --conv <名> --sig "i64, f64 -> i64" [--variadic <命名数>] [--json]
```

- **`abi check`** = 谱的能力视图 × 内置绑定 × 一组代表签名（15 条：整数/指针/浮点/
  向量/16B 与 24B 聚合/1、2、4 成员 HFA/参数多到溢出/变参）。
- **硬错**（`UnresolvedReg`/`BadRules`/`Parse` = 名字或约定写错）一律退出 1；
  **缺口**（`MissingPool`/`Unsupported`/`CapabilityGap`/`PoolExhausted` = 这台机器做不了，
  fail-closed）默认只报 `⚠ GAP`、退出 0，`--strict` 才让它决定退出码。
- `--sig` 的类型表：`i8..i128` / `f32` / `f64` / `ptr` / `v<N>`（N 字节、f32 元素）/
  `agg<N>`（不透明聚合，**不是 HFA**）/ `hfa<N>` / `hf64<N>`；参数可带名字 `a:i64`。

**三份发行谱的实测**（2026-09-24，`forge-isa abi check isa/*.toml`）：

| 谱 | `[meta].name` | 寄存器视图 | 结论 |
| --- | --- | --- | --- |
| `isa/x86_v12.toml` | `x86_64_v12` | GPR 16 + FP 16 | `win64` / `sysv64` 各 15 条代表签名**全部可规划**；硬错 0 |
| `isa/riscv64_v12.toml` | `riscv64_v12` | GPR 32 + FP 32 | `lp64d` 15 条**全部可规划**；链接寄存器 X1 |
| `isa/arm64_v12.toml` | `arm64_v12` | GPR 32 + **FP 0** | `aapcs64` **6 条缺口**：谱里没有 FPR 寄存器组 ⇒ `float`/`ret_float` 池缺 ⇒ 浮点/HFA 无寄存器可落 |

arm64 那 6 条缺口正是矩阵里 175 条 skip 的同一件事，现在**在规划期**就说得清楚
（而不是等到生成/运行）。关闭它属于 A5：谱里加 `[reg.fpr8]`（`V0..V31`）+ 浮点搬运
指令的角色声明。

## 已知缺口（诚实清单）

| 缺口 | 现状 | 关闭时机 |
| --- | --- | --- |
| arm64 无 FPR/VEC 寄存器组 | 浮点/HFA 参数报 `MissingPool`（fail-closed） | A5（谱 + 绑定） |
| SysV 的 eightbyte（INT/SSE 混合）分类 | ≤16B 聚合统一按两个整数槽；真实 SysV 会按成员拆到 XMM | A6 |
| HFA 寄存器不足时"部分在寄存器" | 本片整块走栈（AAPCS64 允许部分在寄存器，需要按成员赋值的规则语言） | A6 |
| ≥3 槽的**返回**搬运 | 模型能表达（`Placement::RegGroup`），返回路径明确 `Unsupported` | A6 |
| 变参 `LEN` 类元信息寄存器 | 模型有 `hidden.va_len_pool`，**没有内置约定启用**（psABI 现状以官方定本为准） | A6（核对后决定） |
| riscv/arm64 的向量 by-value | 谱里没有向量寄存器组 ⇒ 走内存/byval（保守，不是错值） | A5/A6 |
| Win64 的 XMM6-XMM15 | 谱里 `[abi.callee_saved].xmm = []` ⇒ `clobbers` 保守地把它们列为被破坏（安全方向） | A5（若要省寄存器再议） |

## 加自己的约定

**第一步：写规则**（`rules.toml`）——从 `c` 继承，只写与父不同的字段：

```toml
name = "myconv"
parent = "c"
position = "by_class"
stack = { slot_bytes = 8, first_offset_slots = 1 }
classify = [
  { when = { kind = "float", size_le = 8 }, do = { direct = { pool = "float" } } },
  { when = { kind = "scalar", size_le = 8 }, do = { direct = { pool = "int" } } },
]
ret_classify = [
  { when = { kind = "scalar", size_le = 8 }, do = { direct = { pool = "ret_int" } } },
]
hidden = { sret_pool = "int", sret_slot = 0 }
callee_saved = { mechanism = "push", pools = ["cs_gpr"] }
```

**第二步：写绑定**（`binding.toml`）——`isa` = 目标谱的 `[meta].name`：

```toml
isa = "my_isa"
conv = "myconv"
[pools]
int = ["R1", "R2"]
float = ["F1", "F2"]
cs_gpr = ["R10"]
ret_int = ["R0"]
```

**第三步：注册并按需挂钩子**：

```rust
let mut reg = forge_abi::builtin::registry()?;     // 内置 + 内置绑定
reg.insert_rules_toml(include_str!("rules.toml"))?;
reg.insert_binding_toml(include_str!("binding.toml"))?;
let plan = reg.plan(&target, "myconv", &sig)?;
```

参考数据与"为什么这么绑"的说明在
[`crates/foundation/forge-abi/conventions/README.md`](../../crates/foundation/forge-abi/conventions/README.md)。
