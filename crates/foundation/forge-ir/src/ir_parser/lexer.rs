//! LLVM IR 词法器（logos）。
//!
//! 覆盖 LLVM IR 的 token 集合：关键字、类型（标量/向量）、`@`/`%` 标识符、
//! 整数/十六进制/浮点/字符串字面量、标点、`;` 行注释。
//!
//! 注意：指令名（add/sub/icmp/load…）不在此处枚举——它们作为 `Ident` 交给
//! 语法层的 `llvm_mapping` 查表（105 个 opcode 全量映射）。裸标识符 `x`
//! 只在向量/数组类型的分隔位置出现（参数/局部名总是 `%`/`@` 前缀），因此
//! `x` 关键字 token 不会与用户标识符冲突。

use logos::Logos;

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip(r"[ \t\r\n]+|;[^\n]*", allow_greedy = true))]
pub enum Token {
    // ── 模块级关键字 ──
    #[token("define")]
    Define,
    #[token("declare")]
    Declare,
    #[token("global")]
    Global,
    #[token("constant")]
    Constant,
    #[token("target")]
    Target,
    #[token("triple")]
    Triple,
    #[token("datalayout")]
    Datalayout,
    #[token("attributes")]
    Attributes,

    // ── 终结符关键字 ──
    #[token("ret")]
    Ret,
    #[token("br")]
    Br,
    #[token("switch")]
    Switch,
    #[token("unreachable")]
    Unreachable,

    // ── 类型关键字 ──
    #[token("void")]
    VoidTy,
    #[token("label")]
    LabelTy,
    #[token("metadata")]
    MetadataTy,
    #[token("ptr")]
    PtrTy,
    #[token("x", priority = 2)]
    X,
    // 指令关键字（LLVM 保留字，priority 高于 Ident）
    #[token("icmp", priority = 2)]
    Icmp,
    #[token("fcmp", priority = 2)]
    Fcmp,
    #[token("call", priority = 2)]
    Call,
    #[token("alloca", priority = 2)]
    Alloca,
    #[token("load", priority = 2)]
    Load,
    #[token("store", priority = 2)]
    Store,
    #[token("getelementptr", priority = 2)]
    GetElementPtr,
    // 转换指令关键字
    #[token("sext", priority = 2)]
    Sext,
    #[token("zext", priority = 2)]
    Zext,
    #[token("trunc", priority = 2)]
    Trunc,
    #[token("bitcast", priority = 2)]
    Bitcast,
    #[token("to", priority = 2)]
    To,
    // 值关键字
    #[token("true", priority = 2)]
    True,
    #[token("false", priority = 2)]
    False,
    #[token("null", priority = 2)]
    Null,
    #[token("undef", priority = 2)]
    Undef,
    #[token("poison", priority = 2)]
    Poison,

    // ── 标量类型 ──
    #[regex(r"i[1-9][0-9]*", |lex| lex.slice()[1..].parse::<u16>().unwrap_or(32))]
    IntTy(u16),
    #[regex(r"f(16|32|64|128)|bfloat|half|float|double|fp128", |lex| match lex.slice() {
        "half" | "bfloat" => 16,
        "float" => 32,
        "double" => 64,
        "fp128" => 128,
        s => s[1..].parse::<u16>().unwrap_or(64),
    })]
    FloatTy(u16),

    // ── 向量类型（整体，元素为标量 iN/fN/ptr）──
    #[regex(r"<[0-9]+ x (i[1-9][0-9]*|f(16|32|64|128)|bfloat|half|float|double|fp128|ptr)>", |lex| {
        let inner = lex.slice().trim_start_matches('<').trim_end_matches('>');
        let (n, elem) = inner.split_once('x').map(|(n, e)| (n.trim(), e.trim())).unwrap_or(("0", "i32"));
        VecElem {
            len: n.parse().unwrap_or(0),
            elem: if let Some(bits) = elem.strip_prefix('i') {
                ElemTy::Int(bits.parse().unwrap_or(32))
            } else if let Some(bits) = elem.strip_prefix('f') {
                ElemTy::Float(bits.parse().unwrap_or(64))
            } else {
                ElemTy::Ptr
            },
        }
    })]
    VecTy(VecElem),

    // ── 标识符 ──
    #[regex(r"@[a-zA-Z0-9_$.]+", |lex| lex.slice().to_string())]
    GlobalId(String),
    #[regex(r"%[a-zA-Z0-9_$.]+", |lex| lex.slice().to_string())]
    LocalId(String),
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_$.]*", |lex| lex.slice().to_string(), priority = 1)]
    Ident(String),

    // ── 字面量 ──
    #[regex(r"-?0x[0-9a-fA-F]+", |lex| {
        let (neg, digits) = lex.slice().strip_prefix('-').map(|r| (true, r)).unwrap_or((false, lex.slice()));
        let v = u64::from_str_radix(digits.trim_start_matches("0x").replace('_', "").as_str(), 16).unwrap_or(0) as i64;
        if neg { -v } else { v }
    })]
    HexLit(i64),
    #[regex(r"-?[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?", |lex| lex.slice().parse::<f64>().unwrap_or(0.0))]
    FloatLit(f64),
    #[regex(r"-?[0-9]+", |lex| lex.slice().parse::<i64>().unwrap_or(0))]
    IntLit(i64),
    #[regex(r#""[^"]*""#, |lex| lex.slice().trim_matches('"').to_string())]
    StrLit(String),

    // ── 标点 ──
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("<")]
    LAngle,
    #[token(">")]
    RAngle,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token("=")]
    Eq,
    #[token("*")]
    Star,
    #[token("!")]
    Bang,
}

/// 向量类型元素（枚举，避免字符串）。
#[derive(Clone, Debug, PartialEq)]
pub enum ElemTy {
    Int(u16),
    Float(u16),
    Ptr,
}

/// 向量类型（长度 + 元素）。
#[derive(Clone, Debug, PartialEq)]
pub struct VecElem {
    pub len: u32,
    pub elem: ElemTy,
}

/// 便捷词法器入口（Logos::lexer 包装，供 lalrpop 解析器消费）。
pub fn lexer(src: &str) -> logos::Lexer<'_, Token> {
    Token::lexer(src)
}

/// 词法错误（lalrpop 需要 Location/Error 类型）。
#[derive(Debug, Clone, PartialEq)]
pub struct LexError;

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unexpected character")
    }
}

impl std::error::Error for LexError {}

impl<'a> Iterator for TokenStream<'a> {
    type Item = Result<(usize, Token, usize), LexError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.lexer.next() {
            Some(Ok(tok)) => {
                let span = self.lexer.span();
                Some(Ok((span.start, tok, span.end)))
            }
            Some(Err(_)) => Some(Err(LexError)),
            None => None,
        }
    }
}

/// 带位置的 token 流（lalrpop 消费：`Result<(usize, Token, usize), LexError>`）。
pub struct TokenStream<'a> {
    lexer: logos::Lexer<'a, Token>,
}

impl<'a> TokenStream<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            lexer: Token::lexer(src),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_all(src: &str) -> Vec<Token> {
        Token::lexer(src).map(|r| r.expect("lex ok")).collect()
    }

    #[test]
    fn lex_keywords() {
        let toks = lex_all("define declare ret br switch unreachable");
        assert_eq!(
            toks,
            vec![
                Token::Define,
                Token::Declare,
                Token::Ret,
                Token::Br,
                Token::Switch,
                Token::Unreachable,
            ]
        );
    }

    #[test]
    fn lex_types() {
        let toks = lex_all("i32 i1 f64 ptr void label <4 x i32> <2 x ptr>");
        assert_eq!(
            toks,
            vec![
                Token::IntTy(32),
                Token::IntTy(1),
                Token::FloatTy(64),
                Token::PtrTy,
                Token::VoidTy,
                Token::LabelTy,
                Token::VecTy(VecElem {
                    len: 4,
                    elem: ElemTy::Int(32)
                }),
                Token::VecTy(VecElem {
                    len: 2,
                    elem: ElemTy::Ptr
                }),
            ]
        );
    }

    #[test]
    fn lex_ids() {
        let toks = lex_all("@add %s %0 @main");
        assert_eq!(
            toks,
            vec![
                Token::GlobalId("@add".to_string()),
                Token::LocalId("%s".to_string()),
                Token::LocalId("%0".to_string()),
                Token::GlobalId("@main".to_string()),
            ]
        );
    }

    #[test]
    fn lex_literals() {
        let toks = lex_all("42 -1 0x2A 1.5e0 -0.25 \"hello\"");
        assert_eq!(
            toks,
            vec![
                Token::IntLit(42),
                Token::IntLit(-1),
                Token::HexLit(42),
                Token::FloatLit(1.5),
                Token::FloatLit(-0.25),
                Token::StrLit("hello".to_string()),
            ]
        );
    }

    #[test]
    fn lex_punctuation_and_comment() {
        // `;` 注释被跳过；分号本身不是 token
        let toks = lex_all("( ) { } [ ] , : = * ! ; this is a comment");
        assert_eq!(
            toks,
            vec![
                Token::LParen,
                Token::RParen,
                Token::LBrace,
                Token::RBrace,
                Token::LBracket,
                Token::RBracket,
                Token::Comma,
                Token::Colon,
                Token::Eq,
                Token::Star,
                Token::Bang,
            ]
        );
    }

    #[test]
    fn lex_instruction_names_as_ident() {
        // 指令名是 Ident（交给 llvm_mapping 查表）
        let toks = lex_all("add sub icmp load store");
        assert_eq!(
            toks,
            vec![
                Token::Ident("add".to_string()),
                Token::Ident("sub".to_string()),
                Token::Icmp,
                Token::Load,
                Token::Store,
            ]
        );
    }

    #[test]
    fn lex_bad_char_errors() {
        let mut lex = Token::lexer("?");
        assert!(lex.next().unwrap().is_err());
    }
}
