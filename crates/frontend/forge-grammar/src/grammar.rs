//! Grammar IR and .lx file parser.
//!
//! # .lx Grammar File Format
//!
//! The `.lx` format uses a line-oriented EBNF-like syntax:
//!
//! ```text
//! # Line comment
//! token IDENT = "[a-zA-Z_][a-zA-Z0-9_]*"
//! token REG   = "[A-Z][A-Z0-9]*"
//! punct "(" ")" "{" "}" "," ":" ";" "=" "@"
//! skip "[ \\t\\r]+"
//! skip "#[^\\n]*"
//!
//! # Grammar rules
//! program ::= item*
//! item ::= macro_def | stmt
//! block ::= "{" stmt* "}"
//! ```
//!
//! # Expression types:
//! - `A B C`       Sequence
//! - `A | B`       Alternation (first-match wins)
//! - `A*`           Zero or more
//! - `A+`           One or more
//! - `A?`           Optional
//! - `"text"`       Terminal literal (matched against token text)
//! - `TOKEN`        Named token kind reference
//! - `rule_name`    Non-terminal rule reference
//! - `( A B )`     Grouping (parsed, but equivalent to sequence)
//! - `ε` or empty   Epsilon (empty match)

use crate::error::GrammarError;
use std::collections::HashMap;

// ============================================================
// Grammar IR
// ============================================================

/// A complete grammar definition.
#[derive(Debug, Clone)]
pub struct Grammar {
    /// Token definitions in priority order (first = highest).
    pub tokens: Vec<TokenDef>,
    /// Skip patterns (whitespace, comments).
    pub skips: Vec<String>,
    /// Grammar rules.
    pub rules: HashMap<String, RuleDef>,
    /// Entry rule name.
    pub entry: String,
}

impl Grammar {
    /// Merge another grammar into this one.
    /// - Tokens from `other` are inserted at the front (higher priority for longest-match).
    /// - Rules from `other` override existing rules with the same name.
    /// - Skip patterns from `other` are appended.
    ///
    /// Returns an error if the merged grammar fails validation.
    pub fn merge(&mut self, other: Grammar) -> Result<(), GrammarError> {
        // Insert tokens from other at front (higher priority)
        for token in other.tokens.into_iter().rev() {
            if !self.tokens.iter().any(|t| t.name == token.name) {
                self.tokens.insert(0, token);
            }
        }
        // Override/add rules
        for (name, rule) in other.rules {
            self.rules.insert(name, rule);
        }
        // Append skips
        self.skips.extend(other.skips);
        Ok(())
    }

    /// Extend or prepend alternatives to an existing rule.
    /// If the rule exists, `alternatives` is prepended (higher priority in first-match alternation).
    /// If the rule does not exist, it is created.
    pub fn extend_rule(&mut self, name: &str, alternatives: Expr) {
        if let Some(existing) = self.rules.get_mut(name) {
            let merged = match (&existing.expr, alternatives) {
                (Expr::Alt(a), Expr::Alt(b)) => {
                    let mut combined = b;
                    combined.extend(a.clone());
                    Expr::Alt(combined)
                }
                (existing_expr, Expr::Alt(b)) => {
                    let mut combined = b;
                    combined.push(existing_expr.clone());
                    Expr::Alt(combined)
                }
                (_, new_alt) => Expr::Alt(vec![new_alt, existing.expr.clone()]),
            };
            existing.expr = merged;
        } else {
            self.rules.insert(
                name.to_string(),
                RuleDef {
                    name: name.to_string(),
                    expr: alternatives,
                },
            );
        }
    }

    /// Deep validation: returns diagnostics for potential issues.
    ///
    /// Checks:
    /// - Unused token definitions
    /// - Unreachable rules (not referenced and not entry)
    /// - Potentially ambiguous alternatives (same FIRST token for multiple alts)
    pub fn validate_deep(&self) -> Vec<GrammarDiagnostic> {
        let mut diags = Vec::new();

        // 1. Unused token definitions
        let used_tokens: std::collections::HashSet<String> = self
            .rules
            .values()
            .flat_map(|r| self.collect_token_refs(&r.expr))
            .collect();
        for token in &self.tokens {
            if !token.is_literal() && !used_tokens.contains(&token.name) {
                diags.push(GrammarDiagnostic {
                    level: DiagnosticLevel::Warning,
                    message: format!(
                        "token '{}' is defined but never used in any rule",
                        token.name
                    ),
                });
            }
        }

        // 2. Unreachable rules
        let referenced: std::collections::HashSet<String> = self
            .rules
            .values()
            .flat_map(|r| self.collect_rule_refs(&r.expr))
            .collect();
        for name in self.rules.keys() {
            if name != &self.entry && !referenced.contains(name) {
                diags.push(GrammarDiagnostic {
                    level: DiagnosticLevel::Warning,
                    message: format!(
                        "rule '{}' is not referenced by any other rule and is not the entry rule",
                        name
                    ),
                });
            }
        }

        // 3. Ambiguous alternatives: check Alt branches with same FIRST token
        for (rule_name, rule) in &self.rules {
            self.check_ambiguous_alts(&rule.expr, rule_name, &mut diags);
        }

        diags
    }

    /// Collect all token kind names referenced in an expression.
    fn collect_token_refs(&self, expr: &Expr) -> Vec<String> {
        let mut tokens = Vec::new();
        match expr {
            Expr::Token(name) => tokens.push(name.clone()),
            Expr::Seq(items) | Expr::Alt(items) => {
                for item in items {
                    tokens.extend(self.collect_token_refs(item));
                }
            }
            Expr::ZeroOrMore(e) | Expr::OneOrMore(e) | Expr::Opt(e) | Expr::NotLookahead(e) => {
                tokens.extend(self.collect_token_refs(e));
            }
            Expr::Rule(name) => {
                if let Some(rule) = self.rules.get(name) {
                    tokens.extend(self.collect_token_refs(&rule.expr));
                }
            }
            _ => {}
        }
        tokens
    }

    /// Collect all rule names referenced in an expression.
    fn collect_rule_refs(&self, expr: &Expr) -> Vec<String> {
        let mut rules = Vec::new();
        match expr {
            Expr::Rule(name) => rules.push(name.clone()),
            Expr::Seq(items) | Expr::Alt(items) => {
                for item in items {
                    rules.extend(self.collect_rule_refs(item));
                }
            }
            Expr::ZeroOrMore(e) | Expr::OneOrMore(e) | Expr::Opt(e) | Expr::NotLookahead(e) => {
                rules.extend(self.collect_rule_refs(e));
            }
            _ => {}
        }
        rules
    }

    /// Check alternation branches for same FIRST token (potential ambiguity).
    fn check_ambiguous_alts(
        &self,
        expr: &Expr,
        rule_name: &str,
        diags: &mut Vec<GrammarDiagnostic>,
    ) {
        match expr {
            Expr::Alt(items) => {
                // Collect first tokens for each alternative
                let first_sets: Vec<(usize, Vec<String>)> = items
                    .iter()
                    .enumerate()
                    .map(|(i, alt)| (i, self.first_tokens(alt)))
                    .collect();

                for i in 0..first_sets.len() {
                    for j in (i + 1)..first_sets.len() {
                        let common: Vec<_> = first_sets[i]
                            .1
                            .iter()
                            .filter(|t| first_sets[j].1.contains(t))
                            .collect();
                        if !common.is_empty() {
                            diags.push(GrammarDiagnostic {
                                level: DiagnosticLevel::Warning,
                                message: format!(
                                    "rule '{}': alternatives {} and {} share first tokens {:?} — may cause ambiguity",
                                    rule_name, i, j, common
                                ),
                            });
                        }
                    }
                }
                // Recurse
                for item in items {
                    self.check_ambiguous_alts(item, rule_name, diags);
                }
            }
            Expr::Seq(items) => {
                for item in items {
                    self.check_ambiguous_alts(item, rule_name, diags);
                }
            }
            Expr::ZeroOrMore(e) | Expr::OneOrMore(e) | Expr::Opt(e) | Expr::NotLookahead(e) => {
                self.check_ambiguous_alts(e, rule_name, diags);
            }
            _ => {}
        }
    }

    /// Compute the FIRST set of an expression (token kind names).
    pub fn first_tokens(&self, expr: &Expr) -> Vec<String> {
        let mut tokens = Vec::new();
        match expr {
            Expr::Token(name) => {
                tokens.push(name.clone());
            }
            Expr::Lit(_) => {
                // Literal matches by text, not token kind — use a marker
                tokens.push("<lit>".to_string());
            }
            Expr::Rule(name) => {
                if let Some(rule) = self.rules.get(name) {
                    tokens.extend(self.first_tokens(&rule.expr));
                }
            }
            Expr::Seq(items) => {
                for item in items {
                    tokens.extend(self.first_tokens(item));
                    if !self.is_nullable(item) {
                        break;
                    }
                }
            }
            Expr::Alt(items) => {
                for item in items {
                    tokens.extend(self.first_tokens(item));
                }
            }
            Expr::ZeroOrMore(_)
            | Expr::OneOrMore(_)
            | Expr::Opt(_)
            | Expr::NotLookahead(_)
            | Expr::Epsilon => {}
        }
        tokens
    }

    /// Check if expression is nullable (public helper for FIRST computation).
    fn is_nullable(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Epsilon | Expr::ZeroOrMore(_) | Expr::Opt(_) | Expr::NotLookahead(_) => true,
            Expr::Seq(items) => items.iter().all(|e| self.is_nullable(e)),
            Expr::Alt(items) => items.iter().any(|e| self.is_nullable(e)),
            _ => false,
        }
    }
}

/// Diagnostic level for grammar validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Warning,
    Error,
}

/// A diagnostic from grammar deep validation.
#[derive(Debug, Clone)]
pub struct GrammarDiagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
}

/// A token definition.
#[derive(Debug, Clone)]
pub struct TokenDef {
    pub name: String,
    pub pattern: TokenPattern,
}

/// How a token is matched.
#[derive(Debug, Clone)]
pub enum TokenPattern {
    /// Regex pattern: `IDENT = "[a-z][a-z0-9]*"`
    Regex(String),
    /// Exact string literal: `"("`, `"{"`, `"=="`
    Literal(String),
}

impl TokenDef {
    pub fn regex(name: &str, pattern: &str) -> Self {
        Self {
            name: name.to_string(),
            pattern: TokenPattern::Regex(pattern.to_string()),
        }
    }

    pub fn literal(text: &str) -> Self {
        let name = literal_token_name(text);
        Self {
            name,
            pattern: TokenPattern::Literal(text.to_string()),
        }
    }

    /// Is this a literal/punctuation token?
    pub fn is_literal(&self) -> bool {
        matches!(self.pattern, TokenPattern::Literal(_))
    }

    /// Get the literal text if this is a literal token.
    pub fn as_literal(&self) -> Option<&str> {
        match &self.pattern {
            TokenPattern::Literal(s) => Some(s),
            _ => None,
        }
    }
}

/// Generate a canonical token name from a literal string.
pub fn literal_token_name(lit: &str) -> String {
    // Map multi-char operators to readable names
    match lit {
        "<=" => "LE".to_string(),
        ">=" => "GE".to_string(),
        "==" => "EQEQ".to_string(),
        "!=" => "NE".to_string(),
        "&&" => "AND".to_string(),
        "||" => "OR".to_string(),
        "->" => "ARROW".to_string(),
        // Single chars
        "(" => "LPAREN".to_string(),
        ")" => "RPAREN".to_string(),
        "{" => "LBRACE".to_string(),
        "}" => "RBRACE".to_string(),
        "[" => "LBRACKET".to_string(),
        "]" => "RBRACKET".to_string(),
        "," => "COMMA".to_string(),
        ":" => "COLON".to_string(),
        ";" => "SEMI".to_string(),
        "=" => "EQ".to_string(),
        "@" => "AT".to_string(),
        "." => "DOT".to_string(),
        "+" => "PLUS".to_string(),
        "-" => "MINUS".to_string(),
        "*" => "STAR".to_string(),
        "/" => "SLASH".to_string(),
        "%" => "PERCENT".to_string(),
        "<" => "LT".to_string(),
        ">" => "GT".to_string(),
        "!" => "NOT".to_string(),
        "~" => "TILDE".to_string(),
        "&" => "AMP".to_string(),
        "|" => "PIPE".to_string(),
        other => {
            // Generate a stable numeric suffix from up to 7 chars (fits in u64 with base-256 encoding).
            let hash: u64 = other
                .chars()
                .take(7)
                .map(|c| c as u32)
                .fold(0u64, |a, b| a.wrapping_mul(256).wrapping_add(b as u64));
            format!("PUNCT_{:x}", hash)
        }
    }
}

/// A grammar rule definition.
#[derive(Debug, Clone)]
pub struct RuleDef {
    pub name: String,
    pub expr: Expr,
}

// ============================================================
// EBNF Expressions
// ============================================================

/// An EBNF expression node.
#[derive(Debug, Clone)]
pub enum Expr {
    /// Sequence: A B C
    Seq(Vec<Expr>),
    /// Alternation: A | B | C (first-match wins)
    Alt(Vec<Expr>),
    /// Zero or more: A*
    ZeroOrMore(Box<Expr>),
    /// One or more: A+
    OneOrMore(Box<Expr>),
    /// Optional: A?
    Opt(Box<Expr>),
    /// Token reference by kind name: `IDENT`, `REG`, `LPAREN`
    Token(String),
    /// Rule reference: `stmt`, `operand`, `expr`
    Rule(String),
    /// String literal terminal: matches exact token text
    Lit(String),
    /// Epsilon (empty)
    Epsilon,
    /// Negative lookahead: !A (succeeds without consuming if A does NOT match)
    NotLookahead(Box<Expr>),
}

impl Expr {
    /// Construct a sequence from a list of expressions.
    pub fn seq(items: Vec<Expr>) -> Self {
        if items.len() == 1 {
            items.into_iter().next().unwrap()
        } else {
            Expr::Seq(items)
        }
    }

    /// Construct an alternation from a list of alternatives.
    pub fn alt(items: Vec<Expr>) -> Self {
        if items.len() == 1 {
            items.into_iter().next().unwrap()
        } else {
            Expr::Alt(items)
        }
    }

    /// Convenience: match a string literal.
    pub fn lit(s: &str) -> Self {
        Expr::Lit(s.to_string())
    }

    /// Convenience: reference a token kind.
    pub fn token(name: &str) -> Self {
        Expr::Token(name.to_string())
    }

    /// Convenience: reference a grammar rule.
    pub fn rule(name: &str) -> Self {
        Expr::Rule(name.to_string())
    }
}

// ============================================================
// .lx Grammar Parser
// ============================================================

/// Parse a .lx grammar source string into a [`Grammar`].
pub fn parse_grammar(source: &str) -> Result<Grammar, GrammarError> {
    let mut parser = GrammarParser::new(source);
    parser.parse()
}

struct GrammarParser {
    lines: Vec<LineInfo>,
    tokens: Vec<TokenDef>,
    skips: Vec<String>,
    rules: HashMap<String, RuleDef>,
    /// Which token names are literal tokens (used to distinguish Lit vs Token in rules).
    literal_token_names: Vec<String>,
    /// Rule definition order (first defined = entry rule).
    rule_order: Vec<String>,
}

#[derive(Clone)]
struct LineInfo {
    text: String,
    line_num: usize,
}

impl GrammarParser {
    fn new(source: &str) -> Self {
        let lines: Vec<LineInfo> = source
            .lines()
            .enumerate()
            .map(|(i, s)| LineInfo {
                text: s.to_string(),
                line_num: i,
            })
            .collect();
        Self {
            lines,
            tokens: Vec::new(),
            skips: Vec::new(),
            rules: HashMap::new(),
            literal_token_names: Vec::new(),
            rule_order: Vec::new(),
        }
    }

    fn parse(&mut self) -> Result<Grammar, GrammarError> {
        // First pass: collect token/punct/skip definitions
        // Clone lines to avoid borrow conflict with &mut self methods
        let lines: Vec<LineInfo> = self
            .lines
            .iter()
            .map(|l| LineInfo {
                text: l.text.clone(),
                line_num: l.line_num,
            })
            .collect();
        let mut rule_lines = Vec::new();

        for line in &lines {
            let trimmed = line.text.trim();
            // Skip empty lines and comments
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            if let Some(rest) = trimmed.strip_prefix("token ") {
                self.parse_token_def(rest, line.line_num)?;
            } else if let Some(rest) = trimmed.strip_prefix("punct ") {
                self.parse_punct_def(rest, line.line_num)?;
            } else if let Some(rest) = trimmed.strip_prefix("skip ") {
                self.parse_skip_def(rest, line.line_num)?;
            } else if trimmed.contains("::=") {
                rule_lines.push(line.clone());
            } else if trimmed.starts_with('|') || trimmed.starts_with(';') {
                // Continuation of previous rule (alternation or sequence)
                if let Some(last) = rule_lines.last_mut() {
                    last.text.push(' ');
                    last.text.push_str(trimmed);
                } else {
                    // Orphan continuation — might be an alternative on its own line
                    return Err(GrammarError::Parse {
                        message: format!("orphan continuation: '{}'", trimmed),
                        line: line.line_num,
                    });
                }
            } else {
                return Err(GrammarError::Parse {
                    message: format!("unexpected line: '{}'", trimmed),
                    line: line.line_num,
                });
            }
        }

        // Second pass: parse grammar rules
        for line in &rule_lines {
            self.parse_rule_def(&line.text, line.line_num)?;
        }

        // Validation
        if self.rules.is_empty() {
            return Err(GrammarError::EmptyGrammar);
        }

        // Find entry rule (first rule defined)
        let entry = self
            .rule_order
            .first()
            .ok_or(GrammarError::EmptyGrammar)?
            .clone();

        // Check for left recursion
        for name in self.rules.keys().cloned().collect::<Vec<_>>() {
            if self.is_left_recursive(&name, &name, &mut Vec::new()) {
                return Err(GrammarError::LeftRecursion(name));
            }
        }

        // Check rule references
        for rule in self.rules.values() {
            self.validate_expr(&rule.expr, &rule.name)?;
        }

        Ok(Grammar {
            tokens: std::mem::take(&mut self.tokens),
            skips: std::mem::take(&mut self.skips),
            rules: std::mem::take(&mut self.rules),
            entry,
        })
    }

    fn parse_token_def(&mut self, rest: &str, line: usize) -> Result<(), GrammarError> {
        // Format: NAME = "regex"
        let parts: Vec<&str> = rest.splitn(2, '=').collect();
        if parts.len() != 2 {
            return Err(GrammarError::Parse {
                message: "token definition must be 'NAME = \"regex\"'".to_string(),
                line,
            });
        }
        let name = parts[0].trim().to_string();
        let pattern = unquote(parts[1].trim());

        // Check for duplicate
        if self.tokens.iter().any(|t| t.name == name) {
            return Err(GrammarError::DuplicateToken(name));
        }

        self.tokens.push(TokenDef {
            name,
            pattern: TokenPattern::Regex(pattern),
        });
        Ok(())
    }

    fn parse_punct_def(&mut self, rest: &str, line: usize) -> Result<(), GrammarError> {
        // Format: "lit1" "lit2" "lit3" ...
        let literals = parse_quoted_strings(rest);
        if literals.is_empty() {
            return Err(GrammarError::Parse {
                message: "punct definition must have at least one quoted literal".to_string(),
                line,
            });
        }
        for lit in &literals {
            let name = literal_token_name(lit);
            if self.tokens.iter().any(|t| t.name == name) {
                return Err(GrammarError::DuplicateToken(name));
            }
            self.literal_token_names.push(name.clone());
            self.tokens.push(TokenDef {
                name,
                pattern: TokenPattern::Literal(lit.clone()),
            });
        }
        // Sort tokens so literals come before regex tokens (longest match first for literals)
        self.sort_tokens();
        Ok(())
    }

    fn parse_skip_def(&mut self, rest: &str, _line: usize) -> Result<(), GrammarError> {
        let pattern = unquote(rest.trim());
        self.skips.push(pattern);
        Ok(())
    }

    fn parse_rule_def(&mut self, text: &str, line: usize) -> Result<(), GrammarError> {
        // Format: NAME ::= expression
        let parts: Vec<&str> = text.splitn(2, "::=").collect();
        if parts.len() != 2 {
            return Err(GrammarError::Parse {
                message: "rule must be 'NAME ::= expression'".to_string(),
                line,
            });
        }
        let rule_name = parts[0].trim().to_string();
        let expr_str = parts[1].trim();

        if self.rules.contains_key(&rule_name) {
            return Err(GrammarError::DuplicateRule(rule_name));
        }

        let expr = self.parse_expr(expr_str, line)?;
        let name_for_def = rule_name.clone();
        self.rule_order.push(rule_name.clone());
        self.rules.insert(
            rule_name,
            RuleDef {
                name: name_for_def,
                expr,
            },
        );
        Ok(())
    }

    fn parse_expr(&self, s: &str, line: usize) -> Result<Expr, GrammarError> {
        self._parse_alt(s, line)
    }

    /// Parse alternation (lowest precedence): A | B | C
    fn _parse_alt(&self, s: &str, line: usize) -> Result<Expr, GrammarError> {
        let alternatives = self.split_on_bar(s);
        if alternatives.len() > 1 {
            let exprs: Result<Vec<Expr>, _> = alternatives
                .iter()
                .map(|a| self._parse_seq(a.trim(), line))
                .collect();
            return Ok(Expr::alt(exprs?));
        }
        self._parse_seq(s, line)
    }

    /// Parse sequence: A B C ... with postfix operators (* + ?)
    fn _parse_seq(&self, s: &str, line: usize) -> Result<Expr, GrammarError> {
        let tokens = self.tokenize_expr(s);
        let mut items = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            let (expr, next) = self._parse_atom(&tokens, i, line)?;
            items.push(expr);
            i = next;
        }
        Ok(Expr::seq(items))
    }

    fn _parse_atom(
        &self,
        tokens: &[String],
        pos: usize,
        line: usize,
    ) -> Result<(Expr, usize), GrammarError> {
        if pos >= tokens.len() {
            return Ok((Expr::Epsilon, pos));
        }

        let token = &tokens[pos];
        let (mut expr, mut next_pos) = if token == "(" {
            // Group: find matching )
            let (group_str, end_pos) = self.find_matching_paren(tokens, pos)?;
            let inner = self.parse_expr(&group_str, line)?;
            (inner, end_pos + 1)
        } else if token.starts_with('"') && token.ends_with('"') {
            // String literal terminal
            let lit = token[1..token.len() - 1].to_string();
            let tok_name = literal_token_name(&lit);
            if self.literal_token_names.contains(&tok_name) {
                (Expr::Token(tok_name), pos + 1)
            } else {
                (Expr::Lit(lit), pos + 1)
            }
        } else if token.chars().all(|c| c.is_uppercase() || c == '_') && !token.is_empty() {
            // All-uppercase → token kind reference
            (Expr::Token(token.clone()), pos + 1)
        } else if token == "ε" || token == "epsilon" {
            (Expr::Epsilon, pos + 1)
        } else {
            // Rule reference (lowercase or mixed case)
            (Expr::Rule(token.clone()), pos + 1)
        };

        // Check for postfix operators (*, +, ?)
        if next_pos < tokens.len() {
            match tokens[next_pos].as_str() {
                "*" => {
                    expr = Expr::ZeroOrMore(Box::new(expr));
                    next_pos += 1;
                }
                "+" => {
                    expr = Expr::OneOrMore(Box::new(expr));
                    next_pos += 1;
                }
                "?" => {
                    expr = Expr::Opt(Box::new(expr));
                    next_pos += 1;
                }
                _ => {}
            }
        }

        Ok((expr, next_pos))
    }

    fn find_matching_paren(
        &self,
        tokens: &[String],
        open_pos: usize,
    ) -> Result<(String, usize), GrammarError> {
        let mut depth = 0;
        let mut content = String::new();
        let mut i = open_pos;
        while i < tokens.len() {
            let t = &tokens[i];
            if t == "(" {
                if depth > 0 {
                    if !content.is_empty() {
                        content.push(' ');
                    }
                    content.push_str(t);
                }
                depth += 1;
            } else if t == ")" {
                depth -= 1;
                if depth == 0 {
                    return Ok((content, i));
                }
                if !content.is_empty() {
                    content.push(' ');
                }
                content.push_str(t);
            } else {
                if !content.is_empty() {
                    content.push(' ');
                }
                content.push_str(t);
            }
            i += 1;
        }
        Err(GrammarError::Parse {
            message: "unmatched '('".to_string(),
            line: 0,
        })
    }

    /// Split a string on `|` that are not inside parentheses.
    fn split_on_bar(&self, s: &str) -> Vec<String> {
        let tokens = self.tokenize_expr(s);
        let mut result = Vec::new();
        let mut current = Vec::new();
        let mut depth = 0;

        for t in tokens {
            if t == "|" && depth == 0 {
                if !current.is_empty() {
                    result.push(current.join(" "));
                    current.clear();
                }
            } else {
                if t == "(" {
                    depth += 1;
                } else if t == ")" {
                    depth -= 1;
                }
                current.push(t);
            }
        }
        if !current.is_empty() {
            result.push(current.join(" "));
        }
        if result.is_empty() {
            result.push(s.to_string());
        }
        result
    }

    /// Split expression string into tokens: quoted strings, identifiers, operators, punctuation.
    fn tokenize_expr(&self, s: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let chars: Vec<char> = s.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            let c = chars[i];

            // Skip whitespace
            if c.is_whitespace() {
                i += 1;
                continue;
            }

            // Quoted string
            if c == '"' || c == '\'' {
                let quote = c;
                let mut s = String::new();
                s.push(quote);
                i += 1;
                while i < chars.len() && chars[i] != quote {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        s.push(chars[i]);
                        i += 1;
                        s.push(chars[i]);
                    } else {
                        s.push(chars[i]);
                    }
                    i += 1;
                }
                if i < chars.len() {
                    s.push(chars[i]); // closing quote
                    i += 1;
                }
                tokens.push(s);
                continue;
            }

            // Multi-char operators
            if i + 1 < chars.len() {
                let two: String = [c, chars[i + 1]].iter().collect();
                if matches!(
                    two.as_str(),
                    "<=" | ">=" | "==" | "!=" | "&&" | "||" | "->" | "::"
                ) {
                    tokens.push(two);
                    i += 2;
                    continue;
                }
            }

            // Single-char operators/punctuation
            if matches!(
                c,
                '(' | ')' | '{' | '}' | '[' | ']' | '|' | '*' | '+' | '?' | ','
            ) {
                tokens.push(c.to_string());
                i += 1;
                continue;
            }

            // Identifiers / keywords / numbers
            if c.is_alphanumeric() || c == '_' {
                let mut ident = String::new();
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    ident.push(chars[i]);
                    i += 1;
                }
                tokens.push(ident);
                continue;
            }

            // ε (epsilon)
            if c == 'ε' {
                tokens.push("ε".to_string());
                i += 1;
                continue;
            }

            // Skip unknown characters
            i += 1;
        }

        tokens
    }

    // ── Validation ──

    fn is_left_recursive(&self, rule_name: &str, current: &str, visited: &mut Vec<String>) -> bool {
        if visited.contains(&current.to_string()) {
            return false; // cycle detected, but not necessarily left-recursive
        }
        visited.push(current.to_string());

        if let Some(rule) = self.rules.get(current) {
            if Self::expr_starts_with_rule(&rule.expr, rule_name) {
                return true;
            }
            // Check indirect: collect first rules referenced
            for ref_name in Self::first_rules(&rule.expr) {
                if self.is_left_recursive(rule_name, &ref_name, visited) {
                    return true;
                }
            }
        }

        visited.pop();
        false
    }

    fn expr_starts_with_rule(expr: &Expr, rule_name: &str) -> bool {
        match expr {
            Expr::Seq(items) => {
                // P0-10 修复：穿透可空前缀——`b? a` 中 b? 为空时 a 在最前。
                // 逐项检查：可空项跳过（其内部另查），直到不可空项。
                let mut first_non_nullable: Option<&Expr> = None;
                for item in items {
                    if Self::expr_starts_with_rule(item, rule_name) {
                        return true;
                    }
                    if !Self::nullable(item) {
                        first_non_nullable = Some(item);
                        break;
                    }
                }
                // 全部可空：看最后一项（如 `a? b?` 中 a 可空到末尾）
                let _ = first_non_nullable;
                false
            }
            Expr::Alt(items) => items
                .iter()
                .any(|e| Self::expr_starts_with_rule(e, rule_name)),
            Expr::Rule(name) => name == rule_name,
            // P0-10 修复：Opt/ZeroOrMore/OneOrMore 可空——`a ::= b? a` 中
            // `b?` 为空时 `a` 实际在最前 → 左递归。递归检查内部。
            Expr::Opt(inner) | Expr::ZeroOrMore(inner) | Expr::OneOrMore(inner) => {
                Self::expr_starts_with_rule(inner, rule_name)
            }
            Expr::Token(_) | Expr::Lit(_) | Expr::Epsilon | Expr::NotLookahead(_) => false,
        }
    }

    fn first_rules(expr: &Expr) -> Vec<String> {
        let mut result = Vec::new();
        match expr {
            Expr::Seq(items) => {
                for item in items {
                    result.extend(Self::first_rules(item));
                    // Only continue if this item can be empty
                    if !Self::nullable(item) {
                        break;
                    }
                }
            }
            Expr::Alt(items) => {
                for item in items {
                    result.extend(Self::first_rules(item));
                }
            }
            Expr::Rule(name) => {
                result.push(name.clone());
            }
            // P0-10 修复：Opt/ZeroOrMore/OneOrMore 可空——first 集必须
            // 穿透到内部（`a ::= b? a` 中 b? 为空时 a 在前缀）。否则
            // 可空包装器掩盖左递归（运行时无限递归）。
            Expr::ZeroOrMore(inner) | Expr::OneOrMore(inner) | Expr::Opt(inner) => {
                result.extend(Self::first_rules(inner));
            }
            Expr::Epsilon | Expr::NotLookahead(_) => {}
            Expr::Token(_) | Expr::Lit(_) => {}
        }
        result
    }

    fn nullable(expr: &Expr) -> bool {
        match expr {
            Expr::Epsilon => true,
            Expr::ZeroOrMore(_) => true,
            Expr::Opt(_) => true,
            Expr::NotLookahead(_) => true, // negative lookahead consumes no tokens
            Expr::Seq(items) => items.iter().all(Self::nullable),
            Expr::Alt(items) => items.iter().any(Self::nullable),
            _ => false,
        }
    }

    fn validate_expr(&self, expr: &Expr, _rule_name: &str) -> Result<(), GrammarError> {
        match expr {
            Expr::Seq(items) | Expr::Alt(items) => {
                for item in items {
                    self.validate_expr(item, _rule_name)?;
                }
            }
            Expr::ZeroOrMore(e) | Expr::OneOrMore(e) | Expr::Opt(e) | Expr::NotLookahead(e) => {
                self.validate_expr(e, _rule_name)?;
            }
            Expr::Rule(name) => {
                if !self.rules.contains_key(name) {
                    return Err(GrammarError::UnknownRule(name.clone()));
                }
            }
            Expr::Token(name) => {
                if !self.tokens.iter().any(|t| t.name == *name) {
                    return Err(GrammarError::UnknownToken(name.clone()));
                }
            }
            Expr::Lit(_) | Expr::Epsilon => {}
        }
        Ok(())
    }

    fn sort_tokens(&mut self) {
        // Sort: literal tokens first (longest match first), then regex tokens
        self.tokens.sort_by(|a, b| {
            match (&a.pattern, &b.pattern) {
                (TokenPattern::Literal(la), TokenPattern::Literal(lb)) => {
                    lb.len().cmp(&la.len()) // longer literals first
                }
                (TokenPattern::Literal(_), TokenPattern::Regex(_)) => std::cmp::Ordering::Less,
                (TokenPattern::Regex(_), TokenPattern::Literal(_)) => std::cmp::Ordering::Greater,
                (TokenPattern::Regex(_), TokenPattern::Regex(_)) => std::cmp::Ordering::Equal,
            }
        });
    }
}

// ============================================================
// Utility functions
// ============================================================

/// Remove surrounding quotes from a string.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Parse space-separated quoted strings: `"lit1" "lit2"` → `["lit1", "lit2"]`
fn parse_quoted_strings(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Skip whitespace
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }

        if chars[i] == '"' || chars[i] == '\'' {
            let quote = chars[i];
            i += 1;
            let mut lit = String::new();
            while i < chars.len() && chars[i] != quote {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    lit.push(chars[i]);
                } else {
                    lit.push(chars[i]);
                }
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            } // skip closing quote
            result.push(lit);
        } else {
            // Unquoted token — skip
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
        }
    }

    result
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_grammar() {
        let src = r#"
token IDENT = "[a-z][a-z0-9]*"
token NUM = "[0-9]+"
punct "(" ")" "+"
skip "[ \t]+"

program ::= expr
expr ::= IDENT | NUM | "(" expr ")"
"#;
        let grammar = parse_grammar(src).unwrap();
        assert_eq!(grammar.tokens.len(), 5); // IDENT, NUM, LPAREN, RPAREN, PLUS
        assert_eq!(grammar.skips.len(), 1);
        assert_eq!(grammar.rules.len(), 2);
        assert_eq!(grammar.entry, "program");
    }

    #[test]
    fn test_parse_with_star_plus_opt() {
        let src = r#"
token IDENT = "[a-z]+"
skip "[ \t]+"

program ::= stmt*
stmt ::= IDENT ("," IDENT)* ";"?
"#;
        let grammar = parse_grammar(src).unwrap();
        assert_eq!(grammar.rules.len(), 2);
    }

    #[test]
    fn test_parse_with_alternation() {
        let src = r#"
token A = "a"
token B = "b"
token C = "c"
skip "[ \t]+"

letter ::= A | B | C
"#;
        let grammar = parse_grammar(src).unwrap();
        let rule = grammar.rules.get("letter").unwrap();
        match &rule.expr {
            Expr::Alt(items) => assert_eq!(items.len(), 3),
            _ => panic!("expected Alt"),
        }
    }

    #[test]
    fn test_left_recursion_detected() {
        let src = r#"
token IDENT = "[a-z]+"
skip "[ \t]+"

expr ::= expr "+" IDENT | IDENT
"#;
        let result = parse_grammar(src);
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("left recursion"));
    }

    /// P0-10 负向：可空包装器掩盖的左递归必须被拒绝——
    /// `a ::= b? a`（b? 为空时 a 在前缀）旧检测漏检 → 运行时无限递归。
    #[test]
    fn test_left_recursion_nullable_prefix_detected() {
        let src = r#"
token IDENT = "[a-z]+"
skip "[ \t]+"

a ::= b? a
b ::= IDENT
"#;
        let result = parse_grammar(src);
        assert!(
            result.is_err(),
            "可空前缀 b? 掩盖的左递归必须被检测（P0-10 回归）"
        );
    }

    #[test]
    fn test_duplicate_rule() {
        let src = r#"
token IDENT = "[a-z]+"
skip "[ \t]+"

foo ::= IDENT
foo ::= IDENT
"#;
        let result = parse_grammar(src);
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_token_in_rule() {
        let src = r#"
token IDENT = "[a-z]+"
skip "[ \t]+"

rule ::= UNKNOWN
"#;
        let result = parse_grammar(src);
        assert!(result.is_err());
    }

    #[test]
    fn test_epsilon() {
        let src = r#"
token IDENT = "[a-z]+"
skip "[ \t]+"

opt_item ::= IDENT?
maybe ::= ε | IDENT
"#;
        let grammar = parse_grammar(src).unwrap();
        assert_eq!(grammar.rules.len(), 2);
    }

    #[test]
    fn test_literal_token_name() {
        assert_eq!(literal_token_name("<="), "LE");
        assert_eq!(literal_token_name("=="), "EQEQ");
        assert_eq!(literal_token_name("("), "LPAREN");
        assert_eq!(literal_token_name("+"), "PLUS");
        assert_eq!(literal_token_name("*"), "STAR");
    }

    #[test]
    fn test_grammar_merge_overrides_rules() {
        let src1 = r#"
token ALPHA = "[a-z]+"
skip "[ \t]+"
foo ::= ALPHA
"#;
        let mut g1 = parse_grammar(src1).unwrap();

        let src2 = r##"
token DIGIT = "[0-9]+"
skip "#.*"
foo ::= DIGIT
bar ::= DIGIT
"##;
        let g2 = parse_grammar(src2).unwrap();

        g1.merge(g2).unwrap();

        // Both rules should be present, with foo overridden by g2.
        assert_eq!(g1.rules.len(), 2);
        assert!(g1.rules.contains_key("foo"));
        assert!(g1.rules.contains_key("bar"));

        // foo should now reference DIGIT (from g2), not ALPHA.
        let foo = g1.rules.get("foo").unwrap();
        assert!(
            matches!(&foo.expr, Expr::Token(name) if name == "DIGIT"),
            "Expected foo to reference DIGIT, got {:?}",
            foo.expr
        );

        // bar should reference DIGIT.
        let bar = g1.rules.get("bar").unwrap();
        assert!(matches!(&bar.expr, Expr::Token(name) if name == "DIGIT"));

        // Both tokens should be present — DIGIT was inserted (front), ALPHA retained.
        assert!(g1.tokens.iter().any(|t| t.name == "DIGIT"));
        assert!(g1.tokens.iter().any(|t| t.name == "ALPHA"));
    }

    #[test]
    fn test_extend_rule_priority() {
        let src = r#"
token A = "a"
token B = "b"
token C = "c"
skip "[ \t]+"
my_rule ::= A
"#;
        let mut grammar = parse_grammar(src).unwrap();

        // Extend my_rule with a B alternative — B should take priority.
        grammar.extend_rule("my_rule", Expr::Token("B".to_string()));

        let rule = grammar.rules.get("my_rule").unwrap();
        match &rule.expr {
            Expr::Alt(items) => {
                assert_eq!(
                    items.len(),
                    2,
                    "Expected 2 alternatives, got {}",
                    items.len()
                );
                // First alternative should be B (higher priority because it was prepended).
                assert!(
                    matches!(&items[0], Expr::Token(name) if name == "B"),
                    "Expected first alternative to be Token(B), got {:?}",
                    items[0]
                );
                // Second alternative should be the original A.
                assert!(
                    matches!(&items[1], Expr::Token(name) if name == "A"),
                    "Expected second alternative to be Token(A), got {:?}",
                    items[1]
                );
            }
            other => panic!("Expected Alt expression, got {:?}", other),
        }

        // Extending a non-existing rule should create it.
        grammar.extend_rule("new_rule", Expr::Token("C".to_string()));
        assert!(grammar.rules.contains_key("new_rule"));
        let new_rule = grammar.rules.get("new_rule").unwrap();
        assert!(matches!(&new_rule.expr, Expr::Token(name) if name == "C"));
    }

    #[test]
    fn test_validate_deep_unused_token() {
        let src = r#"
token USED = "[a-z]+"
token UNUSED = "[0-9]+"
skip "[ \t]+"
rule ::= USED
"#;
        let grammar = parse_grammar(src).unwrap();
        let diags = grammar.validate_deep();

        // Should produce a warning about the UNUSED token.
        let has_unused = diags.iter().any(|d| d.message.contains("UNUSED"));
        assert!(
            has_unused,
            "Expected a diagnostic about unused token 'UNUSED', got: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );

        // USED should NOT trigger a warning (it IS used by `rule`).
        let has_used_warning = diags
            .iter()
            .any(|d| d.message.contains("USED") && d.message.contains("unused"));
        assert!(
            !has_used_warning,
            "USED is used, should not get an unused warning"
        );
    }
}
