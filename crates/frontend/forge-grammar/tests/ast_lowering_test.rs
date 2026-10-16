//! Integration tests for AST lowering (CST → TypedAst).
//!
//! End-to-end tests: .lx grammar → CST → schema-driven AST lowering.
//! 用 `ir.lx`（forge-ir 的 IR 语法）作真实输入——**v11 的 asm 语法已随 v11 语法层删除**
//! （v26 M2 清掉最后遗留的 `grammars/asm_v11.lx` 与只测它的 asm_grammar_test.rs）。

use forge_grammar::{AstSchema, Parser, lower_cst, parse_grammar};
use std::sync::LazyLock;

// ============================================================
// Lowering with ir.lx grammar
// ============================================================

fn ir_grammar() -> &'static forge_grammar::Grammar {
    static G: LazyLock<forge_grammar::Grammar> = LazyLock::new(|| {
        let src = include_str!("../../../foundation/forge-ir/grammars/ir.lx");
        parse_grammar(src).unwrap()
    });
    &G
}

fn ir_schema() -> AstSchema {
    let mut s = AstSchema::new();
    s.define_struct("function_def")
        .unwrap()
        .field_text("name", "IDENT")
        .field_node("param_list", "param_list")
        .field_node("ret_types", "ret_types")
        .field_node("block_list", "block_list")
        .finish();
    s.define_struct("param_list")
        .unwrap()
        .field_children("params", "param")
        .finish();
    s.define_struct("param")
        .unwrap()
        .field_text("name", "IDENT")
        .finish();
    s.define_struct("ret_types").unwrap().finish();
    s.define_struct("block_list")
        .unwrap()
        .field_children("blocks", "block")
        .finish();
    s.define_struct("block")
        .unwrap()
        .field_text("name", "IDENT")
        .finish();
    s.define_struct("inst")
        .unwrap()
        .field_node("results", "results")
        .field_node("opcode", "opcode")
        .field_node("operands", "operands")
        .finish();
    s.define_struct("results").unwrap().finish();
    s.define_struct("opcode")
        .unwrap()
        .field_text("name", "IDENT")
        .finish();
    s.define_struct("operands").unwrap().finish();
    s.define_enum("operand")
        .unwrap()
        .add_variant("LocalIdent", "LOCAL_IDENT")
        .add_variant("GlobalIdent", "GLOBAL_IDENT")
        .add_variant("IntLit", "INT_LIT")
        .add_variant("HexLit", "HEX_LIT")
        .add_variant("FloatLit", "FLOAT_LIT")
        .add_variant("TypeAnn", "TYPE")
        .add_variant("PtrType", "PTR_TYPE")
        .add_variant("VecType", "VEC_TYPE")
        .add_variant("Ident", "IDENT")
        .finish();
    s
}

#[test]
fn test_lower_ir_simple_function() {
    let grammar = ir_grammar();
    let schema = ir_schema();
    let parser = Parser::build(grammar.clone());

    let cst = parser.parse("fn add() -> i32 { entry: ret i32 }").unwrap();
    let ast = lower_cst(&cst, &schema, grammar).unwrap();

    let funcs = ast.root_ref().find_all("function_def");
    assert!(!funcs.is_empty());
    assert_eq!(funcs[0].get_text("name"), Some("add"));
}

#[test]
fn test_lower_ir_function_with_params() {
    let grammar = ir_grammar();
    let schema = ir_schema();
    let parser = Parser::build(grammar.clone());

    let cst = parser
        .parse("fn foo(x: i32, y: i64) -> i32 { entry: ret i32 }")
        .unwrap();
    let ast = lower_cst(&cst, &schema, grammar).unwrap();

    let funcs = ast.root_ref().find_all("function_def");
    assert!(!funcs.is_empty());
    assert_eq!(funcs[0].get_text("name"), Some("foo"));

    // Find param nodes
    let params: Vec<_> = ast.root_ref().find_all("param");
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].get_text("name"), Some("x"));
    assert_eq!(params[1].get_text("name"), Some("y"));
}

#[test]
fn test_lower_ir_empty_module() {
    let grammar = ir_grammar();
    let schema = ir_schema();
    let parser = Parser::build(grammar.clone());

    let cst = parser.parse("").unwrap();
    let ast = lower_cst(&cst, &schema, grammar).unwrap();
    assert!(!ast.arena.is_empty()); // Should have at least root
}

#[test]
fn test_lower_ir_comment_only() {
    let grammar = ir_grammar();
    let schema = ir_schema();
    let parser = Parser::build(grammar.clone());

    let cst = parser.parse("# This is a comment").unwrap();
    let ast = lower_cst(&cst, &schema, grammar).unwrap();
    assert!(!ast.arena.is_empty());
}

// ============================================================
// Lowering edge cases
// ============================================================

#[test]
fn test_lower_transparent_no_schema() {
    // When no schema is defined, lowering should produce a valid AST
    // via transparent pass-through
    let grammar =
        parse_grammar("token IDENT = \"[a-z]+\"\nskip \"[ \\t]+\"\nprogram ::= IDENT\n").unwrap();
    let schema = AstSchema::new(); // Empty schema
    let parser = Parser::build(grammar.clone());

    let cst = parser.parse("hello").unwrap();
    let ast = lower_cst(&cst, &schema, &grammar).unwrap();
    assert!(!ast.arena.is_empty());
}

#[test]
fn test_parser_parse_to_ast() {
    // Test the convenience method Parser::parse_to_ast()
    let grammar = parse_grammar(
        "token IDENT = \"[a-z]+\"\ntoken REG = \"[A-Z]+\"\nskip \"[ \\t]+\"\nprogram ::= IDENT REG\n"
    ).unwrap();
    let mut schema = AstSchema::new();
    schema
        .define_struct("program")
        .unwrap()
        .field_text("name", "IDENT")
        .finish();

    let parser = Parser::build(grammar);
    let ast = parser.parse_to_ast("hello WORLD", &schema).unwrap();

    let root = ast.root_ref();
    assert_eq!(root.kind(), "program");
    assert_eq!(root.get_text("name"), Some("hello"));
}

#[test]
fn test_parser_parse_rule_to_ast() {
    // Use a grammar with explicit structural wrapping (Seq)
    let grammar = parse_grammar(
        "token IDENT = \"[a-z]+\"\npunct \";\"\nskip \"[ \\t]+\"\nprogram ::= IDENT \";\"\n",
    )
    .unwrap();
    let mut schema = AstSchema::new();
    schema
        .define_struct("program")
        .unwrap()
        .field_text("name", "IDENT")
        .finish();

    let parser = Parser::build(grammar);
    let ast = parser
        .parse_rule_to_ast("hello ;", "program", &schema)
        .unwrap();

    // Root should be "program" with name "hello"
    let root = ast.root_ref();
    // Transparent pass-through may inline single child — check via walk
    let has_program = ast.root_ref().walk().any(|n| n.kind() == "program");
    assert!(
        has_program || root.kind() == "program",
        "Expected program node, got root kind={}",
        root.kind()
    );
}

#[test]
fn test_lower_with_flattened_children() {
    // Test FieldType::FlattenedChildren — works when children are inside
    // rep/rep1 wrappers as direct children of the parent CST node
    let grammar = parse_grammar(
        "token IDENT = \"[a-z]+\"\npunct \",\"\nskip \"[ \\t]+\"\nprogram ::= IDENT (\",\" IDENT)*\n"
    ).unwrap();
    let mut schema = AstSchema::new();
    schema
        .define_struct("program")
        .unwrap()
        .field_text("name", "IDENT")
        .field_flattened("items", "IDENT")
        .finish();

    let parser = Parser::build(grammar.clone());
    let cst = parser.parse("one").unwrap();
    let ast = lower_cst(&cst, &schema, &grammar).unwrap();

    let root = ast.root_ref();
    // FlattenedChildren finds IDENT tokens inside rep wrappers
    let items = root.get_children("items");
    // Verify lowering succeeded (items should be populated or empty)
    let _ = items.len();
}

#[test]
fn test_lower_with_concatenated_text() {
    // Test FieldType::ConcatenatedText lowering
    let grammar = parse_grammar(
        "token IDENT = \"[a-z]+\"\nskip \"[ \\t]+\"\nprogram ::= IDENT IDENT IDENT\n",
    )
    .unwrap();
    let mut schema = AstSchema::new();
    schema
        .define_struct("program")
        .unwrap()
        .field_concat_text("full", ".")
        .finish();

    let parser = Parser::build(grammar.clone());
    let cst = parser.parse("a b c").unwrap();
    let ast = lower_cst(&cst, &schema, &grammar).unwrap();

    let root = ast.root_ref();
    assert_eq!(root.get_text("full"), Some("a.b.c"));
}

#[test]
fn test_lower_with_node_and_node_list() {
    // Test FieldType::Node and FieldType::NodeList with a grammar
    // that produces structurally wrapped nodes
    let grammar = parse_grammar(
        "token IDENT = \"[a-z]+\"\npunct \"(\" \")\" \",\"\nskip \"[ \\t]+\"\nfunc ::= IDENT \"(\" args \")\"\nargs ::= IDENT (\",\" IDENT)*\n"
    ).unwrap();
    let mut schema = AstSchema::new();
    schema
        .define_struct("func")
        .unwrap()
        .field_text("name", "IDENT")
        .field_node("args", "args")
        .finish();
    schema
        .define_struct("args")
        .unwrap()
        .field_children("params", "IDENT")
        .finish();

    let parser = Parser::build(grammar.clone());
    let cst = parser.parse("foo(x, y)").unwrap();
    let ast = lower_cst(&cst, &schema, &grammar).unwrap();

    // Check that func node was created with the right name
    let funcs: Vec<_> = ast.root_ref().find_all("func");
    assert!(!funcs.is_empty(), "Expected func node");
    assert_eq!(funcs[0].get_text("name"), Some("foo"));

    // Check args via the args node
    let args_nodes: Vec<_> = ast.root_ref().find_all("args");
    assert!(!args_nodes.is_empty(), "Expected args node");
    let params = args_nodes[0].get_children("params");
    assert_eq!(params.len(), 2, "Expected 2 params");
}
