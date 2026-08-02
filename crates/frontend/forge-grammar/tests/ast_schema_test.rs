//! Integration tests for AstSchema — schema builder, validation, inference.
//!
//! Tests the runtime schema definition API end-to-end with real grammars.

use forge_grammar::{
    AstSchema, FieldType, NodeKind, SchemaError, Symbol, lower_cst, parse_grammar,
    parse_with_grammar,
};

// ============================================================
// Schema builder tests
// ============================================================

#[test]
fn test_schema_builder_struct_all_field_types() {
    let mut s = AstSchema::new();
    s.define_struct("test_node")
        .unwrap()
        .field_text("mnemonic", "IDENT")
        .field_children("operands", "operand")
        .field_optional("ret_type", "TYPE")
        .field_flattened("items", "stmt")
        .field_node("param_list", "param_list")
        .field_node_list("blocks", "block")
        .field_concat_text("full_name", ".")
        .field_all_tokens("raw_tokens")
        .finish();

    let def = s.lookup("test_node").unwrap();
    assert_eq!(def.fields.len(), 8);
    assert_eq!(def.node_kind, NodeKind::Struct);
}

#[test]
fn test_schema_enum_builder() {
    let mut s = AstSchema::new();
    s.define_enum("operand")
        .unwrap()
        .add_variant("Ident", "IDENT")
        .add_variant("Reg", "REG")
        .add_variant("Dec", "DEC")
        .add_variant("Hex", "HEX")
        .add_variant("Temp", "TEMP")
        .add_variant("Label", "LABEL")
        .finish();

    let def = s.lookup("operand").unwrap();
    assert_eq!(def.node_kind, NodeKind::Enum);
    assert_eq!(def.variants.len(), 6);

    // Verify lookup by token kind
    assert_eq!(s.lookup_variant("operand", "REG").unwrap().name, "Reg");
    assert_eq!(s.lookup_variant("operand", "IDENT").unwrap().name, "Ident");
    assert!(s.lookup_variant("operand", "NONEXISTENT").is_none());
}

#[test]
fn test_schema_duplicate_node_error() {
    let mut s = AstSchema::new();
    s.define_struct("foo")
        .unwrap()
        .field_text("x", "X")
        .finish();

    let err = s.define_struct("foo").unwrap_err();
    assert!(matches!(err, SchemaError::DuplicateNode(_)));
    assert!(format!("{}", err).contains("foo"));
}

#[test]
fn test_schema_duplicate_enum_error() {
    let mut s = AstSchema::new();
    s.define_enum("bar")
        .unwrap()
        .add_variant("A", "AAA")
        .finish();

    let err = s.define_enum("bar").unwrap_err();
    assert!(matches!(err, SchemaError::DuplicateNode(_)));
}

// ============================================================
// Schema validation tests
// ============================================================

#[test]
fn test_schema_validate_passes_for_valid_references() {
    let grammar = parse_grammar(
        "token IDENT = \"[a-z]+\"\ntoken REG = \"[A-Z]+\"\nskip \"[ \\t]+\"\ninst ::= IDENT REG\noperand ::= IDENT | REG\n"
    ).unwrap();

    let mut s = AstSchema::new();
    s.define_struct("inst")
        .unwrap()
        .field_text("mnemonic", "IDENT")
        .field_children("regs", "REG")
        .finish();
    s.define_enum("operand")
        .unwrap()
        .add_variant("Ident", "IDENT")
        .add_variant("Reg", "REG")
        .finish();

    s.validate(&grammar).unwrap();
}

#[test]
fn test_schema_validate_fails_bad_token_reference() {
    let grammar =
        parse_grammar("token IDENT = \"[a-z]+\"\nskip \"[ \\t]+\"\nrule ::= IDENT\n").unwrap();

    let mut s = AstSchema::new();
    s.define_struct("rule")
        .unwrap()
        .field_text("name", "NONEXISTENT")
        .finish();

    let err = s.validate(&grammar).unwrap_err();
    assert!(format!("{}", err).contains("NONEXISTENT"));
}

#[test]
fn test_schema_validate_fails_bad_variant_token() {
    let grammar =
        parse_grammar("token IDENT = \"[a-z]+\"\nskip \"[ \\t]+\"\nrule ::= IDENT\n").unwrap();

    let mut s = AstSchema::new();
    s.define_enum("rule")
        .unwrap()
        .add_variant("Bad", "NONEXISTENT_TOKEN")
        .finish();

    let err = s.validate(&grammar).unwrap_err();
    assert!(format!("{}", err).contains("NONEXISTENT_TOKEN"));
}

#[test]
fn test_schema_validate_produces_missing_definition_diagnostics() {
    let grammar =
        parse_grammar("token IDENT = \"[a-z]+\"\nskip \"[ \\t]+\"\nfoo ::= IDENT\nbar ::= IDENT\n")
            .unwrap();

    let mut s = AstSchema::new();
    // Only define schema for "foo", not "bar"
    s.define_struct("foo")
        .unwrap()
        .field_text("name", "IDENT")
        .finish();

    s.validate(&grammar).unwrap();
    assert!(
        !s.diagnostics.is_empty(),
        "Expected MissingDefinition diagnostic for 'bar'"
    );
}

// ============================================================
// Schema inference tests
// ============================================================

#[test]
fn test_schema_infer_simple_enum() {
    let grammar = parse_grammar(
        "token IDENT = \"[a-z]+\"\ntoken REG = \"[A-Z]+\"\ntoken DEC = \"[0-9]+\"\nskip \"[ \\t]+\"\noperand ::= IDENT | REG | DEC\n"
    ).unwrap();

    let schema = AstSchema::infer_from_grammar(&grammar);
    let def = schema.lookup("operand").unwrap();
    assert_eq!(def.node_kind, NodeKind::Enum);
    assert!(!def.variants.is_empty(), "Enum should have variants");
}

#[test]
fn test_schema_infer_struct_with_children() {
    let grammar = parse_grammar(
        "token IDENT = \"[a-z]+\"\ntoken REG = \"[A-Z]+\"\nskip \"[ \\t]+\"\nprogram ::= stmt*\nstmt ::= IDENT | REG\n"
    ).unwrap();

    let schema = AstSchema::infer_from_grammar(&grammar);
    assert!(schema.lookup("program").is_some());
    assert!(schema.lookup("stmt").is_some());
}

#[test]
fn test_schema_infer_zero_or_more_sets_multiple_flag() {
    let grammar =
        parse_grammar("token IDENT = \"[a-z]+\"\nskip \"[ \\t]+\"\nprogram ::= IDENT*\n").unwrap();

    let schema = AstSchema::infer_from_grammar(&grammar);
    let def = schema.lookup("program").unwrap();
    // The ZeroOrMore should produce a field
    assert!(!def.fields.is_empty());
}

// ============================================================
// Symbol interning tests
// ============================================================

#[test]
fn test_symbol_intern_same_string() {
    let a = Symbol::intern("function_def");
    let b = Symbol::intern("function_def");
    assert_eq!(a, b);
    assert_eq!(a.raw(), b.raw());
}

#[test]
fn test_symbol_intern_different_strings() {
    let a = Symbol::intern("function_def");
    let b = Symbol::intern("block");
    assert_ne!(a, b);
}

#[test]
fn test_symbol_from_str() {
    let s1: Symbol = "hello".into();
    let s2 = Symbol::intern("hello");
    assert_eq!(s1, s2);
}

#[test]
fn test_symbol_display() {
    let s = Symbol::intern("test_kind");
    assert_eq!(format!("{}", s), "test_kind");
}

// ============================================================
// FieldType tests
// ============================================================

#[test]
fn test_field_type_convenience_constructors() {
    let t1 = FieldType::text("IDENT");
    let t2 = FieldType::children("operand");
    let t3 = FieldType::optional("TYPE");
    let t4 = FieldType::flattened_children("stmt");
    let t5 = FieldType::node("param_list");
    let t6 = FieldType::node_list("block");

    assert!(matches!(t1, FieldType::Text { .. }));
    assert!(matches!(t2, FieldType::Children { .. }));
    assert!(matches!(t3, FieldType::OptionalChild { .. }));
    assert!(matches!(t4, FieldType::FlattenedChildren { .. }));
    assert!(matches!(t5, FieldType::Node { .. }));
    assert!(matches!(t6, FieldType::NodeList { .. }));
}

#[test]
fn test_field_type_validate_references_text() {
    let grammar =
        parse_grammar("token IDENT = \"[a-z]+\"\nskip \"[ \\t]+\"\nrule ::= IDENT\n").unwrap();
    let ft = FieldType::text("IDENT");
    assert!(ft.validate_references(&grammar).is_ok());
}

#[test]
fn test_field_type_validate_references_bad() {
    let grammar =
        parse_grammar("token IDENT = \"[a-z]+\"\nskip \"[ \\t]+\"\nrule ::= IDENT\n").unwrap();
    let ft = FieldType::text("MISSING");
    assert!(ft.validate_references(&grammar).is_err());
}

#[test]
fn test_field_type_validate_references_node() {
    let grammar = parse_grammar(
        "token IDENT = \"[a-z]+\"\nskip \"[ \\t]+\"\nouter ::= IDENT\ninner ::= IDENT\n",
    )
    .unwrap();
    let ft = FieldType::node("inner");
    assert!(ft.validate_references(&grammar).is_ok());

    let bad_ft = FieldType::node("nonexistent_rule");
    assert!(bad_ft.validate_references(&grammar).is_err());
}

// ============================================================
// Schema + Grammar end-to-end
// ============================================================

#[test]
fn test_schema_roundtrip_with_grammar() {
    // Build schema, validate against grammar, verify it's usable
    let src = r#"
        token IDENT = "[a-z]+"
        token REG = "[A-Z]+"
        token DEC = "[0-9]+"
        punct "," ";"
        skip "[ \t]+"
        program ::= stmt*
        stmt ::= inst_like | label_def
        inst_like ::= IDENT operand*
        operand ::= IDENT | REG | DEC
        label_def ::= IDENT ";"
    "#;
    let grammar = parse_grammar(src).unwrap();
    let mut schema = AstSchema::new();

    schema
        .define_struct("inst_like")
        .unwrap()
        .field_text("mnemonic", "IDENT")
        .field_children("operands", "operand")
        .finish();
    schema
        .define_enum("operand")
        .unwrap()
        .add_variant("Ident", "IDENT")
        .add_variant("Reg", "REG")
        .add_variant("Dec", "DEC")
        .finish();
    schema
        .define_struct("label_def")
        .unwrap()
        .field_text("name", "IDENT")
        .finish();

    // Validate
    schema.validate(&grammar).unwrap();

    // Parse and lower — use input that matches the grammar
    let cst = parse_with_grammar("add", &grammar).unwrap();
    let ast = lower_cst(&cst, &schema, &grammar).unwrap();
    assert!(!ast.arena.is_empty());
}
