//! Error types for the lang-frontend crate.

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
        Self { start, end, line, col }
    }

    /// Dummy span for programmatic nodes.
    pub fn dummy() -> Self {
        Self { start: 0, end: 0, line: 0, col: 0 }
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
#[derive(Debug, thiserror::Error)]
#[error("parse error at {span}: {message}")]
pub struct ParseError {
    pub message: String,
    pub span: Span,
    pub expected: Vec<String>,
}

impl ParseError {
    pub fn new(message: impl Into<String>, span: Span, expected: Vec<String>) -> Self {
        Self { message: message.into(), span, expected }
    }
}
