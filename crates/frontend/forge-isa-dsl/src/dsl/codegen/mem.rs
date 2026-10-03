//! 内存操作数文本模板（v16/S9；v20 V10 改为**模板列表** + `{size}` 组件）。
//!
//! `[conventions.mem] templates = ["…", "…"]` 用组件占位符描述内存操作数的文本
//! 形态，从同一份列表派生两个生成函数：
//!
//! - `__render_mem`（反汇编渲染）：**只用第 0 条**——渲染必须唯一，否则
//!   `disassemble` 的输出会随解析尝试顺序漂移；
//! - `__mem`（汇编解析）：**按列表序逐条尝试**，第一条整条走通的赢。真实汇编里
//!   同一个操作数的几种合法写法（Intel 的 `[base+disp]` 与 GAS 的 `disp[base]`、
//!   带不带尺寸前缀）就是"同一份数据的多种文本形态"，列表把它们写成并列的几条，
//!   而不是逼作者把几种写法塞进一条模板。
//!
//! 组件：`{base}`（必需 Reg）、`{index}`（可选 Reg）、`{scale}`（可选，乘数
//! 1/2/4/8）、`{disp}`（可选，有符号 i64）、`{size}`（可选，尺寸关键字）。
//! 一个紧随在可选组件之前的字面 token 是该组件的"条件前缀"：仅当组件存在时
//! 发出/消费（`{disp}` 的 `+` 前缀额外对非正 disp 抑制——`-8` 不重复出 `+-`）。
//! 紧随在必需组件之前、或位于末尾的字面 token 恒发出。
//!
//! `{size}`（v20 V10）是**解析专用**组件：它吃掉一个可选的尺寸关键字
//! （关键字表 = `[conventions.mem] size_keywords`，大小写按 `[meta].mnemonic_case`），
//! 值本身不进 `MemRef`——指令的宽度由操作数槽/`opsize` 决定，尺寸前缀只是给
//! 汇编器读的冗余提示（`mov QWORD PTR [rsp-16], rax` 与 `mov [rsp-16], rax` 编出
//! 同样的字节）。渲染时它输出空串，因此 `disassemble → assemble` 仍然闭合；写在
//! `{size}` 紧前面的字面量照常恒发出（渲染时不被当作条件前缀吞掉）。

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::super::model::MemTemplate;

/// x86 缺省模板（不写 `[conventions.mem]` 时等价）。
pub(crate) const DEFAULT_MEM_TEMPLATE: &str = "[{base}+{index}*{scale}+{disp}]";

/// 取生效模板列表：未显式声明（或列表为空）→ 单条缺省 x86 模板。
///
/// 第 0 条 = 渲染形态；其余只在解析时兜底（见模块文档）。
pub(crate) fn effective_templates(conventions: &Option<MemTemplate>) -> Vec<String> {
    match conventions {
        Some(m) if !m.templates.is_empty() => m.templates.clone(),
        _ => vec![DEFAULT_MEM_TEMPLATE.to_string()],
    }
}

/// `{size}` 可接受的关键字（未声明 = 空表 ⇒ 见了 `{size}` 就报错）。
pub(crate) fn size_keywords(conventions: &Option<MemTemplate>) -> Vec<String> {
    conventions
        .as_ref()
        .map(|m| m.size_keywords.clone())
        .unwrap_or_default()
}

/// `size_keywords` → 逐条 token 化（生成期一次做完；与模板字面量同一套词法）。
pub(crate) fn size_keyword_toks(
    conventions: &Option<MemTemplate>,
) -> Result<Vec<Vec<LitTok>>, String> {
    size_keywords(conventions)
        .iter()
        .map(|kw| {
            lex_literal(kw).map_err(|e| format!("[conventions.mem] size_keywords({kw:?}): {e}"))
        })
        .collect()
}

/// 校验一段模板字面量能否被模板词法接受（声明侧校验用）。
pub(crate) fn check_literal(s: &str) -> Result<(), String> {
    lex_literal(s).map(|_| ())
}

/// 组件占位符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Comp {
    Base,
    Index,
    Scale,
    Disp,
    /// 尺寸关键字（解析专用；渲染输出空串）。
    Size,
}

/// 模板字面 token（镜像生成代码里的 `__Tok`，仅取标点 + 标识符子集）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LitTok {
    Ident(String),
    LBracket,
    RBracket,
    LParen,
    RParen,
    Comma,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Dollar,
    Hash,
    Colon,
    Dot,
    Bang,
    Tilde,
    Ampersand,
    Pipe,
    Caret,
    Question,
    Eq,
    Backslash,
}

impl LitTok {
    fn as_tok(&self) -> TokenStream {
        match self {
            LitTok::Ident(s) => {
                let s = s.clone();
                quote! { __Tok::Ident(#s.into()) }
            }
            LitTok::LBracket => quote! { __Tok::LBracket },
            LitTok::RBracket => quote! { __Tok::RBracket },
            LitTok::LParen => quote! { __Tok::LParen },
            LitTok::RParen => quote! { __Tok::RParen },
            LitTok::Comma => quote! { __Tok::Comma },
            LitTok::Plus => quote! { __Tok::Plus },
            LitTok::Minus => quote! { __Tok::Minus },
            LitTok::Star => quote! { __Tok::Star },
            LitTok::Slash => quote! { __Tok::Slash },
            LitTok::Percent => quote! { __Tok::Percent },
            LitTok::Dollar => quote! { __Tok::Dollar },
            LitTok::Hash => quote! { __Tok::Hash },
            LitTok::Colon => quote! { __Tok::Colon },
            LitTok::Dot => quote! { __Tok::Dot },
            LitTok::Bang => quote! { __Tok::Bang },
            LitTok::Tilde => quote! { __Tok::Tilde },
            LitTok::Ampersand => quote! { __Tok::Ampersand },
            LitTok::Pipe => quote! { __Tok::Pipe },
            LitTok::Caret => quote! { __Tok::Caret },
            LitTok::Question => quote! { __Tok::Question },
            LitTok::Eq => quote! { __Tok::Eq },
            LitTok::Backslash => quote! { __Tok::Backslash },
        }
    }
}

/// 模板项：字面（原文 + token 化结果）或组件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Item {
    Lit { text: String, toks: Vec<LitTok> },
    Comp(Comp),
}

/// 解析模板字符串为项序列。`{...}` = 组件占位符，其余为字面标点/标识符/空白。
pub(crate) fn parse_mem_template(s: &str) -> Result<Vec<Item>, String> {
    let mut items: Vec<Item> = Vec::new();
    let mut lit = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if !lit.is_empty() {
                let text = std::mem::take(&mut lit);
                let toks = lex_literal(&text)?;
                items.push(Item::Lit { text, toks });
            }
            let mut name = String::new();
            let mut closed = false;
            for c in chars.by_ref() {
                if c == '}' {
                    closed = true;
                    break;
                }
                name.push(c);
            }
            if !closed {
                return Err(format!("内存模板占位符 `{{{name}` 缺少闭合 `}}`"));
            }
            let comp = match name.as_str() {
                "base" => Comp::Base,
                "index" => Comp::Index,
                "scale" => Comp::Scale,
                "disp" => Comp::Disp,
                "size" => Comp::Size,
                other => {
                    return Err(format!(
                        "内存模板未知组件占位符 {{{other}}}（合法：base/index/scale/disp/size）"
                    ));
                }
            };
            items.push(Item::Comp(comp));
        } else {
            lit.push(c);
        }
    }
    if !lit.is_empty() {
        let text = std::mem::take(&mut lit);
        let toks = lex_literal(&text)?;
        items.push(Item::Lit { text, toks });
    }
    Ok(items)
}

/// 字面子串 → token 序列（镜像生成代码的 `__lex`，忽略空白）。
fn lex_literal(s: &str) -> Result<Vec<LitTok>, String> {
    let mut out = Vec::new();
    let mut cs = s.chars().peekable();
    while let Some(&c) = cs.peek() {
        match c {
            ' ' | '\t' | '\r' | '\n' | '\u{0c}' => {
                cs.next();
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let mut id = String::new();
                while let Some(&c) = cs.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        id.push(c);
                        cs.next();
                    } else {
                        break;
                    }
                }
                out.push(LitTok::Ident(id));
            }
            '[' => {
                cs.next();
                out.push(LitTok::LBracket);
            }
            ']' => {
                cs.next();
                out.push(LitTok::RBracket);
            }
            '(' => {
                cs.next();
                out.push(LitTok::LParen);
            }
            ')' => {
                cs.next();
                out.push(LitTok::RParen);
            }
            ',' => {
                cs.next();
                out.push(LitTok::Comma);
            }
            '+' => {
                cs.next();
                out.push(LitTok::Plus);
            }
            '-' => {
                cs.next();
                out.push(LitTok::Minus);
            }
            '*' => {
                cs.next();
                out.push(LitTok::Star);
            }
            '/' => {
                cs.next();
                out.push(LitTok::Slash);
            }
            '%' => {
                cs.next();
                out.push(LitTok::Percent);
            }
            '$' => {
                cs.next();
                out.push(LitTok::Dollar);
            }
            '#' => {
                cs.next();
                out.push(LitTok::Hash);
            }
            ':' => {
                cs.next();
                out.push(LitTok::Colon);
            }
            '.' => {
                cs.next();
                out.push(LitTok::Dot);
            }
            '!' => {
                cs.next();
                out.push(LitTok::Bang);
            }
            '~' => {
                cs.next();
                out.push(LitTok::Tilde);
            }
            '&' => {
                cs.next();
                out.push(LitTok::Ampersand);
            }
            '|' => {
                cs.next();
                out.push(LitTok::Pipe);
            }
            '^' => {
                cs.next();
                out.push(LitTok::Caret);
            }
            '?' => {
                cs.next();
                out.push(LitTok::Question);
            }
            '=' => {
                cs.next();
                out.push(LitTok::Eq);
            }
            '\\' => {
                cs.next();
                out.push(LitTok::Backslash);
            }
            other => {
                return Err(format!("内存模板字面量不支持字符 '{other}'"));
            }
        }
    }
    Ok(out)
}

fn toks_expr(toks: &[LitTok]) -> Vec<TokenStream> {
    toks.iter().map(LitTok::as_tok).collect()
}

/// 模板结构校验：必须含 `{base}`；`{scale}` 必须紧随 `{index}` 之后。
pub(crate) fn validate_mem_template(items: &[Item]) -> Result<(), String> {
    let mut has_base = false;
    let mut last_comp: Option<Comp> = None;
    for it in items {
        if let Item::Comp(c) = it {
            match c {
                Comp::Base => has_base = true,
                Comp::Scale if last_comp != Some(Comp::Index) => {
                    return Err("`{scale}` 必须跟在 `{index}` 之后".into());
                }
                _ => {}
            }
            last_comp = Some(*c);
        }
    }
    if !has_base {
        return Err("必须包含 `{base}` 组件".into());
    }
    Ok(())
}

/// 单 token `+` 判定：决定 disp 前缀是否做"非正抑制"。
fn is_single_plus(toks: &[LitTok]) -> bool {
    toks == [LitTok::Plus]
}

/// 派生 `__render_mem`（反汇编渲染）。
pub(crate) fn gen_render_mem(items: &[Item]) -> TokenStream {
    let mut stmts: Vec<TokenStream> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let (prefix_text, comp) = match &items[i] {
            Item::Lit { text, .. } => match items.get(i + 1) {
                // 紧随 `{size}` 的字面量**不是**它的条件前缀：`{size}` 渲染输出空串，
                // 字面量照常恒发出（否则 `[` 会随尺寸前缀一起消失，渲染出半个操作数）。
                Some(Item::Comp(Comp::Size)) => {
                    stmts.push(quote! { s.push_str(#text); });
                    i += 1;
                    continue;
                }
                Some(Item::Comp(c)) => (Some(text.as_str()), Some(*c)),
                _ => {
                    // 末尾字面：恒发出
                    stmts.push(quote! { s.push_str(#text); });
                    i += 1;
                    continue;
                }
            },
            Item::Comp(c) => (None, Some(*c)),
        };
        match comp.unwrap() {
            Comp::Base => {
                if let Some(p) = prefix_text {
                    stmts.push(quote! { s.push_str(#p); });
                }
                stmts.push(quote! { s.push_str(&base); });
            }
            Comp::Index => {
                let p = prefix_text.map(|p| quote! { s.push_str(#p); });
                stmts.push(quote! {
                    if let Some(idx) = &m.index {
                        let iname = <&'static str as From<Reg>>::from(*idx);
                        #p
                        s.push_str(&iname);
                    }
                });
            }
            Comp::Scale => {
                let p = prefix_text.map(|p| quote! { s.push_str(#p); });
                stmts.push(quote! {
                    if m.index.is_some() && m.scale != 1 {
                        #p
                        s.push_str(&format!("{}", m.scale));
                    }
                });
            }
            Comp::Disp => match prefix_text {
                None => stmts.push(quote! {
                    if m.disp != 0 { s.push_str(&format!("{}", m.disp)); }
                }),
                Some("+") => {
                    // `+` 前缀对非正 disp 抑制：`-8` 不重复出 `+-`。
                    stmts.push(quote! {
                        if m.disp > 0 { s.push_str("+"); }
                        if m.disp != 0 { s.push_str(&format!("{}", m.disp)); }
                    });
                }
                Some(p) => stmts.push(quote! {
                    if m.disp != 0 {
                        s.push_str(#p);
                        s.push_str(&format!("{}", m.disp));
                    }
                }),
            },
            // `{size}` 解析专用：`MemRef` 不带尺寸，渲染一律不输出。
            Comp::Size => {}
        }
        i += if prefix_text.is_some() { 2 } else { 1 };
    }
    quote! {
        fn __render_mem(m: &MemRef) -> String {
            let base = <&'static str as From<Reg>>::from(m.base);
            let mut s = String::new();
            #(#stmts)*
            s
        }
    }
}

/// 派生 `__mem`（汇编解析）与按需的 `__raw_signed_int` / `__eat_size` 辅助。
///
/// 每个生效模板各派生一个 `__mem_try<k>`；`__mem` 按列表序依次试，**第一条整条
/// 走通的赢**（失败的那条把自己回滚干净）。多条模板 = 同一份 `MemRef` 的多种合法
/// 文本写法（Intel `[base+disp]` / GAS `disp[base]` / 带尺寸前缀），不是"多个指令"。
pub(crate) fn gen_mem_parser(
    templates: &[Vec<Item>],
    size_kws: &[Vec<LitTok>],
    case_insensitive: bool,
) -> TokenStream {
    let mut tries: Vec<TokenStream> = Vec::new();
    let mut names: Vec<syn::Ident> = Vec::new();
    let mut any_signed = false;
    let mut any_size = false;
    for (k, items) in templates.iter().enumerate() {
        let (f, signed, size) = gen_one_mem_try(items, size_kws, case_insensitive, k);
        tries.push(f);
        names.push(format_ident!("__mem_try{k}"));
        any_signed |= signed;
        any_size |= size;
    }
    let signed_helper = if any_signed {
        quote! {
            fn __raw_signed_int(it: &mut __Iter) -> Option<i64> {
                let neg = if it.eat(&__Tok::Minus) {
                    true
                } else if it.eat(&__Tok::Plus) {
                    false
                } else {
                    false
                };
                let v = __raw_int(it)?;
                Some(if neg { -v } else { v })
            }
        }
    } else {
        quote! {}
    };
    let size_helper = if any_size {
        let eat = if case_insensitive {
            quote! { __eat_lit_ci }
        } else {
            quote! { __eat_lit }
        };
        quote! {
            /// `{size}`：吃掉 `size_keywords` 里的任一关键字（大小写按
            /// `[meta].mnemonic_case`）。一个都没匹配 = 尺寸前缀缺席，**不算错**
            ///（`{size}` 是可选组件）——表里全试一遍都不中就原地不动。
            fn __eat_size(it: &mut __Iter, alts: &[&[__Tok]]) -> bool {
                alts.iter().any(|a| #eat(it, a))
            }
        }
    } else {
        quote! {}
    };
    quote! {
        #(#tries)*
        /// 内存操作数解析：按 `[conventions.mem] templates` 的序逐条尝试。
        fn __mem(it: &mut __Iter) -> Option<MemRef> {
            let save = it.pos;
            #(
                if let Some(m) = #names(it) { return Some(m); }
                it.pos = save;
            )*
            None
        }
        #signed_helper
        #size_helper
    }
}

/// 单条模板的解析函数体 + 它需要的辅助（返回值 = 是否需要 `__raw_signed_int` /
/// `__eat_size`）。
fn gen_one_mem_try(
    items: &[Item],
    size_kws: &[Vec<LitTok>],
    case_insensitive: bool,
    k: usize,
) -> (TokenStream, bool, bool) {
    let fname = format_ident!("__mem_try{k}");
    let mut stmts: Vec<TokenStream> = Vec::new();
    let mut has_signed_disp = false;
    let mut has_size = false;
    let mut i = 0;
    while i < items.len() {
        let (prefix_toks, comp) = match &items[i] {
            Item::Lit { toks, .. } => match items.get(i + 1) {
                // 与渲染同一口径：紧邻 `{size}` 的字面量**不是**它的条件前缀，
                // 恒消费（否则 `[` 会随关键字缺席一起消失，`[RAX]` 反而解析不出来）。
                Some(Item::Comp(Comp::Size)) => {
                    let e = toks_expr(toks);
                    stmts.push(quote! {
                        if !__eat_lit(it, &[#(#e),*]) { it.pos = save; return None; }
                    });
                    i += 1;
                    continue;
                }
                Some(Item::Comp(c)) => (Some(toks.as_slice()), Some(*c)),
                _ => {
                    let e = toks_expr(toks);
                    stmts.push(quote! {
                        if !__eat_lit(it, &[#(#e),*]) { it.pos = save; return None; }
                    });
                    i += 1;
                    continue;
                }
            },
            Item::Comp(c) => (None, Some(*c)),
        };
        match comp.unwrap() {
            Comp::Base => {
                if let Some(t) = prefix_toks {
                    let e = toks_expr(t);
                    stmts.push(quote! {
                        if !__eat_lit(it, &[#(#e),*]) { it.pos = save; return None; }
                    });
                }
                stmts.push(quote! {
                    let base = match __reg_cls(it) {
                        Some((r, _)) => r,
                        None => { it.pos = save; return None; }
                    };
                });
            }
            Comp::Index => match prefix_toks {
                Some(t) => {
                    let e = toks_expr(t);
                    stmts.push(quote! {
                        {
                            let p = it.pos;
                            if __eat_lit(it, &[#(#e),*]) {
                                if let Some((r, _)) = __reg_cls(it) {
                                    index = Some(r);
                                } else {
                                    it.pos = p;
                                }
                            }
                        }
                    });
                }
                None => stmts.push(quote! {
                    if let Some((r, _)) = __reg_cls(it) { index = Some(r); }
                }),
            },
            Comp::Scale => {
                let inner = quote! {
                    let s = match it.toks.get(it.pos) {
                        Some(__Tok::Num(v)) if *v == 1 || *v == 2 || *v == 4 || *v == 8 => {
                            it.pos += 1;
                            *v
                        }
                        _ => { it.pos = save; return None; }
                    };
                    scale = s as u8;
                };
                match prefix_toks {
                    Some(t) => {
                        let e = toks_expr(t);
                        stmts.push(quote! { if __eat_lit(it, &[#(#e),*]) { #inner } });
                    }
                    None => stmts.push(inner),
                }
            }
            Comp::Size => {
                has_size = true;
                let alts: Vec<TokenStream> = size_kws
                    .iter()
                    .map(|kw| {
                        let e = toks_expr(kw);
                        quote! { &[#(#e),*][..] }
                    })
                    .collect();
                stmts.push(quote! {
                    {
                        let __alts: &[&[__Tok]] = &[#(#alts),*];
                        let _ = __eat_size(it, __alts);
                    }
                });
            }
            Comp::Disp => match prefix_toks {
                Some(t) if is_single_plus(t) => {
                    stmts.push(quote! {
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
                    });
                }
                Some(t) => {
                    let e = toks_expr(t);
                    stmts.push(quote! {
                        if __eat_lit(it, &[#(#e),*]) {
                            let v = match __raw_signed_int(it) {
                                Some(v) => v,
                                None => { it.pos = save; return None; }
                            };
                            disp = v;
                        }
                    });
                    has_signed_disp = true;
                }
                None => {
                    stmts.push(quote! {
                        if let Some(v) = __raw_signed_int(it) { disp = v; }
                    });
                    has_signed_disp = true;
                }
            },
        }
        i += if prefix_toks.is_some() { 2 } else { 1 };
    }
    let f = quote! {
        #[allow(clippy::all)]
        fn #fname(it: &mut __Iter) -> Option<MemRef> {
            let save = it.pos;
            let mut index: Option<Reg> = None;
            let mut scale: u8 = 1;
            let mut disp: i64 = 0;
            #(#stmts)*
            // scale 必须伴随 index（模板校验亦强制 `{scale}` 紧随 `{index}`）。
            if scale != 1 && index.is_none() { it.pos = save; return None; }
            Some(MemRef { base, disp, index, scale })
        }
    };
    let _ = case_insensitive;
    (f, has_signed_disp, has_size)
}
