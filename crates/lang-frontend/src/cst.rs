//! Concrete Syntax Tree types.
//!
//! The CST mirrors the grammar structure directly. Each grammar rule produces
//! a CST node, and tokens become leaf nodes.
//!
//! Unlike an AST, the CST preserves all syntactic details including punctuation,
//! which makes it suitable for source-to-source transformations and detailed
//! error reporting.

use crate::error::Span;
use crate::lexer::Token;

// ============================================================
// CST Node
// ============================================================

/// A node in the Concrete Syntax Tree.
#[derive(Debug, Clone)]
pub struct CstNode {
    /// The node kind: rule name (e.g., "stmt", "operand") or token kind (e.g., "IDENT", "LPAREN").
    pub kind: String,
    /// Child nodes.
    pub children: Vec<CstNode>,
    /// For leaf nodes: the matched token.
    pub token: Option<Token>,
    /// Source span covering this node.
    pub span: Span,
}

impl CstNode {
    /// Create a rule node (non-terminal).
    pub fn rule(name: impl Into<String>, children: Vec<CstNode>, span: Span) -> Self {
        Self { kind: name.into(), children, token: None, span }
    }

    /// Create a token leaf node.
    pub fn leaf(token: Token) -> Self {
        let span = token.span.clone();
        let kind = token.kind.clone();
        Self { kind, children: vec![], token: Some(token), span }
    }

    /// Create an epsilon (empty match) node.
    pub fn epsilon() -> Self {
        Self {
            kind: "epsilon".to_string(),
            children: vec![],
            token: None,
            span: Span::dummy(),
        }
    }

    /// The source text of this node (from its token, if any).
    pub fn text(&self) -> &str {
        if let Some(ref token) = self.token {
            &token.text
        } else {
            ""
        }
    }

    /// Find the first direct child with the given kind.
    pub fn child(&self, kind: &str) -> Option<&CstNode> {
        self.children.iter().find(|c| c.kind == kind)
    }

    /// Find all direct children with the given kind.
    pub fn children_by(&self, kind: &str) -> Vec<&CstNode> {
        self.children.iter().filter(|c| c.kind == kind).collect()
    }

    /// True if this is a token leaf node.
    pub fn is_leaf(&self) -> bool {
        self.token.is_some()
    }

    /// True if this is a rule node.
    pub fn is_rule(&self) -> bool {
        self.token.is_none() && self.kind != "epsilon"
    }

    /// Walk the CST in depth-first order, yielding each node.
    pub fn walk(&self) -> CstWalker<'_> {
        CstWalker { stack: vec![self] }
    }

    /// Collect only the leaf (token) nodes in depth-first order.
    pub fn leaves(&self) -> Vec<&Token> {
        let mut result = Vec::new();
        for node in self.walk() {
            if let Some(ref token) = node.token {
                result.push(token);
            }
        }
        result
    }
}

// ============================================================
// CST Walker
// ============================================================

/// Depth-first iterator over CST nodes.
pub struct CstWalker<'a> {
    stack: Vec<&'a CstNode>,
}

impl<'a> Iterator for CstWalker<'a> {
    type Item = &'a CstNode;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.stack.pop()?;
        // Push children in reverse order so first child is processed first
        for child in node.children.iter().rev() {
            self.stack.push(child);
        }
        Some(node)
    }
}

// ============================================================
// CST Builder (for parser use)
// ============================================================

/// Builds a CST incrementally during parsing.
#[derive(Debug)]
pub struct CstBuilder {
    /// Stack of (rule_name, accumulated_children).
    stack: Vec<(String, Vec<CstNode>)>,
}

impl CstBuilder {
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Begin a new rule node. Children will be collected until `pop` is called.
    pub fn push(&mut self, rule_name: &str) {
        self.stack.push((rule_name.to_string(), Vec::new()));
    }

    /// Add a child node to the current rule being built.
    pub fn add(&mut self, node: CstNode) {
        if let Some((_, children)) = self.stack.last_mut() {
            children.push(node);
        }
    }

    /// Add a token leaf to the current rule.
    pub fn add_token(&mut self, token: Token) {
        self.add(CstNode::leaf(token));
    }

    /// Finish the current rule node and return it.
    pub fn pop(&mut self) -> Option<CstNode> {
        if let Some((name, children)) = self.stack.pop() {
            let span = if let Some(first) = children.first() {
                let start = first.span.start;
                let end = children.last().map_or(start, |n| n.span.end);
                let line = first.span.line;
                let col = first.span.col;
                Span::new(start, end, line, col)
            } else {
                Span::dummy()
            };
            Some(CstNode::rule(name, children, span))
        } else {
            None
        }
    }

    /// Current nesting depth.
    pub fn depth(&self) -> usize {
        self.stack.len()
    }
}

impl Default for CstBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cst_builder() {
        let mut builder = CstBuilder::new();
        builder.push("inst_like");
        builder.add_token(Token::new("IDENT", "mov", Span::dummy()));
        builder.add_token(Token::new("IDENT", "rd", Span::dummy()));
        builder.add_token(Token::new("IDENT", "rs1", Span::dummy()));
        let node = builder.pop().unwrap();

        assert_eq!(node.kind, "inst_like");
        assert_eq!(node.children.len(), 3);
        assert_eq!(node.children[0].text(), "mov");
    }

    #[test]
    fn test_cst_walker() {
        let mut builder = CstBuilder::new();
        builder.push("program");
        builder.push("stmt");
        builder.add_token(Token::new("IDENT", "mov", Span::dummy()));
        let stmt_node = builder.pop().unwrap(); // stmt
        builder.add(stmt_node);
        let node = builder.pop().unwrap(); // program

        let kinds: Vec<&str> = node.walk().map(|n| n.kind.as_str()).collect();
        assert!(kinds.contains(&"program"), "Expected 'program' in {:?}", kinds);
        assert!(kinds.contains(&"stmt"), "Expected 'stmt' in {:?}", kinds);
        assert!(kinds.contains(&"IDENT"), "Expected 'IDENT' in {:?}", kinds);
    }
}
