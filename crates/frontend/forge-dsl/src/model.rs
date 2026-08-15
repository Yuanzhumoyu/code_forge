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
        // 分支语义校验：effect 声明 Branch/Jump 的指令必须有 BlockTarget 字段
        //（生成 is_branch()/branch_targets() 与 !fixup 编码都需要目标）。
        for (inst_name, inst) in &self.inst {
            let effects = inst.effect.as_deref().unwrap_or(&[]);
            let is_branch_like = effects
                .iter()
                .any(|e| e.as_str() == "Branch" || e.as_str() == "Jump");
            if is_branch_like {
                let has_bt = inst
                    .fields
                    .iter()
                    .any(|f| f.field_type == FieldType::BlockTarget);
                if !has_bt {
                    // 间接分支（目标在寄存器，如 riscv JALR）：需有寄存器目标字段。
                    let has_reg_target = inst.fields.iter().any(|f| {
                        matches!(
                            f.field_type,
                            FieldType::Ireg
                                | FieldType::GprReg
                                | FieldType::Freg
                                | FieldType::XmmReg
                        )
                    });
                    if !has_reg_target {
                        return Err(format!(
                            "[inst.{inst_name}]: effect 'Branch'/'Jump' requires a BlockTarget \
                             field (direct target + '!fixup' placeholder) or a register target \
                             field (indirect target)"
                        ));
                    }
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
                    // Inherit asm from parent unless the variant overrides it
                    let asm = var.asm.clone().unwrap_or_else(|| parent_asm.clone());
                    synthetic.push((
                        syn_name,
                        Instruction {
                            fields: var.fields.clone(),
                            encoding,
                            asm,
                            effect: var.effect.clone().or_else(|| parent_effect.clone()),
                            variants: None,
                            opcodes: None,
                            implicit: inst.implicit.clone(),
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

    /// 展开 lowering 模板规则：`template` + `conditions` → 多条具体规则。
    /// 模板中的 `$CC` 占位符被替换为每个 condition 的值，
    /// 规则名 = `{name}.{cond}`（如 `[lower.Icmp]` + `Equal` → `Icmp.Equal`）。
    /// 纯模板规则（insts 空）展开后删除。
    pub fn expand_templates(&mut self) {
        let mut synthetic: Vec<(String, LowerRule)> = Vec::new();
        for (name, rule) in &self.lower {
            if rule.template.is_empty() || rule.conditions.is_empty() {
                continue;
            }
            for (cond, val) in &rule.conditions {
                let new_name = format!("{name}.{cond}");
                let insts = rule
                    .template
                    .iter()
                    .map(|t| t.replace("$CC", val))
                    .collect();
                synthetic.push((
                    new_name,
                    LowerRule {
                        insts,
                        template: Vec::new(),
                        conditions: std::collections::BTreeMap::new(),
                        variants: Vec::new(),
                    },
                ));
            }
        }
        for (name, rule) in synthetic {
            self.lower.entry(name).or_insert(rule);
        }
        self.lower
            .retain(|_n, r| !r.insts.is_empty() || r.template.is_empty());
    }

    /// 展开 opcode 表：每条 `opcodes = [[name, opcode, mnemonic], ...]` 条目
    /// 生成一条指令——encoding 中的 `{opcode}` 与 asm 中的 `{mnemonic}`
    /// 被替换，fields/effect 继承自模板（fields 中保留的 `mnemonic` 推断
    /// 字段被过滤）。模板条目本身不生成指令。
    pub fn expand_opcodes(&mut self) {
        let mut synthetic: Vec<(String, Instruction)> = Vec::new();
        for (inst_name, inst) in &self.inst {
            let Some(ref opcodes) = inst.opcodes else {
                continue;
            };
            let template_encoding = inst.encoding.as_deref().unwrap_or("");
            for [name, opcode, mnemonic] in opcodes {
                let encoding = template_encoding.replace("{opcode}", opcode);
                let asm = inst.asm.replace("{mnemonic}", mnemonic);
                synthetic.push((
                    name.clone(),
                    Instruction {
                        // 过滤 asm 推断产生的 {mnemonic} 占位符字段
                        fields: inst
                            .fields
                            .iter()
                            .filter(|f| f.name != "mnemonic")
                            .cloned()
                            .collect(),
                        encoding: Some(encoding),
                        asm,
                        effect: inst.effect.clone(),
                        variants: None,
                        opcodes: None,
                        implicit: inst.implicit.clone(),
                    },
                ));
            }
            let _ = inst_name;
        }
        for (name, inst) in synthetic {
            self.inst.entry(name).or_insert(inst);
        }
        // 模板条目（opcodes 非空）不生成指令
        self.inst.retain(|_n, i| i.opcodes.is_none());
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
    /// Canonical ordering of GPR width-view register groups (e.g.
    /// `["gpr64", "gpr32", "gpr16", "gpr8l", "gpr8h"]` for a sub-register
    /// ISA like x86). Controls Reg-enum variant order; empty → fall back to
    /// declaration order of [reg.*] sections.
    #[serde(default)]
    pub gpr_bank_order: Vec<String>,
    /// ModRM/SIB memory addressing: physical base-register numbers that must
    /// always be encoded with a displacement (x86: RBP=5 and R13=13, where
    /// mod=00 encodes RIP-relative instead of `[base]`). Consumed by the
    /// generic `@modrm_mem` / `@lea_sib` encoding primitives.
    #[serde(default)]
    pub modrm_force_disp_base: Vec<u8>,
    /// Opcode byte of the epilogue jump (absolute rel32 form), e.g. x86
    /// `0xE9` (JMP rel32). Absent → the ISA does not override
    /// `emit_epilogue_jump` (trait default returns Unimplemented).
    #[serde(default)]
    pub epilogue_jump_opcode: Option<u8>,
    /// ISA 默认浮点值宽度（字节）——lowering 中浮点 alloc_xreg 的默认目标类
    /// 宽度。x86 = 8（f64），即使其 XMM 寄存器组是 16 字节（值宽 ≤ 寄存器宽）。
    /// 缺省 8。
    #[serde(default)]
    pub default_fpr_width: Option<u8>,
    /// 是否启用 IR 层模式融合（Stage 3 pattern isel：lea/cmp-select/fma）。
    /// 架构中立的 IR 模式（Imul+Iadd 等）对所有 ISA 通用；融合策略由各
    /// ISA 的 lowering 解释。缺省 false。
    #[serde(default)]
    pub enable_pattern_isel: Option<bool>,
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
    /// Physical register-number offset for the group's first member (default 0).
    /// e.g. x86 `gpr8h` (AH/BH/CH/DH) encodes to physical numbers 4..7 even
    /// though its group-internal indices are 0..3 — declared as `base_index = 4`.
    #[serde(default)]
    pub base_index: Option<u32>,
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
    /// 帧布局的额外栈填充（字节）：x86 = 8（align/2）——prologue push rbp +
    /// callee-saved 后 rsp%16==8（call 压入的返回地址造成），sub rsp 必须使
    /// call 前 rsp%16==0（SysV/Windows x64 ABI）。无 call 的函数多 8 字节
    /// 无害；有 call 的否则外部函数（movaps 保存）会 SEGV。
    /// 非 x86 ABI（无此约束）缺省 0。架构事实由 TOML 声明。
    #[serde(default)]
    pub frame_padding: i32,
    /// Bytes the prologue pushes *above* the frame pointer before callee-saved
    /// registers (the frame-pointer save slot): x86 `push rbp` = 8,
    /// aarch64/riscv64 `stp/sd fp,lr` = 16, wasm (no frame) = 0.
    /// Used by codegen to compute the local-variable area base offset
    /// (`rbp - fp_push_bytes - callee_saved_bytes`); was wrong before (only
    /// callee-saved bytes were counted), putting locals inside the callee-saved
    /// push slots and corrupting saved registers (mini_c SEGV).
    #[serde(default = "default_fp_push_bytes")]
    pub fp_push_bytes: u32,
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

fn default_fp_push_bytes() -> u32 {
    8
}

/// Call lowering configuration: which mov instructions to use for moving
/// call arguments into ABI registers / receiving return values, and how
/// stack-passed arguments are addressed. Every field is ISA-declared; the
/// DSL generates a generic skeleton only.
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
    /// Caller-allocated shadow space in bytes (Windows x64 = 32, most ABIs = 0).
    #[serde(default)]
    pub shadow_space: u32,
    /// Size of each stack-passed argument slot (x64 = 8).
    #[serde(default = "default_stack_slot_size")]
    pub stack_slot_size: u32,
    /// First N argument positions are passed in registers (x64 = 4).
    #[serde(default = "default_reg_arg_limit")]
    pub reg_arg_limit: u32,
    /// Instruction that stores a stack-passed argument (e.g. x86 "StoreMemR";
    /// fields: base/src + optional opsize). Only needed when stack arguments exist.
    #[serde(default)]
    pub stack_store_inst: Option<String>,
    /// Instruction that computes the address of a stack-passed argument
    /// (e.g. x86 "LeaR64Sib"; fields: dest/base/index/scale/disp).
    #[serde(default)]
    pub stack_addr_inst: Option<String>,
    /// Scratch register used to hold stack-argument addresses (e.g. x86 "R10").
    #[serde(default)]
    pub stack_addr_scratch: Option<String>,
    /// Instruction that grows the stack for shadow space + stack args
    /// (e.g. x86 "SUB64_R_IMM32"; fields: dest/imm).
    #[serde(default)]
    pub stack_alloc_inst: Option<String>,
    /// Instruction that shrinks the stack after the call (e.g. x86 "ADD64_R_IMM32").
    #[serde(default)]
    pub stack_free_inst: Option<String>,
    /// Stack pointer register (defaults to [abi.frame].sp).
    #[serde(default)]
    pub sp_reg: Option<String>,
    /// Operand width for `stack_store_inst` (x86 = 64).
    #[serde(default = "default_stack_opsize")]
    pub stack_opsize: u32,
    /// Operand width for `arg_mov` (x86 = 64).
    #[serde(default = "default_stack_opsize")]
    pub arg_opsize: u32,
    /// 被调用者入口（@move_args）配置——与调用骨架对称的收参侧。
    /// 帧指针基址寄存器（x86 "RBP"）。
    #[serde(default)]
    pub entry_fp_reg: Option<String>,
    /// 栈参数整数加载 scratch（x86 "R11"）。
    #[serde(default)]
    pub entry_load_scratch: Option<String>,
    /// 栈参数加载指令（x86 "MOV_R_MEM"；字段 dest/base/opsize）。
    #[serde(default)]
    pub entry_stack_load_inst: Option<String>,
    /// i32 收参符号扩展指令（x86 "MOVSXD_R_GPR"；字段 dest/src）。
    #[serde(default)]
    pub entry_sext_inst: Option<String>,
    /// 整数寄存器参数接收 mov（x86 "MOV_RM8_R64"；字段 dest/src/opsize）。
    #[serde(default)]
    pub entry_mov_inst: Option<String>,
    /// GPR→FP 收参 mov（x86 "MOVQ_XMM_FREG"；字段 dest/src）。
    #[serde(default)]
    pub entry_gpr_to_fp_inst: Option<String>,
    /// 返回地址占用字节数（x86 = 8），用于栈参数偏移计算。
    #[serde(default)]
    pub entry_ret_addr_bytes: u32,
}

fn default_stack_slot_size() -> u32 {
    8
}
fn default_reg_arg_limit() -> u32 {
    4
}
fn default_stack_opsize() -> u32 {
    64
}

fn default_stack_align() -> u32 {
    16
}

#[derive(Debug, Clone, Deserialize)]
pub struct AbiFrame {
    pub sp: String,
    #[serde(default)]
    pub fp: Option<String>,
    /// If true, the frame alloc instruction receives `frame_size.wrapping_neg()`
    /// instead of `frame_size`. Required for ISAs (like RISC-V) that use ADDI with
    /// a 12-bit signed immediate as their only subtract-immediate primitive.
    #[serde(default)]
    pub neg_alloc_imm: bool,
    /// Instruction that grows the stack frame (default "SUB64_R_IMM32", a
    /// legacy naming convention; any ISA may declare its own via TOML).
    #[serde(default)]
    pub alloc_inst: Option<String>,
    /// Instruction that shrinks the stack frame (default "ADD64_R_IMM32").
    #[serde(default)]
    pub free_inst: Option<String>,
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

/// 提取 asm 模板中的 `{name}` 占位符（按出现顺序）。
/// 如 `"movrr {dest}, {src}"` → `["dest", "src"]`。
pub(crate) fn asm_placeholder_names(asm: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = asm;
    while let Some(start) = rest.find('{') {
        if let Some(rel_end) = rest[start + 1..].find('}') {
            let end = start + 1 + rel_end;
            out.push(rest[start + 1..end].to_string());
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct Instruction {
    pub fields: Vec<InstField>,
    pub encoding: Option<String>,
    /// Assembly format template, e.g. `"add {dest}, {src}"`.
    /// `{field}` placeholders reference field names declared in `fields`.
    pub asm: String,
    pub effect: Option<Vec<String>>,
    /// Type-signature variants for overloaded mnemonics.
    /// Each variant defines its own field types and encoding;
    /// the asm template and effects are inherited from the parent.
    pub variants: Option<Vec<VariantEntry>>,
    /// Opcode table — expands one template into N instructions differing
    /// only by opcode/mnemonic (e.g. the SSE family). Each entry is
    /// `[name, opcode, mnemonic]`; `{opcode}` in `encoding` and `{mnemonic}`
    /// in `asm` are substituted per entry.
    pub opcodes: Option<Vec<[String; 3]>>,
    /// 指令执行时隐式破坏的物理寄存器（不在操作数里显式出现）——如 x86
    /// cqo 的 RDX（符号扩展）、@shift_reg 的 CL（计数寄存器）。TOML:
    /// `implicit = ["RDX"]`。分配器在该指令点避开（per-Inst clobbers）。
    pub implicit: Option<Vec<String>>,
}

impl<'de> serde::Deserialize<'de> for Instruction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // fields 支持三种形态（为简化 TOML 书写）：
        //   1. 内联表    `fields = { dest = "Ireg", src = "Ireg" }`（字段名 → 类型）
        //   2. 紧凑串    `fields = "Ireg Freg Opsize"`（类型列表，按 asm 占位符顺序映射字段名）
        //   3. 空/缺省   从 asm 模板 `{name}` 占位符推断字段名，类型默认 Ireg（主 GPR 类）
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum FieldsRaw {
            Inline(std::collections::BTreeMap<String, String>),
            Compact(String),
            #[allow(dead_code)]
            Empty(Vec<serde::de::IgnoredAny>),
        }

        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            #[serde(default)]
            fields: Option<FieldsRaw>,
            #[serde(default)]
            encoding: Option<String>,
            asm: String,
            #[serde(default, deserialize_with = "deser_effects")]
            effect: Option<Vec<String>>,
            #[serde(default)]
            variants: Option<Vec<VariantEntry>>,
            #[serde(default)]
            opcodes: Option<Vec<[String; 3]>>,
            #[serde(default)]
            implicit: Option<Vec<String>>,
        }

        let raw = Raw::deserialize(deserializer)?;
        // {mnemonic} 是 opcodes 表模板的保留占位符（展开时替换），不参与字段推断
        let names: Vec<String> = asm_placeholder_names(&raw.asm)
            .into_iter()
            .filter(|n| n != "mnemonic")
            .collect();
        let fields = match raw.fields {
            None | Some(FieldsRaw::Empty(_)) => names
                .into_iter()
                .map(|name| InstField {
                    name,
                    field_type: FieldType::Ireg,
                    role: None,
                })
                .collect(),
            Some(FieldsRaw::Inline(map)) => map
                .into_iter()
                .map(|(name, ty)| InstField {
                    name,
                    field_type: FieldType::from_str(&ty),
                    role: None,
                })
                .collect(),
            Some(FieldsRaw::Compact(s)) => {
                let types: Vec<&str> = s.split_whitespace().collect();
                if types.len() != names.len() {
                    return Err(serde::de::Error::custom(format!(
                        "fields string has {} types but asm template has {} placeholders \
                         (asm: {:?}, types: {:?})",
                        types.len(),
                        names.len(),
                        raw.asm,
                        types
                    )));
                }
                names
                    .into_iter()
                    .zip(types)
                    .map(|(name, ty)| InstField {
                        name,
                        field_type: FieldType::from_str(ty),
                        role: None,
                    })
                    .collect()
            }
        };
        Ok(Instruction {
            fields,
            encoding: raw.encoding,
            asm: raw.asm,
            effect: raw.effect,
            variants: raw.variants,
            opcodes: raw.opcodes,
            implicit: raw.implicit,
        })
    }
}

/// A single variant in an overloaded instruction group.
///
/// In TOML: `{ dest = "Ireg", src = "Ireg", enc = "$modrm_rr ..." }`
/// Keys `enc`, `effect`, `asm`, and `name` are special; all others are field-name→type pairs.
///
/// `enc` is optional — when omitted, the parent instruction's `encoding` is used.
/// `asm` is optional — when omitted, the parent instruction's asm template is used.
/// `name` is optional — when provided, it replaces the auto-generated type-signature suffix.
#[derive(Debug, Clone)]
pub struct VariantEntry {
    pub fields: Vec<InstField>,
    pub enc: Option<String>,
    pub asm: Option<String>,
    pub effect: Option<Vec<String>>,
    pub name: Option<String>,
}

// Custom Deserialize: extract `enc`+`effect`+`asm`+`name`, remainder → field definitions.
impl<'de> serde::Deserialize<'de> for VariantEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let map: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::deserialize(deserializer)?;
        let enc = map.get("enc").cloned();
        let asm = map.get("asm").cloned();
        let name = map.get("name").cloned();
        let effect = map
            .get("effect")
            .map(|e| e.split('+').map(|s| s.trim().to_string()).collect());
        let fields: Vec<InstField> = map
            .iter()
            .filter(|(k, _)| {
                *k != "enc" && *k != "effect" && *k != "asm" && *k != "name" && *k != "role"
            })
            .map(|(name, ty)| InstField {
                name: name.clone(),
                field_type: FieldType::from_str(ty),
                role: map.get("role").cloned(),
            })
            .collect();
        Ok(VariantEntry {
            fields,
            enc,
            asm,
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

/// 反序列化 `conditions`：接受合规的 TOML 数组形式（跨行数组）
/// `[["Equal", "e"], ["NotEqual", "ne"]]`，并兼容旧的内联表 `{ Equal = "e" }`。
fn deser_conditions<'de, D>(
    deserializer: D,
) -> Result<std::collections::BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum CondRepr {
        Map(std::collections::BTreeMap<String, String>),
        List(Vec<[String; 2]>),
    }
    Ok(match CondRepr::deserialize(deserializer)? {
        CondRepr::Map(m) => m,
        CondRepr::List(l) => l.into_iter().map(|[k, v]| (k, v)).collect(),
    })
}

/// Lowering rule — insts (assembly sequence) or template+conditions (multi-rule expansion).
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
    /// TOML: `conditions = [["Equal", "e"], ["NotEqual", "ne"]]`（数组形式，合规跨行）。
    #[serde(default, deserialize_with = "deser_conditions")]
    pub conditions: std::collections::BTreeMap<String, String>,
    /// 宽度/类型条件化变体：按操作数位宽运行时分派指令序列。
    ///
    /// 解决"规则是静态序列、无法按源宽度选择指令"的框架限制（如 Uextend
    /// 按源宽度 movzx/mov、饱和 clamp 常量 32/64 位）。
    /// `when` 为操作数宽度谓词：`"rs1<=16"` / `"rs1==32"` / `"rs1==64"` /
    /// `"rs1<32"` / `"rs1>16"` 等（位宽单位：bit）。
    #[serde(default)]
    pub variants: Vec<RuleVariant>,
}

/// 条件化 lower 规则变体（配合 `LowerRule.variants` 使用）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RuleVariant {
    /// 宽度谓词，如 `"rs1<=16"`（rs1 操作数位宽 ≤ 16）。
    #[serde(default)]
    pub when: Option<String>,
    /// 该分支的指令序列。
    #[serde(default)]
    pub insts: Vec<String>,
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

#[cfg(test)]
mod fields_tests {
    use super::*;

    fn parse_inst(toml: &str) -> Instruction {
        let src = format!("[meta]\nname = \"t\"\n[reg.gpr]\ncount = 8\nwidth = 64\n{toml}");
        let model: IsaModel = toml::from_str(&src).expect("parse");
        model.inst.values().next().expect("one inst").clone()
    }

    /// 紧凑字符串：类型列表按 asm 占位符顺序映射字段名。
    #[test]
    fn test_fields_compact_string() {
        let inst = parse_inst(
            "[inst.A]\nfields = \"Ireg Freg\"\nencoding = \"@modrm 64 0x01 dest src\"\nasm = \"a {dest}, {src}\"",
        );
        assert_eq!(inst.fields.len(), 2);
        assert_eq!(inst.fields[0].name, "dest");
        assert_eq!(inst.fields[0].field_type, FieldType::Ireg);
        assert_eq!(inst.fields[1].name, "src");
        assert_eq!(inst.fields[1].field_type, FieldType::Freg);
    }

    /// 缺省 fields：从 asm 模板占位符推断字段名，类型默认 Ireg。
    #[test]
    fn test_fields_inferred_from_asm() {
        let inst = parse_inst("[inst.B]\nencoding = \"@op_rm 64 0x01 0 x\"\nasm = \"b {x}, {y}\"");
        assert_eq!(inst.fields.len(), 2);
        assert_eq!(inst.fields[0].name, "x");
        assert_eq!(inst.fields[0].field_type, FieldType::Ireg);
        assert_eq!(inst.fields[1].name, "y");
        assert_eq!(inst.fields[1].field_type, FieldType::Ireg);
    }

    /// 内联表写法保持向后兼容。
    #[test]
    fn test_fields_table_backward_compat() {
        let inst = parse_inst(
            "[inst.C]\nfields = { dest = \"GprReg\" }\nencoding = \"{0x48:[0;8]} {0x89:[0;8]}\"\nasm = \"c {dest}\"",
        );
        assert_eq!(inst.fields.len(), 1);
        assert_eq!(inst.fields[0].name, "dest");
        assert_eq!(inst.fields[0].field_type, FieldType::GprReg);
    }

    /// 类型数与占位符数不一致应报错（避免静默错配）。
    #[test]
    fn test_fields_compact_mismatch_errors() {
        let src = "[meta]\nname = \"t\"\n[reg.gpr]\ncount = 8\nwidth = 64\n[inst.D]\nfields = \"Ireg\"\nencoding = \"@modrm 64 0x01 dest src\"\nasm = \"d {dest}, {src}\"";
        assert!(
            toml::from_str::<IsaModel>(src).is_err(),
            "类型数与占位符数不一致应报错"
        );
    }

    /// 无占位符 asm（无字段指令如 ret/nop）推断为空字段集。
    #[test]
    fn test_fields_inferred_empty() {
        let inst = parse_inst("[inst.RET]\nencoding = \"{0xC3:[0;8]}\"\nasm = \"ret\"");
        assert!(inst.fields.is_empty());
    }
}

#[cfg(test)]
mod unknown_key_tests {
    use super::*;

    /// [inst.*] 区块的未知键必须报错（deny_unknown_fields），
    /// 避免笔误（如 `fields_`/`encodng`）被静默忽略。
    #[test]
    fn test_unknown_inst_key_errors() {
        let src = "[meta]\nname = \"t\"\n[reg.gpr]\ncount = 8\nwidth = 64\n[inst.A]\nencoding = \"{0x01:[0;8]}\"\nasm = \"a\"\nencodng = \"typo\"";
        let err = toml::from_str::<IsaModel>(src).expect_err("未知键应报错");
        assert!(
            err.to_string().contains("encodng"),
            "错误信息应包含未知键名: {err}"
        );
    }
}

#[cfg(test)]
mod opcodes_tests {
    use super::*;

    /// opcodes 表展开：{opcode}/{mnemonic} 替换，fields 继承（过滤 mnemonic）。
    #[test]
    fn test_opcodes_table_expansion() {
        let src = "[meta]\nname = \"t\"\n[reg.gpr]\ncount = 8\nwidth = 64\n[inst.SSE_BIN]\nfields = \"Freg Freg\"\nencoding = \"@sse_ps_rr {opcode} dest src\"\nasm = \"{mnemonic} {dest}, {src}\"\nopcodes = [[\"ADDPS\", \"0x58\", \"addps\"], [\"SUBPS\", \"0x5C\", \"subps\"]]";
        let mut model: IsaModel = toml::from_str(src).expect("parse");
        model.expand_opcodes();

        let addps = model.inst.get("ADDPS").expect("ADDPS");
        assert_eq!(addps.encoding.as_deref(), Some("@sse_ps_rr 0x58 dest src"));
        assert_eq!(addps.asm, "addps {dest}, {src}");
        assert_eq!(addps.fields.len(), 2);
        assert_eq!(addps.fields[0].name, "dest");
        assert_eq!(addps.fields[0].field_type, FieldType::Freg);

        let subps = model.inst.get("SUBPS").expect("SUBPS");
        assert_eq!(subps.encoding.as_deref(), Some("@sse_ps_rr 0x5C dest src"));
        assert_eq!(subps.asm, "subps {dest}, {src}");

        assert!(
            !model.inst.contains_key("SSE_BIN"),
            "opcodes 模板不生成指令"
        );
    }

    /// variants 支持 asm 模板覆盖（与 enc 覆盖并列）。
    #[test]
    fn test_variant_asm_override() {
        let src = "[meta]\nname = \"t\"\n[reg.gpr]\ncount = 8\nwidth = 64\n[inst.PSH]\nfields = \"Ireg Ireg\"\nencoding = \"@op_rm 64 0x01 0 dest src\"\nasm = \"psh {dest}, {src}\"\nvariants = [{ name = \"RR\", dest = \"Ireg\", src = \"Ireg\", asm = \"pshrr {dest}, {src}\", enc = \"@op_rm 64 0x02 0 dest src\" }]";
        let mut model: IsaModel = toml::from_str(src).expect("parse");
        model.expand_variants();

        let v = model.inst.get("PSH_RR").expect("variant PSH_RR");
        assert_eq!(v.asm, "pshrr {dest}, {src}");
        assert_eq!(v.encoding.as_deref(), Some("@op_rm 64 0x02 0 dest src"));
    }
}

#[cfg(test)]
mod template_tests {
    use super::*;

    /// template + conditions 展开为多条具体规则（$CC 替换，规则名带 cond 后缀）。
    #[test]
    fn test_lower_template_expansion() {
        let src = "[meta]\nname = \"t\"\n[reg.gpr]\ncount = 8\nwidth = 64\n[inst.SET]\nfields = \"Ireg\"\nencoding = \"{0x01:[0;8]}\"\nasm = \"set$CC {r}\"\n[lower.Icmp]\ntemplate = [\"cmp rs1, rs2\", \"set$CC rd\"]\nconditions = { Equal = \"e\", NotEqual = \"ne\" }";
        let mut model: IsaModel = toml::from_str(src).expect("parse");
        model.expand_templates();

        let eq = model.lower.get("Icmp.Equal").expect("Icmp.Equal");
        assert_eq!(eq.insts, vec!["cmp rs1, rs2", "sete rd"]);
        let ne = model.lower.get("Icmp.NotEqual").expect("Icmp.NotEqual");
        assert_eq!(ne.insts, vec!["cmp rs1, rs2", "setne rd"]);
        assert!(!model.lower.contains_key("Icmp"), "纯模板规则展开后应删除");
    }
}

#[cfg(test)]
mod branch_validate_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn branch_model(has_bt: bool, has_reg: bool) -> IsaModel {
        let mut reg = BTreeMap::new();
        reg.insert(
            "gpr64".into(),
            RegGroup {
                count: 8,
                width: 64,
                names: None,
                prefix: None,
                base_index: None,
            },
        );
        let mut fields = vec![];
        if has_reg {
            fields.push(InstField {
                name: "target".into(),
                field_type: FieldType::Ireg,
                role: None,
            });
        }
        if has_bt {
            fields.push(InstField {
                name: "rel".into(),
                field_type: FieldType::BlockTarget,
                role: None,
            });
        }
        let mut inst = BTreeMap::new();
        inst.insert(
            "J".into(),
            Instruction {
                fields,
                encoding: Some("!rel4".into()),
                asm: "j .L{rel}".into(),
                effect: Some(vec!["Jump".into()]),
                variants: None,
                opcodes: None,
                implicit: None,
            },
        );
        IsaModel {
            reg,
            reg_classes: BTreeMap::new(),
            spill: BTreeMap::new(),
            meta: crate::model::Meta {
                name: "minimal".into(),
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
            inst,
            lower: BTreeMap::new(),
            lower_term: BTreeMap::new(),
            lower_pattern: BTreeMap::new(),
            emit: None,
            abi: None,
            enc_macros: BTreeMap::new(),
            enc_scatters: BTreeMap::new(),
            enc_constants: BTreeMap::new(),
            cc_names: BTreeMap::new(),
            lang_tokens: None,
            lang_keywords: None,
            lang_rules: None,
            dyn_types: BTreeMap::new(),
        }
    }

    #[test]
    fn test_branch_requires_target() {
        // 无任何目标字段 → 报错
        let err = branch_model(false, false).validate().expect_err("应报错");
        assert!(err.contains("BlockTarget"), "错误应提示目标字段: {err}");
    }

    #[test]
    fn test_direct_branch_with_blocktarget_ok() {
        // BlockTarget 字段 → 通过
        branch_model(true, false)
            .validate()
            .expect("直接分支应有 BlockTarget");
    }

    #[test]
    fn test_indirect_branch_with_reg_target_ok() {
        // Ireg 目标字段（如 riscv JALR）→ 通过
        branch_model(false, true)
            .validate()
            .expect("间接分支应有寄存器目标");
    }
}
