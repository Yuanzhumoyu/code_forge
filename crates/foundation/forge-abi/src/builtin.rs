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
hidden = { sret_pool = "int", sret_slot = 0, va_list = "win64_stack", va_list_size = 8, va_list_align = 8 }
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
stack = { slot_bytes = 8, first_offset_slots = 1 }
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
hidden = { sret_pool = "int", sret_slot = 0, va_meta_pool = "va_meta", va_list = "sysv_reg_save", va_list_size = 24, va_list_align = 8 }
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
hidden = { sret_pool = "sret", sret_slot = 0, va_list = "aapcs64_struct", va_list_size = 32, va_list_align = 8 }
callee_saved = { mechanism = "store_to_frame", pools = ["cs_gpr"], includes_link = true }
extensions = { widen_to_bits = 32 }
variadic_stack_only = true
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
/// 变参未命名实参走栈。
///
/// **未启用** `va_len_pool`：历史上 RISC-V 生态有过"LEN 实参"的写法，本片不猜
/// psABI 细节（A6 按官方定本核对后再启用；引擎支持这条路径，见 `tests/invariants.rs`）。
pub const LP64D: &str = r#"
name = "lp64d"
parent = "c"
stack = { slot_bytes = 8, first_offset_slots = 2 }
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
hidden = { sret_pool = "int", sret_slot = 0, va_list = "riscv_save_area", va_list_size = 24, va_list_align = 8 }
callee_saved = { mechanism = "store_to_frame", pools = ["cs_gpr"], includes_link = true }
variadic_stack_only = true
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
