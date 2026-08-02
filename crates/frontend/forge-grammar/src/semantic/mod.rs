//! 语义分析框架 — 符号表、作用域、名称解析、类型检查。
//!
//! # 核心类型
//!
//! - [`AnalysisContext`] — 语义分析上下文，持有 AST、作用域、诊断
//! - [`SemanticDiagnostic`] — 语义分析诊断（错误/警告）
//!
//! # 子模块
//!
//! - [`scope`] — 符号表 & 作用域管理
//! - [`resolve`] — 名称解析
//! - [`types`] — 类型系统

pub mod resolve;
pub mod scope;
pub mod types;

use crate::ast::{AstId, TypedAst};
use crate::error::Span;

// ============================================================
// SemanticDiagnostic
// ============================================================

/// 语义分析诊断级别。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Error,
    Warning,
    Note,
}

/// 语义分析诊断（错误、警告、提示）。
#[derive(Debug, Clone)]
pub struct SemanticDiagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
    pub span: Option<Span>,
    pub node: Option<AstId>,
}

impl SemanticDiagnostic {
    pub fn error(message: impl Into<String>, span: Span) -> Self {
        Self {
            level: DiagnosticLevel::Error,
            message: message.into(),
            span: Some(span),
            node: None,
        }
    }

    pub fn warning(message: impl Into<String>, span: Span) -> Self {
        Self {
            level: DiagnosticLevel::Warning,
            message: message.into(),
            span: Some(span),
            node: None,
        }
    }

    pub fn at_node(
        level: DiagnosticLevel,
        message: impl Into<String>,
        node_id: AstId,
        ast: &TypedAst,
    ) -> Self {
        let span = ast.arena.get(node_id).map(|n| n.span.clone());
        Self {
            level,
            message: message.into(),
            span,
            node: Some(node_id),
        }
    }
}

// ============================================================
// AnalysisContext
// ============================================================

/// 语义分析上下文 — 持有分析过程中需要的所有状态。
#[derive(Debug, Clone)]
pub struct AnalysisContext {
    /// 被分析的 AST。
    pub ast: TypedAst,
    /// 符号表（作用域栈）。
    pub scopes: scope::SymbolTable,
    /// 诊断列表。
    pub diagnostics: Vec<SemanticDiagnostic>,
}

impl AnalysisContext {
    /// 创建新的分析上下文。
    pub fn new(ast: TypedAst) -> Self {
        Self {
            ast,
            scopes: scope::SymbolTable::new(),
            diagnostics: Vec::new(),
        }
    }

    /// 添加一个错误诊断。
    pub fn error(&mut self, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(SemanticDiagnostic::error(message, span));
    }

    /// 添加一个警告诊断。
    pub fn warn(&mut self, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(SemanticDiagnostic::warning(message, span));
    }

    /// 是否有错误。
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.level == DiagnosticLevel::Error)
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::schema::AstSchema;

    #[test]
    fn test_semantic_diagnostic_basic() {
        let diag = SemanticDiagnostic::error("test error", Span::dummy());
        assert_eq!(diag.level, DiagnosticLevel::Error);
        assert_eq!(diag.message, "test error");
    }

    #[test]
    fn test_analysis_context() {
        let schema = AstSchema::new();
        let ast = TypedAst::new(schema);
        let mut cx = AnalysisContext::new(ast);
        assert!(!cx.has_errors());

        cx.error("something wrong", Span::dummy());
        assert!(cx.has_errors());
        assert_eq!(cx.diagnostics.len(), 1);
    }
}
