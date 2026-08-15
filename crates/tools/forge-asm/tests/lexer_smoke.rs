//! forge-asm 词法基础设施冒烟测试。
//!
//! 完整 Token 枚举由 forge-dsl 生成器按 ISA 输出；这里用一份「生成器风格」的
//! 测试 Token 枚举（共享标点 + mnemonic 变体，与 asm_grammar.rs 的模板相同）
//! 验证泛型 `TokenStream` 与 logos 的边界行为：
//! 点号 mnemonic、`.L`/`@`/`%` 前缀、`#5` vs `#注释`、hex、负号分离、注释。

use forge_asm::TokenStream;
use forge_asm::logos::Logos;

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip(r"[ \t\r]+|//[^\n]*|;[^\n]*|#[^\n\d][^\n]*", allow_greedy = true))]
enum TestToken {
    // ── mnemonic 变体（priority 高于 Ident）──
    #[token("mov", priority = 2)]
    Mov,
    #[token("mov_imm", priority = 2)]
    MovImm,
    #[token("sete", priority = 2)]
    Sete,
    // ── 通用 token ──
    // priority=1：与 mnemonic 变体（priority=2）重叠时由 mnemonic 胜出
    #[regex(r"[A-Za-z_][A-Za-z0-9_.]*", |l| l.slice().to_string(), priority = 1)]
    Ident(String),
    #[regex(r"\.[A-Za-z_][A-Za-z0-9_.]*", |l| l.slice().to_string())]
    DotLabel(String),
    #[regex(r"@[A-Za-z_][A-Za-z0-9_.]*", |l| l.slice().to_string())]
    AtLabel(String),
    #[regex(r"%[A-Za-z_][A-Za-z0-9_.]*", |l| l.slice().to_string())]
    TmpReg(String),
    #[regex(r"0[xX][0-9a-fA-F]+", |l| i64::from_str_radix(&l.slice()[2..], 16).unwrap_or(0))]
    HexLit(i64),
    #[regex(r"[0-9]+", |l| l.slice().parse().unwrap_or(0))]
    IntLit(i64),
    #[regex(r"#[0-9]+", |l| l.slice()[1..].parse().unwrap_or(0), priority = 2)]
    HashInt(i64),
    #[regex(r"[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?", |l| l.slice().parse().unwrap_or(0.0))]
    FloatLit(f64),
    #[token(",")]
    Comma,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("!")]
    Bang,
    #[token(":")]
    Colon,
    #[token("{")]
    ConstOpen,
    #[token("}")]
    ConstClose,
    #[token("\n")]
    Newline,
}

fn lex_all(src: &str) -> Vec<TestToken> {
    TokenStream::<TestToken>::new(src)
        .map(|r| r.expect("no lex error"))
        .map(|(_, tok, _)| tok)
        .collect()
}

#[test]
fn mnemonic_token_beats_ident() {
    // "mov" 是 mnemonic 变体；"jmp" 无变体 → Ident
    assert_eq!(
        lex_all("mov jmp"),
        vec![TestToken::Mov, TestToken::Ident("jmp".into())]
    );
}

#[test]
fn longest_match_prefix_mnemonic() {
    // "mov_imm" 最长匹配优先于 "mov" 前缀
    assert_eq!(lex_all("mov_imm"), vec![TestToken::MovImm]);
}

#[test]
fn hex_lit() {
    assert_eq!(
        lex_all("mov_imm R0, 0xFF"),
        vec![
            TestToken::MovImm,
            TestToken::Ident("R0".into()),
            TestToken::Comma,
            TestToken::HexLit(255),
        ]
    );
}

#[test]
fn dot_ident_and_dot_label() {
    // 点号 mnemonic（fadd.s）是 Ident；.L 前缀是 DotLabel
    assert_eq!(
        lex_all("fadd.s R0, .Lloop"),
        vec![
            TestToken::Ident("fadd.s".into()),
            TestToken::Ident("R0".into()),
            TestToken::Comma,
            TestToken::DotLabel(".Lloop".into()),
        ]
    );
}

#[test]
fn at_label_and_tmp_reg() {
    assert_eq!(
        lex_all("@pop_callee mov %t, rs2"),
        vec![
            TestToken::AtLabel("@pop_callee".into()),
            TestToken::Mov,
            TestToken::TmpReg("%t".into()),
            TestToken::Comma,
            TestToken::Ident("rs2".into()),
        ]
    );
}

#[test]
fn hash_int_vs_hash_comment() {
    // #5 → HashInt；# comment（# 后非数字）→ 注释跳过；注释行的换行仍是 Newline
    assert_eq!(
        lex_all("brk #0 ; tail comment\n# full line comment\nret"),
        vec![
            TestToken::Ident("brk".into()),
            TestToken::HashInt(0),
            TestToken::Newline,
            TestToken::Newline,
            TestToken::Ident("ret".into()),
        ]
    );
}

#[test]
fn minus_is_separate_punct() {
    // 负数由语法层组合（Minus + IntLit）
    assert_eq!(
        lex_all("sub R0, -1"),
        vec![
            TestToken::Ident("sub".into()),
            TestToken::Ident("R0".into()),
            TestToken::Comma,
            TestToken::Minus,
            TestToken::IntLit(1),
        ]
    );
}

#[test]
fn float_lit() {
    assert_eq!(
        lex_all("mov R0, 3.14"),
        vec![
            TestToken::Mov,
            TestToken::Ident("R0".into()),
            TestToken::Comma,
            #[allow(clippy::approx_constant)]
            TestToken::FloatLit(3.14),
        ]
    );
}

#[test]
fn brackets_bang_const() {
    assert_eq!(
        lex_all("ldr R0, [R1]! {const 42}"),
        vec![
            TestToken::Ident("ldr".into()),
            TestToken::Ident("R0".into()),
            TestToken::Comma,
            TestToken::LBracket,
            TestToken::Ident("R1".into()),
            TestToken::RBracket,
            TestToken::Bang,
            TestToken::ConstOpen,
            TestToken::Ident("const".into()),
            TestToken::IntLit(42),
            TestToken::ConstClose,
        ]
    );
}

#[test]
fn token_stream_spans() {
    let src = "mov RAX, 5";
    let spans: Vec<(usize, usize)> = TokenStream::<TestToken>::new(src)
        .map(|r| r.unwrap())
        .map(|(s, _, e)| (s, e))
        .collect();
    assert_eq!(spans, vec![(0, 3), (4, 7), (7, 8), (9, 10)]);
}

// ── 中间表示类型 ──

#[test]
fn operand_value_type_signature() {
    use forge_asm::{OperandTy, OperandValue};
    assert_eq!(OperandValue::Ident("rd".into()).ty(), OperandTy::Reg);
    assert_eq!(OperandValue::Imm(5).ty(), OperandTy::Imm);
    assert_eq!(OperandValue::Float(1.5).ty(), OperandTy::Float);
    assert_eq!(OperandValue::Label(".L1".into()).ty(), OperandTy::Label);
    assert_eq!(OperandValue::Const(7).ty(), OperandTy::Imm);
    assert_eq!(OperandValue::TmpReg("%t".into()).ty(), OperandTy::Reg);
    assert_eq!(OperandValue::VReg(3).ty(), OperandTy::Reg);
}

#[test]
fn operand_ty_display() {
    use forge_asm::OperandTy;
    assert_eq!(OperandTy::Reg.to_string(), "register");
    assert_eq!(OperandTy::MemRef.to_string(), "memory reference");
}
