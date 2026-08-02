//! Integration tests for semantic analysis — SymbolTable, NameResolver, scopes.
//!
//! Tests the full semantic pipeline: parse → lower → build scopes → resolve names.

use forge_grammar::semantic::DiagnosticLevel;
use forge_grammar::{
    AnalysisContext, AstSchema, AstVisitor, NameResolver, Parser, ResolutionMap, ScopeKind,
    SemanticDiagnostic, SymbolKind, SymbolTable, TypeInfo, lower_cst, parse_grammar,
};

// ============================================================
// SymbolTable tests
// ============================================================

#[test]
fn test_symbol_table_empty() {
    let table = SymbolTable::new();
    assert!(table.is_empty());
    assert_eq!(table.len(), 0);
    assert!(table.current().is_none());
}

#[test]
fn test_symbol_table_default() {
    let table = SymbolTable::default();
    assert!(table.is_empty());
}

#[test]
fn test_symbol_table_single_scope() {
    let mut table = SymbolTable::new();
    let idx = table.enter(ScopeKind::Global);
    assert_eq!(table.current(), Some(idx));
    assert_eq!(table.len(), 1);

    table.leave();
    assert!(table.current().is_none());
}

#[test]
fn test_symbol_table_define_and_resolve() {
    let mut table = SymbolTable::new();
    table.enter(ScopeKind::Global);
    table.define("x", SymbolKind::Variable, forge_grammar::AstId(1));
    table.define("main", SymbolKind::Function, forge_grammar::AstId(2));

    let x = table.resolve("x").unwrap();
    assert_eq!(x.kind, SymbolKind::Variable);
    assert_eq!(x.def_node.0, 1);

    let main = table.resolve("main").unwrap();
    assert_eq!(main.kind, SymbolKind::Function);

    assert!(table.resolve("y").is_none());
}

#[test]
fn test_symbol_table_define_with_type() {
    let mut table = SymbolTable::new();
    table.enter(ScopeKind::Global);
    table.define_with_type(
        "count",
        SymbolKind::Variable,
        forge_grammar::AstId(5),
        TypeInfo::int("i32", 32),
    );

    let sym = table.resolve("count").unwrap();
    let ty = sym.ty.as_ref().unwrap();
    assert_eq!(ty.name, "i32");
    assert_eq!(ty.width, Some(32));
}

#[test]
fn test_symbol_table_nested_three_levels() {
    let mut table = SymbolTable::new();
    table.enter(ScopeKind::Global);
    table.define("global_x", SymbolKind::Variable, forge_grammar::AstId(1));

    table.enter(ScopeKind::Function);
    table.define("param_y", SymbolKind::Parameter, forge_grammar::AstId(2));

    table.enter(ScopeKind::Block);
    table.define("local_z", SymbolKind::Variable, forge_grammar::AstId(3));

    // All visible from innermost
    assert!(table.resolve("global_x").is_some());
    assert!(table.resolve("param_y").is_some());
    assert!(table.resolve("local_z").is_some());

    // Leave block → local_z hidden
    table.leave();
    assert!(table.resolve("local_z").is_none());
    assert!(table.resolve("param_y").is_some());

    // Leave function → param_y hidden
    table.leave();
    assert!(table.resolve("param_y").is_none());
    assert!(table.resolve("global_x").is_some());
}

#[test]
fn test_symbol_table_shadowing() {
    let mut table = SymbolTable::new();
    table.enter(ScopeKind::Global);
    table.define("x", SymbolKind::Variable, forge_grammar::AstId(1));

    table.enter(ScopeKind::Block);
    table.define("x", SymbolKind::Variable, forge_grammar::AstId(2));

    // Should resolve to inner (shadowing)
    assert_eq!(table.resolve("x").unwrap().def_node.0, 2);

    table.leave();
    assert_eq!(table.resolve("x").unwrap().def_node.0, 1);
}

#[test]
fn test_symbol_table_resolve_with_scope() {
    let mut table = SymbolTable::new();
    table.enter(ScopeKind::Global);
    table.define("x", SymbolKind::Variable, forge_grammar::AstId(1));

    let (sym, scope_idx) = table.resolve_with_scope("x").unwrap();
    assert_eq!(sym.kind, SymbolKind::Variable);
    assert_eq!(scope_idx, 0); // First scope
}

#[test]
fn test_symbol_table_scope_get() {
    let mut table = SymbolTable::new();
    let idx = table.enter(ScopeKind::Custom("test"));
    table.define("item", SymbolKind::Type, forge_grammar::AstId(10));

    let scope = table.get(idx).unwrap();
    assert_eq!(scope.kind, ScopeKind::Custom("test"));
    assert!(scope.lookup_local("item").is_some());
    assert!(scope.lookup_local("missing").is_none());
}

#[test]
fn test_symbol_table_loop_scope() {
    let mut table = SymbolTable::new();
    table.enter(ScopeKind::Global);
    table.enter(ScopeKind::Loop);
    table.define("i", SymbolKind::Variable, forge_grammar::AstId(7));

    assert!(table.resolve("i").is_some());
}

// ============================================================
// ResolutionMap tests
// ============================================================

#[test]
fn test_resolution_map_record_and_query() {
    let mut map = ResolutionMap::new();
    let ref_id = forge_grammar::AstId(10);
    let def_id = forge_grammar::AstId(1);

    map.record(ref_id, def_id);
    assert_eq!(map.definition_of(ref_id), Some(def_id));
    assert_eq!(map.references_to(def_id), &[ref_id]);
    assert!(map.all_resolved());
}

#[test]
fn test_resolution_map_multiple_refs() {
    let mut map = ResolutionMap::new();
    let def_id = forge_grammar::AstId(1);
    map.record(forge_grammar::AstId(10), def_id);
    map.record(forge_grammar::AstId(20), def_id);
    map.record(forge_grammar::AstId(30), def_id);

    assert_eq!(map.references_to(def_id).len(), 3);
}

#[test]
fn test_resolution_map_unresolved() {
    let mut map = ResolutionMap::new();
    map.record_unresolved("missing_fn".to_string(), forge_grammar::AstId(5));
    assert!(!map.all_resolved());
    assert_eq!(map.unresolved.get("missing_fn").unwrap().len(), 1);

    assert!(map.definition_of(forge_grammar::AstId(5)).is_none());
}

// ============================================================
// SemanticDiagnostic tests
// ============================================================

#[test]
fn test_semantic_diagnostic_error() {
    let diag = SemanticDiagnostic::error("type mismatch", forge_grammar::Span::dummy());
    assert_eq!(diag.level, DiagnosticLevel::Error);
    assert_eq!(diag.message, "type mismatch");
}

#[test]
fn test_semantic_diagnostic_warning() {
    let diag = SemanticDiagnostic::warning("unused variable", forge_grammar::Span::dummy());
    assert_eq!(diag.level, DiagnosticLevel::Warning);
}

#[test]
fn test_semantic_diagnostic_at_node() {
    let mut arena = forge_grammar::AstArena::new();
    let schema = AstSchema::new();
    let root = arena.alloc(forge_grammar::AstNodeData::new(
        "test",
        forge_grammar::Span::dummy(),
    ));
    let ast = forge_grammar::TypedAst::with_root(schema, arena, root);

    let diag = SemanticDiagnostic::at_node(DiagnosticLevel::Error, "bad node", root, &ast);
    assert_eq!(diag.level, DiagnosticLevel::Error);
    assert_eq!(diag.node, Some(root));
}

// ============================================================
// AnalysisContext tests
// ============================================================

#[test]
fn test_analysis_context_errors_and_warnings() {
    let schema = AstSchema::new();
    let ast = forge_grammar::TypedAst::new(schema);
    let mut cx = AnalysisContext::new(ast);

    assert!(!cx.has_errors());

    cx.error("err1", forge_grammar::Span::dummy());
    assert!(cx.has_errors());

    cx.warn("warn1", forge_grammar::Span::dummy());
    assert_eq!(cx.diagnostics.len(), 2);
}

// ============================================================
// NameResolver integration (full pipeline)
// ============================================================

#[test]
fn test_name_resolver_ir_function() {
    // Parse a simple IR function through the full pipeline
    let ir_src = include_str!("../../../foundation/forge-ir/grammars/ir.lx");
    let grammar = parse_grammar(ir_src).unwrap();

    let mut schema = AstSchema::new();
    schema
        .define_struct("function_def")
        .unwrap()
        .field_text("name", "IDENT")
        .field_node("param_list", "param_list")
        .field_node("block_list", "block_list")
        .finish();
    schema
        .define_struct("param_list")
        .unwrap()
        .field_children("params", "param")
        .finish();
    schema
        .define_struct("block_list")
        .unwrap()
        .field_children("blocks", "block")
        .finish();
    schema
        .define_struct("block")
        .unwrap()
        .field_text("name", "IDENT")
        .finish();
    schema
        .define_struct("param")
        .unwrap()
        .field_text("name", "IDENT")
        .finish();

    let parser = Parser::build(grammar.clone());
    let cst = parser
        .parse("fn main(x: i32) -> i32 { entry: ret i32 }")
        .unwrap();
    let ast = lower_cst(&cst, &schema, &grammar).unwrap();

    let ast_walk = ast.clone();
    let mut cx = AnalysisContext::new(ast);
    let mut resolver = NameResolver::new(&mut cx);
    resolver.walk(&ast_walk);

    let _resolution = resolver.finish();
    // The resolver ran without panic — pipeline integration verified
}

// ============================================================
// TypeInfo tests
// ============================================================

#[test]
fn test_type_info_basic_types() {
    let i32_ty = TypeInfo::int("i32", 32);
    assert_eq!(i32_ty.name, "i32");
    assert_eq!(i32_ty.width, Some(32));
    assert!(!i32_ty.is_float);
    assert!(!i32_ty.is_ptr);

    let f64_ty = TypeInfo::float("f64", 64);
    assert!(f64_ty.is_float);

    let ptr_ty = TypeInfo::ptr("i32");
    assert!(ptr_ty.is_ptr);
    assert_eq!(ptr_ty.name, "*i32");

    let named_ty = TypeInfo::named("mytype");
    assert_eq!(named_ty.width, None);
}

// ============================================================
// ScopeKind tests
// ============================================================

#[test]
fn test_scope_kind_all_variants() {
    let kinds = [
        ScopeKind::Global,
        ScopeKind::Function,
        ScopeKind::Block,
        ScopeKind::Loop,
        ScopeKind::Custom("namespace"),
    ];
    for kind in &kinds {
        let mut table = SymbolTable::new();
        table.enter(*kind);
        assert_eq!(table.len(), 1);
    }
}

#[test]
fn test_scope_kind_custom() {
    let kind = ScopeKind::Custom("class_body");
    let mut table = SymbolTable::new();
    table.enter(kind);
    table.define("field", SymbolKind::Variable, forge_grammar::AstId(42));

    let scope = table.get(0).unwrap();
    assert_eq!(scope.kind, ScopeKind::Custom("class_body"));
    assert!(scope.lookup_local("field").is_some());
}
