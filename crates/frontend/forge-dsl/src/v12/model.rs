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

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// v12 顶层模型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V12Model {
    pub meta: Meta,
    /// 寄存器组（`[reg.NAME]`）。
    pub reg: BTreeMap<String, RegGroup>,
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

/// 寄存器组。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegGroup {
    /// 寄存器宽度（位）。
    pub width: u32,
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
    /// 持有 ModRM.reg（3 位）的位域名。
    pub reg_field: String,
    /// 持有 ModRM.rm（3 位，+REX.X 扩展第 4 位）的位域名。
    pub rm_field: String,
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
    /// reg：所属 [reg.*] 组名。
    #[serde(default)]
    pub class: Option<String>,
    /// reg：编码宽度（位）。
    #[serde(default)]
    pub field_width: Option<u32>,
    /// imm：值宽度（位）。
    #[serde(default)]
    pub width: Option<u32>,
    /// imm：有符号（缺省 false）。
    #[serde(default)]
    pub signed: Option<bool>,
    /// 该槽可承担的角色；缺省 ["in"]。"inout" = 读改写（in 且 out）。
    #[serde(default)]
    pub roles: Option<Vec<OperandRole>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperandKind {
    Reg,
    Imm,
    Mem,
    Label,
    Cond,
    Opsize,
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
    /// REX 发射策略（"auto" | "never" | ...）。
    #[serde(default)]
    pub rex: Option<String>,
    /// VEX 结构（map/pp/l 来源；迭代 4）。
    #[serde(default)]
    pub vex: Option<VexSpec>,
    /// 变长：固定前缀来源。`"field"` → fields.prefix（SSE 的 66/F2/F3/0）；
    /// 数字字符串 → 固定字节。缺省无前缀。
    #[serde(default)]
    pub prefix: Option<String>,
    /// 变长：编码宽度语义。`"auto"` → 指令有 opsize 操作数（16 → 0x66 前缀、
    /// 64 → REX.W=1）；数字 → 固定 opsize（无 opsize 操作数）。
    #[serde(default)]
    pub opsize: Option<OpsizeSpec>,
    /// 变长：REX.W 位来源。`"auto"` → opsize==64；`"field"` → fields.w。
    /// 缺省恒 0（REX.W 仅在 reg/rm≥8 时随 REX 出现）。
    #[serde(default)]
    pub rex_w: Option<String>,
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

/// VEX 字段来源声明（迭代 4）。
///
/// map/pp/w/l 每个值为：数字（固定）或 `"field"`（取指令 `fields.vex_map`/
/// `vex_pp`/`vex_w`/`vex_l`，缺省 0）。vvvv 语义由操作数数量决定：
/// 3 操作数（VEX_RRV 类）→ vvvv = ~op2；2 操作数 → vvvv = 0x0F（无源）。
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
    /// VEX.L 来源。
    #[serde(default)]
    pub l: Option<String>,
}

/// 变长 form 的 opsize 语义：`"auto"`（指令有 opsize 操作数）或固定值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OpsizeSpec {
    /// `"auto"`：指令有 opsize 操作数。
    Auto(String),
    /// 固定 opsize（8/16/32/64）。
    Fixed(u64),
}

// ──────────────────── [[instructions]] ────────────────────

/// 单条指令。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    pub name: String,
    pub form: String,
    /// 主 opcode（值或首个 opcode 字节）。
    #[serde(default)]
    pub opcode: Option<u64>,
    /// 固定字段值（funct3/funct7/前缀字节...），按位域名引用。
    #[serde(default)]
    pub fields: Option<BTreeMap<String, u64>>,
    #[serde(default)]
    pub operands: Vec<OperandUse>,
    /// 汇编助记符（缺省 = 指令名小写）。
    #[serde(default)]
    pub mnemonic: Option<String>,
    /// 汇编模板：`"{mnemonic} {0}, {1}"`（严格 TOML 字符串，占位符语法见文档）。
    #[serde(default)]
    pub asm: Option<String>,
    /// 结构化谓词（迭代 4 定型：{ and = [...], eq = [...] }）。
    #[serde(default)]
    pub when: Option<toml::Value>,
    /// 指令级 VEX 字段覆盖。
    #[serde(default)]
    pub vex: Option<VexSpec>,
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
    /// 家族共享 asm 模板（完整格式；`{mnemonic}` 占位符替换为变体 mnemonic）。
    #[serde(default)]
    pub asm: Option<String>,
    #[serde(default)]
    pub operands: Vec<OperandUse>,
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
    #[serde(default)]
    pub mnemonic: Option<String>,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmitBlock {
    pub insts: Vec<String>,
}
