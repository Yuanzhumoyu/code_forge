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
        // 完整格式 = 整条 asm（前导字面 + 操作数模板段序列渲染）
        let segs = parse_template(&info.inst.asm)?;
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
        OperandKind::Bits => {
            let pairs: Vec<TokenStream> = slot
                .table_entries
                .iter()
                .map(|(n, b)| {
                    let n = syn::LitStr::new(n, proc_macro2::Span::call_site());
                    let b = proc_macro2::Literal::u64_unsuffixed(*b);
                    quote! { (#n, #b) }
                })
                .collect();
            quote! { __render_bitset(*#fid, &[#(#pairs),*]) }
        }
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
///
/// v15 起模板里的占位符**只能是索引**（`{0}`/`{1}`…）——命名形态
/// （`{dst}`）由 `collect_inst_infos` 按 `ops` 声明序规范化成索引后才进来；
/// v14 的内联声明 `{i:[槽:角色]}` 已删除（声明归 `ops`，模板只负责引用）。
/// `pub(crate)`：`collect_inst_infos`（mod.rs）复用。
pub(crate) fn parse_template(tpl: &str) -> Result<Vec<Seg>, String> {
    if tpl.is_empty() {
        return Ok(Vec::new());
    }
    let mut segs = Vec::new();
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
            let n: usize = inner.trim().parse().map_err(|_| {
                format!(
                    "bad placeholder '{{{inner}}}' in asm template '{tpl}'：\
                     模板只能引用操作数序号（命名引用需在 `ops` 里声明）"
                )
            })?;
            if !lit.is_empty() {
                segs.push(Seg::Lit(std::mem::take(&mut lit)));
            }
            segs.push(Seg::Op(n));
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
    Ok(segs)
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

/// 命名占位模板（`[[pseudo]].asm`）的段：`{名字}` 是**参数名**，不是操作数序号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NamedSeg {
    Lit(String),
    Op(String),
}

/// 解析命名占位模板（`"li {rd}, {imm}"`）→ 段序列。
///
/// 与操作数模板（[`parse_template`]，占位符是**序号**、由 `ops` 声明序规范化后进来）
/// 分开：伪指令的 `asm` 是**这条伪指令自己的书写规范**，占位符就是**参数名**——
/// 它是 `emit` 里 `{名字}` 的唯一来源（不再有第二份 `params` 清单可与它漂移）。
pub(crate) fn parse_named_template(tpl: &str) -> Result<Vec<NamedSeg>, String> {
    if tpl.trim().is_empty() {
        return Err("empty asm template".into());
    }
    let mut segs: Vec<NamedSeg> = Vec::new();
    let mut lit = String::new();
    let mut chars = tpl.chars();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                let mut inner = String::new();
                let mut closed = false;
                for d in chars.by_ref() {
                    if d == '}' {
                        closed = true;
                        break;
                    }
                    inner.push(d);
                }
                if !closed {
                    return Err(format!("unterminated '{{' in asm template '{tpl}'"));
                }
                let name = inner.trim().to_string();
                if name.is_empty() {
                    return Err(format!("empty placeholder '{{}}' in asm template '{tpl}'"));
                }
                if !lit.is_empty() {
                    segs.push(NamedSeg::Lit(std::mem::take(&mut lit)));
                }
                segs.push(NamedSeg::Op(name));
            }
            '}' => return Err(format!("stray '}}' in asm template '{tpl}'")),
            _ => lit.push(c),
        }
    }
    if !lit.is_empty() {
        segs.push(NamedSeg::Lit(lit));
    }
    if segs.is_empty() {
        return Err(format!("empty asm template '{tpl}'"));
    }
    Ok(segs)
}

/// 模板里的参数名（**按首次出现序**、去重）——`[[pseudo]]` 的参数表就长在这里。
pub(crate) fn named_params(segs: &[NamedSeg]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in segs {
        if let NamedSeg::Op(n) = s
            && !out.iter().any(|x| x == n)
        {
            out.push(n.clone());
        }
    }
    out
}

/// 模板的**分派键**：首个字面段的首词。伪指令按**写法**选候选（不是按 `name`——
/// `name` 只是这条声明的标识，唯一即可）。
///
/// 模板必须字面开头（否则没有可判定的前缀，会吞掉别的行）⇒ 恒为 `Some`；
/// 返回 `None` 表示形状非法（由 [`validate_named_template`] 报错）。
pub(crate) fn named_dispatch_key(segs: &[NamedSeg]) -> Option<String> {
    match segs.first() {
        Some(NamedSeg::Lit(l)) => l.split_whitespace().next().map(str::to_string),
        _ => None,
    }
}

/// 命名模板的形状校验：**字面开头**（分派前缀）、字面段非空、占位符之间要有字面分隔。
///
/// 与 `[[instructions]].name` 无关：`name` 只是这条声明的唯一标识，写法由 `asm` 自己
/// 说清楚——两者不必相同、也不同步（`name = "load_imm"` + `asm = "li {rd}, {imm}"` 合法）。
pub(crate) fn validate_named_template(segs: &[NamedSeg]) -> Result<(), String> {
    match segs.first() {
        Some(NamedSeg::Lit(_)) => {}
        _ => {
            return Err(
                "asm 模板必须以**字面**开头（分派按模板的前导字面；占位符开头的写法\
                 没有可判定的前缀，会吞掉别的行）"
                    .into(),
            );
        }
    }
    let mut prev_op = false;
    for seg in segs {
        match seg {
            NamedSeg::Lit(l) => {
                if l.is_empty() {
                    return Err("asm 模板里有空字面段".into());
                }
                prev_op = false;
            }
            NamedSeg::Op(_) => {
                if prev_op {
                    return Err(
                        "asm 模板里两个占位符相邻——中间至少要有一个字面分隔符（否则切不开）".into(),
                    );
                }
                prev_op = true;
            }
        }
    }
    Ok(())
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
fn gen_lexer_ts(_model: &IsaModel) -> Result<TokenStream, String> {
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

        /// 符号常量表（`.equ`/`.set`；parse_insts 在汇编前填充，指令的立即数与位移
        /// 求值读取——两者共用同一套值语法）。
        #[allow(dead_code)]
        thread_local! {
            static __EQU: std::cell::RefCell<std::collections::HashMap<String, i64>> =
                std::cell::RefCell::new(std::collections::HashMap::new());
        }

        /// **符号引用收集**（`[[operand_slots]].symbols`）：`__primary` 遇到未知 ident 且
        /// 被**武装**时写这里（`(修饰下标, 名字)`），当 0 参与算术；命中后随 `__lsyms` 一起
        /// 提交、由两遍回填解析成"标签的块下标（或 `.` 的自引用）"。只有声明了 `symbols` 的槽
        /// 在解析自己的值时武装——`.equ`/`.set` 的表达式与其它槽照旧"未知 ident 即不匹配"。
        #[allow(dead_code)]
        thread_local! {
            static __SYMREF: std::cell::RefCell<Vec<(Option<usize>, String)>> =
                std::cell::RefCell::new(Vec::new());
            static __SYMREF_ON: std::cell::Cell<bool> = std::cell::Cell::new(false);
            static __SYMREF_MOD: std::cell::Cell<Option<usize>> = std::cell::Cell::new(None);
        }

        #[allow(dead_code)]
        /// `.` 是否算 ident 的开头：下一字符是 ident 片段才算
        /// （`.Lp` 是局部标签名；裸 `.` 是位置符号，走 `__Tok::Dot`）。
        fn is_ident_start(cs: &mut std::iter::Peekable<std::str::Chars<'_>>) -> bool {
            if cs.peek() != Some(&'.') {
                return true;
            }
            let mut t = cs.clone();
            t.next();
            t.peek()
                .is_some_and(|c| c.is_alphabetic() || *c == '_' || *c == '.')
        }
        fn __lex(s: &str) -> Result<Vec<__Tok>, String> {
            let mut out: Vec<__Tok> = Vec::new();
            let mut cs = s.chars().peekable();
            while let Some(&c) = cs.peek() {
                match c {
                    ' ' | '\t' | '\r' | '\n' | '\u{0c}' => { cs.next(); }
                    // **点开头的局部标签名**（`.Lp`、`.Lpcrel_hi0`——真实语料里遍地都是）：
                    // 下一个字符还是 ident 片段时**整个当一个 ident**；单独一个 `.` 才是
                    // 位置符号（`jal zero, .`，走下面的 '.' 分支）。
                    'a'..='z' | 'A'..='Z' | '_' | '.' if is_ident_start(&mut cs) => {
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
                            // **十六进制字面量是位模式**（不是"数学值"）：先按 u64 读、再按位
                            // 重解释成 i64——否则 64 位槽写不了 `0xfffffffffffffff0`（= -16）、
                            // `0x8000000000000000` 这类写法（实测 `and sp, x5, #0xfffffffffffffff0`
                            // 报 "bad hex immediate"）。十进制仍是**值**（超 i64 就报错）。
                            out.push(__Tok::Num(
                                u64::from_str_radix(&h, 16).map_err(|_| "bad hex immediate".to_string())? as i64,
                            ));
                        } else if first == '0' && matches!(cs.peek(), Some('b') | Some('B')) {
                            cs.next();
                            let mut b = String::new();
                            while let Some(&d) = cs.peek() {
                                if d == '0' || d == '1' { b.push(d); cs.next(); } else { break; }
                            }
                            if b.is_empty() { return Err("bad binary immediate".into()); }
                            // 二进制同上：位模式（`0b1000…` 64 位写法）。
                            out.push(__Tok::Num(
                                u64::from_str_radix(&b, 2).map_err(|_| "bad binary immediate".to_string())? as i64,
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

/// 生成共享解析原语（按需：仅当 ISA 使用对应槽类别时发出）。
fn gen_asm_primitives(model: &IsaModel, infos: &[InstInfo]) -> Result<TokenStream, String> {
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
    let _ = (has_imm, has_label);

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
    // 表达式/标签helper **无条件生成**：`.equ`/`.set` 伪指令、内存位移与 `parse_insts`
    // 的符号回填永远引用它们（`__expr` / `__set_label_operand`），与"本 ISA 有没有
    // imm/label 操作数"无关。历史实现按 has_imm/has_label 门控定义 → 只有寄存器操作数的
    // ISA（夹具 `demo_inst12`）生成出**引用未定义 helper** 的模块（编译不过）。
    // 未被本 ISA 用到的入口函数加 `#[allow(dead_code)]`。
    // 立即数修饰（`[[conventions.imm_fn]]`）：**谱声明的数据**——本文件不认识任何具体
    // 修饰名（`%hi`/`:lower16:` 都只是谱里的两行文本），只按 `text` 的模板匹配、
    // 按 `expr` 的值语义求值（复用既有表达式求值器）。
    let (imm_fn_defs, imm_fn_tries) = gen_imm_fns(model)?;
    out.extend(imm_fn_defs);
    // 立即数前缀（`[meta].imm_prefix`）：**按声明吃对应 token**——历史上这里硬编码只认
    // `$`，声明 `#` 的谱（A64 风格）写了却不起作用（静默不生效）。token 只有 `Dollar`/`Hash`
    // 两种（词法就这些），别的字符在校验期就被拒了。
    //
    // `__imm`（立即数）与 `__label`（标签/分支目标）两个助手都要吃它——A64 的 `b #28`
    // 与 `add x0, x1, #5` 都是 `#` 开头的源文本。
    let imm_pref: TokenStream = match model.meta.imm_prefix.as_deref() {
        Some("$") => quote! { it.eat(&__Tok::Dollar) },
        Some("#") => quote! { it.eat(&__Tok::Hash) },
        _ => quote! { false },
    };
    {
        out.extend(quote! {
            /// 立即数/表达式求值：数字、正负号、括号、算术（+ - * / % << >> & | ^ ~）、
            /// 符号常量（`.equ`/`.set`）、**（声明了 `symbols` 的槽）符号引用**。
            /// 失败回滚 token 位置。
            ///
            /// `unit` = 该槽的源值单位（缺省 1；`#`-前缀的字节偏移类操作数用它）：
            /// 源值必须是它的整数倍，否则本条候选**不匹配**（与越界同一处理，
            /// 不静默取整）。
            ///
            /// `syms`/`op`/`allow_sym`：声明了 `symbols` 的槽在求值期间**武装**符号收集
            /// （`__primary` 把未知 ident 记成"符号引用 + 当前修饰下标"并当 0 参与运算），
            /// 命中后随 `__lsyms` 一起提交、由两遍回填解析。未声明 `symbols` 的槽
            /// 不武装——未知 ident 照旧是不匹配（**能力是声明的，不是隐含的**）。
            #[allow(dead_code)]
            fn __imm(
                it: &mut __Iter,
                min: i64,
                max: i64,
                float: bool,
                unit: i64,
                syms: &mut Vec<(usize, Option<usize>, String)>,
                op: usize,
                allow_sym: bool,
            ) -> Option<i64> {
                let save = it.pos;
                let _d = #imm_pref;
                __SYMREF.with(|r| r.borrow_mut().clear());
                __SYMREF_ON.with(|a| a.set(allow_sym));
                let v = __expr(it, float);
                __SYMREF_ON.with(|a| a.set(false));
                let v = v?;
                if v < min || v > max || (unit > 1 && v % unit != 0) {
                    __SYMREF.with(|r| r.borrow_mut().clear());
                    it.pos = save;
                    return None;
                }
                for (m, n) in __SYMREF.with(|r| r.borrow_mut().split_off(0)) {
                    syms.push((op, m, n));
                }
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

            /// 一元：正负号 / 按位非 / 谱声明的立即数修饰（`[[conventions.imm_fn]]`）/
            /// 主元（数字、括号、符号常量）。
            fn __unary(it: &mut __Iter, float: bool) -> Option<i64> {
                let save = it.pos;
                match it.toks.get(it.pos)? {
                    // 一元 `+` 是**恒等**：上游语料里 `lwu x2, +4(x3)`、`[+8]` 都这么写。
                    __Tok::Plus => {
                        it.pos += 1;
                        __unary(it, float)
                    }
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
                    // 一元 `!` = **逻辑非**（`!x` = x==0 ? 1 : 0），**不是**按位取反——
                    // 上游语料实证：`lh t1, !1(zero)` 的期望位移是 **0**（`03 13 00 00`），
                    // `ori a0, a1, !1` 期望 `ori a0, a1, 0`；按位取反那条写的是 `~2047`
                    // （期望 `-2048`）。这是**表达式运算符**（与 `~` 同类，任何 ISA 都可能
                    // 出现在立即数里），不是 ISA 知识。
                    __Tok::Bang => {
                        it.pos += 1;
                        let v = __unary(it, float)?;
                        Some(if v == 0 { 1 } else { 0 })
                    }
                    // 谱声明的立即数修饰（`%hi(x)`、`:lower16:x`…）：**本文件不认识任何
                    // 具体修饰名**——文本形态与值语义都来自 `[[conventions.imm_fn]]`。
                    _ => {
                        #imm_fn_tries
                        __primary(it, save, float)
                    }
                }
            }

            /// 主元：数字 / 浮点位模式 / 括号表达式 / 符号常量（`.equ`/`.set`）。
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
                        // 符号常量（`.equ`/`.set` 定义）：表在 parse_insts 层维护；
                        // 找不到 → （声明了 `symbols` 的槽被武装时）记成**符号引用**，
                        // 当 0 参与算术、由两遍回填解析；否则回滚。
                        if let Some(v) = __lookup_equ(s) {
                            it.pos += 1;
                            return Some(v);
                        }
                        if __SYMREF_ON.with(|a| a.get()) {
                            __SYMREF.with(|r| {
                                r.borrow_mut().push((__SYMREF_MOD.with(|m| m.get()), s.clone()))
                            });
                            it.pos += 1;
                            return Some(0);
                        }
                        None
                    }
                    _ => {
                        it.pos = save;
                        None
                    }
                }
            }

            /// 符号常量查找（表由 `.equ`/`.set` 填充）。
            fn __lookup_equ(name: &str) -> Option<i64> {
                __EQU.with(|m| m.borrow().get(name).copied())
            }
        });
    }
    {
        out.extend(gen_symbol_setter(infos, model));
        // 标签/分支目标槽的解析助手（`__label`）：数字/表达式 = 源单位偏移；
        // 非寄存器 ident = 符号引用（回填期解析成"块下标 × unit"）；`.` = 自引用。
        out.extend(quote! {
            /// 标签：数字/表达式 = **源单位**偏移；非寄存器 ident = 符号引用
            /// （回填期解析成 `块下标 × unit`）；`.` = **本条指令自身的地址**（自引用，
            /// 回填期换成当前指令的下标）。失败回滚。
            #[allow(dead_code)]
            fn __label(
                it: &mut __Iter,
                min: i64,
                max: i64,
                syms: &mut Vec<(usize, Option<usize>, String)>,
                op: usize,
                unit: i64,
            ) -> Option<i64> {
                let save = it.pos;
                // `unit`（缺省 1）：源值必须是它的整数倍——`b #28` 的 28 是字节偏移，
                // 而字段数的是 4 字节字。不取整、不静默截断，不匹配就退下一条候选。
                let ok = |v: i64| v >= min && v <= max && (unit <= 1 || v % unit == 0);
                let _d = #imm_pref;
                // 优先尝试完整表达式（数字、算术、括号、符号常量）：
                // `40+2` → 42、`A*2` → 求值。失败回滚到**含前缀**的起点。
                if let Some(v) = __expr(it, false) {
                    if !ok(v) { it.pos = save; return None; }
                    return Some(v);
                }
                it.pos = save;
                let _d = #imm_pref;
                let v = match it.toks.get(it.pos)? {
                    __Tok::Num(v) => {
                        let v = *v;
                        it.pos += 1;
                        v
                    }
                    __Tok::Minus => {
                        it.pos += 1;
                        let v = match it.toks.get(it.pos)? {
                            __Tok::Num(v) => { it.pos += 1; *v }
                            _ => { it.pos = save; return None; }
                        };
                        match v.checked_neg() {
                            Some(x) => x,
                            None => { it.pos = save; return None; }
                        }
                    }
                    __Tok::Ident(s) => {
                        if <Reg as FromStr>::from_str(s).is_ok() { return None; }
                        syms.push((op, None, s.clone()));
                        it.pos += 1;
                        return Some(0);
                    }
                    __Tok::Dot => {
                        // `.` = 本条指令自身的地址：记成**自引用**（空名），回填期换成
                        // 当前指令的下标——与"标签回填块下标"同一口径。
                        syms.push((op, None, String::new()));
                        it.pos += 1;
                        return Some(0);
                    }
                    _ => { it.pos = save; return None; }
                };
                if !ok(v) { it.pos = save; return None; }
                Some(v)
            }
        });
    }
    if has_mem {
        let tpls = super::mem::effective_templates(&model.conventions.mem);
        let mut parsed: Vec<Vec<super::mem::Item>> = Vec::with_capacity(tpls.len());
        for (k, t) in tpls.iter().enumerate() {
            parsed.push(
                super::mem::parse_mem_template(t)
                    .map_err(|e| format!("[conventions.mem] templates[{k}]: {e}"))?,
            );
        }
        let kws = super::mem::size_keyword_toks(&model.conventions.mem)?;
        let ci = matches!(model.meta.mnemonic_case, MnemonicCase::Insensitive);
        let mem_parser = super::mem::gen_mem_parser(&parsed, &kws, ci);
        out.extend(quote! {
            /// 内存操作数解析（由 `[conventions.mem] templates` 派生；v16/V10）。
            #mem_parser
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
        // 条件码表：**必须由 ISA 声明**（v18 S3b 起不再回退 x86 的 16 项表——
        // 那等于把别家的条件名与编码写进通用生成器）。键 = 汇编可见的名字。
        let Some(table) = model.conventions.cond.as_ref() else {
            return Err(
                "[conventions.cond]: 本 ISA 有 `cond` 槽操作数，必须声明条件码表\
                 （键 = 汇编可见的条件名；每条给 code，并用 ir = \"<IR 条件名>\" 指出\
                 它实现哪个 IR 整数条件）——不按某个 ISA 的表兜底"
                    .into(),
            );
        };
        let table: Vec<(String, u64)> = table.iter().map(|(k, e)| (k.clone(), e.code)).collect();
        // 按编码值分组别名（b|c|nae => 2）；渲染取该编码**字母序最小**的名字
        //（BTreeMap 迭代序 = 字母序，故与 S3b 之前的行为逐字节一致）。
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

/// 拆 `text`/`expr` 里的 `{0}`：返回 (前段, 后段)；没有或不止一个 ⇒ None。
///
/// `{0}` 是**唯一**的占位：`text` 里它是"内层表达式"的位置，`expr` 里它是"内层值"。
pub(crate) fn split_hole(s: &str) -> Option<(&str, &str)> {
    let (pre, rest) = s.split_once("{0}")?;
    if rest.contains("{0}") {
        return None;
    }
    Some((pre, rest))
}

/// 立即数修饰（`[[conventions.imm_fn]]`）的生成：**谱声明的数据**，本文件不认识任何
/// 具体修饰名（`%hi(x)`、`:lower16:x`、`hi(x)` 都只是谱里的两行文本）。
///
/// 每条声明两件事：
///
/// - `text`：源文本形态，`{0}` 处是内层表达式；其余按**字面 token** 匹配
///   （`mnemonic_case = insensitive` 时 Ident token 豁免大小写）；允许前缀/后缀/两侧都有；
/// - `expr`：值语义，`{0}` 处是内层表达式的**值**。生成期把 `expr` 词法化成 token 流：
///   运行期把 `{0}` 换成数值 token 再交给**既有表达式求值器** `__expr`——因此任意算术、
///   任意嵌套（`%hi(%lo(x))`、或某条修饰的 `expr` 里再用别的修饰）都天然成立，
///   不需要第二套表达式语言，也不需要为某个 ISA 写任何特殊分支。
///
/// 返回 `(定义块, __unary 里逐个试的语句)`；谱没声明 → 两者都空（生成物逐字不变）。
fn gen_imm_fns(model: &IsaModel) -> Result<(TokenStream, TokenStream), String> {
    let Some(defs) = model.conventions.imm_fn.as_ref().filter(|d| !d.is_empty()) else {
        return Ok((quote! {}, quote! {}));
    };
    let ci = matches!(model.meta.mnemonic_case, MnemonicCase::Insensitive);
    let mut fns: Vec<TokenStream> = Vec::new();
    let mut tries: Vec<TokenStream> = Vec::new();
    for (i, d) in defs.iter().enumerate() {
        let ctx = || format!("[[conventions.imm_fn.{}]]", d.name);
        let (pre_txt, post_txt) = split_hole(&d.text)
            .ok_or_else(|| format!("{}: text 必须恰好含一个 `{{0}}`（内层表达式的位置）", ctx()))?;
        let (pre_expr, post_expr) = split_hole(&d.expr)
            .ok_or_else(|| format!("{}: expr 必须恰好含一个 `{{0}}`（内层值）", ctx()))?;
        let pre_lit = tokenize(pre_txt.trim())
            .map_err(|e| format!("{}: text 前段 '{pre_txt}': {e}", ctx()))?;
        let post_lit = tokenize(post_txt.trim())
            .map_err(|e| format!("{}: text 后段 '{post_txt}': {e}", ctx()))?;
        if pre_lit.is_empty() && post_lit.is_empty() {
            return Err(format!(
                "{}: text 里没有任何字面——光一个 `{{0}}` 会吞掉任意表达式（改写作法的拼写）",
                ctx()
            ));
        }
        let pre_expr_toks: Vec<TokenStream> = tokenize(pre_expr.trim())
            .map_err(|e| format!("{}: expr 前段: {e}", ctx()))?
            .iter()
            .map(tok_expr)
            .collect();
        let post_expr_toks: Vec<TokenStream> = tokenize(post_expr.trim())
            .map_err(|e| format!("{}: expr 后段: {e}", ctx()))?
            .iter()
            .map(tok_expr)
            .collect();
        let fname = format_ident!("__imm_fn{i}");
        fns.push(quote! {
            /// 立即数修饰的值语义（谱声明；`{0}` 已换成本次的内层值）。
            #[allow(dead_code)]
            fn #fname(__x: i64) -> Option<i64> {
                let __toks: Vec<__Tok> = vec![
                    #(#pre_expr_toks,)*
                    __Tok::Num(__x),
                    #(#post_expr_toks),*
                ];
                let mut __it = __Iter { toks: &__toks, pos: 0 };
                let __v = __expr(&mut __it, false)?;
                if !__it.eof() { return None; }
                Some(__v)
            }
        });
        let pre_lits: Vec<TokenStream> = pre_lit.iter().map(tok_expr).collect();
        let post_lits: Vec<TokenStream> = post_lit.iter().map(tok_expr).collect();
        let pre_match = if pre_lits.is_empty() {
            quote! { true }
        } else {
            quote! { __imm_fn_lit(it, &[#(#pre_lits),*], #ci) }
        };
        let post_match = if post_lits.is_empty() {
            quote! { true }
        } else {
            quote! { __imm_fn_lit(it, &[#(#post_lits),*], #ci) }
        };
        tries.push(quote! {
            {
                let __msave = it.pos;
                if #pre_match {
                    let __inner = {
                        __SYMREF_MOD.with(|m| m.set(Some(#i)));
                        let __r = __expr(it, float);
                        __SYMREF_MOD.with(|m| m.set(None));
                        __r
                    };
                    if let Some(__inner) = __inner {
                        if #post_match {
                            if let Some(__v) = #fname(__inner) { return Some(__v); }
                        }
                    }
                }
                it.pos = __msave;
            }
        });
    }
    let helper = quote! {
        /// 立即数修饰的**字面**匹配（`ci` = Ident token 豁免大小写）。
        fn __imm_fn_lit(it: &mut __Iter, lit: &[__Tok], ci: bool) -> bool {
            let mut p = it.pos;
            for t in lit {
                match (it.toks.get(p), t) {
                    (Some(__Tok::Ident(x)), __Tok::Ident(y)) => {
                        let ok = if ci { x.eq_ignore_ascii_case(y) } else { x == y };
                        if !ok { return false; }
                    }
                    (Some(x), y) => { if x != y { return false; } }
                    (None, _) => return false,
                }
                p += 1;
            }
            it.pos = p;
            true
        }
    };
    Ok((quote! { #helper #(#fns)* }, quote! { #(#tries)* }))
}

/// 一个**字面段**的匹配表达式（布尔；匹配则消费 token 并推进 `it.pos`）。
/// 首段（`idx == 0`）在 `mnemonic_case = "insensitive"` 时对 Ident token 豁免大小写：
/// 单 token 字面走 `__eat_name`，多 token 字面走 `__eat_lit_ci`（如 `lock cmpxchg [`）。
/// **其余字面段严格按 asm 格式匹配**（`__eat_lit`）。
///
/// 只说"段"不说"助记符"：v17 起 asm 模板可以**操作数前置**（首段不是字面），
/// 所以"首段是不是指令名"这件事既不能假设、也不能提取（见 `gen_scan_probe`）。
fn lit_seg_expr(l: &str, idx: usize, case_insensitive: bool) -> Result<TokenStream, String> {
    let toks = tokenize(l).map_err(|e| format!("asm template literal '{l}': {e}"))?;
    let leading_name = if idx == 0 && case_insensitive && toks.len() == 1 {
        match &toks[0] {
            Tok::Ident(s) => Some(s.as_str()),
            _ => None,
        }
    } else {
        None
    };
    Ok(match leading_name {
        Some(name) => {
            let name_lit = syn::LitStr::new(name, proc_macro2::Span::call_site());
            quote! { __eat_name(&mut it, #name_lit) }
        }
        None if idx == 0 && case_insensitive => {
            let exprs: Vec<_> = toks.iter().map(tok_expr).collect();
            quote! { __eat_lit_ci(&mut it, &[#(#exprs),*]) }
        }
        None => {
            let exprs: Vec<_> = toks.iter().map(tok_expr).collect();
            quote! { __eat_lit(&mut it, &[#(#exprs),*]) }
        }
    })
}

/// 线性扫描探针（`could_be_instruction`）：**只跑扫描的第一步**——本 ISA 的候选里
/// 有没有哪条的**首段**能在这段文本上匹配。
///
/// **为什么不能用"助记符表"**（v20 V9 首版的做法，已废弃）：那等于在生成期假设
/// `asm` 模板的**首个空白分隔词就是助记符**，再拿它当"本 ISA 有哪些指令"的清单。
/// 三条都不成立——① v17 起模板可以操作数前置（首段是 `{0}`，首"词"是操作数）；
/// ② 首段可以是多 token 字面（`lock cmpxchg [`），拆词就错；③ 空白分词与扫描器的
/// token 切分不是一回事（`amoadd.w.aqrl` 是一个 Ident）。
///
/// 现在改成**问扫描器自己**：探针与 `__assemble` 用同一份 `infos`、同一个
/// [`lit_seg_expr`]、同一个 lexer，问的只是"第一步走不走得动"。它是**必要条件**
/// 而非充分条件：`false` ⇒ 这段文本一定不是本 ISA 的指令；`true` ⇒ 首段对得上，
/// 后面还得看整条匹配。首段是操作数（或模板为空）的候选**不构成约束**，直接算 `true`。
fn gen_scan_probe(infos: &[InstInfo], case_insensitive: bool) -> Result<TokenStream, String> {
    let mut probes: Vec<TokenStream> = Vec::new();
    for info in infos {
        let segs = parse_template(&info.inst.asm)?;
        let first_lit = match segs.first() {
            Some(Seg::Lit(l)) => Some(l.clone()),
            _ => None,
        };
        probes.push(match first_lit {
            Some(l) => {
                let e = lit_seg_expr(&l, 0, case_insensitive)?;
                quote! {
                    {
                        let mut it = __Iter { toks: &__toks, pos: 0 };
                        if #e { return true; }
                    }
                }
            }
            // 首段是操作数占位符（或模板为空）：任何文本都可能从这里开始。
            None => quote! { return true; },
        });
    }
    Ok(quote! {
        /// 线性扫描探针（**诊断用**）：这段文本能不能作为本 ISA 某条候选指令的扫描起点。
        ///
        /// 只判**首段**，是必要性判断：`false` ⇒ 一定不是本 ISA 的指令（本 ISA 没有
        /// 以这段开头对应的指令）；`true` ⇒ 首段对得上，**不代表整条能汇编**。
        /// 不假设"首词 = 助记符"（见生成器侧 `gen_scan_probe`），与 `assemble`/
        /// `parse_insts` 用同一份候选与同一个 lexer。
        pub fn could_be_instruction(text: &str) -> bool {
            let Ok(__toks) = __lex(text) else { return false; };
            #(#probes)*
            false
        }
    })
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
    let op = info.form.opsize.clone();
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
            // 一致性检查对 opsize=Slot/Max 的多宽 GPR form（宽度由操作数推导）
            // ——`max` 只放宽 IR 降级的混宽编码，汇编文本仍要求同宽
            // （`cmp RAX, EBX` 拒绝）。
            let need_cls = matches!(op, Some(Opsize::Slot(_) | Opsize::Max)) && gpr_only;
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
            // `wrap` 打开时用**接受**值域收字面量，再规范化回 `signed` 的读数
            // （`0x90909090` → -1869574000）：编码/解码/渲染仍用规范值域。
            let (min, max) = slot.imm_accept_range().unwrap_or((i64::MIN, i64::MAX));
            let float = slot.float == Some(true);
            let unit = proc_macro2::Literal::i64_suffixed(slot.unit());
            let allow_sym = slot.symbols.unwrap_or(false);
            let elem = match (slot.imm_wrap_modulus(), slot.imm_range()) {
                (Some(m), Some((clo, chi))) => quote! {
                    {
                        let __w = __imm(&mut it, #min, #max, #float, #unit, &mut __lsyms, #n, #allow_sym);
                        __w.map(|v| {
                            if v > #chi { v.wrapping_sub(#m) } else { v }
                        })
                        .map(|v| if v < #clo { v.wrapping_add(#m) } else { v })
                    }
                },
                _ => {
                    quote! { __imm(&mut it, #min, #max, #float, #unit, &mut __lsyms, #n, #allow_sym) }
                }
            };
            Ok((elem, quote! { Some(#fid) }, None))
        }
        OperandKind::Label => {
            let (min, max) = slot.imm_range().unwrap_or((i64::MIN, i64::MAX));
            let unit = proc_macro2::Literal::i64_suffixed(slot.unit());
            // 符号写进候选局部的 `__lsyms`（非共享 `__syms`）：元组求值是急切的，
            // 前导 `__eat_name` 失败时 `__label` 仍会被调用；若写入共享缓冲会污染
            // 后续候选的匹配结果（S10c 平铺扫描后暴露）。局部缓冲仅在整条 asm
            // 完全命中时随 `return` 提交。
            let elem = quote! { __label(&mut it, #min, #max, &mut __lsyms, #n, #unit) };
            Ok((elem, quote! { Some(#fid) }, None))
        }
        OperandKind::Bits => {
            // 命名位集合：源文本是表里若干名字的**拼接**（贪心最长匹配，整串吃干净才算命中）。
            let pairs: Vec<TokenStream> = slot
                .table_entries
                .iter()
                .map(|(n, b)| {
                    let n = syn::LitStr::new(n, proc_macro2::Span::call_site());
                    let b = proc_macro2::Literal::u64_unsuffixed(*b);
                    quote! { (#n, #b) }
                })
                .collect();
            Ok((
                quote! { __bitset(&mut it, &[#(#pairs),*]) },
                quote! { Some(#fid) },
                None,
            ))
        }
        OperandKind::Mem => Ok((quote! { __mem(&mut it) }, quote! { Some(#fid) }, None)),
        OperandKind::Cond => Ok((quote! { __cond(&mut it) }, quote! { Some(#fid) }, None)),
    }
}

/// 单条指令的 assemble 尝试：对**整条 asm 模板**（前导字面 + 操作数）从左到右
/// 逐段消费 token（字面段 = token 序列匹配；占位符段 = 按槽类型解析）。任一失败
/// 静默回退下一形状（多形状回退语义）。前导字面段（助记符位）按 `case_insensitive`
/// 豁免大小写（`__eat_name`），其余字面段严格遵守 asm 格式（`__eat_lit`）。
fn gen_assemble_try_tok(info: &InstInfo, case_insensitive: bool) -> Result<TokenStream, String> {
    let vn = &info.vn;
    let segs = parse_template(&info.inst.asm)?;
    validate_segs(&segs, info)?;
    let mut elems: Vec<TokenStream> = Vec::new();
    let mut pats: Vec<TokenStream> = Vec::new();
    let mut cls_binds: Vec<syn::Ident> = Vec::new();
    for (idx, seg) in segs.iter().enumerate() {
        match seg {
            Seg::Lit(l) => {
                // 首段（助记符位）在 case_insensitive 下豁免大小写，其余字面段严格匹配；
                // 判定与扫描探针 `could_be_instruction` 共用 `lit_seg_expr`。
                let e = lit_seg_expr(l, idx, case_insensitive)?;
                elems.push(quote! { #e.then_some(()) });
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
    // opsize=Slot/Max 多宽 form：全部 GPR 操作数宽度一致性（`mov rax, ebx` → 拒绝）
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
            // 每候选独立的符号缓冲：失败即弃（不污染共享 __syms），整条 asm
            // 完全命中时才随 return 提交。见 S10c 平铺扫描后 label 候选被误试的修复。
            let mut __lsyms: Vec<(usize, Option<usize>, String)> = Vec::new();
            let mut it = __Iter { toks: &__toks, pos: 0 };
            if let (#(#pats),*) = (#(#elems),*) {
                if #consistency {
                    return Ok((#ctor, __lsyms));
                }
            }
        }
    })
}

/// form 特异性：候选**能接受的输入越少越先试**（数值越小越靠前）。多 form 同模板时靠它分发。
///
/// - 寄存器槽 = 可接受的**类数**（无 class = 任意类，按 64 计，最不具体）；
/// - 立即数/标签槽 = 可接受的**取值个数**（`imm8s` 的 257 个值 vs `imm32` 的 2^32+1）；
///   用 [`crate::dsl::model::OperandSlot::imm_accept_range`]——`wrap` 槽多收另一种读数，
///   属"能接受多少输入"（规范值域只影响解码/渲染/边界，见 `imm_range`）。
/// - 内存/条件码槽按 1 计（不参与）。
///
/// 立即数也计入是 v20 V9 修的**真缺陷**：x86 的 `83 /n ib`（符号扩展 imm8）比
/// `81 /n id` 更具体，但它的目的槽是多宽度 `gprx`（3 类）而旧的 `ADD64_R_IMM32` 是
/// 单类 `gpr8`——"只看寄存器类数"会把后者排前面，于是 `add rax, -12` 编成 7 字节的
/// imm32 形式，而规范/上游用 4 字节的符号扩展 imm8 形式（字节 oracle 抓到的）。
/// 两类都计入"能接受多少输入"后，窄值域的形式自然先试，放不进窄值域的立即数照旧落回宽形式。
fn form_specificity(info: &InstInfo, _model: &IsaModel) -> u64 {
    let mut spec = 1u64;
    for (_, _, slot, _) in &info.operands {
        let n = match slot.kind {
            OperandKind::Reg => slot.classes().map(|c| c.len() as u64).unwrap_or(64).max(1),
            OperandKind::Imm | OperandKind::Label => {
                let (lo, hi) = slot.imm_accept_range().unwrap_or((i64::MIN, i64::MAX));
                (hi as i128 - lo as i128 + 1).clamp(1, u64::MAX as i128) as u64
            }
            _ => 1,
        };
        spec = spec.saturating_mul(n);
    }
    spec
}

/// 类型签名：整条 asm 的字面 token 序列 + 每操作数槽签名（真重复检测）。
///
/// 立即数槽的签名带上**值域**（`I{lo}:{hi}`）：x86 的 `83 /n ib`（符号扩展 imm8）与
/// `81 /n id`（imm32）汇编文本一模一样，但**能接受的立即数范围不同**——它们不是"真重复"，
/// 去重掉一条会让放不进 imm8 的立即数没候选可匹配（实测：`xor rax, 12` 走短形式，
/// `xor rax, 0x12345` 必须还能落回 imm32）。带值域后两条都留在候选里，按声明序先试窄的。
fn type_signature(info: &InstInfo) -> Result<String, String> {
    let segs = parse_template(&info.inst.asm)?;
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
                    OperandKind::Imm => {
                        let (lo, hi) = slot.imm_accept_range().unwrap_or((i64::MIN, i64::MAX));
                        format!("I{lo}:{hi}")
                    }
                    OperandKind::Label => "L".to_string(),
                    OperandKind::Mem => "M".to_string(),
                    OperandKind::Cond => "C".to_string(),
                    // 命名位集合：签名带上表（不同表的名字空间不同，不能互相当候选）
                    OperandKind::Bits => format!("S{}", slot.table.as_deref().unwrap_or("")),
                };
                s.push_str(&format!("O{n}:{sig}"));
            }
        }
    }
    Ok(s)
}

/// assemble 入口：`pub(crate)` 由 `codegen::generate()` 调用。
///
/// v17 起**不再分派助记符**：按声明序把每条指令的**整条 asm 模板**（前导字面 +
/// 操作数）从左到右逐 token 匹配，首个完整命中即停。候选全局按 `form_specificity`
/// 排序（窄约束先，声明序稳定）并做真重复去重（`type_signature`）——同助记符多
/// 形状的自动分发语义保留，只是不再经过 `match 助记符` 这道前置分派。
/// **符号引用回填**的生成（`[[operand_slots]].symbols` + 标签槽）。
///
/// 两遍布局的产出：`(指令下标, 操作数下标, 修饰下标, 符号名)`——符号名 = 标签名（或 `.` 的
/// 自引用 = 空名）、修饰下标 = `[[conventions.imm_fn]]` 的下标（`%hi(foo)` 的 `foo` 要连
/// 修饰一起回填：值 = `hi(标签的块下标)`）。
///
/// 生成两件事：
/// 1. `__set_symbol_operand`：按 `(指令, 操作数)` 写值（label/imm 槽直接写字段）；
/// 2. `__apply_imm_fn`：按修饰下标把值过一遍修饰（没有 `imm_fn` 的谱不发这个函数）。
fn gen_symbol_setter(infos: &[InstInfo], model: &IsaModel) -> TokenStream {
    let mut arms: Vec<TokenStream> = Vec::new();
    for info in infos {
        let vn = &info.vn;
        for (n, (_, fid, s, _)) in info.operands.iter().enumerate() {
            // **两个 kind 的准入条件不同，不能合并判断**——这正是这里出过回归的地方：
            // 早先把「imm 槽要 `symbols`」与「label 槽天然是符号」写成一个条件，
            // 顺手对两者一起跳过 `unit > 1`，于是 A64 的 `b L1`（label 槽 + `unit = 4`）
            // 不再生成回填臂，语料红桶 0 → 25（棘轮抓到）。两条规则分别写：
            // - label：天然可回填，`unit` 由下面的乘法负责；
            // - imm：要显式声明 `symbols`（能力是声明的），且校验期已保证 `unit == 1`
            //   （`unit != 1` 的 `symbols` 是歧义，直接拒——见 validate.rs），故此处不再判。
            let ok = match s.kind {
                OperandKind::Label => true,
                OperandKind::Imm => s.symbols.unwrap_or(false),
                _ => false,
            };
            if !ok {
                continue;
            }
            let unit = s.unit();
            let arm = if unit > 1 {
                // 标签槽的**源单位**（A64 分支偏移按字节）：块下标 × unit 写进 Inst 字段，
                // 编码期再除回字段单位。
                let u = proc_macro2::Literal::i64_suffixed(unit);
                quote! { (Inst::#vn { #fid, .. }, #n) => { *#fid = val * #u; true } }
            } else {
                quote! { (Inst::#vn { #fid, .. }, #n) => { *#fid = val; true } }
            };
            arms.push(arm);
        }
    }
    let n_fns = model.conventions.imm_fn.as_ref().map_or(0, |d| d.len());
    // **无条件发射**：回填路径总是调用它（没有修饰的谱只剩 `_ => val` 那一条臂）。
    let apply = {
        let cases: Vec<TokenStream> = (0..n_fns)
            .map(|i| {
                let f = format_ident!("__imm_fn{i}");
                quote! { #i => #f(val).unwrap_or(val), }
            })
            .collect();
        quote! {
            /// 按 `[[conventions.imm_fn]]` 的下标把回填值过一遍修饰
            /// （`%hi(foo)` 这类：修饰作用于**标签的块下标**，与源文本里的写法同义）。
            fn __apply_imm_fn(idx: usize, val: i64) -> i64 {
                match idx {
                    #(#cases)*
                    _ => val,
                }
            }
        }
    };
    quote! {
        /// 回填（两遍布局的第二遍）：把符号的值写进对应操作数。
        fn __set_symbol_operand(inst: &mut Inst, idx: usize, val: i64) -> bool {
            match (inst, idx) {
                #(#arms,)*
                _ => false,
            }
        }

        #apply
    }
}
pub(crate) fn gen_assemble(infos: &[InstInfo], model: &IsaModel) -> Result<TokenStream, String> {
    let lexer = gen_lexer_ts(model)?;
    let primitives = gen_asm_primitives(model, infos)?;
    let case_insensitive = matches!(model.meta.mnemonic_case, MnemonicCase::Insensitive);
    let scan_probe = gen_scan_probe(infos, case_insensitive)?;
    // 全局特异性排序（窄约束先，声明序稳定）
    let mut cands: Vec<&InstInfo> = infos.iter().collect();
    cands.sort_by_key(|info| form_specificity(info, model));
    // 真重复（同字面骨架 + 同类型签名）跳过，保留首个声明
    let mut seen: Vec<String> = Vec::new();
    let mut tries: Vec<TokenStream> = Vec::new();
    for info in cands {
        let sig = type_signature(info)?;
        if seen.contains(&sig) {
            continue;
        }
        seen.push(sig);
        tries.push(gen_assemble_try_tok(info, case_insensitive)?);
    }
    // `[[pseudo]]`（v18 S3e）：单条 API 只接受展开成 1 条的伪指令；多条明确报错，
    // 让调用方改用 `parse_insts`（整段汇编）。
    let pseudo_single: TokenStream = if model.pseudo.is_empty() {
        quote! {}
    } else {
        quote! {
            {
                let mut __plines: Vec<String> = Vec::new();
                if __pseudo_expand(text, 0, &mut __plines)? {
                    if __plines.len() != 1 {
                        return Err(format!(
                            "伪指令展开为 {} 条指令——单条 assemble() 装不下，\
                             请用 TargetAssembler::parse_insts（整段汇编）",
                            __plines.len()
                        ));
                    }
                    let (inst, syms) = __assemble(&__plines[0])?;
                    if !syms.is_empty() {
                        return Err(
                            "label operands require TargetAssembler::parse_insts (two-pass layout)"
                                .into(),
                        );
                    }
                    return Ok(inst);
                }
            }
        }
    };
    Ok(quote! {
        #lexer
        #primitives
        #scan_probe
        /// 前导字面（助记符位）大小写豁免匹配（`__eat_name`）。
        #[allow(dead_code)]
        fn __eat_name(it: &mut __Iter, name: &str) -> bool {
            if matches!(it.toks.get(it.pos), Some(__Tok::Ident(x)) if x.eq_ignore_ascii_case(name)) {
                it.pos += 1;
                true
            } else {
                false
            }
        }
        /// 前导多 token 字面大小写豁免匹配：Ident token 用 `eq_ignore_ascii_case`，
        /// 标点/数值 token 严格相等（LOCK 前缀族 `lock cmpxchg [` 等）。
        #[allow(dead_code)]
        fn __eat_lit_ci(it: &mut __Iter, lit: &[__Tok]) -> bool {
            let mut p = it.pos;
            for t in lit {
                match (it.toks.get(p), t) {
                    (Some(__Tok::Ident(x)), __Tok::Ident(y)) => {
                        if !x.eq_ignore_ascii_case(y.as_str()) {
                            return false;
                        }
                    }
                    (Some(x), y) => {
                        if x != y {
                            return false;
                        }
                    }
                    (None, _) => return false,
                }
                p += 1;
            }
            it.pos = p;
            true
        }
        /// 汇编单条文本 → 指令（token 驱动；左→右整模板扫描，多 form 按类型签名自动分发）。
        ///
        /// `[[pseudo]]`（v18 S3e）：只有**展开成 1 条指令**的伪指令能走这个 API
        /// （多条展开要用 `TargetAssembler::parse_insts`——单条 API 装不下多条）。
        pub fn assemble(text: &str) -> Result<Inst, String> {
            #pseudo_single
            let (inst, syms) = __assemble(text)?;
            if syms.is_empty() {
                Ok(inst)
            } else {
                Err("label operands require TargetAssembler::parse_insts (two-pass layout)".into())
            }
        }
        /// 内部装配：返回 (指令, 未解析符号引用列表 (操作数序号, 符号名))。
        pub(crate) fn __assemble(text: &str) -> Result<(Inst, Vec<(usize, Option<usize>, String)>), String> {
            let __toks = __lex(text)?;
            #(#tries)*
            Err("no matching instruction".into())
        }
    })
}
