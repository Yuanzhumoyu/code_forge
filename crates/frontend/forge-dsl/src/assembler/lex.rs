//! 参考词法器：汇编源码文本 → [`Tok`] 序列。
//!
//! 这是 v12 asm 语法的**规范**实现（logos 驱动）。`v12/codegen` 生成器：
//! 1. 在**编译期**用 [`tokenize`] 把 asm 模板的字面段转成 token 列表，用于
//!    生成逐 token 匹配代码（`__eat_lit`）；
//! 2. 在**生成模块内**镜像本词法的 token 集合，发出 std-only 手写 `__lex`
//!    （`v12/codegen::gen_lexer_ts`），两者词法语义必须逐 token 一致。
//!
//! 词法要点：
//! - 空白（含 `\r`）跳过。**注释剥离在 `parse_insts` 行级完成**（注释字符由
//!   `[meta].comment_char` 声明）——词法器不吞 `#`，保证 `#` 可作为立即数
//!   前缀（`imm_prefix`）的 ISA 不受影响。
//! - `-` 独立成 token（负数由操作数解析器吸收 `Minus + Num`；内存位移
//!   `[rax-8]` 共用同一词法）。
//! - 立即数：`0x` 十六进制 / `0b` 二进制 / 十进制 / 浮点（`1.5`、`1e3`）；
//!   字符 `'c'`、字符串 `"s"`。
//! - 标识符允许 `.`/`$`（`fadd.s` 助记符、符号名）。

use logos::{Lexer, Logos};

/// 汇编源码 token（v12 规范；生成器镜像此集合发出 `__Tok`）。
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\n\f\r]+")]
pub enum Tok {
    /// 助记符 / 寄存器名 / 符号 / 标签（`fadd.s` 这类含点的助记符也在此）。
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_.$]*", |l| l.slice().to_string())]
    Ident(String),
    #[regex(r"0[xX][0-9a-fA-F]+", |l| i64::from_str_radix(&l.slice()[2..], 16).ok())]
    Hex(i64),
    #[regex(r"0[bB][01]+", |l| i64::from_str_radix(&l.slice()[2..], 2).ok())]
    Bin(i64),
    #[regex(r"[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?", |l| l.slice().parse().ok())]
    Float(f64),
    #[regex(r"[0-9]+", |l| l.slice().parse().ok())]
    Dec(i64),
    #[regex(r"'([^'\\]|\\.)'", |l| l.slice().chars().nth(1))]
    Char(char),
    #[regex(r#""([^"\\]|\\.)*""#, |l| l.slice()[1..l.slice().len() - 1].to_string())]
    Str(String),
    #[token("<<")]
    Shl,
    #[token(">>")]
    Shr,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token(",")]
    Comma,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("$")]
    Dollar,
    #[token("#")]
    Hash,
    #[token(":")]
    Colon,
    #[token(".")]
    Dot,
    #[token("!")]
    Bang,
    #[token("~")]
    Tilde,
    #[token("&")]
    Ampersand,
    #[token("|")]
    Pipe,
    #[token("^")]
    Caret,
    #[token("?")]
    Question,
    #[token("=")]
    Eq,
    #[token("\\")]
    Backslash,
}

/// 词法化一段汇编文本（含注释/空白剥离）。
pub fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let lexer = Lexer::<Tok>::new(s);
    let mut out = Vec::new();
    for t in lexer {
        out.push(t.map_err(|()| format!("asm lex error near '{s}'"))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idents_numbers_and_punct() {
        let toks = tokenize("mov RAX, 0x10 + 0b101 - 1.5, [rbx-8]").unwrap();
        use Tok::*;
        assert_eq!(
            toks,
            vec![
                Ident("mov".into()),
                Ident("RAX".into()),
                Comma,
                Hex(0x10),
                Plus,
                Bin(5),
                Minus,
                Float(1.5),
                Comma,
                LBracket,
                Ident("rbx".into()),
                Minus,
                Dec(8),
                RBracket,
            ]
        );
    }

    #[test]
    fn hash_is_a_token_not_comment() {
        // 注释剥离在 parse_insts 行级（meta.comment_char）；词法器保留 `#`，
        // 以便 `imm_prefix = "#"` 的 ISA（ARM 风格）可用。
        let toks = tokenize("#5").unwrap();
        assert_eq!(toks, vec![Tok::Hash, Tok::Dec(5)]);
    }

    #[test]
    fn dotted_mnemonic_and_char_str() {
        let toks = tokenize("fadd.s x1, 'a', \"s\"").unwrap();
        use Tok::*;
        assert_eq!(
            toks,
            vec![
                Ident("fadd.s".into()),
                Ident("x1".into()),
                Comma,
                Char('a'),
                Comma,
                Str("s".into()),
            ]
        );
    }
}
