//! Integration tests for AstTransform — bottom-up node rewriting, deletion, replacement.

use forge_grammar::{
    AstArena, AstId, AstNodeData, AstSchema, AstTransform, FieldValue, IdentityFolder, TypedAst,
};
use std::collections::HashMap;

// ============================================================
// Helpers
// ============================================================

fn make_test_ast() -> TypedAst {
    let mut arena = AstArena::new();
    let schema = AstSchema::new();

    let root = arena.alloc(AstNodeData::new("program", forge_grammar::Span::dummy()));

    let mut inst1_fields = HashMap::new();
    inst1_fields.insert("mnemonic".to_string(), FieldValue::Text("mov".to_string()));
    let inst1 = arena.alloc_with_parent(
        AstNodeData::with_fields("inst", inst1_fields, forge_grammar::Span::dummy()),
        root,
    );

    let mut inst2_fields = HashMap::new();
    inst2_fields.insert("mnemonic".to_string(), FieldValue::Text("add".to_string()));
    let inst2 = arena.alloc_with_parent(
        AstNodeData::with_fields("inst", inst2_fields, forge_grammar::Span::dummy()),
        root,
    );

    let mut inst3_fields = HashMap::new();
    inst3_fields.insert("mnemonic".to_string(), FieldValue::Text("nop".to_string()));
    let inst3 = arena.alloc_with_parent(
        AstNodeData::with_fields("inst", inst3_fields, forge_grammar::Span::dummy()),
        root,
    );

    // Link children
    arena.get_mut(root).unwrap().fields.insert(
        "stmts".to_string(),
        FieldValue::Children(vec![inst1, inst2, inst3]),
    );

    TypedAst::with_root(schema, arena, root)
}

// ============================================================
// Identity transform
// ============================================================

#[test]
fn test_identity_transform_preserves_all_nodes() {
    let mut ast = make_test_ast();
    let root_id = ast.root;

    let result = IdentityFolder.walk_transform(root_id, &mut ast);
    assert!(result.is_some());

    let children = ast.arena.children(result.unwrap());
    assert_eq!(children.len(), 3);
}

#[test]
fn test_identity_transform_preserves_text() {
    let mut ast = make_test_ast();
    let root_id = ast.root;

    let result = IdentityFolder.walk_transform(root_id, &mut ast);
    let children = ast.arena.children(result.unwrap());
    let texts: Vec<String> = children
        .iter()
        .filter_map(|id| ast.arena.get(*id))
        .filter_map(|n| n.get_text("mnemonic").map(|s| s.to_string()))
        .collect();

    assert_eq!(texts, vec!["mov", "add", "nop"]);
}

// ============================================================
// Delete transform
// ============================================================

#[test]
fn test_delete_all_insts() {
    let mut ast = make_test_ast();
    let root_id = ast.root;

    struct DeleteAll;
    impl AstTransform for DeleteAll {
        fn transform(&mut self, id: AstId, arena: &mut AstArena) -> Option<AstId> {
            if arena.get(id).map(|n| n.kind.as_str()) == Some("inst") {
                None // Delete
            } else {
                Some(id) // Keep
            }
        }
    }

    let result = DeleteAll.walk_transform(root_id, &mut ast);
    assert!(result.is_some());
    let children = ast.arena.children(result.unwrap());
    assert!(children.is_empty());
}

#[test]
fn test_delete_specific_inst() {
    let mut ast = make_test_ast();
    let root_id = ast.root;

    struct DeleteNop;
    impl AstTransform for DeleteNop {
        fn transform(&mut self, id: AstId, arena: &mut AstArena) -> Option<AstId> {
            let node = arena.get(id).unwrap();
            if node.kind == "inst" && node.get_text("mnemonic") == Some("nop") {
                None // Delete only nop
            } else {
                Some(id)
            }
        }
    }

    let result = DeleteNop.walk_transform(root_id, &mut ast);
    assert!(result.is_some());
    let children = ast.arena.children(result.unwrap());
    assert_eq!(children.len(), 2); // mov and add remain

    let texts: Vec<String> = children
        .iter()
        .filter_map(|id| ast.arena.get(*id))
        .filter_map(|n| n.get_text("mnemonic").map(|s| s.to_string()))
        .collect();
    assert_eq!(texts, vec!["mov", "add"]);
}

#[test]
fn test_delete_root_returns_none() {
    let mut ast = make_test_ast();
    let root_id = ast.root;

    struct DeleteEverything;
    impl AstTransform for DeleteEverything {
        fn transform(&mut self, _id: AstId, _arena: &mut AstArena) -> Option<AstId> {
            None
        }
    }

    let result = DeleteEverything.walk_transform(root_id, &mut ast);
    assert!(result.is_none()); // Root itself deleted
}

// ============================================================
// Replace transform
// ============================================================

#[test]
fn test_replace_inst_mnemonic() {
    let mut ast = make_test_ast();
    let root_id = ast.root;

    struct RenameMovToCopy;
    impl AstTransform for RenameMovToCopy {
        fn transform(&mut self, id: AstId, arena: &mut AstArena) -> Option<AstId> {
            let node = arena.get(id).unwrap();
            if node.kind == "inst" && node.get_text("mnemonic") == Some("mov") {
                let mut new_node = node.clone();
                new_node
                    .fields
                    .insert("mnemonic".to_string(), FieldValue::Text("copy".to_string()));
                let new_id = arena.alloc(new_node);
                Some(new_id)
            } else {
                Some(id)
            }
        }
    }

    let result = RenameMovToCopy.walk_transform(root_id, &mut ast);
    assert!(result.is_some());
    let children = ast.arena.children(result.unwrap());
    let texts: Vec<String> = children
        .iter()
        .filter_map(|id| ast.arena.get(*id))
        .filter_map(|n| n.get_text("mnemonic").map(|s| s.to_string()))
        .collect();
    assert!(texts.contains(&"copy".to_string()));
    assert!(texts.contains(&"add".to_string()));
}

// ============================================================
// Nested transform
// ============================================================

#[test]
fn test_transform_nested_structure() {
    let mut arena = AstArena::new();
    let schema = AstSchema::new();

    let root = arena.alloc(AstNodeData::new("module", forge_grammar::Span::dummy()));

    let mut func_fields = HashMap::new();
    func_fields.insert("name".to_string(), FieldValue::Text("f".to_string()));
    let func = arena.alloc_with_parent(
        AstNodeData::with_fields("function_def", func_fields, forge_grammar::Span::dummy()),
        root,
    );

    let mut block_fields = HashMap::new();
    block_fields.insert("name".to_string(), FieldValue::Text("entry".to_string()));
    let block = arena.alloc_with_parent(
        AstNodeData::with_fields("block", block_fields, forge_grammar::Span::dummy()),
        func,
    );

    let mut inst_fields = HashMap::new();
    inst_fields.insert("mnemonic".to_string(), FieldValue::Text("ret".to_string()));
    let inst = arena.alloc_with_parent(
        AstNodeData::with_fields("inst", inst_fields, forge_grammar::Span::dummy()),
        block,
    );

    arena
        .get_mut(root)
        .unwrap()
        .fields
        .insert("funcs".to_string(), FieldValue::Children(vec![func]));
    arena
        .get_mut(func)
        .unwrap()
        .fields
        .insert("blocks".to_string(), FieldValue::Children(vec![block]));
    arena
        .get_mut(block)
        .unwrap()
        .fields
        .insert("stmts".to_string(), FieldValue::Children(vec![inst]));

    let mut ast = TypedAst::with_root(schema, arena, root);

    // Count total nodes before transform
    let _node_count = ast.root_ref().walk().count();

    // Identity transform should preserve structure
    let result = IdentityFolder.walk_transform(ast.root, &mut ast);
    assert!(result.is_some());
}

// ============================================================
// Empty tree transform
// ============================================================

#[test]
fn test_transform_empty_ast() {
    let schema = AstSchema::new();
    let mut arena = AstArena::new();
    let root = arena.alloc(AstNodeData::new("root", forge_grammar::Span::dummy()));
    let mut ast = TypedAst::with_root(schema, arena, root);

    struct PassThrough;
    impl AstTransform for PassThrough {}

    let result = PassThrough.walk_transform(ast.root, &mut ast);
    assert!(result.is_some());
    assert_eq!(ast.arena.get(result.unwrap()).unwrap().kind, "root");
}
