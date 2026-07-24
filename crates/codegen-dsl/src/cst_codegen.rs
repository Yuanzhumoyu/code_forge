//! CST-based code generation using lang-frontend.
//!
//! This module replaces the hand-written V11 pipeline (asm_lexer, asm_parser,
//! asm_ast, macro_registry, macro_expander) with grammar-driven parsing via
//! the `lang-frontend` crate.
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
use lang_frontend::{CstNode, Parser, parse_grammar};
#[cfg(test)]
use lang_frontend::{Expr, Grammar, TokenPattern};
use proc_macro2::TokenStream;
use quote::quote;
#[cfg(test)]
use std::collections::{HashMap, HashSet};

// ============================================================
// Grammar building — merge base.lx + TOML model
// (test-only: used by test_parser() helper)
// ============================================================

#[cfg(test)]
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
                .push(lang_frontend::TokenDef::regex(name, &def.pattern));
        }
    }

    // 2. Add keyword tokens — as IDENT-level tokens, not separate kinds.
    // Keywords match with higher priority than the general IDENT pattern.
    // They produce the same token kind ("IDENT") but match specific text first.
    if let Some(keywords) = &model.lang_keywords {
        for (_name, text) in keywords {
            // Insert keyword as a regex that matches exact text, before general IDENT
            // Use name like "fn" → creates a KW_FN-like matcher but produces IDENT kind
            grammar.tokens.push(lang_frontend::TokenDef {
                name: "IDENT".to_string(), // Same kind as IDENT
                pattern: lang_frontend::TokenPattern::Literal(text.clone()),
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
                lang_frontend::grammar::RuleDef {
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
            lang_frontend::grammar::RuleDef {
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
#[cfg(test)]
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

#[cfg(test)]
/// Generate the `operand` rule from instruction asm templates in the ISA model.
fn generate_operand_from_insts(
    insts: &HashMap<String, crate::model::Instruction>,
    _cc_names: &HashMap<String, String>,
) -> Expr {
    // Collect all operand token types from inst field types
    let mut token_types = HashSet::new();
    for inst in insts.values() {
        for field in &inst.fields {
            match field.field_type {
                crate::model::FieldType::VReg => {
                    token_types.insert("IDENT");
                    token_types.insert("VREG");
                }
                crate::model::FieldType::Reg => {
                    token_types.insert("REG");
                }
                crate::model::FieldType::I64
                | crate::model::FieldType::U8
                | crate::model::FieldType::U32 => {
                    token_types.insert("DEC");
                    token_types.insert("HEX");
                    token_types.insert("IDENT");
                }
                crate::model::FieldType::BlockTarget => {
                    token_types.insert("LABEL");
                }
                crate::model::FieldType::F64 => {
                    token_types.insert("FLOAT");
                }
                crate::model::FieldType::CondCode => {} // expanded in mnemonic, not an operand
                crate::model::FieldType::Opsize => {} // implicit, not an operand
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

    Expr::alt(alts)
}

#[cfg(test)]
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
                lang_frontend::TokenDef {
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

    for (field_name, arg_val) in &resolved.bindings {
        let field = inst_def
            .fields
            .iter()
            .find(|f| &f.name == field_name)
            .ok_or_else(|| format!("field '{field_name}' not found in '{}'", resolved.inst_name))?;
        let fi = quote::format_ident!("{field_name}");
        let expr = match mode {
            GenMode::Lowering => lowering_arg_expr(arg_val, &field.field_type, scratch),
            GenMode::Emit => emit_arg_expr(arg_val, &field.field_type, model),
        };
        field_exprs.push(quote! { #fi: #expr });
    }

    // Fill missing fields with defaults
    for field in &inst_def.fields {
        if !resolved.bindings.iter().any(|(n, _)| *n == field.name) {
            let fi = quote::format_ident!("{}", field.name);
            let default = default_for_type(&field.field_type);
            field_exprs.push(quote! { #fi: #default });
        }
    }

    Ok(quote! { Inst::#ivn { #(#field_exprs),* } })
}

// ============================================================
// Main lowering/emit functions
// ============================================================

/// Generate lowering instruction TokenStreams from V11-format assembly strings.
///
/// This is the CST-based replacement for `gen_lower_insts_v11()`.
/// Uses dynamic grammar built from base.lx + TOML model.
pub fn gen_lowering_insts_cst(
    insts: &[String],
    model: &IsaModel,
) -> Result<Vec<TokenStream>, String> {
    let grammar = parse_grammar(include_str!("../grammars/base.lx"))
        .map_err(|e| format!("grammar error: {e}"))?;
    let parser = Parser::build(grammar);
    let resolver = AsmResolver::build(model);

    let mut result = Vec::new();
    for line in insts {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Handle pseudo-calls
        if let Some(name) = trimmed.strip_prefix('@') {
            let toks = gen_pseudo(name, model)?;
            result.extend(toks);
            continue;
        }

        // Handle label definitions
        if trimmed.ends_with(':') {
            let label = trimmed.trim_end_matches(':');
            result.push(quote! { /* label .{} */ let _ = #label; });
            continue;
        }

        // Parse and resolve
        let cst = parser
            .parse_rule(trimmed, "inst_like")
            .map_err(|e| format!("parse error in '{trimmed}': {e}"))?;

        let inst_tok = gen_inst_from_cst(&cst, &resolver, model, GenMode::Lowering)
            .map_err(|e| format!("in '{trimmed}': {e}"))?;
        result.push(inst_tok);
    }

    Ok(result)
}

/// Generate emit TokenStream from V11-format assembly strings.
///
/// This is the CST-based replacement for `gen_emit_insts_sequence_v11()`.
/// Used for prologue/epilogue emit sequences.
pub fn gen_emit_insts_cst(insts: &[String], model: &IsaModel) -> TokenStream {
    let parser = match parse_grammar(include_str!("../grammars/base.lx")) {
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
            meta: crate::model::Meta {
                name: "test".into(),
                version: "1".into(),
                endian: "little".into(),
                mode: 64,
                max_inst_len: 15,
                capabilities: Default::default(),
                no_default_lowering: true,
            },
            reg: {
                let mut m = HashMap::new();
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
            inst: HashMap::new(),
            lower: HashMap::new(),
            lower_term: HashMap::new(),
            lower_pattern: HashMap::new(),
            emit: None,
            enc_macros: HashMap::new(),
            enc_scatters: HashMap::new(),
            cc_names: HashMap::new(),
            dyn_types: HashMap::new(),
            lang_tokens: {
                let mut t = HashMap::new();
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
                let mut k = HashMap::new();
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
                field_type: crate::model::FieldType::VReg,
            }
        }
        fn reg_field(n: &str) -> crate::model::InstField {
            crate::model::InstField {
                name: n.into(),
                field_type: crate::model::FieldType::Reg,
            }
        }
        fn blk(n: &str) -> crate::model::InstField {
            crate::model::InstField {
                name: n.into(),
                field_type: crate::model::FieldType::BlockTarget,
            }
        }

        model.inst.insert(
            "ADD".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "add {dest}, {src}".into(),
                effect: None,
            },
        );
        model.inst.insert(
            "MOV".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "mov {dest}, {src}".into(),
                effect: None,
            },
        );
        model.inst.insert(
            "PUSH".into(),
            crate::model::Instruction {
                fields: vec![reg_field("reg")],
                encoding: None,
                asm: "push {reg}".into(),
                effect: None,
            },
        );
        model.inst.insert(
            "RET".into(),
            crate::model::Instruction {
                fields: vec![],
                encoding: None,
                asm: "ret".into(),
                effect: None,
            },
        );
        model.inst.insert(
            "JMP".into(),
            crate::model::Instruction {
                fields: vec![blk("rel")],
                encoding: None,
                asm: "jmp {rel}".into(),
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
                    },
                    blk("rel"),
                ],
                encoding: None,
                asm: "j{cond} {rel}".into(),
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
                    },
                    vreg("dest"),
                ],
                encoding: None,
                asm: "set{cond} {dest}".into(),
                effect: None,
            },
        );
        model.inst.insert(
            "XOR".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "xor {dest}, {src}".into(),
                effect: None,
            },
        );
        model.inst.insert(
            "SHL".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "shl {dest}, {src}".into(),
                effect: None,
            },
        );
        model.inst.insert(
            "SHR".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "shr {dest}, {src}".into(),
                effect: None,
            },
        );
        model.inst.insert(
            "SAR".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "sar {dest}, {src}".into(),
                effect: None,
            },
        );
        model.inst.insert(
            "IMUL".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "imul {dest}, {src}".into(),
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
                        field_type: crate::model::FieldType::I64,
                    },
                    crate::model::InstField {
                        name: "disp".into(),
                        field_type: crate::model::FieldType::I64,
                    },
                ],
                encoding: None,
                asm: "lea {dest}, [{base}+{index}*{scale}]".into(),
                effect: None,
            },
        );
        model.inst.insert(
            "MOVSD".into(),
            crate::model::Instruction {
                fields: vec![vreg("dest"), vreg("src")],
                encoding: None,
                asm: "movsd {dest}, {src}".into(),
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
        let result = gen_lowering_insts_cst(&["mov rd, rs1".into()], &model).unwrap();
        assert_eq!(
            result.len(),
            1,
            "Expected 1 lowered inst, got {}",
            result.len()
        );
        let toks = result[0].to_string();
        assert!(
            toks.contains("Inst"),
            "Expected Inst in output, got: {toks}"
        );
        assert!(toks.contains("Mov"), "Expected Mov variant, got: {toks}");
    }

    #[test]
    fn test_lower2_add() {
        let model = test_model();
        let result = gen_lowering_insts_cst(&["add rd, rs2".into()], &model).unwrap();
        assert_eq!(result.len(), 1);
        let toks = result[0].to_string();
        assert!(toks.contains("Add"), "Expected Add variant, got: {toks}");
    }

    #[test]
    fn test_lower3_multi_inst() {
        let model = test_model();
        let result =
            gen_lowering_insts_cst(&["mov rd, rs1".into(), "add rd, rs2".into()], &model).unwrap();
        assert_eq!(
            result.len(),
            2,
            "Expected 2 lowered insts, got {}",
            result.len()
        );
    }

    #[test]
    fn test_lower4_with_label() {
        let model = test_model();
        let result =
            gen_lowering_insts_cst(&[".L0:".into(), "mov rd, rs1".into()], &model).unwrap();
        assert_eq!(
            result.len(),
            2,
            "Expected label + inst, got {}",
            result.len()
        );
    }

    #[test]
    fn test_lower5_ret() {
        let model = test_model();
        let result = gen_lowering_insts_cst(&["ret".into()], &model).unwrap();
        assert_eq!(result.len(), 1);
        let toks = result[0].to_string();
        assert!(toks.contains("Ret"), "Expected Ret variant, got: {toks}");
    }

    #[test]
    fn test_lower6_empty_and_comment() {
        let model = test_model();
        let result =
            gen_lowering_insts_cst(&["".into(), "# comment".into(), "ret".into()], &model).unwrap();
        assert_eq!(
            result.len(),
            1,
            "Expected 1 inst (empty+comment skipped), got {}",
            result.len()
        );
    }
} // mod tests
