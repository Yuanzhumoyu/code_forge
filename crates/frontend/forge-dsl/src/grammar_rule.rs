//! Mini parser for grammar rule expressions in TOML [lang.rules].
//!
//! Parses EBNF-like rule strings into forge_grammar::Expr nodes.
//! Supports: sequences, alternation (|), repetition (* + ?), grouping, tokens, literals.

use forge_grammar::Expr;

pub struct RuleExprParser {
    tokens: Vec<String>,
    pos: usize,
}

impl RuleExprParser {
    pub fn new(source: &str) -> Self {
        let tokens = tokenize_rule(source);
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(|s| s.as_str())
    }

    fn advance(&mut self) -> Option<String> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    pub fn parse_alt(&mut self) -> Result<Expr, String> {
        let mut alts = Vec::new();
        alts.push(self.parse_seq()?);
        while self.peek() == Some("|") {
            self.advance();
            alts.push(self.parse_seq()?);
        }
        if alts.len() == 1 {
            Ok(alts.pop().unwrap())
        } else {
            Ok(Expr::alt(alts))
        }
    }

    fn parse_seq(&mut self) -> Result<Expr, String> {
        let mut items = Vec::new();
        while self.pos < self.tokens.len() && self.peek() != Some("|") && self.peek() != Some(")") {
            items.push(self.parse_atom()?);
        }
        if items.is_empty() {
            Ok(Expr::Epsilon)
        } else if items.len() == 1 {
            Ok(items.pop().unwrap())
        } else {
            Ok(Expr::seq(items))
        }
    }

    fn parse_atom(&mut self) -> Result<Expr, String> {
        let token = self
            .advance()
            .ok_or_else(|| "unexpected end of rule".to_string())?;

        let mut expr = if token == "(" {
            let inner = self.parse_alt()?;
            if self.peek() == Some(")") {
                self.advance();
            }
            inner
        } else if token.starts_with('"') && token.ends_with('"') {
            Expr::Lit(token[1..token.len() - 1].to_string())
        } else if token.chars().all(|c| c.is_uppercase() || c == '_') && !token.is_empty() {
            Expr::Token(token.clone())
        } else if token == "ε" || token == "epsilon" {
            Expr::Epsilon
        } else {
            Expr::Rule(token.clone())
        };

        // Postfix operators
        while self.pos < self.tokens.len() {
            match self.peek() {
                Some("*") => {
                    self.advance();
                    expr = Expr::ZeroOrMore(Box::new(expr));
                }
                Some("+") => {
                    self.advance();
                    expr = Expr::OneOrMore(Box::new(expr));
                }
                Some("?") => {
                    self.advance();
                    expr = Expr::Opt(Box::new(expr));
                }
                _ => break,
            }
        }

        Ok(expr)
    }
}

/// Tokenize a rule expression string into tokens.
fn tokenize_rule(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }

        if c == '"' {
            let mut s = String::new();
            s.push('"');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                s.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                s.push('"');
                i += 1;
            }
            tokens.push(s);
            continue;
        }

        if matches!(c, '(' | ')' | '|' | '*' | '+' | '?') {
            tokens.push(c.to_string());
            i += 1;
            continue;
        }

        // Identifier
        if c.is_alphanumeric() || c == '_' {
            let mut ident = String::new();
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                ident.push(chars[i]);
                i += 1;
            }
            tokens.push(ident);
            continue;
        }

        i += 1;
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_rule() {
        let mut p = RuleExprParser::new(r#"IDENT ("," IDENT)*"#);
        let expr = p.parse_alt().unwrap();
        // Should be: Seq([Token("IDENT"), ZeroOrMore(Seq([Lit(","), Token("IDENT")]))])
        match &expr {
            Expr::Seq(items) => assert_eq!(items.len(), 2),
            _ => panic!("expected Seq"),
        }
    }

    #[test]
    fn test_alternation() {
        let mut p = RuleExprParser::new("IDENT | REG | DEC");
        let expr = p.parse_alt().unwrap();
        match &expr {
            Expr::Alt(items) => assert_eq!(items.len(), 3),
            _ => panic!("expected Alt"),
        }
    }
}
