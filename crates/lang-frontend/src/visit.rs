//! CST visitor utilities.
//!
//! Helper functions for traversing and querying CST nodes.

use crate::cst::CstNode;
use crate::lexer::Token;

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
