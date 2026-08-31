//! AST 类型系统 — 运行时 arena-based 的抽象语法树。
//!
//! # 核心类型
//!
//! - [`AstId`] — 轻量级节点引用（Copy, 等价于 arena index）
//! - [`AstArena`] — 拥有所有节点的 arena 存储
//! - [`AstNodeData`] — 单个节点的运行时数据
//! - [`TypedAst`] — arena + schema 的组合，提供类型安全访问
//! - [`AstRef`] — 对 AstId + TypedAst 的引用，提供字段访问 API
//!
//! # 设计
//!
//! 与 CST 不同，AST 节点有**结构化字段**（由 [`AstSchema`] 定义）。
//! 字段通过名称访问（`get_text("name")`, `get_children("params")`），
//! 而非遍历原始子节点。这使得语义分析代码不依赖语法树的偶然结构。

pub mod lower;
pub mod node;
pub mod schema;

use crate::error::Span;
use std::collections::HashMap;
use std::fmt;

use schema::AstSchema;

// ============================================================
// AstId
// ============================================================

/// AST 节点的唯一标识 — 等价于 arena 中的索引。
///
/// `Copy` + 轻量级（usize），适合在 HashMap 和 Vec 中大量使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AstId(pub usize);

impl AstId {
    /// 哨兵值：表示无效/缺失的节点引用。
    pub const INVALID: AstId = AstId(usize::MAX);
}

impl fmt::Display for AstId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == AstId::INVALID {
            write!(f, "AstId(INVALID)")
        } else {
            write!(f, "AstId({})", self.0)
        }
    }
}

// ============================================================
// FieldValue
// ============================================================

/// AST 节点中单个字段的值。
///
/// 区分文本字段、单个子节点、多个子节点、可选子节点。
#[derive(Debug, Clone)]
pub enum FieldValue {
    /// 文本字段（从 token text 提取）。
    Text(String),
    /// 单个子节点引用。
    Child(AstId),
    /// 多个子节点引用（保持插入顺序）。
    Children(Vec<AstId>),
    /// 可选子节点。
    OptionalChild(Option<AstId>),
}

impl FieldValue {
    /// 如果这是 Text variant，返回其内容。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            FieldValue::Text(s) => Some(s),
            _ => None,
        }
    }

    /// 如果这是 Child variant，返回其 AstId。
    pub fn as_child(&self) -> Option<AstId> {
        match self {
            FieldValue::Child(id) => Some(*id),
            _ => None,
        }
    }

    /// 如果这是 Children variant，返回其切片。
    pub fn as_children(&self) -> Option<&[AstId]> {
        match self {
            FieldValue::Children(ids) => Some(ids),
            _ => None,
        }
    }

    /// 如果这是 OptionalChild variant，返回其内容。
    pub fn as_optional(&self) -> Option<&Option<AstId>> {
        match self {
            FieldValue::OptionalChild(opt) => Some(opt),
            _ => None,
        }
    }
}

// ============================================================
// AstNodeData
// ============================================================

/// 单个 AST 节点的运行时数据。
#[derive(Debug, Clone)]
pub struct AstNodeData {
    /// 节点 kind（对应 schema 中的 rule_name 或 enum variant）。
    pub kind: String,
    /// 结构化字段（字段名 → 字段值）。
    pub fields: HashMap<String, FieldValue>,
    /// 源位置 span。
    pub span: Span,
    /// 父节点（用于向上遍历；根节点为 None）。
    pub parent: Option<AstId>,
}

impl AstNodeData {
    /// 创建一个新的 AST 节点。
    pub fn new(kind: impl Into<String>, span: Span) -> Self {
        Self {
            kind: kind.into(),
            fields: HashMap::new(),
            span,
            parent: None,
        }
    }

    /// 创建一个带字段的 AST 节点。
    pub fn with_fields(
        kind: impl Into<String>,
        fields: HashMap<String, FieldValue>,
        span: Span,
    ) -> Self {
        Self {
            kind: kind.into(),
            fields,
            span,
            parent: None,
        }
    }

    /// 获取指定字段的文本值。
    pub fn get_text(&self, field: &str) -> Option<&str> {
        self.fields.get(field)?.as_text()
    }

    /// 获取指定字段的子节点 ID。
    pub fn get_child(&self, field: &str) -> Option<AstId> {
        self.fields.get(field)?.as_child()
    }

    /// 获取指定字段的子节点列表。
    pub fn get_children(&self, field: &str) -> Option<&[AstId]> {
        self.fields.get(field)?.as_children()
    }

    /// 是否有指定名称的字段。
    pub fn has_field(&self, field: &str) -> bool {
        self.fields.contains_key(field)
    }
}

// ============================================================
// AstArena
// ============================================================

/// 所有 AST 节点的 arena 存储。
///
/// Arena 拥有所有节点；`AstId` 是 arena 索引，可自由 Copy。
/// 支持 parent → children 和 children → parent 双向遍历。
#[derive(Debug, Clone, Default)]
pub struct AstArena {
    nodes: Vec<AstNodeData>,
}

impl AstArena {
    /// 创建空 arena。
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// 分配一个新节点，返回其 AstId。
    ///
    /// 如果提供了 `parent`，会自动设置节点的 parent 链接。
    pub fn alloc(&mut self, node: AstNodeData) -> AstId {
        let id = AstId(self.nodes.len());
        // parent 如果已被外部设置则保留（AstNodeData 的 parent 默认即 None，无需修改）
        self.nodes.push(node);
        id
    }

    /// 分配一个节点并设置其父节点。
    pub fn alloc_with_parent(&mut self, mut node: AstNodeData, parent: AstId) -> AstId {
        node.parent = Some(parent);
        self.alloc(node)
    }

    /// 获取指定 AstId 的节点数据引用。
    #[inline]
    pub fn get(&self, id: AstId) -> Option<&AstNodeData> {
        self.nodes.get(id.0)
    }

    /// 获取指定 AstId 的节点数据可变引用。
    #[inline]
    pub fn get_mut(&mut self, id: AstId) -> Option<&mut AstNodeData> {
        self.nodes.get_mut(id.0)
    }

    /// 获取节点的父节点。
    pub fn parent(&self, id: AstId) -> Option<AstId> {
        self.get(id)?.parent
    }

    /// 获取节点的所有直接子节点（从字段中收集）。
    /// P0-12 修复：按字段名排序保证确定性（fields 是 HashMap，原实现遍历
    /// 随机序 → 生成顺序跨进程漂移、回归不可复现）。
    pub fn children(&self, id: AstId) -> Vec<AstId> {
        let mut result = Vec::new();
        if let Some(node) = self.get(id) {
            let mut field_names: Vec<&String> = node.fields.keys().collect();
            field_names.sort();
            for field_name in field_names {
                match node.fields.get(field_name) {
                    Some(FieldValue::Text(_)) => {}
                    Some(FieldValue::Child(child_id)) => result.push(*child_id),
                    Some(FieldValue::Children(child_ids)) => result.extend(child_ids),
                    Some(FieldValue::OptionalChild(Some(child_id))) => {
                        result.push(*child_id);
                    }
                    _ => {}
                }
            }
        }
        result
    }

    /// 获取节点的所有直接子节点（包括其字段名，便于调试）。
    /// P0-12 修复：按字段名排序保证确定性。
    pub fn children_named(&self, id: AstId) -> Vec<(String, AstId)> {
        let mut result = Vec::new();
        if let Some(node) = self.get(id) {
            let mut field_names: Vec<&String> = node.fields.keys().collect();
            field_names.sort();
            for field_name in field_names {
                match node.fields.get(field_name) {
                    Some(FieldValue::Text(_)) => {}
                    Some(FieldValue::Child(child_id)) => {
                        result.push((field_name.clone(), *child_id));
                    }
                    Some(FieldValue::Children(child_ids)) => {
                        for child_id in child_ids {
                            result.push((field_name.clone(), *child_id));
                        }
                    }
                    Some(FieldValue::OptionalChild(Some(child_id))) => {
                        result.push((field_name.clone(), *child_id));
                    }
                    _ => {}
                }
            }
        }
        result
    }

    /// 设置节点的父链接（在 lowering 完成后的 fixup pass 中调用）。
    pub fn set_parent(&mut self, child_id: AstId, parent_id: AstId) {
        if let Some(node) = self.get_mut(child_id) {
            node.parent = Some(parent_id);
        }
    }

    /// 修复所有节点的 parent 链接（从根节点开始遍历）。
    /// 在 lowering 完成后调用一次。
    pub fn fixup_parents(&mut self, root: AstId) {
        // 根节点无 parent
        if let Some(root_node) = self.get_mut(root) {
            root_node.parent = None;
        }
        let mut worklist: Vec<(AstId, Option<AstId>)> = vec![(root, None)];
        while let Some((id, parent)) = worklist.pop() {
            if let Some(node) = self.get_mut(id) {
                node.parent = parent;
            }
            let child_ids = self.children(id);
            for child_id in child_ids {
                worklist.push((child_id, Some(id)));
            }
        }
    }

    /// Arena 中的节点总数。
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Arena 是否为空。
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// 遍历所有节点。
    pub fn iter(&self) -> impl Iterator<Item = (AstId, &AstNodeData)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (AstId(i), node))
    }
}

// ============================================================
// TypedAst
// ============================================================

/// 类型化的 AST — arena + schema + 根节点的组合。
///
/// 这是 lowering 后的主要数据结构。所有后续分析（visitor、
/// semantic）都通过 `TypedAst` 进行。
#[derive(Debug, Clone)]
pub struct TypedAst {
    /// 节点存储。
    pub arena: AstArena,
    /// AST 形状定义。
    pub schema: AstSchema,
    /// 根节点 ID（通常是 "program" 或 "module" 规则）。
    pub root: AstId,
}

impl TypedAst {
    /// 创建带 schema 的空 TypedAst（用于手动构建）。
    pub fn new(schema: AstSchema) -> Self {
        Self {
            arena: AstArena::new(),
            schema,
            root: AstId::INVALID,
        }
    }

    /// 创建完整的 TypedAst。
    pub fn with_root(schema: AstSchema, arena: AstArena, root: AstId) -> Self {
        Self {
            arena,
            schema,
            root,
        }
    }

    /// 获取根节点的 AstRef。
    pub fn root_ref(&self) -> AstRef<'_> {
        AstRef {
            id: self.root,
            ast: self,
        }
    }

    /// 获取指定 AstId 的 AstRef。
    pub fn ref_to(&self, id: AstId) -> AstRef<'_> {
        AstRef { id, ast: self }
    }

    /// 遍历所有 kind 匹配的节点。
    pub fn nodes_of_kind(&self, kind: &str) -> Vec<AstRef<'_>> {
        self.arena
            .iter()
            .filter(|(_, node)| node.kind == kind)
            .map(|(id, _)| self.ref_to(id))
            .collect()
    }

    /// 获取所有根级子节点（root 的 children 字段中的节点）。
    pub fn root_children(&self) -> Vec<AstRef<'_>> {
        self.root_ref().children()
    }
}

// ============================================================
// AstRef
// ============================================================

/// 对 TypedAst 中某个节点的引用，提供类型安全的结构化访问。
///
/// `AstRef` 是轻量级 Copy 类型（两个指针），通过它访问节点字段
/// 而非直接操作 `AstNodeData`。
#[derive(Clone, Copy)]
pub struct AstRef<'a> {
    /// 节点 ID。
    pub id: AstId,
    /// 所属的 TypedAst。
    pub ast: &'a TypedAst,
}

impl<'a> fmt::Debug for AstRef<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AstRef")
            .field("id", &self.id)
            .field("kind", &self.kind())
            .finish()
    }
}

impl<'a> AstRef<'a> {
    /// 获取节点 kind（对应 grammar rule 名或 token 名）。
    pub fn kind(&self) -> &str {
        self.ast
            .arena
            .get(self.id)
            .map(|n| n.kind.as_str())
            .unwrap_or("<invalid>")
    }

    /// 节点原始 span。
    pub fn span(&self) -> Span {
        self.ast
            .arena
            .get(self.id)
            .map(|n| n.span.clone())
            .unwrap_or_else(Span::dummy)
    }

    // ── 字段访问 ──

    /// 获取指定字段的文本值。
    /// 如果字段不存在或不是 Text 类型，返回 None。
    pub fn get_text(&self, field: &str) -> Option<&str> {
        self.ast.arena.get(self.id)?.get_text(field)
    }

    /// 获取指定字段的单个子节点。
    pub fn get_child(&self, field: &str) -> Option<AstRef<'a>> {
        let child_id = self.ast.arena.get(self.id)?.get_child(field)?;
        Some(self.ast.ref_to(child_id))
    }

    /// 获取指定字段的子节点列表。
    pub fn get_children(&self, field: &str) -> Vec<AstRef<'a>> {
        self.ast
            .arena
            .get(self.id)
            .and_then(|n| n.get_children(field))
            .map(|ids| ids.iter().map(|&id| self.ast.ref_to(id)).collect())
            .unwrap_or_default()
    }

    /// 获取指定字段的可选子节点。
    pub fn get_optional(&self, field: &str) -> Option<Option<AstRef<'a>>> {
        let opt = self
            .ast
            .arena
            .get(self.id)?
            .fields
            .get(field)?
            .as_optional()?;
        Some(opt.map(|id| self.ast.ref_to(id)))
    }

    // ── 节点内容 ──

    /// 节点的原始 token 文本（当节点是终端/叶子节点时）。
    /// 从 "text" 字段获取（enum lowering 会在 text 字段存储 token text）。
    pub fn text(&self) -> Option<&str> {
        self.get_text("text")
    }

    /// 对于 enum 节点：获取当前 variant 名称。
    /// 从 "variant" 字段获取。
    pub fn variant(&self) -> Option<&str> {
        self.get_text("variant")
    }

    /// 节点的所有直接子节点（从所有字段中收集）。
    pub fn children(&self) -> Vec<AstRef<'a>> {
        self.ast
            .arena
            .children(self.id)
            .into_iter()
            .map(|id| self.ast.ref_to(id))
            .collect()
    }

    /// 所有子节点（带字段名，owned）。
    pub fn children_named(&self) -> Vec<(String, AstRef<'a>)> {
        self.ast
            .arena
            .children_named(self.id)
            .into_iter()
            .map(|(name, id)| (name, self.ast.ref_to(id)))
            .collect()
    }

    // ── 树遍历 ──

    /// 获取父节点。
    pub fn parent(&self) -> Option<AstRef<'a>> {
        let parent_id = self.ast.arena.parent(self.id)?;
        Some(self.ast.ref_to(parent_id))
    }

    /// 查找第一个匹配 kind 的祖先节点（包含自身）。
    pub fn ancestor(&self, kind: &str) -> Option<AstRef<'a>> {
        if self.kind() == kind {
            return Some(*self);
        }
        let mut current = self.parent();
        while let Some(node) = current {
            if node.kind() == kind {
                return Some(node);
            }
            current = node.parent();
        }
        None
    }

    /// 深度优先遍历此子树中的所有节点。
    pub fn walk(&self) -> AstWalker<'a> {
        AstWalker { stack: vec![*self] }
    }

    /// 收集此子树中所有 kind 匹配的节点。
    pub fn find_all(&self, kind: &str) -> Vec<AstRef<'a>> {
        self.walk().filter(|n| n.kind() == kind).collect()
    }

    /// 查找此子树中第一个 kind 匹配的节点。
    pub fn find_first(&self, kind: &str) -> Option<AstRef<'a>> {
        self.walk().find(|n| n.kind() == kind)
    }
}

// ============================================================
// AstWalker
// ============================================================

/// 深度优先 AST 遍历器。
pub struct AstWalker<'a> {
    stack: Vec<AstRef<'a>>,
}

impl<'a> Iterator for AstWalker<'a> {
    type Item = AstRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.stack.pop()?;
        // 将子节点以逆序推入（保证原序遍历）
        let mut children = node.children();
        children.reverse();
        for child in children {
            self.stack.push(child);
        }
        Some(node)
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_arena() -> (AstArena, AstId, AstId) {
        let mut arena = AstArena::new();
        let root = arena.alloc(AstNodeData::new("program", Span::dummy()));

        let mut fields = HashMap::new();
        fields.insert(
            "name".to_string(),
            FieldValue::Text("test_func".to_string()),
        );
        fields.insert("params".to_string(), FieldValue::Children(vec![]));
        let func = arena.alloc_with_parent(
            AstNodeData::with_fields("function_def", fields, Span::dummy()),
            root,
        );

        (arena, root, func)
    }

    #[test]
    fn test_arena_alloc_and_get() {
        let mut arena = AstArena::new();
        let id = arena.alloc(AstNodeData::new("test_node", Span::dummy()));
        assert_eq!(arena.get(id).unwrap().kind, "test_node");
        assert_eq!(arena.len(), 1);
    }

    #[test]
    fn test_arena_parent_child() {
        let (arena, root, func) = make_test_arena();

        assert_eq!(arena.parent(func), Some(root));
        assert_eq!(arena.parent(root), None);

        let root_node = arena.get(root).unwrap();
        assert_eq!(root_node.kind, "program");

        let func_node = arena.get(func).unwrap();
        assert_eq!(func_node.kind, "function_def");
        assert_eq!(func_node.get_text("name"), Some("test_func"));
    }

    #[test]
    fn test_arena_fixup_parents() {
        let mut arena = AstArena::new();

        let root = arena.alloc(AstNodeData::new("program", Span::dummy()));
        let mut fields = HashMap::new();
        fields.insert("items".to_string(), FieldValue::Children(vec![]));
        let _func = arena.alloc(AstNodeData::with_fields(
            "function_def",
            fields,
            Span::dummy(),
        ));
        // Parent not yet set for func

        arena.fixup_parents(root);
        // After fixup, root parent should be None
        assert_eq!(arena.parent(root), None);
    }

    #[test]
    fn test_field_value_accessors() {
        assert_eq!(
            FieldValue::Text("hello".to_string()).as_text(),
            Some("hello")
        );
        assert_eq!(FieldValue::Child(AstId(5)).as_child(), Some(AstId(5)));
        assert_eq!(
            FieldValue::Children(vec![AstId(1), AstId(2)]).as_children(),
            Some(&[AstId(1), AstId(2)][..])
        );
        assert_eq!(
            FieldValue::OptionalChild(Some(AstId(3))).as_optional(),
            Some(&Some(AstId(3)))
        );
        assert_eq!(FieldValue::OptionalChild(None).as_optional(), Some(&None));
        assert_eq!(FieldValue::Text("x".to_string()).as_child(), None);
    }

    #[test]
    fn test_typed_ast_and_astref() {
        let mut arena = AstArena::new();
        let schema = AstSchema::new();

        let mut root_fields = HashMap::new();
        root_fields.insert("items".to_string(), FieldValue::Children(vec![]));
        let root = arena.alloc(AstNodeData::with_fields(
            "program",
            root_fields,
            Span::dummy(),
        ));

        let typed_ast = TypedAst::with_root(schema, arena, root);
        let root_ref = typed_ast.root_ref();

        assert_eq!(root_ref.kind(), "program");
        assert!(root_ref.get_children("items").is_empty());
        assert_eq!(root_ref.get_text("nonexistent"), None);
    }

    #[test]
    fn test_astref_ancestor() {
        let mut arena = AstArena::new();
        let schema = AstSchema::new();

        let root = arena.alloc(AstNodeData::new("module", Span::dummy()));

        let mut func_fields = HashMap::new();
        func_fields.insert("blocks".to_string(), FieldValue::Children(vec![]));
        let func = arena.alloc_with_parent(
            AstNodeData::with_fields("function_def", func_fields, Span::dummy()),
            root,
        );

        let mut block_fields = HashMap::new();
        block_fields.insert("name".to_string(), FieldValue::Text("entry".to_string()));
        let block = arena.alloc_with_parent(
            AstNodeData::with_fields("block", block_fields, Span::dummy()),
            func,
        );

        let typed_ast = TypedAst::with_root(schema, arena, root);
        let block_ref = typed_ast.ref_to(block);

        assert_eq!(block_ref.kind(), "block");
        assert_eq!(block_ref.get_text("name"), Some("entry"));

        let func_ancestor = block_ref.ancestor("function_def");
        assert!(func_ancestor.is_some());
        assert_eq!(func_ancestor.unwrap().id, func);

        let module_ancestor = block_ref.ancestor("module");
        assert!(module_ancestor.is_some());
        assert_eq!(module_ancestor.unwrap().id, root);

        assert!(block_ref.ancestor("nonexistent").is_none());
    }

    #[test]
    fn test_ast_walker() {
        let mut arena = AstArena::new();
        let schema = AstSchema::new();

        let mut root_fields = HashMap::new();
        root_fields.insert("stmts".to_string(), FieldValue::Children(vec![]));
        let root = arena.alloc(AstNodeData::with_fields(
            "program",
            root_fields,
            Span::dummy(),
        ));

        let mut inst_fields = HashMap::new();
        inst_fields.insert("mnemonic".to_string(), FieldValue::Text("mov".to_string()));
        let _inst1 = arena.alloc_with_parent(
            AstNodeData::with_fields("inst", inst_fields, Span::dummy()),
            root,
        );

        let mut inst2_fields = HashMap::new();
        inst2_fields.insert("mnemonic".to_string(), FieldValue::Text("add".to_string()));
        let _inst2 = arena.alloc_with_parent(
            AstNodeData::with_fields("inst", inst2_fields, Span::dummy()),
            root,
        );

        // Update root's stmts field to include both inst children
        if let Some(root_node) = arena.get_mut(root) {
            root_node.fields.insert(
                "stmts".to_string(),
                FieldValue::Children(vec![_inst1, _inst2]),
            );
        }

        let typed_ast = TypedAst::with_root(schema, arena, root);
        let root_ref = typed_ast.root_ref();

        let kinds: Vec<String> = root_ref.walk().map(|n| n.kind().to_string()).collect();
        assert!(kinds.iter().any(|k| k == "program"));
        assert!(kinds.iter().any(|k| k == "inst"));

        let insts = root_ref.find_all("inst");
        assert_eq!(insts.len(), 2);
    }
}
