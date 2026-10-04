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
//! 组件：`{base}`（可选 Reg）、`{index}`（可选 Reg）、`{scale}`（可选，乘数
//! 1/2/4/8）、`{disp}`（可选，有符号 i64）、`{size}`（可选，尺寸关键字）。
//!
//! **基址的有无由模板形状声明**（v20 V10）：模板里**写了 `{base}`** ⇒ 这条写法
//! 必须有基址（缺了整条模板不匹配，与"必须有 `[`"同级）；**没写 `{base}`** ⇒ 它是
//! **无基址**写法（`base = None`，x86 `mov rax, [0x1234]` 这类绝对地址），且此时
//! 其余组件一律按**必需**处理——没有基址时，位移/索引本身就是地址，不是可省的附加项
//! （否则 `[0]` 会渲染成 `]`）。这样"要不要基址"只有一处声明（模板本身），既不需要
//! 新键，也没有第二个开关可与之矛盾。
//!
//! 有基址的模板里，紧跟可选组件的字面 token 是该组件的"条件前缀"：仅当组件存在时
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

/// 模板结构校验（v20 V10）：
///
/// - **最多一个 `{base}`**（两个没有意义，第二个只会覆盖第一个）；
/// - `{scale}` 必须紧随 `{index}` 之后；
/// - **不写 `{base}` = 无基址写法**（绝对地址）⇒ 模板必须有别的**地址组件**
///   （`{disp}` 或 `{index}`），否则它只能匹配空文本（`[]`），是作者漏写而非意图。
pub(crate) fn validate_mem_template(items: &[Item]) -> Result<(), String> {
    let mut n_base = 0usize;
    let mut n_addr = 0usize;
    let mut last_comp: Option<Comp> = None;
    for it in items {
        if let Item::Comp(c) = it {
            match c {
                Comp::Base => n_base += 1,
                Comp::Index | Comp::Disp => n_addr += 1,
                Comp::Scale if last_comp != Some(Comp::Index) => {
                    return Err("`{scale}` 必须跟在 `{index}` 之后".into());
                }
                _ => {}
            }
            last_comp = Some(*c);
        }
    }
    if n_base > 1 {
        return Err("模板最多只能有一个 `{base}`".into());
    }
    if n_base == 0 && n_addr == 0 {
        return Err(
            "无基址模板（没写 `{base}`）必须含 `{disp}` 或 `{index}`——空模板只能匹配 `[]`".into(),
        );
    }
    Ok(())
}

/// 单 token `+` 判定：决定 disp 前缀是否做"非正抑制"。
fn is_single_plus(toks: &[LitTok]) -> bool {
    toks == [LitTok::Plus]
}

/// 派生 `__render_mem`（反汇编渲染）。
///
/// `required` = 本模板**没有** `{base}`（无基址写法，见模块文档）：此时其余组件
/// 按必需处理（前缀与值都恒发出，含 `{disp} == 0`），否则保持"可选组件缺省不输出"。
pub(crate) fn gen_render_mem(items: &[Item]) -> TokenStream {
    let required = !has_base(items);
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
        // 需要的组件里，前缀与值都恒发出；可选的组件里，前缀只在值存在时发出。
        let hard = required && !matches!(comp.unwrap(), Comp::Base | Comp::Size);
        let p = prefix_text.map(|p| quote! { s.push_str(#p); });
        match comp.unwrap() {
            Comp::Base => {
                if let Some(t) = prefix_text {
                    stmts.push(quote! { s.push_str(#t); });
                }
                // `base: None` = 无基址写法（本模板没写 `{base}`，走不到这里）。
                stmts.push(quote! {
                    if let Some(b) = m.base {
                        s.push_str(<&'static str as From<Reg>>::from(b));
                    }
                });
            }
            Comp::Index if hard => stmts.push(quote! {
                #p
                if let Some(idx) = m.index {
                    s.push_str(<&'static str as From<Reg>>::from(idx));
                }
            }),
            Comp::Index => stmts.push(quote! {
                if let Some(idx) = &m.index {
                    let iname = <&'static str as From<Reg>>::from(*idx);
                    #p
                    s.push_str(&iname);
                }
            }),
            Comp::Scale if hard => stmts.push(quote! {
                #p
                s.push_str(&format!("{}", m.scale));
            }),
            Comp::Scale => stmts.push(quote! {
                if m.index.is_some() && m.scale != 1 {
                    #p
                    s.push_str(&format!("{}", m.scale));
                }
            }),
            // 必需（无基址写法）⇒ 位移就是**地址本身**，0 也要印出来。
            Comp::Disp if hard => stmts.push(quote! {
                #p
                s.push_str(&format!("{}", m.disp));
            }),
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
            let mut s = String::new();
            #(#stmts)*
            s
        }
    }
}

/// 模板里是否写了 `{base}`（= 这条写法**要求**基址；不写 = 无基址绝对寻址）。
pub(crate) fn has_base(items: &[Item]) -> bool {
    items.iter().any(|it| matches!(it, Item::Comp(Comp::Base)))
}

/// 派生 `__mem`（汇编解析）与按需的 `__eat_size` 辅助。
///
/// 每个生效模板各派生一个 `__mem_try<k>`；`__mem` 按列表序依次试，**第一条整条
/// 走通的赢**（失败的那条把自己回滚干净）。多条模板 = 同一份 `MemRef` 的多种合法
/// 文本写法（Intel `[base+disp]` / GAS `disp[base]` / 带尺寸前缀），不是"多个指令"。
///
/// 组件里的值（`{disp}`/`{scale}`）复用生成物的表达式求值器：位移读 `__expr`
/// （数字/符号常量/算术），scale 仍是 `1|2|4|8` 字面量。
pub(crate) fn gen_mem_parser(
    templates: &[Vec<Item>],
    size_kws: &[Vec<LitTok>],
    case_insensitive: bool,
) -> TokenStream {
    let mut tries: Vec<TokenStream> = Vec::new();
    let mut names: Vec<syn::Ident> = Vec::new();
    let mut any_size = false;
    for (k, items) in templates.iter().enumerate() {
        let (f, size) = gen_one_mem_try(items, size_kws, case_insensitive, k);
        tries.push(f);
        names.push(format_ident!("__mem_try{k}"));
        any_size |= size;
    }
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
        /// 地址寄存器：内存的基址/索引必须是**地址类**寄存器（`__ADDR_CLASSES`：
        /// 缺省地址类 + 地址尺寸覆盖类，x86 = 64 位与 32 位 GPR）。
        ///
        /// 32 位地址寄存器（`[eax]`）在 64 位模式下要发 `0x67` 地址尺寸前缀——那条
        /// 由**编码器**按基址/索引的实际宽度发（见 `vlen.rs` 的地址尺寸覆盖），这里
        /// 只保证不是地址类的寄存器（`[al]`、`[xmm0]`）进不来。
        fn __addr_reg(it: &mut __Iter) -> Option<Reg> {
            __reg_f(it, __ADDR_CLASSES, None).map(|(r, _)| r)
        }
        #size_helper
    }
}

/// 单条模板的解析函数体 + 它需要的辅助（返回值 = 是否需要 `__eat_size`）。
///
/// `required = !has_base(items)`（无基址写法）：其余组件一律必需——没有基址时，
/// 位移/索引就是地址本身，`[]` 这种"什么都没写"的形态必须解析失败。
fn gen_one_mem_try(
    items: &[Item],
    size_kws: &[Vec<LitTok>],
    case_insensitive: bool,
    k: usize,
) -> (TokenStream, bool) {
    let fname = format_ident!("__mem_try{k}");
    let required = !has_base(items);
    let mut stmts: Vec<TokenStream> = Vec::new();
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
        // 必需组件（无基址模板里的非 Base 组件）与"必需基址"走同一套硬失败分支。
        let hard = required && !matches!(comp.unwrap(), Comp::Base | Comp::Size);
        let eat_prefix = prefix_toks.map(|t| {
            let e = toks_expr(t);
            quote! { if !__eat_lit(it, &[#(#e),*]) { it.pos = save; return None; } }
        });
        match comp.unwrap() {
            Comp::Base => {
                if let Some(s) = &eat_prefix {
                    stmts.push(s.clone());
                }
                // 模板写了 `{base}` ⇒ 基址必需（缺了整条不匹配），所以这里是 `Some`。
                stmts.push(quote! {
                    base = Some(match __addr_reg(it) {
                        Some(r) => r,
                        None => { it.pos = save; return None; }
                    });
                });
            }
            Comp::Index if hard => {
                if let Some(s) = &eat_prefix {
                    stmts.push(s.clone());
                }
                stmts.push(quote! {
                    index = Some(match __addr_reg(it) {
                        Some(r) => r,
                        None => { it.pos = save; return None; }
                    });
                });
            }
            Comp::Index => match prefix_toks {
                Some(t) => {
                    let e = toks_expr(t);
                    stmts.push(quote! {
                        {
                            let p = it.pos;
                            if __eat_lit(it, &[#(#e),*]) {
                                if let Some(r) = __addr_reg(it) {
                                    index = Some(r);
                                } else {
                                    it.pos = p;
                                }
                            }
                        }
                    });
                }
                None => stmts.push(quote! {
                    if let Some(r) = __addr_reg(it) { index = Some(r); }
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
                if hard {
                    if let Some(s) = &eat_prefix {
                        stmts.push(s.clone());
                    }
                    stmts.push(inner);
                } else {
                    match prefix_toks {
                        Some(t) => {
                            let e = toks_expr(t);
                            stmts.push(quote! { if __eat_lit(it, &[#(#e),*]) { #inner } });
                        }
                        None => stmts.push(inner),
                    }
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
            Comp::Disp => {
                // 位移与立即数**共用同一套值语法**（`__expr`）：数字、一元 `+`/`-`、`~`、
                // 括号、算术、符号常量（`.equ`/`.set`）——`ld x10, 4(x11)` 与
                // `ld x10, SYM(x11)` 因此是同一条路径，不再有"位移只认字面量"的第二套读数。
                //
                // 紧邻 `{disp}` 的**单 `+` 前缀**（Intel `[base+disp]`）**可选**：
                // `[base+8]` / `[base-8]` / `[base]` 都要能解，所以只"有就吃"；其它字面量
                // 前缀（多 token 写法）照旧必需。
                let optional_plus = prefix_toks.is_some_and(is_single_plus);
                let pre = if optional_plus {
                    quote! { let _ = it.eat(&__Tok::Plus); }
                } else {
                    eat_prefix.clone().unwrap_or_default()
                };
                // 前缀可选 ⇒ "有没有值"由模板说了算：**无基址**写法里位移就是地址本身，
                // 必须有值；有基址写法里位移可缺省。前缀必需 ⇒ 前缀匹配上就必须有值。
                let required = hard || (prefix_toks.is_some() && !optional_plus);
                let read = if required {
                    quote! {
                        match __expr(it, false) {
                            Some(v) => disp = v,
                            None => { it.pos = save; return None; }
                        }
                    }
                } else {
                    quote! { if let Some(v) = __expr(it, false) { disp = v; } }
                };
                stmts.push(quote! { #pre #read });
            }
        }
        i += if prefix_toks.is_some() { 2 } else { 1 };
    }
    let f = quote! {
        #[allow(clippy::all)]
        fn #fname(it: &mut __Iter) -> Option<MemRef> {
            let save = it.pos;
            // 模板没写 `{base}` ⇒ 这条写法**没有基址**（绝对地址）。
            let mut base: Option<Reg> = None;
            let mut index: Option<Reg> = None;
            let mut scale: u8 = 1;
            let mut disp: i64 = 0;
            #(#stmts)*
            // 必需基址的模板一定赋过值；无基址模板保持 None。
            let base = base;
            // scale 必须伴随 index（模板校验亦强制 `{scale}` 紧随 `{index}`）。
            if scale != 1 && index.is_none() { it.pos = save; return None; }
            Some(MemRef { base, disp, index, scale })
        }
    };
    let _ = case_insensitive;
    (f, has_size)
}
