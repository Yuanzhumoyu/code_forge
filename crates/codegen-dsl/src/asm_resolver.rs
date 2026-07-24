//! Assembly instruction resolver — matches asm mnemonics to instruction definitions.
//!
//! Given an assembly call like `"add rd, rs1"`, the resolver:
//!
//! 1. Looks up `"add"` in the mnemonic table
//! 2. Finds candidate instruction(s) (e.g. `ADD_RM8_R8`)
//! 3. Maps operand positions to field names via the asm template
//! 4. Disambiguates when multiple instructions share a mnemonic
//!
//! # Condition code expansion
//!
//! Templates containing `{cond}` (CondCode field) are expanded:
//!   `"j{cond} .L{rel}"` + cc_names → "je", "jne", "jl", ...
//!   `"set{cond} {dest}"` + cc_names → "sete", "setne", "setl", ...

use crate::model::{FieldType, IsaModel};
use std::collections::HashMap;

// ============================================================
// Template fragment
// ============================================================

/// A fragment of an asm template.
#[derive(Debug, Clone)]
pub enum AsmFrag {
    /// Literal text: `"add "`, `", "`, `" .L"`
    Lit(String),
    /// Field placeholder: `{dest}`, `{src}`, `{cond}`
    Field { name: String, ty: FieldType },
}

/// A parsed asm template.
#[derive(Debug, Clone)]
pub struct ParsedTemplate {
    pub frags: Vec<AsmFrag>,
    /// The mnemonic prefix — first identifier-like segment.
    /// For `"add {dest}, {src}"` → `"add"`
    /// For `"j{cond} .L{rel}"` → `"j{cond}"` (contains a field)
    pub mnemonic: String,
    /// Whether the template contains a CondCode field (for cc expansion).
    pub has_cond_field: bool,
}

impl ParsedTemplate {
    /// Parse an asm template string.
    pub fn parse(asm: &str, fields: &[(String, FieldType)]) -> Self {
        let mut frags: Vec<AsmFrag> = Vec::new();
        let mut rest = asm;
        let field_map: HashMap<&str, &FieldType> =
            fields.iter().map(|(n, t)| (n.as_str(), t)).collect();

        while let Some(pos) = rest.find('{') {
            if pos > 0 {
                frags.push(AsmFrag::Lit(rest[..pos].to_string()));
            }
            let end = match rest[pos + 1..].find('}') {
                Some(e) => pos + 1 + e,
                None => {
                    frags.push(AsmFrag::Lit(rest[pos..].to_string()));
                    rest = "";
                    break;
                }
            };
            let field_name = &rest[pos + 1..end];
            // Check for format specifier: {field:x} → extract just "field"
            let clean_name = field_name.split(':').next().unwrap_or(field_name);
            let ty = field_map
                .get(clean_name)
                .cloned()
                .cloned()
                .unwrap_or(FieldType::VReg);
            frags.push(AsmFrag::Field {
                name: clean_name.to_string(),
                ty,
            });
            rest = &rest[end + 1..];
        }
        if !rest.is_empty() {
            frags.push(AsmFrag::Lit(rest.to_string()));
        }

        // Extract mnemonic
        let mnemonic = Self::extract_mnemonic(&frags);

        // Check for CondCode field
        let has_cond_field = frags.iter().any(|f| matches!(f, AsmFrag::Field { ty: FieldType::CondCode, .. }));

        ParsedTemplate {
            frags,
            mnemonic,
            has_cond_field,
        }
    }

    /// Extract the mnemonic prefix from template fragments.
    fn extract_mnemonic(frags: &[AsmFrag]) -> String {
        let mut mnem = String::new();
        for frag in frags {
            match frag {
                AsmFrag::Lit(s) => {
                    // Take everything up to the first whitespace or punctuation
                    for ch in s.chars() {
                        if ch.is_whitespace() || ch == ',' || ch == '.' {
                            return mnem;
                        }
                        mnem.push(ch);
                    }
                }
                AsmFrag::Field { name, .. } => {
                    mnem.push('{');
                    mnem.push_str(name);
                    mnem.push('}');
                    // If the mnemonic contains fields (like j{cond}), stop after it
                    return mnem;
                }
            }
        }
        mnem
    }

    /// Expand a CondCode template by replacing `{cond}` with a specific cc name.
    /// Returns the expanded mnemonic (e.g. "je") and the template with the field removed.
    pub fn expand_cond(&self, cc_name: &str) -> String {
        let mut result = String::new();
        for frag in &self.frags {
            match frag {
                AsmFrag::Lit(s) => result.push_str(s),
                AsmFrag::Field { ty: FieldType::CondCode, .. } => result.push_str(cc_name),
                AsmFrag::Field { name, .. } => {
                    result.push('{');
                    result.push_str(name);
                    result.push('}');
                }
            }
        }
        // Extract just the mnemonic part
        let mnem_end = result.find(|c: char| c.is_whitespace()).unwrap_or(result.len());
        // For j{cond} templates: "je .L{rel}" → mnemonic is "je"
        result[..mnem_end].to_string()
    }

    /// Get the ordered list of (field_name, FieldType) from the template.
    pub fn field_order(&self) -> Vec<(String, FieldType)> {
        self.frags
            .iter()
            .filter_map(|f| match f {
                AsmFrag::Field { name, ty } => Some((name.clone(), ty.clone())),
                _ => None,
            })
            .collect()
    }
}

// ============================================================
// Resolver
// ============================================================

/// An entry in the mnemonic lookup table.
#[derive(Debug, Clone)]
pub struct InstEntry {
    pub inst_name: String,
    /// Field order from the asm template (operand positions).
    pub template_order: Vec<(String, FieldType)>,
}

/// Result of resolving an asm call.
#[derive(Debug, Clone)]
pub struct ResolvedInst {
    /// The TOML instruction name (e.g. "ADD_RM8_R8").
    pub inst_name: String,
    /// Field bindings in template order: (field_name, operand_text).
    pub bindings: Vec<(String, String)>,
}

/// The main assembly resolver.
pub struct AsmResolver {
    /// Mnemonic string → candidate instruction entries.
    mnemonic_table: HashMap<String, Vec<InstEntry>>,
    /// Condition code mnemonic → (inst_name, cond_value as u8).
    /// e.g. "je" → ("JCC_REL32", 0x94)
    cc_mnemonics: HashMap<String, (String, u8)>,
    /// Condition code mnemonic → (inst_name, cond_value as u8) for SETcc.
    /// e.g. "sete" → ("SETCC_RM8", 0x94)
    cc_set_mnemonics: HashMap<String, (String, u8)>,
    /// All registered instruction names (for validation).
    inst_names: Vec<String>,
}

impl AsmResolver {
    /// Build the resolver from the ISA model.
    pub fn build(model: &IsaModel) -> Self {
        let mut mnemonic_table: HashMap<String, Vec<InstEntry>> = HashMap::new();
        let mut cc_mnemonics: HashMap<String, (String, u8)> = HashMap::new();
        let mut cc_set_mnemonics: HashMap<String, (String, u8)> = HashMap::new();
        let inst_names: Vec<String> = model.inst.keys().cloned().collect();

        for (inst_name, inst) in &model.inst {
            let fields: Vec<(String, FieldType)> = inst
                .fields
                .iter()
                .map(|f| (f.name.clone(), f.field_type.clone()))
                .collect();

            let template = ParsedTemplate::parse(&inst.asm, &fields);
            let template_order = template.field_order();

            if template.has_cond_field {
                // Expand condition code mnemonics
                for (hex_key, cc_name) in &model.cc_names {
                    let cond_val = u64::from_str_radix(
                        hex_key.trim_start_matches("0x").trim_start_matches("0X"),
                        16,
                    )
                    .unwrap_or(0) as u8;
                    let expanded_mnemonic = template.expand_cond(cc_name);

                    let cc_map = if template.mnemonic.starts_with('j') {
                        &mut cc_mnemonics
                    } else {
                        &mut cc_set_mnemonics
                    };
                    cc_map.insert(expanded_mnemonic, (inst_name.clone(), cond_val));
                }

                // Also add to mnemonic_table so resolve_with_cond can find it by inst_name
                mnemonic_table
                    .entry(format!("__cc__{}", inst_name))
                    .or_default()
                    .push(InstEntry {
                        inst_name: inst_name.clone(),
                        template_order,
                    });
            } else {
                let mnemonic = template.mnemonic.clone();
                mnemonic_table
                    .entry(mnemonic)
                    .or_default()
                    .push(InstEntry {
                        inst_name: inst_name.clone(),
                        template_order,
                    });
            }
        }

        AsmResolver {
            mnemonic_table,
            cc_mnemonics,
            cc_set_mnemonics,
            inst_names,
        }
    }

    /// Resolve an asm call: `"add rd, rs1"` → ("ADD_RM8_R8", [(dest, "rd"), (src, "rs1")])
    ///
    /// `operands` are the operand text strings (after comma-splitting and trimming).
    pub fn resolve(
        &self,
        mnemonic: &str,
        operands: &[String],
    ) -> Result<ResolvedInst, String> {
        // 1. Check condition code mnemonics
        if let Some((inst_name, cond_val)) = self.cc_mnemonics.get(mnemonic) {
            return self.resolve_with_cond(inst_name, cond_val, operands);
        }
        if let Some((inst_name, cond_val)) = self.cc_set_mnemonics.get(mnemonic) {
            return self.resolve_with_cond(inst_name, cond_val, operands);
        }

        // 2. Look up in mnemonic table
        let candidates = self.mnemonic_table.get(mnemonic).ok_or_else(|| {
            let suggestions = self.suggest(mnemonic);
            format!(
                "unknown mnemonic '{}'.{} Available: {:?}",
                mnemonic, suggestions, self.inst_names
            )
        })?;

        // 3. Filter by operand count
        let matching: Vec<&InstEntry> = candidates
            .iter()
            .filter(|e| e.template_order.len() == operands.len())
            .collect();

        if matching.is_empty() {
            return Err(format!(
                "no variant of '{}' accepts {} operand(s) (expected one of {:?})",
                mnemonic,
                operands.len(),
                candidates
                    .iter()
                    .map(|e| e.template_order.len())
                    .collect::<Vec<_>>()
            ));
        }

        // 4. If unambiguous, use it
        if matching.len() == 1 {
            let entry = matching[0];
            return Ok(ResolvedInst {
                inst_name: entry.inst_name.clone(),
                bindings: entry
                    .template_order
                    .iter()
                    .zip(operands.iter())
                    .map(|((name, _), val)| (name.clone(), val.clone()))
                    .collect(),
            });
        }

        // 5. Disambiguate by operand type hints
        self.disambiguate(mnemonic, &matching, operands)
    }

    /// Resolve with a pre-determined cond value.
    fn resolve_with_cond(
        &self,
        inst_name: &str,
        cond_val: &u8,
        operands: &[String],
    ) -> Result<ResolvedInst, String> {
        // Build bindings using the template from any entry with this inst_name
        let cond_str = format!("0x{cond_val:02X}");
        let mut bindings = Vec::new();

        // Find an entry with this inst_name (it may or may not be in the mnemonic table,
        // since cond-code instructions are only in cc_mnemonics).
        let entry = self
            .mnemonic_table
            .values()
            .flatten()
            .find(|e| e.inst_name == inst_name);

        if let Some(entry) = entry {
            let mut op_idx = 0;
            for (name, ty) in &entry.template_order {
                if matches!(ty, FieldType::CondCode) {
                    bindings.push((name.clone(), cond_str.clone()));
                } else if op_idx < operands.len() {
                    bindings.push((name.clone(), operands[op_idx].clone()));
                    op_idx += 1;
                }
            }
        } else {
            // Fallback: assume first field is cond (for cc instructions not in mnemonic table)
            bindings.push(("cond".to_string(), cond_str.clone()));
            for (i, op) in operands.iter().enumerate() {
                bindings.push((format!("field_{i}"), op.clone()));
            }
        }

        Ok(ResolvedInst {
            inst_name: inst_name.to_string(),
            bindings,
        })
    }

    /// Disambiguate between multiple matching candidates.
    fn disambiguate(
        &self,
        _mnemonic: &str,
        candidates: &[&InstEntry],
        operands: &[String],
    ) -> Result<ResolvedInst, String> {
        // Helper: classify an operand string
        let classify = |o: &str| -> OperandKind {
            if o.starts_with("VReg(") {
                return OperandKind::VReg;
            }
            if o.starts_with("0x") || o.starts_with("0X") || o.starts_with("0b") || o.starts_with("0B") || o.starts_with("0o") || o.starts_with("0O") {
                return OperandKind::Literal;
            }
            if o.contains('.') && o.parse::<f64>().is_ok() {
                return OperandKind::Literal; // float
            }
            if o.parse::<i64>().is_ok() {
                return OperandKind::Literal;
            }
            if o.starts_with('%') {
                return OperandKind::Temp;
            }
            if o.starts_with('.') {
                return OperandKind::Label;
            }
            if o.chars().next().is_some_and(|c| c.is_uppercase()) || o.starts_with("XMM") {
                return OperandKind::Reg;
            }
            OperandKind::Ident
        };

        // Score each candidate: count how many operands match the expected field type.
        // Higher weight for exact type matches, lower for generic matches.
        let mut scored: Vec<(i32, &&InstEntry)> = candidates.iter().map(|e| {
            let score = e.template_order.iter().zip(operands.iter()).map(|((_, ft), op)| {
                let kind = classify(op);
                let is_const_keyword = op == "iconst" || op == "fconst";
                match (ft, kind) {
                    // Exact matches → score 2
                    (FieldType::Reg, OperandKind::Reg) => 2,
                    (FieldType::VReg, OperandKind::VReg) => 2,
                    (FieldType::I64, OperandKind::Literal) => 2,
                    (FieldType::U8, OperandKind::Literal) => 2,
                    (FieldType::U32, OperandKind::Literal) => 2,
                    (FieldType::BlockTarget, OperandKind::Label) => 2,
                    // Good matches → score 1
                    (FieldType::VReg, OperandKind::Ident) => 1,
                    (FieldType::VReg, OperandKind::Temp) => 1,
                    (FieldType::BlockTarget, OperandKind::Ident) => 1,
                    // I64/U8/U32 matching Ident (for iconst/fconst) → score 2 for keywords, 0 otherwise
                    (FieldType::I64, OperandKind::Ident) if is_const_keyword => 2,
                    (FieldType::I64, OperandKind::Ident) => 0,
                    (FieldType::U8, OperandKind::Ident) if is_const_keyword => 2,
                    (FieldType::U8, OperandKind::Ident) => 0,
                    (FieldType::U32, OperandKind::Ident) if is_const_keyword => 2,
                    (FieldType::U32, OperandKind::Ident) => 0,
                    _ => 0,
                }
            }).sum();
            (score, e)
        }).collect();

        // Sort by score (highest first)
        scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));

        let (_best_score, entry) = scored[0];
        Ok(ResolvedInst {
            inst_name: entry.inst_name.clone(),
            bindings: entry
                .template_order
                .iter()
                .zip(operands.iter())
                .map(|((name, _), val)| (name.clone(), val.clone()))
                .collect(),
        })
    }

    /// Suggest similar mnemonics for error messages.
    fn suggest(&self, mnemonic: &str) -> String {
        let all_mnemonics: Vec<&String> = self.mnemonic_table.keys().collect();
        let mut best: Option<(&String, usize)> = None;
        let target = mnemonic.to_lowercase();

        for m in &all_mnemonics {
            let dist = levenshtein(&target, &m.to_lowercase());
            if dist <= 3 && (best.is_none() || dist < best.unwrap().1) {
                best = Some((m, dist));
            }
        }

        match best {
            Some((name, _)) => format!(" Did you mean '{}'?", name),
            None => String::new(),
        }
    }
}

/// Operand kind classification for disambiguation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandKind {
    Ident,
    Reg,
    Temp,
    VReg,
    Label,
    Literal,
}

/// Simple Levenshtein distance.
pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let n = a.len();
    let m = b.len();
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr = vec![0usize; m + 1];
    for i in 1..=n {
        curr[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[m]
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use std::collections::HashMap;

    fn make_model() -> IsaModel {
        let mut inst = HashMap::new();
        inst.insert(
            "ADD_RM8_R8".into(),
            Instruction {
                fields: vec![
                    InstField { name: "dest".into(), field_type: FieldType::VReg },
                    InstField { name: "src".into(), field_type: FieldType::VReg },
                ],
                encoding: Some("$rex_modrm_rr 0x01 src dest".into()),
                asm: "add {dest}, {src}".into(),
                effect: None,
            },
        );
        inst.insert(
            "MOV_R8_RM".into(),
            Instruction {
                fields: vec![
                    InstField { name: "dest".into(), field_type: FieldType::VReg },
                    InstField { name: "src".into(), field_type: FieldType::VReg },
                ],
                encoding: Some("$rex_modrm_rr 0x8B dest src".into()),
                asm: "mov {dest}, {src}".into(),
                effect: None,
            },
        );
        inst.insert(
            "MOV64_RR".into(),
            Instruction {
                fields: vec![
                    InstField { name: "dest".into(), field_type: FieldType::Reg },
                    InstField { name: "src".into(), field_type: FieldType::Reg },
                ],
                encoding: Some("$rexw_modrm_rr 0x8B dest src".into()),
                asm: "mov {dest}, {src}".into(),
                effect: None,
            },
        );
        inst.insert(
            "MOV_REG_IMM64".into(),
            Instruction {
                fields: vec![
                    InstField { name: "reg".into(), field_type: FieldType::VReg },
                    InstField { name: "imm".into(), field_type: FieldType::I64 },
                ],
                encoding: Some("$mov_imm64 reg imm".into()),
                asm: "mov {reg}, 0x{imm:x}".into(),
                effect: None,
            },
        );
        inst.insert(
            "JCC_REL32".into(),
            Instruction {
                fields: vec![
                    InstField { name: "cond".into(), field_type: FieldType::CondCode },
                    InstField { name: "rel".into(), field_type: FieldType::BlockTarget },
                ],
                encoding: Some("{0x0F:[0;8]} {cond:[0;8]} !rel4".into()),
                asm: "j{cond} .L{rel}".into(),
                effect: Some(vec!["Branch".into()]),
            },
        );
        inst.insert(
            "SETCC_RM8".into(),
            Instruction {
                fields: vec![
                    InstField { name: "cond".into(), field_type: FieldType::CondCode },
                    InstField { name: "dest".into(), field_type: FieldType::VReg },
                ],
                encoding: Some("$setcc dest cond".into()),
                asm: "set{cond} {dest}".into(),
                effect: None,
            },
        );
        inst.insert(
            "PUSH64_R".into(),
            Instruction {
                fields: vec![
                    InstField { name: "reg".into(), field_type: FieldType::Reg },
                ],
                encoding: Some("$push_reg reg".into()),
                asm: "push {reg}".into(),
                effect: Some(vec!["Write".into()]),
            },
        );
        inst.insert(
            "RET".into(),
            Instruction {
                fields: vec![],
                encoding: Some("{0xC3:[0;8]}".into()),
                asm: "ret".into(),
                effect: Some(vec!["Ret".into()]),
            },
        );

        let mut cc_names = HashMap::new();
        cc_names.insert("0x94".into(), "e".into());
        cc_names.insert("0x95".into(), "ne".into());
        cc_names.insert("0x9C".into(), "l".into());

        IsaModel {
            meta: Meta {
                name: "test".into(), version: "1".into(), endian: "little".into(),
                mode: 64, max_inst_len: 15, capabilities: Default::default(),
                no_default_lowering: true,
            },
            reg: {
                let mut m = HashMap::new();
                m.insert("gpr".into(), RegGroup {
                    count: 16, width: 64,
                    names: Some(["RAX","RCX","RDX","RBX","RSP","RBP","RSI","RDI",
                                 "R8","R9","R10","R11","R12","R13","R14","R15"]
                        .iter().map(|s| s.to_string()).collect()),
                    prefix: None,
                });
                m
            },
            abi: None,
            inst,
            lower: HashMap::new(),
            lower_term: HashMap::new(),
            lower_pattern: HashMap::new(),
            emit: None,
            enc_macros: HashMap::new(),
            enc_scatters: HashMap::new(),
            cc_names,
            lang_tokens: None,
            lang_keywords: None,
            lang_rules: None,
            dyn_types: HashMap::new(),
        }
    }

    #[test]
    fn test_template_parse_simple() {
        let fields = vec![
            ("dest".into(), FieldType::VReg),
            ("src".into(), FieldType::VReg),
        ];
        let t = ParsedTemplate::parse("add {dest}, {src}", &fields);
        assert_eq!(t.mnemonic, "add");
        assert!(!t.has_cond_field);
        assert_eq!(t.field_order().len(), 2);
    }

    #[test]
    fn test_template_parse_cond() {
        let fields = vec![
            ("cond".into(), FieldType::CondCode),
            ("rel".into(), FieldType::BlockTarget),
        ];
        let t = ParsedTemplate::parse("j{cond} .L{rel}", &fields);
        assert!(t.has_cond_field);
        // Mnemonic includes the cond field
        assert!(t.mnemonic.contains("{cond}"));
    }

    #[test]
    fn test_template_expand_cond() {
        let fields = vec![
            ("cond".into(), FieldType::CondCode),
            ("rel".into(), FieldType::BlockTarget),
        ];
        let t = ParsedTemplate::parse("j{cond} .L{rel}", &fields);
        assert_eq!(t.expand_cond("e"), "je");
        assert_eq!(t.expand_cond("ne"), "jne");
    }

    #[test]
    fn test_resolve_simple() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);

        let result = resolver.resolve("add", &["rd".into(), "rs1".into()]).unwrap();
        assert_eq!(result.inst_name, "ADD_RM8_R8");
        assert_eq!(result.bindings[0], ("dest".into(), "rd".into()));
        assert_eq!(result.bindings[1], ("src".into(), "rs1".into()));
    }

    #[test]
    fn test_resolve_no_operands() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);

        let result = resolver.resolve("ret", &[]).unwrap();
        assert_eq!(result.inst_name, "RET");
        assert!(result.bindings.is_empty());
    }

    #[test]
    fn test_resolve_cc_mnemonic() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);

        let result = resolver.resolve("je", &["target".into()]).unwrap();
        assert_eq!(result.inst_name, "JCC_REL32");
        // Should have cond=0x94 and rel=target
        let has_cond = result.bindings.iter().any(|(n, v)| n == "cond" && v == "0x94");
        assert!(has_cond);
    }

    #[test]
    fn test_resolve_mov_disambiguate_reg() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);

        // "mov RBP, RSP" with Reg operands → MOV64_RR
        let result = resolver.resolve("mov", &["RBP".into(), "RSP".into()]).unwrap();
        assert_eq!(result.inst_name, "MOV64_RR");
    }

    #[test]
    fn test_resolve_mov_disambiguate_vreg() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);

        // "mov rd, rs1" with VReg operands → MOV_R8_RM
        let result = resolver.resolve("mov", &["rd".into(), "rs1".into()]).unwrap();
        assert_eq!(result.inst_name, "MOV_R8_RM");
    }

    #[test]
    fn test_resolve_push() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);

        let result = resolver.resolve("push", &["RBP".into()]).unwrap();
        assert_eq!(result.inst_name, "PUSH64_R");
    }

    #[test]
    fn test_resolve_setcc() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);

        let result = resolver.resolve("sete", &["rd".into()]).unwrap();
        assert_eq!(result.inst_name, "SETCC_RM8");
    }

    #[test]
    fn test_resolve_unknown() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);

        let err = resolver.resolve("nonexistent", &[]).unwrap_err();
        assert!(err.contains("unknown mnemonic"));
    }

    #[test]
    fn test_resolve_mov_imm() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);

        // "mov rd, 0x42" — one VReg, one literal → MOV_REG_IMM64
        let result = resolver.resolve("mov", &["rd".into(), "0x42".into()]).unwrap();
        assert_eq!(result.inst_name, "MOV_REG_IMM64");
    }
}
