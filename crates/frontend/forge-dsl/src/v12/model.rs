//! v12 ISA 模型 — 唯一 DSL 模型（严格 TOML，无 v11 字符串语法层）。
//!
//! 设计原则：
//! - 一切用户语法都是 TOML 数据：命名位域（SLEIGH 风格）、编码形式（语义键组合）、
//!   结构化谓词（`when`，迭代 4 定型）。仅 `asm`/`lowering.insts`/`emit.insts`
//!   保留模板字符串（带 `{0}`/`{name}` 占位符，语法在文档中定义）。
//! - `deny_unknown_fields`：未知键/表一律报错（严格 TOML；v11 文件必然无法解析）。
//! - 位域位置从 `[conventions.bitfields]` 推导，指令不再写位偏移；
//!   ModRM/REX/VEX 等 x86 机制降级为 form 的**语义键**（`modrm = "rr"` 等），
//!   实现为 forge-dsl 内部函数，语义键集合在迭代 3/4 定型。

use quote::quote;
use serde::{Deserialize, Serialize, de::Visitor};
use std::{
    collections::BTreeMap,
    fmt::{self, Display},
    str::FromStr,
};

/// v12 顶层模型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V12Model {
    pub meta: Meta,
    /// 寄存器组（`[reg.NAME]`）。
    pub reg: BTreeMap<RegClass, RegGroup>,
    /// ISA 约定（位域/ModRM/REX/操作数宽度前缀）。
    #[serde(default)]
    pub conventions: Conventions,
    /// 操作数槽（`[[operand_slots]]`）。
    pub operand_slots: Vec<OperandSlot>,
    /// 编码形式（`[[forms]]`）。
    #[serde(default)]
    pub forms: Vec<Form>,
    /// 指令（`[[instructions]]`）。
    #[serde(default)]
    pub instructions: Vec<Instruction>,
    /// 参数化指令族（`[[families]]`）。
    #[serde(default)]
    pub families: Vec<Family>,
    /// 指令选择规则（`[[lowering]]`）。
    #[serde(default)]
    pub lowering: Vec<Lowering>,
    /// 调用约定（`[abi]`，为 YMM by-ref 铺路）。
    #[serde(default)]
    pub abi: Option<Abi>,
    /// 序言/尾声（`[emit]`）。
    #[serde(default)]
    pub emit: Option<EmitSection>,
    /// 溢出模板（`[spill.GPR]`/`[spill.FPR]`：load/store 指令 + 基址寄存器）。
    #[serde(default)]
    pub spill: BTreeMap<String, SpillTemplate>,
}

// ─────────────────────────── [meta] ───────────────────────────

/// ISA 元信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    /// ISA 名（派生生成模块名：小写 + `-`→`_`）。
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default = "default_endian")]
    pub endian: Endian,
    /// 默认模式（x86 64 位模式 = 64）。
    #[serde(default = "default_mode")]
    pub mode: u8,
    /// 定宽指令编码（riscv/aarch64 = 32）。缺省 → 变长 ISA。
    #[serde(default)]
    pub default_inst_width: Option<u32>,
    /// 变长 ISA 标志（x86）。与 `default_inst_width` 互斥。
    #[serde(default)]
    pub variable_length: bool,
    /// 变长 ISA 最大指令长度（x86 = 15）。
    #[serde(default)]
    pub max_inst_len: Option<u8>,
    /// 控制寄存器解析是否大小写不敏感。
    #[serde(default)]
    pub case_insensitive_regs: Option<bool>,
    /// 行注释起始字符（缺省 "#"）。
    #[serde(default = "default_comment_char")]
    pub comment_char: String,
    /// 标签定义后缀（缺省 ":"；如 "foo:"）。
    #[serde(default = "default_label_suffix")]
    pub label_suffix: String,
    /// 助记符大小写策略（缺省 insensitive）。
    #[serde(default)]
    pub mnemonic_case: MnemonicCase,
    /// 立即数前缀（缺省无；x86 AT&T 可设 "$"，ARM 可设 "#"）。
    #[serde(default)]
    pub imm_prefix: Option<String>,
    /// 伪指令前缀（缺省 "."）。
    #[serde(default = "default_directive_prefix")]
    pub directive_prefix: String,
    /// 缺省 opsize（位；如 32/64）。无显式 opsize 语义的 form 在 decode 时
    /// 的 `__opsize` 初始值（影响 REX.W/宽度 guard 的缺省判定）。缺省 None
    /// = 现状（decode 初始 4 = 32 位）。
    #[serde(default)]
    pub default_opsize: Option<u8>,
}

fn default_comment_char() -> String {
    "#".into()
}

fn default_label_suffix() -> String {
    ":".into()
}

fn default_directive_prefix() -> String {
    ".".into()
}

/// 助记符大小写策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MnemonicCase {
    /// 大小写不敏感（缺省；assemble 时统一小写匹配）。
    #[default]
    Insensitive,
    /// 大小写敏感（精确匹配模板首词）。
    Sensitive,
}

fn default_endian() -> Endian {
    Endian::Little
}

fn default_mode() -> u8 {
    64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Endian {
    Little,
    Big,
}

// ─────────────────────────── [reg.*] ───────────────────────────
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RegClass {
    /// 通用整数寄存器，payload = 字节宽度（任意 ISA 自定义宽度）。
    GPR(u16),
    /// 浮点寄存器，payload = 字节宽度。
    FPR(u16),
    /// 向量寄存器，payload = 字节宽度。
    VEC(u16),
    /// 掩码寄存器（如 x86 AVX-512 k0-k7），payload = 字节宽度。
    KReg(u16),
}

impl RegClass {
    pub fn width(&self) -> u16 {
        match self {
            RegClass::GPR(w) | RegClass::FPR(w) | RegClass::VEC(w) | RegClass::KReg(w) => *w,
        }
    }
}

impl FromStr for RegClass {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(width) = s.strip_prefix("gpr") {
            if width.is_empty() {
                return Ok(RegClass::GPR(4));
            }
            let width: u16 = width
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            Ok(RegClass::GPR(width))
        } else if let Some(width) = s.strip_prefix("fpr") {
            if width.is_empty() {
                return Ok(RegClass::FPR(4));
            }
            let width: u16 = width
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            Ok(RegClass::FPR(width))
        } else if let Some(width) = s.strip_prefix("vec") {
            let width: u16 = width
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            Ok(RegClass::VEC(width))
        } else if let Some(width) = s.strip_prefix("kreg") {
            if width.is_empty() {
                return Ok(RegClass::KReg(8));
            }
            let width: u16 = width
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            Ok(RegClass::KReg(width))
        } else {
            Err("invalid reg class".to_string())
        }
    }
}

impl fmt::Display for RegClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegClass::GPR(width) => write!(f, "gpr{}", width),
            RegClass::FPR(width) => write!(f, "fpr{}", width),
            RegClass::VEC(width) => write!(f, "vec{}", width),
            RegClass::KReg(width) => write!(f, "kreg{}", width),
        }
    }
}

impl Serialize for RegClass {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

struct RegClassVisitor;

impl<'de> Visitor<'de> for RegClassVisitor {
    type Value = RegClass;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string like 'gpr2', 'fpr4', 'vec8', or 'kreg8'")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        // We use our FromStr implementation to parse the string.
        RegClass::from_str(value)
            .map_err(|_| E::custom(format!("invalid format for RegClass: '{}'", value)))
    }
}

impl<'de> Deserialize<'de> for RegClass {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(RegClassVisitor)
    }
}

impl quote::ToTokens for RegClass {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let class = match self {
            RegClass::GPR(w) => quote! { forge_ir::RegClass::GPR(#w)},
            RegClass::FPR(w) => quote! { forge_ir::RegClass::FPR(#w)},
            RegClass::VEC(w) => quote! { forge_ir::RegClass::VEC(#w)},
            RegClass::KReg(w) => quote! { forge_ir::RegClass::KReg(#w)},
        };
        tokens.extend(class);
    }
}

/// 寄存器组。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegGroup {
    /// 显式寄存器名；缺省时由 `prefix` + 序号生成。
    #[serde(default)]
    pub names: Option<Vec<String>>,
    /// 生成名称的前缀（"R" → R0, R1, ...）。
    #[serde(default)]
    pub prefix: Option<String>,
    /// 首成员物理寄存器号偏移（x86 gpr8h 用 4）。
    #[serde(default)]
    pub base_index: Option<u32>,
    /// 组内寄存器数（仅 `names` 缺省时需要）。
    #[serde(default)]
    pub count: Option<u16>,
}

// ─────────────────────── [conventions.*] ───────────────────────

/// ISA 约定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Conventions {
    /// 命名位域（SLEIGH 风格），form/指令引用。
    #[serde(default)]
    pub bitfields: BTreeMap<String, Bitfield>,
    /// ModRM 结构约定（x86 家族）。
    #[serde(default)]
    pub modrm: Option<ModrmConvention>,
    /// REX 前缀约定（x86-64）。
    #[serde(default)]
    pub rex: Option<RexConvention>,
    /// 操作数宽度 → 强制前缀字节（x86：16 → 0x66）。
    /// 键为数字字符串（TOML 裸整数键），校验时解析为 u32。
    #[serde(default)]
    pub opsize_prefix: Option<BTreeMap<String, u64>>,
    /// 条件码表（name → 编码值）。cond 槽必须引用本表；缺省 = x86 16 项
    /// （o/no/b/ae/e/ne/be/a/s/ns/p/np/l/ge/le/g ↔ 0..15）。
    #[serde(default)]
    pub cond: Option<BTreeMap<String, u64>>,
    /// 变长解码前缀扫描表：条目 = 单字节或范围 + 效果集
    /// （"opsize16"/"lock"/"repe"/"repne"/"addr16"/"rex"）。缺省 = x86 扫描集。
    #[serde(default)]
    pub prefix_scan: Option<Vec<PrefixScanEntry>>,
}

/// 前缀扫描条目：`byte`（单字节）与 `range`（如 "0x40..0x4F"）二选一。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixScanEntry {
    #[serde(default)]
    pub byte: Option<u64>,
    /// "0x40..0x4F" / "0x40..=0x4F"（闭区间）。
    #[serde(default)]
    pub range: Option<String>,
    /// 效果："opsize16"（66 → opsize=2）、"lock"、"repe"、"repne"、
    /// "addr16"、"rex"（40-4F：REX.R/B/W 位）。
    pub effects: Vec<String>,
}

/// 命名位域：定宽 ISA 的编码单元。
///
/// 二选一（校验强制）：
/// - 单一位段：`{ offset, width }`；
/// - 散布位段：`{ pieces = [ { offset, width, shift }, ... ] }` —— 立即数分段
///   放置（S/U/B/J 型）。编码：`word |= ((value >> shift) & mask) << offset`；
///   解码：`value |= ((word >> offset) & mask) << shift`（可逆）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bitfield {
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub pieces: Option<Vec<BitfieldPiece>>,
}

/// 散布位段的一块：`value >> shift` 取 `width` 位，放置于 `offset`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BitfieldPiece {
    pub offset: u32,
    pub width: u32,
    /// 放置前的右移（缺省 0）。
    #[serde(default)]
    pub shift: u32,
}

/// ModRM 约定（表存在即启用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModrmConvention {
    /// 持有 ModRM.reg（3 位）的位域名（定宽 ISA）。
    #[serde(default)]
    pub reg_field: Option<String>,
    /// 持有 ModRM.rm（3 位，+REX.X 扩展第 4 位）的位域名（定宽 ISA）。
    #[serde(default)]
    pub rm_field: Option<String>,
    /// 必须强制带位移的 base 寄存器号（mod=00+rm=101 是 RIP-relative，
    /// 无法表达 [base]；x86 = [5, 13]）。
    #[serde(default)]
    pub force_disp_base: Vec<u8>,
}

/// REX 前缀约定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RexConvention {
    /// 触发 REX.W=1 的操作数宽度（位；x86 = 64）。
    #[serde(default)]
    pub w_opsize: Option<u32>,
}

// ──────────────────── [[operand_slots]] ────────────────────

/// 操作数槽：指令操作数的抽象类别。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperandSlot {
    pub name: String,
    pub kind: OperandKind,
    /// reg：所属 [reg.*] 组名（单类型糖，等价于 `classes = ["gpr8"]`）。
    #[serde(default)]
    pub class: Option<RegClass>,
    /// reg：可接纳的寄存器组集合（多宽度/多类型）。`class` 为单元素糖；
    /// 两者皆无 = 任意寄存器类（不推荐，会吞掉更具体的重载形式）。
    #[serde(default)]
    pub classes: Option<Vec<RegClass>>,
    /// reg：编码宽度（位）。
    #[serde(default)]
    pub field_width: Option<u32>,
    /// reg：8 位寄存器操作数（spl/bpl/sil/dil 无 REX 时编码为 ah/ch/dh/bh，
    /// 索引 4-7 必须强制 REX 前缀）。
    #[serde(default)]
    pub byte_reg: Option<bool>,
    /// imm：值宽度（位）。
    #[serde(default)]
    pub width: Option<u32>,
    /// imm：有符号（缺省 false）。
    #[serde(default)]
    pub signed: Option<bool>,
    /// imm：允许浮点立即数（IEEE-754 位模式存储）。
    #[serde(default)]
    pub float: Option<bool>,
    /// imm：最小值约束（缺省 = 按 width/signed 推导）。
    #[serde(default)]
    pub min: Option<i64>,
    /// imm：最大值约束（缺省 = 按 width/signed 推导）。
    #[serde(default)]
    pub max: Option<i64>,
    /// imm：允许的枚举值集合。
    #[serde(default)]
    pub values: Option<Vec<i64>>,
    /// 该槽可承担的角色；缺省 ["in"]。"inout" = 读改写（in 且 out）。
    #[serde(default)]
    pub roles: Option<Vec<OperandRole>>,
}

impl OperandSlot {
    /// 规范化寄存器约束集：`classes` 优先；否则 `class` → 单元素；两者皆无 → None
    /// （任意寄存器类）。
    pub fn classes(&self) -> Option<Vec<RegClass>> {
        if let Some(cs) = &self.classes {
            return Some(cs.clone());
        }
        self.class.map(|c| vec![c])
    }

    /// 立即数/标签槽的值域：(min, max)。缺省由 width/signed 推导。
    pub fn imm_range(&self) -> Option<(i64, i64)> {
        if self.kind != OperandKind::Imm && self.kind != OperandKind::Label {
            return None;
        }
        let w = self.width.unwrap_or(64);
        let (lo, hi) = if self.signed.unwrap_or(false) && w < 64 {
            let half = 1i64 << (w - 1);
            (-half, half - 1)
        } else if w >= 64 {
            (i64::MIN, i64::MAX)
        } else {
            (0, (1i64 << w) - 1)
        };
        Some((self.min.unwrap_or(lo), self.max.unwrap_or(hi)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperandKind {
    Reg,
    Imm,
    Mem,
    Label,
    Cond,
}

impl OperandKind {
    /// 人类可读名称（错误消息用）。
    pub fn kind_name(self) -> &'static str {
        match self {
            OperandKind::Reg => "reg",
            OperandKind::Imm => "imm",
            OperandKind::Mem => "mem",
            OperandKind::Label => "label",
            OperandKind::Cond => "cond",
        }
    }
}

/// 操作数角色。
///
/// - `in`：只读源
/// - `out`：只写目标
/// - `inout`：读改写（同时是源与目标，如 x86 `ADD RM, R` 的 RM、`XCHG`）——
///   减少模板里同一操作数重复声明 in/out 的冗余。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperandRole {
    In,
    Out,
    InOut,
}

// ──────────────────────── [[forms]] ────────────────────────

/// 编码形式：语义键组合，替代 v11 编码字符串。
///
/// 语义键（modrm/rex/vex/prefix/escape）是**开放集合**：迭代 3/4 按需扩展
/// （sib/leb128/reloc/...），每个键对应 forge-dsl 内部一个发射/解码实现。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Form {
    pub name: String,
    /// ModRM 结构之前的 opcode 字节数。
    #[serde(default)]
    pub opcode_bytes: Option<u8>,
    /// ModRM 结构键（迭代 3：`"rr"` reg=op0+rm=op1、`"ext"` reg=fields.ext
    /// +rm=op0；迭代 3b+：`"rm_mem"` 等内存形式）。
    #[serde(default)]
    pub modrm: Option<String>,
    /// 固定 ModRM 字节（无操作数指令如 MFENCE 0F AE F0：mod=11/reg/rm 全固定）。
    #[serde(default)]
    pub modrm_fixed: Option<u64>,
    /// REX 发射策略（"auto" | "never" | ...）。
    #[serde(default)]
    pub rex: Option<String>,
    /// VEX 结构（map/pp/l 来源；迭代 4）。
    #[serde(default)]
    pub vex: Option<VexSpec>,
    /// EVEX 结构（AVX-512；复用 [`VexSpec`] 数据键 map/pp/w/l，l=0/1/2 →
    /// L'L=128/256/512 位；最小集：reg-reg、无 opmask/broadcast）。
    #[serde(default)]
    pub evex: Option<VexSpec>,
    /// 变长：固定前缀来源。`"field"` → fields.prefix（SSE 的 66/F2/F3/0）；
    /// 数字字符串 → 固定字节。缺省无前缀。
    #[serde(default)]
    pub prefix: Option<String>,
    /// 变长：编码宽度语义。`opsize = <操作数序号>`：宽度由该操作数的寄存器
    /// 自动推导（RAX→64 发 REX.W、EAX→32 无前缀、AX→16 发 0x66）；缺省 =
    /// 第一个 Reg 槽操作数。槽声明固定宽度组（如 `[gpr64]`）时推导恒为该值
    /// 并在 encode 期校验操作数寄存器宽度匹配（严格类型检测）。
    #[serde(default)]
    pub opsize: Option<Opsize>,
    /// 变长：REX.W 位来源。`"auto"` → opsize==64；`"field"` → fields.w；
    /// `"always"` → 恒发 REX.W（+r 形式的 mov_imm64/bswap）。
    /// 缺省恒 0（REX.W 仅在 reg/rm≥8 时随 REX 出现）。
    #[serde(default)]
    pub rex_w: Option<String>,
    /// 变长：opcode 含寄存器低 3 位（`+r` 形式：50+r/push、58+r/pop、
    /// B8+r/mov_imm64、C8+r/bswap）。opcode 字节 = 基值 | (op0 & 7)；
    /// REX.B = op0>>3；无 ModRM。
    #[serde(default)]
    pub opcode_reg: Option<u64>,
    /// 变长：尾部立即数宽度（位；如 32）。指令最后操作数（imm 槽）编码于此。
    #[serde(default)]
    pub imm: Option<u32>,
    /// 强制 escape 字节（x86：[0x0F]）。
    #[serde(default)]
    pub escape: Option<Vec<u8>>,
    /// 定宽：主 opcode 所在位域名。
    #[serde(default)]
    pub opcode_field: Option<String>,
    /// 定宽：按操作数位置绑定位域名（第 i 个操作数 → bitfields[i]）。
    /// 操作数少于该列表时，多余位域取 `fields` 固定值或隐式 0。
    #[serde(default)]
    pub operand_fields: Option<Vec<String>>,
    /// 该形式接受的操作数槽（编码顺序）。
    #[serde(default)]
    pub operand_slots: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Opsize {
    Slot(u16),
    Reg(u16),
}

impl Default for Opsize {
    fn default() -> Self {
        Self::Slot(0)
    }
}

impl FromStr for Opsize {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let n = s[1..].parse::<u16>().map_err(|e| e.to_string())?;
        match s.chars().next() {
            Some('s') => Ok(Self::Slot(n)),
            Some('r') => Ok(Self::Reg(n)),
            _ => Err("opsize must start with 's' or 'r'".to_string()),
        }
    }
}

impl Display for Opsize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Opsize::Slot(idx) => write!(f, "s{}", idx),
            Opsize::Reg(width) => write!(f, "r{}", width),
        }
    }
}

impl Serialize for Opsize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

struct OpsizeVisitor;

impl<'de> Visitor<'de> for OpsizeVisitor {
    type Value = Opsize;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "an opsize")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Self::Value::from_str(v).map_err(|e| E::custom(format!("invalid opsize: {}", e)))
    }
}

impl<'de> Deserialize<'de> for Opsize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(OpsizeVisitor)
    }
}

/// VEX 字段来源声明（迭代 4）。
///
/// map/pp/w/l 每个值为：数字（固定）或 `"field"`（取指令 `fields.vex_map`/
/// `vex_pp`/`vex_w`/`vex_l`，缺省 0）。vvvv 语义由操作数数量决定：
/// 3 操作数（VEX_RRV 类）→ vvvv = ~op2；2 操作数 → vvvv = 0x0F（无源）。
/// EVEX 复用本结构（`form.evex`），额外键 b（broadcast）与 disp_scale
/// （压缩位移缩放 N）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VexSpec {
    /// VEX.mmmmm 来源（数字或 "field"）。
    #[serde(default)]
    pub map: Option<String>,
    /// VEX.pp 来源。
    #[serde(default)]
    pub pp: Option<String>,
    /// VEX.W 来源。
    #[serde(default)]
    pub w: Option<String>,
    /// VEX.L 来源（EVEX：L'L 值 0/1/2 → 128/256/512 位）。
    #[serde(default)]
    pub l: Option<String>,
    /// EVEX 专用：broadcast 位（"0"/"1" 或 "field" → fields.evex_b）。
    #[serde(default)]
    pub b: Option<String>,
    /// EVEX 专用：零掩码位 z（"0"/"1" 或 "field" → fields.evex_z；P2 bit7）。
    #[serde(default)]
    pub z: Option<String>,
    /// EVEX 专用：压缩位移缩放 N（"1"/"2"/"4"/"8"/"16"/"32"/"64"；
    /// "field" → fields.evex_disp_scale；缺省 1 = 不缩放）。
    #[serde(default)]
    pub disp_scale: Option<String>,
}

// ──────────────────── [[instructions]] ────────────────────

/// 单条指令。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    /// 内部 Rust 标识符（Inst 变体名；与汇编助记符解耦，可含语义后缀）。
    pub name: String,
    pub form: String,
    /// 主 opcode（值或首个 opcode 字节）。
    #[serde(default)]
    pub opcode: Option<u64>,
    /// 固定字段值（funct3/funct7/前缀字节...），按位域名引用。
    #[serde(default)]
    pub fields: Option<BTreeMap<String, u64>>,
    /// 汇编模板（必填，完整格式）：`"add {0:[gpr:in]}, {1:[gpr:inout]}"`。
    /// 首词 = 汇编助记符（唯一事实来源，替代已删除的 mnemonic 字段）；
    /// 操作数占位符 `{i:[槽:角色]}` 内联声明操作数（角色缺省 in）。
    pub asm: String,
    /// 结构化谓词（迭代 4 定型：{ and = [...], eq = [...] }）。
    #[serde(default)]
    pub when: Option<toml::Value>,
    /// 指令级 VEX 字段覆盖。
    #[serde(default)]
    pub vex: Option<VexSpec>,
    /// 指令级 opsize 覆盖（form 的 opsize 优先级低）：`opsize = <操作数序号>`
    /// ——宽度由该操作数寄存器自动推导（REX.W/66 前缀驱动）。多宽度合并
    /// （cvtsi2sd 32/64 源）用：同助记符 + opsize 驱动 REX.W 自动分发。
    #[serde(default)]
    pub opsize: Option<Opsize>,
    /// 指令级 rex_w 覆盖（form 的 rex_w 优先级低）："auto" → opsize==64。
    #[serde(default)]
    pub rex_w: Option<String>,
    /// 效果标签（Pure/Read/Write/Branch/Jump/Call/Ret；缺省 Pure）。
    /// 驱动 MachineInst::effects/is_branch/is_call/is_ret（TargetMachine 集成）。
    #[serde(default)]
    pub effect: Vec<String>,
    /// 隐式破坏的物理寄存器名（如 cqo 的 RDX、idiv 的 RAX/RDX）——regalloc
    /// 在本指令点避开（MachineInst::clobbers）。与 lowering 模板的显式物理
    /// 寄存器（collect_phys_clobbers）互补：这是指令自身的隐式写。
    #[serde(default)]
    pub implicit_regs: Option<Vec<String>>,
    /// 全局地址重定位语义（GlobalAddr lowering 专用指令）：
    /// - `"abs8"`：imm 槽 < 0 编码 GlobalId → ABS8 "G{id}"（x86 MOVABS_GLOBAL）
    /// - `"pcrel_hi"`/`"pcrel_lo"`：PC-relative hi20/lo12 对（riscv
    ///   AUIPC_GLOBAL/ADDI_GLOBAL；patcher 按 opcode 分写位段）
    /// 生成器按此字段生成 encoder reloc arm——替代按指令名特判。
    #[serde(default)]
    pub global_reloc: Option<String>,
}

/// 操作数使用：槽 + 角色 + （定宽）位域绑定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperandUse {
    /// [[operand_slots]] 槽名。
    pub slot: String,
    /// 角色覆盖（缺省 "in"）。
    #[serde(default)]
    pub role: Option<OperandRole>,
    /// 定宽：该操作数编码到的位域名（rd/rs1/rs2...）。
    #[serde(default)]
    pub field: Option<String>,
}

// ──────────────────────── [[families]] ────────────────────────

/// 参数化指令族：共享 form/操作数布局的变体集合。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Family {
    pub name: String,
    pub form: String,
    /// 家族共享固定字段（如 SSE 的 prefix/w）；variant.fields 覆盖/追加。
    #[serde(default)]
    pub fields: Option<BTreeMap<String, u64>>,
    /// 家族共享 asm 模板（完整格式；`{name}` 占位符替换为变体名小写 =
    /// 变体助记符；操作数占位符内联声明，如 `"addps {0:[fpr:out]}, ..."`）。
    pub asm: String,
    pub variants: Vec<FamilyVariant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FamilyVariant {
    pub name: String,
    #[serde(default)]
    pub opcode: Option<u64>,
    #[serde(default)]
    pub fields: Option<BTreeMap<String, u64>>,
    /// 变体级 asm 覆盖（罕见；缺省用家族模板 `{name}` 展开）。
    #[serde(default)]
    pub asm: Option<String>,
    #[serde(default)]
    pub when: Option<toml::Value>,
}

// ──────────────────────── [[lowering]] ────────────────────────

/// 指令选择规则。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lowering {
    /// IR 操作名（"Iadd"）。
    pub op: String,
    /// 符号化指令模板：`"{out} = MOV_R_RM {0}, {1}"`。
    pub insts: Vec<String>,
    /// 结构化谓词（宽度条件化 lowering）。
    #[serde(default)]
    pub when: Option<toml::Value>,
}

// ───────────────────────── [abi] ─────────────────────────

/// 调用约定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Abi {
    #[serde(default)]
    pub stack_align: Option<u32>,
    #[serde(default)]
    pub arg_class: Vec<ArgClass>,
    /// 帧布局（sp/fp 寄存器名、帧分配/释放指令名）。
    #[serde(default)]
    pub frame: Option<AbiFrame>,
    /// 被调用者保存寄存器（prologue push / epilogue pop 顺序）。
    #[serde(default)]
    pub callee_saved: Option<CalleeSaved>,
    /// 溢出 scratch 寄存器（spill load/store 用；x86 R10/R11）。
    #[serde(default)]
    pub scratch: Vec<String>,
    /// `@move_args` 收参移动指令名（缺省 "MOV_RM8_R64"——x86 语义；demo 等
    /// 定宽 ISA 可声明自己的 mov 指令名，如 "MOV64"）。
    #[serde(default)]
    pub move_inst: Option<String>,
    /// Return 返回值→返回寄存器移动指令名（缺省 "MOV_RM8_R64"）。
    #[serde(default)]
    pub ret_mov_inst: Option<String>,
    /// 返回寄存器（物理名；如 riscv "X10"=a0）。缺省空 = index 0（x86 RAX
    /// 语义）。Return/Call lowering 的返回值移动目标用此列表首项。
    #[serde(default)]
    pub ret_regs: Vec<String>,
    /// Call 调用指令名（缺省 "CALL_RIP_REL"=x86）。riscv 声明 "JAL"：
    /// 定宽 label 槽 = -(FuncRef+1)（encoder 转 "@N" 符号 reloc），
    /// 其余 Reg 槽填 `call_ret_reg`（返回地址寄存器）。
    #[serde(default)]
    pub call_inst: Option<String>,
    /// CallIndirect 调用指令名（缺省 "CALL_RM"=x86 FF /2）。定宽 ISA 可
    /// 声明 "JALR"（rs1 = 目标地址、imm = 0、out Reg 槽 = call_ret_reg）。
    #[serde(default)]
    pub call_indirect_inst: Option<String>,
    /// Return 指令名（terminator lowering 用；缺省 "RET"）。
    #[serde(default)]
    pub ret_inst: Option<String>,
    /// 无条件跳转指令名（epilogue/block jump；缺省变长 "JMP_REL32"、
    /// 定宽 "JAL"——按存在性回退）。
    #[serde(default)]
    pub jump_inst: Option<String>,
    /// 条件分支指令名（Branch lowering；缺省 "JCC_REL32"/定宽 "BEQ"）。
    #[serde(default)]
    pub branch_inst: Option<String>,
    /// 条件测试指令名（Branch 的 test-cond 序列；缺省 "TEST_RM_R"）。
    #[serde(default)]
    pub test_inst: Option<String>,
    /// 硬件 push/pop 指令名（@push_callee 用；缺省 "PUSH"/"POP"——
    /// 不存在时回退 [spill.GPR] store/load 模板）。
    #[serde(default)]
    pub push_inst: Option<String>,
    /// 硬件 pop 指令名（@pop_callee 用；缺省 "POP"）。
    #[serde(default)]
    pub pop_inst: Option<String>,
    /// 浮点返回/参数移动指令名（f64；缺省 "MOVSD"）。
    /// 生成代码直接构造该指令变体（fpr out, fpr in）。
    #[serde(default)]
    pub fpr_mov_inst: Option<String>,
    /// 浮点返回/参数移动指令名（f32；缺省 "MOVSS"）。
    #[serde(default)]
    pub fpr_mov_inst32: Option<String>,
    /// Call 的返回地址寄存器（缺省 "X1"=riscv ra）。
    #[serde(default)]
    pub call_ret_reg: Option<String>,
    /// Call 点被调用方可能破坏的寄存器（物理名）——regalloc 的 call clobber
    /// 集。缺省 = 整数参数寄存器 + 返回寄存器（x86 语义）。定宽 ISA 无
    /// callee-saved 保存序列（如 riscv 当前 callee_saved=[]）时，callee 会
    /// 破坏全部 caller-saved（临时）寄存器 → 必须把 t0-t6 等也列入，否则
    /// 跨调用存活值留在寄存器被覆盖（实测递归 fib 死循环）。
    #[serde(default)]
    pub call_clobbers: Option<Vec<String>>,
    /// regalloc 不可分配的寄存器（物理名；如 riscv 的 X0=zero 不可写、
    /// X1=ra 返回地址被 prologue/call 占用、X3/X4=gp/tp）。缺省空。
    #[serde(default)]
    pub reserved: Vec<String>,
}

/// 帧布局配置（[abi.frame]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbiFrame {
    /// 栈指针寄存器名（"RSP"）。
    pub sp: String,
    /// 帧指针寄存器名（"RBP"；None = 无帧指针）。
    #[serde(default)]
    pub fp: Option<String>,
    /// 帧分配指令（SUB RSP, imm）——[emit] 的 @frame_alloc 展开用。
    #[serde(default)]
    pub alloc_inst: Option<String>,
    /// 帧释放指令（ADD RSP, imm）——@frame_free 用。
    #[serde(default)]
    pub free_inst: Option<String>,
    /// prologue 在帧指针上方 push 的字节数（帧指针保存槽；x86 = 8）。
    #[serde(default)]
    pub fp_push_bytes: Option<u32>,
    /// @frame_alloc 的立即数取负（riscv `addi sp, sp, -N`：ADDI 是加法指令、
    /// 帧分配需负偏移；x86 用 SUB 语义不需要）。缺省 false。
    #[serde(default)]
    pub alloc_neg: bool,
    /// 帧最小字节数（riscv 的 ra/fp 保存槽需帧 ≥ 固定值；缺省 0）。
    #[serde(default)]
    pub min_frame_bytes: Option<u32>,
    /// callee-saved 区字节数覆盖（缺省 = frame_pointer_overhead +
    /// callee_saved×宽——x86 语义：push 在帧外/fp 上方）。riscv 的
    /// callee_saved 保存槽在**帧内顶部**（@push_callee 的 SD 到
    /// [sp+frame-16-k*8]，min_frame_bytes 覆盖）→ 覆盖为 0：spill 槽
    /// sp_base = -(frame)（帧内底部）、StackAddr 平移 0（栈槽帧内），
    /// 否则 spill 槽落在帧外与递归帧重叠（实测 fib 死循环）。
    #[serde(default)]
    pub callee_saved_bytes_override: Option<u32>,
    /// 栈槽（StackAddr/Alloca）的帧顶平移字节数（riscv = fp_push_bytes=16：
    /// 栈槽基准 fp-16，避开 ra/fp 保存槽且递归各帧独立；缺省 None = 回退
    /// callee_saved_bytes——x86 语义）。独立于 callee_saved_bytes_override
    ///（后者管 spill 布局、前者管栈槽平移）。
    #[serde(default)]
    pub stack_slot_shift: Option<i32>,
}

/// 被调用者保存寄存器（[abi.callee_saved]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalleeSaved {
    #[serde(default)]
    pub gpr: Vec<String>,
    #[serde(default)]
    pub xmm: Vec<String>,
}

/// 类型类别 → 传参寄存器/策略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgClass {
    /// 类别名（"int" | "float" | "vector" | ...）。
    pub class: String,
    #[serde(default)]
    pub regs: Vec<String>,
    /// 传参策略："by-ref"（>limit 位向量按引用，YMM ABI）等。
    #[serde(default)]
    pub strategy: Option<String>,
    /// 策略适用的大小上限（位）。
    #[serde(default)]
    pub limit: Option<u32>,
}

// ───────────────────────── [emit] ─────────────────────────

/// 序言/尾声指令块。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmitSection {
    #[serde(default)]
    pub prologue: Option<EmitBlock>,
    #[serde(default)]
    pub epilogue: Option<EmitBlock>,
    /// `.align N` 伪指令的填充字节（缺省 0x00）。
    #[serde(default)]
    pub align_pad: Option<u8>,
    /// 是否生成独立尾声标签 + return 块的 epilogue 跳转（缺省 true =
    /// x86 语义：return block 经 epilogue_jump 跳到统一尾声）。定宽 ISA
    /// 无 JMP 指令时可设 false：return block 直接 fall-through 到尾声
    /// （仅单 return block 函数安全）。
    #[serde(default)]
    pub epilogue_label: Option<bool>,
    /// 尾声跳转指令名（`emit_epilogue_jump` 用；缺省 None = 自动检测
    /// `JMP_REL32`（变长 x86，0xE9 rel32 手写）/ `JAL`（定宽 riscv，
    /// label 槽 = 块号）。显式声明替代名称检测）。
    #[serde(default)]
    pub epilogue_jump_inst: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmitBlock {
    pub insts: Vec<String>,
}

// ───────────────────────── [spill.*] ─────────────────────────

/// 溢出槽模板：load/store 指令 + 基址寄存器。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpillTemplate {
    /// 加载指令模板（`{0}` = 目标寄存器，`{1}` = 帧偏移——spill 生成时替换）。
    pub load: String,
    /// 存储指令模板（`{0}` = 源寄存器，`{1}` = 帧偏移）。
    pub store: String,
    /// 基址寄存器名（"RBP"）。
    #[serde(default)]
    pub base: Option<String>,
}
