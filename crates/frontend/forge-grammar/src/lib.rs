//! forge-grammar — Grammar-driven compiler frontend framework (v21).
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
//! use forge_grammar::{Grammar, Parser};
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

pub mod cst;
pub mod error;
pub mod grammar;
pub mod lexer;
pub mod parser;
pub mod visit;

// ── NEW: AST 层 (Phase A+) ──
pub mod ast;

// ── NEW: 语义分析层 (Phase D+) ──
pub mod semantic;

// Re-exports for convenience
pub use cst::{CstBuilder, CstNode, CstWalker};
pub use error::{GrammarError, LexError, LowerError, ParseError, Span};
pub use grammar::{
    DiagnosticLevel, Expr, Grammar, GrammarDiagnostic, TokenDef, TokenPattern, parse_grammar,
};
pub use lexer::{Lexer, Token};
pub use parser::{Parser, ParserConfig, parse_rule_with_grammar, parse_with_grammar};
pub use visit::CstPattern;
pub use visit::transform::IdentityFolder;
pub use visit::{AstTransform, AstVisitor, NodeCollector, NodeCounter, TreePrinter, VisitAction};

// AST re-exports
pub use ast::lower::lower_cst;
pub use ast::node::{Ident, Literal, Seq};
pub use ast::{AstArena, AstId, AstNodeData, AstRef, AstWalker, FieldValue, TypedAst};

// Semantic re-exports (note: scope::Symbol and schema::Symbol are different types)
pub use ast::schema::{
    AstSchema, EnumBuilder, FieldDef, FieldType, NodeDef, NodeKind, SchemaError, StructBuilder,
    Symbol, VariantDef,
};
pub use semantic::resolve::{NameResolver, ResolutionMap};
pub use semantic::scope::Symbol as ScopeSymbol; // Disambiguate from schema::Symbol
pub use semantic::scope::{Scope, ScopeKind, SymbolKind, SymbolTable, TypeInfo};
pub use semantic::types::TypeChecker;
pub use semantic::{AnalysisContext, SemanticDiagnostic};
