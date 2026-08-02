//! AST 变换器 — bottom-up 节点重写。
//!
//! `AstTransform` trait 允许在遍历过程中重写 AST 节点。
//! 变换是 bottom-up 的：子节点先被变换，父节点后变换。

use crate::ast::{AstArena, AstId, FieldValue, TypedAst};
use std::collections::HashMap;

// ============================================================
// AstTransform trait
// ============================================================

/// AST 变换器 trait — bottom-up 重写。
///
/// 实现此 trait 来进行 AST 优化、脱糖、规范化等变换。
///
/// 注意：`transform` 接收 `AstId` + `&mut AstArena`（而非 `AstRef`）
/// 以避免借用冲突。用 `arena.get(id)` 读取节点数据。
///
/// # 使用示例
///
/// ```ignore
/// struct ConstantFolder;
/// impl AstTransform for ConstantFolder {
///     fn transform(&mut self, id: AstId, arena: &mut AstArena) -> Option<AstId> {
///         let node = arena.get(id)?;
///         if node.kind == "binary_expr" {
///             // Try to fold constants...
///             return Some(new_id);
///         }
///         Some(id) // Keep node unchanged
///     }
/// }
/// ```
pub trait AstTransform {
    /// 变换单个节点。返回：
    /// - `Some(new_id)` — 用新节点替换当前节点（默认：保持不变的 id）
    /// - `None` — 删除当前节点
    ///
    /// 默认实现保持节点不变（返回 `Some(id)`）。
    fn transform(&mut self, id: AstId, _arena: &mut AstArena) -> Option<AstId> {
        Some(id)
    }

    /// 标准的 bottom-up 变换遍历。
    ///
    /// 先变换所有子节点，再变换当前节点。
    /// 如果子节点被删除，父节点的字段会相应更新。
    fn walk_transform(&mut self, id: AstId, ast: &mut TypedAst) -> Option<AstId> {
        // 1. Transform children first (bottom-up)
        self.transform_children(id, ast);

        // 2. Transform current node
        // None → delete, Some(new_id) → replace
        self.transform(id, &mut ast.arena)
    }

    /// 变换所有子节点，就地更新父节点的字段。
    fn transform_children(&mut self, parent_id: AstId, ast: &mut TypedAst) {
        // Collect current child ids
        let child_ids: Vec<AstId> = ast.arena.children(parent_id);

        // Transform each child
        let mut new_ids: Vec<Option<AstId>> = Vec::new();
        for child_id in &child_ids {
            let transformed = self.walk_transform(*child_id, ast);
            new_ids.push(transformed);
        }

        // Update parent's fields with transformed children
        let parent_node = match ast.arena.get_mut(parent_id) {
            Some(n) => n,
            None => return,
        };

        let mut new_fields: HashMap<String, FieldValue> = HashMap::new();
        for (field_name, value) in &parent_node.fields {
            let new_value = match value {
                FieldValue::Text(_) => value.clone(),
                FieldValue::Child(old_id) => {
                    // Find the transformed version of this child
                    if let Some(idx) = child_ids.iter().position(|id| id == old_id) {
                        match new_ids[idx] {
                            Some(new_id) => FieldValue::Child(new_id),
                            None => continue, // child was deleted → skip this field
                        }
                    } else {
                        value.clone()
                    }
                }
                FieldValue::Children(old_ids) => {
                    let mut new_children = Vec::new();
                    for old_id in old_ids {
                        if let Some(idx) = child_ids.iter().position(|id| id == old_id) {
                            if let Some(new_id) = new_ids[idx] {
                                new_children.push(new_id);
                            }
                            // If None, child was deleted → omit
                        } else {
                            new_children.push(*old_id);
                        }
                    }
                    FieldValue::Children(new_children)
                }
                FieldValue::OptionalChild(old_opt) => match old_opt {
                    Some(old_id) => {
                        if let Some(idx) = child_ids.iter().position(|id| id == old_id) {
                            FieldValue::OptionalChild(new_ids[idx])
                        } else {
                            value.clone()
                        }
                    }
                    None => value.clone(),
                },
            };
            new_fields.insert(field_name.clone(), new_value);
        }

        parent_node.fields = new_fields;
    }
}

// ============================================================
// IdentityFolder — 保持所有节点不变（用于派生实现）
// ============================================================

/// 恒等变换器：保持所有节点不变。
///
/// 用作自定义变换器的基类——只需重写 `transform` 方法。
/// 默认的 `AstTransform::transform` 已经返回 `Some(id)`，
/// 所以 `IdentityFolder` 使用默认实现即可。
pub struct IdentityFolder;

impl AstTransform for IdentityFolder {}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::schema::AstSchema;
    use crate::ast::{AstArena, AstNodeData, FieldValue};
    use crate::error::Span;
    use std::collections::HashMap;

    fn make_test_ast() -> TypedAst {
        let mut arena = AstArena::new();
        let schema = AstSchema::new();

        let root = arena.alloc(AstNodeData::new("program", Span::dummy()));

        let mut inst_fields = HashMap::new();
        inst_fields.insert("mnemonic".to_string(), FieldValue::Text("mov".to_string()));
        let inst1 = arena.alloc_with_parent(
            AstNodeData::with_fields("inst", inst_fields, Span::dummy()),
            root,
        );

        let mut inst2_fields = HashMap::new();
        inst2_fields.insert("mnemonic".to_string(), FieldValue::Text("add".to_string()));
        let inst2 = arena.alloc_with_parent(
            AstNodeData::with_fields("inst", inst2_fields, Span::dummy()),
            root,
        );

        arena.get_mut(root).unwrap().fields.insert(
            "stmts".to_string(),
            FieldValue::Children(vec![inst1, inst2]),
        );

        TypedAst::with_root(schema, arena, root)
    }

    #[test]
    fn test_transform_identity() {
        let mut ast = make_test_ast();
        let root_id = ast.root;

        // Apply identity transform
        let result = IdentityFolder.walk_transform(root_id, &mut ast);

        // Root should still exist
        assert!(result.is_some());
        let root_node = ast.arena.get(result.unwrap()).unwrap();
        assert_eq!(root_node.kind, "program");

        // Children should still be there
        let children = ast.arena.children(result.unwrap());
        assert_eq!(children.len(), 2);

        // Both inst nodes should have their text preserved
        for child_id in &children {
            let node = ast.arena.get(*child_id).unwrap();
            assert!(
                node.get_text("mnemonic") == Some("mov")
                    || node.get_text("mnemonic") == Some("add")
            );
        }
    }

    #[test]
    fn test_transform_delete_children() {
        let mut ast = make_test_ast();
        let root_id = ast.root;

        struct DeleteAll;
        impl AstTransform for DeleteAll {
            fn transform(&mut self, id: AstId, arena: &mut AstArena) -> Option<AstId> {
                let node = arena.get(id).unwrap();
                if node.kind == "inst" {
                    None // Delete all inst nodes
                } else {
                    Some(id) // Keep non-inst nodes
                }
            }
        }

        let result = DeleteAll.walk_transform(root_id, &mut ast);
        assert!(result.is_some());

        // Root should have no inst children after deletion
        let children = ast.arena.children(result.unwrap());
        assert!(
            children.is_empty(),
            "Expected 0 children after delete, got {}",
            children.len()
        );
    }
}
