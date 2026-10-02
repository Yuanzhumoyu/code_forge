//! [`AbiRules`] — **约定本身**（平台无关的规则，可序列化成 TOML）。
//!
//! 每条字段都对应"某个真实约定里的一个事实"，而不是"某个 ISA 的实现细节"：
//! 规则里只出现**抽象的池名**（`int`/`float`/`vector`/`sret`…），具体寄存器由
//! [`crate::binding::AbiBinding`] 绑定。这样同一份 `win64` 规则可以被任何"寄存器编号
//! 与 Windows x64 一致"的 ISA 复用，而 ISA 换了名字/编号只改绑定。

use serde::{Deserialize, Serialize};

use crate::error::AbiError;
use crate::plan::CalleeSaveMechanism;

/// 寄存器位置的计数规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PositionRule {
    /// 按类各自计数（riscv SysV：整数与浮点序列独立推进）。
    #[default]
    ByClass,
    /// 按**位置**计数：第 i 个参数用第 i 个槽（Windows x64：第 2 个参数即使第 1 个是整数，
    /// 浮点也走 XMM1）——int/float 共用同一个位置游标。
    ByPosition,
}

/// 类型形状谓词（分类规则的 `when`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TyPred {
    /// 族：`int`/`ptr`/`float`/`vector`/`aggregate`/`other`/`scalar`（= 非聚合非向量）。
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub size_le: Option<u32>,
    #[serde(default)]
    pub size_gt: Option<u32>,
    /// 命中"同质浮点聚合"（HFA/HVA）且成员数 ≤ 该值。
    #[serde(default)]
    pub hfa_max: Option<u32>,
}

impl TyPred {
    /// 谓词是否命中该类型视图。
    pub fn matches(&self, ty: &crate::ty::TyView) -> bool {
        if let Some(k) = &self.kind {
            let ok = match k.as_str() {
                "int" => ty.is_int_like(),
                "ptr" => matches!(ty.kind, crate::ty::TyKind::Ptr),
                "float" => ty.is_float(),
                "vector" => ty.is_vector(),
                "aggregate" => ty.is_aggregate(),
                "other" => matches!(ty.kind, crate::ty::TyKind::Other),
                "scalar" => !ty.is_aggregate() && !ty.is_vector(),
                _ => false, // 未知族名恒不命中（规则自洽性由 `validate` 报出）
            };
            if !ok {
                return false;
            }
        }
        if let Some(n) = self.size_le
            && ty.size > n
        {
            return false;
        }
        if let Some(n) = self.size_gt
            && ty.size <= n
        {
            return false;
        }
        if let Some(n) = self.hfa_max
            && ty.homogeneous_float_agg(n).is_none()
        {
            return false;
        }
        true
    }
}

/// 间接传递的落点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndirectVia {
    /// 调用方在**自己的栈上**做副本并传指针（LLVM 的 `byval`；Win64 的 >8B 聚合、
    /// AAPCS64 的 >16B 聚合、RISC-V 的 >2×XLEN 聚合）。
    CallerStackCopy,
    /// 走"隐藏的间接结果指针"（LLVM `sret`）：指针占一个 hidden 槽
    /// （x86 RCX / AAPCS64 x8 / riscv a0）。
    HiddenSret,
}

/// 连续槽的个数：**定数**或**由类型决定**。
///
/// 定数不够用：HFA/AAPCS64 的 `{f32}` 占 1 个浮点槽、`{f32,f32,f32,f32}` 占 4 个——
/// 写死 `slots = 4` 会让单成员 HFA 白白吃掉 4 个寄存器（并把后面的参数挤到栈上），
/// 写死 `2` 更错。TOML 写法：`slots = 2`（定数）或 `slots = "hfa"`（同质浮点成员数）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SlotsSpec {
    Fixed(u32),
    /// 具名取数规则；目前只有 `"hfa"`（`TyView::homogeneous_float_agg` 的成员数）。
    Named(String),
}

impl Default for SlotsSpec {
    fn default() -> Self {
        SlotsSpec::Fixed(1)
    }
}

impl SlotsSpec {
    /// 在给定类型上求槽数。
    pub fn resolve(&self, ty: &crate::ty::TyView) -> Option<u32> {
        match self {
            SlotsSpec::Fixed(n) => Some(*n),
            SlotsSpec::Named(name) if name == "hfa" => ty.homogeneous_float_agg(u32::MAX),
            SlotsSpec::Named(_) => None,
        }
    }

    /// 具名取数规则的合法名字（`validate` 用；写错就报错，不做静默兜底）。
    pub const NAMES: &'static [&'static str] = &["hfa"];
}

/// 分类动作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ClassAction {
    /// 直接从某个池取 `slots` 个**连续**槽（`slots` 缺省 1）。
    Direct {
        pool: String,
        #[serde(default = "one")]
        slots: SlotsSpec,
    },
    /// 两个池各取一槽（`(int,float)` 拆分 / `(float,float)` 的 HFA ≤2）。
    Pair { lo: String, hi: String },
    /// 间接。
    Indirect { via: IndirectVia },
    /// 强制走栈（含对齐）。
    Stack {
        #[serde(default)]
        align: Option<u32>,
    },
    /// 不传递（例如返回值的 `Ignore`）。
    Ignore,
}

fn one() -> SlotsSpec {
    SlotsSpec::Fixed(1)
}

/// 一条分类规则：`when → do`（**顺序即优先级**，首条命中者胜）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassRule {
    pub when: TyPred,
    /// TOML 键是 `do`（`do` 是 Rust 关键字 → 字段名 `do_` + 显式 rename；
    /// 漏了这行 TOML 里写 `do` 会报 "unknown field `do`, expected `do_`"，
    /// 曾把内置约定的解析整片打挂）。
    #[serde(rename = "do")]
    pub do_: ClassAction,
}

/// callee-saved 规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalleeSavedRules {
    /// 保存机制（`none` = 完全没有 callee-saved，全 caller-saved）。
    #[serde(default)]
    pub mechanism: CalleeSaveMechanism,
    /// 有序池名列表（按保存顺序，池内按绑定顺序）。
    #[serde(default)]
    pub pools: Vec<String>,
    /// 帧指针也在保存之列（x86：RBP 由 prologue 保存，但**不算**"被调用者保存的通用寄存器"，
    /// 因此这里缺省 false，帧指针的保存由帧布局单独表达）。
    #[serde(default)]
    pub includes_fp: bool,
    /// 链接寄存器（ra/lr）也在保存之列（riscv/arm64：fp-inside 帧顶部保存）。
    #[serde(default)]
    pub includes_link: bool,
}

impl Default for CalleeSavedRules {
    fn default() -> Self {
        Self {
            mechanism: CalleeSaveMechanism::None,
            pools: Vec::new(),
            includes_fp: false,
            includes_link: false,
        }
    }
}

/// 被叫方弹栈规则（stdcall/fastcall/pascal）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CalleePop {
    /// 调用方弹栈（cdecl/SysV/AAPCS64/RISC-V…）。
    #[default]
    None,
    /// 被叫方按实际栈参数字节数弹栈（stdcall/pascal）。
    SumStackArgs,
    /// 固定字节数（thiscall 的 `ret 4` 等）。
    Fixed(u32),
}

/// 隐藏槽规则（sret 指针 / context / 变参元信息）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HiddenRules {
    /// 间接结果指针占哪个池的槽（`None` = 该约定不用隐藏指针，宽返回直接拒绝）。
    #[serde(default)]
    pub sret_pool: Option<String>,
    /// 池内第几槽（x86/riscv = 0；AAPCS64 用独立的 `x8` 池 → 0）。
    #[serde(default)]
    pub sret_slot: u32,
    /// context/env 指针（Swift `self`、Go context 寄存器、闭包 env）。
    #[serde(default)]
    pub context_pool: Option<String>,
    #[serde(default)]
    pub context_slot: u32,
    /// 变参"用了几个向量寄存器"的元信息寄存器（SysV 的 `%al`）。
    #[serde(default)]
    pub va_meta_pool: Option<String>,
    /// 变参"实际参数个数/字节数"元信息寄存器（RISC-V 生态里出现过的 `LEN` 实参；
    /// **内置约定都没启用**——psABI 现状以官方定本为准，A6 核对后再定）。
    #[serde(default)]
    pub va_len_pool: Option<String>,
    /// **`va_list` 的形状**（`None` = 该约定不支持变参）：预置名或显式表，见 [`VaListDecl`]。
    /// 形状本身（字段布局/保存区/取参规则）是**约定数据**，引擎只按数据算。
    #[serde(default)]
    pub va_list: Option<VaListDecl>,
}

/// `va_list` 的形状声明：**预置名**（`va_list = "win64_stack"`）或**显式表**
/// （`va_list = { size = …, align = …, fields = […], int_arg = { … }, … }`）。
///
/// 这里刻意**没有"每份约定一个枚举变体"**这种东西——形状全是数据：
///
/// - 引擎按字段**名字**把取参规则解析成下标，上限/步长由保存区槽表推（显式键优先）；
/// - 加一份约定/一台机器，写个预置名或直接写显式表即可，**不改任何代码**；
/// - 形状对不上（例如自定义约定的某个字段名写错）⇒ 读取侧 fail-closed，而不是套别的形状。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VaListDecl {
    /// 预置形状名（见 [`preset_va_shape`]）。
    Preset(String),
    /// 显式形状表（boxed：形状表比名字大得多，避免枚举本身被撑大）。
    Shape(Box<VaListShape>),
}

impl VaListDecl {
    /// 解析成形状。返回的引用借自 `self`（预置名查表命中时是 `'static`）。
    pub fn resolve(&self) -> Option<&VaListShape> {
        match self {
            VaListDecl::Preset(name) => preset_va_shape(name),
            VaListDecl::Shape(s) => Some(s),
        }
    }

    /// 诊断用的名字（预置名；显式表返回 `None`）。
    pub fn preset_name(&self) -> Option<&str> {
        match self {
            VaListDecl::Preset(name) => Some(name),
            VaListDecl::Shape(_) => None,
        }
    }
}

/// **`va_list` 的形状**（约定数据）：字段布局 + 保存区 + 取参规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaListShape {
    /// 对象字节数（`va_list_size`）。
    pub size: u32,
    /// 对象对齐（`size` 必须是它的整数倍）。
    pub align: u32,
    /// 对象字段（**名字是权威**：取参规则按名字引用字段）。
    pub fields: Vec<VaFieldDecl>,
    /// 寄存器保存区（`None` = 没有：未命名实参只在栈上，游标就是栈地址）。
    #[serde(default)]
    pub save: Option<VaSaveDecl>,
    /// 整数类实参的取参规则。
    pub int_arg: VaArgDecl,
    /// 浮点类实参的取参规则。
    pub float_arg: VaArgDecl,
}

/// `va_list` 对象里的一个字段声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaFieldDecl {
    pub name: String,
    pub offset: u32,
    pub size: u32,
}

/// 寄存器保存区声明：两类的**槽宽**（槽数与顺序由绑定的寄存器池给——那是 ISA 数据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct VaSaveDecl {
    /// 整数池每槽字节数。
    pub int_slot: u32,
    /// 浮点池每槽字节数（psABI 事实：SysV/AAPCS64 = 16，RISC-V = XLEN）。
    pub float_slot: u32,
}

/// 一类实参的取参规则声明（字段用**名字**引用，引擎解析成下标）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaArgDecl {
    /// 游标字段名。
    pub cursor: String,
    /// 偏移式游标的基准字段（地址 = 基准 + 游标）；缺省 = 游标本身就是要取的地址。
    #[serde(default)]
    pub base: Option<String>,
    /// 溢出/栈区字段；缺省 = 没有溢出支。
    #[serde(default)]
    pub overflow: Option<String>,
    /// 上限（缺省按保存区槽表推：有符号游标 = 0，否则 = 本类区域字节数）。
    #[serde(default)]
    pub limit: Option<u64>,
    /// 上限比较是否**按有符号**看（AAPCS64 的 `__gr_offs`/`__vr_offs` 是负数计数）。
    #[serde(default)]
    pub signed_limit: bool,
    /// 游标步长（缺省 = 本类槽宽）。
    #[serde(default)]
    pub step: Option<u32>,
    /// 溢出指针步长（缺省 = 整数槽宽）。
    #[serde(default)]
    pub overflow_step: Option<u32>,
}

/// **预置形状表**（`hidden.va_list = "…"` 引用的名字）。
///
/// 这是**数据**（psABI 事实的固化写法），不是引擎里的分支：引擎拿到的是解析后的
/// [`VaListShape`]，对形状名一无所知。要新的 psABI，加一条预置或直接写显式表。
pub fn preset_va_shape(name: &str) -> Option<&'static VaListShape> {
    use std::sync::OnceLock;
    static PRESETS: OnceLock<Vec<(String, VaListShape)>> = OnceLock::new();
    let table = PRESETS.get_or_init(|| {
        let f = |n: &str, o: u32, s: u32| VaFieldDecl {
            name: n.to_string(),
            offset: o,
            size: s,
        };
        let a =
            |cursor: &str, base: Option<&str>, overflow: Option<&str>, signed: bool| VaArgDecl {
                cursor: cursor.to_string(),
                base: base.map(str::to_string),
                overflow: overflow.map(str::to_string),
                limit: None,
                signed_limit: signed,
                step: None,
                overflow_step: None,
            };
        vec![
            // Win64：`va_list` = 一个指向栈上实参槽的游标（没有保存区、没有溢出支）。
            (
                "win64_stack".to_string(),
                VaListShape {
                    size: 8,
                    align: 8,
                    fields: vec![f("cursor", 0, 8)],
                    save: None,
                    int_arg: a("cursor", None, None, false),
                    float_arg: a("cursor", None, None, false),
                },
            ),
            // SysV AMD64：gp/fp 偏移游标 + 溢出指针 + 保存区（6×8B GP + 8×16B XMM）。
            (
                "sysv_reg_save".to_string(),
                VaListShape {
                    size: 24,
                    align: 8,
                    fields: vec![
                        f("gp_offset", 0, 4),
                        f("fp_offset", 4, 4),
                        f("overflow_arg_area", 8, 8),
                        f("reg_save_area", 16, 8),
                    ],
                    save: Some(VaSaveDecl {
                        int_slot: 8,
                        float_slot: 16,
                    }),
                    int_arg: a(
                        "gp_offset",
                        Some("reg_save_area"),
                        Some("overflow_arg_area"),
                        false,
                    ),
                    float_arg: a(
                        "fp_offset",
                        Some("reg_save_area"),
                        Some("overflow_arg_area"),
                        false,
                    ),
                },
            ),
            // AAPCS64：`{ __stack, __gr_top, __vr_top, __gr_offs, __vr_offs }`，两个游标都是
            // **有符号**计数（从负值数到 0；0 ⇒ 寄存器用完，改取 `__stack`）。
            (
                "aapcs64_struct".to_string(),
                VaListShape {
                    size: 32,
                    align: 8,
                    fields: vec![
                        f("__stack", 0, 8),
                        f("__gr_top", 8, 8),
                        f("__vr_top", 16, 8),
                        f("__gr_offs", 24, 4),
                        f("__vr_offs", 28, 4),
                    ],
                    save: Some(VaSaveDecl {
                        int_slot: 8,
                        float_slot: 16,
                    }),
                    int_arg: a("__gr_offs", Some("__gr_top"), Some("__stack"), true),
                    float_arg: a("__vr_offs", Some("__vr_top"), Some("__stack"), true),
                },
            ),
            // RISC-V LP64D：`area` 单指针（未命名实参只在栈上）+ 保存区（8×8B GP + 8×8B FP）。
            //
            // **`size = 8`（= 一个指针）不是随手写的**：定本 `riscv-cc.adoc` 的
            // 「`va_list`, `va_start`, and `va_arg`」一节第一句就是 *"The `va_list` type has the
            // same representation as `void*`"* ⇒ `sizeof(va_list)` = 指针宽度（LP64D = 8）。
            // 早先这里写 24 是从 sysv64 抄来的余量（那 16 字节既没字段也没人读），但它让
            // `va_copy`/把 `va_list` 交给 `vprintf` 一类**跨编译器**用法在对象布局上就对不上；
            // 守卫 `invariants.rs::va_object_layout_matches_the_psabi_numbers` 钉字段布局，
            // `va_shapes_match_the_documented_table` 钉这里的 size 与文档表逐字一致。
            (
                "riscv_save_area".to_string(),
                VaListShape {
                    size: 8,
                    align: 8,
                    fields: vec![f("area", 0, 8)],
                    save: Some(VaSaveDecl {
                        int_slot: 8,
                        float_slot: 8,
                    }),
                    int_arg: a("area", None, None, false),
                    float_arg: a("area", None, None, false),
                },
            ),
        ]
    });
    table.iter().find(|(n, _)| n == name).map(|(_, s)| s)
}

/// 扩展/符号性规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ExtensionRules {
    /// 形参/实参至少要扩展到多少位（AArch64 硬性 ≥32 位）。
    #[serde(default)]
    pub widen_to_bits: Option<u16>,
    /// 被叫方可以忽略高位（x86 SysV：调用方负责把 <64 位的值扩到 64 位；
    /// 而 Win64 要求调用方零/符号扩展，被叫方可只读低位）。
    #[serde(default)]
    pub callee_ignores_upper_bits: bool,
}

/// 尾调用规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TailCallRules {
    #[serde(default)]
    pub allowed: bool,
    /// `musttail` 语义：栈布局必须严格一致（否则拒绝）。
    #[serde(default)]
    pub must_match_stack: bool,
}

impl Default for TailCallRules {
    fn default() -> Self {
        Self {
            allowed: false,
            must_match_stack: true,
        }
    }
}

/// 栈参数的布置（被叫方视角）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackRules {
    /// 槽单位（字节；x86 = 8）。
    #[serde(default = "eight")]
    pub slot_bytes: u32,
    /// 被叫方第一个栈参数相对 `frame_base` 的槽数（x86 = 2：返回地址 + 保存的 fp）。
    #[serde(default)]
    pub first_offset_slots: u32,
    /// 参数从右往左入栈（stdcall/pascal）。
    #[serde(default)]
    pub right_to_left: bool,
}

fn eight() -> u32 {
    8
}

impl Default for StackRules {
    fn default() -> Self {
        Self {
            slot_bytes: 8,
            first_offset_slots: 0,
            right_to_left: false,
        }
    }
}

/// **一份调用约定**（可序列化 TOML）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbiRules {
    pub name: String,
    /// 继承：本规则只在父规则之上覆写列出的字段（用 `merge_parent` 展开）。
    #[serde(default)]
    pub parent: Option<String>,
    /// 本约定**额外响应**的约定名（缺省空；**不随 `parent` 继承**）。
    ///
    /// 用途：IR 的缺省 `CallConvId::Builtin(C)` 只是一个**抽象名**——"这台机器上的 C
    /// 就是 Win64 / AAPCS64 / LP64D"是平台事实。声明 `aliases = ["c"]` 即"本约定在这台
    /// 机器上代答 `c`"，于是 `c` 解析成**整套**约定（规则 + 该机器的绑定），而不是
    /// 拿 `c` 的抽象规则去配别家的寄存器（那样 by_class/by_position、返回池、sret 槽
    /// 全会错——这是实测踩过的坑）。
    ///
    /// 纪律：同一 ISA 上最多一份**有绑定的**约定能代答同一别名，多于一份 ⇒
    /// 明确报错（不按注册序猜）；宿主想覆写某台机器的 C，就注册自己的 `c` 规则
    /// （覆盖内置那份）+ `(ISA, "c")` 绑定——**显式绑定优先于别名**。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,

    // ── 位置与池 ──
    /// 位置计数规则。
    #[serde(default)]
    pub position: PositionRule,
    /// 整数/指针池名。
    #[serde(default = "pool_int")]
    pub int_pool: String,
    /// 浮点池名。
    #[serde(default = "pool_float")]
    pub float_pool: String,
    /// 向量池名（缺省 = 浮点池）。
    #[serde(default)]
    pub vector_pool: Option<String>,

    // ── 栈与帧（调用方视角的强制要求 + 被叫方视角的取参偏移）──
    /// 调用点栈对齐（字节）。
    #[serde(default = "sixteen")]
    pub stack_align: u32,
    /// 帧填充（x86 = 8：push rbp + callee-saved 后需凑回 16 对齐）。
    #[serde(default)]
    pub frame_padding: i32,
    /// 红区字节数（SysV AMD64 = 128；Win64 无）。
    #[serde(default)]
    pub red_zone: Option<u32>,
    /// 调用方为被叫方预留的 shadow space（Win64 = 32）。
    #[serde(default)]
    pub shadow_bytes: u32,
    #[serde(default)]
    pub stack: StackRules,

    // ── 分类 ──
    /// **参数位**分类规则（顺序即优先级）。
    #[serde(default)]
    pub classify: Vec<ClassRule>,
    /// **返回位**专属规则（同样按序，**先于** `classify` 判定）。
    ///
    /// 为什么必须分方向：同一个类型形状在两个方向的落点可以不同——AAPCS64 的
    /// 超 16B 聚合**当参数**是"调用方栈上副本的指针"（byval），**当返回**却是
    /// 隐藏的间接结果寄存器 **x8**；Win64 同理（参数 byref，返回 RCX sret）。
    /// 只写一份 `classify` 会把 x8/RCX 的 sret 语义错当成"返回值就是那个指针"。
    #[serde(default)]
    pub ret_classify: Vec<ClassRule>,
    /// 没有规则命中时的兜底（缺省 = 强制走栈；写不出就 fail-closed）。
    #[serde(default = "fallback_default")]
    pub fallback: ClassAction,

    // ── 保存/弹栈/hidden/扩展 ──
    #[serde(default)]
    pub callee_saved: CalleeSavedRules,
    #[serde(default)]
    pub callee_pop: CalleePop,
    #[serde(default)]
    pub hidden: HiddenRules,
    #[serde(default)]
    pub extensions: ExtensionRules,

    // ── 变参与尾调用 ──
    /// 变参实参**只能走栈**（Win64/AAPCS64/RISC-V 的未命名参数）。
    #[serde(default)]
    pub variadic_stack_only: bool,
    #[serde(default)]
    pub tail_calls: TailCallRules,
    /// 备注（人类可读：出处/已知偏差）。
    #[serde(default)]
    pub note: Option<String>,
}

fn pool_int() -> String {
    "int".into()
}
fn pool_float() -> String {
    "float".into()
}
fn sixteen() -> u32 {
    16
}
fn fallback_default() -> ClassAction {
    ClassAction::Stack { align: None }
}

impl AbiRules {
    /// 从 TOML 文本解析（`deny_unknown_fields`：写错的键直接报）。
    pub fn from_toml(text: &str) -> Result<Self, AbiError> {
        toml::from_str(text).map_err(|e| AbiError::Parse(e.to_string()))
    }

    /// 规则自洽性检查（结构化解析不做的那部分）。
    pub fn validate(&self) -> Result<(), AbiError> {
        let bad = |why: String| AbiError::BadRules {
            name: self.name.clone(),
            why,
        };
        if self.name.trim().is_empty() {
            return Err(bad("name 不能为空".into()));
        }
        for (i, a) in self.aliases.iter().enumerate() {
            if a.trim().is_empty() {
                return Err(bad(format!("aliases[{i}] 为空——要么删掉它，要么写约定名")));
            }
            if *a == self.name {
                return Err(bad(format!(
                    "aliases[{i}] = `{a}` 与自身 name 同名——别名是给**别的**约定名用的"
                )));
            }
            if self.aliases[..i].contains(a) {
                return Err(bad(format!("aliases 里 `{a}` 重复")));
            }
        }
        if self.stack.slot_bytes == 0 {
            return Err(bad("stack.slot_bytes 不能为 0".into()));
        }
        // shadow space（调用方在 call 前预留）必须是槽单位的整数倍——这条校验原先在
        // 谱面（`[abi.stack_args].shadow_bytes`，v20 A5-3 已删），现在归规则：
        // 它是**约定事实**，谱里没有可校验的键了。
        if self.shadow_bytes != 0 {
            let unit = self.stack.slot_bytes;
            if unit > 1 && !self.shadow_bytes.is_multiple_of(unit) {
                return Err(bad(format!(
                    "shadow_bytes = {} 必须是槽单位（{} 字节）的正整数倍",
                    self.shadow_bytes, unit
                )));
            }
        }
        if self.stack_align == 0 || !self.stack_align.is_power_of_two() {
            return Err(bad(format!(
                "stack_align = {} 必须是 2 的幂",
                self.stack_align
            )));
        }
        for (side, list) in [
            ("classify", &self.classify),
            ("ret_classify", &self.ret_classify),
        ] {
            for (i, r) in list.iter().enumerate() {
                if let Some(k) = &r.when.kind
                    && !matches!(
                        k.as_str(),
                        "int" | "ptr" | "float" | "vector" | "aggregate" | "other" | "scalar"
                    )
                {
                    return Err(bad(format!("{side}[{i}].when.kind = `{k}` 不是已知族名")));
                }
                if r.when.size_le.is_none()
                    && r.when.size_gt.is_none()
                    && r.when.kind.is_none()
                    && r.when.hfa_max.is_none()
                {
                    return Err(bad(format!(
                        "{side}[{i}] 的 when 没有任何条件——它会吃掉后面所有规则（要么写条件，要么用 fallback）"
                    )));
                }
                if let ClassAction::Direct { slots, .. } = &r.do_
                    && let SlotsSpec::Named(n) = slots
                    && !SlotsSpec::NAMES.contains(&n.as_str())
                {
                    return Err(bad(format!(
                        "{side}[{i}].do.direct.slots = `{n}` 不是已知的取数规则（已知：{}）",
                        SlotsSpec::NAMES.join(", ")
                    )));
                }
            }
        }
        if let ClassAction::Direct { slots, .. } = &self.fallback
            && let SlotsSpec::Named(n) = slots
            && !SlotsSpec::NAMES.contains(&n.as_str())
        {
            return Err(bad(format!(
                "fallback.direct.slots = `{n}` 不是已知的取数规则（已知：{}）",
                SlotsSpec::NAMES.join(", ")
            )));
        }
        Ok(())
    }

    /// 把父规则的字段并进自身（本规则里显式写了的字段为准）。
    ///
    /// 覆盖是**整字段**级别（不是深合并）：`callee_saved`/`hidden`/`stack` 这类子表
    /// 一旦写出就整体替换——避免"父的某个池混进子的私有项"这种隐性合并。
    pub fn merge_parent(&self, parent: &AbiRules) -> AbiRules {
        let mut out = self.clone();
        // 用 serde 的"字段是否被显式写出"很难判，这里按"本规则是否为默认值"决定：
        // 默认值 = 未覆写。分组字段（callee_saved/hidden/stack/classify）里，
        // classify/callee_saved.pools 非空即视为覆写，其余子表用"非默认"判定。
        if out.position == PositionRule::default() {
            out.position = parent.position;
        }
        if out.int_pool == pool_int() {
            out.int_pool = parent.int_pool.clone();
        }
        if out.float_pool == pool_float() {
            out.float_pool = parent.float_pool.clone();
        }
        if out.vector_pool.is_none() {
            out.vector_pool = parent.vector_pool.clone();
        }
        if out.stack_align == sixteen() {
            out.stack_align = parent.stack_align;
        }
        if out.frame_padding == 0 {
            out.frame_padding = parent.frame_padding;
        }
        if out.red_zone.is_none() {
            out.red_zone = parent.red_zone;
        }
        if out.shadow_bytes == 0 {
            out.shadow_bytes = parent.shadow_bytes;
        }
        if out.stack == StackRules::default() {
            out.stack = parent.stack.clone();
        }
        if out.classify.is_empty() {
            out.classify = parent.classify.clone();
        }
        if out.ret_classify.is_empty() {
            out.ret_classify = parent.ret_classify.clone();
        }
        if out.fallback == fallback_default() {
            out.fallback = parent.fallback.clone();
        }
        if out.callee_saved == CalleeSavedRules::default() {
            out.callee_saved = parent.callee_saved.clone();
        }
        if out.callee_pop == CalleePop::None {
            out.callee_pop = parent.callee_pop;
        }
        if out.hidden == HiddenRules::default() {
            out.hidden = parent.hidden.clone();
        }
        if out.extensions == ExtensionRules::default() {
            out.extensions = parent.extensions.clone();
        }
        if !out.variadic_stack_only {
            out.variadic_stack_only = parent.variadic_stack_only;
        }
        if out.tail_calls == TailCallRules::default() {
            out.tail_calls = parent.tail_calls.clone();
        }
        out.parent = None;
        out
    }
}
