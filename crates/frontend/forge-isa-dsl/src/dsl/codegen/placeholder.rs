//! 占位符表达式引擎（v28 M1）。
//!
//! **设计目标**：新增占位符只写谱内数据，不改本文件。本文件只保留：
//!
//! 1. **原语源**（固定的机器/ABI 事实）：符号寄存器、临时寄存器、帧偏移、
//!    常量池、立即数、条件码、终结块、返回槽、sret；
//! 2. **原语函数**（纯整数/位表达式）：`bits/sext/zext/round/sat_sub/shufps/
//!    vconst_*`；
//! 3. **表达式解析/求值**：字面量、`+ - * / & | ^ << >>`、一元 `- ~`、括号；
//! 4. 由表达式派生的 kind / 临时寄存器 / xreg / `mentions`（sret/ret/…）。
//!
//! 谱里这样定义（`[placeholder.<名>]`）：
//!
//! ```toml
//! [placeholder.iconst_hi20]
//! expr = "(iconst + 0x800) >> 12 & 0xFFFFF"
//! ```
//!
//! 旧 66 个占位符名由 [`default_placeholders.toml`](default_placeholders.toml)
//! 以同一语法提供（数据，不是代码）；谱内同名定义可覆盖（但不得覆盖原语名）。
//! 表达式只能引用已知原语/定义；未知名字、循环定义、把 Mem/Cond 参与算术都会
//! 在生成期报错，**绝不回退字面量 0**。

use super::super::model::{IsaModel, OperandKind, OperandSlot};
use super::field_ctor_expr;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::{BTreeMap, BTreeSet};

/// 槽类型约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhKind {
    Reg,
    Imm,
    Cond,
    /// 内存操作数（占位符自己产 `MemRef`，如 `{sret}`）。
    Mem,
}

/// 临时寄存器类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhTemp {
    Gpr,
    Fpr,
}

/// 一个占位符在生成期被引用的“副作用”（用来决定是否要预生成 `__ret_dst` 等局部）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mentions {
    pub sret: bool,
    pub ret: bool,
    pub retf: bool,
    pub retn: bool,
    pub cond: bool,
}

impl Mentions {
    fn merge(&mut self, o: Mentions) {
        self.sret |= o.sret;
        self.ret |= o.ret;
        self.retf |= o.retf;
        self.retn |= o.retn;
        self.cond |= o.cond;
    }
}

/// 原语源（固定集合）。**新增 ISA 不应往这里加东西**：先看能否用现有原语组合。
#[derive(Clone, Copy)]
struct Prim {
    name: &'static str,
    kind: PhKind,
    loose: bool,
    token_kind: &'static str,
    temp: Option<&'static str>,
    temp_class: Option<PhTemp>,
    xreg: &'static str,
    ctor: fn(&OperandSlot) -> TokenStream,
    mentions: Mentions,
}

const NONE: Mentions = Mentions {
    sret: false,
    ret: false,
    retf: false,
    retn: false,
    cond: false,
};
const RET: Mentions = Mentions {
    sret: false,
    ret: true,
    retf: false,
    retn: false,
    cond: false,
};
const RETF: Mentions = Mentions {
    sret: false,
    ret: false,
    retf: true,
    retn: false,
    cond: false,
};
const RETN: Mentions = Mentions {
    sret: false,
    ret: false,
    retf: false,
    retn: true,
    cond: false,
};
const SRET: Mentions = Mentions {
    sret: true,
    ret: false,
    retf: false,
    retn: false,
    cond: false,
};
const COND: Mentions = Mentions {
    sret: false,
    ret: false,
    retf: false,
    retn: false,
    cond: true,
};

fn sym_ctor(slot: &OperandSlot) -> TokenStream {
    field_ctor_expr(slot, quote! { 0u32 })
}

/// 生成期没有真实槽（校验/临时收集/token 分类）时给一个占位 ctor。
fn field_ctor(slot: Option<&OperandSlot>) -> TokenStream {
    slot.map(|s| field_ctor_expr(s, quote! { 0u32 }))
        .unwrap_or_else(|| quote! { 0u32 })
}

/// 原语表。`name` 不带花括号。
fn primitives() -> &'static [Prim] {
    static PRIMS: &[Prim] = &[
        // ── 符号化寄存器 ──
        Prim {
            name: "out",
            kind: PhKind::Reg,
            loose: true,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "rd",
            ctor: sym_ctor,
            mentions: NONE,
        },
        Prim {
            name: "out2",
            kind: PhKind::Reg,
            loose: true,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "rd2",
            ctor: sym_ctor,
            mentions: NONE,
        },
        Prim {
            name: "g",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: Some("__g"),
            temp_class: Some(PhTemp::Gpr),
            xreg: "__g",
            ctor: sym_ctor,
            mentions: NONE,
        },
        Prim {
            name: "f",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: Some("__f"),
            temp_class: Some(PhTemp::Fpr),
            xreg: "__f",
            ctor: sym_ctor,
            mentions: NONE,
        },
        Prim {
            name: "cond",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "__cond",
            ctor: sym_ctor,
            mentions: COND,
        },
        // ── 返回槽 / sret ──
        Prim {
            name: "ret",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| quote! { __ret_dst },
            mentions: RET,
        },
        Prim {
            name: "ret_src",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "__ret_src",
            ctor: sym_ctor,
            mentions: RET,
        },
        Prim {
            name: "retf",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| quote! { __retf_dst },
            mentions: RETF,
        },
        Prim {
            name: "retf_src",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "__retf_src",
            ctor: sym_ctor,
            mentions: RETF,
        },
        Prim {
            name: "retN",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| quote! { __retN },
            mentions: RETN,
        },
        Prim {
            name: "retN_src",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "__retN_src",
            ctor: sym_ctor,
            mentions: RETN,
        },
        Prim {
            name: "sret",
            kind: PhKind::Mem,
            loose: false,
            token_kind: "mem",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| {
                quote! {
                    MemRef {
                        base: Some(__sret),
                        disp: 0,
                        index: None,
                        scale: 1,
                    }
                }
            },
            mentions: SRET,
        },
        // ── 帧/地址意图 ──
        Prim {
            name: "off",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| quote! { ctx.current_offset as i64 },
            mentions: NONE,
        },
        Prim {
            name: "alloca",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| quote! { ctx.current_alloca_offset as i64 },
            mentions: NONE,
        },
        Prim {
            name: "global",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| quote! { ctx.current_global.map(|g| -(g.index() as i64) - 1).unwrap_or(0) },
            mentions: NONE,
        },
        // ── 常量池 ──
        Prim {
            name: "iconst",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| {
                quote! {
                    ctx.constant_pool.as_ref()
                        .and_then(|p| p.as_i64(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0)
                }
            },
            mentions: NONE,
        },
        Prim {
            name: "fconst",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| {
                quote! {
                    ctx.constant_pool.as_ref()
                        .and_then(|p| p.as_f64_bits(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0) as i64
                }
            },
            mentions: NONE,
        },
        Prim {
            name: "imm0",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| quote! { ctx.current_immediates.first().copied().unwrap_or(0) as i64 },
            mentions: NONE,
        },
        // ── 条件码 / 终结块 ──
        Prim {
            name: "cc",
            kind: PhKind::Cond,
            loose: false,
            token_kind: "cond",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| quote! { __cc },
            mentions: NONE,
        },
        Prim {
            name: "cc_inv",
            kind: PhKind::Cond,
            loose: false,
            token_kind: "cond",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| quote! { __cc_inv },
            mentions: NONE,
        },
        Prim {
            name: "true",
            kind: PhKind::Cond,
            loose: false,
            token_kind: "label",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| quote! { __true_block },
            mentions: NONE,
        },
        Prim {
            name: "false",
            kind: PhKind::Cond,
            loose: false,
            token_kind: "label",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| quote! { __false_block },
            mentions: NONE,
        },
        Prim {
            name: "target",
            kind: PhKind::Cond,
            loose: false,
            token_kind: "label",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: |_| quote! { __target_block },
            mentions: NONE,
        },
    ];
    PRIMS
}

/// 一个已解析的占位符（表达式求值产物）。
#[derive(Clone)]
pub(crate) struct Resolved {
    pub ctor: TokenStream,
    pub kind: PhKind,
    pub loose: bool,
    pub token_kind: &'static str,
    pub xreg: TokenStream,
    pub temps: Vec<(String, PhTemp)>,
    pub mentions: Mentions,
}

/// 占位符表：原语 + 谱内定义 + 内置旧名别名。
pub(crate) struct PlaceholderTable {
    defs: BTreeMap<String, Expr>,
}

impl PlaceholderTable {
    /// 只含内置旧名（测试/无谱校验用）。
    pub fn default_table() -> Self {
        let text = include_str!("default_placeholders.toml");
        #[derive(serde::Deserialize)]
        struct File {
            #[serde(default)]
            placeholder: BTreeMap<String, super::super::model::PlaceholderDecl>,
        }
        let f: File = toml::from_str(text).expect("default_placeholders.toml 必须是合法 TOML");
        let mut defs = BTreeMap::new();
        for (name, d) in f.placeholder {
            let expr = parse_expr(&d.expr)
                .unwrap_or_else(|e| panic!("default_placeholders.toml `{name}`: {e}"));
            defs.insert(name, expr);
        }
        Self { defs }
    }

    /// 谱内定义 + 内置旧名。谱定义可覆盖旧名，但**不得覆盖原语名**。
    pub fn from_model(model: &IsaModel) -> Result<Self, String> {
        let mut t = Self::default_table();
        for (name, d) in &model.placeholder_decls {
            if primitives().iter().any(|p| p.name == name) {
                return Err(format!(
                    "[placeholder.{name}]: 名字与原语源冲突（原语名不能重定义）"
                ));
            }
            let expr =
                parse_expr(&d.expr).map_err(|e| format!("[placeholder.{name}].expr: {e}"))?;
            t.defs.insert(name.clone(), expr);
        }
        assert_unique(&t)?;
        Ok(t)
    }

    fn def(&self, name: &str) -> Option<&Expr> {
        self.defs.get(name)
    }

    /// 解析一个模板 token（`{...}` 或裸名字）。
    pub fn resolve(&self, token: &str, slot: Option<&OperandSlot>) -> Result<Resolved, String> {
        let body = token
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or(token)
            .trim();
        if body.is_empty() {
            return Err("空占位符 `{}`".to_string());
        }
        // 精确名字优先（旧名/谱定义/原语/编号操作数/编号临时）。
        if let Some(r) = self.resolve_name(body, slot, &mut Vec::new())? {
            return Ok(r);
        }
        // 否则按表达式解析（`imm0 - 4`、`off >> 4 & 0xF`）。
        let expr = parse_expr(body)
            .map_err(|e| format!("未知占位符 '{token}'（也不是合法表达式：{e}）"))?;
        let node = self.eval(&expr, slot, &mut Vec::new())?;
        Ok(node.into_resolved())
    }

    fn resolve_name(
        &self,
        name: &str,
        slot: Option<&OperandSlot>,
        stack: &mut Vec<String>,
    ) -> Result<Option<Resolved>, String> {
        // {N} 编号操作数
        if numbered_operand(&format!("{{{name}}}")).is_some() {
            let xreg = numbered_xreg(name)?;
            return Ok(Some(Resolved {
                ctor: field_ctor(slot),
                kind: PhKind::Reg,
                loose: true,
                token_kind: "reg",
                xreg,
                temps: Vec::new(),
                mentions: NONE,
            }));
        }
        // {gN}/{fN} 编号临时
        if let Some((var, cls)) = numbered_temp(&format!("{{{name}}}")) {
            let id = format_ident!("{var}");
            return Ok(Some(Resolved {
                ctor: field_ctor(slot),
                kind: PhKind::Reg,
                loose: false,
                token_kind: "reg",
                xreg: quote! { #id },
                temps: vec![(var, cls)],
                mentions: NONE,
            }));
        }
        if let Some(p) = primitives().iter().find(|p| p.name == name) {
            let xreg = if p.xreg == "0u32" {
                quote! { 0u32 }
            } else {
                let id = format_ident!("{}", p.xreg);
                quote! { #id }
            };
            let temps = p
                .temp
                .map(|t| vec![(t.to_string(), p.temp_class.unwrap_or(PhTemp::Gpr))])
                .unwrap_or_default();
            return Ok(Some(Resolved {
                ctor: slot.map(|s| (p.ctor)(s)).unwrap_or_else(|| quote! { 0u32 }),
                kind: p.kind,
                loose: p.loose,
                token_kind: p.token_kind,
                xreg,
                temps,
                mentions: p.mentions,
            }));
        }
        if let Some(expr) = self.def(name) {
            if stack.iter().any(|s| s == name) {
                return Err(format!("占位符定义成环：{} → {name}", stack.join(" → ")));
            }
            stack.push(name.to_string());
            let node = self.eval(expr, slot, stack)?;
            stack.pop();
            return Ok(Some(node.into_resolved()));
        }
        Ok(None)
    }

    fn eval(
        &self,
        expr: &Expr,
        slot: Option<&OperandSlot>,
        stack: &mut Vec<String>,
    ) -> Result<Node, String> {
        match expr {
            Expr::Int(v) => Ok(Node::imm(quote! { (#v as i64) })),
            Expr::Ident(name) => match self.resolve_name(name, slot, stack)? {
                Some(r) => Ok(Node::from_resolved(r)),
                None => Err(format!("表达式引用了未知名字 `{name}`")),
            },
            Expr::Unary(op, inner) => {
                let a = self.eval(inner, slot, stack)?;
                a.require_imm("一元运算")?;
                let at = &a.tokens;
                let tk = match op {
                    '-' => quote! { - },
                    '~' => quote! { ! },
                    other => return Err(format!("未知一元运算符 `{other}`")),
                };
                Ok(Node {
                    tokens: quote! { (#tk (#at)) },
                    ..Node::imm(quote! { 0i64 })
                })
            }
            Expr::Bin(op, l, r) => {
                let a = self.eval(l, slot, stack)?;
                let b = self.eval(r, slot, stack)?;
                a.require_imm("二元运算")?;
                b.require_imm("二元运算")?;
                let at = &a.tokens;
                let bt = &b.tokens;
                let tk = match op.as_str() {
                    "+" => quote! { + },
                    "-" => quote! { - },
                    "*" => quote! { * },
                    "/" => quote! { / },
                    "&" => quote! { & },
                    "|" => quote! { | },
                    "^" => quote! { ^ },
                    "<<" => quote! { << },
                    ">>" => quote! { >> },
                    other => return Err(format!("未知二元运算符 `{other}`")),
                };
                let mut n = Node::imm(quote! { ((#at) #tk (#bt)) });
                n.kind = a.kind;
                n.token_kind = a.token_kind;
                n.loose = a.loose;
                n.temps = a.temps;
                n.temps.extend(b.temps);
                n.mentions.merge(a.mentions);
                n.mentions.merge(b.mentions);
                Ok(n)
            }
            Expr::Call(name, args) => self.eval_call(name, args, slot, stack),
        }
    }

    fn eval_call(
        &self,
        name: &str,
        args: &[Expr],
        slot: Option<&OperandSlot>,
        stack: &mut Vec<String>,
    ) -> Result<Node, String> {
        let arity = |n: usize| {
            if args.len() == n {
                Ok(())
            } else {
                Err(format!("`{name}` 需要 {n} 个参数，给了 {}", args.len()))
            }
        };
        let konst = |e: &Expr| -> Result<i64, String> {
            match e {
                Expr::Int(v) => Ok(*v),
                _ => Err(format!("`{name}` 的参数必须是整数字面量")),
            }
        };
        match name {
            "arg" => {
                arity(1)?;
                let n = konst(&args[0])?;
                if !(0..64).contains(&n) {
                    return Err(format!("arg({n}) 越界（0..64）"));
                }
                let idx = n as usize;
                let rs = format_ident!("rs{}", idx + 1);
                Ok(Node {
                    tokens: field_ctor(slot),
                    xreg: Some(quote! { #rs }),
                    ..Node::reg()
                })
            }
            "bits" => {
                arity(3)?;
                let v = self.eval(&args[0], slot, stack)?;
                v.require_imm("bits")?;
                let shift = konst(&args[1])?;
                let width = konst(&args[2])?;
                if !(0..=63).contains(&shift) || !(1..=64).contains(&width) {
                    return Err(format!(
                        "bits 的 shift/width 越界：shift={shift}, width={width}"
                    ));
                }
                let mask: u64 = if width == 64 {
                    u64::MAX
                } else {
                    (1u64 << width) - 1
                };
                let vt = &v.tokens;
                Ok(Node {
                    tokens: quote! { ((((#vt) as u64) >> #shift) & #mask) as i64 },
                    ..Node::imm(quote! { 0i64 })
                })
            }
            "sext" => {
                arity(2)?;
                let v = self.eval(&args[0], slot, stack)?;
                v.require_imm("sext")?;
                let width = konst(&args[1])?;
                if !(1..=63).contains(&width) {
                    return Err(format!("sext 的 width 越界：{width}"));
                }
                let sh = 64 - width;
                let vt = &v.tokens;
                Ok(Node {
                    tokens: quote! { ((((#vt) << #sh) >> #sh)) },
                    ..Node::imm(quote! { 0i64 })
                })
            }
            "zext" => {
                arity(2)?;
                let v = self.eval(&args[0], slot, stack)?;
                v.require_imm("zext")?;
                let width = konst(&args[1])?;
                if !(1..=63).contains(&width) {
                    return Err(format!("zext 的 width 越界：{width}"));
                }
                let mask: u64 = (1u64 << width) - 1;
                let vt = &v.tokens;
                Ok(Node {
                    tokens: quote! { (((#vt) as u64) & #mask) as i64 },
                    ..Node::imm(quote! { 0i64 })
                })
            }
            "round" => {
                arity(2)?;
                let v = self.eval(&args[0], slot, stack)?;
                v.require_imm("round")?;
                let shift = konst(&args[1])?;
                if !(1..=63).contains(&shift) {
                    return Err(format!("round 的 shift 越界：{shift}"));
                }
                let half = 1i64 << (shift - 1);
                let vt = &v.tokens;
                Ok(Node {
                    tokens: quote! { (((#vt) + #half) >> #shift) },
                    ..Node::imm(quote! { 0i64 })
                })
            }
            "sat_sub" => {
                arity(2)?;
                let a = self.eval(&args[0], slot, stack)?;
                let b = self.eval(&args[1], slot, stack)?;
                a.require_imm("sat_sub")?;
                b.require_imm("sat_sub")?;
                let at = &a.tokens;
                let bt = &b.tokens;
                Ok(Node {
                    tokens: quote! { (#at).saturating_sub(#bt) },
                    ..Node::imm(quote! { 0i64 })
                })
            }
            "shufps" => {
                arity(1)?;
                let off = konst(&args[0])?;
                if !(0..=63).contains(&off) {
                    return Err(format!("shufps 的 offset 越界：{off}"));
                }
                let off = off as usize;
                Ok(Node {
                    tokens: quote! { __shufps_imm8(ctx.current_immediates.as_slice(), #off) },
                    ..Node::imm(quote! { 0i64 })
                })
            }
            "vconst_raw64" => {
                arity(0)?;
                Ok(Node {
                    tokens: quote! {
                        __vconst_raw64(ctx.constant_pool.as_ref(), ctx.current_const_index)
                    },
                    ..Node::imm(quote! { 0i64 })
                })
            }
            "vconst_raw_half" => {
                arity(1)?;
                let hi = konst(&args[0])?;
                let hi = hi != 0;
                Ok(Node {
                    tokens: quote! {
                        __vconst_raw_half(ctx.constant_pool.as_ref(), ctx.current_const_index, #hi)
                    },
                    ..Node::imm(quote! { 0i64 })
                })
            }
            "vconst_half" => {
                arity(2)?;
                let half = konst(&args[0])?;
                let hi = konst(&args[1])?;
                if !(0..=3).contains(&half) || (hi != 0 && hi != 1) {
                    return Err(format!("vconst_half 参数越界：half={half}, hi={hi}"));
                }
                let hi = hi != 0;
                Ok(Node {
                    tokens: quote! {
                        __vconst_half(
                            ctx.constant_pool.as_ref(),
                            ctx.current_const_index,
                            #half as usize,
                            #hi,
                            __vconst_elem_bits(ctx, results) == 64,
                        )
                    },
                    ..Node::imm(quote! { 0i64 })
                })
            }
            "vconst_elem_bits" => {
                arity(0)?;
                Ok(Node {
                    tokens: quote! { __vconst_elem_bits(ctx, results) as i64 },
                    ..Node::imm(quote! { 0i64 })
                })
            }
            "mem" => {
                arity(1)?;
                let base = self.eval(&args[0], slot, stack)?;
                if base.kind != PhKind::Reg {
                    return Err("mem(base) 的 base 必须是寄存器表达式".into());
                }
                let bt = &base.tokens;
                Ok(Node {
                    tokens: quote! {
                        MemRef { base: Some(#bt), disp: 0, index: None, scale: 1 }
                    },
                    kind: PhKind::Mem,
                    token_kind: "mem",
                    ..Node::imm(quote! { 0i64 })
                })
            }
            "g" | "f" => {
                arity(1)?;
                let n = konst(&args[0])?;
                if n <= 0 {
                    return Err(format!("{name}({n})：编号必须 ≥ 1"));
                }
                let cls = if name == "g" {
                    PhTemp::Gpr
                } else {
                    PhTemp::Fpr
                };
                let var = format!("__{name}{n}");
                let id = format_ident!("{var}");
                Ok(Node {
                    tokens: field_ctor(slot),
                    kind: PhKind::Reg,
                    token_kind: "reg",
                    xreg: Some(quote! { #id }),
                    temps: vec![(var, cls)],
                    ..Node::reg()
                })
            }
            other => Err(format!("未知占位符函数 `{other}()`")),
        }
    }
}

#[derive(Clone)]
struct Node {
    tokens: TokenStream,
    kind: PhKind,
    loose: bool,
    token_kind: &'static str,
    xreg: Option<TokenStream>,
    temps: Vec<(String, PhTemp)>,
    mentions: Mentions,
}

impl Node {
    fn imm(tokens: TokenStream) -> Self {
        Self {
            tokens,
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            xreg: None,
            temps: Vec::new(),
            mentions: NONE,
        }
    }
    fn reg() -> Self {
        Self {
            tokens: quote! { 0u32 },
            kind: PhKind::Reg,
            loose: true,
            token_kind: "reg",
            xreg: None,
            temps: Vec::new(),
            mentions: NONE,
        }
    }
    fn from_resolved(r: Resolved) -> Self {
        Self {
            tokens: r.ctor,
            kind: r.kind,
            loose: r.loose,
            token_kind: r.token_kind,
            xreg: if r.xreg.is_empty() {
                None
            } else {
                Some(r.xreg)
            },
            temps: r.temps,
            mentions: r.mentions,
        }
    }
    fn into_resolved(self) -> Resolved {
        Resolved {
            ctor: self.tokens,
            kind: self.kind,
            loose: self.loose,
            token_kind: self.token_kind,
            xreg: self.xreg.unwrap_or_else(|| quote! { 0u32 }),
            temps: self.temps,
            mentions: self.mentions,
        }
    }
    fn require_imm(&self, what: &str) -> Result<(), String> {
        if self.kind == PhKind::Imm {
            Ok(())
        } else {
            Err(format!(
                "{what} 的操作数必须是立即数/整数表达式（这里是 {:?}）",
                self.kind
            ))
        }
    }
}

// ─────────────────────────── 表达式解析 ───────────────────────────

#[derive(Clone, Debug)]
enum Expr {
    Int(i64),
    Ident(String),
    Call(String, Vec<Expr>),
    Unary(char, Box<Expr>),
    Bin(String, Box<Expr>, Box<Expr>),
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

fn parse_expr(s: &str) -> Result<Expr, String> {
    let mut p = Parser {
        src: s.as_bytes(),
        pos: 0,
    };
    let e = p.parse_bin(0)?;
    p.ws();
    if p.pos != p.src.len() {
        return Err(format!("表达式结尾有多余字符：`{}`", &s[p.pos..]));
    }
    Ok(e)
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.src.get(self.pos).copied()
    }
    fn eat(&mut self, b: u8) -> bool {
        self.ws();
        if self.src.get(self.pos) == Some(&b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    /// 运算符优先级：| 1, ^ 2, & 3, << >> 4, + - 5, * / 6。
    fn parse_bin(&mut self, min_prec: u8) -> Result<Expr, String> {
        let mut lhs = self.parse_unary()?;
        loop {
            self.ws();
            let (op, prec) = match self.operator() {
                Some(v) => v,
                None => break,
            };
            if prec < min_prec {
                break;
            }
            // 只有确定要消费这个运算符时才推进位置（否则低优先级运算符会被
            // RHS 的递归层提前吃掉，表达式结尾留下 `0xFFFFF`）。
            self.pos += op.len();
            let next = prec + 1;
            let rhs = self.parse_bin(next)?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn operator(&mut self) -> Option<(String, u8)> {
        let rest = &self.src[self.pos..];
        for (s, prec) in [
            ("<<", 4u8),
            (">>", 4),
            ("+", 5),
            ("-", 5),
            ("*", 6),
            ("/", 6),
            ("&", 3),
            ("|", 1),
            ("^", 2),
        ] {
            if rest.starts_with(s.as_bytes()) {
                return Some((s.to_string(), prec));
            }
        }
        None
    }
    fn parse_unary(&mut self) -> Result<Expr, String> {
        if self.eat(b'-') {
            return Ok(Expr::Unary('-', Box::new(self.parse_unary()?)));
        }
        if self.eat(b'~') {
            return Ok(Expr::Unary('~', Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }
    fn parse_primary(&mut self) -> Result<Expr, String> {
        if self.eat(b'(') {
            let e = self.parse_bin(0)?;
            if !self.eat(b')') {
                return Err("缺少 `)`".into());
            }
            return Ok(e);
        }
        let c = self.peek().ok_or("表达式意外结束")?;
        if c.is_ascii_digit() {
            return self.parse_int();
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let name = self.parse_ident();
            if self.eat(b'(') {
                let mut args = Vec::new();
                if !self.eat(b')') {
                    loop {
                        args.push(self.parse_bin(0)?);
                        if self.eat(b')') {
                            break;
                        }
                        if !self.eat(b',') {
                            return Err(format!("`{name}(…)` 参数之间缺逗号"));
                        }
                    }
                }
                return Ok(Expr::Call(name, args));
            }
            return Ok(Expr::Ident(name));
        }
        Err(format!("无法解析的字符 `{}`", c as char))
    }
    fn parse_int(&mut self) -> Result<Expr, String> {
        self.ws();
        let start = self.pos;
        if self.src[self.pos..].starts_with(b"0x") || self.src[self.pos..].starts_with(b"0X") {
            self.pos += 2;
            let h = self.pos;
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_hexdigit() {
                self.pos += 1;
            }
            let s = std::str::from_utf8(&self.src[h..self.pos]).unwrap();
            let v = i64::from_str_radix(s, 16).map_err(|e| format!("十六进制字面量：{e}"))?;
            return Ok(Expr::Int(v));
        }
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        let s = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        Ok(Expr::Int(
            s.parse().map_err(|e| format!("整数字面量：{e}"))?,
        ))
    }
    fn parse_ident(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.src.len()
            && (self.src[self.pos].is_ascii_alphanumeric() || self.src[self.pos] == b'_')
        {
            self.pos += 1;
        }
        String::from_utf8_lossy(&self.src[start..self.pos]).into_owned()
    }
}

// ─────────────────────────── 查询 API ───────────────────────────

/// 编号临时 `{gN}`/`{fN}`（N ≥ 1）。
pub(crate) fn numbered_temp(name: &str) -> Option<(String, PhTemp)> {
    let body = name.strip_prefix('{').and_then(|r| r.strip_suffix('}'))?;
    let (cls, digits) = match body.strip_prefix('g') {
        Some(d) => (PhTemp::Gpr, d),
        None => (PhTemp::Fpr, body.strip_prefix('f')?),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    if n == 0 {
        return None;
    }
    let var = if cls == PhTemp::Gpr {
        format!("__g{n}")
    } else {
        format!("__f{n}")
    };
    Some((var, cls))
}

/// 编号操作数 `{N}`（N ≥ 0），任意多操作数。
pub(crate) fn numbered_operand(name: &str) -> Option<usize> {
    let digits = name.strip_prefix('{').and_then(|r| r.strip_suffix('}'))?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: usize = digits.parse().ok()?;
    (n < 64).then_some(n)
}

fn numbered_xreg(name: &str) -> Result<TokenStream, String> {
    let n = numbered_operand(&format!("{{{name}}}"))
        .ok_or_else(|| format!("`{name}` 不是编号操作数"))?;
    let id = format_ident!("rs{}", n + 1);
    Ok(quote! { #id })
}

/// 形状签名（token → reg/imm/cond/num/mem）；未知 token 走旧 fallback。
pub(crate) fn token_kind(table: &PlaceholderTable, op: &str) -> &'static str {
    // 裸整数字面量保持旧语义 "num"（既能匹配 imm 槽，也能匹配 cond 槽——x86
    // `CMOVCC {out}, {1}, 5` 的第三操作数是条件码挖槽）。
    if super::integration::parse_i64_lit(op).is_ok() {
        return "num";
    }
    if let Ok(r) = table.resolve(op, None) {
        return r.token_kind;
    }
    let inner = op
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(op);
    // 物理寄存器（RAX 等全大写/数字）
    if op.starts_with('{')
        && op.ends_with('}')
        && inner
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return "reg";
    }
    // 条件枚举字面量（IntCC::Ne / FloatCC::Oeq / FlagCC::O）
    if let Some((en, _)) = inner.split_once("::")
        && matches!(en, "IntCC" | "FloatCC" | "FlagCC")
    {
        return "cond";
    }
    if super::integration::parse_i64_lit(inner).is_ok() {
        return "num";
    }
    if inner.contains('+') || inner.contains('-') || inner.contains('(') || inner.contains(')') {
        return "mem";
    }
    "reg"
}

/// 模板中出现的临时声明（去重）。
pub(crate) fn collect_temps(
    table: &PlaceholderTable,
    templates: &[String],
) -> Vec<(String, PhTemp)> {
    let mut out: Vec<(String, PhTemp)> = Vec::new();
    for t in templates {
        for tok in brace_tokens(t) {
            if let Ok(r) = table.resolve(tok, None) {
                for (var, cls) in r.temps {
                    if !out.iter().any(|(v, _)| *v == var) {
                        out.push((var, cls));
                    }
                }
            }
        }
    }
    out
}

/// 显式临时声明校验。
pub(crate) fn explicit_temps(
    table: &PlaceholderTable,
    decl: &[String],
    templates: &[String],
) -> Result<Vec<(String, PhTemp)>, String> {
    let mut out: Vec<(String, PhTemp)> = Vec::new();
    for d in decl {
        let (name, cls) = d
            .split_once(':')
            .ok_or_else(|| format!("temps: `{d}` 缺类别（写法如 `g1:gpr` / `f2:fpr`）"))?;
        let temp_class = match cls.trim() {
            "gpr" => PhTemp::Gpr,
            "fpr" => PhTemp::Fpr,
            other => {
                return Err(format!(
                    "temps: `{d}` 的类别 `{other}` 不认识（只认 gpr/fpr）"
                ));
            }
        };
        let tok = format!("{{{}}}", name.trim());
        let var = numbered_temp(&tok)
            .map(|(v, _)| v)
            .ok_or_else(|| format!("temps: `{d}` 不是合法临时名（要 `{{gN}}`/`{{fN}}` 形式）"))?;
        if !out.iter().any(|(v, _)| *v == var) {
            out.push((var, temp_class));
        }
    }
    for (var, _) in collect_temps(table, templates) {
        if !out.iter().any(|(v, _)| *v == var) {
            let shown = var.trim_start_matches("__").to_string();
            return Err(format!(
                "temps: `insts` 用了 `{{{shown}}}`，但它不在本规则的 `temps` 里（声明了 temps 就必须显式列出）"
            ));
        }
    }
    Ok(out)
}

/// 提取模板里的 `{...}` token。
fn brace_tokens(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && let Some(end) = s[i + 1..].find('}')
        {
            let end = i + 1 + end;
            out.push(&s[i..=end]);
            i = end + 1;
            continue;
        }
        i += 1;
    }
    out
}

/// ctor 构建：解析成功即返回表达式。
pub(crate) fn ctor_for(
    table: &PlaceholderTable,
    name: &str,
    slot: &OperandSlot,
) -> Option<TokenStream> {
    table.resolve(name, Some(slot)).ok().map(|r| r.ctor)
}

/// xreg 绑定表达式（符号寄存器/临时；复合表达式返回 `0u32`）。
pub(crate) fn xreg_for(table: &PlaceholderTable, name: &str) -> TokenStream {
    table
        .resolve(name, None)
        .map(|r| r.xreg)
        .unwrap_or_else(|_| quote! { 0u32 })
}

/// 槽类型约束检查。
pub(crate) fn kind_matches(table: &PlaceholderTable, name: &str, slot_kind: OperandKind) -> bool {
    // 裸整数字面量不是占位符（否则 `MOVZW {g1}, 0` 会被当成 Imm 占位符，
    // 生成 0u32 而不是 i64 字面量）。
    if super::integration::parse_i64_lit(name).is_ok() {
        return false;
    }
    // 终结层块号符号值可落 Imm/Label（各 ISA 的槽种类不同）。
    let bare = name.trim();
    if matches!(
        bare,
        "{true}" | "{false}" | "{target}" | "true" | "false" | "target"
    ) {
        return matches!(slot_kind, OperandKind::Imm | OperandKind::Label);
    }
    let Ok(r) = table.resolve(name, None) else {
        return false;
    };
    if r.loose {
        return true;
    }
    match r.kind {
        PhKind::Reg => slot_kind == OperandKind::Reg,
        PhKind::Mem => slot_kind == OperandKind::Mem,
        PhKind::Imm => slot_kind == OperandKind::Imm,
        PhKind::Cond => slot_kind == OperandKind::Cond,
    }
}

/// 该 `{…}` token 是否是已知占位符（校验期用）。
pub(crate) fn is_known(table: &PlaceholderTable, name: &str) -> bool {
    table.resolve(name, None).is_ok()
}

/// 模板里是否用到 sret / ret / retf / retN / cond（供 lowering 预声明局部）。
pub(crate) fn mentions(table: &PlaceholderTable, templates: &[String]) -> Mentions {
    let mut m = NONE;
    for t in templates {
        for tok in brace_tokens(t) {
            if let Ok(r) = table.resolve(tok, None) {
                m.merge(r.mentions);
            }
        }
    }
    m
}

/// 唯一性断言（测试用）：谱定义不能重名/成环；原语名不可被覆盖。
pub(crate) fn assert_unique(table: &PlaceholderTable) -> Result<(), String> {
    let names: BTreeSet<&String> = table.defs.keys().collect();
    for name in names {
        if primitives().iter().any(|p| p.name == name.as_str()) {
            return Err(format!("谱定义覆盖了原语名：{name}"));
        }
    }
    // 所有定义都必须能解析（成环/未知名字在这里被抓住）。
    for name in table.defs.keys() {
        table
            .resolve(&format!("{{{name}}}"), None)
            .map_err(|e| format!("占位符 `{name}`：{e}"))?;
    }
    Ok(())
}
