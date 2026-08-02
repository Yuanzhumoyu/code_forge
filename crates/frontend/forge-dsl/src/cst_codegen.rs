//! CST-based code generation using forge-grammar.
//!
//! This module replaces the hand-written V11 pipeline (asm_lexer, asm_parser,
//! asm_ast, macro_registry, macro_expander) with grammar-driven parsing via
//! the `forge-grammar` crate.
//!
//! # Key functions
//!
//! - `gen_lowering_insts_cst()` — replacement for `gen_lower_insts_v11()`
//! - `gen_emit_insts_cst()` — replacement for `gen_emit_insts_sequence_v11()`
//! - `is_v11_format()` — V11 format detection (shared with codegen.rs)

use crate::asm_resolver::{AsmResolver, OperandKind};
use crate::codegen::{
    GenMode, default_for_type, emit_arg_expr, gen_frame_alloc_free, gen_move_args, gen_push_callee,
    lowering_arg_expr, pascal_ident,
};
use crate::model::IsaModel;
use forge_grammar::{CstNode, Expr, Grammar, Parser, TokenPattern, parse_grammar};
use proc_macro2::TokenStream;
use quote::quote;
use std::collections::{BTreeMap, HashSet};

// ============================================================
// Grammar building — merge base.lx + TOML model
// (test-only: used by test_parser() helper)
// ============================================================

/// Build a complete grammar from the base.lx file and the TOML ISA model.
/// The TOML provides: tokens (IDENT, REG, ...), keywords, rules, and operand patterns.
pub fn build_grammar_from_model(model: &IsaModel) -> Result<Grammar, String> {
    let base = include_str!("../grammars/base.lx");
    let mut grammar = parse_grammar(base).map_err(|e| format!("base.lx parse error: {e}"))?;

    // 1. Add user-defined tokens from TOML [lang.tokens.*]
    if let Some(tokens) = &model.lang_tokens {
        for (name, def) in tokens {
            grammar
                .tokens
                .push(forge_grammar::TokenDef::regex(name, &def.pattern));
        }
    }

    // 2. Add keyword tokens — as IDENT-level tokens, not separate kinds.
    // Keywords match with higher priority than the general IDENT pattern.
    // They produce the same token kind ("IDENT") but match specific text first.
    if let Some(keywords) = &model.lang_keywords {
        for text in keywords.values() {
            // Insert keyword as a regex that matches exact text, before general IDENT
            // Use name like "fn" → creates a KW_FN-like matcher but produces IDENT kind
            grammar.tokens.push(forge_grammar::TokenDef {
                name: "IDENT".to_string(), // Same kind as IDENT
                pattern: forge_grammar::TokenPattern::Literal(text.clone()),
            });
        }
    }

    // 3. Add user-defined grammar rules from TOML [lang.rules]
    if let Some(rules) = &model.lang_rules {
        for (name, source) in rules {
            // Parse the rule expression from the inline string
            let mut parser = crate::grammar_rule::RuleExprParser::new(source);
            let expr = parser
                .parse_alt()
                .map_err(|e| format!("rule '{}': {}", name, e))?;
            grammar.rules.insert(
                name.clone(),
                forge_grammar::grammar::RuleDef {
                    name: name.clone(),
                    expr,
                },
            );
        }
    }

    // 4. Extend operand rule with patterns from inst.*.asm templates.
    // The base.lx operand rule already has standard token types.
    // Generated patterns add ISA-specific operand structures.
    if let Some(existing) = grammar.rules.get("operand") {
        let inst_operands = generate_operand_from_insts(&model.inst, &model.cc_names);
        // Merge: existing base alternatives + generated alternatives
        let merged = merge_operand_exprs(&existing.expr, &inst_operands);
        grammar.rules.insert(
            "operand".to_string(),
            forge_grammar::grammar::RuleDef {
                name: "operand".to_string(),
                expr: merged,
            },
        );
    }

    // 5. Add punct tokens needed by the grammar
    add_punctuation_tokens(&mut grammar);

    Ok(grammar)
}

/// Merge two operand expressions, keeping unique alternatives from both.
fn merge_operand_exprs(base: &Expr, generated: &Expr) -> Expr {
    let mut alts: Vec<Expr> = Vec::new();
    let mut seen = HashSet::new();

    // Collect from base
    if let Expr::Alt(items) = base {
        for item in items {
            let key = format!("{:?}", item);
            if seen.insert(key) {
                alts.push(item.clone());
            }
        }
    } else {
        alts.push(base.clone());
    }

    // Collect from generated (skip duplicates)
    if let Expr::Alt(items) = generated {
        for item in items {
            let key = format!("{:?}", item);
            if seen.insert(key) {
                alts.push(item.clone());
            }
        }
    } else {
        let key = format!("{:?}", generated);
        if seen.insert(key) {
            alts.push(generated.clone());
        }
    }

    Expr::alt(alts)
}

/// Generate the `operand` rule from instruction asm templates in the ISA model.
fn generate_operand_from_insts(
    insts: &BTreeMap<String, crate::model::Instruction>,
    _cc_names: &BTreeMap<String, String>,
) -> Expr {
    // Collect all operand token types from inst field types
    let mut token_types = HashSet::new();
    for inst in insts.values() {
        for field in &inst.fields {
            match field.field_type {
                crate::model::FieldType::Ireg | crate::model::FieldType::Freg => {
                    token_types.insert("IDENT");
                    token_types.insert("VREG");
                }
                crate::model::FieldType::GprReg | crate::model::FieldType::XmmReg => {
                    token_types.insert("REG");
                }
                crate::model::FieldType::I8
                | crate::model::FieldType::I16
                | crate::model::FieldType::I32
                | crate::model::FieldType::I64
                | crate::model::FieldType::U8
                | crate::model::FieldType::U16
                | crate::model::FieldType::U32
                | crate::model::FieldType::U64 => {
                    token_types.insert("DEC");
                    token_types.insert("HEX");
                    token_types.insert("IDENT");
                }
                crate::model::FieldType::BlockTarget => {
                    token_types.insert("LABEL");
                }
                crate::model::FieldType::F64 | crate::model::FieldType::F32 => {
                    token_types.insert("FLOAT");
                }
                crate::model::FieldType::MemRef => {
                    token_types.insert("LBRACKET");
                    token_types.insert("IDENT");
                    token_types.insert("REG");
                    token_types.insert("DEC");
                }
                crate::model::FieldType::CondCode => {} // expanded in mnemonic, not an operand
                crate::model::FieldType::Opsize => {}   // implicit, not an operand
            }
        }
    }

    // Build a union operand rule from collected token types
    let mut alts: Vec<Expr> = Vec::new();
    for t in &[
        "IDENT", "REG", "TEMP", "LABEL", "VREG", "HEX", "BIN", "OCT", "DEC", "FLOAT", "STRING",
    ] {
        if token_types.contains(t)
            || *t == "TEMP"
            || *t == "BIN"
            || *t == "OCT"
            || *t == "STRING"
            || *t == "LABEL"
            || *t == "FLOAT"
        {
            alts.push(Expr::Token(t.to_string()));
        }
    }
    // Also add parenthesized and bracket expressions
    alts.push(Expr::seq(vec![
        Expr::Lit("(".to_string()),
        Expr::Rule("expr".to_string()),
        Expr::Lit(")".to_string()),
    ]));
    alts.push(Expr::seq(vec![
        Expr::Lit("[".to_string()),
        Expr::Rule("expr".to_string()),
        Expr::Lit("]".to_string()),
    ]));
    // 支持 "[SP, #-16]!" 风格（base 与立即数用逗号分隔，带可选的 ! 标记）
    // 负位移：lexer 把 "#-16" 切成 HASH + MINUS + DEC——需显式 MINUS 替代。
    alts.push(Expr::seq(vec![
        Expr::Lit("[".to_string()),
        Expr::Rule("operand".to_string()),
        Expr::Lit(",".to_string()),
        Expr::Lit("#".to_string()),
        Expr::Token("DEC".to_string()),
        Expr::Lit("]".to_string()),
        Expr::Lit("!".to_string()),
    ]));
    alts.push(Expr::seq(vec![
        Expr::Lit("[".to_string()),
        Expr::Rule("operand".to_string()),
        Expr::Lit(",".to_string()),
        Expr::Lit("#".to_string()),
        Expr::Token("DEC".to_string()),
        Expr::Lit("]".to_string()),
    ]));
    alts.push(Expr::seq(vec![
        Expr::Lit("[".to_string()),
        Expr::Rule("operand".to_string()),
        Expr::Lit(",".to_string()),
        Expr::Lit("#".to_string()),
        Expr::Token("MINUS".to_string()),
        Expr::Token("DEC".to_string()),
        Expr::Lit("]".to_string()),
        Expr::Lit("!".to_string()),
    ]));
    alts.push(Expr::seq(vec![
        Expr::Lit("[".to_string()),
        Expr::Rule("operand".to_string()),
        Expr::Lit(",".to_string()),
        Expr::Lit("#".to_string()),
        Expr::Token("MINUS".to_string()),
        Expr::Token("DEC".to_string()),
        Expr::Lit("]".to_string()),
    ]));
    // 独立立即数操作数（epilogue 的 "[SP], #16" 是单独 operand）
    alts.push(Expr::seq(vec![
        Expr::Lit("#".to_string()),
        Expr::Token("DEC".to_string()),
    ]));
    alts.push(Expr::seq(vec![
        Expr::Lit("#".to_string()),
        Expr::Token("MINUS".to_string()),
        Expr::Token("DEC".to_string()),
    ]));

    Expr::alt(alts)
}

/// Add standard punctuation tokens to the grammar.
fn add_punctuation_tokens(grammar: &mut Grammar) {
    let puncts = [
        ("LPAREN", "("),
        ("RPAREN", ")"),
        ("LBRACE", "{"),
        ("RBRACE", "}"),
        ("LBRACKET", "["),
        ("RBRACKET", "]"),
        ("COMMA", ","),
        ("COLON", ":"),
        ("SEMI", ";"),
        ("EQ", "="),
        ("AT", "@"),
        ("DOT", "."),
        ("PLUS", "+"),
        ("MINUS", "-"),
        ("STAR", "*"),
        ("SLASH", "/"),
        ("PERCENT", "%"),
        ("LT", "<"),
        ("GT", ">"),
        ("LE", "<="),
        ("GE", ">="),
        ("EQEQ", "=="),
        ("NE", "!="),
        ("AND", "&&"),
        ("OR", "||"),
        ("NOT", "!"),
        ("TILDE", "~"),
        ("HASH", "#"),
    ];
    for (name, lit) in &puncts {
        // Check if not already present
        if !grammar.tokens.iter().any(|t| t.name == *name) {
            grammar.tokens.insert(
                0,
                forge_grammar::TokenDef {
                    name: name.to_string(),
                    pattern: TokenPattern::Literal(lit.to_string()),
                },
            );
        }
    }
    grammar
        .tokens
        .sort_by(|a, b| match (&a.pattern, &b.pattern) {
            (TokenPattern::Literal(la), TokenPattern::Literal(lb)) => lb.len().cmp(&la.len()),
            (TokenPattern::Literal(_), TokenPattern::Regex(_)) => std::cmp::Ordering::Less,
            _ => std::cmp::Ordering::Equal,
        });
}

// ============================================================
// CST query helpers
// ============================================================

/// Extract the mnemonic (first IDENT) from an inst_like CST node.
pub fn get_mnemonic(node: &CstNode) -> Option<String> {
    node.leaves()
        .iter()
        .find(|t| t.kind == "IDENT")
        .map(|t| t.text.clone())
}

/// Extract operand texts from nested `operand` CST nodes within an inst_like.
///
/// Structured operands (mem_ref, paren_ref, imm_prefix) are returned as full text:
/// - `mem_ref`: `[rs1+rs2*1]`
/// - `paren_ref`: `8(sp)`
/// - `imm_prefix`: `#42`
pub fn get_operand_texts(node: &CstNode) -> Vec<String> {
    let mut operands = Vec::new();
    for n in node.walk() {
        if n.kind == "operand" {
            let leaves = n.leaves();
            // Check for structured operand types
            let has_struct = leaves
                .iter()
                .any(|t| matches!(t.kind.as_str(), "LBRACKET" | "RBRACKET"))
                || n.children
                    .iter()
                    .any(|c| matches!(c.kind.as_str(), "mem_ref" | "paren_ref" | "imm_prefix"));

            if has_struct {
                // Structured operand: concatenate all leaf texts
                let text: String = leaves
                    .iter()
                    .map(|t| t.text.as_str())
                    .collect::<Vec<_>>()
                    .join("");
                operands.push(text);
            } else {
                // Regular operand: get first meaningful token
                if let Some(leaf) = leaves
                    .into_iter()
                    .find(|t| t.kind != "LPAREN" && t.kind != "RPAREN")
                {
                    operands.push(leaf.text.clone());
                }
            }
        }
    }
    operands
}

/// Classify an operand token kind string to the resolver's OperandKind.
pub fn classify_operand(token_kind: &str) -> OperandKind {
    match token_kind {
        "IDENT" => OperandKind::Ident,
        "REG" => OperandKind::Reg,
        "TEMP" => OperandKind::Temp,
        "VREG" => OperandKind::VReg,
        "LABEL" => OperandKind::Label,
        "DEC" | "HEX" | "BIN" | "OCT" | "FLOAT" | "STRING" => OperandKind::Literal,
        _ => OperandKind::Ident,
    }
}

/// Extract operand kinds from nested `operand` CST nodes.
pub fn get_operand_kinds(node: &CstNode) -> Vec<OperandKind> {
    let mut kinds = Vec::new();
    for n in node.walk() {
        if n.kind == "operand" {
            let leaves = n.leaves();
            if let Some(leaf) = leaves
                .into_iter()
                .find(|t| t.kind != "LPAREN" && t.kind != "RPAREN")
            {
                kinds.push(classify_operand(&leaf.kind));
            }
        }
    }
    kinds
}

#[cfg(test)]
/// Check if a line is a pseudo-call (`@name`).
pub fn is_pseudo_call(line: &str) -> bool {
    line.trim().starts_with('@')
}

#[cfg(test)]
/// Check if a line is a label definition (`.name:`).
pub fn is_label_def(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('.') && trimmed.ends_with(':')
}

/// Check if a line is a `.if` directive.
fn is_if_directive(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with(".if ") && !trimmed.ends_with(':')
}

/// Check if a line is an `.else` directive.
fn is_else_directive(line: &str) -> bool {
    line.trim() == ".else"
}

/// Check if a line is an `.endif` directive.
fn is_endif_directive(line: &str) -> bool {
    line.trim() == ".endif"
}

/// Parse a `.if` condition and return a TokenStream for the runtime check.
///
/// Supported conditions:
/// - `<operand>_is_float` → `ctx.is_float_vreg(<operand>)`
/// - `<operand>_is_int` → `!ctx.is_float_vreg(<operand>)`
/// - `is_float_return` → `ctx.is_float_return`
/// - `has_frame` → `frame_size > 0`
fn parse_if_condition(cond_str: &str) -> Option<TokenStream> {
    let cond = cond_str.trim();
    // `<operand>_is_float` — check if a VReg is a float type
    if let Some(operand) = cond.strip_suffix("_is_float") {
        let op_ident = quote::format_ident!("{operand}");
        return Some(quote! { #op_ident.class().is_fp() });
    }
    // `<operand>_is_int` — check if a VReg is an integer type
    if let Some(operand) = cond.strip_suffix("_is_int") {
        let op_ident = quote::format_ident!("{operand}");
        return Some(quote! { !#op_ident.class().is_fp() });
    }
    // `is_float_return` — check if the function returns a float
    if cond == "is_float_return" {
        return Some(quote! { ctx.is_float_return });
    }
    // `has_frame` — check if there are stack frame allocations
    if cond == "has_frame" {
        return Some(quote! { frame_size > 0 });
    }
    None
}

/// State for conditional assembly parsing.
enum IfState {
    Normal,
    InIf {
        /// The condition TokenStream
        cond: TokenStream,
        /// Instructions collected in the `.if` block
        true_insts: Vec<TokenStream>,
    },
    InElse {
        /// The condition TokenStream
        cond: TokenStream,
        /// Instructions collected in the `.if` block
        true_insts: Vec<TokenStream>,
        /// Instructions collected in the `.else` block
        false_insts: Vec<TokenStream>,
    },
}

// ============================================================
// V11 format detection
// ============================================================

/// Check if a sequence of instruction lines uses V11 assembly format.
/// V11 format: first instruction line starts with lowercase (mnemonic) or '@' (pseudo).
pub fn is_v11_format(insts: &[String]) -> bool {
    if let Some(first) = insts.first() {
        let trimmed = first.trim();
        if trimmed.is_empty() {
            return false;
        }
        let first_char = trimmed.chars().next().unwrap();
        first_char.is_ascii_lowercase() || first_char == '@'
    } else {
        false
    }
}

// ============================================================
// Pseudo-directive handling
// ============================================================

/// Handle a pseudo-call: `@push_callee`, `@move_args`, etc.
fn gen_pseudo(name: &str, model: &IsaModel) -> Result<Vec<TokenStream>, String> {
    match name {
        "push_callee" => Ok(vec![gen_push_callee(model, true)]),
        "pop_callee" => Ok(vec![gen_push_callee(model, false)]),
        "move_args" => Ok(vec![gen_move_args(model)]),
        "frame_alloc" => Ok(vec![gen_frame_alloc_free(model, true)]),
        "frame_free" => Ok(vec![gen_frame_alloc_free(model, false)]),
        _ => Err(format!("unknown pseudo-directive '@{name}'")),
    }
}

// ============================================================
// CST-based instruction resolution
// ============================================================

/// Resolve a CST inst_like node and generate `Inst::Name { fields }` TokenStream.
fn gen_inst_from_cst(
    node: &CstNode,
    resolver: &AsmResolver,
    model: &IsaModel,
    mode: GenMode,
) -> Result<TokenStream, String> {
    let mnemonic = get_mnemonic(node).ok_or_else(|| "no mnemonic in inst_like".to_string())?;
    let operand_texts = get_operand_texts(node);
    let _operand_kinds = get_operand_kinds(node);

    let resolved = resolver
        .resolve(&mnemonic, &operand_texts)
        .map_err(|e| format!("cannot resolve '{}': {e}", mnemonic))?;

    let inst_def = model
        .inst
        .get(&resolved.inst_name)
        .ok_or_else(|| format!("instruction '{}' not found", resolved.inst_name))?;
    let ivn = pascal_ident(&resolved.inst_name);

    let mut field_exprs: Vec<TokenStream> = Vec::new();
    let scratch = model.abi.as_ref().map(|a| &a.scratch);

    // Lowering 模式：记录 Ireg/Freg 字段的 XReg → (指令索引, 字段顺序索引) 映射
    let mut map_calls: Vec<TokenStream> = Vec::new();

    for (field_name, arg_val) in &resolved.bindings {
        let field = inst_def
            .fields
            .iter()
            .find(|f| &f.name == field_name)
            .ok_or_else(|| format!("field '{field_name}' not found in '{}'", resolved.inst_name))?;
        let fi = quote::format_ident!("{field_name}");
        let expr = match mode {
            GenMode::Lowering => {
                lowering_arg_expr(arg_val, &field.field_type, scratch, Some(model))
            }
            GenMode::Emit => emit_arg_expr(arg_val, &field.field_type, model),
        };
        // Lowering 模式：寄存器字段（Ireg/Freg）字段以默认物理 Reg 占位，
        // XReg 表达式记录到指令包 xreg_map（分配器分配后回填真正寄存器）
        if matches!(mode, GenMode::Lowering)
            && matches!(
                field.field_type,
                crate::model::FieldType::Ireg | crate::model::FieldType::Freg
            )
        {
            let xreg_expr = lowering_arg_expr(arg_val, &field.field_type, scratch, Some(model));
            map_calls.push(quote! {
                { let _: crate::prelude::XReg = #xreg_expr; __pack.map_reg_field(#xreg_expr, __idx); }
            });
            let cls = if matches!(field.field_type, crate::model::FieldType::Freg) {
                quote! { crate::prelude::RegClass::FPR }
            } else {
                quote! { crate::prelude::RegClass::GPR }
            };
            field_exprs.push(quote! { #fi: { let __v: Reg = <Reg as forge_ir::PhysReg>::from_index(0, #cls); __v } });
        } else {
            field_exprs.push(quote! { #fi: #expr });
        }
    }

    // Fill missing fields with defaults
    for field in &inst_def.fields {
        if !resolved.bindings.iter().any(|(n, _)| *n == field.name) {
            let fi = quote::format_ident!("{}", field.name);
            let default = default_for_type(&field.field_type);
            field_exprs.push(quote! { #fi: #default });
        }
    }

    // Lowering 模式：推入指令包并记录寄存器字段映射（供分配器分配后回填真正寄存器）
    if matches!(mode, GenMode::Lowering) {
        return Ok(quote! {
            {
                let __idx = __pack.push_inst(Inst::#ivn { #(#field_exprs),* });
                #(#map_calls)*
            }
        });
    }

    Ok(quote! { Inst::#ivn { #(#field_exprs),* } })
}

// ============================================================
// Main lowering/emit functions
// ============================================================

/// Result of lowering instruction generation — includes temp VReg declarations
/// and the actual instruction expressions.
pub struct LoweringInsts {
    /// Temp VReg declarations: `let __vreg_<name> = ctx.alloc_vreg();`
    pub temps: Vec<TokenStream>,
    /// Instruction expressions: `Inst::Name { field: val, ... }`
    pub insts: Vec<TokenStream>,
}

/// Scan lowering instruction strings for `%name` temporary VReg references.
/// Returns unique names in sorted order (deterministic).
fn collect_temp_names(insts: &[String]) -> Vec<String> {
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for line in insts {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('@') {
            continue;
        }
        // Split on whitespace and commas to find operand tokens
        for part in trimmed.split(|c: char| c.is_whitespace() || c == ',') {
            let part = part.trim();
            if let Some(name) = part.strip_prefix('%')
                && !name.is_empty()
                && name.chars().all(|c| c.is_alphanumeric() || c == '_')
            {
                names.insert(name.to_string());
            }
        }
    }
    names.into_iter().collect()
}

/// Generate lowering instruction TokenStreams from V11-format assembly strings.
///
/// This is the CST-based replacement for `gen_lower_insts_v11()`.
/// Uses dynamic grammar built from base.lx + TOML model.
pub fn gen_lowering_insts_cst(insts: &[String], model: &IsaModel) -> Result<LoweringInsts, String> {
    let grammar =
        build_grammar_from_model(model).map_err(|e| format!("grammar build error: {e}"))?;
    let parser = Parser::build(grammar);
    let resolver = AsmResolver::build(model);

    // Collect %name temporary VReg references
    let temp_names = collect_temp_names(insts);
    let mut temps: Vec<TokenStream> = Vec::new();
    for name in &temp_names {
        let vi = quote::format_ident!("__vreg_{name}");
        temps.push(quote! { let #vi = ctx.alloc_xreg(crate::prelude::RegClass::GPR); });
    }

    let mut result = Vec::new();
    let mut if_state = IfState::Normal;

    // Helper: push TokenStream(s) to the correct buffer based on conditional state.
    let push_tokens =
        |state: &mut IfState, toks: Vec<TokenStream>, result: &mut Vec<TokenStream>| match state {
            IfState::Normal => result.extend(toks),
            IfState::InIf { true_insts, .. } => true_insts.extend(toks),
            IfState::InElse { false_insts, .. } => false_insts.extend(toks),
        };

    for line in insts {
        let trimmed = line.trim();

        // Handle .if / .else / .endif directives (conditional assembly)
        if is_if_directive(trimmed) {
            // Extract condition after ".if "
            let cond_str = trimmed[4..].trim();
            let cond = parse_if_condition(cond_str).ok_or_else(|| {
                format!("invalid .if condition: '{cond_str}' — supported: <operand>_is_float, <operand>_is_int, is_float_return, has_frame")
            })?;
            if_state = IfState::InIf {
                cond,
                true_insts: Vec::new(),
            };
            continue;
        }
        if is_else_directive(trimmed) {
            match if_state {
                IfState::InIf { cond, true_insts } => {
                    if_state = IfState::InElse {
                        cond,
                        true_insts,
                        false_insts: Vec::new(),
                    };
                }
                _ => return Err(".else without matching .if".into()),
            }
            continue;
        }
        if is_endif_directive(trimmed) {
            match if_state {
                IfState::Normal => return Err(".endif without matching .if".into()),
                IfState::InIf { cond, true_insts } => {
                    result.push(quote! { if #cond { #(#true_insts);* } });
                    if_state = IfState::Normal;
                }
                IfState::InElse {
                    cond,
                    true_insts,
                    false_insts,
                } => {
                    result
                        .push(quote! { if #cond { #(#true_insts);* } else { #(#false_insts);* } });
                    if_state = IfState::Normal;
                }
            }
            continue;
        }

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Handle pseudo-calls
        if let Some(name) = trimmed.strip_prefix('@') {
            let toks = gen_pseudo(name, model)?;
            push_tokens(&mut if_state, toks, &mut result);
            continue;
        }

        // Handle label definitions
        if trimmed.ends_with(':') {
            let label = trimmed.trim_end_matches(':');
            push_tokens(
                &mut if_state,
                vec![quote! { /* label .{} */ let _ = #label; }],
                &mut result,
            );
            continue;
        }

        // Uppercase mnemonic = V10 legacy: use direct key lookup (BTreeMap field order).
        // Lowercase mnemonic = V11 asm format: use AsmResolver (template field order).
        let first_char = trimmed.chars().next().unwrap_or('x');
        if first_char.is_ascii_uppercase() {
            // Direct key lookup — split "LEA_R64_SIB op1, op2, ..."
            let (key, rest) = match trimmed.find(char::is_whitespace) {
                Some(pos) => (trimmed[..pos].trim(), trimmed[pos..].trim()),
                None => (trimmed, ""),
            };
            let operands: Vec<&str> = if rest.is_empty() {
                Vec::new()
            } else {
                rest.split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect()
            };
            let inst_def = model
                .inst
                .get(key)
                .ok_or_else(|| format!("instruction '{}' not found in [inst.*]", key))?;
            let ivn = pascal_ident(key);
            let mut field_exprs: Vec<TokenStream> = Vec::new();
            let scratch = model.abi.as_ref().map(|a| &a.scratch);
            // Field order: follow the asm template's placeholder order (TOML
            // definition order), NOT inst_def.fields — serde parses the inline
            // table into an alphabetized Vec, which corrupts positional binding
            // for immediate fields (e.g. riscv64 `addi rd, rs1, imm`).
            let field_tuples: Vec<(String, crate::model::FieldType)> = inst_def
                .fields
                .iter()
                .map(|f| (f.name.clone(), f.field_type.clone()))
                .collect();
            let template_order =
                crate::asm_resolver::ParsedTemplate::parse(&inst_def.asm, &field_tuples)
                    .field_order();
            let mut bound: Vec<String> = Vec::new();
            let mut op_idx = 0;
            let mut reg_map_calls: Vec<TokenStream> = Vec::new();
            for (field_name, field_type) in &template_order {
                if matches!(field_type, crate::model::FieldType::Opsize) {
                    continue;
                }
                let arg_val = if op_idx < operands.len() {
                    operands[op_idx]
                } else {
                    return Err(format!(
                        "missing operand for field '{field_name}' in '{trimmed}'"
                    ));
                };
                op_idx += 1;
                let fi = quote::format_ident!("{}", field_name);
                bound.push(field_name.clone());
                // 寄存器字段（Ireg/Freg）以默认物理 Reg 占位，XReg 记录到 xreg_map
                if matches!(
                    field_type,
                    crate::model::FieldType::Ireg | crate::model::FieldType::Freg
                ) {
                    let xreg_expr = lowering_arg_expr(arg_val, field_type, scratch, Some(model));
                    reg_map_calls.push(quote! {
                        __pack.map_reg_field(#xreg_expr, __idx);
                    });
                    let cls = if matches!(field_type, crate::model::FieldType::Freg) {
                        quote! { crate::prelude::RegClass::FPR }
                    } else {
                        quote! { crate::prelude::RegClass::GPR }
                    };
                    field_exprs
                        .push(quote! { #fi: <Reg as forge_ir::PhysReg>::from_index(0, #cls) });
                } else {
                    let expr = lowering_arg_expr(arg_val, field_type, scratch, Some(model));
                    field_exprs.push(quote! { #fi: #expr });
                }
            }
            for field in &inst_def.fields {
                if !bound.contains(&field.name) {
                    let fi = quote::format_ident!("{}", field.name);
                    let default = default_for_type(&field.field_type);
                    field_exprs.push(quote! { #fi: #default });
                }
            }
            // 记录寄存器字段的 XReg → (指令索引, 字段顺序) 映射（已在模板顺序循环中收集）
            let push_block = quote! {
                {
                    let __idx = __pack.push_inst(Inst::#ivn { #(#field_exprs),* });
                    #(#reg_map_calls)*
                }
            };
            push_tokens(&mut if_state, vec![push_block], &mut result);
            continue;
        }

        // Parse and resolve via CST + AsmResolver (V11 asm format)
        let cst = parser
            .parse_rule(trimmed, "inst_like")
            .map_err(|e| format!("parse error in '{trimmed}': {e}"))?;

        let inst_tok = gen_inst_from_cst(&cst, &resolver, model, GenMode::Lowering)
            .map_err(|e| format!("in '{trimmed}': {e}"))?;
        push_tokens(&mut if_state, vec![inst_tok], &mut result);
    }

    // Check for unclosed .if at end of insts
    if !matches!(if_state, IfState::Normal) {
        return Err("unclosed .if directive at end of lowering sequence".into());
    }

    Ok(LoweringInsts {
        temps,
        insts: result,
    })
}

/// Generate emit TokenStream from V11-format assembly strings.
///
/// This is the CST-based replacement for `gen_emit_insts_sequence_v11()`.
/// Used for prologue/epilogue emit sequences.
pub fn gen_emit_insts_cst(insts: &[String], model: &IsaModel) -> TokenStream {
    let parser = match build_grammar_from_model(model) {
        Ok(g) => Parser::build(g),
        Err(_) => return quote! { let _ = (frame_size, rm, sink); },
    };
    let resolver = AsmResolver::build(model);

    let mut all_stmts: Vec<TokenStream> = Vec::new();
    let mut pending: Vec<TokenStream> = Vec::new();

    let flush = |pending: &mut Vec<TokenStream>, stmts: &mut Vec<TokenStream>| {
        if !pending.is_empty() {
            stmts.push(quote::quote! {
                for __inst in &[#(#pending),*] {
                    if let Err(e) = emit_inst(__inst, rm, &mut *sink) {
                        return Err(e);
                    }
                }
            });
            pending.clear();
        }
    };

    for line in insts {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Handle pseudo-calls
        if let Some(name) = trimmed.strip_prefix('@') {
            flush(&mut pending, &mut all_stmts);
            match name {
                "push_callee" => all_stmts.push(gen_push_callee(model, true)),
                "pop_callee" => all_stmts.push(gen_push_callee(model, false)),
                "move_args" => all_stmts.push(gen_move_args(model)),
                "frame_alloc" => all_stmts.push(gen_frame_alloc_free(model, true)),
                "frame_free" => all_stmts.push(gen_frame_alloc_free(model, false)),
                _ => {}
            }
            continue;
        }

        // Parse via CST
        if let Ok(cst) = parser.parse_rule(trimmed, "inst_like")
            && let Ok(inst_tok) = gen_inst_from_cst(&cst, &resolver, model, GenMode::Emit)
        {
            pending.push(inst_tok);
        }
    }

    flush(&mut pending, &mut all_stmts);
    if all_stmts.is_empty() {
        quote::quote! { let _ = (frame_size, rm, sink); }
    } else {
        quote::quote! { #(#all_stmts)* }
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_grammar() {
        let grammar = build_grammar_from_model(&test_model()).unwrap();
        assert!(grammar.rules.contains_key("inst_like"));
        assert!(grammar.rules.contains_key("operand"));
    }

    #[test]
    fn parse_stp_preindex() {
        let model = test_model();
        let grammar = build_grammar_from_model(&model).unwrap();
        let parser = Parser::build(grammar);
        let grammar2 = build_grammar_from_model(&test_model()).unwrap();
        let hashes: Vec<_> = grammar2
            .tokens
            .iter()
            .filter(|t| t.name == "HASH")
            .collect();
        eprintln!("grammar HASH tokens: {}", hashes.len());
        let mut lx = forge_grammar::Lexer::build(&grammar2);
        let toks = lx.tokenize("[SP, #-16]!").expect("lex");
        eprintln!(
            "lex tokens: {:?}",
            toks.iter().map(|t| &t.kind).collect::<Vec<_>>()
        );
        let cst = parser
            .parse_rule("stp X29, X30, [SP, #-16]!", "inst_like")
            .expect("stp should parse");
        let texts = get_operand_texts(&cst);
        eprintln!("operand texts: {texts:?}");
        assert!(texts.iter().any(|t| t.contains("[SP")), "got {texts:?}");
    }

    #[test]
    fn test_parse_simple_inst() {
        let parser = test_parser();
        let cst = parser.parse_rule("mov rd, rs1", "inst_like").unwrap();
        let texts = get_operand_texts(&cst);
        assert_eq!(texts, vec!["rd", "rs1"]);
    }

    #[test]
    fn test_parse_with_reg() {
        let parser = test_parser();
        let cst = parser.parse_rule("push RBP", "inst_like").unwrap();
        let texts = get_operand_texts(&cst);
        assert_eq!(texts, vec!["RBP"]);
    }

    #[test]
    fn test_parse_with_immediate() {
        let parser = test_parser();
        let cst = parser.parse_rule("mov RAX, 42", "inst_like").unwrap();
        let texts = get_operand_texts(&cst);
        assert_eq!(texts, vec!["RAX", "42"]);
    }

    #[test]
    fn test_is_pseudo() {
        assert!(is_pseudo_call("@move_args"));
        assert!(!is_pseudo_call("mov rd, rs1"));
    }

    #[test]
    fn test_is_label() {
        assert!(is_label_def(".L0:"));
        assert!(!is_label_def("mov rd, rs1"));
    }

    #[test]
    fn test_is_v11_format() {
        assert!(is_v11_format(&["mov rd, rs1".into()]));
        assert!(is_v11_format(&["@move_args".into()]));
        assert!(!is_v11_format(&["MOV_R8_RM rd, rs1".into()]));
    }

    // ============================================================
    // Test helpers
    // ============================================================

    /// Build a minimal ISA model for testing the grammar.
    /// Includes keywords for syntax instructions and micro-instructions for resolver tests.
    fn test_model() -> IsaModel {
        let mut model = IsaModel {
            reg_classes: BTreeMap::new(),
            spill: BTreeMap::new(),
            meta: crate::model::Meta {
                name: "test".into(),
                version: "1".into(),
                endian: "little".into(),
                mode: 64,
                max_inst_len: 15,
                capabilities: Default::default(),
                no_default_lowering: true,
                no_epilogue_label: false,
            },
            reg: {
                let mut m = BTreeMap::new();
                m.insert(
                    "gpr".into(),
                    crate::model::RegGroup {
                        count: 16,
                        width: 64,
                        names: None,
                        prefix: None,
                    },
                );
                m
            },
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
            dyn_types: BTreeMap::new(),
            lang_tokens: {
                let mut t = BTreeMap::new();
                t.insert(
                    "IDENT".into(),
                    crate::model::LangTokenDef {
                        pattern: "[a-z_][a-zA-Z0-9_]*".into(),
                    },
                );
                t.insert(
                    "REG".into(),
                    crate::model::LangTokenDef {
                        pattern: "[A-Z][A-Z0-9]*".into(),
                    },
                );
                t.insert(
                    "TEMP".into(),
                    crate::model::LangTokenDef {
                        pattern: "%[a-zA-Z_][a-zA-Z0-9_]*".into(),
                    },
                );
                t.insert(
                    "LABEL".into(),
                    crate::model::LangTokenDef {
                        pattern: "\\.[a-zA-Z_][a-zA-Z0-9_]*".into(),
                    },
                );
                t.insert(
                    "VREG".into(),
                    crate::model::LangTokenDef {
                        pattern: "VReg\\([0-9]+\\)".into(),
                    },
                );
                Some(t)
            },
            lang_keywords: {
                let mut k = BTreeMap::new();
                k.insert("fn".into(), "fn".into());
                k.insert("if".into(), "if".into());
                k.insert("else".into(), "else".into());
                k.insert("while".into(), "while".into());
                Some(k)
            },
            lang_rules: None,
        };
        // ── micro-instructions (for CST parse + resolver tests) ──
        fn vreg(n: &str) -> crate::model::InstField {
            crate::model::InstField {
                name: n.into(),
                field_type: crate::model::FieldType::Ireg,
                role: None,
            }
        }
        fn reg_field(n: &str) -> crate::model::InstField {
            crate::model::InstField {
                name: n.into(),
                field_type: crate::model::FieldType::GprReg,
                role: None,
            }
        }
        fn blk(n: &str) -> crate::model::InstField {
            crate::model::InstField {
                name: n.into(),
                field_type: crate::model::FieldType::BlockTarget,
                role: None,
            }
        }

        model.inst.insert(
            "ADD".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "add {dest}, {src}".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "MOV".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "mov {dest}, {src}".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "PUSH".into(),
            crate::model::Instruction {
                fields: vec![reg_field("reg")],
                encoding: None,
                asm: "push {reg}".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "RET".into(),
            crate::model::Instruction {
                fields: vec![],
                encoding: None,
                asm: "ret".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "JMP".into(),
            crate::model::Instruction {
                fields: vec![blk("rel")],
                encoding: None,
                asm: "jmp {rel}".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "JE".into(),
            crate::model::Instruction {
                fields: vec![
                    crate::model::InstField {
                        name: "cond".into(),
                        field_type: crate::model::FieldType::CondCode,
                        role: None,
                    },
                    blk("rel"),
                ],
                encoding: None,
                asm: "j{cond} {rel}".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "SETE".into(),
            crate::model::Instruction {
                fields: vec![
                    crate::model::InstField {
                        name: "cond".into(),
                        field_type: crate::model::FieldType::CondCode,
                        role: None,
                    },
                    vreg("dest"),
                ],
                encoding: None,
                asm: "set{cond} {dest}".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "XOR".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "xor {dest}, {src}".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "SHL".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "shl {dest}, {src}".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "SHR".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "shr {dest}, {src}".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "SAR".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "sar {dest}, {src}".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "IMUL".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "imul {dest}, {src}".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "LEA".into(),
            crate::model::Instruction {
                fields: vec![
                    vreg("dest"),
                    vreg("base"),
                    vreg("index"),
                    crate::model::InstField {
                        name: "scale".into(),
                        field_type: crate::model::FieldType::U8,
                        role: None,
                    },
                    crate::model::InstField {
                        name: "disp".into(),
                        field_type: crate::model::FieldType::I64,
                        role: None,
                    },
                ],
                encoding: None,
                asm: "lea {dest}, [{base}+{index}*{scale}]".into(),
                variants: None,
                effect: None,
            },
        );
        model.inst.insert(
            "MOVSD".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "movsd {dest}, {src}".into(),
                variants: None,
                effect: None,
            },
        );
        // Add cc_names for JE/SETE CondCode expansion
        model.cc_names.insert("e".into(), "0x94".into());
        model.cc_names.insert("ne".into(), "0x95".into());
        model
    }

    fn test_parser() -> Parser {
        let grammar = build_grammar_from_model(&test_model()).unwrap();
        Parser::build(grammar)
    }

    fn parse_inst(source: &str) -> (String, Vec<String>) {
        let parser = test_parser();
        let cst = parser.parse_rule(source, "inst_like").unwrap();
        let mnemonic = get_mnemonic(&cst).unwrap();
        let operands = get_operand_texts(&cst);
        (mnemonic, operands)
    }

    // ============================================================
    // 微指令测试 (M1-M14)
    // ============================================================

    #[test]
    fn test_m1_simple_two_operand() {
        let (mnemonic, operands) = parse_inst("add rd, rs1");
        assert_eq!(mnemonic, "add");
        assert_eq!(operands, vec!["rd", "rs1"]);
    }

    #[test]
    fn test_m2_zero_operand() {
        let (mnemonic, operands) = parse_inst("ret");
        assert_eq!(mnemonic, "ret");
        assert!(operands.is_empty());
    }

    #[test]
    fn test_m3_one_operand() {
        let (mnemonic, operands) = parse_inst("push RBP");
        assert_eq!(mnemonic, "push");
        assert_eq!(operands, vec!["RBP"]);
    }

    #[test]
    fn test_m4_register_operands() {
        let (mnemonic, operands) = parse_inst("mov RAX, RBX");
        assert_eq!(mnemonic, "mov");
        assert_eq!(operands, vec!["RAX", "RBX"]);
    }

    #[test]
    fn test_m5_immediate() {
        let (mnemonic, operands) = parse_inst("mov RAX, 42");
        assert_eq!(mnemonic, "mov");
        assert_eq!(operands, vec!["RAX", "42"]);
    }

    #[test]
    fn test_m6_hex_immediate() {
        let (mnemonic, operands) = parse_inst("mov RAX, 0x2A");
        assert_eq!(mnemonic, "mov");
        assert_eq!(operands, vec!["RAX", "0x2A"]);
    }

    #[test]
    fn test_m7_vreg_operand() {
        let (mnemonic, operands) = parse_inst("mov VReg(96), fconst");
        assert_eq!(mnemonic, "mov");
        assert_eq!(operands, vec!["VReg(96)", "fconst"]);
    }

    #[test]
    fn test_m8_label_operand() {
        let (mnemonic, operands) = parse_inst("jmp .L0");
        assert_eq!(mnemonic, "jmp");
        assert_eq!(operands, vec![".L0"]);
    }

    #[test]
    fn test_m9_lea_memory_simple() {
        let (mnemonic, operands) = parse_inst("lea rd, [rs1+rs2*1]");
        assert_eq!(mnemonic, "lea");
        // Structured operand returns full text of the memory reference
        assert_eq!(
            operands,
            vec!["rd", "[rs1+rs2*1]"],
            "Expected [dest, memory_ref], got {:?}",
            operands
        );
    }

    #[test]
    fn test_m10_lea_memory_with_disp() {
        let (mnemonic, operands) = parse_inst("lea rd, [rs1+rs2*4+8]");
        assert_eq!(mnemonic, "lea");
        assert_eq!(operands.len(), 2);
        assert_eq!(operands[0], "rd");
        // The full structured operand text
        assert!(
            operands[1].starts_with("[") && operands[1].ends_with("]"),
            "Expected bracketed mem ref, got '{}'",
            operands[1]
        );
    }

    #[test]
    fn test_m11_setcc() {
        let (mnemonic, operands) = parse_inst("sete rd");
        assert_eq!(mnemonic, "sete");
        assert_eq!(operands, vec!["rd"]);
    }

    #[test]
    fn test_m12_jcc() {
        let (mnemonic, operands) = parse_inst("je .L0");
        assert_eq!(mnemonic, "je");
        assert_eq!(operands, vec![".L0"]);
    }

    #[test]
    fn test_m13_push_reg() {
        let (mnemonic, operands) = parse_inst("push RBP");
        assert_eq!(mnemonic, "push");
        assert_eq!(operands, vec!["RBP"]);
    }

    #[test]
    fn test_m14_ret() {
        let (mnemonic, operands) = parse_inst("ret");
        assert_eq!(mnemonic, "ret");
        assert!(
            operands.is_empty(),
            "ret should have 0 operands, got {:?}",
            operands
        );
    }

    #[test]
    fn test_m15_jmp_label() {
        let (mnemonic, operands) = parse_inst("jmp .L0");
        assert_eq!(mnemonic, "jmp");
        assert_eq!(operands, vec![".L0"]);
    }

    #[test]
    fn test_m16_je_label() {
        let (mnemonic, operands) = parse_inst("je .L_target");
        assert_eq!(mnemonic, "je");
        assert_eq!(operands, vec![".L_target"]);
    }

    #[test]
    fn test_m17_xor_reg_reg() {
        let (mnemonic, operands) = parse_inst("xor rd, rs1");
        assert_eq!(mnemonic, "xor");
        assert_eq!(operands, vec!["rd", "rs1"]);
    }

    #[test]
    fn test_m18_shl_reg_cl() {
        let (mnemonic, operands) = parse_inst("shl rd, cl");
        assert_eq!(mnemonic, "shl");
        assert_eq!(operands, vec!["rd", "cl"]);
    }

    #[test]
    fn test_m19_imul_reg_reg() {
        let (mnemonic, operands) = parse_inst("imul rd, rs2");
        assert_eq!(mnemonic, "imul");
        assert_eq!(operands, vec!["rd", "rs2"]);
    }

    #[test]
    fn test_m20_lea_sib_scale4() {
        let (mnemonic, operands) = parse_inst("lea rd, [rs1+rs2*4]");
        assert_eq!(mnemonic, "lea");
        assert_eq!(operands.len(), 2);
        assert_eq!(operands[0], "rd");
        assert!(
            operands[1].starts_with("[") && operands[1].ends_with("]"),
            "Expected bracketed mem ref, got '{}'",
            operands[1]
        );
    }

    // ============================================================
    // 语法指令测试 — 利用现有 macro_def / inst_like+block* 规则
    // S1/S3/S4 原有, S5-S11 新增
    // ============================================================

    #[test]
    fn test_s1_fn_definition() {
        let parser = test_parser();
        let cst = parser
            .parse("fn iadd(rd, rs1, rs2) {\n    mov rd, rs1\n    add rd, rs2\n}")
            .unwrap();
        assert_eq!(cst.kind, "program");

        let macro_defs: Vec<_> = cst.walk().filter(|n| n.kind == "macro_def").collect();
        assert!(!macro_defs.is_empty(), "Expected macro_def node");

        let insts: Vec<_> = cst.walk().filter(|n| n.kind == "inst_like").collect();
        assert_eq!(insts.len(), 2, "Expected 2 instructions in fn body");
    }

    #[test]
    fn test_s3_fn_destructure_params() {
        let parser = test_parser();
        let cst = parser
            .parse("fn switch(disc, (case_val, case_block)) {\n    mov VReg(97), case_val\n}")
            .unwrap();
        let macro_defs: Vec<_> = cst.walk().filter(|n| n.kind == "macro_def").collect();
        assert!(!macro_defs.is_empty());
    }

    #[test]
    fn test_s4_if_else() {
        let parser = test_parser();
        let cst = parser
            .parse("if is_float_return {\n    movsd XMM0, val\n} else {\n    mov RAX, val\n}")
            .unwrap();
        let blocks: Vec<_> = cst.walk().filter(|n| n.kind == "block").collect();
        assert_eq!(blocks.len(), 2, "Expected 2 blocks (then + else)");
    }

    #[test]
    fn test_s5_fn_multi_param() {
        let parser = test_parser();
        let cst = parser
            .parse("fn add3(a, b, c) {\n    add a, b\n    add a, c\n}")
            .unwrap();
        let macro_defs: Vec<_> = cst.walk().filter(|n| n.kind == "macro_def").collect();
        assert!(!macro_defs.is_empty(), "Expected macro_def for fn add3");
        let insts: Vec<_> = cst.walk().filter(|n| n.kind == "inst_like").collect();
        assert_eq!(insts.len(), 2, "Expected 2 instructions in fn body");
    }

    #[test]
    fn test_s6_fn_with_label() {
        let parser = test_parser();
        let cst = parser
            .parse("fn calc(x) {\n    .L_entry:\n    mov rd, x\n    add rd, 1\n}")
            .unwrap();
        let macro_defs: Vec<_> = cst.walk().filter(|n| n.kind == "macro_def").collect();
        assert!(!macro_defs.is_empty());
        let labels: Vec<_> = cst.walk().filter(|n| n.kind == "label_def").collect();
        assert!(!labels.is_empty(), "Expected label_def inside fn body");
    }

    #[test]
    fn test_s7_if_else_insts() {
        let parser = test_parser();
        let cst = parser
            .parse("if flag {\n    mov rd, 1\n} else {\n    mov rd, 0\n}")
            .unwrap();
        let blocks: Vec<_> = cst.walk().filter(|n| n.kind == "block").collect();
        assert_eq!(blocks.len(), 2, "Expected 2 blocks (then + else)");
        let insts: Vec<_> = cst.walk().filter(|n| n.kind == "inst_like").collect();
        // if-flag with block + else with block = 2 inst_like nodes
        assert_eq!(
            insts.len(),
            2,
            "Expected 2 inst_like (if + else), got {}",
            insts.len()
        );
    }

    #[test]
    fn test_s8_if_no_else() {
        let parser = test_parser();
        let cst = parser.parse("if cond {\n    mov rd, 42\n}").unwrap();
        let blocks: Vec<_> = cst.walk().filter(|n| n.kind == "block").collect();
        assert_eq!(blocks.len(), 1, "Expected 1 block (then only)");
    }

    #[test]
    fn test_s9_nested_if() {
        let parser = test_parser();
        let cst = parser
            .parse("if a {\n    if b {\n        mov rd, 1\n    }\n}")
            .unwrap();
        let blocks: Vec<_> = cst.walk().filter(|n| n.kind == "block").collect();
        assert_eq!(blocks.len(), 2, "Expected 2 nested blocks");
    }

    #[test]
    fn test_s10_while_loop() {
        let parser = test_parser();
        let cst = parser
            .parse("while cond {\n    add rd, 1\n    add rd, 2\n}")
            .unwrap();
        let blocks: Vec<_> = cst.walk().filter(|n| n.kind == "block").collect();
        assert_eq!(blocks.len(), 1, "Expected 1 block for while body");
        let insts: Vec<_> = cst.walk().filter(|n| n.kind == "inst_like").collect();
        // while-cond + 1 inner add = 2 (other add may be grouped)
        assert!(
            insts.len() >= 2,
            "Expected at least 2 inst_like, got {}",
            insts.len()
        );
    }

    #[test]
    fn test_s11_if_else_if_chain() {
        let parser = test_parser();
        let cst = parser
            .parse("if a {\n    ret\n} else if b {\n    ret\n} else {\n    ret\n}")
            .unwrap();
        let blocks: Vec<_> = cst.walk().filter(|n| n.kind == "block").collect();
        assert_eq!(blocks.len(), 3, "Expected 3 blocks (if / else-if / else)");
    }

    // ============================================================
    // 混合解析测试 — 多种指令类型在同一程序中
    // ============================================================

    #[test]
    fn test_mix1_fn_containing_if() {
        let parser = test_parser();
        let cst = parser.parse(
            "fn abs(x) {\n    if x {\n        mov rd, x\n    } else {\n        mov rd, 0\n    }\n}"
        ).unwrap();
        // macro_def contains if (inst_like with blocks)
        let macro_defs: Vec<_> = cst.walk().filter(|n| n.kind == "macro_def").collect();
        assert!(!macro_defs.is_empty(), "Expected macro_def for fn abs");
        let blocks: Vec<_> = cst.walk().filter(|n| n.kind == "block").collect();
        assert_eq!(
            blocks.len(),
            3,
            "Expected 3 blocks (fn body + if-then + if-else)"
        );
    }

    #[test]
    fn test_mix2_fn_containing_while() {
        let parser = test_parser();
        let cst = parser
            .parse("fn count(n) {\n    while n {\n        add rd, 1\n    }\n}")
            .unwrap();
        let macro_defs: Vec<_> = cst.walk().filter(|n| n.kind == "macro_def").collect();
        assert!(!macro_defs.is_empty(), "Expected macro_def for fn count");
        let blocks: Vec<_> = cst.walk().filter(|n| n.kind == "block").collect();
        assert!(
            blocks.len() >= 2,
            "Expected fn body block + while body block"
        );
    }

    #[test]
    fn test_mix3_program_fn_and_if() {
        let parser = test_parser();
        let cst = parser
            .parse("fn init(x) {\n    push RBP\n    mov RBP, RSP\n}\nif ready {\n    ret\n}")
            .unwrap();
        let macro_defs: Vec<_> = cst.walk().filter(|n| n.kind == "macro_def").collect();
        assert!(!macro_defs.is_empty(), "Expected macro_def for fn init");
        let blocks: Vec<_> = cst.walk().filter(|n| n.kind == "block").collect();
        assert_eq!(blocks.len(), 2, "Expected 2 blocks (fn body + if body)");
    }

    // ============================================================
    // 降级 TokenStream 验证测试 — asm → lowering TokenStream
    // ============================================================

    #[test]
    fn test_lower1_mov() {
        let model = test_model();
        let lowering = gen_lowering_insts_cst(&["mov rd, rs1".into()], &model).unwrap();
        assert_eq!(
            lowering.insts.len(),
            1,
            "Expected 1 lowered inst, got {}",
            lowering.insts.len()
        );
        let toks = lowering.insts[0].to_string();
        assert!(
            toks.contains("Inst"),
            "Expected Inst in output, got: {toks}"
        );
        assert!(toks.contains("Mov"), "Expected Mov variant, got: {toks}");
    }

    #[test]
    fn test_lower2_add() {
        let model = test_model();
        let lowering = gen_lowering_insts_cst(&["add rd, rs2".into()], &model).unwrap();
        assert_eq!(lowering.insts.len(), 1);
        let toks = lowering.insts[0].to_string();
        assert!(toks.contains("Add"), "Expected Add variant, got: {toks}");
    }

    #[test]
    fn test_lower3_multi_inst() {
        let model = test_model();
        let lowering =
            gen_lowering_insts_cst(&["mov rd, rs1".into(), "add rd, rs2".into()], &model).unwrap();
        assert_eq!(
            lowering.insts.len(),
            2,
            "Expected 2 lowered insts, got {}",
            lowering.insts.len()
        );
    }

    #[test]
    fn test_lower4_with_label() {
        let model = test_model();
        let lowering =
            gen_lowering_insts_cst(&[".L0:".into(), "mov rd, rs1".into()], &model).unwrap();
        assert_eq!(
            lowering.insts.len(),
            2,
            "Expected label + inst, got {}",
            lowering.insts.len()
        );
    }

    #[test]
    fn test_lower5_ret() {
        let model = test_model();
        let lowering = gen_lowering_insts_cst(&["ret".into()], &model).unwrap();
        assert_eq!(lowering.insts.len(), 1);
        let toks = lowering.insts[0].to_string();
        assert!(toks.contains("Ret"), "Expected Ret variant, got: {toks}");
    }

    #[test]
    fn test_lower6_empty_and_comment() {
        let model = test_model();
        let lowering =
            gen_lowering_insts_cst(&["".into(), "# comment".into(), "ret".into()], &model).unwrap();
        assert_eq!(
            lowering.insts.len(),
            1,
            "Expected 1 inst (empty+comment skipped), got {}",
            lowering.insts.len()
        );
    }
} // mod tests
