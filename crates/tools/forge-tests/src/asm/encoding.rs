//! 语料**内联期望字节**的抽取：上游 LLVM MC 用例把每条的编码写在注释里
//! （`# CHECK-ASM: encoding: [0x03,0xe0,0x40,0x00]`），紧跟在它要校验的那条指令之前。
//!
//! 抽出 `(指令原文, 上游字节)` 配对后，凡是**本汇编器解析得动**的那条，就要求
//! `encode()` 出来的字节与上游**逐字节相等**——这是不依赖任何模拟器、也不依赖
//! 我们自己黄金表的**外部编码对拍**（P1 只有 riscv64 的语料带这种注释；
//! aarch64/x86 的补齐见 `asm/PROVENANCE.md` 的 P2 清单）。
//!
//! 解析不动的行**不算失败**（那是解析档的账，见 `classify`）——这里只保证
//! "能解析的就不能编错"。

use super::corpus::{Suite, comment_body, strip_comment};

/// 一条"上游给了期望字节"的指令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodingCase {
    /// 相对语料根的路径。
    pub file: String,
    /// 1-based 行号（指令那一行）。
    pub line_no: usize,
    /// 指令原文（已剥行内注释）。
    pub text: String,
    /// 上游期望字节。
    pub bytes: Vec<u8>,
}

/// 从注释正文里抠出 `encoding: [0x.., 0x..]`。
fn parse_encoding_comment(body: &str) -> Option<Vec<u8>> {
    let rest = body.split_once("encoding:")?.1;
    let open = rest.find('[')?;
    let close = rest[open..].find(']')? + open;
    let inner = &rest[open + 1..close];
    let mut out = Vec::new();
    for tok in inner.split(',') {
        let tok = tok.trim();
        let hex = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X"))?;
        out.push(u8::from_str_radix(hex, 16).ok()?);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// 抽一份语料文件里的全部 `(指令, 上游字节)` 配对。
///
/// 配对规则：`encoding:` 注释**挂在紧随其后的那条指令行**上（LLVM 的写法）。
/// 注释块里出现多条 `encoding:` 时后者覆盖前者（同一条指令不会有两份期望）。
pub fn extract_cases(suite: &Suite, file: &str, src: &str) -> Vec<EncodingCase> {
    let mut out = Vec::new();
    let mut pending: Option<Vec<u8>> = None;
    for (i, raw) in src.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(body) = comment_body(trimmed, suite.comments) {
            if let Some(b) = parse_encoding_comment(body) {
                pending = Some(b);
            }
            continue;
        }
        let body = strip_comment(trimmed, suite.comments).trim();
        if body.is_empty() {
            continue;
        }
        if let Some(bytes) = pending.take() {
            out.push(EncodingCase {
                file: file.to_string(),
                line_no: i + 1,
                text: body.to_string(),
                bytes,
            });
        }
    }
    out
}
