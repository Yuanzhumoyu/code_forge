//! Parser generator and runtime.
//!
//! Takes a [`Grammar`] and implements a recursive-descent parser with
//! backtracking for alternation. The parser interprets grammar rules
//! dynamically — no code generation needed.
//!
//! # Algorithm
//!
//! Each grammar rule is parsed by a recursive function that mirrors the EBNF:
//! - `Seq(A, B, C)` → try A, then B, then C (all must succeed)
//! - `Alt(A, B, C)` → try A; if fail, backtrack and try B; etc.
//! - `ZeroOrMore(A)` → while A matches, collect
//! - `OneOrMore(A)` → A must match at least once, then ZeroOrMore
//! - `Opt(A)` → try A, succeed either way
//! - `Token(name)` → consume if current token kind matches
//! - `Lit(text)` → consume if current token text matches
//! - `Rule(name)` → recursively call parse_rule(name)
//! - `Epsilon` → always succeeds, no tokens consumed

use crate::cst::{CstBuilder, CstNode};
use crate::error::{ParseError, Span};
use crate::grammar::{Expr, Grammar};
use crate::lexer::Token;

// ============================================================
// Parser
// ============================================================

/// Runtime parser. Interprets grammar rules to parse a token stream.
pub struct Parser {
    /// The grammar driving this parser.
    grammar: Grammar,
}

impl Parser {
    /// Build a parser from a grammar definition.
    pub fn build(grammar: Grammar) -> Self {
        Self { grammar }
    }

    /// Access the grammar.
    pub fn grammar(&self) -> &Grammar {
        &self.grammar
    }

    /// Parse source text using the grammar's entry rule.
    /// Returns the CST root node.
    pub fn parse(&self, source: &str) -> Result<CstNode, ParseError> {
        self.parse_rule(source, &self.grammar.entry.clone())
    }

    /// Parse source text starting from a specific grammar rule.
    pub fn parse_rule(&self, source: &str, rule_name: &str) -> Result<CstNode, ParseError> {
        // Build lexer and tokenize
        let mut lexer = crate::lexer::Lexer::build(&self.grammar);
        let tokens = lexer.tokenize(source).map_err(|e| ParseError {
            message: e.to_string(),
            span: e.span,
            expected: vec![],
        })?;

        // Check that the rule exists
        if !self.grammar.rules.contains_key(rule_name) {
            return Err(ParseError::new(
                format!("unknown rule '{}'", rule_name),
                Span::dummy(),
                vec![],
            ));
        }

        // Parse
        let mut ctx = ParseContext::new(&self.grammar, tokens);
        let result = ctx.parse_rule(rule_name)?;

        // Check that all tokens were consumed
        if ctx.pos < ctx.tokens.len() && !ctx.tokens[ctx.pos].is_eof() {
            let tok = &ctx.tokens[ctx.pos];
            return Err(ParseError::new(
                format!("unexpected token '{}' after parsing", tok.text),
                tok.span.clone(),
                vec!["EOF".to_string()],
            ));
        }

        Ok(result)
    }
}

// ============================================================
// Parse context (internal state)
// ============================================================

struct ParseContext<'a> {
    grammar: &'a Grammar,
    tokens: Vec<Token>,
    pos: usize,
    builder: CstBuilder,
}

impl<'a> ParseContext<'a> {
    fn new(grammar: &'a Grammar, tokens: Vec<Token>) -> Self {
        Self { grammar, tokens, pos: 0, builder: CstBuilder::new() }
    }

    fn current_kind(&self) -> &str {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos].kind
        } else {
            "EOF"
        }
    }

    fn current_text(&self) -> &str {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos].text
        } else {
            ""
        }
    }

    fn current_span(&self) -> Span {
        if self.pos < self.tokens.len() {
            self.tokens[self.pos].span.clone()
        } else {
            Span::dummy()
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.tokens.len() || self.tokens[self.pos].is_eof()
    }

    /// Save position for backtracking.
    fn snapshot(&self) -> (usize, usize) {
        (self.pos, self.builder.depth())
    }

    /// Restore position after a failed attempt.
    fn restore(&mut self, snapshot: (usize, usize)) {
        let (pos, depth) = snapshot;
        self.pos = pos;
        // Pop builder stack back to saved depth
        while self.builder.depth() > depth {
            self.builder.pop();
        }
    }

    /// Advance to next token.
    fn advance(&mut self) {
        if !self.is_eof() {
            self.pos += 1;
        }
    }

    /// Parse a grammar rule by name.
    fn parse_rule(&mut self, name: &str) -> Result<CstNode, ParseError> {
        let rule = self.grammar.rules.get(name)
            .ok_or_else(|| ParseError::new(
                format!("unknown rule '{}'", name),
                self.current_span(),
                vec![],
            ))?;

        let expr = rule.expr.clone();
        self.parse_expr(&expr, Some(name))
    }

    /// Parse an EBNF expression.
    fn parse_expr(&mut self, expr: &Expr, label: Option<&str>) -> Result<CstNode, ParseError> {
        match expr {
            Expr::Seq(items) => {
                self.builder.push(label.unwrap_or("seq"));
                for item in items {
                    let child = self.parse_expr(item, None)?;
                    self.builder.add(child);
                }
                Ok(self.builder.pop().unwrap_or(CstNode::epsilon()))
            }
            Expr::Alt(alternatives) => {
                // Try each alternative with backtracking
                let mut errors = Vec::new();

                for (i, alt) in alternatives.iter().enumerate() {
                    let snapshot = self.snapshot();
                    self.builder.push(label.unwrap_or("alt"));

                    match self.parse_expr(alt, None) {
                        Ok(child) => {
                            self.builder.add(child);
                            let node = self.builder.pop().unwrap_or(CstNode::epsilon());
                            return Ok(node);
                        }
                        Err(e) => {
                            errors.push(format!("alternative {i}: {}", e.message));
                            self.restore(snapshot);
                        }
                    }
                }

                Err(ParseError::new(
                    format!(
                        "no alternative matched for '{}': {}",
                        label.unwrap_or("?"),
                        errors.join("; ")
                    ),
                    self.current_span(),
                    vec![],
                ))
            }
            Expr::ZeroOrMore(inner) => {
                self.builder.push(label.unwrap_or("rep"));
                loop {
                    if self.is_eof() { break; }
                    let snapshot = self.snapshot();
                    match self.parse_expr(inner, None) {
                        Ok(child) => {
                            // Ensure we actually consumed something
                            if self.pos == snapshot.0 {
                                break; // prevent infinite loop on epsilon match
                            }
                            self.builder.add(child);
                        }
                        Err(_) => {
                            self.restore(snapshot);
                            break;
                        }
                    }
                }
                Ok(self.builder.pop().unwrap_or(CstNode::epsilon()))
            }
            Expr::OneOrMore(inner) => {
                self.builder.push(label.unwrap_or("rep1"));
                // Must match at least once
                let first = self.parse_expr(inner, None)?;
                self.builder.add(first);

                // Then zero or more
                loop {
                    if self.is_eof() { break; }
                    let snapshot = self.snapshot();
                    match self.parse_expr(inner, None) {
                        Ok(child) => {
                            if self.pos == snapshot.0 { break; }
                            self.builder.add(child);
                        }
                        Err(_) => {
                            self.restore(snapshot);
                            break;
                        }
                    }
                }
                Ok(self.builder.pop().unwrap_or(CstNode::epsilon()))
            }
            Expr::Opt(inner) => {
                self.builder.push(label.unwrap_or("opt"));
                let snapshot = self.snapshot();
                match self.parse_expr(inner, None) {
                    Ok(child) => {
                        self.builder.add(child);
                    }
                    Err(_) => {
                        self.restore(snapshot);
                        // Optional — empty is fine
                    }
                }
                Ok(self.builder.pop().unwrap_or(CstNode::epsilon()))
            }
            Expr::Token(kind_name) => {
                if self.is_eof() {
                    return Err(ParseError::new(
                        format!("expected token '{}', got EOF", kind_name),
                        Span::dummy(),
                        vec![kind_name.clone()],
                    ));
                }

                if self.current_kind() == kind_name {
                    let token = self.tokens[self.pos].clone();
                    self.advance();
                    Ok(CstNode::leaf(token))
                } else {
                    Err(ParseError::new(
                        format!(
                            "expected token '{}', got '{}'",
                            kind_name,
                            self.current_kind()
                        ),
                        self.current_span(),
                        vec![kind_name.clone()],
                    ))
                }
            }
            Expr::Lit(text) => {
                if self.is_eof() {
                    return Err(ParseError::new(
                        format!("expected '{}', got EOF", text),
                        Span::dummy(),
                        vec![text.clone()],
                    ));
                }

                if self.current_text() == text {
                    let token = self.tokens[self.pos].clone();
                    self.advance();
                    Ok(CstNode::leaf(token))
                } else {
                    Err(ParseError::new(
                        format!(
                            "expected '{}', got '{}'",
                            text,
                            self.current_text()
                        ),
                        self.current_span(),
                        vec![text.clone()],
                    ))
                }
            }
            Expr::Rule(name) => {
                self.parse_rule(name)
            }
            Expr::Epsilon => {
                Ok(CstNode::epsilon())
            }
        }
    }
}

// ============================================================
// Convenience functions
// ============================================================

/// Parse source text with a grammar and return the CST.
pub fn parse_with_grammar(source: &str, grammar: &Grammar) -> Result<CstNode, ParseError> {
    let parser = Parser::build(grammar.clone());
    parser.parse(source)
}

/// Parse source text with a grammar, starting from a specific rule.
pub fn parse_rule_with_grammar(
    source: &str,
    grammar: &Grammar,
    rule_name: &str,
) -> Result<CstNode, ParseError> {
    let parser = Parser::build(grammar.clone());
    parser.parse_rule(source, rule_name)
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::parse_grammar;

    fn make_grammar() -> Grammar {
        let src = r##"
token IDENT = "[a-z][a-z0-9_]*"
token REG = "[A-Z][A-Z0-9]*"
token DEC = "[0-9]+"
punct "(" ")" "," ";" "="
skip "[\n \t\r]+"
skip "#[^\n]*"

program ::= stmt*
stmt ::= inst_like | label_def
inst_like ::= IDENT (operand ("," operand)*)?
operand ::= IDENT | REG | DEC
label_def ::= IDENT ";"
"##;
        parse_grammar(src).unwrap()
    }

    #[test]
    fn test_parse_simple_inst() {
        let grammar = make_grammar();
        let parser = Parser::build(grammar);
        let cst = parser.parse_rule("mov rd, rs1", "inst_like").unwrap();

        assert_eq!(cst.kind, "inst_like");
        // CST structure: inst_like → [IDENT(mov), operand(rd), COMMA, operand(rs1)]
        // Find all IDENT tokens (nested inside operand nodes)
        let all_idents: Vec<String> = cst.leaves().iter()
            .filter(|t| t.kind == "IDENT")
            .map(|t| t.text.clone())
            .collect();
        assert_eq!(all_idents, vec!["mov", "rd", "rs1"]);
    }

    #[test]
    fn test_parse_with_reg() {
        let grammar = make_grammar();
        let parser = Parser::build(grammar);
        let cst = parser.parse_rule("push RBP", "inst_like").unwrap();

        // REG token is nested under operand
        let all_regs: Vec<String> = cst.leaves().iter()
            .filter(|t| t.kind == "REG")
            .map(|t| t.text.clone())
            .collect();
        assert!(!all_regs.is_empty());
        assert_eq!(all_regs[0], "RBP");
    }

    #[test]
    fn test_parse_label() {
        let grammar = make_grammar();
        let parser = Parser::build(grammar);
        let cst = parser.parse_rule("loop_start ;", "label_def").unwrap();

        assert_eq!(cst.kind, "label_def");
        let idents: Vec<String> = cst.leaves().iter()
            .filter(|t| t.kind == "IDENT")
            .map(|t| t.text.clone())
            .collect();
        assert_eq!(idents[0], "loop_start");
    }

    #[test]
    fn test_parse_multiple_stmts() {
        let grammar = make_grammar();
        let parser = Parser::build(grammar);
        let cst = parser.parse("mov rd, rs1\nadd rd, rs2").unwrap();

        assert_eq!(cst.kind, "program");
        // Should have 2 stmt children (one per line)
        let stmts = cst.children_by("stmt");
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn test_parse_alternation() {
        let grammar = make_grammar();
        let parser = Parser::build(grammar);

        // "ret" has 0 operands, should match inst_like (optional operand)
        let cst = parser.parse_rule("ret", "inst_like").unwrap();
        assert_eq!(cst.kind, "inst_like");

        // label_def should match "exit ;" directly
        let cst = parser.parse_rule("exit ;", "label_def").unwrap();
        assert_eq!(cst.kind, "label_def");
    }

    #[test]
    fn test_error_on_invalid() {
        let grammar = make_grammar();
        let parser = Parser::build(grammar);
        // REG is not valid as a mnemonic (mnemonic must be IDENT/lowercase)
        let result = parser.parse_rule("RAX", "inst_like");
        // RAX matches REG, not IDENT — so inst_like should fail
        assert!(result.is_err());
    }

    #[test]
    fn test_zero_or_more_operands() {
        let grammar = make_grammar();
        let parser = Parser::build(grammar);

        // inst_like with optional operand list: "ret" (0 operands)
        let cst = parser.parse_rule("ret", "inst_like").unwrap();
        assert_eq!(cst.kind, "inst_like");

        // inst_like with multiple operands: find nested operand nodes
        let cst = parser.parse_rule("add rd, rs1, rs2", "inst_like").unwrap();
        let operands: Vec<_> = cst.walk().filter(|n| n.kind == "operand").collect();
        assert_eq!(operands.len(), 3, "Expected 3 operands");
    }

    #[test]
    fn debug_lexer_output() {
        let grammar = make_grammar();
        let mut lexer = crate::lexer::Lexer::build(&grammar);
        let tokens = lexer.tokenize("mov rd, rs1").unwrap();
        let kinds: Vec<String> = tokens.iter().map(|t| format!("{}:'{}'", t.kind, t.text)).collect();
        eprintln!("Tokens: {:?}", kinds);
        // Expected: IDENT:mov, IDENT:rd, COMMA:,, IDENT:rs1, EOF:
        assert_eq!(tokens.len(), 5, "Expected 5 tokens, got {:?}", kinds);
        assert_eq!(tokens[0].kind, "IDENT");
        assert_eq!(tokens[0].text, "mov");
        assert_eq!(tokens[1].kind, "IDENT");
        assert_eq!(tokens[1].text, "rd");
        assert_eq!(tokens[2].kind, "COMMA");
        assert_eq!(tokens[3].kind, "IDENT");
        assert_eq!(tokens[3].text, "rs1");
    }
}
