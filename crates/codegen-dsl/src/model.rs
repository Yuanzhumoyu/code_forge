//! ISA DSL v10 model types — serde-driven, matches TOML hierarchy directly.

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct IsaModel {
    pub meta: Meta,
    pub reg: HashMap<String, RegGroup>,
    #[serde(default)]
    pub abi: Option<Abi>,
    #[serde(default)]
    pub inst: HashMap<String, Instruction>,
    #[serde(default)]
    pub lower: HashMap<String, LowerRule>,
    #[serde(default, rename = "lower_term")]
    pub lower_term: HashMap<String, LowerRule>,
    #[serde(default, rename = "lower_pattern")]
    pub lower_pattern: HashMap<String, LowerRule>,
    #[serde(default)]
    pub emit: Option<EmitSection>,
    /// Named encoding macros — callable as `$name arg1 arg2` in encoding strings.
    #[serde(default, rename = "enc_macros")]
    pub enc_macros: HashMap<String, EncMacro>,
    /// Bitfield scatter patterns — referenced as `{field:pattern_name}`.
    #[serde(default, rename = "enc_scatters")]
    pub enc_scatters: HashMap<String, EncMacro>,
    /// Condition code → assembly name mapping (e.g. "0x94" → "e").
    #[serde(default)]
    pub cc_names: HashMap<String, String>,

    // ── v11: lang-frontend grammar configuration ──

    /// Language token definitions (`[lang.tokens.*]`).
    #[serde(default, rename = "lang_tokens")]
    pub lang_tokens: Option<HashMap<String, LangTokenDef>>,

    /// Language keyword definitions (`[lang.keywords]`).
    #[serde(default, rename = "lang_keywords")]
    pub lang_keywords: Option<HashMap<String, String>>,

    /// Language grammar rules (`[lang.rules]`).
    #[serde(default, rename = "lang_rules")]
    pub lang_rules: Option<HashMap<String, String>>,

    /// Dynamic type dimensions (`[dyn.*]`) — e.g. opsize with values [8,16,32,64].
    #[serde(default, rename = "dyn")]
    pub dyn_types: HashMap<String, DynType>,
}

/// TOML definition of a user-defined token.
#[derive(Debug, Clone, Deserialize)]
pub struct LangTokenDef {
    pub pattern: String,
}

impl IsaModel {
    pub fn validate(&self) -> Result<(), String> {
        if !self.reg.contains_key("gpr") {
            return Err("missing [reg.gpr] section".into());
        }
        // Validate dyn_types: check kind is supported, default ∈ values.
        const SUPPORTED_KINDS: &[&str] = &["u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64"];
        for (name, dt) in &self.dyn_types {
            if !SUPPORTED_KINDS.contains(&dt.kind.as_str()) {
                return Err(format!(
                    "[dyn.{name}]: unsupported kind '{}'. Supported: {:?}",
                    dt.kind, SUPPORTED_KINDS
                ));
            }
            if let Some(d) = dt.default {
                if !dt.values.contains(&d) {
                    return Err(format!(
                        "[dyn.{name}]: default value {d} not found in values {:?}",
                        dt.values
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Meta {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default = "default_endian")]
    pub endian: String,
    #[serde(default = "default_mode")]
    pub mode: u8,
    #[serde(default)]
    pub max_inst_len: u8,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub no_default_lowering: bool,
}

fn default_endian() -> String { "little".into() }
fn default_mode() -> u8 { 64 }

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Capabilities {
    #[serde(default)] pub variable_length: bool,
    #[serde(default)] pub prefix_layers: u8,
    #[serde(default)] pub simd_widths: Vec<u16>,
    #[serde(default)] pub mask_registers: bool,
    #[serde(default)] pub broadcast: bool,
    #[serde(default)] pub rounding_mode: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegGroup {
    pub count: u16,
    #[serde(default = "default_width")]
    pub width: u16,
    #[serde(default)]
    pub names: Option<Vec<String>>,
    #[serde(default)]
    pub prefix: Option<String>,
}

fn default_width() -> u16 { 64 }

#[derive(Debug, Clone, Deserialize)]
pub struct Abi {
    #[serde(default = "default_stack_align")]
    pub stack_align: u32,
    #[serde(default)] pub red_zone: Option<u32>,
    #[serde(default)] pub frame: Option<AbiFrame>,
    #[serde(default)] pub callee_saved: RegList,
    #[serde(default)] pub arg_regs: RegList,
    #[serde(default)] pub ret_regs: RegList,
    #[serde(default)] pub precolor: HashMap<String, String>,
    /// Named aliases for frequently-used virtual registers, e.g. `TMP0 = 96`.
    #[serde(default)]
    pub scratch: HashMap<String, u32>,
}

fn default_stack_align() -> u32 { 16 }

#[derive(Debug, Clone, Deserialize)]
pub struct AbiFrame {
    pub sp: String,
    #[serde(default)] pub fp: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RegList {
    #[serde(default)] pub gpr: Vec<String>,
    #[serde(default)] pub xmm: Vec<String>,
}

// ============================================================
// [inst.<NAME>]
// ============================================================

fn deser_effects<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum EffectValue {
        Array(Vec<String>),
        String(String),
        Section { effect: Vec<String> },
    }
    let v = Option::<EffectValue>::deserialize(deserializer)?;
    Ok(v.map(|ev| match ev {
        EffectValue::Array(arr) => arr,
        EffectValue::String(s) => s.split('+').map(|p| p.trim().to_string()).collect(),
        EffectValue::Section { effect } => effect,
    }))
}

fn deser_fields<'de, D>(deserializer: D) -> Result<Vec<InstField>, D::Error>
where D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum FieldFormat {
        Array(Vec<InstField>),
        Inline(std::collections::BTreeMap<String, String>),
    }
    match FieldFormat::deserialize(deserializer)? {
        FieldFormat::Array(fields) => Ok(fields),
        FieldFormat::Inline(map) => Ok(map.into_iter().map(|(name, ty)| InstField {
            name, field_type: FieldType::from_str(&ty),
        }).collect()),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Instruction {
    #[serde(default, deserialize_with = "deser_fields")]
    pub fields: Vec<InstField>,
    #[serde(default)] pub encoding: Option<String>,
    /// Assembly format template, e.g. `"add {dest}, {src}"`.
    /// `{field}` placeholders reference field names declared in `fields`.
    pub asm: String,
    #[serde(default, deserialize_with = "deser_effects")]
    pub effect: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstField {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub enum FieldType { VReg, I64, U8, U32, F64, BlockTarget, Reg, CondCode, Opsize }

impl FieldType {
    pub fn from_str(s: &str) -> Self {
        match s {
            "VReg" => Self::VReg, "I64" => Self::I64, "U8" => Self::U8,
            "U32" => Self::U32, "F64" => Self::F64, "BlockTarget" => Self::BlockTarget,
            "Reg" => Self::Reg, "CondCode" => Self::CondCode, "Opsize" => Self::Opsize,
            _ => panic!("unknown field type '{}'. Valid types: VReg, I64, U8, U32, F64, BlockTarget, Reg, CondCode, Opsize", s),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LowerRule {
    /// Lowering instruction sequence as assembly strings.
    /// Each string is of the form `"MNEMONIC op1, op2, ..."`.
    #[serde(default)]
    pub insts: Vec<String>,
    /// Template for Icmp/Fcmp lowering — `$CC` placeholder will be replaced.
    #[serde(default)]
    pub template: Vec<String>,
    /// Condition code mapping: cond_name → hex/quoted value (replaces `$CC` in template).
    #[serde(default)]
    pub conditions: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct EmitSection {
    #[serde(default)] pub prologue: Option<EmitBlock>,
    #[serde(default)] pub epilogue: Option<EmitBlock>,
}

/// Prologue/epilogue definition — assembly instruction sequence.
/// Each string is an asm instruction: `"MNEMONIC op1, op2"` or a pseudo-instruction: `"@directive"`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct EmitBlock {
    #[serde(default)]
    pub insts: Vec<String>,
}

/// User-defined encoding macro. Can be used as:
/// - Encoding macro: `$name arg1 arg2` — params bound positionally, `${param}` replaced in pattern
/// - Scatter pattern: `{field:name}` — field name bound to `_` placeholder in pattern
#[derive(Debug, Clone, Deserialize)]
pub struct EncMacro {
    /// Parameter names (positional). For scatters, typically empty (the field name is `_`).
    #[serde(default)]
    pub params: Vec<String>,
    /// The pattern string with `${param}` placeholders (or `_` for scatter field).
    pub pattern: String,
}

/// Dynamic type dimension — e.g. `[dyn.opsize]` with values [8, 16, 32, 64].
#[derive(Debug, Clone, Deserialize)]
pub struct DynType {
    pub kind: String,
    pub values: Vec<u8>,
    #[serde(default)]
    pub default: Option<u8>,
}
