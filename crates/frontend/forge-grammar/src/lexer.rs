//! Lexer generator and runtime.
//!
//! The lexer takes a [`Grammar`] and produces a tokenizer that can split
//! source text into a stream of [`Token`]s.
//!
//! # Matching algorithm
//!
//! 1. Skip whitespace and comments using the grammar's skip patterns.
//! 2. Try literal tokens first (longest match wins).
//! 3. Try regex token patterns in definition order.
//! 4. If nothing matches, emit an error.

use crate::error::{LexError, Span};
use crate::grammar::{Grammar, TokenPattern};

// ============================================================
// Token
// ============================================================

/// A token produced by the lexer.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// The token kind name (e.g., "IDENT", "REG", "LPAREN").
    pub kind: String,
    /// The raw source text of this token.
    pub text: String,
    /// Source location.
    pub span: Span,
}

impl Token {
    pub fn new(kind: impl Into<String>, text: impl Into<String>, span: Span) -> Self {
        Self {
            kind: kind.into(),
            text: text.into(),
            span,
        }
    }

    /// End-of-file token.
    pub fn eof(pos: usize) -> Self {
        Self {
            kind: "EOF".to_string(),
            text: String::new(),
            span: Span::new(pos, pos, 0, 0),
        }
    }

    pub fn is_eof(&self) -> bool {
        self.kind == "EOF"
    }
}

// ============================================================
// Lexer
// ============================================================

/// Runtime lexer. Tokenizes source text according to grammar rules.
pub struct Lexer {
    /// Compiled token definitions (literals first, then regex patterns).
    tokens: Vec<CompiledTokenMatcher>,
    /// Skip patterns (compiled as regex-like prefix matchers).
    skips: Vec<String>,
    /// The input source.
    input: String,
    /// Current byte position.
    pos: usize,
    /// Current line (0-indexed).
    line: usize,
    /// Current column (0-indexed).
    col: usize,
    /// Cached character representation of input.
    chars: Vec<char>,
}

struct CompiledTokenMatcher {
    name: String,
    kind: MatcherKind,
}

#[derive(Clone)]
enum MatcherKind {
    Literal(String),
    Regex(String),
}

impl Lexer {
    /// Build a lexer from a grammar definition.
    pub fn build(grammar: &Grammar) -> Self {
        let tokens: Vec<CompiledTokenMatcher> = grammar
            .tokens
            .iter()
            .map(|td| CompiledTokenMatcher {
                name: td.name.clone(),
                kind: match &td.pattern {
                    TokenPattern::Literal(s) => MatcherKind::Literal(s.clone()),
                    TokenPattern::Regex(s) => MatcherKind::Regex(s.clone()),
                },
            })
            .collect();

        let skips: Vec<String> = grammar.skips.clone();

        Lexer {
            tokens,
            skips,
            input: String::new(),
            pos: 0,
            line: 0,
            col: 0,
            chars: Vec::new(),
        }
    }

    /// Tokenize source text, returning all tokens (including EOF).
    pub fn tokenize(&mut self, source: &str) -> Result<Vec<Token>, LexError> {
        self.input = source.to_string();
        self.chars = source.chars().collect();
        self.pos = 0;
        self.line = 0;
        self.col = 0;
        // P0-11 修复：Span 从 char 索引转字节偏移（下游 `&src[span.start..span.end]`
        // 按字节切片；char 索引对非 ASCII 会 char-boundary panic）。
        // 每 char 的字节起始位置：chars[i] 的字节偏移 = 前 i 个 char 的字节和。
        let mut char_to_byte = Vec::with_capacity(self.chars.len() + 1);
        let mut acc = 0usize;
        for &c in &self.chars {
            char_to_byte.push(acc);
            acc += c.len_utf8();
        }
        char_to_byte.push(acc); // EOF 位置（源末尾字节偏移）

        let mut tokens = Vec::new();
        loop {
            let start_pos = self.pos;
            let start_line = self.line;
            let start_col = self.col;

            self.skip_whitespace_and_comments();

            if self.pos >= self.chars.len() {
                tokens.push(Token::eof(char_to_byte[self.pos]));
                break;
            }

            match self.next_token() {
                Ok(Some(mut token)) => {
                    // If we skipped over content, adjust span
                    if token.span.start == 0 && token.span.end == 0 {
                        token.span = Span::new(start_pos, self.pos, start_line, start_col);
                    }
                    // char 索引 → 字节偏移
                    let s = token.span.start.min(char_to_byte.len() - 1);
                    let e = token.span.end.min(char_to_byte.len() - 1);
                    token.span.start = char_to_byte[s];
                    token.span.end = char_to_byte[e];
                    tokens.push(token);
                }
                Ok(None) => {
                    // Should not happen after EOF check
                    break;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(tokens)
    }

    fn next_token(&mut self) -> Result<Option<Token>, LexError> {
        if self.pos >= self.chars.len() {
            return Ok(None);
        }

        let start_pos = self.pos;
        let start_line = self.line;
        let start_col = self.col;
        // 零复制：直接 slice 剩余字符（旧实现每次 collect 剩余全部 → O(n²)）
        let remaining = &self.chars[self.pos..];

        // Clone token matcher data to avoid borrow conflict
        let token_data: Vec<(String, MatcherKind)> = self
            .tokens
            .iter()
            .map(|t| (t.name.clone(), t.kind.clone()))
            .collect();

        // Try ALL tokens and pick the LONGEST match.
        // On tie, first-defined wins (literal tokens are sorted first).
        let mut best: Option<(String, usize)> = None; // (name, len)

        for (name, kind) in &token_data {
            match kind {
                MatcherKind::Literal(lit) => {
                    let lit_chars: Vec<char> = lit.chars().collect();
                    if remaining.starts_with(&lit_chars) {
                        let len = lit_chars.len();
                        if best.as_ref().is_none_or(|(_, best_len)| len > *best_len) {
                            best = Some((name.clone(), len));
                        }
                    }
                }
                MatcherKind::Regex(pattern) => {
                    if let Some(len) = match_regex_prefix(pattern, remaining)
                        && best.as_ref().is_none_or(|(_, best_len)| len > *best_len)
                    {
                        best = Some((name.clone(), len));
                    }
                }
            }
        }

        if let Some((name, len)) = best {
            let text: String = self.chars[start_pos..start_pos + len].iter().collect();
            self.advance_chars(len);
            let span = Span::new(start_pos, self.pos, start_line, start_col);
            return Ok(Some(Token::new(name, text, span)));
        }

        Err(LexError {
            message: format!("unexpected character '{}'", self.chars[self.pos]),
            span: Span::new(start_pos, start_pos + 1, start_line, start_col),
        })
    }

    /// Advance the position counters by `n` characters.
    fn advance_chars(&mut self, n: usize) {
        for _ in 0..n {
            if self.pos < self.chars.len() {
                if self.chars[self.pos] == '\n' {
                    self.line += 1;
                    self.col = 0;
                } else {
                    self.col += 1;
                }
                self.pos += 1;
            }
        }
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            if self.pos >= self.chars.len() {
                break;
            }
            let remaining = &self.chars[self.pos..];

            let mut matched = false;
            for skip_pattern in &self.skips {
                if let Some(len) = match_regex_prefix(skip_pattern, remaining) {
                    self.advance_chars(len);
                    matched = true;
                    break;
                }
            }

            if !matched {
                break;
            }
        }
    }
}

// ============================================================
// Simple regex prefix matcher
// ============================================================

/// Try to match a simplified regex pattern at the start of input.
/// Returns the longest match, or None.
///
/// Supported constructs:
/// - Literal characters: `a`, `1`, `_`
/// - Character classes: `[a-z]`, `[a-zA-Z0-9_]`, `[^"]`, `[^\\n]`
/// - Shorthand: `\d` (digit), `\w` (word), `\s` (whitespace), `\n` (newline), `\t` (tab), `\r` (CR)
/// - Quantifiers: `*` (zero or more), `+` (one or more), `?` (zero or one)
/// - Grouping: `(...)`
/// - Alternation: `|`
/// - Escapes: `\.`, `\\`, `\(`, `\)`, `\[`, `\]`, `\+`, `\*`, `\|`
fn match_regex_prefix(pattern: &str, chars: &[char]) -> Option<usize> {
    // match_here 直接从开头匹配并返回匹配长度——O(匹配长度)，
    // 不需要从最长前缀递减尝试（旧实现每次 O(剩余长度) → lexer O(n²)）。
    let matcher = SimpleRegex::compile(pattern);
    let len = matcher.match_here(chars)?;
    if len > 0 { Some(len) } else { None }
}

/// Check if a string matches a simplified regex pattern (full match).
#[allow(dead_code)] // 测试辅助
fn matches_pattern(pattern: &str, chars: &[char]) -> bool {
    let matcher = SimpleRegex::compile(pattern);
    // Must match the ENTIRE input (not just a prefix)
    matcher
        .match_here(chars)
        .is_some_and(|len| len == chars.len())
}

/// A simplified regex engine sufficient for lexer token patterns.
#[derive(Debug, Clone)]
enum SimpleRegex {
    /// Match a sequence of sub-patterns.
    Seq(Vec<SimpleRegex>),
    /// Alternation: A | B
    Alt(Vec<SimpleRegex>),
    /// Zero or more
    Star(Box<SimpleRegex>),
    /// One or more
    Plus(Box<SimpleRegex>),
    /// Optional
    Opt(Box<SimpleRegex>),
    /// Literal character
    Char(char),
    /// Character class: inclusive set
    Class {
        ranges: Vec<(char, char)>,
        negated: bool,
    },
    /// Digit: [0-9]
    Digit,
    /// Word: [a-zA-Z0-9_]
    Word,
    /// Whitespace: [ \t\r\n]
    Space,
    /// Any character except newline
    Any,
    /// Newline
    Newline,
    /// Tab
    Tab,
    /// Carriage return
    CR,
}

impl SimpleRegex {
    fn compile(pattern: &str) -> Self {
        let mut parser = RegexParser::new(pattern);
        let result = parser.parse_alt();
        // 曾静默退化为匹配字面 '?'（InvalidRegex 死变体从未被构造，审计 P3）——
        // 非法 token 正则必须在文法加载期暴露，否则产生静默错匹配
        result.unwrap_or_else(|| panic!("invalid regex pattern `{pattern}` (RegexParser 解析失败)"))
    }

    /// Try to match at the start of input. Returns Some(len) if matched.
    fn match_here(&self, chars: &[char]) -> Option<usize> {
        self._match(chars, 0)
    }

    fn _match(&self, chars: &[char], pos: usize) -> Option<usize> {
        match self {
            SimpleRegex::Seq(items) => {
                let mut p = pos;
                for item in items {
                    let len = item._match(chars, p)?;
                    p += len;
                }
                Some(p - pos)
            }
            SimpleRegex::Alt(items) => {
                for item in items {
                    if let Some(len) = item._match(chars, pos) {
                        return Some(len);
                    }
                }
                None
            }
            SimpleRegex::Star(inner) => {
                let mut p = pos;
                loop {
                    match inner._match(chars, p) {
                        Some(len) if len > 0 => p += len,
                        _ => break,
                    }
                }
                Some(p - pos)
            }
            SimpleRegex::Plus(inner) => {
                let mut p = pos;
                let first = inner._match(chars, p)?;
                if first == 0 {
                    return None;
                }
                p += first;
                loop {
                    match inner._match(chars, p) {
                        Some(len) if len > 0 => p += len,
                        _ => break,
                    }
                }
                Some(p - pos)
            }
            SimpleRegex::Opt(inner) => match inner._match(chars, pos) {
                Some(len) => Some(len),
                None => Some(0),
            },
            SimpleRegex::Char(c) => {
                if pos < chars.len() && chars[pos] == *c {
                    Some(1)
                } else {
                    None
                }
            }
            SimpleRegex::Class { ranges, negated } => {
                if pos >= chars.len() {
                    return None;
                }
                let c = chars[pos];
                let in_class = ranges.iter().any(|(lo, hi)| c >= *lo && c <= *hi);
                if in_class != *negated { Some(1) } else { None }
            }
            SimpleRegex::Digit => {
                if pos < chars.len() && chars[pos].is_ascii_digit() {
                    Some(1)
                } else {
                    None
                }
            }
            SimpleRegex::Word => {
                if pos < chars.len() && (chars[pos].is_ascii_alphanumeric() || chars[pos] == '_') {
                    Some(1)
                } else {
                    None
                }
            }
            SimpleRegex::Space => {
                if pos < chars.len() && matches!(chars[pos], ' ' | '\t' | '\r' | '\n') {
                    Some(1)
                } else {
                    None
                }
            }
            SimpleRegex::Any => {
                if pos < chars.len() && chars[pos] != '\n' {
                    Some(1)
                } else {
                    None
                }
            }
            SimpleRegex::Newline => {
                if pos < chars.len() && chars[pos] == '\n' {
                    Some(1)
                } else {
                    None
                }
            }
            SimpleRegex::Tab => {
                if pos < chars.len() && chars[pos] == '\t' {
                    Some(1)
                } else {
                    None
                }
            }
            SimpleRegex::CR => {
                if pos < chars.len() && chars[pos] == '\r' {
                    Some(1)
                } else {
                    None
                }
            }
        }
    }
}

/// Mini parser for simplified regex patterns.
struct RegexParser {
    chars: Vec<char>,
    pos: usize,
}

impl RegexParser {
    fn new(pattern: &str) -> Self {
        Self {
            chars: pattern.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn parse_alt(&mut self) -> Option<SimpleRegex> {
        let mut items = Vec::new();
        items.push(self.parse_seq()?);
        while self.peek() == Some('|') {
            self.advance();
            items.push(self.parse_seq()?);
        }
        if items.len() == 1 {
            Some(items.pop().unwrap())
        } else {
            Some(SimpleRegex::Alt(items))
        }
    }

    fn parse_seq(&mut self) -> Option<SimpleRegex> {
        let mut items = Vec::new();
        while self.pos < self.chars.len() && self.peek() != Some('|') && self.peek() != Some(')') {
            if let Some(atom) = self.parse_atom() {
                // Check for quantifier
                let quantified = match self.peek() {
                    Some('*') => {
                        self.advance();
                        SimpleRegex::Star(Box::new(atom))
                    }
                    Some('+') => {
                        self.advance();
                        SimpleRegex::Plus(Box::new(atom))
                    }
                    Some('?') => {
                        self.advance();
                        SimpleRegex::Opt(Box::new(atom))
                    }
                    _ => atom,
                };
                items.push(quantified);
            } else {
                break;
            }
        }
        if items.is_empty() {
            None
        } else if items.len() == 1 {
            Some(items.pop().unwrap())
        } else {
            Some(SimpleRegex::Seq(items))
        }
    }

    fn parse_atom(&mut self) -> Option<SimpleRegex> {
        match self.peek() {
            Some('\\') => {
                self.advance();
                match self.advance() {
                    Some('d') => Some(SimpleRegex::Digit),
                    Some('w') => Some(SimpleRegex::Word),
                    Some('s') => Some(SimpleRegex::Space),
                    Some('n') => Some(SimpleRegex::Newline),
                    Some('t') => Some(SimpleRegex::Tab),
                    Some('r') => Some(SimpleRegex::CR),
                    Some('\\') => Some(SimpleRegex::Char('\\')),
                    Some('.') => Some(SimpleRegex::Char('.')),
                    Some('(') => Some(SimpleRegex::Char('(')),
                    Some(')') => Some(SimpleRegex::Char(')')),
                    Some('[') => Some(SimpleRegex::Char('[')),
                    Some(']') => Some(SimpleRegex::Char(']')),
                    Some('+') => Some(SimpleRegex::Char('+')),
                    Some('*') => Some(SimpleRegex::Char('*')),
                    Some('|') => Some(SimpleRegex::Char('|')),
                    Some('?') => Some(SimpleRegex::Char('?')),
                    Some(c) => Some(SimpleRegex::Char(c)),
                    None => None,
                }
            }
            Some('[') => self.parse_class(),
            Some('.') => {
                self.advance();
                Some(SimpleRegex::Any)
            }
            Some('(') => {
                self.advance(); // skip '('
                let inner = self.parse_alt();
                if self.peek() == Some(')') {
                    self.advance();
                }
                inner
            }
            Some(c) if c != ')' && c != '|' && c != '*' && c != '+' && c != '?' => {
                self.advance();
                Some(SimpleRegex::Char(c))
            }
            _ => None,
        }
    }

    fn parse_class(&mut self) -> Option<SimpleRegex> {
        self.advance(); // skip '['
        let negated = self.peek() == Some('^');
        if negated {
            self.advance();
        }

        let mut ranges = Vec::new();
        loop {
            match self.peek() {
                None | Some(']') => break,
                _ => {}
            }

            // Handle escape sequences inside character class
            let lo = if self.peek() == Some('\\') {
                self.advance(); // skip backslash
                match self.advance() {
                    Some('d') => return Some(SimpleRegex::Digit),
                    Some('w') => return Some(SimpleRegex::Word),
                    Some('s') => return Some(SimpleRegex::Space),
                    Some('n') => '\n',
                    Some('t') => '\t',
                    Some('r') => '\r',
                    Some('\\') => '\\',
                    Some(']') => ']',
                    Some('-') => '-',
                    Some('^') => '^',
                    Some(c) => c,
                    None => break,
                }
            } else {
                self.advance()?
            };

            if self.peek() == Some('-')
                && self.pos + 1 < self.chars.len()
                && self.chars[self.pos + 1] != ']'
            {
                self.advance(); // skip '-'
                let hi = if self.peek() == Some('\\') {
                    self.advance();
                    match self.advance() {
                        Some('n') => '\n',
                        Some('t') => '\t',
                        Some('r') => '\r',
                        Some(c) => c,
                        None => break,
                    }
                } else {
                    self.advance()?
                };
                ranges.push((lo, hi));
            } else {
                ranges.push((lo, lo));
            }
        }

        if self.peek() == Some(']') {
            self.advance();
        }

        Some(SimpleRegex::Class { ranges, negated })
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_ident() {
        let pattern = "[a-z][a-z0-9_]*";
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert!(matches_pattern(pattern, &c("mov")));
        assert!(matches_pattern(pattern, &c("rd")));
        assert!(matches_pattern(pattern, &c("is_float_return")));
        assert!(!matches_pattern(pattern, &c("RBP"))); // uppercase first
        assert!(!matches_pattern(pattern, &c("42")));
    }

    #[test]
    fn test_match_reg() {
        let pattern = "[A-Z][A-Z0-9]*";
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert!(matches_pattern(pattern, &c("RBP")));
        assert!(matches_pattern(pattern, &c("XMM0")));
        assert!(matches_pattern(pattern, &c("RAX")));
        assert!(!matches_pattern(pattern, &c("rd")));
    }

    #[test]
    fn test_match_hex() {
        let pattern = "0x[0-9a-fA-F_]+";
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert!(matches_pattern(pattern, &c("0x2A")));
        assert!(matches_pattern(pattern, &c("0xFF_FF")));
        assert!(!matches_pattern(pattern, &c("42")));
    }

    #[test]
    fn test_match_temp() {
        let pattern = "%[a-zA-Z_][a-zA-Z0-9_]*";
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert!(matches_pattern(pattern, &c("%tmp")));
        assert!(matches_pattern(pattern, &c("%quotient")));
        assert!(!matches_pattern(pattern, &c("tmp")));
    }

    #[test]
    fn test_match_label() {
        let pattern = "\\.[a-zA-Z_][a-zA-Z0-9_]*";
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert!(matches_pattern(pattern, &c(".L0")));
        assert!(matches_pattern(pattern, &c(".L_exit")));
        assert!(!matches_pattern(pattern, &c("L0")));
    }

    #[test]
    fn test_match_vreg() {
        let pattern = "VReg\\([0-9]+\\)";
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert!(matches_pattern(pattern, &c("VReg(97)")));
        assert!(matches_pattern(pattern, &c("VReg(0)")));
        assert!(!matches_pattern(pattern, &c("VReg")));
    }

    #[test]
    fn test_regex_compilation() {
        // Test that various patterns compile without error
        let patterns = [
            "[a-zA-Z_][a-zA-Z0-9_]*",
            "[A-Z][A-Z0-9]*",
            "0x[0-9a-fA-F_]+",
            "VReg\\([0-9]+\\)",
            "[0-9]+\\.[0-9]+([eE][+-]?[0-9]+)?",
            "[ \\t\\r]+",
            "#[^\\n]*",
        ];
        for p in &patterns {
            let _re = SimpleRegex::compile(p);
            // Should not panic
        }
    }

    /// P0-11：Unicode 源文本的 Span 必须是字节偏移（下游按字节切片；
    /// char 索引会 char-boundary panic）。
    #[test]
    fn test_unicode_span_byte_offsets() {
        use crate::grammar::parse_grammar;
        let grammar_src = "token IDENT = \"[a-z]+\"\nskip \"[ \\t\\n]+\"\nskip \"#[^\\n]*\"\nstart ::= IDENT*\n";
        let grammar = match parse_grammar(grammar_src) {
            Ok(g) => g,
            Err(e) => panic!("grammar parse failed: {e:?}"),
        };
        // 含多字节字符的源（中文注释在前，IDENT 在后）
        let src = "# 中文注释\nabc # 尾部注释\n";
        let mut lexer = Lexer::build(&grammar);
        let tokens = match lexer.tokenize(src) {
            Ok(t) => t,
            Err(e) => panic!("tokenize failed: {e:?}"),
        };
        let ident = tokens.iter().find(|t| t.kind == "IDENT").expect("IDENT token");
        // 字节切片不应 panic（char 索引会在这里崩）；内容应为 "abc"
        let text = &src[ident.span.start..ident.span.end];
        assert_eq!(text, "abc", "Span 必须是字节偏移（P0-11 回归）");
    }
}
