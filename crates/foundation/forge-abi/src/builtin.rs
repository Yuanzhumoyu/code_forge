//! 内置约定规则（**数据**，TOML 文本）：`c` / `win64` / `sysv64` / `aapcs64` / `lp64d`。
//!
//! 它们只描述**规则**，不含任何寄存器名——寄存器由 `conventions/*.toml` 的绑定给出
//! （见 `crate::binding`）。写在这里是为了让"约定"成为可读、可 diff、可测试的数据，
//! 而不是散落在生成器里的 if-else。
//!
//! 出处：SysV AMD64 ABI / Microsoft x64 ABI / AAPCS64 / RISC-V psABI。
//! **已知简化**（A6 扩展，逐条列在规则 `note` 里，绝不含糊过去）：
//!
//! - SysV 的 eightbyte 分类（INTEGER/SSE 混合）尚未逐字节建模：≤16B 聚合统一按
//!   两个整数槽处理（真实 SysV 会按成员拆到 XMM）；
//! - AAPCS64/RISC-V 的 HFA 用 `slots = "hfa"`（**按类型**取成员数：`{f32}` 占 1 个、
//!   `{f32,f32,f32,f32}` 占 4 个），**不是**写死 4/2——写死会让单成员 HFA 白吃寄存器；
//!   同质判定只认**浮点**成员（`{i64,i64}` 是"2×XLEN 整数槽"，不是 HFA）；
//!   寄存器不够时**整块走栈**（规范允许"部分在寄存器"，需要按成员赋值的规则语言）；
//! - 变参的"未命名实参"按 `Signature::fixed_count` 分界；va_list 的内存形态只声明
//!   尺寸/对齐，取用由前端负责；
//! - **没有任何内置约定启用 `hidden.va_len_pool`**（RISC-V 生态里出现过的 `LEN` 实参，
//!   psABI 现状以官方定本为准，A6 逐条核对后再决定是否启用）；引擎支持它，路径由
//!   测试里的自定义约定覆盖。
//!
//! 寄存器绑定（**(ISA, 约定) → 具体寄存器**）是**另一份数据**：`conventions/*.toml`
//! 经 [`bindings`] 嵌入，[`registry`] 把它们和规则一起装成可直接用的注册表。

use crate::binding::AbiBinding;
use crate::error::AbiError;
use crate::registry::AbiRegistry;
use crate::rules::AbiRules;

/// C 家族共享的兜底规则（其余约定都从它派生）。
pub const C: &str = r#"
name = "c"
position = "by_class"
int_pool = "int"
float_pool = "float"
stack_align = 16
stack = { slot_bytes = 8, first_offset_slots = 0 }
classify = [
  { when = { kind = "float", size_le = 8 },  do = { direct = { pool = "float" } } },
  { when = { kind = "vector", size_le = 16 }, do = { direct = { pool = "float" } } },
  { when = { kind = "vector", size_gt = 16 }, do = { indirect = { via = "caller_stack_copy" } } },
  { when = { kind = "aggregate", size_le = 16 }, do = { direct = { pool = "int", slots = 2 } } },
  { when = { kind = "aggregate", size_gt = 16 }, do = { stack = {} } },
  { when = { kind = "scalar", size_le = 8 },  do = { direct = { pool = "int" } } },
]
fallback = { stack = {} }
"#;

/// Windows x64：int/float **共享位置计数**、32 字节 shadow space、帧填充 8、
/// 变参未命名实参走栈、>8B 聚合按引用传、sret 指针在 RCX。
pub const WIN64: &str = r#"
name = "win64"
parent = "c"
position = "by_position"
shadow_bytes = 32
frame_padding = 8
stack = { slot_bytes = 8, first_offset_slots = 2 }
aliases = ["c"]  # 这台机器（x86_64）上的 C 约定就是 Win64——整套代答（规则 + 绑定）
classify = [
  { when = { kind = "float", size_le = 8 },   do = { direct = { pool = "float" } } },
  { when = { kind = "vector", size_le = 16 }, do = { direct = { pool = "float" } } },
  { when = { kind = "vector", size_gt = 16 }, do = { indirect = { via = "caller_stack_copy" } } },
  { when = { kind = "aggregate", size_gt = 8 }, do = { indirect = { via = "caller_stack_copy" } } },
  { when = { kind = "aggregate" },            do = { direct = { pool = "int" } } },
  { when = { kind = "scalar" },               do = { direct = { pool = "int" } } },
]
# 返回位：**独占**（不落回上面的参数池——x64 的标量返回在 RAX/XMM0，参数却从 RCX 起）。
ret_classify = [
  { when = { kind = "float", size_le = 8 },      do = { direct = { pool = "ret_float" } } },
  { when = { kind = "vector", size_le = 16 },    do = { direct = { pool = "ret_float" } } },
  { when = { kind = "aggregate", size_gt = 8 },  do = { indirect = { via = "hidden_sret" } } },
  { when = { kind = "vector", size_gt = 16 },    do = { indirect = { via = "hidden_sret" } } },
  { when = { kind = "aggregate" },               do = { direct = { pool = "ret_int" } } },
  { when = { kind = "scalar" },                  do = { direct = { pool = "ret_int" } } },
]
hidden = { sret_pool = "int", sret_slot = 0, va_list = "win64_stack" }
callee_saved = { mechanism = "push", pools = ["cs_gpr"], includes_fp = true }
variadic_stack_only = true
extensions = { callee_ignores_upper_bits = true }
tail_calls = { allowed = true, must_match_stack = true }
note = "使用者的 C 兼容约定之一；寄存器名/编号由 AbiBinding 给出（见 conventions/win64-x86_64.toml）"
"#;

/// SysV AMD64：rdi/rsi/rdx/rcx/r8/r9 + xmm0-7、128 字节红区、`%al` 报向量寄存器数、
/// ≤16B 聚合按两整数槽（**简化**：真实实现按 eightbyte 拆 INT/SSE）。
pub const SYSV64: &str = r#"
name = "sysv64"
parent = "c"
red_zone = 128
# 帧填充 8（= align/2）：SysV 的入口 rsp ≡ 8 (mod 16)（call 压入返回地址），
# 本实现的序言是 `push fp` + `push` 全部 callee-saved（5 个）= **偶数次 push**，
# 故 sub rsp 前 rsp ≡ 8 ⇒ 需要 8 字节填充才能在 call 点回到 16 对齐
# （与 Win64 同值，理由同：两边推入次数都是偶数）。
frame_padding = 8
# `first_offset_slots = 2`：**不是** psABI 的 1，而是"本实现的被调方帧"里的槽数——序言
# 总是 `push fp`，所以从 `rbp` 看第一个栈实参在 `[rbp + 16]`（返回地址 + 保存的 fp 各一槽）。
# 2026-10-01 实测：写 1（只算返回地址）时，sysv64 的第 7 个整数形参读到的是**返回地址**
# （`test_jit_sysv64_seventh_integer_arg_comes_from_the_stack`）；win64/aapcs64/lp64d 本来
# 就是 2，只有这一份漏了。
stack = { slot_bytes = 8, first_offset_slots = 2 }
classify = [
  { when = { kind = "float", size_le = 8 },   do = { direct = { pool = "float" } } },
  { when = { kind = "vector", size_le = 16 }, do = { direct = { pool = "float" } } },
  { when = { kind = "vector", size_gt = 16 }, do = { indirect = { via = "caller_stack_copy" } } },
  { when = { kind = "aggregate", size_le = 16 }, do = { direct = { pool = "int", slots = 2 } } },
  { when = { kind = "aggregate", size_gt = 16 }, do = { stack = {} } },
  { when = { kind = "scalar", size_le = 8 },  do = { direct = { pool = "int" } } },
]
ret_classify = [
  { when = { kind = "float", size_le = 8 },      do = { direct = { pool = "ret_float" } } },
  { when = { kind = "vector", size_le = 16 },    do = { direct = { pool = "ret_float" } } },
  { when = { kind = "aggregate", size_gt = 16 }, do = { indirect = { via = "hidden_sret" } } },
  { when = { kind = "vector", size_gt = 16 },    do = { indirect = { via = "hidden_sret" } } },
  { when = { kind = "aggregate", size_le = 16 }, do = { direct = { pool = "ret_int", slots = 2 } } },
  { when = { kind = "scalar", size_le = 8 },     do = { direct = { pool = "ret_int" } } },
]
hidden = { sret_pool = "int", sret_slot = 0, va_meta_pool = "va_meta", va_list = "sysv_reg_save" }
callee_saved = { mechanism = "push", pools = ["cs_gpr"], includes_fp = true }
extensions = { callee_ignores_upper_bits = true }
tail_calls = { allowed = true, must_match_stack = true }
note = "eightbyte（INT/SSE 混合）分类未建模：≤16B 聚合按两整数槽（A6 扩展）"
"#;

/// AAPCS64：x0-x7/v0-v7、**HFA ≤4**、>16B 返回走 **x8** 间接结果寄存器、
/// 变参未命名实参全走栈、形参至少扩到 32 位。
pub const AAPCS64: &str = r#"
name = "aapcs64"
parent = "c"
stack = { slot_bytes = 8, first_offset_slots = 2 }
aliases = ["c"]  # arm64 上的 C 约定
classify = [
  { when = { kind = "float", size_le = 8 },      do = { direct = { pool = "float" } } },
  { when = { kind = "vector", size_le = 16 },    do = { direct = { pool = "float" } } },
  { when = { kind = "aggregate", hfa_max = 4 },  do = { direct = { pool = "float", slots = "hfa" } } },
  { when = { kind = "aggregate", size_le = 16 }, do = { direct = { pool = "int", slots = 2 } } },
  { when = { kind = "aggregate", size_gt = 16 }, do = { indirect = { via = "caller_stack_copy" } } },
  { when = { kind = "scalar" },                  do = { direct = { pool = "int" } } },
]
ret_classify = [
  { when = { kind = "float", size_le = 8 },      do = { direct = { pool = "ret_float" } } },
  { when = { kind = "vector", size_le = 16 },    do = { direct = { pool = "ret_float" } } },
  { when = { kind = "aggregate", hfa_max = 4 },  do = { direct = { pool = "ret_float", slots = "hfa" } } },
  { when = { kind = "aggregate", size_le = 16 }, do = { direct = { pool = "ret_int", slots = 2 } } },
  { when = { kind = "aggregate", size_gt = 16 }, do = { indirect = { via = "hidden_sret" } } },
  { when = { kind = "vector", size_gt = 16 },    do = { indirect = { via = "hidden_sret" } } },
  { when = { kind = "scalar" },                  do = { direct = { pool = "ret_int" } } },
]
hidden = { sret_pool = "sret", sret_slot = 0, va_list = "aapcs64_struct" }
callee_saved = { mechanism = "store_to_frame", pools = ["cs_gpr", "cs_fpr"], includes_link = true }
extensions = { widen_to_bits = 32 }
# 未命名实参**走寄存器**（x0-x7 / v0-v7，溢出才上栈）——AAPCS64 的 `va_list` 是
# `{__stack, __gr_top, __vr_top, __gr_offs, __vr_offs}` 的**计数式**结构：被调方把自己用过的
# **通用/向量寄存器**存进保存区、`va_arg` 按 `__gr_offs`/`__vr_offs`（从负值数到 0）从保存区取、
# 数到 0 再落到 `__stack`。这套结构只在"未命名实参确实进寄存器"时才讲得通。
# 2026-10-01 修正：此前写的是 `true`（只走栈）——那与保存区语义矛盾（同一个模式先例：lp64d
# 也是这么写的，照 psABI 定本核对后确认不符，见 `docs/plans/varargs-plan.md` §5）。
# `forge-isa abi check` 的"可疑组合"告警就是抓这个矛盾的（声明了保存区却只走栈）。
variadic_stack_only = false
tail_calls = { allowed = true, must_match_stack = true }
note = "HFA 用连续浮点槽表达；寄存器不足时整块走栈（规范允许部分在寄存器，A6 扩展）；未建模 LEN 类元信息寄存器（本片不猜 psABI，A6 核对后启用）"
"#;

/// 内置绑定文本（随 crate 数据，见 `conventions/README.md`）。
///
/// 顺序无关（绑定按 `(isa, conv)` 索引），列在这里是为了让 `forge-isa abi list`
/// 有稳定的枚举序。
pub const BINDING_FILES: &[&str] = &[
    include_str!("../conventions/win64-x86_64.toml"),
    include_str!("../conventions/sysv64-x86_64.toml"),
    include_str!("../conventions/aapcs64-arm64.toml"),
    include_str!("../conventions/lp64d-riscv64.toml"),
];

/// 解析全部内置绑定。
pub fn bindings() -> Result<Vec<AbiBinding>, AbiError> {
    let mut out = Vec::with_capacity(BINDING_FILES.len());
    for text in BINDING_FILES {
        let b = AbiBinding::from_toml(text)?;
        b.validate()?;
        out.push(b);
    }
    Ok(out)
}

/// **开箱可用的注册表**：内置规则 + 内置绑定（无宿主钩子）。
///
/// `forge-isa abi` 与集成测试从这里起步，宿主再按需 `insert_rules_toml` /
/// `insert_binding_toml` / `insert_hooks` 加自己的。
pub fn registry() -> Result<AbiRegistry, AbiError> {
    let mut r = AbiRegistry::with_builtin_rules()?;
    for b in bindings()? {
        r.insert_binding(b)?;
    }
    Ok(r)
}

/// RISC-V LP64D：a0-a7/fa0-fa7、**按类**计数、HFA ≤2、>2×XLEN 聚合按引用、
/// 变参未命名实参**走整数寄存器**（定本口径，2026-10-01 落地）。
///
/// **未启用** `va_len_pool`：历史上 RISC-V 生态有过"LEN 实参"的写法，本片不猜
/// psABI 细节（A6 按官方定本核对后再启用；引擎支持这条路径，见 `tests/invariants.rs`）。
///
/// **变参四条按定本**（2026-10-01 核对 `riscv-cc.adoc` 原文后一起改完，缺一条就会静默读错值）：
/// ① `variadic_stack_only = false`（未命名实参走 a0-a7，溢出上栈）；② `variadic_classify`
/// 按**整数约定**分类（变参的浮点也进整数池）；③ 保存区只装整数参数寄存器（`float_slot = 0`）；
/// ④ 保存区**紧贴入口 `sp`**（`contiguous = true`），`va_list` 初值 = 保存区起点 + 已用整数
/// 寄存器数 × 槽宽，于是它先走完寄存器里的变参、再接着走栈上的变参。守卫
/// `invariants.rs::lp64d_variadic_arguments_follow_the_psabi` 逐条钉住这四件。
pub const LP64D: &str = r#"
name = "lp64d"
parent = "c"
# `first_offset_slots = 0`：本实现的 RISC-V 被调方**帧基址 = 入口 sp**（`X8 = 入口 sp`，
# 帧只在它**之下**展开：`addi sp, sp, -N` + ra/fp 保存槽都在 X8 以下），所以调用方写在
# `[sp + caller_offset]` 的传出实参，在被调方看来就是 `[X8 + caller_offset]`——**没有**"返回
# 地址 + 保存的 fp"这两个槽（RISC-V 不 push 返回地址）。
# 2026-10-01 实测：写 2（照搬 x86 的"序言总是 push fp"）时，QEMU 通道的变参用例
# `variadic_va_arg_int_only` 读到 0（应 47）——被调方的 `va_list` 比调用方写的槽**高 16 字节**；
# 改成 0 后两条支对齐。
stack = { slot_bytes = 8, first_offset_slots = 0 }
aliases = ["c"]  # riscv64 上的 C 约定
classify = [
  { when = { kind = "float", size_le = 8 },      do = { direct = { pool = "float" } } },
  { when = { kind = "aggregate", hfa_max = 2 },  do = { direct = { pool = "float", slots = "hfa" } } },
  { when = { kind = "aggregate", size_le = 16 }, do = { direct = { pool = "int", slots = 2 } } },
  { when = { kind = "aggregate", size_gt = 16 }, do = { indirect = { via = "caller_stack_copy" } } },
  { when = { kind = "scalar" },                  do = { direct = { pool = "int" } } },
]
ret_classify = [
  { when = { kind = "float", size_le = 8 },      do = { direct = { pool = "ret_float" } } },
  { when = { kind = "aggregate", hfa_max = 2 },  do = { direct = { pool = "ret_float", slots = "hfa" } } },
  { when = { kind = "aggregate", size_le = 16 }, do = { direct = { pool = "ret_int", slots = 2 } } },
  { when = { kind = "aggregate", size_gt = 16 }, do = { indirect = { via = "hidden_sret" } } },
  { when = { kind = "vector", size_gt = 16 },    do = { indirect = { via = "hidden_sret" } } },
  { when = { kind = "scalar" },                  do = { direct = { pool = "ret_int" } } },
]
hidden = { sret_pool = "int", sret_slot = 0, va_list = "riscv_save_area" }
callee_saved = { mechanism = "store_to_frame", pools = ["cs_gpr"], includes_link = true }
# 未命名实参**走寄存器**（a0-a7，溢出才上栈）——定本：「被调方把未被命名形参用掉的**整数**
# 参数寄存器按序存进**紧贴入口 sp** 的 save area，`va_list` 指向该区起点，于是先走完寄存器里的
# 变参、再接着走栈上的变参」（2026-10-01 核对 `riscv-cc.adoc` 原文；形状侧
# `riscv_save_area` 的 `contiguous = true` / `float_slot = 0` / `float_arg.step = 8` 与此配套）。
variadic_stack_only = false
# **变参按整数约定分类**：定本浮点调用约定一节写着 *"The remainder of this section applies only to
# named arguments. **Variadic arguments are passed according to the integer calling convention.**"*
# ⇒ LP64D 上变参的浮点也走整数寄存器（`double` 的位模式进 a0-a7）。这正是"单一线性游标"能成立的
# 前提（变参只有一个寄存器组）。**命名**形参仍按 `classify`（浮点进 F 寄存器组）。
variadic_classify = [
  { when = { kind = "aggregate", size_le = 16 }, do = { direct = { pool = "int", slots = 2 } } },
  { when = { kind = "aggregate", size_gt = 16 }, do = { indirect = { via = "caller_stack_copy" } } },
  { when = { kind = "scalar" },                  do = { direct = { pool = "int" } } },
]
tail_calls = { allowed = true, must_match_stack = true }
note = "HFA 用连续浮点槽表达；寄存器不足时整块走栈（A6 扩展）"
"#;

/// 内置约定文本（名字, TOML）。顺序即依赖顺序（子约定在父之后）。
pub const ALL: &[(&str, &str)] = &[
    ("c", C),
    ("win64", WIN64),
    ("sysv64", SYSV64),
    ("aapcs64", AAPCS64),
    ("lp64d", LP64D),
];

/// 解析全部内置约定（已展开继承）。
pub fn rules() -> Result<Vec<AbiRules>, AbiError> {
    let mut out: Vec<AbiRules> = Vec::new();
    for (_, text) in ALL {
        let mut r = AbiRules::from_toml(text)?;
        if let Some(p) = &r.parent {
            let parent = out
                .iter()
                .find(|x| &x.name == p)
                .ok_or_else(|| AbiError::BadRules {
                    name: r.name.clone(),
                    why: format!("内置约定的 parent `{p}` 不在 ALL 里（顺序错了）"),
                })?;
            r = r.merge_parent(parent);
        }
        r.validate()?;
        out.push(r);
    }
    Ok(out)
}
