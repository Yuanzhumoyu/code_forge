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
                .unwrap_or(FieldType::Ireg);
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
        let has_cond_field = frags.iter().any(|f| {
            matches!(
                f,
                AsmFrag::Field {
                    ty: FieldType::CondCode,
                    ..
                }
            )
        });

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
                AsmFrag::Field {
                    ty: FieldType::CondCode,
                    ..
                } => result.push_str(cc_name),
                AsmFrag::Field { name, .. } => {
                    result.push('{');
                    result.push_str(name);
                    result.push('}');
                }
            }
        }
        // Extract just the mnemonic part
        let mnem_end = result
            .find(|c: char| c.is_whitespace())
            .unwrap_or(result.len());
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

    /// Get each template field's operand shape, aligned with `field_order()`.
    ///
    /// 判定字段是否为内存操作数：模板中该字段紧跟在 `[` 之后（如 `xadd [{base}], {src}`
    /// 的 base、`stp ..., [{rn}, #{imm}]!` 的 rn——前一个 Literal 片段以 `[` 结尾）。
    /// lower 规则书写操作数时内存操作数必须带方括号（`[rs1]`），普通操作数禁止带
    /// 方括号——DSL 据此校验，防止"碰巧按位置绑定但不符合 asm 编码格式"的写法。
    pub fn operand_shapes(&self) -> Vec<OperandShape> {
        let mut shapes = Vec::new();
        for (i, frag) in self.frags.iter().enumerate() {
            if matches!(frag, AsmFrag::Field { .. }) {
                let prev_lit = self.frags[..i].iter().rev().find_map(|f| match f {
                    AsmFrag::Lit(s) => Some(s.as_str()),
                    _ => None,
                });
                let is_mem = prev_lit.is_some_and(|s| s.ends_with('['));
                shapes.push(if is_mem {
                    OperandShape::Memory
                } else {
                    OperandShape::Plain
                });
            }
        }
        shapes
    }
}

/// 模板字段的操作数形状（与 `field_order()` 对齐）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandShape {
    /// `[field]`——内存操作数，lower 规则书写时必须带方括号（如 `[rs1]`）。
    Memory,
    /// `{field}`——普通操作数（寄存器/立即数），书写时禁止带方括号。
    Plain,
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
    /// 每个模板字段的操作数形状（与 template_order 对齐）——lower/emit 书写时
    /// 内存操作数必须带方括号，AsmResolver 据此校验格式一致性。
    pub template_shapes: Vec<OperandShape>,
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
    /// GPR register names (from [reg.gpr].names), for type tagging.
    gpr_names: Vec<String>,
    /// XMM register names (from [reg.xmm].names), for type tagging.
    xmm_names: Vec<String>,
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
            let template_shapes = template.operand_shapes();

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
                        template_shapes: template_shapes.clone(),
                    });
            } else {
                let mnemonic = template.mnemonic.clone();
                mnemonic_table.entry(mnemonic).or_default().push(InstEntry {
                    inst_name: inst_name.clone(),
                    template_order,
                    template_shapes,
                });
            }
        }

        // Collect GPR register names from all GPR bank variants (gpr, gpr64, gpr32, gpr16, gpr8l, gpr8h)
        let gpr_names: Vec<String> = model
            .reg
            .iter()
            .filter(|(k, _)| *k == "gpr" || k.starts_with("gpr"))
            .filter_map(|(_, g)| g.names.clone())
            .flatten()
            .collect();
        let xmm_names: Vec<String> = model
            .reg
            .get("xmm")
            .and_then(|g| g.names.clone())
            .unwrap_or_default();

        AsmResolver {
            mnemonic_table,
            cc_mnemonics,
            cc_set_mnemonics,
            inst_names,
            gpr_names,
            xmm_names,
        }
    }

    /// Resolve an asm call: `"add rd, rs1"` → ("ADD_RM8_R8", [(dest, "rd"), (src, "rs1")])
    ///
    /// `operands` are the operand text strings (after comma-splitting and trimming).
    pub fn resolve(&self, mnemonic: &str, operands: &[String]) -> Result<ResolvedInst, String> {
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

        // 3. Filter by operand count (memory operands like `[reg+imm]` expand to multiple fields)
        let matching: Vec<&InstEntry> = candidates
            .iter()
            .filter(|e| {
                // Count how many bindings the operands would produce:
                // each `[...]` operand maps to 1-4 fields; others map 1:1.
                let expanded = operands.iter().fold(0usize, |acc, op| {
                    if op.starts_with('[') {
                        acc + 4 // MemRef components: base, index, scale, disp (at most 4)
                    } else {
                        acc + 1
                    }
                });
                // Candidate matches if template has enough but not too many fields
                // (at least expanded - 3, at most expanded): memory operands may
                // miss some components (e.g. just [REG] → only base).
                e.template_order.len() >= expanded.saturating_sub(3)
                    && e.template_order.len() <= expanded
            })
            .collect();

        if matching.is_empty() {
            let hint = Self::vreg_hint(operands);
            return Err(format!(
                "no variant of '{}' accepts {} operand(s) (expected one of {:?}){}",
                mnemonic,
                operands.len(),
                candidates
                    .iter()
                    .map(|e| e.template_order.len())
                    .collect::<Vec<_>>(),
                hint,
            ));
        }

        // 4. If unambiguous, use it
        if matching.len() == 1 {
            let entry = matching[0];
            let bindings =
                Self::build_bindings(&entry.template_order, &entry.template_shapes, operands)?;
            return Ok(ResolvedInst {
                inst_name: entry.inst_name.clone(),
                bindings,
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

    /// Infer the `FieldType` of an operand string by checking it against the ISA model.
    ///
    /// Looks up register names in `[reg.gpr].names` and `[reg.xmm].names`,
    /// recognises numeric literals, labels, and lowering keywords.
    pub fn type_tag(&self, op: &str) -> FieldType {
        // 1. Known physical register names
        if self.gpr_names.iter().any(|n| n.eq_ignore_ascii_case(op)) {
            return FieldType::GprReg;
        }
        if self.xmm_names.iter().any(|n| n.eq_ignore_ascii_case(op)) {
            return FieldType::XmmReg;
        }

        // 2. VReg(N) literal
        if op.to_uppercase().starts_with("VREG(") {
            return FieldType::Ireg;
        }

        // 3. Hex/oct/bin literal
        if op.starts_with("0x")
            || op.starts_with("0X")
            || op.starts_with("0b")
            || op.starts_with("0B")
            || op.starts_with("0o")
            || op.starts_with("0O")
        {
            // Try to infer width from value range
            if let Ok(n) = u64::from_str_radix(
                &op[2..].replace('_', ""),
                if op.starts_with("0x") || op.starts_with("0X") {
                    16
                } else if op.starts_with("0b") || op.starts_with("0B") {
                    2
                } else {
                    8
                },
            ) {
                if n <= u8::MAX as u64 {
                    return FieldType::U8;
                }
                if n <= u16::MAX as u64 {
                    return FieldType::U16;
                }
                if n <= u32::MAX as u64 {
                    return FieldType::U32;
                }
            }
            return FieldType::I64;
        }

        // 4. Float literal
        if (op.contains('.') || op.contains('e') || op.contains('E')) && op.parse::<f64>().is_ok() {
            return FieldType::F64;
        }

        // 5. Decimal integer literal
        if op.parse::<i64>().is_ok() {
            if let Ok(n) = op.parse::<i64>() {
                if n >= 0 && n <= u8::MAX as i64 {
                    return FieldType::U8;
                }
                if n >= i8::MIN as i64 && n <= i8::MAX as i64 {
                    return FieldType::I8;
                }
                if n >= i16::MIN as i64 && n <= i16::MAX as i64 {
                    return FieldType::I16;
                }
                if n >= i32::MIN as i64 && n <= i32::MAX as i64 {
                    return FieldType::I32;
                }
            }
            return FieldType::I64;
        }

        // 6. Label (starts with '.')
        if op.starts_with('.') {
            return FieldType::BlockTarget;
        }

        // 7. Lowering keywords
        if op == "iconst" {
            return FieldType::I64;
        }
        if op == "fconst" {
            return FieldType::F64;
        }

        // 8. Default: positional param (rd, rs1, rs2) or identifier → treat as Ireg
        FieldType::Ireg
    }

    /// Generate a deprecation hint if any operand uses VReg(N) syntax.
    /// Physical register names (RAX, R10, XMM0) are preferred.
    fn vreg_hint(operands: &[String]) -> String {
        let vreg_count = operands
            .iter()
            .filter(|o| o.to_uppercase().starts_with("VREG("))
            .count();
        if vreg_count > 0 {
            ". Hint: use physical register names (e.g. RAX, R10, XMM0) instead of VReg(N)"
                .to_string()
        } else {
            String::new()
        }
    }

    /// Check whether an actual operand type is compatible with an expected field type.
    fn type_compatible(actual: &FieldType, expected: &FieldType) -> bool {
        if actual == expected {
            return true; // exact match
        }
        // Physical register subsumes into virtual register
        if *actual == FieldType::GprReg && *expected == FieldType::Ireg {
            return true;
        }
        if *actual == FieldType::XmmReg && *expected == FieldType::Freg {
            return true;
        }
        // Template positional params (rd/rs1/rs2) default to the Ireg type
        // tag, but the real class is decided by LowerCtx at codegen time —
        // an Ireg-tagged operand may legitimately drive an Freg instruction
        // (e.g. aarch64 `neon_fadd rd, rs1, rs2`). Without this, such
        // templates fail to resolve at ISA build time.
        if *actual == FieldType::Ireg && *expected == FieldType::Freg {
            return true;
        }
        // Numeric literal matches any integer immediate
        if actual.is_int_imm() && expected.is_int_imm() {
            return true;
        }
        // Float literal matches any float type
        if actual.is_float_imm() && expected.is_float_imm() {
            return true;
        }
        // BlockTarget matches label
        if *actual == FieldType::BlockTarget && *expected == FieldType::BlockTarget {
            return true;
        }
        false
    }

    /// Disambiguate between multiple matching candidates using type signatures.
    fn disambiguate(
        &self,
        mnemonic: &str,
        candidates: &[&InstEntry],
        operands: &[String],
    ) -> Result<ResolvedInst, String> {
        // Infer operand types from the ISA model
        let op_types: Vec<FieldType> = operands.iter().map(|o| self.type_tag(o)).collect();

        // Score candidates: +3 exact match, +1 compatible match, -1 mismatch
        let mut scored: Vec<(i32, &&InstEntry)> = candidates
            .iter()
            .map(|e| {
                let score: i32 = e
                    .template_order
                    .iter()
                    .zip(&op_types)
                    .map(|((_name, expected_ty), actual_ty)| {
                        if actual_ty == expected_ty {
                            3 // exact type match
                        } else if Self::type_compatible(actual_ty, expected_ty) {
                            1 // compatible (e.g. GprReg→Ireg, literal→imm)
                        } else {
                            -1 // mismatch
                        }
                    })
                    .sum();
                (score, e)
            })
            .collect();

        scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));

        let (best_score, entry) = scored[0];

        // If best score is negative, no candidate matches well — but still return best effort
        if best_score < 0 {
            let expected_sigs: Vec<String> = candidates
                .iter()
                .map(|e| {
                    e.template_order
                        .iter()
                        .map(|(_, t)| t.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .collect();
            return Err(format!(
                "cannot resolve '{}' with operands {:?}: no type-compatible variant. \
                 Operand types: {:?}. Expected signatures: [{}]",
                mnemonic,
                operands,
                op_types,
                expected_sigs.join("] [")
            ));
        }

        let bindings =
            Self::build_bindings(&entry.template_order, &entry.template_shapes, operands)?;
        Ok(ResolvedInst {
            inst_name: entry.inst_name.clone(),
            bindings,
        })
    }

    /// Build operand→field bindings, handling memory operands (`[...]`) which
    /// decompose into multiple fields (base, index, scale, disp).
    fn build_bindings(
        template_order: &[(String, FieldType)],
        shapes: &[OperandShape],
        operands: &[String],
    ) -> Result<Vec<(String, String)>, String> {
        let mut bindings: Vec<(String, String)> = Vec::new();
        let mut op_idx = 0;
        let mut tpl_idx = 0;

        while tpl_idx < template_order.len() && op_idx < operands.len() {
            let op = &operands[op_idx];

            if op.starts_with('[') {
                // 形状校验：内存操作数必须落在模板的内存字段（[field]）。
                if shapes.get(tpl_idx) != Some(&OperandShape::Memory) {
                    return Err(format!(
                        "operand '{}' is a bracketed memory operand, but template field \
                         '{}' expects a plain operand",
                        op, template_order[tpl_idx].0
                    ));
                }
                // 单个 MemRef 字段：整个内存操作数绑定到该字段（不再拆 base/index/scale/disp）
                if let Some((field_name, FieldType::MemRef)) = template_order.get(tpl_idx) {
                    bindings.push((field_name.clone(), op.clone()));
                    tpl_idx += 1;
                    op_idx += 1;
                    continue;
                }
                let mem = decompose_mem_operand(op)?;
                // Consume template fields that match MemRef component names
                while tpl_idx < template_order.len() {
                    let (field_name, _) = &template_order[tpl_idx];
                    match field_name.as_str() {
                        "base" | "rn" => {
                            bindings
                                .push((field_name.clone(), mem.base.clone().unwrap_or_default()));
                            tpl_idx += 1;
                        }
                        "index" => {
                            bindings.push(("index".into(), mem.index.clone().unwrap_or_default()));
                            tpl_idx += 1;
                        }
                        "scale" => {
                            bindings.push(("scale".into(), mem.scale.to_string()));
                            tpl_idx += 1;
                        }
                        "disp" | "imm" => {
                            bindings.push((field_name.clone(), mem.disp.to_string()));
                            tpl_idx += 1;
                        }
                        _ => break, // Non-memory field → done with this MemRef
                    }
                }
                op_idx += 1;
            } else {
                // 形状校验：普通操作数不得落在模板的内存字段（[field]）。
                if shapes.get(tpl_idx) == Some(&OperandShape::Memory) {
                    return Err(format!(
                        "operand '{}' must be a memory operand '[...]' per asm template \
                         (field '{}' is bracketed)",
                        op, template_order[tpl_idx].0
                    ));
                }
                let (field_name, _) = &template_order[tpl_idx];
                bindings.push((field_name.clone(), op.clone()));
                tpl_idx += 1;
                op_idx += 1;
            }
        }

        Ok(bindings)
    }

    /// Suggest similar mnemonics for error messages.
    fn suggest(&self, mnemonic: &str) -> String {
        let all_mnemonics: Vec<&String> = self.mnemonic_table.keys().collect();
        let mut best: Option<(&String, usize)> = None;
        let target = mnemonic.to_lowercase();

        for m in &all_mnemonics {
            let dist = levenshtein(&target, &m.to_lowercase());
            let is_closer = match best {
                Some((_, best_dist)) => dist < best_dist,
                None => true,
            };
            if dist <= 3 && is_closer {
                best = Some((m, dist));
            }
        }

        match best {
            Some((name, _)) => format!(" Did you mean '{}'?", name),
            None => String::new(),
        }
    }
}

/// Decomposed memory operand: `[base + index * scale + disp]`.
#[derive(Debug, Clone, Default)]
pub struct MemRef {
    pub base: Option<String>,
    pub index: Option<String>,
    pub scale: i64,
    pub disp: i64,
}

/// Parse a memory operand string like `[RAX+RCX*4+8]` into structured MemRef components.
pub fn decompose_mem_operand(text: &str) -> Result<MemRef, String> {
    let trimmed = text.trim();
    // 去掉 pre/post-index 标记（"[SP, #-16]!"）
    let body = trimmed.strip_suffix('!').unwrap_or(trimmed);
    let inner = body
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| format!("not a memory operand: {text}"))?
        .trim();

    if inner.is_empty() {
        return Ok(MemRef::default());
    }

    let mut base: Option<String> = None;
    let mut index: Option<String> = None;
    let mut scale: i64 = 1;
    let mut disp: i64 = 0;

    // 支持 "[SP, #-16]!" 风格（逗号分隔的 base 与 displacement）
    for piece in inner.split(',') {
        for term in piece.split('+') {
            let term = term.trim();
            if term.is_empty() {
                continue;
            }

            let term = if let Some(stripped) = term.strip_prefix('#') {
                // 立即数前缀（"#-16"、"#16"）——剥掉 '#' 再按数字处理
                stripped.to_string()
            } else {
                term.to_string()
            };
            let term = term.as_str();
            if let Some(pos) = term.find('*') {
                // REG * scale
                let reg = term[..pos].trim();
                let scale_str = term[pos + 1..].trim();
                if reg.is_empty() {
                    return Err(format!("empty register in scale term: '{term}'"));
                }
                index = Some(reg.to_string());
                scale = scale_str
                    .parse::<i64>()
                    .map_err(|_| format!("invalid scale value: '{scale_str}'"))?;
            } else if term.starts_with("0x") || term.starts_with("0X") {
                disp += i64::from_str_radix(&term[2..], 16)
                    .map_err(|_| format!("invalid hex displacement: '{term}'"))?;
            } else if let Ok(n) = term.parse::<i64>() {
                disp += n;
            } else if term.starts_with('-') && term.len() > 1 {
                // Subtraction: "-16" etc.
                if let Ok(n) = term.parse::<i64>() {
                    disp += n;
                } else {
                    return Err(format!("invalid displacement: '{term}'"));
                }
            } else {
                // Plain register name
                if base.is_none() {
                    base = Some(term.to_string());
                } else if index.is_none() {
                    index = Some(term.to_string());
                } else {
                    return Err(format!(
                        "too many registers in memory operand '{text}': already have base={base:?}, index={index:?}, found extra '{term}'"
                    ));
                }
            }
        }
    }

    Ok(MemRef {
        base,
        index,
        scale,
        disp,
    })
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
    use std::collections::BTreeMap;

    fn make_model() -> IsaModel {
        let mut inst = BTreeMap::new();
        inst.insert(
            "ADD_RM8_R8".into(),
            Instruction {
                fields: vec![
                    InstField {
                        role: None,
                        name: "dest".into(),
                        field_type: FieldType::Ireg,
                    },
                    InstField {
                        role: None,
                        name: "src".into(),
                        field_type: FieldType::Ireg,
                    },
                ],
                encoding: Some("$rex_modrm_rr 0x01 src dest".into()),
                asm: "add {dest}, {src}".into(),
                variants: None,
                opcodes: None,
                implicit: None,
                effect: None,
            },
        );
        inst.insert(
            "MOV_R8_RM".into(),
            Instruction {
                fields: vec![
                    InstField {
                        role: None,
                        name: "dest".into(),
                        field_type: FieldType::Ireg,
                    },
                    InstField {
                        role: None,
                        name: "src".into(),
                        field_type: FieldType::Ireg,
                    },
                ],
                encoding: Some("$rex_modrm_rr 0x8B dest src".into()),
                asm: "mov {dest}, {src}".into(),
                variants: None,
                opcodes: None,
                implicit: None,
                effect: None,
            },
        );
        inst.insert(
            "MOV64_RR".into(),
            Instruction {
                fields: vec![
                    InstField {
                        role: None,
                        name: "dest".into(),
                        field_type: FieldType::GprReg,
                    },
                    InstField {
                        role: None,
                        name: "src".into(),
                        field_type: FieldType::GprReg,
                    },
                ],
                encoding: Some("$rexw_modrm_rr 0x8B dest src".into()),
                asm: "mov {dest}, {src}".into(),
                variants: None,
                opcodes: None,
                implicit: None,
                effect: None,
            },
        );
        inst.insert(
            "MOV_REG_IMM64".into(),
            Instruction {
                fields: vec![
                    InstField {
                        role: None,
                        name: "reg".into(),
                        field_type: FieldType::Ireg,
                    },
                    InstField {
                        role: None,
                        name: "imm".into(),
                        field_type: FieldType::I64,
                    },
                ],
                encoding: Some("$mov_imm64 reg imm".into()),
                asm: "mov {reg}, 0x{imm:x}".into(),
                variants: None,
                opcodes: None,
                implicit: None,
                effect: None,
            },
        );
        inst.insert(
            "JCC_REL32".into(),
            Instruction {
                fields: vec![
                    InstField {
                        role: None,
                        name: "cond".into(),
                        field_type: FieldType::CondCode,
                    },
                    InstField {
                        role: None,
                        name: "rel".into(),
                        field_type: FieldType::BlockTarget,
                    },
                ],
                encoding: Some("{0x0F:[0;8]} {cond:[0;8]} !rel4".into()),
                asm: "j{cond} .L{rel}".into(),
                variants: None,
                opcodes: None,
                implicit: None,
                effect: Some(vec!["Branch".into()]),
            },
        );
        inst.insert(
            "SETCC_RM8".into(),
            Instruction {
                fields: vec![
                    InstField {
                        role: None,
                        name: "cond".into(),
                        field_type: FieldType::CondCode,
                    },
                    InstField {
                        role: None,
                        name: "dest".into(),
                        field_type: FieldType::Ireg,
                    },
                ],
                encoding: Some("$setcc dest cond".into()),
                asm: "set{cond} {dest}".into(),
                variants: None,
                opcodes: None,
                implicit: None,
                effect: None,
            },
        );
        inst.insert(
            "PUSH64_R".into(),
            Instruction {
                fields: vec![InstField {
                    role: None,
                    name: "reg".into(),
                    field_type: FieldType::GprReg,
                }],
                encoding: Some("$push_reg reg".into()),
                asm: "push {reg}".into(),
                variants: None,
                opcodes: None,
                implicit: None,
                effect: Some(vec!["Write".into()]),
            },
        );
        inst.insert(
            "RET".into(),
            Instruction {
                fields: vec![],
                encoding: Some("{0xC3:[0;8]}".into()),
                asm: "ret".into(),
                variants: None,
                opcodes: None,
                implicit: None,
                effect: Some(vec!["Ret".into()]),
            },
        );

        let mut cc_names = BTreeMap::new();
        cc_names.insert("0x94".into(), "e".into());
        cc_names.insert("0x95".into(), "ne".into());
        cc_names.insert("0x9C".into(), "l".into());

        IsaModel {
            reg_classes: BTreeMap::new(),
            spill: BTreeMap::new(),
            meta: Meta {
                name: "test".into(),
                version: "1".into(),
                endian: "little".into(),
                mode: 64,
                max_inst_len: 15,
                capabilities: Default::default(),
                no_default_lowering: true,
                no_epilogue_label: false,
                gpr_bank_order: vec![],
                modrm_force_disp_base: vec![],
                epilogue_jump_opcode: None,
                default_fpr_width: None,
                enable_pattern_isel: None,
            },
            reg: {
                let mut m = BTreeMap::new();
                m.insert(
                    "gpr".into(),
                    RegGroup {
                        count: 16,
                        width: 64,
                        names: Some(
                            [
                                "RAX", "RCX", "RDX", "RBX", "RSP", "RBP", "RSI", "RDI", "R8", "R9",
                                "R10", "R11", "R12", "R13", "R14", "R15",
                            ]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                        ),
                        prefix: None,
                        base_index: None,
                    },
                );
                m
            },
            abi: None,
            inst,
            lower: BTreeMap::new(),
            lower_term: BTreeMap::new(),
            lower_pattern: BTreeMap::new(),
            emit: None,
            enc_macros: BTreeMap::new(),
            enc_scatters: BTreeMap::new(),
            enc_constants: BTreeMap::new(),
            cc_names,
            lang_tokens: None,
            lang_keywords: None,
            lang_rules: None,
            dyn_types: BTreeMap::new(),
        }
    }

    #[test]
    fn test_template_parse_simple() {
        let fields = vec![
            ("dest".into(), FieldType::Ireg),
            ("src".into(), FieldType::Ireg),
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

        let result = resolver
            .resolve("add", &["rd".into(), "rs1".into()])
            .unwrap();
        assert_eq!(result.inst_name, "ADD_RM8_R8");
        assert_eq!(result.bindings[0], ("dest".into(), "rd".into()));
        assert_eq!(result.bindings[1], ("src".into(), "rs1".into()));
    }

    #[test]
    fn resolve_stp_preindex() {
        // 从 aarch64 的 TOML 解析真实模型验证 "[SP, #-16]!" 的 rn/imm 绑定
        let toml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../isa/aarch64_v10.toml"),
        )
        .expect("read aarch64 toml");
        let m = crate::parser::parse(&toml).expect("parse model");
        let resolver = AsmResolver::build(&m);
        let r = resolver.resolve("stp", &["X29".into(), "X30".into(), "[SP, #-16]!".into()]);
        eprintln!("stp resolve: {r:?}");
        let ok = r.unwrap();
        assert!(
            ok.bindings.iter().any(|(n, v)| n == "rt1" && v == "X29"),
            "{:?}",
            ok.bindings
        );
        assert!(
            ok.bindings.iter().any(|(n, v)| n == "rn" && v == "SP"),
            "{:?}",
            ok.bindings
        );
        assert!(
            ok.bindings.iter().any(|(n, v)| n == "imm" && v == "-16"),
            "{:?}",
            ok.bindings
        );
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
        let has_cond = result
            .bindings
            .iter()
            .any(|(n, v)| n == "cond" && v == "0x94");
        assert!(has_cond);
    }

    #[test]
    fn test_resolve_mov_disambiguate_reg() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);

        // "mov RBP, RSP" with Reg operands → MOV64_RR
        let result = resolver
            .resolve("mov", &["RBP".into(), "RSP".into()])
            .unwrap();
        assert_eq!(result.inst_name, "MOV64_RR");
    }

    #[test]
    fn test_resolve_mov_disambiguate_vreg() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);

        // "mov rd, rs1" with VReg operands → MOV_R8_RM
        let result = resolver
            .resolve("mov", &["rd".into(), "rs1".into()])
            .unwrap();
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
        let result = resolver
            .resolve("mov", &["rd".into(), "0x42".into()])
            .unwrap();
        assert_eq!(result.inst_name, "MOV_REG_IMM64");
    }

    // ── P8a: type_tag tests ──

    #[test]
    fn test_type_tag_gpr_reg() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);
        // "RAX", "RBP", "R12" are all in [reg.gpr].names
        assert_eq!(resolver.type_tag("RAX"), FieldType::GprReg);
        assert_eq!(resolver.type_tag("RBP"), FieldType::GprReg);
        assert_eq!(resolver.type_tag("R12"), FieldType::GprReg);
    }

    #[test]
    fn test_type_tag_unknown_is_ireg() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);
        // Unknown identifiers default to Ireg (positional params like rd, rs1)
        assert_eq!(resolver.type_tag("rd"), FieldType::Ireg);
        assert_eq!(resolver.type_tag("rs1"), FieldType::Ireg);
        assert_eq!(resolver.type_tag("unknown"), FieldType::Ireg);
    }

    #[test]
    fn test_type_tag_vreg_literal() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);
        assert_eq!(resolver.type_tag("VReg(0)"), FieldType::Ireg);
        assert_eq!(resolver.type_tag("VReg(97)"), FieldType::Ireg);
        assert_eq!(resolver.type_tag("vreg(5)"), FieldType::Ireg);
    }

    #[test]
    fn test_type_tag_hex_immediate() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);
        // Small hex → U8
        assert_eq!(resolver.type_tag("0x42"), FieldType::U8);
        // Larger hex → wider type
        assert_eq!(resolver.type_tag("0xFFFF"), FieldType::U16);
    }

    #[test]
    fn test_type_tag_decimal_immediate() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);
        assert_eq!(resolver.type_tag("42"), FieldType::U8);
        assert_eq!(resolver.type_tag("-1"), FieldType::I8);
        assert_eq!(resolver.type_tag("1000"), FieldType::I16);
    }

    #[test]
    fn test_type_tag_label() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);
        assert_eq!(resolver.type_tag(".L0"), FieldType::BlockTarget);
        assert_eq!(resolver.type_tag(".L_target"), FieldType::BlockTarget);
    }

    #[test]
    fn test_type_tag_lowering_keywords() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);
        assert_eq!(resolver.type_tag("iconst"), FieldType::I64);
        assert_eq!(resolver.type_tag("fconst"), FieldType::F64);
    }

    // ── P8b: type-based disambiguation tests ──

    #[test]
    fn test_disambiguate_reg_vs_vreg_by_type() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);
        // "mov RBP, RSP" → MOV64_RR (both GPR regs)
        let result = resolver
            .resolve("mov", &["RBP".into(), "RSP".into()])
            .unwrap();
        assert_eq!(result.inst_name, "MOV64_RR");
        // "mov rd, rs1" → MOV_R8_RM (both VRegs)
        let result = resolver
            .resolve("mov", &["rd".into(), "rs1".into()])
            .unwrap();
        assert_eq!(result.inst_name, "MOV_R8_RM");
    }

    #[test]
    fn test_disambiguate_imm_vs_reg_by_type() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);
        // "mov rd, 0x42" → MOV_REG_IMM64 (VReg + immediate)
        let result = resolver
            .resolve("mov", &["rd".into(), "0x42".into()])
            .unwrap();
        assert_eq!(result.inst_name, "MOV_REG_IMM64");
    }

    #[test]
    fn test_disambiguate_unknown_type_best_effort() {
        let model = make_model();
        let resolver = AsmResolver::build(&model);
        // Even "mov unknown, xy" should resolve to something (both default to Ireg → MOV_R8_RM)
        let result = resolver
            .resolve("mov", &["unknown".into(), "xy".into()])
            .unwrap();
        assert_eq!(result.inst_name, "MOV_R8_RM");
    }

    #[test]
    fn test_type_compatible_matrix() {
        // GprReg subsumes into Ireg
        assert!(AsmResolver::type_compatible(
            &FieldType::GprReg,
            &FieldType::Ireg
        ));
        // Exact match
        assert!(AsmResolver::type_compatible(
            &FieldType::Ireg,
            &FieldType::Ireg
        ));
        // Literal→immediate
        assert!(AsmResolver::type_compatible(
            &FieldType::I64,
            &FieldType::I32
        ));
        assert!(AsmResolver::type_compatible(
            &FieldType::U8,
            &FieldType::I64
        ));
        // Float→float
        assert!(AsmResolver::type_compatible(
            &FieldType::F64,
            &FieldType::F32
        ));
        // Mismatch
        assert!(!AsmResolver::type_compatible(
            &FieldType::Ireg,
            &FieldType::GprReg
        ));
        assert!(!AsmResolver::type_compatible(
            &FieldType::I64,
            &FieldType::F64
        ));
    }

    // ============================================================
    // Edge case tests
    // ============================================================

    #[test]
    fn test_build_resolver_empty_inst_set() {
        let mut reg = BTreeMap::new();
        reg.insert(
            "gpr".into(),
            RegGroup {
                count: 16,
                width: 64,
                names: Some(vec![
                    "RAX".into(),
                    "RCX".into(),
                    "RDX".into(),
                    "RBX".into(),
                    "RSP".into(),
                    "RBP".into(),
                    "RSI".into(),
                    "RDI".into(),
                ]),
                prefix: None,
                base_index: None,
            },
        );
        let model = IsaModel {
            reg_classes: BTreeMap::new(),
            spill: BTreeMap::new(),
            meta: Meta {
                name: "empty".into(),
                version: "1".into(),
                endian: "little".into(),
                mode: 64,
                max_inst_len: 15,
                capabilities: Default::default(),
                no_default_lowering: true,
                no_epilogue_label: false,
                gpr_bank_order: vec![],
                modrm_force_disp_base: vec![],
                epilogue_jump_opcode: None,
                default_fpr_width: None,
                enable_pattern_isel: None,
            },
            reg,
            abi: None,
            inst: BTreeMap::new(),
            lower: BTreeMap::new(),
            lower_term: BTreeMap::new(),
            lower_pattern: BTreeMap::new(),
            emit: None,
            enc_macros: BTreeMap::new(),
            enc_scatters: BTreeMap::new(),
            enc_constants: BTreeMap::new(),
            cc_names: BTreeMap::new(),
            lang_tokens: None,
            lang_keywords: None,
            lang_rules: None,
            dyn_types: BTreeMap::new(),
        };
        let resolver = AsmResolver::build(&model);
        // Shouldn't panic; any resolve attempt should return an error
        let err = resolver.resolve("anything", &[]).unwrap_err();
        assert!(err.contains("unknown mnemonic"), "Got: {err}");
    }

    #[test]
    fn test_resolve_instruction_with_empty_asm() {
        let mut inst = BTreeMap::new();
        inst.insert(
            "EMPTY_ASM".into(),
            Instruction {
                fields: vec![],
                encoding: None,
                asm: String::new(),
                variants: None,
                opcodes: None,
                implicit: None,
                effect: None,
            },
        );
        let mut reg = BTreeMap::new();
        reg.insert(
            "gpr".into(),
            RegGroup {
                count: 16,
                width: 64,
                names: Some(vec![
                    "RAX".into(),
                    "RCX".into(),
                    "RDX".into(),
                    "RBX".into(),
                    "RSP".into(),
                    "RBP".into(),
                    "RSI".into(),
                    "RDI".into(),
                ]),
                prefix: None,
                base_index: None,
            },
        );
        let model = IsaModel {
            reg_classes: BTreeMap::new(),
            spill: BTreeMap::new(),
            meta: Meta {
                name: "test".into(),
                version: "1".into(),
                endian: "little".into(),
                mode: 64,
                max_inst_len: 15,
                capabilities: Default::default(),
                no_default_lowering: true,
                no_epilogue_label: false,
                gpr_bank_order: vec![],
                modrm_force_disp_base: vec![],
                epilogue_jump_opcode: None,
                default_fpr_width: None,
                enable_pattern_isel: None,
            },
            reg,
            abi: None,
            inst,
            lower: BTreeMap::new(),
            lower_term: BTreeMap::new(),
            lower_pattern: BTreeMap::new(),
            emit: None,
            enc_macros: BTreeMap::new(),
            enc_scatters: BTreeMap::new(),
            enc_constants: BTreeMap::new(),
            cc_names: BTreeMap::new(),
            lang_tokens: None,
            lang_keywords: None,
            lang_rules: None,
            dyn_types: BTreeMap::new(),
        };
        let resolver = AsmResolver::build(&model);
        // With empty asm, the mnemonic table should be empty
        // (or contain an entry with empty mnemonic)
        // Resolve should fail for any query
        let err = resolver.resolve("anything", &[]).unwrap_err();
        assert!(err.contains("unknown mnemonic"));
    }

    #[test]
    fn test_template_parse_empty_asm() {
        let fields: Vec<(String, FieldType)> = vec![("dest".into(), FieldType::Ireg)];
        let t = ParsedTemplate::parse("", &fields);
        assert_eq!(t.mnemonic, "");
        assert!(!t.has_cond_field);
        assert!(t.field_order().is_empty());
    }

    #[test]
    fn test_template_parse_no_fields() {
        let fields: Vec<(String, FieldType)> = vec![];
        let t = ParsedTemplate::parse("ret", &fields);
        assert_eq!(t.mnemonic, "ret");
        assert!(!t.has_cond_field);
        assert!(t.field_order().is_empty());
    }

    // ═══════════════════════════════════════════════
    // MemRef decomposition tests
    // ═══════════════════════════════════════════════

    #[test]
    fn test_memref_simple_reg() {
        let m = decompose_mem_operand("[RAX]").unwrap();
        assert_eq!(m.base.as_deref(), Some("RAX"));
        assert_eq!(m.index, None);
        assert_eq!(m.scale, 1);
        assert_eq!(m.disp, 0);
    }

    #[test]
    fn test_memref_base_disp() {
        let m = decompose_mem_operand("[RAX+8]").unwrap();
        assert_eq!(m.base.as_deref(), Some("RAX"));
        assert_eq!(m.index, None);
        assert_eq!(m.disp, 8);
    }

    #[test]
    fn test_memref_base_index_scale() {
        let m = decompose_mem_operand("[RAX+RCX*4]").unwrap();
        assert_eq!(m.base.as_deref(), Some("RAX"));
        assert_eq!(m.index.as_deref(), Some("RCX"));
        assert_eq!(m.scale, 4);
        assert_eq!(m.disp, 0);
    }

    #[test]
    fn test_memref_full() {
        let m = decompose_mem_operand("[RAX+RCX*4+1024]").unwrap();
        assert_eq!(m.base.as_deref(), Some("RAX"));
        assert_eq!(m.index.as_deref(), Some("RCX"));
        assert_eq!(m.scale, 4);
        assert_eq!(m.disp, 1024);
    }

    #[test]
    fn test_memref_disp_only() {
        let m = decompose_mem_operand("[0x1000]").unwrap();
        assert_eq!(m.base, None);
        assert_eq!(m.index, None);
        assert_eq!(m.disp, 0x1000);
    }

    #[test]
    fn test_memref_empty() {
        let m = decompose_mem_operand("[]").unwrap();
        assert_eq!(m.base, None);
        assert_eq!(m.index, None);
        assert_eq!(m.disp, 0);
    }

    #[test]
    fn test_memref_decimal_disp() {
        let m = decompose_mem_operand("[RSP+16]").unwrap();
        assert_eq!(m.base.as_deref(), Some("RSP"));
        assert_eq!(m.disp, 16);
    }

    #[test]
    fn test_shape_check_missing_brackets() {
        // 模板 'xadd [{base}], {src}'：内存操作数必须带方括号。
        let mut m = make_model();
        m.inst.insert(
            "XADD_MEM_R".into(),
            crate::model::Instruction {
                fields: vec![
                    InstField {
                        role: None,
                        name: "base".into(),
                        field_type: FieldType::Ireg,
                    },
                    InstField {
                        role: None,
                        name: "src".into(),
                        field_type: FieldType::Ireg,
                    },
                ],
                encoding: Some("$rex_modrm_mem 0xC1 src base".into()),
                asm: "xadd [{base}], {src}".into(),
                variants: None,
                opcodes: None,
                implicit: None,
                effect: None,
            },
        );
        let resolver = AsmResolver::build(&m);

        // 缺方括号 → 报错（防"碰巧编译但不符合 asm 格式"）
        let err = resolver
            .resolve("xadd", &["rs1".into(), "rs2".into()])
            .unwrap_err();
        assert!(err.contains("memory operand"), "应提示缺方括号: {err}");

        // 正确形式 → base 绑定剥离方括号后的 rs1
        let ok = resolver
            .resolve("xadd", &["[rs1]".into(), "rs2".into()])
            .unwrap();
        assert_eq!(ok.bindings[0], ("base".into(), "rs1".into()));
        assert_eq!(ok.bindings[1], ("src".into(), "rs2".into()));
    }

    #[test]
    fn test_shape_check_extra_brackets() {
        // 模板 'add {dest}, {src}'：普通操作数禁止方括号。
        let resolver = AsmResolver::build(&make_model());
        let err = resolver
            .resolve("add", &["[rd]".into(), "rs1".into()])
            .unwrap_err();
        assert!(err.contains("plain operand"), "应提示禁止方括号: {err}");
    }
}
