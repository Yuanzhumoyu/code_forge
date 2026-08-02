//! Grammar loader — loads and caches the `Grammar` from `grammars/mini_c.lx`.
//!
//! Uses `parse_grammar` to parse the `.lx` EBNF file at compile time via
//! `include_str!`, then validates the grammar.

use code_forge::forge_grammar::{Grammar, parse_grammar};

/// Build and return the Mini C grammar.
///
/// Panics if the embedded `mini_c.lx` has syntax errors.
pub fn build_grammar() -> Grammar {
    let src = include_str!("../grammars/mini_c.lx");
    parse_grammar(src).expect("invalid mini_c grammar")
}
