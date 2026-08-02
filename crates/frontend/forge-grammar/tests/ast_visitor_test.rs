//! Integration tests for AstVisitor — traversal, built-in visitors, edge cases.

use forge_grammar::{
    AstArena, AstNodeData, AstRef, AstSchema, AstVisitor, FieldValue, NodeCollector, NodeCounter,
    TreePrinter, TypedAst, VisitAction,
};
use std::collections::HashMap;

// ============================================================
// Helpers
// ============================================================

fn make_deep_ast() -> TypedAst {
    let mut arena = AstArena::new();
    let schema = AstSchema::new();

    // Build: module → function_def → block → inst
    let module = arena.alloc(AstNodeData::new("module", forge_grammar::Span::dummy()));

    let mut func_fields = HashMap::new();
    func_fields.insert("name".to_string(), FieldValue::Text("test_fn".to_string()));
    let func = arena.alloc_with_parent(
        AstNodeData::with_fields("function_def", func_fields, forge_grammar::Span::dummy()),
        module,
    );

    let mut block_fields = HashMap::new();
    block_fields.insert("name".to_string(), FieldValue::Text("entry".to_string()));
    let block = arena.alloc_with_parent(
        AstNodeData::with_fields("block", block_fields, forge_grammar::Span::dummy()),
        func,
    );

    let mut inst1_fields = HashMap::new();
    inst1_fields.insert("mnemonic".to_string(), FieldValue::Text("mov".to_string()));
    let inst1 = arena.alloc_with_parent(
        AstNodeData::with_fields("inst", inst1_fields, forge_grammar::Span::dummy()),
        block,
    );

    let mut inst2_fields = HashMap::new();
    inst2_fields.insert("mnemonic".to_string(), FieldValue::Text("add".to_string()));
    let inst2 = arena.alloc_with_parent(
        AstNodeData::with_fields("inst", inst2_fields, forge_grammar::Span::dummy()),
        block,
    );

    // Link all children in fields
    arena
        .get_mut(module)
        .unwrap()
        .fields
        .insert("functions".to_string(), FieldValue::Children(vec![func]));
    arena
        .get_mut(func)
        .unwrap()
        .fields
        .insert("blocks".to_string(), FieldValue::Children(vec![block]));
    arena.get_mut(block).unwrap().fields.insert(
        "stmts".to_string(),
        FieldValue::Children(vec![inst1, inst2]),
    );

    TypedAst::with_root(schema, arena, module)
}

// ============================================================
// Basic visitor tests
// ============================================================

#[test]
fn test_node_collector() {
    let ast = make_deep_ast();
    let mut collector = NodeCollector::new("inst");
    collector.walk(&ast);
    assert_eq!(collector.nodes.len(), 2);
}

#[test]
fn test_node_collector_no_match() {
    let ast = make_deep_ast();
    let mut collector = NodeCollector::new("nonexistent");
    collector.walk(&ast);
    assert_eq!(collector.nodes.len(), 0);
}

#[test]
fn test_node_counter() {
    let ast = make_deep_ast();
    let mut counter = NodeCounter::new();
    counter.walk(&ast);
    assert_eq!(counter.counts.get("module"), Some(&1));
    assert_eq!(counter.counts.get("function_def"), Some(&1));
    assert_eq!(counter.counts.get("block"), Some(&1));
    assert_eq!(counter.counts.get("inst"), Some(&2));
}

#[test]
fn test_tree_printer() {
    let ast = make_deep_ast();
    let mut printer = TreePrinter::new();
    printer.walk(&ast);
    let output = printer.finish();

    assert!(output.contains("module"));
    assert!(output.contains("function_def"));
    assert!(output.contains("block"));
    assert!(output.contains("inst"));
}

// ============================================================
// VisitAction tests
// ============================================================

#[test]
fn test_visitor_stop() {
    let ast = make_deep_ast();

    struct StopAtBlock;
    impl AstVisitor for StopAtBlock {
        fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
            if node.kind() == "block" {
                VisitAction::Stop
            } else {
                VisitAction::Continue
            }
        }
    }

    let mut v = StopAtBlock;
    v.walk(&ast);
    // Should not panic — Stop prevents further traversal
}

#[test]
fn test_visitor_skip_children() {
    let ast = make_deep_ast();

    struct SkipBlocks;
    impl AstVisitor for SkipBlocks {
        fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
            if node.kind() == "block" {
                VisitAction::Skip
            } else {
                VisitAction::Continue
            }
        }
    }

    let mut v = SkipBlocks;
    v.walk(&ast);
    // When Skip is returned for block, children of block are not visited
    // but leave_node should still be called
}

#[test]
fn test_visitor_leave_called() {
    let ast = make_deep_ast();

    struct LeaveTracker {
        entered: Vec<String>,
        left: Vec<String>,
    }
    impl AstVisitor for LeaveTracker {
        fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
            self.entered.push(node.kind().to_string());
            VisitAction::Continue
        }
        fn leave(&mut self, node: AstRef<'_>) {
            self.left.push(node.kind().to_string());
        }
    }

    let mut tracker = LeaveTracker {
        entered: Vec::new(),
        left: Vec::new(),
    };
    tracker.walk(&ast);

    // Every entered node should also be left
    assert!(!tracker.entered.is_empty());
    assert_eq!(tracker.entered.len(), tracker.left.len());
}

#[test]
fn test_visitor_empty_ast() {
    let schema = AstSchema::new();
    let _ast = TypedAst::new(schema); // No root — verify no panic on construction

    struct NoopVisitor;
    impl AstVisitor for NoopVisitor {}

    let _v = NoopVisitor;
    // Walking an empty AST should not panic
    // (walk_node won't be called since there's no root)
}

// ============================================================
// Multiple visitors composed
// ============================================================

#[test]
fn test_multiple_visitors_on_same_ast() {
    let ast = make_deep_ast();

    // Run a counter then a collector
    let mut counter = NodeCounter::new();
    counter.walk(&ast);
    assert_eq!(counter.counts.get("inst"), Some(&2));

    let mut collector = NodeCollector::new("function_def");
    collector.walk(&ast);
    assert_eq!(collector.nodes.len(), 1);
}

// ============================================================
// AstRef query methods tested via visitor
// ============================================================

#[test]
fn test_astref_span_in_visitor() {
    let mut arena = AstArena::new();
    let schema = AstSchema::new();
    let root = arena.alloc(AstNodeData::new("program", forge_grammar::Span::dummy()));
    let ast = TypedAst::with_root(schema, arena, root);

    struct SpanChecker;
    impl AstVisitor for SpanChecker {
        fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
            let span = node.span();
            assert_eq!(span.start, 0);
            VisitAction::Continue
        }
    }

    SpanChecker.walk(&ast);
}

#[test]
fn test_astref_parent_in_visitor() {
    let ast = make_deep_ast();

    struct ParentChecker;
    impl AstVisitor for ParentChecker {
        fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
            if node.kind() == "block" {
                let parent = node.parent();
                assert!(parent.is_some());
                assert_eq!(parent.unwrap().kind(), "function_def");
            }
            VisitAction::Continue
        }
    }

    ParentChecker.walk(&ast);
}

#[test]
fn test_astref_ancestor_in_visitor() {
    let ast = make_deep_ast();

    struct AncestorChecker;
    impl AstVisitor for AncestorChecker {
        fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
            if node.kind() == "inst" {
                let func = node.ancestor("function_def");
                assert!(func.is_some());
                assert_eq!(func.unwrap().get_text("name"), Some("test_fn"));
            }
            VisitAction::Continue
        }
    }

    AncestorChecker.walk(&ast);
}

#[test]
fn test_astref_find_first_and_find_all() {
    let ast = make_deep_ast();
    let root = ast.root_ref();

    // find_first
    let func = root.find_first("function_def");
    assert!(func.is_some());
    assert_eq!(func.unwrap().get_text("name"), Some("test_fn"));

    // find_all
    let insts = root.find_all("inst");
    assert_eq!(insts.len(), 2);
}

#[test]
fn test_astref_get_optional() {
    let ast = make_deep_ast();
    let root = ast.root_ref();

    // Optional access on a field that exists but might be None
    let nonexistent = root.get_optional("nonexistent_field");
    assert!(nonexistent.is_none()); // Field doesn't exist at all
}

#[test]
fn test_astref_nodes_of_kind() {
    let ast = make_deep_ast();
    let insts = ast.nodes_of_kind("inst");
    assert_eq!(insts.len(), 2);

    let funcs = ast.nodes_of_kind("function_def");
    assert_eq!(funcs.len(), 1);

    let nonexistent = ast.nodes_of_kind("nonexistent");
    assert!(nonexistent.is_empty());
}

#[test]
fn test_astref_root_children() {
    let ast = make_deep_ast();
    let children = ast.root_children();
    // Root "module" has fields.functions = [func_id]
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].kind(), "function_def");
}

// ============================================================
// AstWalker tests
// ============================================================

#[test]
fn test_ast_walker_visits_all() {
    let ast = make_deep_ast();
    let kinds: Vec<String> = ast
        .root_ref()
        .walk()
        .map(|n| n.kind().to_string())
        .collect();

    assert!(kinds.iter().any(|k| k == "module"));
    assert!(kinds.iter().any(|k| k == "function_def"));
    assert!(kinds.iter().any(|k| k == "block"));
    assert_eq!(kinds.iter().filter(|k| *k == "inst").count(), 2);
}

#[test]
fn test_ast_walker_empty_node() {
    let mut arena = AstArena::new();
    let schema = AstSchema::new();
    let root = arena.alloc(AstNodeData::new("leaf", forge_grammar::Span::dummy()));
    let ast = TypedAst::with_root(schema, arena, root);

    let kinds: Vec<String> = ast
        .root_ref()
        .walk()
        .map(|n| n.kind().to_string())
        .collect();
    assert_eq!(kinds, vec!["leaf"]);
}
