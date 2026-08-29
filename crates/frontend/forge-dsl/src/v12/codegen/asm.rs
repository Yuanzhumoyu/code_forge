//! disassemble / assemble（模板驱动）与通用汇编模板段模型。
//!
//! 从 `codegen/mod.rs` 拆分（原 822-1945 行）。模板解析、助记符分派、
//! 操作数渲染/解析集中于此；`InstInfo` 来自父模块，token 模型来自
//! `crate::assembler`。

use super::super::model::*;
use super::InstInfo;
use crate::assembler::{Tok, tokenize};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::BTreeMap;

// ─────────────────────────────── disassemble ───────────────────────────────

/// disassemble 入口：`pub(crate)` 由 `codegen::generate()` 调用。
pub(crate) fn gen_disassemble(infos: &[InstInfo]) -> Result<TokenStream, String> {
    let mut arms = Vec::new();
    for info in infos {
        let vn = &info.vn;
        let mut fmt = String::new();
        let mut locals: Vec<TokenStream> = Vec::new();
        // 完整格式 = mnemonic + 操作数模板（段序列渲染）
        fmt.push_str(&info.mnemonic);
        let ops = ops_template(info);
        if !ops.is_empty() {
            fmt.push(' ');
            let segs = parse_template(&ops)?;
            validate_segs(&segs, info)?;
            for seg in segs {
                match seg {
                    Seg::Lit(l) => fmt.push_str(&l),
                    Seg::Op(n) => {
                        let (_, fid, slot, _) = &info.operands[n];
                        let local = format_ident!("__o{n}");
                        let expr = render_expr(slot, fid);
                        locals.push(quote! { let #local = #expr; });
                        fmt.push_str(&format!("{{__o{n}}}"));
                    }
                }
            }
        }
        let fmt_lit = syn::LitStr::new(&fmt, proc_macro2::Span::call_site());
        let pat = if info.operands.is_empty() {
            quote! { Inst::#vn }
        } else {
            let ids: Vec<_> = info
                .operands
                .iter()
                .map(|(_, fid, _, _)| fid.clone())
                .collect();
            quote! { Inst::#vn { #(#ids),* } }
        };
        arms.push(quote! { #pat => { #(#locals)* format!(#fmt_lit) } });
    }
    Ok(quote! {
        /// 反汇编为汇编文本。
        pub fn disassemble(inst: &Inst) -> String {
            match inst {
                #(#arms,)*
                Inst::Raw(bytes) => format!(
                    ".byte {}",
                    bytes.iter().map(|b| format!("0x{b:02x}")).collect::<Vec<_>>().join(", ")
                ),
            }
        }
    })
}

/// 操作数渲染表达式（disassemble 用）。
fn render_expr(slot: &OperandSlot, fid: &syn::Ident) -> TokenStream {
    match slot.kind {
        OperandKind::Reg => {
            quote! { <&str as From<Reg>>::from(*#fid) }
        }
        OperandKind::Mem => quote! { __render_mem(#fid) },
        OperandKind::Cond => quote! { __render_cond(*#fid as u8) },
        _ => quote! { #fid.to_string() },
    }
}

// ─────────────────────── 通用汇编模板段模型 ───────────────────────

/// 操作数模板的段：字面片段或操作数占位符（`{n}`）。
/// `pub(crate)`：`collect_inst_infos`（mod.rs）复用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Seg {
    /// 字面文本（分隔符/方括号/括号/关键字等，原样匹配与输出）。
    Lit(String),
    /// 操作数占位符 `{n}`（索引 = 指令操作数位置）。
    Op(usize),
}

/// 解析操作数模板为段序列（字面与占位符交替，任意字面格式）。
///
/// 替换迭代 3b 的封闭 TokenShape（Simple/Bracket/Mem/Mem0）：`[{n}]`、
/// `{off}({base})`、`byte ptr [{n}]` 等都是字面段的自然组合，用户模板自由。
/// 模板解析产物：段序列 + 操作数声明表（序号, 槽名, 角色名）。
type ParsedTemplate = (Vec<Seg>, Vec<(usize, String, Option<String>)>);

fn parse_template(tpl: &str) -> Result<Vec<Seg>, String> {
    Ok(parse_template_full(tpl)?.0)
}

/// 解析操作数模板为段序列 + 操作数声明表。
///
/// 占位符语法：`{n}`（兼容旧式）或 `{n:[槽]}` 或 `{n:[槽:角色]}`（v12.1：
/// 操作数声明内联在 asm 中；角色缺省 "in"）。返回 (段序列, 声明表
/// (序号, 槽名, 角色名))——序号可能乱序/跳号，由调用方校验连续性。
/// `pub(crate)`：`collect_inst_infos`（mod.rs）复用。
pub(crate) fn parse_template_full(tpl: &str) -> Result<ParsedTemplate, String> {
    if tpl.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut segs = Vec::new();
    let mut decls: Vec<(usize, String, Option<String>)> = Vec::new();
    let mut lit = String::new();
    let mut chars = tpl.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut inner = String::new();
            while let Some(&d) = chars.peek() {
                if d == '}' {
                    break;
                }
                inner.push(d);
                chars.next();
            }
            if chars.next() != Some('}') {
                return Err(format!("unterminated '{{' in asm template '{tpl}'"));
            }
            // inner = "n" | "n:[slot]" | "n:[slot:role]"
            let mut parts = inner.split(':');
            let n: usize = parts
                .next()
                .and_then(|p| p.trim().parse().ok())
                .ok_or_else(|| format!("bad placeholder '{{{inner}}}' in asm template '{tpl}'"))?;
            let slot = parts.next().map(|s| {
                s.trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_string()
            });
            let role = parts.next().map(|r| {
                r.trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_string()
            });
            if parts.next().is_some() {
                return Err(format!(
                    "bad placeholder '{{{inner}}}' (expected {{n:[slot:role]}}) in asm template '{tpl}'"
                ));
            }
            if !lit.is_empty() {
                segs.push(Seg::Lit(std::mem::take(&mut lit)));
            }
            segs.push(Seg::Op(n));
            if let Some(s) = slot {
                decls.push((n, s, role));
            }
        } else {
            lit.push(c);
        }
    }
    if !lit.is_empty() {
        segs.push(Seg::Lit(lit));
    }
    if segs.is_empty() {
        return Err(format!("empty asm template '{tpl}'"));
    }
    Ok((segs, decls))
}

/// 校验段序列：占位符索引越界、连续占位符无字面分隔。
fn validate_segs(segs: &[Seg], info: &InstInfo) -> Result<(), String> {
    let ctx = || format!("[[instructions.{}]] asm", info.inst.name);
    let mut prev_op = false;
    for seg in segs {
        match seg {
            Seg::Lit(l) => {
                if l.is_empty() {
                    return Err(format!("{}: empty literal segment", ctx()));
                }
                prev_op = false;
            }
            Seg::Op(n) => {
                if *n >= info.operands.len() {
                    return Err(format!("{}: placeholder {{{n}}} out of range", ctx()));
                }
                if prev_op {
                    return Err(format!(
                        "{}: adjacent placeholders need a literal separator (ambiguity)",
                        ctx()
                    ));
                }
                prev_op = true;
            }
        }
    }
    Ok(())
}

/// 完整汇编格式（含 mnemonic）：`asm` 字段必填（v12.1 起）。
fn asm_full(info: &InstInfo) -> String {
    info.inst.asm.clone()
}

/// 操作数模板（完整格式去掉 mnemonic 前缀）。
fn ops_template(info: &InstInfo) -> String {
    let full = asm_full(info);
    match full.split_once(char::is_whitespace) {
        Some((_, r)) => r.trim().to_string(),
        None => String::new(),
    }
}

// ─────────────────────────────── assemble ───────────────────────────────
//
// 汇编器重写（换血式）：**token 化解析 + 类型签名自动分发**。
// - 生成模块内发出 std-only 手写 tokenizer（`__Tok`/`__Iter`/`__lex`），
//   镜像 `assembler::lex` 的 token 集合（词法语义必须逐 token 一致）。
// - 模板字面段在**编译期**用 `assembler::tokenize` 转成 token 列表，
//   生成逐 token 匹配（`__eat_lit`）——空白天然免疫。
// - 同助记符多 form（不同宽度/类型版本）按**类型签名特异性**排序：
//   槽约束集越窄越先；解析按实际操作数类型过滤（`__reg_f`），
//   opsize 语义的 form 再做跨操作数宽度一致性检查——后端按类型约束
//   自动分发，无需手写 `mov32`/`mov64` 等拆分助记符。

/// 参考 token → 生成模块 `__Tok` 表达式（编译期字面段 token 化用）。
fn tok_expr(t: &Tok) -> TokenStream {
    use Tok::*;
    match t {
        Ident(s) => quote! { __Tok::Ident(#s.into()) },
        Hex(v) | Bin(v) | Dec(v) => quote! { __Tok::Num(#v) },
        Float(f) => {
            let bits = f.to_bits();
            quote! { __Tok::Float(f64::from_bits(#bits)) }
        }
        Char(c) => quote! { __Tok::Char(#c) },
        Str(s) => quote! { __Tok::Str(#s.into()) },
        Shl => quote! { __Tok::Shl },
        Shr => quote! { __Tok::Shr },
        LBracket => quote! { __Tok::LBracket },
        RBracket => quote! { __Tok::RBracket },
        LParen => quote! { __Tok::LParen },
        RParen => quote! { __Tok::RParen },
        Comma => quote! { __Tok::Comma },
        Plus => quote! { __Tok::Plus },
        Minus => quote! { __Tok::Minus },
        Star => quote! { __Tok::Star },
        Slash => quote! { __Tok::Slash },
        Percent => quote! { __Tok::Percent },
        Dollar => quote! { __Tok::Dollar },
        Hash => quote! { __Tok::Hash },
        Colon => quote! { __Tok::Colon },
        Dot => quote! { __Tok::Dot },
        Bang => quote! { __Tok::Bang },
        Tilde => quote! { __Tok::Tilde },
        Ampersand => quote! { __Tok::Ampersand },
        Pipe => quote! { __Tok::Pipe },
        Caret => quote! { __Tok::Caret },
        Question => quote! { __Tok::Question },
        Eq => quote! { __Tok::Eq },
        Backslash => quote! { __Tok::Backslash },
    }
}

/// 生成 std-only 手写 tokenizer（镜像 `assembler::lex`，token 集合一致）。
fn gen_lexer_ts(_model: &V12Model) -> Result<TokenStream, String> {
    Ok(quote! {
        #[allow(dead_code)]
        #[derive(Debug, Clone, PartialEq)]
        pub(crate) enum __Tok {
            Ident(String), Num(i64), Float(f64), Char(char), Str(String),
            LBracket, RBracket, LParen, RParen, Comma, Plus, Minus, Star, Slash,
            Percent, Dollar, Hash, Colon, Dot, Bang, Tilde, Ampersand, Pipe,
            Caret, Question, Eq, Backslash, Shl, Shr,
        }

        #[allow(dead_code)]
        struct __Iter<'a> { toks: &'a [__Tok], pos: usize }
        impl<'a> __Iter<'a> {
            fn peek(&self) -> Option<&__Tok> { self.toks.get(self.pos) }
            fn eof(&self) -> bool { self.pos >= self.toks.len() }
            fn eat(&mut self, t: &__Tok) -> bool {
                if self.toks.get(self.pos) == Some(t) { self.pos += 1; true } else { false }
            }
            fn eat_ident(&mut self, s: &str) -> bool {
                if matches!(self.toks.get(self.pos), Some(__Tok::Ident(x)) if x == s) {
                    self.pos += 1; true
                } else { false }
            }
            fn reset(&mut self) { self.pos = 0; }
        }

        /// `.equ` 符号常量表（parse_insts 在汇编前填充；指令立即数求值读取）。
        #[allow(dead_code)]
        thread_local! {
            static __EQU: std::cell::RefCell<std::collections::HashMap<String, i64>> =
                std::cell::RefCell::new(std::collections::HashMap::new());
        }

        #[allow(dead_code)]
        fn __lex(s: &str) -> Result<Vec<__Tok>, String> {
            let mut out: Vec<__Tok> = Vec::new();
            let mut cs = s.chars().peekable();
            while let Some(&c) = cs.peek() {
                match c {
                    ' ' | '\t' | '\r' | '\n' | '\u{0c}' => { cs.next(); }
                    'a'..='z' | 'A'..='Z' | '_' => {
                        let mut id = String::new();
                        while let Some(&c) = cs.peek() {
                            if c.is_alphanumeric() || c == '_' || c == '.' || c == '$' {
                                id.push(c); cs.next();
                            } else { break; }
                        }
                        out.push(__Tok::Ident(id));
                    }
                    '0'..='9' => {
                        let first = c;
                        let mut num = String::new();
                        num.push(first);
                        cs.next();
                        if first == '0' && matches!(cs.peek(), Some('x') | Some('X')) {
                            cs.next();
                            let mut h = String::new();
                            while let Some(&d) = cs.peek() {
                                if d.is_ascii_hexdigit() { h.push(d); cs.next(); } else { break; }
                            }
                            if h.is_empty() { return Err("bad hex immediate".into()); }
                            out.push(__Tok::Num(
                                i64::from_str_radix(&h, 16).map_err(|_| "bad hex immediate".to_string())?,
                            ));
                        } else if first == '0' && matches!(cs.peek(), Some('b') | Some('B')) {
                            cs.next();
                            let mut b = String::new();
                            while let Some(&d) = cs.peek() {
                                if d == '0' || d == '1' { b.push(d); cs.next(); } else { break; }
                            }
                            if b.is_empty() { return Err("bad binary immediate".into()); }
                            out.push(__Tok::Num(
                                i64::from_str_radix(&b, 2).map_err(|_| "bad binary immediate".to_string())?,
                            ));
                        } else {
                            while let Some(&d) = cs.peek() {
                                if d.is_ascii_digit() { num.push(d); cs.next(); } else { break; }
                            }
                            let mut is_float = false;
                            if cs.peek() == Some(&'.') {
                                let mut t = cs.clone();
                                t.next();
                                if matches!(t.peek(), Some(d) if d.is_ascii_digit()) { is_float = true; }
                            }
                            if is_float {
                                num.push(cs.next().unwrap());
                                while let Some(&d) = cs.peek() {
                                    if d.is_ascii_digit() { num.push(d); cs.next(); } else { break; }
                                }
                                if matches!(cs.peek(), Some('e') | Some('E')) {
                                    num.push(cs.next().unwrap());
                                    if matches!(cs.peek(), Some('+') | Some('-')) { num.push(cs.next().unwrap()); }
                                    while let Some(&d) = cs.peek() {
                                        if d.is_ascii_digit() { num.push(d); cs.next(); } else { break; }
                                    }
                                }
                                out.push(__Tok::Float(num.parse().map_err(|_| format!("bad float '{num}'"))?));
                            } else {
                                out.push(__Tok::Num(num.parse().map_err(|_| format!("bad number '{num}'"))?));
                            }
                        }
                    }
                    '\'' => {
                        cs.next();
                        let ch = cs.next().ok_or("unterminated char literal")?;
                        if cs.next() != Some('\'') { return Err("unterminated char literal".into()); }
                        out.push(__Tok::Char(ch));
                    }
                    '"' => {
                        cs.next();
                        let mut st = String::new();
                        loop {
                            match cs.next() {
                                Some('"') => break,
                                Some('\\') => match cs.next() {
                                    Some('n') => st.push('\n'),
                                    Some('t') => st.push('\t'),
                                    Some('r') => st.push('\r'),
                                    Some('"') => st.push('"'),
                                    Some('\\') => st.push('\\'),
                                    Some('0') => st.push('\0'),
                                    Some(other) => {
                                        st.push('\\');
                                        st.push(other);
                                    }
                                    None => return Err("bad escape in string".into()),
                                },
                                Some(ch) => st.push(ch),
                                None => return Err("unterminated string literal".into()),
                            }
                        }
                        out.push(__Tok::Str(st));
                    }
                    '[' => { cs.next(); out.push(__Tok::LBracket); }
                    ']' => { cs.next(); out.push(__Tok::RBracket); }
                    '(' => { cs.next(); out.push(__Tok::LParen); }
                    ')' => { cs.next(); out.push(__Tok::RParen); }
                    ',' => { cs.next(); out.push(__Tok::Comma); }
                    '+' => { cs.next(); out.push(__Tok::Plus); }
                    '-' => { cs.next(); out.push(__Tok::Minus); }
                    '*' => { cs.next(); out.push(__Tok::Star); }
                    '/' => { cs.next(); out.push(__Tok::Slash); }
                    '%' => { cs.next(); out.push(__Tok::Percent); }
                    '$' => { cs.next(); out.push(__Tok::Dollar); }
                    '#' => { cs.next(); out.push(__Tok::Hash); }
                    ':' => { cs.next(); out.push(__Tok::Colon); }
                    '.' => { cs.next(); out.push(__Tok::Dot); }
                    '!' => { cs.next(); out.push(__Tok::Bang); }
                    '~' => { cs.next(); out.push(__Tok::Tilde); }
                    '&' => { cs.next(); out.push(__Tok::Ampersand); }
                    '|' => { cs.next(); out.push(__Tok::Pipe); }
                    '^' => { cs.next(); out.push(__Tok::Caret); }
                    '?' => { cs.next(); out.push(__Tok::Question); }
                    '=' => { cs.next(); out.push(__Tok::Eq); }
                    '\\' => { cs.next(); out.push(__Tok::Backslash); }
                    '<' => {
                        cs.next();
                        if cs.next_if_eq(&'<').is_some() { out.push(__Tok::Shl); }
                        else { return Err("expected '<<'".into()); }
                    }
                    '>' => {
                        cs.next();
                        if cs.next_if_eq(&'>').is_some() { out.push(__Tok::Shr); }
                        else { return Err("expected '>>'".into()); }
                    }
                    other => return Err(format!("unexpected char '{other}'")),
                }
            }
            Ok(out)
        }
    })
}

/// x86 缺省条件码表（`[conventions.cond]` 未声明时使用；含别名）。
fn cond_default() -> Vec<(&'static str, u64)> {
    vec![
        ("o", 0),
        ("no", 1),
        ("b", 2),
        ("c", 2),
        ("nae", 2),
        ("ae", 3),
        ("nb", 3),
        ("nc", 3),
        ("e", 4),
        ("z", 4),
        ("ne", 5),
        ("nz", 5),
        ("be", 6),
        ("na", 6),
        ("a", 7),
        ("nbe", 7),
        ("s", 8),
        ("ns", 9),
        ("p", 10),
        ("pe", 10),
        ("np", 11),
        ("po", 11),
        ("l", 12),
        ("nge", 12),
        ("ge", 13),
        ("nl", 13),
        ("le", 14),
        ("ng", 14),
        ("g", 15),
        ("nle", 15),
    ]
}

/// 生成共享解析原语（按需：仅当 ISA 使用对应槽类别时发出）。
fn gen_asm_primitives(model: &V12Model, infos: &[InstInfo]) -> Result<TokenStream, String> {
    let has_reg = infos.iter().any(|i| {
        i.operands
            .iter()
            .any(|(_, _, s, _)| s.kind == OperandKind::Reg)
    });
    let has_mem = infos.iter().any(|i| {
        i.operands
            .iter()
            .any(|(_, _, s, _)| s.kind == OperandKind::Mem)
    });
    let has_imm = infos.iter().any(|i| {
        i.operands
            .iter()
            .any(|(_, _, s, _)| s.kind == OperandKind::Imm)
    });
    let has_label = infos.iter().any(|i| {
        i.operands
            .iter()
            .any(|(_, _, s, _)| s.kind == OperandKind::Label)
    });
    let has_cond = infos.iter().any(|i| {
        i.operands
            .iter()
            .any(|(_, _, s, _)| s.kind == OperandKind::Cond)
    });

    let mut out = TokenStream::new();
    if has_reg || has_mem {
        out.extend(quote! {
            fn __reg_cls(it: &mut __Iter) -> Option<(Reg, forge_ir::RegClass)> {
                let __Tok::Ident(name) = it.toks.get(it.pos)?.clone() else { return None; };
                let r = <Reg as FromStr>::from_str(&name).ok()?;
                it.pos += 1;
                Some((r, <Reg as forge_ir::PhysReg>::class(r)))
            }
            /// 约束集 + 固定宽度过滤（wreq 位；None = 不约束）。失败回滚（不消费）。
            fn __reg_f(
                it: &mut __Iter,
                classes: &[forge_ir::RegClass],
                wreq: Option<u16>,
            ) -> Option<(Reg, forge_ir::RegClass)> {
                let save = it.pos;
                let (r, cls) = __reg_cls(it)?;
                if !classes.is_empty() && !classes.contains(&cls) {
                    it.pos = save;
                    return None;
                }
                if let Some(w) = wreq {
                    if <Reg as forge_ir::PhysReg>::width(r) != w {
                        it.pos = save;
                        return None;
                    }
                }
                Some((r, cls))
            }
        });
    }
    if has_imm || has_label {
        let dollar = model.meta.imm_prefix.as_deref() == Some("$");
        out.extend(quote! {
            /// 立即数/表达式求值：数字、负号、括号、算术（+ - * / % << >> & | ^ ~）、
            /// 符号常量（.equ）。失败回滚 token 位置。
            fn __imm(it: &mut __Iter, min: i64, max: i64, float: bool) -> Option<i64> {
                let save = it.pos;
                let _d = if #dollar { it.eat(&__Tok::Dollar) } else { false };
                let v = __expr(it, float)?;
                if v < min || v > max { it.pos = save; return None; }
                Some(v)
            }

            /// 递归下降表达式求值（优先级爬升）。
            fn __expr(it: &mut __Iter, float: bool) -> Option<i64> {
                let save = it.pos;
                let mut lhs = __term(it, float)?;
                loop {
                    match it.toks.get(it.pos) {
                        Some(__Tok::Plus) => {
                            it.pos += 1;
                            let rhs = __term(it, float)?;
                            lhs = lhs.checked_add(rhs)?;
                        }
                        Some(__Tok::Minus) => {
                            it.pos += 1;
                            let rhs = __term(it, float)?;
                            lhs = lhs.checked_sub(rhs)?;
                        }
                        Some(__Tok::Pipe) => {
                            it.pos += 1;
                            let rhs = __term(it, float)?;
                            lhs |= rhs;
                        }
                        _ => break,
                    }
                }
                let _ = save;
                Some(lhs)
            }

            /// 乘除/移位/位与（+/- 之下、一元之上）。
            fn __term(it: &mut __Iter, float: bool) -> Option<i64> {
                let mut lhs = __unary(it, float)?;
                loop {
                    match it.toks.get(it.pos) {
                        Some(__Tok::Star) => {
                            it.pos += 1;
                            let rhs = __unary(it, float)?;
                            lhs = lhs.checked_mul(rhs)?;
                        }
                        Some(__Tok::Slash) => {
                            it.pos += 1;
                            let rhs = __unary(it, float)?;
                            if rhs == 0 { return None; }
                            lhs = lhs.checked_div(rhs)?;
                        }
                        Some(__Tok::Percent) => {
                            it.pos += 1;
                            let rhs = __unary(it, float)?;
                            if rhs == 0 { return None; }
                            lhs = lhs.checked_rem(rhs)?;
                        }
                        Some(__Tok::Shl) => {
                            it.pos += 1;
                            let rhs = __unary(it, float)?;
                            if !(0..64).contains(&rhs) { return None; }
                            lhs = lhs.checked_shl(rhs as u32)?;
                        }
                        Some(__Tok::Shr) => {
                            it.pos += 1;
                            let rhs = __unary(it, float)?;
                            if !(0..64).contains(&rhs) { return None; }
                            lhs = lhs.checked_shr(rhs as u32)?;
                        }
                        Some(__Tok::Ampersand) => {
                            it.pos += 1;
                            let rhs = __unary(it, float)?;
                            lhs &= rhs;
                        }
                        Some(__Tok::Caret) => {
                            it.pos += 1;
                            let rhs = __unary(it, float)?;
                            lhs ^= rhs;
                        }
                        _ => break,
                    }
                }
                Some(lhs)
            }

            /// 一元：负号 / 按位非 / 主元（数字、括号、符号常量）。
            fn __unary(it: &mut __Iter, float: bool) -> Option<i64> {
                let save = it.pos;
                match it.toks.get(it.pos)? {
                    __Tok::Minus => {
                        it.pos += 1;
                        let v = __unary(it, float)?;
                        v.checked_neg()
                    }
                    __Tok::Tilde => {
                        it.pos += 1;
                        let v = __unary(it, float)?;
                        Some(!v)
                    }
                    _ => __primary(it, save, float),
                }
            }

            /// 主元：数字 / 浮点位模式 / 括号表达式 / 符号常量（.equ）。
            /// 未定义符号 → None（由调用方决定回滚或报错）。
            fn __primary(it: &mut __Iter, save: usize, float: bool) -> Option<i64> {
                match it.toks.get(it.pos)? {
                    __Tok::Num(v) => {
                        let v = *v;
                        it.pos += 1;
                        Some(v)
                    }
                    __Tok::Float(f) if float => {
                        let f = *f;
                        it.pos += 1;
                        Some(f.to_bits() as i64)
                    }
                    __Tok::LParen => {
                        it.pos += 1;
                        let v = __expr(it, float)?;
                        if !it.eat(&__Tok::RParen) {
                            it.pos = save;
                            return None;
                        }
                        Some(v)
                    }
                    __Tok::Ident(s) => {
                        // .equ 符号常量：equ 表在 parse_insts 层维护，经
                        // __assembler_equ 查找；找不到 → 回滚（调用方可能
                        // 转标签/寄存器解析）。
                        let v = __lookup_equ(s)?;
                        it.pos += 1;
                        Some(v)
                    }
                    _ => {
                        it.pos = save;
                        None
                    }
                }
            }

            /// .equ 符号查找（由 gen_assembler 注入的全局常量表）。
            fn __lookup_equ(name: &str) -> Option<i64> {
                __EQU.with(|m| m.borrow().get(name).copied())
            }
        });
    }
    if has_label {
        let mut label_set_arms: Vec<TokenStream> = Vec::new();
        for info in infos {
            let vn = &info.vn;
            if let Some((n, fid)) = info
                .operands
                .iter()
                .enumerate()
                .find_map(|(i, (_, fid, s, _))| (s.kind == OperandKind::Label).then_some((i, fid)))
            {
                label_set_arms
                    .push(quote! { (Inst::#vn { #fid, .. }, #n) => { *#fid = val; true } });
            }
        }
        out.extend(quote! {
            /// 标签：数字/表达式 = 偏移；非寄存器 ident = 符号引用（回填期解析）。
            /// 失败回滚。
            fn __label(
                it: &mut __Iter,
                min: i64,
                max: i64,
                syms: &mut Vec<(usize, String)>,
                op: usize,
            ) -> Option<i64> {
                let save = it.pos;
                // 优先尝试完整表达式（数字、算术、括号、符号常量）：
                // `40+2` → 42、`A*2` → 求值。失败回滚到单数字/符号分支。
                if let Some(v) = __expr(it, false) {
                    if v < min || v > max { it.pos = save; return None; }
                    return Some(v);
                }
                it.pos = save;
                match it.toks.get(it.pos)? {
                    __Tok::Num(v) => {
                        let v = *v;
                        it.pos += 1;
                        if v < min || v > max { it.pos = save; return None; }
                        Some(v)
                    }
                    __Tok::Minus => {
                        it.pos += 1;
                        let v = match it.toks.get(it.pos)? {
                            __Tok::Num(v) => { it.pos += 1; *v }
                            _ => { it.pos = save; return None; }
                        };
                        let v = match v.checked_neg() {
                            Some(x) => x,
                            None => { it.pos = save; return None; }
                        };
                        if v < min || v > max { it.pos = save; return None; }
                        Some(v)
                    }
                    __Tok::Ident(s) => {
                        if <Reg as FromStr>::from_str(s).is_ok() { return None; }
                        syms.push((op, s.clone()));
                        it.pos += 1;
                        Some(0)
                    }
                    _ => None,
                }
            }
            /// 按操作数序号回填 label 槽值（parse_insts 两遍布局用）。
            fn __set_label_operand(inst: &mut Inst, idx: usize, val: i64) -> bool {
                match (inst, idx) {
                    #(#label_set_arms,)*
                    _ => false,
                }
            }
        });
    }
    if has_mem {
        out.extend(quote! {
            /// 内存 `[base(±disp)]` / `[base+index(*scale)(±disp)]`：token 化后
            /// 括号内空白免疫。失败回滚。
            fn __mem(it: &mut __Iter) -> Option<MemRef> {
                let save = it.pos;
                if !it.eat(&__Tok::LBracket) { return None; }
                let base = match __reg_cls(it) {
                    Some((r, _)) => r,
                    None => { it.pos = save; return None; }
                };
                let mut index: Option<Reg> = None;
                let mut scale: u8 = 1;
                let mut disp: i64 = 0;
                if it.eat(&__Tok::Plus) {
                    if let Some((r, _)) = __reg_cls(it) {
                        // `+index`（可带 `*scale`）
                        index = Some(r);
                        if it.eat(&__Tok::Star) {
                            let s = match it.toks.get(it.pos) {
                                Some(__Tok::Num(v)) => { it.pos += 1; *v }
                                _ => { it.pos = save; return None; }
                            };
                            if s != 1 && s != 2 && s != 4 && s != 8 {
                                it.pos = save;
                                return None;
                            }
                            scale = s as u8;
                        }
                    } else {
                        // 纯位移 `+disp`
                        let neg = it.eat(&__Tok::Minus);
                        let v = match __raw_int(it) {
                            Some(v) => v,
                            None => { it.pos = save; return None; }
                        };
                        disp = if neg { -v } else { v };
                    }
                    // 有 index 时可选 `±disp`
                    if index.is_some() {
                        if it.eat(&__Tok::Plus) {
                            let v = match __raw_int(it) {
                                Some(v) => v,
                                None => { it.pos = save; return None; }
                            };
                            disp = v;
                        } else if it.eat(&__Tok::Minus) {
                            let v = match __raw_int(it) {
                                Some(v) => v,
                                None => { it.pos = save; return None; }
                            };
                            disp = -v;
                        }
                    }
                } else if it.eat(&__Tok::Minus) {
                    let v = match __raw_int(it) {
                        Some(v) => v,
                        None => { it.pos = save; return None; }
                    };
                    disp = -v;
                }
                if !it.eat(&__Tok::RBracket) { it.pos = save; return None; }
                Some(MemRef { base, disp, index, scale })
            }
            fn __raw_int(it: &mut __Iter) -> Option<i64> {
                let v = match it.toks.get(it.pos)? { __Tok::Num(v) => *v, _ => return None };
                it.pos += 1;
                Some(v)
            }
            fn __eat_lit(it: &mut __Iter, lit: &[__Tok]) -> bool {
                let mut p = it.pos;
                for t in lit {
                    if it.toks.get(p) != Some(t) { return false; }
                    p += 1;
                }
                it.pos = p;
                true
            }
        });
    } else if has_reg {
        out.extend(quote! {
            fn __eat_lit(it: &mut __Iter, lit: &[__Tok]) -> bool {
                let mut p = it.pos;
                for t in lit {
                    if it.toks.get(p) != Some(t) { return false; }
                    p += 1;
                }
                it.pos = p;
                true
            }
        });
    }
    if has_cond {
        // 条件码表：声明优先，缺省 x86
        let table = model
            .conventions
            .cond
            .as_ref()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect::<Vec<_>>())
            .unwrap_or_else(|| {
                cond_default()
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect()
            });
        // 按编码值分组别名（b|c|nae => 2）
        let mut by_code: BTreeMap<u64, Vec<String>> = BTreeMap::new();
        for (k, v) in &table {
            by_code.entry(*v).or_default().push(k.clone());
        }
        let mut parse_arms: Vec<TokenStream> = Vec::new();
        let mut render_arms: Vec<TokenStream> = Vec::new();
        for (code, names) in &by_code {
            let alts: Vec<_> = names.iter().map(|n| n.as_str()).collect();
            let c = *code as u8;
            parse_arms.push(quote! { #(#alts)|* => #c });
            // 渲染用首选名（声明表首项）
            let first = syn::LitStr::new(alts[0], proc_macro2::Span::call_site());
            render_arms.push(quote! { #c => #first.into() });
        }
        out.extend(quote! {
            fn __cond(it: &mut __Iter) -> Option<u8> {
                let __Tok::Ident(name) = it.toks.get(it.pos)?.clone() else { return None; };
                let n = name.to_ascii_lowercase();
                let code = match n.as_str() {
                    #(#parse_arms,)*
                    _ => return None,
                };
                it.pos += 1;
                Some(code)
            }
            fn __render_cond(c: u8) -> String {
                match c & 0xF {
                    #(#render_arms,)*
                    _ => "?".into(),
                }
            }
        });
    }
    Ok(out)
}

/// 单操作数的解析表达式 + 匹配模式 + （可选）宽度一致性绑定名。
/// `wreq`：form opsize = r<宽> 时对全 GPR 槽的固定宽度过滤（位）。
fn operand_parse_tok(
    slot: &OperandSlot,
    fid: &syn::Ident,
    n: usize,
    info: &InstInfo,
) -> Result<(TokenStream, TokenStream, Option<syn::Ident>), String> {
    let all_gpr = |cs: &[RegClass]| cs.iter().all(|c| matches!(c, RegClass::GPR(_)));
    let gpr_only = slot.classes().as_deref().map(all_gpr).unwrap_or(true);
    // `PhysReg::width()` 返回组 payload（字节，如 gpr8 → 8）；Opsize::Reg(w)
    // 的 w 同为字节（r8 = 64 位）——同单位比较。**只作用于多类槽**：
    // 单类槽（含内存基址寄存器）已由 class 过滤保证宽度，不受 opsize 影响。
    let op = info.inst.opsize.or(info.form.opsize);
    let wreq = match op {
        Some(Opsize::Reg(w)) if gpr_only && slot.classes().map(|c| c.len() > 1).unwrap_or(true) => {
            Some(w)
        }
        _ => None,
    };
    match slot.kind {
        OperandKind::Reg => {
            let classes = slot.classes().unwrap_or_default();
            let class_toks: Vec<_> = classes.iter().map(|c| quote! { #c }).collect();
            let wreq_ts = match wreq {
                Some(w) => quote! { Some(#w) },
                None => quote! { None },
            };
            // 一致性检查仅对 opsize=Slot 的多宽 GPR form（宽度由操作数推导）
            let need_cls = matches!(op, Some(Opsize::Slot(_))) && gpr_only;
            let cb = if need_cls {
                Some(format_ident!("__c{n}"))
            } else {
                None
            };
            let pat = if let Some(c) = &cb {
                quote! { Some((#fid, #c)) }
            } else {
                quote! { Some((#fid, _)) }
            };
            let elem = quote! { __reg_f(&mut it, &[#(#class_toks),*], #wreq_ts) };
            Ok((elem, pat, cb))
        }
        OperandKind::Imm => {
            let (min, max) = slot.imm_range().unwrap_or((i64::MIN, i64::MAX));
            let float = slot.float == Some(true);
            let elem = quote! { __imm(&mut it, #min, #max, #float) };
            Ok((elem, quote! { Some(#fid) }, None))
        }
        OperandKind::Label => {
            let (min, max) = slot.imm_range().unwrap_or((i64::MIN, i64::MAX));
            let elem = quote! { __label(&mut it, #min, #max, &mut __syms, #n) };
            Ok((elem, quote! { Some(#fid) }, None))
        }
        OperandKind::Mem => Ok((quote! { __mem(&mut it) }, quote! { Some(#fid) }, None)),
        OperandKind::Cond => Ok((quote! { __cond(&mut it) }, quote! { Some(#fid) }, None)),
    }
}

/// 单条指令的 assemble 尝试：逐段消费 token（字面段 = token 序列匹配；
/// 占位符段 = 按槽类型解析）。任一失败静默回退下一形状（多形状回退语义）。
fn gen_assemble_try_tok(info: &InstInfo) -> Result<TokenStream, String> {
    let vn = &info.vn;
    let ops = ops_template(info);
    let segs = parse_template(&ops)?;
    validate_segs(&segs, info)?;
    let mut elems: Vec<TokenStream> = Vec::new();
    let mut pats: Vec<TokenStream> = Vec::new();
    let mut cls_binds: Vec<syn::Ident> = Vec::new();
    for seg in &segs {
        match seg {
            Seg::Lit(l) => {
                let toks = tokenize(l).map_err(|e| format!("asm template literal '{l}': {e}"))?;
                let exprs: Vec<_> = toks.iter().map(tok_expr).collect();
                elems.push(quote! { __eat_lit(&mut it, &[#(#exprs),*]).then_some(()) });
                pats.push(quote! { Some(()) });
            }
            Seg::Op(n) => {
                let (_, fid, slot, _) = &info.operands[*n];
                let (e, p, cb) = operand_parse_tok(slot, fid, *n, info)?;
                elems.push(e);
                pats.push(p);
                if let Some(c) = cb {
                    cls_binds.push(c);
                }
            }
        }
    }
    elems.push(quote! { it.eof() });
    pats.push(quote! { true });
    // opsize=Slot 多宽 form：全部 GPR 操作数宽度一致性（`mov rax, ebx` → 拒绝）
    let mut eqs: Vec<TokenStream> = Vec::new();
    for pair in cls_binds.windows(2) {
        let a = &pair[0];
        let b = &pair[1];
        eqs.push(quote! { #a == #b });
    }
    let consistency = if eqs.is_empty() {
        quote! { true }
    } else {
        quote! { #(#eqs)&&* }
    };
    let ctor = if info.operands.is_empty() {
        quote! { Inst::#vn }
    } else {
        let ids: Vec<_> = info
            .operands
            .iter()
            .map(|(_, fid, _, _)| fid.clone())
            .collect();
        quote! { Inst::#vn { #(#ids),* } }
    };
    Ok(quote! {
        {
            let mut it = __Iter { toks: __rest, pos: 0 };
            if let (#(#pats),*) = (#(#elems),*) {
                if #consistency {
                    return Ok((#ctor, std::mem::take(&mut __syms)));
                }
            }
        }
    })
}

/// form 特异性：槽约束集越小越具体（多 form 同助记符时分发顺序）。
/// 无 class（任意类）按 64 计（最不具体）。非 reg 槽按 1 计。
fn form_specificity(info: &InstInfo, _model: &V12Model) -> u64 {
    let mut spec = 1u64;
    for (_, _, slot, _) in &info.operands {
        if slot.kind == OperandKind::Reg {
            let n = slot.classes().map(|c| c.len() as u64).unwrap_or(64).max(1);
            spec = spec.saturating_mul(n);
        }
    }
    spec
}

/// 类型签名：字面 token 序列 + 每操作数槽签名（真重复检测）。
fn type_signature(info: &InstInfo) -> Result<String, String> {
    let segs = parse_template(&ops_template(info))?;
    validate_segs(&segs, info)?;
    let mut s = String::new();
    for seg in &segs {
        match seg {
            Seg::Lit(l) => {
                let toks = tokenize(l).map_err(|e| format!("asm template literal '{l}': {e}"))?;
                for t in toks {
                    s.push_str(&format!("T{:?}", t));
                }
            }
            Seg::Op(n) => {
                let slot = &info.operands[*n].2;
                let sig = match slot.kind {
                    OperandKind::Reg => format!("R{:?}", slot.classes().unwrap_or_default()),
                    OperandKind::Imm => "I".to_string(),
                    OperandKind::Label => "L".to_string(),
                    OperandKind::Mem => "M".to_string(),
                    OperandKind::Cond => "C".to_string(),
                };
                s.push_str(&format!("O{n}:{sig}"));
            }
        }
    }
    Ok(s)
}

/// assemble 入口：`pub(crate)` 由 `codegen::generate()` 调用。
pub(crate) fn gen_assemble(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    let lexer = gen_lexer_ts(model)?;
    let primitives = gen_asm_primitives(model, infos)?;
    // 按助记符分组（保声明序）；组内特异性排序（窄约束先，声明序稳定）
    let mut groups: Vec<(String, Vec<&InstInfo>)> = Vec::new();
    for info in infos {
        let mn = &info.mnemonic;
        match groups.iter_mut().find(|(m, _)| m == mn) {
            Some((_, v)) => v.push(info),
            None => groups.push((mn.clone(), vec![info])),
        }
    }
    let mut arms: Vec<TokenStream> = Vec::new();
    for (mn, group) in &mut groups {
        group.sort_by_key(|info| form_specificity(info, model));
        // 真重复（同字面骨架 + 同类型签名）跳过，保留首个声明
        let mut seen: Vec<String> = Vec::new();
        let mut tries: Vec<TokenStream> = Vec::new();
        for info in group {
            let sig = type_signature(info)?;
            if seen.contains(&sig) {
                continue;
            }
            seen.push(sig);
            tries.push(gen_assemble_try_tok(info)?);
        }
        let mn_lit = syn::LitStr::new(mn, proc_macro2::Span::call_site());
        let err_lit = syn::LitStr::new(
            &format!("{mn}: operand mismatch"),
            proc_macro2::Span::call_site(),
        );
        arms.push(quote! {
            #mn_lit => {
                let mut __syms: Vec<(usize, String)> = Vec::new();
                #(#tries)*
                return Err(String::from(#err_lit));
            }
        });
    }
    let mn_prep = match model.meta.mnemonic_case {
        MnemonicCase::Insensitive => quote! { let __mn = mn.to_ascii_lowercase(); },
        MnemonicCase::Sensitive => quote! { let __mn = mn.clone(); },
    };
    Ok(quote! {
        #lexer
        #primitives
        /// 汇编单条文本 → 指令（token 驱动；同助记符多 form 按类型签名自动分发）。
        pub fn assemble(text: &str) -> Result<Inst, String> {
            let (inst, syms) = __assemble(text)?;
            if syms.is_empty() {
                Ok(inst)
            } else {
                Err("label operands require TargetAssembler::parse_insts (two-pass layout)".into())
            }
        }
        /// 内部装配：返回 (指令, 未解析符号引用列表 (操作数序号, 符号名))。
        pub(crate) fn __assemble(text: &str) -> Result<(Inst, Vec<(usize, String)>), String> {
            let toks = __lex(text)?;
            let Some(__Tok::Ident(mn)) = toks.first() else {
                return Err("expected mnemonic".into());
            };
            let __rest = &toks[1..];
            #mn_prep
            match __mn.as_str() {
                #(#arms,)*
                _ => Err(format!("unknown mnemonic '{mn}'")),
            }
        }
    })
}

