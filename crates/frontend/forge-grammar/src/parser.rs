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
//! - `NotLookahead(A)` → succeed without consuming if A does NOT match
//! - `Token(name)` → consume if current token kind matches
//! - `Lit(text)` → consume if current token text matches
//! - `Rule(name)` → recursively call parse_rule(name)
//! - `Epsilon` → always succeeds, no tokens consumed
//!
//! # Error Recovery
//!
//! When `ParserConfig::error_recovery` is enabled, the parser collects
//! multiple errors and attempts to synchronize after each one by skipping
//! to the next sync point (punctuation, keywords, or rule-start tokens).

use crate::ast::TypedAst;
use crate::ast::lower::lower_cst;
use crate::ast::schema::AstSchema;
use crate::cst::{CstBuilder, CstNode};
use crate::error::{LowerError, ParseError, Span};
use crate::grammar::{Expr, Grammar};
use crate::lexer::Token;

// ============================================================
// Parser Configuration
// ============================================================

/// Configuration for parser behavior.
#[derive(Debug, Clone)]
pub struct ParserConfig {
    /// Enable error recovery: on parse failure, skip to sync point and continue.
    /// Default: false (strict mode — stop at first error).
    pub error_recovery: bool,
    /// Maximum number of errors to collect before giving up.
    /// Default: 20.
    pub max_errors: usize,
    /// Explicit sync tokens. When empty (default), sync tokens are auto-derived:
    /// all punctuation/literal tokens plus FIRST sets of all rules.
    pub sync_tokens: Vec<String>,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            error_recovery: false,
            max_errors: 20,
            sync_tokens: vec![],
        }
    }
}

// ============================================================
// Parser
// ============================================================

/// Runtime parser. Interprets grammar rules to parse a token stream.
pub struct Parser {
    /// The grammar driving this parser.
    grammar: Grammar,
    /// Parser configuration.
    config: ParserConfig,
}

impl Parser {
    /// Build a parser from a grammar definition with default config.
    pub fn build(grammar: Grammar) -> Self {
        Self {
            grammar,
            config: ParserConfig::default(),
        }
    }

    /// Build a parser with custom configuration.
    pub fn with_config(grammar: Grammar, config: ParserConfig) -> Self {
        Self { grammar, config }
    }

    /// Access the grammar.
    pub fn grammar(&self) -> &Grammar {
        &self.grammar
    }

    /// Access the parser configuration.
    pub fn config(&self) -> &ParserConfig {
        &self.config
    }

    /// Parse source text using the grammar's entry rule.
    /// Returns the CST root node.
    pub fn parse(&self, source: &str) -> Result<CstNode, ParseError> {
        self.parse_rule(source, &self.grammar.entry.clone())
    }

    /// Parse source text and lower directly to TypedAst.
    /// This combines lexing → parsing → lowering in one call.
    pub fn parse_to_ast(&self, source: &str, schema: &AstSchema) -> Result<TypedAst, LowerError> {
        let cst = self
            .parse(source)
            .map_err(|e| LowerError::lowering(format!("parse error: {}", e), e.span))?;
        lower_cst(&cst, schema, &self.grammar)
    }

    /// Parse source text starting from a specific rule and lower to TypedAst.
    pub fn parse_rule_to_ast(
        &self,
        source: &str,
        rule_name: &str,
        schema: &AstSchema,
    ) -> Result<TypedAst, LowerError> {
        let cst = self
            .parse_rule(source, rule_name)
            .map_err(|e| LowerError::lowering(format!("parse error: {}", e), e.span))?;
        lower_cst(&cst, schema, &self.grammar)
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

        // Compute sync tokens for error recovery
        let sync_tokens = if self.config.error_recovery {
            self.compute_sync_tokens()
        } else {
            vec![]
        };

        // Parse
        let mut ctx = ParseContext::new(&self.grammar, tokens, sync_tokens, self.config.max_errors);
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

    /// Parse with error recovery, returning (CST_or_partial, collected_errors).
    pub fn parse_with_recovery(&self, source: &str) -> (Option<CstNode>, Vec<ParseError>) {
        let mut lexer = crate::lexer::Lexer::build(&self.grammar);
        let tokens = match lexer.tokenize(source) {
            Ok(t) => t,
            Err(e) => {
                return (
                    None,
                    vec![ParseError {
                        message: e.to_string(),
                        span: e.span,
                        expected: vec![],
                    }],
                );
            }
        };

        if !self.grammar.rules.contains_key(&self.grammar.entry) {
            return (
                None,
                vec![ParseError::new(
                    format!("unknown entry rule '{}'", self.grammar.entry),
                    Span::dummy(),
                    vec![],
                )],
            );
        }

        let sync_tokens = self.compute_sync_tokens();
        let mut ctx = ParseContext::new(&self.grammar, tokens, sync_tokens, self.config.max_errors);

        let result = ctx.parse_rule(&self.grammar.entry.clone());
        let errors = std::mem::take(&mut ctx.errors);

        match result {
            Ok(cst) => (Some(cst), errors),
            Err(e) => {
                let mut all_errors = errors;
                all_errors.push(e);
                (None, all_errors)
            }
        }
    }

    /// Compute sync tokens for error recovery:
    /// all literal/punctuation tokens + FIRST token of every rule.
    fn compute_sync_tokens(&self) -> Vec<String> {
        if !self.config.sync_tokens.is_empty() {
            return self.config.sync_tokens.clone();
        }
        let mut syncs = Vec::new();
        // All literal/punctuation token kinds
        for token in &self.grammar.tokens {
            if token.is_literal() {
                syncs.push(token.name.clone());
            }
        }
        // FIRST sets of all rules
        for rule in self.grammar.rules.values() {
            for tok in self.grammar.first_tokens(&rule.expr) {
                if tok != "<lit>" && !syncs.contains(&tok) {
                    syncs.push(tok);
                }
            }
        }
        syncs
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
    /// Accumulated parse errors (for error recovery mode).
    errors: Vec<ParseError>,
    /// Token kinds to synchronize on after an error.
    sync_tokens: Vec<String>,
    /// Maximum errors before aborting recovery.
    max_errors: usize,
}

impl<'a> ParseContext<'a> {
    fn new(
        grammar: &'a Grammar,
        tokens: Vec<Token>,
        sync_tokens: Vec<String>,
        max_errors: usize,
    ) -> Self {
        Self {
            grammar,
            tokens,
            pos: 0,
            builder: CstBuilder::new(),
            errors: Vec::new(),
            sync_tokens,
            max_errors,
        }
    }

    /// Check if recovery is active.
    fn recovery_enabled(&self) -> bool {
        !self.sync_tokens.is_empty()
    }

    /// Record an error and attempt recovery by skipping to next sync point.
    /// Returns true if parsing may continue, false if max errors reached.
    fn record_error(&mut self, error: ParseError) -> bool {
        self.errors.push(error);
        if self.errors.len() >= self.max_errors {
            return false;
        }
        self.recover()
    }

    /// Skip tokens until a sync point is found.
    fn recover(&mut self) -> bool {
        while !self.is_eof() {
            if self.sync_tokens.iter().any(|t| *t == self.current_kind()) {
                return true; // found sync point
            }
            self.advance();
        }
        !self.is_eof()
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
        let rule = self.grammar.rules.get(name).ok_or_else(|| {
            ParseError::new(
                format!("unknown rule '{}'", name),
                self.current_span(),
                vec![],
            )
        })?;

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

                let err = ParseError::new(
                    format!(
                        "no alternative matched for '{}': {}",
                        label.unwrap_or("?"),
                        errors.join("; ")
                    ),
                    self.current_span(),
                    vec![],
                );

                // In recovery mode: record error, skip to sync, return epsilon
                if self.recovery_enabled() && self.record_error(err.clone()) {
                    self.builder.push(label.unwrap_or("alt"));
                    let node = self.builder.pop().unwrap_or(CstNode::epsilon());
                    return Ok(node);
                }

                Err(err)
            }
            Expr::ZeroOrMore(inner) => {
                self.builder.push(label.unwrap_or("rep"));
                loop {
                    if self.is_eof() {
                        break;
                    }
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
                    if self.is_eof() {
                        break;
                    }
                    let snapshot = self.snapshot();
                    match self.parse_expr(inner, None) {
                        Ok(child) => {
                            if self.pos == snapshot.0 {
                                break;
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
                    let err = ParseError::new(
                        format!("expected token '{}', got EOF", kind_name),
                        Span::dummy(),
                        vec![kind_name.clone()],
                    );
                    if self.recovery_enabled() && self.record_error(err.clone()) {
                        return Ok(CstNode::epsilon());
                    }
                    return Err(err);
                }

                if self.current_kind() == kind_name {
                    let token = self.tokens[self.pos].clone();
                    self.advance();
                    Ok(CstNode::leaf(token))
                } else {
                    let err = ParseError::new(
                        format!(
                            "expected token '{}', got '{}'",
                            kind_name,
                            self.current_kind()
                        ),
                        self.current_span(),
                        vec![kind_name.clone()],
                    );
                    if self.recovery_enabled() && self.record_error(err.clone()) {
                        return Ok(CstNode::epsilon());
                    }
                    Err(err)
                }
            }
            Expr::Lit(text) => {
                if self.is_eof() {
                    let err = ParseError::new(
                        format!("expected '{}', got EOF", text),
                        Span::dummy(),
                        vec![text.clone()],
                    );
                    if self.recovery_enabled() && self.record_error(err.clone()) {
                        return Ok(CstNode::epsilon());
                    }
                    return Err(err);
                }

                if self.current_text() == text {
                    let token = self.tokens[self.pos].clone();
                    self.advance();
                    Ok(CstNode::leaf(token))
                } else {
                    let err = ParseError::new(
                        format!("expected '{}', got '{}'", text, self.current_text()),
                        self.current_span(),
                        vec![text.clone()],
                    );
                    if self.recovery_enabled() && self.record_error(err.clone()) {
                        return Ok(CstNode::epsilon());
                    }
                    Err(err)
                }
            }
            Expr::NotLookahead(inner) => {
                // Negative lookahead: succeed without consuming if inner does NOT match
                let snapshot = self.snapshot();
                match self.parse_expr(inner, None) {
                    Ok(_) => {
                        self.restore(snapshot);
                        Err(ParseError::new(
                            "negative lookahead matched (expected it to fail)".to_string(),
                            self.current_span(),
                            vec![],
                        ))
                    }
                    Err(_) => {
                        self.restore(snapshot);
                        Ok(CstNode::epsilon())
                    }
                }
            }
            Expr::Rule(name) => self.parse_rule(name),
            Expr::Epsilon => Ok(CstNode::epsilon()),
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
        let all_idents: Vec<String> = cst
            .leaves()
            .iter()
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
        let all_regs: Vec<String> = cst
            .leaves()
            .iter()
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
        let idents: Vec<String> = cst
            .leaves()
            .iter()
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
        let kinds: Vec<String> = tokens
            .iter()
            .map(|t| format!("{}:'{}'", t.kind, t.text))
            .collect();
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

    #[test]
    fn test_error_recovery_collects_errors() {
        let grammar = make_grammar();
        let parser = Parser::with_config(
            grammar,
            ParserConfig {
                error_recovery: true,
                max_errors: 20,
                sync_tokens: vec![],
            },
        );
        // Input starts with a number (DEC), which is not valid as a mnemonic (needs IDENT).
        // Error recovery should record this and attempt to continue.
        let (_cst, errors) = parser.parse_with_recovery("123 mov rd, rs1");
        // Error recovery must have collected at least one error.
        assert!(
            !errors.is_empty(),
            "Expected error recovery to collect errors, got none"
        );
    }

    /// Build a grammar programmatically that uses `NotLookahead(!REG)`.
    fn make_not_lookahead_grammar() -> Grammar {
        let mut rules = std::collections::HashMap::new();
        rules.insert(
            "not_reg".to_string(),
            crate::grammar::RuleDef {
                name: "not_reg".to_string(),
                // !REG IDENT  — succeed only if the next token is NOT a REG
                expr: Expr::seq(vec![
                    Expr::NotLookahead(Box::new(Expr::Token("REG".to_string()))),
                    Expr::Token("IDENT".to_string()),
                ]),
            },
        );

        Grammar {
            tokens: vec![
                crate::grammar::TokenDef::regex("IDENT", "[a-z]+"),
                crate::grammar::TokenDef::regex("REG", "[A-Z]+"),
            ],
            skips: vec!["[ \t]+".to_string()],
            rules,
            entry: "not_reg".to_string(),
        }
    }

    #[test]
    fn test_not_lookahead_rejects_when_token_matches() {
        let grammar = make_not_lookahead_grammar();
        let parser = Parser::build(grammar);
        // "ABC" is a REG — !REG should fail, causing the whole parse to fail.
        let result = parser.parse("ABC");
        assert!(
            result.is_err(),
            "Expected parse to fail because REG matched inside NotLookahead"
        );
    }

    #[test]
    fn test_not_lookahead_succeeds_when_token_does_not_match() {
        let grammar = make_not_lookahead_grammar();
        let parser = Parser::build(grammar);
        // "abc" is an IDENT — !REG succeeds (REG does NOT match), so the parse succeeds.
        let cst = parser.parse("abc").unwrap();
        assert_eq!(cst.kind, "not_reg");
        // The CST should contain the IDENT leaf.
        let leaves: Vec<String> = cst.leaves().iter().map(|t| t.text.clone()).collect();
        assert_eq!(leaves, vec!["abc"]);
    }

    #[test]
    fn debug_ir_parse_simple() {
        let ir_src = include_str!("../../../foundation/forge-ir/grammars/ir.lx");
        let grammar = crate::grammar::parse_grammar(ir_src).unwrap();
        let parser = Parser::with_config(
            grammar,
            ParserConfig {
                error_recovery: false,
                ..Default::default()
            },
        );
        let result = parser.parse_rule("fn foo() -> i32 { entry: ret i32 }", "module");
        match result {
            Ok(cst) => {
                eprintln!(
                    "CST module kind={}, children.len={}",
                    cst.kind,
                    cst.children.len()
                );
                for child in &cst.children {
                    eprintln!(
                        "  child kind={}, children.len={}",
                        child.kind,
                        child.children.len()
                    );
                }
                for node in cst.walk() {
                    eprintln!(
                        "  {} [{}]",
                        node.kind,
                        if node.is_leaf() { node.text() } else { "" }
                    );
                }
            }
            Err(e) => {
                eprintln!("Parse error: {}", e);
                panic!("IR parse failed: {}", e);
            }
        }
    }

    #[test]
    fn debug_ir_parse_multi() {
        let ir_src = include_str!("../../../foundation/forge-ir/grammars/ir.lx");
        let grammar = crate::grammar::parse_grammar(ir_src).unwrap();
        let parser = Parser::with_config(
            grammar,
            ParserConfig {
                error_recovery: false,
                ..Default::default()
            },
        );
        let result = parser.parse_rule(
            "fn f1() -> i32 { entry: ret i32 } fn f2() -> i64 { entry: ret i64 }",
            "module",
        );
        match result {
            Ok(cst) => {
                eprintln!("MULTI OK: children={}", cst.children.len());
                for (i, child) in cst.children.iter().enumerate() {
                    let flat = child.flatten();
                    eprintln!(
                        "  child[{}]: kind={}, flat.kind={}",
                        i, child.kind, flat.kind
                    );
                }
            }
            Err(e) => {
                eprintln!("MULTI ERROR: {}", e);
            }
        }
    }
}
