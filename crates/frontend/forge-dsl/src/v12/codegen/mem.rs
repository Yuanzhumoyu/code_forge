//! 内存操作数文本模板（v16/S9）。
//!
//! `[conventions.mem] template = "..."` 用组件占位符描述内存操作数的文本形态，
//! 从同一份模板派生两个生成函数：
//!
//! - `__render_mem`（反汇编渲染）：按模板序输出；可选组件缺省值不输出。
//! - `__mem`（汇编解析）：按模板序左→右走 token；组件按类型消费，可选组件用
//!   "尝试 + 回滚" 跳过。
//!
//! 组件：`{base}`（必需 Reg）、`{index}`（可选 Reg）、`{scale}`（可选，乘数
//! 1/2/4/8）、`{disp}`（可选，有符号 i64）。一个紧随在可选组件之前的字面 token
//! 是该组件的"条件前缀"：仅当组件存在时发出/消费（`{disp}` 的 `+` 前缀额外对
//! 非正 disp 抑制——`-8` 不重复出 `+-`）。紧随在必需组件之前、或位于末尾的字面
//! token 恒发出。

use proc_macro2::TokenStream;
use quote::quote;

use super::super::model::MemTemplate;

/// x86 缺省模板（不写 `[conventions.mem]` 时等价）。
pub(crate) const DEFAULT_MEM_TEMPLATE: &str = "[{base}+{index}*{scale}+{disp}]";

/// 取生效模板字符串：未显式声明 → 缺省 x86 模板。
pub(crate) fn effective_template(conventions: &Option<MemTemplate>) -> String {
    conventions
        .as_ref()
        .map(|m| m.template.clone())
        .unwrap_or_else(|| DEFAULT_MEM_TEMPLATE.to_string())
}

/// 组件占位符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Comp {
    Base,
    Index,
    Scale,
    Disp,
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
                other => {
                    return Err(format!(
                        "内存模板未知组件占位符 {{{other}}}（合法：base/index/scale/disp）"
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

/// 派生 `__mem`（汇编解析）与按需的 `__raw_signed_int` 辅助。
pub(crate) fn gen_mem_parser(items: &[Item]) -> TokenStream {
    let mut stmts: Vec<TokenStream> = Vec::new();
    let mut has_signed_disp = false;
    let mut i = 0;
    while i < items.len() {
        let (prefix_toks, comp) = match &items[i] {
            Item::Lit { toks, .. } => match items.get(i + 1) {
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
    let signed_helper = if has_signed_disp {
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
    quote! {
        fn __mem(it: &mut __Iter) -> Option<MemRef> {
            let save = it.pos;
            let mut index: Option<Reg> = None;
            let mut scale: u8 = 1;
            let mut disp: i64 = 0;
            #(#stmts)*
            // scale 必须伴随 index（模板校验亦强制 `{scale}` 紧随 `{index}`）。
            if scale != 1 && index.is_none() { it.pos = save; return None; }
            Some(MemRef { base, disp, index, scale })
        }
        #signed_helper
    }
}
