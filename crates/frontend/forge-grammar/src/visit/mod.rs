//! CST visitor and pattern matching utilities.
//!
//! Helper functions for traversing and querying CST nodes,
//! plus a declarative [`CstPattern`] DSL for structural matching.
//!
//! Also includes AST-level visitor ([`AstVisitor`]) and transform
//! ([`AstTransform`]) traits.

pub mod transform;

use crate::ast::AstId;
use crate::cst::CstNode;
use crate::lexer::Token;

// Re-export AST visitor types from this module for convenience
pub use transform::AstTransform;

// ============================================================
// AstVisitor — 只读 AST 遍历器
// ============================================================

use crate::ast::{AstRef, TypedAst};

/// AST 遍历器 trait（只读）。
///
/// 实现此 trait 来构建语义分析 pass。与 `CstVisitor` 不同，
/// `AstVisitor` 遍历的是类型化的 AST 节点（`AstRef`），
/// 而非原始 CST。
///
/// # 使用示例
///
/// ```ignore
/// struct Counter { count: usize }
/// impl AstVisitor for Counter {
///     fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
///         if node.kind() == "inst_like" {
///             self.count += 1;
///         }
///         VisitAction::Continue
///     }
/// }
/// ```
pub trait AstVisitor {
    /// 进入节点时调用（pre-order）。
    /// 返回 `VisitAction` 控制遍历行为。
    fn visit(&mut self, _node: AstRef<'_>) -> VisitAction {
        VisitAction::Continue
    }

    /// 离开节点时调用（post-order），在所有子节点已遍历后。
    fn leave(&mut self, _node: AstRef<'_>) {}

    /// 标准的深度优先遍历整个 AST。
    fn walk(&mut self, ast: &TypedAst) {
        self.walk_node(ast.root_ref());
    }

    /// 深度优先遍历某个节点及其子树。
    fn walk_node(&mut self, node: AstRef<'_>) {
        match self.visit(node) {
            VisitAction::Stop => return,
            VisitAction::Skip => {
                self.leave(node);
                return;
            }
            VisitAction::Continue => {
                for child in node.children() {
                    self.walk_node(child);
                }
            }
        }
        self.leave(node);
    }
}

// Note: VisitAction is defined above (reused by both CstVisitor and AstVisitor)

// ============================================================
// Built-in visitors
// ============================================================

/// 收集所有匹配指定 kind 的节点。
pub struct NodeCollector {
    pub kind: String,
    pub nodes: Vec<AstId>,
}

impl NodeCollector {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            nodes: Vec::new(),
        }
    }
}

impl AstVisitor for NodeCollector {
    fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
        if node.kind() == self.kind {
            self.nodes.push(node.id);
        }
        VisitAction::Continue
    }
}

/// 统计 AST 中每种节点 kind 的数量。
#[derive(Default)]
pub struct NodeCounter {
    pub counts: std::collections::HashMap<String, usize>,
}

impl NodeCounter {
    pub fn new() -> Self {
        Self {
            counts: std::collections::HashMap::new(),
        }
    }
}

impl AstVisitor for NodeCounter {
    fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
        *self.counts.entry(node.kind().to_string()).or_insert(0) += 1;
        VisitAction::Continue
    }
}

/// 将 AST 树结构打印为缩进文本（调试用）。
#[derive(Default)]
pub struct TreePrinter {
    pub output: String,
    indent: usize,
}

impl TreePrinter {
    pub fn new() -> Self {
        Self {
            output: String::new(),
            indent: 0,
        }
    }

    /// 打印完 AST；返回格式化的字符串。
    pub fn finish(self) -> String {
        self.output
    }
}

impl AstVisitor for TreePrinter {
    fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
        let indent_str = "  ".repeat(self.indent);
        let text = node.get_text("text").unwrap_or("");
        let kind = node.kind();
        if text.is_empty() {
            self.output.push_str(&format!("{}{}\n", indent_str, kind));
        } else {
            self.output
                .push_str(&format!("{}{}: {}\n", indent_str, kind, text));
        }
        self.indent += 1;
        VisitAction::Continue
    }

    fn leave(&mut self, _node: AstRef<'_>) {
        self.indent = self.indent.saturating_sub(1);
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod visitor_tests {
    use super::*;
    use crate::ast::schema::AstSchema;
    use crate::ast::{AstArena, AstNodeData, FieldValue, TypedAst};
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

        // Update root to reference children
        arena.get_mut(root).unwrap().fields.insert(
            "stmts".to_string(),
            FieldValue::Children(vec![inst1, inst2]),
        );

        TypedAst::with_root(schema, arena, root)
    }

    #[test]
    fn test_visitor_node_collector() {
        let ast = make_test_ast();
        let mut collector = NodeCollector::new("inst");
        collector.walk(&ast);
        assert_eq!(collector.nodes.len(), 2);
    }

    #[test]
    fn test_visitor_node_counter() {
        let ast = make_test_ast();
        let mut counter = NodeCounter::new();
        counter.walk(&ast);
        assert_eq!(counter.counts.get("program"), Some(&1));
        assert_eq!(counter.counts.get("inst"), Some(&2));
    }

    #[test]
    fn test_visitor_tree_printer() {
        let ast = make_test_ast();
        let mut printer = TreePrinter::new();
        printer.walk(&ast);
        let output = printer.finish();
        assert!(output.contains("program"));
        assert!(output.contains("inst"));
    }

    #[test]
    fn test_visitor_skip_children() {
        let ast = make_test_ast();

        struct SkipFirst {
            visited: Vec<String>,
            skip_next: bool,
        }
        impl AstVisitor for SkipFirst {
            fn visit(&mut self, node: AstRef<'_>) -> VisitAction {
                self.visited.push(node.kind().to_string());
                if self.skip_next {
                    self.skip_next = false;
                    return VisitAction::Skip;
                }
                VisitAction::Continue
            }
        }

        let mut v = SkipFirst {
            visited: Vec::new(),
            skip_next: true,
        };
        v.walk(&ast);
        // First inst should be skipped (children not visited)
        assert!(v.visited.contains(&"program".to_string()));
    }
}

/// Collect all terminal (leaf) tokens from a CST subtree.
pub fn collect_terminals(node: &CstNode) -> Vec<&Token> {
    node.leaves()
}

/// Collect all direct children of a given kind.
pub fn collect_by_kind<'a>(node: &'a CstNode, kind: &str) -> Vec<&'a CstNode> {
    node.children_by(kind)
}

/// Find the first (depth-first) node of the given kind.
pub fn find_first<'a>(node: &'a CstNode, kind: &str) -> Option<&'a CstNode> {
    node.walk().find(|n| n.kind == kind)
}

/// Find all nodes of the given kind in the subtree.
pub fn find_all<'a>(node: &'a CstNode, kind: &str) -> Vec<&'a CstNode> {
    node.walk().filter(|n| n.kind == kind).collect()
}

/// Get the concatenated text of all leaf tokens in a subtree.
pub fn subtree_text(node: &CstNode) -> String {
    node.leaves()
        .iter()
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Check if a CST node is a punctuation/token leaf (not a rule node).
pub fn is_punctuation(node: &CstNode) -> bool {
    node.is_leaf()
}

/// Strip punctuation/token children from a node, returning only rule children.
pub fn strip_punctuation(node: &CstNode) -> Vec<&CstNode> {
    node.children.iter().filter(|c| c.is_rule()).collect()
}

// ============================================================
// CST Visitor trait
// ============================================================

/// Visitor trait for CST depth-first traversal with pre/post order hooks.
pub trait CstVisitor {
    /// Called on entering a node (pre-order).
    /// Return `Skip` to skip children, `Stop` to abort traversal entirely.
    fn visit_node(&mut self, _node: &CstNode) -> VisitAction {
        VisitAction::Continue
    }

    /// Called on leaving a node (post-order), after children have been visited.
    fn leave_node(&mut self, _node: &CstNode) {}
}

/// Action returned by `CstVisitor::visit_node`.
pub enum VisitAction {
    /// Continue traversal into children.
    Continue,
    /// Skip this node's children (but still call leave_node).
    Skip,
    /// Stop traversal entirely.
    Stop,
}

/// Walk a CST with a visitor (depth-first, pre- and post-order).
pub fn walk_cst(node: &CstNode, visitor: &mut dyn CstVisitor) {
    match visitor.visit_node(node) {
        VisitAction::Stop => return,
        VisitAction::Skip => {
            visitor.leave_node(node);
            return;
        }
        VisitAction::Continue => {
            for child in &node.children {
                walk_cst(child, visitor);
            }
        }
    }
    visitor.leave_node(node);
}

// ============================================================
// CstPattern — declarative CST matching DSL
// ============================================================

/// A pattern for matching CST substructures declaratively.
///
/// # Examples
///
/// ```ignore
/// // Match an inst_like node with mnemonic "mov"
/// let pattern = CstPattern::kind("inst_like")
///     .with_child(CstPattern::token_text("mov"));
///
/// // Match any REG token
/// let pattern = CstPattern::token_kind("REG");
///
/// // Match a sequence: label_def = LABEL ":"
/// let pattern = CstPattern::seq(vec![
///     CstPattern::token_kind("LABEL"),
///     CstPattern::token_text(":"),
/// ]);
/// ```
#[derive(Debug, Clone)]
pub enum CstPattern {
    /// Match node by kind name (rule or token kind).
    Kind(String),
    /// Match a sequence: children must match patterns in order.
    Seq(Vec<CstPattern>),
    /// Match any of the alternatives (first-match).
    Any(Vec<CstPattern>),
    /// Match zero or more consecutive children matching the inner pattern.
    Many(Box<CstPattern>),
    /// Match a leaf token by its kind name.
    TokenKind(String),
    /// Match a leaf token by its text content.
    TokenText(String),
    /// Wildcard: matches any node.
    Wildcard,
    /// Match zero or one child matching the inner pattern.
    Optional(Box<CstPattern>),
}

impl CstPattern {
    /// Shorthand: match a node by its kind name.
    pub fn kind(name: impl Into<String>) -> Self {
        CstPattern::Kind(name.into())
    }

    /// Shorthand: match a leaf token by kind.
    pub fn token_kind(name: impl Into<String>) -> Self {
        CstPattern::TokenKind(name.into())
    }

    /// Shorthand: match a leaf token by text.
    pub fn token_text(text: impl Into<String>) -> Self {
        CstPattern::TokenText(text.into())
    }

    /// Shorthand: match a sequence of child patterns.
    pub fn seq(children: Vec<CstPattern>) -> Self {
        CstPattern::Seq(children)
    }

    /// Shorthand: match any of the alternatives.
    pub fn any(alternatives: Vec<CstPattern>) -> Self {
        CstPattern::Any(alternatives)
    }

    /// Add a required child pattern to a Kind/Seq pattern.
    /// Converts this pattern into a Seq if it's a Kind.
    pub fn with_child(mut self, child: CstPattern) -> Self {
        match &mut self {
            CstPattern::Seq(children) => {
                children.push(child);
                self
            }
            _ => CstPattern::Seq(vec![self, child]),
        }
    }

    /// Check if a CST node matches this pattern.
    pub fn matches(&self, node: &CstNode) -> bool {
        match self {
            CstPattern::Kind(name) => node.kind == *name,
            CstPattern::Seq(patterns) => {
                // Find a subsequence of children matching patterns in order
                Self::match_sequence(&node.children, patterns, 0, 0)
            }
            CstPattern::Any(alternatives) => alternatives.iter().any(|p| p.matches(node)),
            CstPattern::Many(pattern) => {
                // Matches a node that has children, all matching the pattern.
                // For a leaf node, Many matches if there are no children.
                // But typically Many is used as a child pattern inside Seq.
                node.children.iter().all(|c| pattern.matches(c))
            }
            CstPattern::TokenKind(kind_name) => node.is_leaf() && node.kind == *kind_name,
            CstPattern::TokenText(text) => node.is_leaf() && node.text() == text,
            CstPattern::Wildcard => true,
            CstPattern::Optional(_pattern) => true, // Optional always matches at the node level
        }
    }

    /// Backtracking subsequence match for Seq patterns against children list.
    fn match_sequence(
        children: &[CstNode],
        patterns: &[CstPattern],
        child_idx: usize,
        pat_idx: usize,
    ) -> bool {
        if pat_idx >= patterns.len() {
            return true; // all patterns matched
        }
        if child_idx >= children.len() {
            // Check if remaining patterns are all optional/empty
            return patterns[pat_idx..].iter().all(|p| p.is_optional());
        }

        let pattern = &patterns[pat_idx];

        // Try matching current child with current pattern
        if pattern.matches(&children[child_idx]) {
            // Greedy: try to advance both
            if Self::match_sequence(children, patterns, child_idx + 1, pat_idx + 1) {
                return true;
            }
        }

        // If current pattern is optional, try skipping it
        if pattern.is_optional() && Self::match_sequence(children, patterns, child_idx, pat_idx + 1)
        {
            return true;
        }

        // If current pattern is Many, try consuming more children
        if matches!(pattern, CstPattern::Many(_) | CstPattern::Wildcard) {
            if Self::match_sequence(children, patterns, child_idx + 1, pat_idx) {
                return true;
            }
            // Also try moving past the Many
            if Self::match_sequence(children, patterns, child_idx, pat_idx + 1) {
                return true;
            }
        }

        false
    }

    /// Check if this pattern can match zero children (is optional).
    fn is_optional(&self) -> bool {
        matches!(self, CstPattern::Optional(_) | CstPattern::Many(_))
    }
}

// ============================================================
// CstNode pattern query extensions
// ============================================================

impl CstNode {
    /// Find all direct children matching a pattern (shallow).
    pub fn find_children(&self, pattern: &CstPattern) -> Vec<&CstNode> {
        self.children
            .iter()
            .filter(|c| pattern.matches(c))
            .collect()
    }

    /// Find the first descendant matching a pattern (depth-first).
    pub fn find_first_deep(&self, pattern: &CstPattern) -> Option<&CstNode> {
        self.walk().find(|n| pattern.matches(n))
    }

    /// Find all descendants matching a pattern (depth-first).
    pub fn find_all_deep(&self, pattern: &CstPattern) -> Vec<&CstNode> {
        self.walk().filter(|n| pattern.matches(n)).collect()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cst::CstBuilder;
    use crate::error::Span;
    use crate::lexer::Token;

    fn make_token(kind: &str, text: &str) -> Token {
        Token::new(kind, text, Span::dummy())
    }

    fn make_test_cst() -> CstNode {
        let mut builder = CstBuilder::new();
        builder.push("inst_like");
        builder.add_token(make_token("IDENT", "mov"));
        builder.push("operand");
        builder.add_token(make_token("REG", "RAX"));
        let op1 = builder.pop().unwrap();
        builder.add(op1);
        builder.add_token(make_token("COMMA", ","));
        builder.push("operand");
        builder.add_token(make_token("DEC", "42"));
        let op2 = builder.pop().unwrap();
        builder.add(op2);
        builder.pop().unwrap()
    }

    #[test]
    fn test_pattern_kind() {
        let cst = make_test_cst();
        let pattern = CstPattern::kind("inst_like");
        assert!(pattern.matches(&cst));
        assert!(!CstPattern::kind("macro_def").matches(&cst));
    }

    #[test]
    fn test_pattern_token_kind() {
        let cst = make_test_cst();
        let ident_node = &cst.children[0]; // "mov" token
        assert!(CstPattern::token_kind("IDENT").matches(ident_node));
        assert!(!CstPattern::token_kind("REG").matches(ident_node));
    }

    #[test]
    fn test_pattern_token_text() {
        let cst = make_test_cst();
        let ident_node = &cst.children[0]; // "mov" token
        assert!(CstPattern::token_text("mov").matches(ident_node));
        assert!(!CstPattern::token_text("add").matches(ident_node));
    }

    #[test]
    fn test_pattern_seq() {
        let cst = make_test_cst();
        // inst_like = IDENT("mov") operand COMMA(",") operand
        let pattern = CstPattern::seq(vec![
            CstPattern::token_kind("IDENT"),
            CstPattern::Wildcard, // operand
            CstPattern::Wildcard, // comma
            CstPattern::Wildcard, // operand
        ]);
        assert!(pattern.matches(&cst));
    }

    #[test]
    fn test_pattern_wildcard() {
        let cst = make_test_cst();
        assert!(CstPattern::Wildcard.matches(&cst));
    }

    #[test]
    fn test_find_children() {
        let cst = make_test_cst();
        let operands = cst.find_children(&CstPattern::kind("operand"));
        assert_eq!(operands.len(), 2);
    }

    #[test]
    fn test_find_first_deep() {
        let cst = make_test_cst();
        let found = cst.find_first_deep(&CstPattern::token_kind("REG"));
        assert!(found.is_some());
        assert_eq!(found.unwrap().text(), "RAX");
    }

    #[test]
    fn test_find_all_deep() {
        let cst = make_test_cst();
        let all_kinds = cst.find_all_deep(&CstPattern::token_kind("COMMA"));
        assert_eq!(all_kinds.len(), 1);
    }

    #[test]
    fn test_visitor_basic() {
        let cst = make_test_cst();

        struct Collector {
            kinds: Vec<String>,
        }
        impl CstVisitor for Collector {
            fn visit_node(&mut self, node: &CstNode) -> VisitAction {
                self.kinds.push(node.kind.clone());
                VisitAction::Continue
            }
        }

        let mut collector = Collector { kinds: Vec::new() };
        walk_cst(&cst, &mut collector);
        assert!(collector.kinds.contains(&"inst_like".to_string()));
        assert!(collector.kinds.contains(&"operand".to_string()));
    }

    #[test]
    fn test_pattern_many_empty() {
        // CstPattern::Many on a leaf node (no children): vacuous truth.
        let leaf = CstNode::leaf(make_token("IDENT", "test"));
        let pattern = CstPattern::Many(Box::new(CstPattern::token_kind("IDENT")));
        // A node with zero children matches Many vacuously (all of 0 children match).
        assert!(pattern.matches(&leaf));

        // Many with Wildcard on a node that has children should also pass.
        let cst = make_test_cst();
        let wildcard_many = CstPattern::Many(Box::new(CstPattern::Wildcard));
        assert!(wildcard_many.matches(&cst));
    }

    #[test]
    fn test_pattern_deeply_nested_seq() {
        let cst = make_test_cst();
        // cst: inst_like -> [IDENT("mov"), operand(REG("RAX")), COMMA(","), operand(DEC("42"))]
        // Match with a Seq that contains nested Seqs for operands.
        let pattern = CstPattern::seq(vec![
            CstPattern::token_kind("IDENT"),
            CstPattern::seq(vec![CstPattern::token_kind("REG")]),
            CstPattern::token_kind("COMMA"),
            CstPattern::seq(vec![CstPattern::token_kind("DEC")]),
        ]);
        assert!(pattern.matches(&cst));

        // A slightly different nested Seq should fail.
        let bad_pattern = CstPattern::seq(vec![
            CstPattern::token_kind("IDENT"),
            CstPattern::seq(vec![
                CstPattern::token_kind("DEC"), // wrong: first operand is REG, not DEC
            ]),
        ]);
        assert!(!bad_pattern.matches(&cst));
    }

    #[test]
    fn test_pattern_wildcard_on_root() {
        let cst = make_test_cst();
        // Wildcard matches any single node, including the root.
        assert!(CstPattern::Wildcard.matches(&cst));

        // Wildcard as the entire Seq pattern on root children (all 4 children are wildcards).
        let pattern = CstPattern::Seq(vec![
            CstPattern::Wildcard,
            CstPattern::Wildcard,
            CstPattern::Wildcard,
            CstPattern::Wildcard,
        ]);
        assert!(pattern.matches(&cst));

        // Wildcard in Seq can match zero children via the skip fallback.
        // Five wildcards still passes because one wildcard can be skipped.
        let five_wildcards = CstPattern::Seq(vec![
            CstPattern::Wildcard,
            CstPattern::Wildcard,
            CstPattern::Wildcard,
            CstPattern::Wildcard,
            CstPattern::Wildcard,
        ]);
        assert!(five_wildcards.matches(&cst));

        // But a required Kind in the wrong position does fail.
        let wrong_kind = CstPattern::Seq(vec![
            CstPattern::Wildcard,
            CstPattern::kind("nonexistent"), // no child has this kind
        ]);
        assert!(!wrong_kind.matches(&cst));
    }
}
