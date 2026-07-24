//! lang-frontend — Grammar-driven compiler frontend framework.
//!
//! Define your language syntax in EBNF-like `.lx` grammar files,
//! and this crate provides a ready-to-use lexer, parser, and CST.
//!
//! # Architecture
//!
//! ```text
//! Grammar (.lx)  ──►  Grammar IR  ──►  Lexer + Parser  ──►  CST
//! ```
//!
//! # Quick start
//!
//! ```rust,ignore
//! use lang_frontend::{Grammar, Parser};
//!
//! let grammar_src = r#"
//!     token IDENT = "[a-z][a-z0-9_]*"
//!     token NUM = "[0-9]+"
//!     punct "+" "*"
//!     skip "[ \\t]+"
//!
//!     expr ::= term ( "+" term )*
//!     term ::= NUM | IDENT
//! "#;
//!
//! let grammar = Grammar::parse(grammar_src).unwrap();
//! let parser = Parser::build(grammar);
//! let cst = parser.parse("x + 42").unwrap();
//! // Walk CST to inspect parse tree
//! for node in cst.walk() {
//!     println!("{}: {:?}", node.kind, node.text());
//! }
//! ```

pub mod error;
pub mod grammar;
pub mod lexer;
pub mod parser;
pub mod cst;
pub mod visit;

// Re-exports for convenience
pub use error::{GrammarError, LexError, ParseError, Span};
pub use grammar::{Grammar, Expr, TokenDef, TokenPattern, parse_grammar};
pub use lexer::{Lexer, Token};
pub use parser::{Parser, parse_with_grammar, parse_rule_with_grammar};
pub use cst::{CstBuilder, CstNode, CstWalker};
