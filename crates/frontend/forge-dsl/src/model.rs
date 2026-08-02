//! ISA DSL v10 model types — serde-driven, matches TOML hierarchy directly.

use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
pub struct IsaModel {
    pub meta: Meta,
    pub reg: BTreeMap<String, RegGroup>,
    #[serde(default)]
    pub abi: Option<Abi>,
    #[serde(default)]
    pub inst: BTreeMap<String, Instruction>,
    #[serde(default)]
    pub lower: BTreeMap<String, LowerRule>,
    #[serde(default, rename = "lower_term")]
    pub lower_term: BTreeMap<String, LowerRule>,
    #[serde(default, rename = "lower_pattern")]
    pub lower_pattern: BTreeMap<String, LowerRule>,
    #[serde(default)]
    pub emit: Option<EmitSection>,
    /// Named encoding macros — callable as `$name arg1 arg2` in encoding strings.
    #[serde(default, rename = "enc_macros")]
    pub enc_macros: BTreeMap<String, EncMacro>,
    /// Bitfield scatter patterns — referenced as `{field:pattern_name}`.
    #[serde(default, rename = "enc_scatters")]
    pub enc_scatters: BTreeMap<String, EncMacro>,
    /// Condition code → assembly name mapping (e.g. "0x94" → "e").
    #[serde(default)]
    pub cc_names: BTreeMap<String, String>,

    /// Encoding constants — named values for use in pattern strings.
    /// Defined as `[enc_constants]` in TOML, referenced as `$NAME` in patterns.
    #[serde(default, rename = "enc_constants")]
    pub enc_constants: BTreeMap<String, String>,

    // ── v11: forge-grammar grammar configuration ──
    /// Language token definitions (`[lang.tokens.*]`).
    #[serde(default, rename = "lang_tokens")]
    pub lang_tokens: Option<BTreeMap<String, LangTokenDef>>,

    /// Language keyword definitions (`[lang.keywords]`).
    #[serde(default, rename = "lang_keywords")]
    pub lang_keywords: Option<BTreeMap<String, String>>,

    /// Language grammar rules (`[lang.rules]`).
    #[serde(default, rename = "lang_rules")]
    pub lang_rules: Option<BTreeMap<String, String>>,

    /// Dynamic type dimensions (`[dyn.*]`) — e.g. opsize with values [8,16,32,64].
    #[serde(default, rename = "dyn")]
    pub dyn_types: BTreeMap<String, DynType>,

    /// Register class definitions (`[reg_classes.*]`) — width-aware allocation classes.
    #[serde(default, rename = "reg_classes")]
    pub reg_classes: BTreeMap<String, RegClassDef>,

    /// Spill instruction templates (`[spill.*]`) — width-aware load/store for stack spills.
    #[serde(default)]
    pub spill: BTreeMap<String, SpillTemplate>,
}

/// Register class definition (`[reg_classes.NAME]`).
#[derive(Debug, Clone, Deserialize)]
pub struct RegClassDef {
    /// Register width in bytes.
    pub width: u8,
    /// Allocatable physical register indices.
    pub allocatable: Vec<u8>,
}

/// Spill instruction template (`[spill.NAME]`).
#[derive(Debug, Clone, Deserialize)]
pub struct SpillTemplate {
    /// Load instruction template (stack → register).
    pub load: SpillInstTemplate,
    /// Store instruction template (register → stack).
    pub store: SpillInstTemplate,
}

/// Single spill instruction (load or store).
#[derive(Debug, Clone, Deserialize)]
pub struct SpillInstTemplate {
    /// Instruction name (e.g. "MOV64_RM", "LD").
    pub inst: String,
    /// Base register (e.g. "RBP", "X8").
    pub base: String,
}

/// TOML definition of a user-defined token.
#[derive(Debug, Clone, Deserialize)]
pub struct LangTokenDef {
    pub pattern: String,
}

impl IsaModel {
    pub fn validate(&self) -> Result<(), String> {
        if !self.reg.contains_key("gpr64") && !self.reg.contains_key("gpr") {
            return Err("missing [reg.gpr64] or [reg.gpr] section".into());
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
            if let Some(d) = dt.default
                && !dt.values.contains(&d)
            {
                return Err(format!(
                    "[dyn.{name}]: default value {d} not found in values {:?}",
                    dt.values
                ));
            }
        }
        // Validate reg bindings: Ireg/GprReg require a GPR section, Freg/XmmReg require [reg.xmm].
        for (inst_name, inst) in &self.inst {
            for field in &inst.fields {
                match &field.field_type {
                    FieldType::Ireg | FieldType::GprReg
                        if !self.reg.contains_key("gpr64") && !self.reg.contains_key("gpr") =>
                    {
                        return Err(format!(
                            "[inst.{inst_name}].{}: type '{}' requires [reg.gpr64] or [reg.gpr] section",
                            field.name,
                            field.field_type.name()
                        ));
                    }
                    FieldType::Freg | FieldType::XmmReg if !self.reg.contains_key("xmm") => {
                        return Err(format!(
                            "[inst.{inst_name}].{}: type '{}' requires [reg.xmm] section",
                            field.name,
                            field.field_type.name()
                        ));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Validate lowering rules: check that every instruction mnemonic in `[lower.*]`,
    /// `[lower_term.*]`, and `[lower_pattern.*]` resolves to a known instruction.
    ///
    /// - Uppercase mnemonics (e.g. `LEA_R64_SIB`): must be a direct key in `self.inst`.
    /// - Lowercase mnemonics (e.g. `mov`, `add`): must match the first word of some
    ///   instruction's `asm` template.
    /// - Pseudo-calls (`@...`), labels (`...:`), comments (`#...`), and empty lines are skipped.
    /// - Template rules (`template` field populated, `insts` empty) are skipped.
    ///
    /// Called after `expand_variants()` so that synthetic variant instruction names
    /// are available for validation.
    pub fn validate_lowering(&self) -> Result<(), String> {
        // Collect all rule sections into a flat iterator with section labels
        let sections: &[(&str, &BTreeMap<String, LowerRule>)] = &[
            ("lower", &self.lower),
            ("lower_term", &self.lower_term),
            ("lower_pattern", &self.lower_pattern),
        ];

        for &(section, rules) in sections {
            for (rule_name, rule) in rules {
                // Skip template-only rules (no insts to validate, expanded later)
                if rule.insts.is_empty() {
                    continue;
                }

                for inst_str in &rule.insts {
                    let trimmed = inst_str.trim();
                    // Skip empty, comments, pseudo-calls, and labels
                    if trimmed.is_empty()
                        || trimmed.starts_with('#')
                        || trimmed.starts_with('@')
                        || trimmed.ends_with(':')
                    {
                        continue;
                    }

                    // Skip template strings with unexpanded $CC placeholder
                    if trimmed.contains("$CC") {
                        continue;
                    }

                    // Extract mnemonic (first whitespace-delimited token)
                    let mnemonic = match trimmed.find(char::is_whitespace) {
                        Some(pos) => &trimmed[..pos],
                        None => trimmed,
                    };

                    let first_char = mnemonic.chars().next().unwrap_or('x');

                    if first_char.is_ascii_uppercase() {
                        // Uppercase = direct key lookup in inst table
                        if !self.inst.contains_key(mnemonic) {
                            // Suggest similar instruction names
                            let suggestion = Self::suggest_inst(mnemonic, self.inst.keys());
                            let hint = if suggestion.is_empty() {
                                String::new()
                            } else {
                                format!(". Did you mean '{suggestion}'?")
                            };
                            return Err(format!(
                                "[{section}.{rule_name}]: instruction key '{mnemonic}' not found in [inst.*]{hint}"
                            ));
                        }
                    } else {
                        // Lowercase = asm template mnemonic match (direct or CC-expanded)
                        if !self.mnemonic_exists_in_asm(mnemonic) {
                            return Err(format!(
                                "[{section}.{rule_name}]: mnemonic '{mnemonic}' does not match any [inst.*].asm template"
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Check whether a lowercase mnemonic matches any instruction's asm template,
    /// either directly or via condition-code expansion (e.g. `sete` matches `set{cc}`).
    fn mnemonic_exists_in_asm(&self, mnemonic: &str) -> bool {
        // Direct match: first word of any asm template equals mnemonic
        let direct = self.inst.values().any(|inst| {
            inst.asm
                .split_whitespace()
                .next()
                .map(|m| m == mnemonic)
                .unwrap_or(false)
        });
        if direct {
            return true;
        }

        // CC-expanded match: check each instruction whose asm template first word
        // contains a CondCode field placeholder (e.g. `{cc}`, `{cond}`).
        // Expand with every cc_names value and compare against the mnemonic.
        let cc_values: Vec<&String> = self.cc_names.values().collect();
        if cc_values.is_empty() {
            return false;
        }

        self.inst.values().any(|inst| {
            let first_word = inst.asm.split_whitespace().next().unwrap_or("");
            // Find which field (if any) has CondCode type — its placeholder name
            // in the asm template determines the expansion pattern.
            let cond_field_name = inst
                .fields
                .iter()
                .find(|f| matches!(f.field_type, FieldType::CondCode))
                .map(|f| f.name.as_str());
            if let Some(field_name) = cond_field_name {
                let placeholder = format!("{{{field_name}}}");
                if let Some(cc_pos) = first_word.find(&placeholder) {
                    let prefix = &first_word[..cc_pos];
                    let suffix = &first_word[cc_pos + placeholder.len()..];
                    return cc_values.iter().any(|cc| {
                        let expanded = format!("{prefix}{cc}{suffix}");
                        expanded == mnemonic
                    });
                }
            }
            false
        })
    }

    /// Suggest the closest instruction name by Levenshtein distance.
    fn suggest_inst(target: &str, candidates: impl Iterator<Item = impl AsRef<str>>) -> String {
        let mut best: Option<(usize, String)> = None;
        for c in candidates {
            let c = c.as_ref();
            let dist = Self::levenshtein(target, c);
            if dist == 0 {
                return c.to_string();
            }
            let is_closer = match best.as_ref() {
                Some((best_dist, _)) => dist < *best_dist,
                None => true,
            };
            if dist <= 3 && is_closer {
                best = Some((dist, c.to_string()));
            }
        }
        best.map(|(_, s)| s).unwrap_or_default()
    }

    /// Levenshtein distance between two strings.
    fn levenshtein(a: &str, b: &str) -> usize {
        let a_chars: Vec<char> = a.chars().collect();
        let b_chars: Vec<char> = b.chars().collect();
        let m = a_chars.len();
        let n = b_chars.len();
        let mut dp = vec![vec![0usize; n + 1]; m + 1];
        for (i, v) in dp.iter_mut().enumerate().take(m + 1) {
            v[0] = i;
        }
        for (j, v) in dp[0].iter_mut().enumerate().take(n + 1) {
            *v = j;
        }
        for i in 1..=m {
            for j in 1..=n {
                let cost = if a_chars[i - 1] == b_chars[j - 1] {
                    0
                } else {
                    1
                };
                dp[i][j] = (dp[i - 1][j] + 1)
                    .min(dp[i][j - 1] + 1)
                    .min(dp[i - 1][j - 1] + cost);
            }
        }
        dp[m][n]
    }

    /// Expand `inst.*.variants` into synthetic `inst.*` entries.
    ///
    /// Each variant gets a unique name `{parent}_{N}` and inherits the
    /// parent's `asm` template and `effect`. Called before codegen.
    pub fn expand_variants(&mut self) {
        let mut synthetic: Vec<(String, Instruction)> = Vec::new();
        for (inst_name, inst) in &self.inst {
            if let Some(ref variants) = inst.variants {
                let parent_asm = inst.asm.clone();
                let parent_effect = inst.effect.clone();
                for var in variants.iter() {
                    // Use custom name if provided, otherwise auto-generate from type signature
                    let suffix = var
                        .name
                        .clone()
                        .unwrap_or_else(|| Self::variant_suffix(&var.fields));
                    let syn_name = format!("{inst_name}_{suffix}");
                    // Inherit encoding from parent if variant doesn't specify its own
                    let encoding = var.enc.clone().or_else(|| inst.encoding.clone());
                    synthetic.push((
                        syn_name,
                        Instruction {
                            fields: var.fields.clone(),
                            encoding,
                            asm: parent_asm.clone(),
                            effect: var.effect.clone().or_else(|| parent_effect.clone()),
                            variants: None,
                        },
                    ));
                }
                // Use first variant's fields for parent entry (needed by lowering parser)
                // but the parent entry itself won't be used for encoding.
            }
        }
        for (name, inst) in synthetic {
            self.inst.entry(name).or_insert(inst);
        }
        // Remove pure variant-group entries (those that only exist to define variants).
        // Keep instructions that have their own fields (they're valid instructions with extra overloads).
        self.inst
            .retain(|_name, inst| inst.variants.is_none() || !inst.fields.is_empty());
    }

    /// Generate a short type-signature suffix for variant naming.
    fn variant_suffix(fields: &[InstField]) -> String {
        fields
            .iter()
            .map(|f| match &f.field_type {
                FieldType::I8 => "i8",
                FieldType::I16 => "i16",
                FieldType::I32 => "i32",
                FieldType::I64 => "i64",
                FieldType::U8 => "u8",
                FieldType::U16 => "u16",
                FieldType::U32 => "u32",
                FieldType::U64 => "u64",
                FieldType::F32 => "f32",
                FieldType::F64 => "f64",
                FieldType::Ireg => "Ireg",
                FieldType::Freg => "Freg",
                FieldType::GprReg => "Gpr",
                FieldType::XmmReg => "Xmm",
                FieldType::MemRef => "Mem",
                FieldType::BlockTarget => "Blk",
                FieldType::CondCode => "CC",
                FieldType::Opsize => "Op",
            })
            .collect::<Vec<_>>()
            .join("_")
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
    #[serde(default)]
    pub no_epilogue_label: bool,
}

fn default_endian() -> String {
    "little".into()
}
fn default_mode() -> u8 {
    64
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Capabilities {
    #[serde(default)]
    pub variable_length: bool,
    #[serde(default)]
    pub prefix_layers: u8,
    /// SIMD register widths in bits (e.g. [128, 256] for SSE+AVX).
    /// Used by IsaInfo metadata; codegen does not yet generate width-aware logic from this.
    #[serde(default)]
    pub simd_widths: Vec<u16>,
    #[serde(default)]
    pub mask_registers: bool,
    #[serde(default)]
    pub broadcast: bool,
    #[serde(default)]
    pub rounding_mode: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegGroup {
    pub count: u16,
    #[serde(default = "default_width")]
    #[allow(dead_code)]
    pub width: u16,
    #[serde(default)]
    pub names: Option<Vec<String>>,
    #[serde(default)]
    pub prefix: Option<String>,
}

fn default_width() -> u16 {
    64
}

#[derive(Debug, Clone, Deserialize)]
pub struct Abi {
    #[serde(default = "default_stack_align")]
    pub stack_align: u32,
    #[serde(default)]
    pub red_zone: Option<u32>,
    #[serde(default)]
    pub frame: Option<AbiFrame>,
    #[serde(default)]
    pub callee_saved: RegList,
    #[serde(default)]
    pub arg_regs: RegList,
    #[serde(default)]
    pub ret_regs: RegList,
    #[serde(default)]
    pub precolor: BTreeMap<String, String>,
    /// Named aliases for frequently-used virtual registers, e.g. `TMP0 = 96`.
    #[serde(default)]
    pub scratch: BTreeMap<String, u32>,
    /// Type-classified call lowering configuration (mov instruction names and
    /// the call instruction's target field). Absent → template-based Call
    /// lowering (arg0-arg7) is used.
    #[serde(default)]
    pub call: Option<AbiCall>,
}

/// Call lowering configuration: which mov instructions to use for moving
/// call arguments into ABI registers / receiving return values.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AbiCall {
    /// Integer argument mov (dest = physical GPR, src = VReg).
    #[serde(default)]
    pub arg_mov: Option<String>,
    /// Float argument mov (dest = physical XMM/FP, src = VReg).
    #[serde(default)]
    pub arg_mov_f: Option<String>,
    /// Integer return mov (dest = VReg, src = physical GPR).
    #[serde(default)]
    pub ret_mov: Option<String>,
    /// Float return mov (dest = VReg, src = physical XMM/FP).
    #[serde(default)]
    pub ret_mov_f: Option<String>,
    /// Call instruction name (e.g. "CALL_RIP_REL").
    #[serde(default)]
    pub call_inst: Option<String>,
    /// Field of the call instruction that holds the FuncRef number.
    #[serde(default)]
    pub call_field: Option<String>,
}

fn default_stack_align() -> u32 {
    16
}

#[derive(Debug, Clone, Deserialize)]
pub struct AbiFrame {
    pub sp: String,
    #[serde(default)]
    pub fp: Option<String>,
    /// If true, SUB64_R_IMM32 will receive `(0u32.wrapping_sub(frame_size)) & 0xFFF`
    /// instead of `frame_size`. Required for ISAs (like RISC-V) that use ADDI with
    /// a 12-bit signed immediate as their only subtract-immediate primitive.
    #[serde(default)]
    pub neg_alloc_imm: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RegList {
    #[serde(default)]
    pub gpr: Vec<String>,
    #[serde(default)]
    pub xmm: Vec<String>,
}

// ============================================================
// [inst.<NAME>]
// ============================================================

fn deser_effects<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
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
where
    D: serde::Deserializer<'de>,
{
    // Two field declaration formats:
    // 1. Inline table:  `fields = { dest = "Ireg", src = "Ireg" }`
    // 2. Empty:          `fields = {}` or `fields = []`
    //
    // Field order in the inline table is NOT significant — the asm template
    // defines the operand-to-field mapping via its {field} placeholders.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum FieldFormat {
        Inline(std::collections::BTreeMap<String, String>),
        #[allow(dead_code)]
        EmptyArray(Vec<serde::de::IgnoredAny>),
    }
    match FieldFormat::deserialize(deserializer)? {
        FieldFormat::Inline(map) => Ok(map
            .into_iter()
            .map(|(name, ty)| InstField {
                name,
                field_type: FieldType::from_str(&ty),
                role: None,
            })
            .collect()),
        FieldFormat::EmptyArray(_) => Ok(Vec::new()),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Instruction {
    #[serde(default, deserialize_with = "deser_fields")]
    pub fields: Vec<InstField>,
    #[serde(default)]
    pub encoding: Option<String>,
    /// Assembly format template, e.g. `"add {dest}, {src}"`.
    /// `{field}` placeholders reference field names declared in `fields`.
    pub asm: String,
    #[serde(default, deserialize_with = "deser_effects")]
    pub effect: Option<Vec<String>>,
    /// Type-signature variants for overloaded mnemonics.
    /// Each variant defines its own field types and encoding;
    /// the asm template and effects are inherited from the parent.
    #[serde(default)]
    pub variants: Option<Vec<VariantEntry>>,
}

/// A single variant in an overloaded instruction group.
///
/// In TOML: `{ dest = "Ireg", src = "Ireg", enc = "$modrm_rr ..." }`
/// Keys `enc`, `effect`, and `name` are special; all others are field-name→type pairs.
///
/// `enc` is optional — when omitted, the parent instruction's `encoding` is used.
/// `name` is optional — when provided, it replaces the auto-generated type-signature suffix.
#[derive(Debug, Clone)]
pub struct VariantEntry {
    pub fields: Vec<InstField>,
    pub enc: Option<String>,
    pub effect: Option<Vec<String>>,
    pub name: Option<String>,
}

// Custom Deserialize: extract `enc`+`effect`+`name`, remainder → field definitions.
impl<'de> serde::Deserialize<'de> for VariantEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let map: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::deserialize(deserializer)?;
        let enc = map.get("enc").cloned();
        let name = map.get("name").cloned();
        let effect = map
            .get("effect")
            .map(|e| e.split('+').map(|s| s.trim().to_string()).collect());
        let fields: Vec<InstField> = map
            .iter()
            .filter(|(k, _)| *k != "enc" && *k != "effect" && *k != "name" && *k != "role")
            .map(|(name, ty)| InstField {
                name: name.clone(),
                field_type: FieldType::from_str(ty),
                role: map.get("role").cloned(),
            })
            .collect();
        Ok(VariantEntry {
            fields,
            enc,
            effect,
            name,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstField {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    /// Optional explicit role: "def" (write), "use" (read), or "both".
    /// When absent, role is inferred from field name (dest/reg → def, src*/base → use).
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldType {
    // ── signed integer immediates ──
    I8,
    I16,
    I32,
    I64,
    // ── unsigned integer immediates ──
    U8,
    U16,
    U32,
    U64,
    // ── float immediates ──
    F32,
    F64,
    // ── virtual registers (register allocator managed) ──
    /// Integer GPR virtual register — must bind to [reg.gpr].
    Ireg,
    /// Float XMM virtual register — must bind to [reg.xmm].
    Freg,
    // ── physical registers ──
    /// Physical GPR — must exist in [reg.gpr].names.
    GprReg,
    /// Physical XMM — must exist in [reg.xmm].names.
    XmmReg,
    // ── structured ──
    /// Memory reference: [base + index*scale + disp].
    MemRef,
    // ── special ──
    BlockTarget,
    CondCode,
    Opsize,
}

impl FieldType {
    /// Parse a field type string, returning an error for unknown types.
    pub fn try_from_str(s: &str) -> Result<Self, String> {
        match s {
            // signed integers
            "i8" => Ok(Self::I8),
            "i16" => Ok(Self::I16),
            "i32" => Ok(Self::I32),
            "i64" => Ok(Self::I64),
            // unsigned integers
            "u8" => Ok(Self::U8),
            "u16" => Ok(Self::U16),
            "u32" => Ok(Self::U32),
            "u64" => Ok(Self::U64),
            // floats
            "f32" => Ok(Self::F32),
            "f64" => Ok(Self::F64),
            // virtual registers
            "Ireg" => Ok(Self::Ireg),
            "Freg" => Ok(Self::Freg),
            // physical registers
            "GprReg" => Ok(Self::GprReg),
            "XmmReg" => Ok(Self::XmmReg),
            // structured
            "MemRef" => Ok(Self::MemRef),
            // special (unchanged)
            "BlockTarget" => Ok(Self::BlockTarget),
            "CondCode" => Ok(Self::CondCode),
            "Opsize" => Ok(Self::Opsize),
            _ => Err(format!(
                "unknown field type '{s}'. Valid types: i8,i16,i32,i64, u8,u16,u32,u64, \
                 f32,f64, Ireg,Freg, GprReg,XmmReg, MemRef, BlockTarget, CondCode, Opsize"
            )),
        }
    }

    /// Parse a field type string, panicking on unknown types.
    /// Prefer [`try_from_str`] for recoverable error handling.
    pub fn from_str(s: &str) -> Self {
        Self::try_from_str(s).unwrap_or_else(|msg| panic!("{msg}"))
    }

    /// Human-readable name for error messages.
    pub fn name(&self) -> &'static str {
        match self {
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Ireg => "Ireg",
            Self::Freg => "Freg",
            Self::GprReg => "GprReg",
            Self::XmmReg => "XmmReg",
            Self::MemRef => "MemRef",
            Self::BlockTarget => "BlockTarget",
            Self::CondCode => "CondCode",
            Self::Opsize => "Opsize",
        }
    }

    /// Is this an integer immediate type (signed or unsigned)?
    pub fn is_int_imm(&self) -> bool {
        matches!(
            self,
            Self::I8
                | Self::I16
                | Self::I32
                | Self::I64
                | Self::U8
                | Self::U16
                | Self::U32
                | Self::U64
        )
    }

    /// Is this a floating-point immediate type?
    pub fn is_float_imm(&self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }
}

// Custom Deserialize for FieldType — uses from_str() to match TOML string names
// (e.g. "i64", "u8", "Ireg", "GprReg" — mixed case over lower/sentence-case conventions).
impl<'de> serde::Deserialize<'de> for FieldType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct FieldTypeVisitor;
        impl<'de> serde::de::Visitor<'de> for FieldTypeVisitor {
            type Value = FieldType;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a field type string (i8,i16,i32,i64,u8,u16,u32,u64,f32,f64,Ireg,Freg,GprReg,XmmReg,MemRef,BlockTarget,CondCode,Opsize)")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<FieldType, E> {
                FieldType::try_from_str(s).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(FieldTypeVisitor)
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
    pub conditions: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct EmitSection {
    #[serde(default)]
    pub prologue: Option<EmitBlock>,
    #[serde(default)]
    pub epilogue: Option<EmitBlock>,
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
