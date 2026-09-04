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

// ──────────────────── [[forms]] / 编码语义键 ────────────────────

/// 编码语义键集合——`[[forms]]`（预设）与 `[[instructions]]`（逐键覆盖）
/// **共用同一组字段**。
///
/// v14 之前 form 是"必须预先命名的组合点"，指令只能覆盖 opsize/rex_w/vex 三个
/// 键，于是每出现一个新组合就得新起一个 form 名——x86 47 个 form 里 17 个只被
/// 一条指令用，命名已到 `MRR_0F_NOOS_MEM` 与 `MRR_MEM_0F_NOOS` 并存（只差
/// `rex_w`）的程度。v15 起 form 退化为**可选混入的预设**，任意键都能在指令上
/// 覆盖，组合不再需要命名。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncKeys {
    /// ModRM 结构映射（v15）：`{ reg = <操作数名 | 固定扩展码>, rm = <操作数名> }`；
    /// `rm` 加方括号（`"[base]"`）= 内存形式（mod≠11），与 asm 里 `[{base}]` 同形。
    ///
    /// 取代 v14 的六个魔法串（`rr`/`rr_rev`/`rr_src2`/`ext`/`rm_mem`/`rm_memref`）
    /// ——它们把"哪个操作数进 reg 字段、哪个进 rm 字段"编进了一个不透明的名字，
    /// 读者必须回查生成器才知道 `rr_src2` 是 reg=op0+rm=op2。现在直接写名字。
    /// 内存形式的两种风味由 `rm` 引用的槽 kind 区分：`mem` 槽 → 带
    /// base/disp/index/scale；`reg` 槽 → 仅 `[base]`（disp 恒 0）。
    #[serde(default)]
    pub modrm: Option<ModrmMap>,
    /// 固定 ModRM 字节（无操作数指令如 MFENCE 0F AE F0：mod=11/reg/rm 全固定）。
    #[serde(default)]
    pub modrm_fixed: Option<u64>,
    /// REX 发射策略（"auto" | "never" | ...）。
    #[serde(default)]
    pub rex: Option<String>,
    /// VEX 结构（map/pp/l 来源）。
    #[serde(default)]
    pub vex: Option<VexSpec>,
    /// EVEX 结构（AVX-512；复用 [`VexSpec`] 数据键 map/pp/w/l，l=0/1/2 →
    /// L'L=128/256/512 位）。
    #[serde(default)]
    pub evex: Option<VexSpec>,
    /// 变长：固定前缀来源。`"field"` → fields.prefix（SSE 的 66/F2/F3/0）；
    /// 数字字符串 → 固定字节。缺省无前缀。
    #[serde(default)]
    pub prefix: Option<String>,
    /// 变长：编码宽度语义（见 [`Opsize`]）。
    #[serde(default)]
    pub opsize: Option<Opsize>,
    /// 变长：REX.W 位来源（枚举——未知值由 serde 报错并列出候选）。
    #[serde(default)]
    pub rex_w: Option<RexW>,
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
}

impl EncKeys {
    /// 逐键覆盖：`self`（指令级）优先，缺省取 `base`（form 预设）。
    pub fn over(&self, base: &EncKeys) -> EncKeys {
        macro_rules! pick {
            ($f:ident) => {
                self.$f.clone().or_else(|| base.$f.clone())
            };
        }
        EncKeys {
            modrm: pick!(modrm),
            modrm_fixed: pick!(modrm_fixed),
            rex: pick!(rex),
            vex: pick!(vex),
            evex: pick!(evex),
            prefix: pick!(prefix),
            opsize: pick!(opsize),
            rex_w: pick!(rex_w),
            opcode_reg: pick!(opcode_reg),
            imm: pick!(imm),
            escape: pick!(escape),
            opcode_field: pick!(opcode_field),
            operand_fields: pick!(operand_fields),
        }
    }
}

/// 编码形式预设（`[[forms]]`）：具名的 [`EncKeys`]，指令用 `form = "名字"` 混入。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Form {
    pub name: String,
    #[serde(flatten)]
    pub keys: EncKeys,
}

/// ModRM 映射：哪个操作数进 `reg` 字段、哪个进 `rm` 字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModrmMap {
    /// `reg` 字段来源：操作数名，或固定扩展码（`/digit` 形式的整数）。
    pub reg: ModrmReg,
    /// `rm` 字段来源：操作数名；`"[名字]"` = 内存形式（mod≠11）。
    pub rm: String,
}

impl ModrmMap {
    /// `rm` 是否内存形式，以及去掉方括号后的操作数名。
    pub fn rm_operand(&self) -> (bool, &str) {
        let t = self.rm.trim();
        match t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            Some(inner) => (true, inner.trim()),
            None => (false, t),
        }
    }
}

/// `modrm.reg` 的两种来源。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModrmReg {
    /// 固定扩展码（x86 的 `/0`..`/7`）——无操作数占用 reg 字段。
    Ext(u64),
    /// 操作数名。
    Op(String),
}

/// REX.W 位来源（x86-64）。
///
/// v14 前是自由字符串 + 手写 validate 分支；改枚举后未知值由 serde 直接报
/// "unknown variant" 并列出候选，且带 TOML 行号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RexW {
    /// opsize == 64 位时置 1（多数算术/传送）。
    Auto,
    /// 取指令 `fields.w`（SSE/VEX 系：W 位是 opcode 的一部分）。
    Field,
    /// 恒置 1（`+r` 形式的 mov_imm64 / bswap）。
    Always,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Opsize {
    Slot(u16),
    Reg(u16),
    /// `opsize = "<操作数名>"`：取该命名操作数的寄存器宽度。
    ///
    /// 自解释形态，取代 `"s<N>"` 的位置引用——`opsize = "dst"` 一眼看出取的是
    /// 目的操作数。`collect_inst_infos` 里按 `ops` 声明序解析成 [`Opsize::Slot`]，
    /// 下游只见索引。
    Named(String),
    /// `opsize = "max"`：宽度 = 全部 Reg 操作数宽度的**最大值**。
    ///
    /// 用于两个源槽都不是"结果"的同宽指令（x86 CMP/TEST：`cmp r/m, r` 两操作数
    /// 必须同宽，没有目的槽可取）。IR 层允许混宽（`TypeId::upcast`：
    /// `icmp(PTR, I32)`），取任一单槽宽度都会按较窄者编码——32 位 CMP 只比低半，
    /// 高半非 0 的指针与 0 判等为真。取宽者 = upcast 结果宽度 = 正确比较宽度。
    ///
    /// 汇编文本路径不受影响：多类 GPR 槽的宽度一致性检查同样对 `max` 生效
    /// （`cmp RAX, EBX` 仍拒绝），只有 IR 降级产生的混宽组合走取宽语义。
    Max,
}

impl Default for Opsize {
    fn default() -> Self {
        Self::Slot(0)
    }
}

impl FromStr for Opsize {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "max" {
            return Ok(Self::Max);
        }
        // "s<N>" / "r<N>" 是位置/字节形态；其余非空字符串 = 命名操作数引用
        let digits = s.get(1..).unwrap_or("");
        let n: u16 = match digits.parse() {
            Ok(n) if !digits.is_empty() => n,
            _ => {
                return if s.is_empty() {
                    Err("opsize must not be empty".to_string())
                } else {
                    Ok(Self::Named(s.to_string()))
                };
            }
        };
        match s.chars().next() {
            Some('s') => Ok(Self::Slot(n)),
            // "r<字节>" 是 v14 及以前的写法（r8 = 8 字节 = 64 位），与
            // `[conventions]` 的位单位互相矛盾。v15 起固定宽度写**裸整数位宽**
            // （`opsize = 64`），这里保留读旧值的能力只为给出明确的迁移提示。
            Some('r') => Err(format!(
                "opsize = \"r{n}\" 是字节单位的旧写法，改写成位宽整数 opsize = {}",
                n as u32 * 8
            )),
            _ => Ok(Self::Named(s.to_string())),
        }
    }
}

impl Opsize {
    /// 裸整数 → 固定宽度（**位**；内部按字节存，与 `PhysReg::width()` 同单位）。
    fn from_bits(bits: u64) -> Result<Self, String> {
        if bits == 0 || bits % 8 != 0 {
            return Err(format!("opsize = {bits} 必须是 8 的正整数倍（位宽）"));
        }
        Ok(Self::Reg((bits / 8) as u16))
    }
}

impl Display for Opsize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Opsize::Slot(idx) => write!(f, "s{}", idx),
            // 序列化回位宽整数形态（往返一致）
            Opsize::Reg(bytes) => write!(f, "{}", *bytes as u32 * 8),
            Opsize::Named(n) => write!(f, "{n}"),
            Opsize::Max => write!(f, "max"),
        }
    }
}

impl Serialize for Opsize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // 固定宽度序列化为整数位宽（与 TOML 写法一致，往返无损）
        match self {
            Opsize::Reg(bytes) => serializer.serialize_u64(*bytes as u64 * 8),
            other => serializer.serialize_str(&other.to_string()),
        }
    }
}

struct OpsizeVisitor;

impl<'de> Visitor<'de> for OpsizeVisitor {
    type Value = Opsize;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            r#"an opsize: bit width integer (16/32/64), "s<N>" (operand slot) or "max""#
        )
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Self::Value::from_str(v).map_err(|e| E::custom(format!("invalid opsize: {}", e)))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Opsize::from_bits(v).map_err(|e| E::custom(format!("invalid opsize: {e}")))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if v < 0 {
            return Err(E::custom("invalid opsize: 位宽不能为负"));
        }
        self.visit_u64(v as u64)
    }
}

impl<'de> Deserialize<'de> for Opsize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(OpsizeVisitor)
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
    /// 编码预设名（`[[forms]]`）。**可省略**——省略时全部编码键由本指令的
    /// [`EncKeys`] 直接给出（组合不需要预先命名一个 form）。
    #[serde(default)]
    pub form: Option<String>,
    /// 主 opcode（值或首个 opcode 字节）。
    #[serde(default)]
    pub opcode: Option<u64>,
    /// 固定字段值（funct3/funct7/前缀字节...），按位域名引用。
    #[serde(default)]
    pub fields: Option<BTreeMap<String, u64>>,
    /// **命名操作数声明**（v15）：每项 `"名字:槽[:角色]"`，**数组序 = 编码序**
    /// （modrm reg/rm、定宽位域绑定都按这个序）。角色缺省 `in`。
    ///
    /// 声明与打印从此分离：`asm` 只用 `{名字}` **引用**操作数，不再内联声明。
    /// v14 的写法把两件事塞在一起——`asm = "add {1:[gprx:inout]}, {0:[gprx:in]}"`
    /// 里索引与打印序解耦，读者无法从 `opsize = "s0"` 看出 s0 是源还是目的
    /// （ADD_RM_R 里恰好是**源**，这正是 v14 那个 32 位截断指针 bug 的根源）。
    /// 命名之后写 `opsize = "dst"`，自解释。
    ///
    /// 省略 `ops` 时回退 v14 的内联声明形态（迁移期并存）。
    #[serde(default)]
    pub ops: Option<Vec<String>>,
    /// 汇编模板（必填，完整格式）。首词 = 助记符（唯一事实来源）。
    /// 有 `ops` 时用 `{名字}` 引用；无 `ops` 时用 `{i:[槽:角色]}` 内联声明。
    pub asm: String,
    /// 结构化谓词（迭代 4 定型：{ and = [...], eq = [...] }）。
    #[serde(default)]
    pub when: Option<toml::Value>,
    /// **指令级编码键覆盖**（逐键压过 `form` 预设，见 [`EncKeys`]）。
    /// 组合不再需要预先命名一个 form——直接在指令上写差异那一两个键。
    #[serde(flatten)]
    pub enc: EncKeys,
    /// 效果标签（缺省空 = 无声明）。驱动 `MachineInst::effects` /
    /// `is_branch` / `is_call` / `is_ret` / `is_move`（TargetMachine 集成）。
    #[serde(default)]
    pub effect: Vec<Effect>,
    /// 语义角色（见 [`Role`]）——生成器按角色查指令，取代 `[abi]` 的 13 个
    /// `*_inst` 名指针与 v14 的 `tags` 字符串标签。每个角色全 ISA 唯一。
    #[serde(default)]
    pub roles: Vec<Role>,
    /// 隐式破坏的物理寄存器名（如 cqo 的 RDX、idiv 的 RAX/RDX）——regalloc
    /// 在本指令点避开（MachineInst::clobbers）。与 lowering 模板的显式物理
    /// 寄存器（collect_phys_clobbers）互补：这是指令自身的隐式写。
    #[serde(default)]
    pub implicit_regs: Option<Vec<String>>,
    /// 全局地址重定位语义（GlobalAddr lowering 专用指令）——生成器按此字段
    /// 生成 encoder reloc arm，替代按指令名特判。
    #[serde(default)]
    pub global_reloc: Option<GlobalReloc>,
}

/// 指令的**语义角色**（v15-S4）。
///
/// 生成器需要"某个语义位置上的指令"时按角色查表。v14 是反过来的：`[abi]` 里
/// 13 个 `*_inst` 键存指令**名字**，生成器 `unwrap_or_else(|| "MOV_RM8_R64")`
/// 兜底——共 16 处 x86 指令名硬编码，于是 x86 靠默认"恰好能跑"、非 x86 必须逐个
/// 覆盖。角色化之后：指令自己声明担任什么角色，缺角色 → 明确的 `Unsupported`，
/// 不会静默去查一个别的 ISA 的名字。`tags`（宽向量 by-ref 那几个）一并并入。
///
/// 每个角色全 ISA 唯一（validate 强制）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// 整数寄存器移动（收参 / Copy / 溢出前搬运）。
    GprMov,
    /// 返回值 → 返回寄存器的移动。
    RetMov,
    /// f64 标量寄存器移动。
    FprMovF64,
    /// f32 标量寄存器移动。
    FprMovF32,
    /// ≤16B 向量按值的**全宽**寄存器移动（x86 MOVAPS；缺则向量 by-value 不支持）。
    VecMov,
    /// 直接调用（函数符号 reloc）。
    Call,
    /// 间接调用（寄存器/内存目标）。
    CallIndirect,
    /// 返回。
    Ret,
    /// 无条件跳转。
    Jump,
    /// 条件分支。
    Branch,
    /// 条件测试（Branch 的 test-cond 序列）。
    Test,
    /// 硬件 push（callee-saved 保存；缺则回退 `[spill.*]` store 模板）。
    Push,
    /// 硬件 pop。
    Pop,
    /// 帧分配（`@frame_alloc`）。
    FrameAlloc,
    /// 帧释放（`@frame_free`）。
    FrameFree,
    /// 尾声跳转（缺省用 `jump`；需要不同指令时单独声明）。
    EpilogueJump,
    /// 宽向量 by-ref：调用方栈拷贝 store（32 字节）。
    #[serde(rename = "wide_vec_store_32")]
    WideVecStore32,
    /// 宽向量 by-ref：调用方栈拷贝 store（64 字节）。
    #[serde(rename = "wide_vec_store_64")]
    WideVecStore64,
    /// 宽向量 by-ref/sret：收参与回读 load（32 字节）。
    #[serde(rename = "wide_vec_load_32")]
    WideVecLoad32,
    /// 宽向量 by-ref/sret：收参与回读 load（64 字节）。
    #[serde(rename = "wide_vec_load_64")]
    WideVecLoad64,
    /// 帧内 `[FP+disp]` 地址计算（sret / by-ref 临时槽）。
    FrameAddr,
    /// 栈参数收参 load（第 5+ 个参数从 `[FP+shadow+…]` 取）。
    StackArgLoad,
    /// 栈参数传参 store（调用方把第 5+ 个参数写到 `[SP+shadow+…]`）。
    StackArgStore,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 与 serde 的 snake_case 命名一致（错误消息里直接给 TOML 写法）
        let s = serde_json_name(self);
        f.write_str(s)
    }
}

/// 角色的 TOML 写法（snake_case）。
fn serde_json_name(r: &Role) -> &'static str {
    match r {
        Role::GprMov => "gpr_mov",
        Role::RetMov => "ret_mov",
        Role::FprMovF64 => "fpr_mov_f64",
        Role::FprMovF32 => "fpr_mov_f32",
        Role::VecMov => "vec_mov",
        Role::Call => "call",
        Role::CallIndirect => "call_indirect",
        Role::Ret => "ret",
        Role::Jump => "jump",
        Role::Branch => "branch",
        Role::Test => "test",
        Role::Push => "push",
        Role::Pop => "pop",
        Role::FrameAlloc => "frame_alloc",
        Role::FrameFree => "frame_free",
        Role::EpilogueJump => "epilogue_jump",
        Role::WideVecStore32 => "wide_vec_store_32",
        Role::WideVecStore64 => "wide_vec_store_64",
        Role::WideVecLoad32 => "wide_vec_load_32",
        Role::WideVecLoad64 => "wide_vec_load_64",
        Role::FrameAddr => "frame_addr",
        Role::StackArgLoad => "stack_arg_load",
        Role::StackArgStore => "stack_arg_store",
    }
}

/// 指令效果标签。
///
/// v14 前是自由字符串，生成器 `match e.as_str()` 的兜底分支把打错的标签静默
/// 变成 `EffectKind::Custom(0)`（既不报错也不生效）。改枚举后未知标签由 serde
/// 直接拒绝并列出候选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    /// 无副作用纯运算。
    Pure,
    /// 读内存。
    Read,
    /// 写内存。
    Write,
    /// 条件分支（+ Label 槽 → branch_targets）。
    Branch,
    /// 无条件跳转（+ Label 槽 → branch_targets）。
    Jump,
    /// 调用。
    Call,
    /// 返回。
    Ret,
    /// 陷入（ud2/ebreak——`Trap` lowering 取无操作数的那条）。
    Trap,
    /// 纯寄存器移动（regalloc 的 coalesce 依据；效果语义等同 `Pure`）。
    Move,
}

/// 全局地址重定位语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalReloc {    /// imm 槽 < 0 时编码 GlobalId → ABS8 `"G{id}"`（x86 MOVABS_GLOBAL）。
    Abs8,
    /// PC-relative hi20（riscv AUIPC_GLOBAL；patcher 按 opcode 分写位段）。
    PcrelHi,
    /// PC-relative lo12（riscv ADDI_GLOBAL）。
    PcrelLo,
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
    /// 家族共享命名操作数声明（同 [`Instruction::ops`]）。
    #[serde(default)]
    pub ops: Option<Vec<String>>,
    /// 家族共享编码键覆盖（同 [`Instruction::enc`]）——`modrm` 映射引用 `ops`
    /// 的名字，family 内全部变体共用同一份声明，故可放在家族层。
    #[serde(flatten)]
    pub enc: EncKeys,
    /// 家族共享 asm 模板（完整格式；`{name}` 占位符替换为变体名小写 =
    /// 变体助记符）。
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
    /// 变体级语义角色（见 [`Role`]）——家族里只有个别变体担任 ABI 角色
    /// （如 SSE 家族里 MOVSD 是 f64 移动、MOVAPS 是向量全宽移动）。
    #[serde(default)]
    pub roles: Vec<Role>,
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
    /// 参数化行表：各列表**等长**，按下标 zip 成行展开成多条具体规则。
    ///
    /// 键分两类：
    /// - 名字在 [`crate::v12::pred::PRED_ATTRS`] 里 → 该行自动追加
    ///   `eq = [键, 值]` 到 `when`，**且**可在模板里用 `{键}` 引用；
    /// - 其余 → 纯替换变量（只在模板里用 `{键}`）。
    ///
    /// 消除"一个 op 一堆只差助记符的规则"：x86 `Vadd` 8 条（4 elem × 2 宽度）
    /// → 2 条，`Fcmp` 32 条（16 cond × 2 elem）→ 4 条。
    #[serde(default)]
    pub vary: Option<BTreeMap<String, Vec<VaryValue>>>,
    /// 显式优先级（缺省 0，大者先试）。规则**排序不依赖声明序**：
    /// 按 (priority 降, 谓词叶子数降, 声明序升) 裁决。
    ///
    /// 只在"故意让更宽的规则赢过更具体的规则"时才需要——例如 x86 `Vextract`
    /// 的 lane 0 快路径（`imm0 == 0` 两个约束）必须压过 V256 通路
    /// （`rd/elem/imm0` 三个约束）。其余场合留空，让特异性自动裁决。
    #[serde(default)]
    pub priority: Option<i32>,
}

/// `vary` 的取值：整数（可作谓词值）或字符串（只作模板替换）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VaryValue {
    Int(i64),
    Str(String),
}

impl VaryValue {
    /// 模板替换用的文本。
    pub fn as_text(&self) -> String {
        match self {
            VaryValue::Int(v) => v.to_string(),
            VaryValue::Str(s) => s.clone(),
        }
    }
    /// 谓词值（仅整数可用）。
    pub fn as_int(&self) -> Option<i64> {
        match self {
            VaryValue::Int(v) => Some(*v),
            VaryValue::Str(_) => None,
        }
    }
}

impl V12Model {
    /// 按 op 分组的 lowering 规则，**每组内按裁决序排好**：
    /// `(priority 降, 谓词叶子数降, 声明序升)`。
    ///
    /// 排序取代"声明序首匹配"：作者不再需要记住"兜底规则必须写在最后"，
    /// 也不会因为插入一条规则的位置不对而静默改变分派。校验器与生成器读同一
    /// 个顺序（唯一事实源在此），故编译期报出的死规则就是运行期真的死规则。
    /// op 之间保持首次出现的声明序（生成代码的 match arm 顺序稳定）。
    pub fn lowering_by_op(&self) -> Vec<(&str, Vec<&Lowering>)> {
        let mut by_op: Vec<(&str, Vec<(usize, &Lowering)>)> = Vec::new();
        for (i, rule) in self.lowering.iter().enumerate() {
            match by_op.iter_mut().find(|(op, _)| *op == rule.op.as_str()) {
                Some((_, rules)) => rules.push((i, rule)),
                None => by_op.push((rule.op.as_str(), vec![(i, rule)])),
            }
        }
        by_op
            .into_iter()
            .map(|(op, mut rules)| {
                rules.sort_by_key(|(i, r)| {
                    let leaves = r
                        .when
                        .as_ref()
                        .and_then(|v| crate::v12::pred::parse(v).ok())
                        .map(|p| crate::v12::pred::leaf_count(&p))
                        .unwrap_or(0);
                    (
                        -r.priority.unwrap_or(0) as i64,
                        -(leaves as i64),
                        *i as i64,
                    )
                });
                (op, rules.into_iter().map(|(_, r)| r).collect())
            })
            .collect()
    }
}

impl Lowering {
    /// 展开 `vary` 行表 → 多条具体规则（无 `vary` 时返回自身单元素）。
    ///
    /// 每行：模板里 `{键}` 换成该行取值；键若是谓词属性（`PRED_ATTRS`）则
    /// 额外把 `eq = [键, 值]` 合入 `when`（与原 `when` 取 `and`）。
    pub fn expand_vary(&self) -> Result<Vec<Lowering>, String> {
        let Some(vary) = &self.vary else {
            return Ok(vec![self.clone()]);
        };
        let path = format!("[[lowering.{}]].vary", self.op);
        if vary.is_empty() {
            return Err(format!("{path}: 不能为空表"));
        }
        let rows = vary.values().next().map(Vec::len).unwrap_or(0);
        if rows == 0 {
            return Err(format!("{path}: 列表不能为空"));
        }
        for (k, v) in vary {
            if v.len() != rows {
                return Err(format!(
                    "{path}: 各列表必须等长（按下标 zip 成行）——'{k}' 长 {} ≠ {rows}",
                    v.len()
                ));
            }
        }
        let mut out = Vec::with_capacity(rows);
        for row in 0..rows {
            let mut insts = self.insts.clone();
            let mut extra: Vec<toml::Value> = Vec::new();
            for (k, vals) in vary {
                let val = &vals[row];
                let needle = format!("{{{k}}}");
                let text = val.as_text();
                for line in insts.iter_mut() {
                    if line.contains(&needle) {
                        *line = line.replace(&needle, &text);
                    }
                }
                if crate::v12::pred::PRED_ATTRS.contains(&k.as_str()) {
                    let iv = val.as_int().ok_or_else(|| {
                        format!("{path}: '{k}' 是谓词属性，取值必须是整数，got {val:?}")
                    })?;
                    extra.push(toml::Value::Table(
                        [(
                            "eq".to_string(),
                            toml::Value::Array(vec![
                                toml::Value::String(k.clone()),
                                toml::Value::Integer(iv),
                            ]),
                        )]
                        .into_iter()
                        .collect(),
                    ));
                }
            }
            let when = match (&self.when, extra.len()) {
                (base, 0) => base.clone(),
                (None, 1) => Some(extra.remove(0)),
                (base, _) => {
                    let mut all = Vec::with_capacity(extra.len() + 1);
                    if let Some(b) = base {
                        all.push(b.clone());
                    }
                    all.append(&mut extra);
                    Some(toml::Value::Table(
                        [("and".to_string(), toml::Value::Array(all))]
                            .into_iter()
                            .collect(),
                    ))
                }
            };
            out.push(Lowering {
                op: self.op.clone(),
                insts,
                when,
                vary: None,
                priority: self.priority,
            });
        }
        Ok(out)
    }
}

// ───────────────────────── [abi] ─────────────────────────

/// 调用约定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Abi {
    #[serde(default)]
    pub stack_align: Option<u32>,
    /// 帧布局的额外栈填充（字节）：x86 = 8（align/2，SysV/Windows x64
    /// ABI：prologue push rbp + callee-saved 后 rsp%16==8，sub rsp 需使
    /// call 前 rsp%16==0）。缺省 0。
    #[serde(default)]
    pub frame_padding: Option<i32>,
    /// 调用方 call 前预留的 shadow space 字节数（Windows x64 = 0x20）。
    /// Some(n) 启用栈参数：第 5+ 个参数（寄存器耗尽后）由调用方 store 到
    /// [rsp+n+(k-nregs)*8]、被调方从 [rbp+n+8+(k-nregs)*8] load。
    /// None = 不支持栈参数（超寄存器参数 → Unsupported）。riscv 缺省 None
    ///（8 个 GPR + 8 个 FPR 足够，SysV 无 shadow space）。
    #[serde(default)]
    pub stack_arg_shadow: Option<u32>,
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
    /// 返回寄存器（物理名；如 riscv "X10"=a0）。缺省空 = index 0（x86 RAX
    /// 语义）。Return/Call lowering 的返回值移动目标用此列表首项。
    #[serde(default)]
    pub ret_regs: Vec<String>,
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
    /// 参数槽位分配规则（语义显式声明，见 [`ArgSlot`]）。
    #[serde(default)]
    pub arg_slot: Option<ArgSlot>,
}

/// 参数槽位分配规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArgSlot {
    /// 缺省（riscv SysV）：int/float 各自独立推进——int 序列 RCX/RDX/… 与
    /// float 序列 XMM0/… 分开计数。
    #[default]
    ByClass,
    /// Windows x64：int/float 共享位置计数——参数 i 用 GPR{i}/XMM{i}
    /// （第 2 参数即使第 1 个是整数也用 XMM1）。
    ByPosition,
}

/// 帧布局模式（[abi.frame].layout）：决定 callee-saved 保存槽相对帧的位置。
/// 其余帧数值（min_frame_bytes / callee_saved_bytes / stack_slot_shift）全部
/// 由运行期从本模式 + fp_push_bytes + callee_saved 表**推导**（pipeline/
/// frame_layout.rs::frame_layout_info），不再在 TOML 里手工写魔法数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayoutMode {
    /// fp-outside（缺省，x86/demo）：callee-saved 用硬件 push 在帧指针**上方**
    /// （帧外）。spill 槽 sp_base = -(frame) - callee_saved_bytes、栈槽基准
    /// fp - callee_saved_bytes。
    #[serde(rename = "fp-outside")]
    FpOutside,
    /// fp-inside（riscv）：ra/fp/callee-saved 保存槽在帧**内顶部**
    /// （@push_callee 的 SD 到 [sp+frame-fp_push-(k+1)*8]，帧分配覆盖到固定
    /// 最小帧）。spill 槽 sp_base = -(frame)（帧内底部）、栈槽平移 = fp_push。
    #[serde(rename = "fp-inside")]
    FpInside,
}

impl Default for LayoutMode {
    fn default() -> Self {
        LayoutMode::FpOutside
    }
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
    /// 帧布局模式（见 [`LayoutMode`]；缺省 fp-outside）。
    #[serde(default)]
    pub layout: LayoutMode,
    /// prologue 在帧指针上方 push 的字节数（帧指针保存槽；x86 = 8）。
    #[serde(default)]
    pub fp_push_bytes: Option<u32>,
    /// @frame_alloc 的立即数取负（riscv `addi sp, sp, -N`：ADDI 是加法指令、
    /// 帧分配需负偏移；x86 用 SUB 语义不需要）。缺省 false。
    #[serde(default)]
    pub alloc_neg: bool,
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

/// 传参类别：arg_class 的类型语义（决定传参寄存器族与策略）。
/// serde 用小写字符串（"int"/"float"/"vector"/...），未知类别 → 解析失败
///（deny_unknown 语义提前到反序列化层，validate 不再做字符串自由检查）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArgClassKind {
    /// 整数/指针参数（GPR 族）。
    Int,
    /// 浮点标量参数（FPR 族）。
    Float,
    /// 向量参数（VEC 族；可配 by-ref 策略）。
    Vector,
    /// 其他自定义类别（KReg/掩码等）——生成器按通用寄存器槽处理。
    #[serde(rename = "other")]
    Other,
}

impl ArgClassKind {
    /// 人类可读名（错误消息用）。
    pub fn name(self) -> &'static str {
        match self {
            ArgClassKind::Int => "int",
            ArgClassKind::Float => "float",
            ArgClassKind::Vector => "vector",
            ArgClassKind::Other => "other",
        }
    }
}

/// 类型类别 → 传参寄存器/策略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgClass {
    /// 传参类别（int/float/vector/other——枚举，语义显式）。
    pub class: ArgClassKind,
    #[serde(default)]
    pub regs: Vec<String>,
    /// 传参策略（见 [`ArgStrategy`]）。
    #[serde(default)]
    pub strategy: Option<ArgStrategy>,
    /// 策略适用的大小上限（位）。
    #[serde(default)]
    pub limit: Option<u32>,
}

/// 传参策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArgStrategy {
    /// 超过 `limit` 位的值按引用传（调用方栈拷贝 + 传指针；YMM/ZMM ABI）。
    ByRef,
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
