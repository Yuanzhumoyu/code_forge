//! Integration tests for the ASM v11 grammar.
//!
//! Tests that the .lx grammar correctly parses real-world lowering examples.

use lang_frontend::{Grammar, Parser, parse_grammar};

fn load_asm_grammar() -> Grammar {
    let src = include_str!("../grammars/asm_v11.lx");
    parse_grammar(src).expect("Failed to parse asm_v11.lx grammar")
}

#[test]
fn test_grammar_loads() {
    let grammar = load_asm_grammar();
    assert!(!grammar.rules.is_empty());
    assert!(grammar.rules.contains_key("program"));
    assert!(grammar.rules.contains_key("inst_like"));
    assert!(grammar.rules.contains_key("operand"));
    assert!(grammar.rules.contains_key("expr"));
}

#[test]
fn test_parse_simple_lowering() {
    let grammar = load_asm_grammar();
    let parser = Parser::build(grammar);

    // Simple two-instruction lowering: "mov rd, rs1" and "add rd, rs2"
    let cst = parser.parse("mov rd, rs1\nadd rd, rs2").unwrap();
    assert_eq!(cst.kind, "program");

    // Find all inst_like nodes
    let insts: Vec<_> = cst.walk().filter(|n| n.kind == "inst_like").collect();
    assert_eq!(insts.len(), 2, "Expected 2 instructions");
}

#[test]
fn test_parse_mov_inst() {
    let grammar = load_asm_grammar();
    let parser = Parser::build(grammar);

    let cst = parser.parse_rule("mov rd, rs1", "inst_like").unwrap();
    assert_eq!(cst.kind, "inst_like");

    // Collect all IDENT leaf tokens
    let idents: Vec<String> = cst.leaves().iter()
        .filter(|t| t.kind == "IDENT")
        .map(|t| t.text.clone())
        .collect();
    assert_eq!(idents, vec!["mov", "rd", "rs1"]);
}

#[test]
fn test_parse_push_reg() {
    let grammar = load_asm_grammar();
    let parser = Parser::build(grammar);

    let cst = parser.parse_rule("push RBP", "inst_like").unwrap();
    assert_eq!(cst.kind, "inst_like");

    // Find REG leaf
    let regs: Vec<String> = cst.leaves().iter()
        .filter(|t| t.kind == "REG")
        .map(|t| t.text.clone())
        .collect();
    assert!(!regs.is_empty());
    assert_eq!(regs[0], "RBP");
}

#[test]
fn test_parse_pseudo_call() {
    let grammar = load_asm_grammar();
    let parser = Parser::build(grammar);

    let cst = parser.parse_rule("@move_args", "pseudo_call").unwrap();
    assert_eq!(cst.kind, "pseudo_call");

    let idents: Vec<String> = cst.leaves().iter()
        .filter(|t| t.kind == "IDENT")
        .map(|t| t.text.clone())
        .collect();
    assert!(idents.contains(&"move_args".to_string()));
}

#[test]
fn test_parse_label_def() {
    let grammar = load_asm_grammar();
    let parser = Parser::build(grammar);

    let cst = parser.parse_rule(".L0:", "label_def").unwrap();
    assert_eq!(cst.kind, "label_def");

    let labels: Vec<String> = cst.leaves().iter()
        .filter(|t| t.kind == "LABEL")
        .map(|t| t.text.clone())
        .collect();
    assert!(!labels.is_empty());
}

#[test]
fn test_parse_if_else() {
    let grammar = load_asm_grammar();
    let parser = Parser::build(grammar);

    let cst = parser.parse_rule(
        "if is_float_return {\n    movsd XMM0, val\n} else {\n    mov RAX, val\n}",
        "inst_like"
    ).unwrap();
    assert_eq!(cst.kind, "inst_like");

    // Should have blocks (from if)
    let blocks: Vec<_> = cst.walk().filter(|n| n.kind == "block").collect();
    assert_eq!(blocks.len(), 2, "Expected 2 blocks (then + else)");
}

#[test]
fn test_parse_with_immediate() {
    let grammar = load_asm_grammar();
    let parser = Parser::build(grammar);

    // Instruction with immediate value
    let cst = parser.parse_rule("mov RAX, 42", "inst_like").unwrap();
    assert_eq!(cst.kind, "inst_like");

    let decs: Vec<String> = cst.leaves().iter()
        .filter(|t| t.kind == "DEC")
        .map(|t| t.text.clone())
        .collect();
    assert!(!decs.is_empty());
    assert_eq!(decs[0], "42");
}

#[test]
fn test_parse_hex_immediate() {
    let grammar = load_asm_grammar();
    let parser = Parser::build(grammar);

    let cst = parser.parse_rule("mov RAX, 0x8000_0000", "inst_like").unwrap();
    let hexes: Vec<String> = cst.leaves().iter()
        .filter(|t| t.kind == "HEX")
        .map(|t| t.text.clone())
        .collect();
    assert!(!hexes.is_empty());
}

#[test]
fn test_parse_branch_lowering() {
    let grammar = load_asm_grammar();
    let parser = Parser::build(grammar);

    // Branch lowering: test + conditional jump
    let cst = parser.parse("test cond, cond\nje false_block\njmp true_block").unwrap();
    assert_eq!(cst.kind, "program");

    let insts: Vec<_> = cst.walk().filter(|n| n.kind == "inst_like").collect();
    assert_eq!(insts.len(), 3);
}

#[test]
fn test_parse_prologue() {
    let grammar = load_asm_grammar();
    let parser = Parser::build(grammar);

    // Prologue sequence
    let cst = parser.parse("@move_args\npush RBP\nmov RBP, RSP\n@push_callee").unwrap();
    assert_eq!(cst.kind, "program");

    // Check we have pseudo calls and inst_likes
    let pseudos: Vec<_> = cst.walk().filter(|n| n.kind == "pseudo_call").collect();
    let insts: Vec<_> = cst.walk().filter(|n| n.kind == "inst_like").collect();
    assert_eq!(pseudos.len(), 2); // @move_args, @push_callee
    assert_eq!(insts.len(), 2);   // push RBP, mov RBP, RSP
}
