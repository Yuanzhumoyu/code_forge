//! Error types for the forge-grammar crate.

use std::fmt;

/// Span in source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub col: usize,
}

impl Span {
    pub fn new(start: usize, end: usize, line: usize, col: usize) -> Self {
        Self {
            start,
            end,
            line,
            col,
        }
    }

    /// Dummy span for programmatic nodes.
    pub fn dummy() -> Self {
        Self {
            start: 0,
            end: 0,
            line: 0,
            col: 0,
        }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, col {}", self.line + 1, self.col + 1)
    }
}

/// Errors from grammar parsing.
#[derive(Debug, thiserror::Error)]
pub enum GrammarError {
    #[error("parse error at line {line}: {message}")]
    Parse { message: String, line: usize },

    #[error("unknown token kind '{0}' referenced in rule")]
    UnknownToken(String),

    #[error("unknown rule '{0}' referenced")]
    UnknownRule(String),

    #[error("duplicate rule '{0}'")]
    DuplicateRule(String),

    #[error("duplicate token '{0}'")]
    DuplicateToken(String),

    #[error("left recursion detected in rule '{0}'")]
    LeftRecursion(String),

    #[error("empty grammar: no rules defined")]
    EmptyGrammar,

    #[error("missing entry rule '{0}'")]
    MissingEntry(String),

    #[error("invalid regex for token '{name}': {error}")]
    InvalidRegex { name: String, error: String },
}

/// Errors from lexing.
#[derive(Debug, thiserror::Error)]
#[error("lex error at {span}: {message}")]
pub struct LexError {
    pub message: String,
    pub span: Span,
}

/// Errors from parsing.
#[derive(Debug, Clone, thiserror::Error)]
#[error("parse error at {span}: {message}")]
pub struct ParseError {
    pub message: String,
    pub span: Span,
    pub expected: Vec<String>,
}

impl ParseError {
    pub fn new(message: impl Into<String>, span: Span, expected: Vec<String>) -> Self {
        Self {
            message: message.into(),
            span,
            expected,
        }
    }
}

/// Errors from CST → AST lowering.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LowerError {
    #[error("lowering error at {span}: {message}")]
    Lower { message: String, span: Span },

    #[error("missing required field '{field}' in node '{node}'")]
    MissingField { node: String, field: String },

    #[error("unknown variant '{variant}' for enum node '{node}'")]
    UnknownVariant { node: String, variant: String },

    #[error("recursion limit ({0}) exceeded during lowering")]
    RecursionLimit(usize),

    #[error("schema error: {0}")]
    Schema(String),
}

impl LowerError {
    pub fn missing_field(node: impl Into<String>, field: impl Into<String>) -> Self {
        LowerError::MissingField {
            node: node.into(),
            field: field.into(),
        }
    }

    pub fn unknown_variant(node: impl Into<String>, variant: impl Into<String>) -> Self {
        LowerError::UnknownVariant {
            node: node.into(),
            variant: variant.into(),
        }
    }

    pub fn lowering(message: impl Into<String>, span: Span) -> Self {
        LowerError::Lower {
            message: message.into(),
            span,
        }
    }

    pub fn schema(message: impl Into<String>) -> Self {
        LowerError::Schema(message.into())
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lower_error_display() {
        let e = LowerError::missing_field("function_def", "name");
        let msg = format!("{}", e);
        assert!(msg.contains("function_def"));
        assert!(msg.contains("name"));
    }

    #[test]
    fn test_lower_error_unknown_variant() {
        let e = LowerError::unknown_variant("operand", "UnknownType");
        let msg = format!("{}", e);
        assert!(msg.contains("operand"));
        assert!(msg.contains("UnknownType"));
    }

    #[test]
    fn test_lower_error_recursion_limit() {
        let e = LowerError::RecursionLimit(256);
        let msg = format!("{}", e);
        assert!(msg.contains("256"));
    }

    #[test]
    fn test_lower_error_lowering() {
        let e = LowerError::lowering("bad input", Span::dummy());
        let msg = format!("{}", e);
        assert!(msg.contains("bad input"));
    }

    #[test]
    fn test_lower_error_schema() {
        let e = LowerError::schema("invalid schema config");
        let msg = format!("{}", e);
        assert!(msg.contains("invalid schema config"));
    }
}
