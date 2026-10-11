//! 占位符注册表——lowering 模板 `{...}` token 的唯一事实源。
//!
//! 第三轮破坏性重构的核心目标之一：**不能每出现一个框架（ISA）就另写一套
//! 占位符系统**。此前占位符的语义分散在 4 处、手工同步：
//!
//! 1. `lowering::gen_lowering_insts` 的 32+ 个 ctor match arm；
//! 2. `integration::lowering_token_kind` 的分类表（token → reg/imm/cond）；
//! 3. `lowering::gen_lowering` 的临时寄存器预声明清单（{g}/{gN}/{f}/{fN}）；
//! 4. ctor arm 内联的 xreg 绑定（{out}→rd、{g}→__g …）。
//!
//! 现在全部收敛到本注册表：**新增占位符只改 `placeholders()` 一处**，
//! token 分类、临时声明、xreg 绑定、ctor 表达式自动派生。
//!
//! 占位符命名约定（与既有 token 无前缀冲突，精确匹配）：
//! - 符号化寄存器：`{out}`/`{out2}`（结果）、`{N}`（第 N+1 个参数，动态）；
//! - 临时寄存器：`{g}`/`{gN}` = 整数（GPR，变量 `__g`/`__gN`）、
//!   `{f}`/`{fN}` = 浮点（FPR，变量 `__f`/`__fN`）——与 `PhTemp::{Gpr,Fpr}`
//!   一一对应；`{g}` 不与 `{global}` 冲突、`{f}` 不与 `{fconst}` 冲突
//!   （编号临时仅认 `{g`/`{f` + 数字后缀）；
//! - 常量/立即数：`{iconst*}`/`{fconst*}`/`{off}`/`{alloca}`/`{global}`/
//!   `{imm0}`/`{shufps*}`/`{vconst*}`；
//! - 条件码：`{cc}`。
//!
//! 条目字段语义：
//! - `name`：模板中的 token 文本（如 `{out}`）。
//! - `kind`：槽类型（Reg/Imm/Cond）——决定该占位符匹配哪种指令操作数槽。
//! - `token_kind`：形状消歧签名（reg/imm/cond），供 lowering_token_kind。
//! - `temp`：需要预声明的临时 XReg 变量名（`__g`/`__f` 等）；None = 不需要。
//! - `temp_class`：临时寄存器类别（Gpr/Fpr）；None = 无临时。
//! - `xreg`：map_reg_field 绑定的 XReg 表达式（`rd`/`rs1`/`__g`/`0u32`…）。
//! - `ctor`：指令字段 ctor 表达式（占位 0 + 运行期求值），以 `ctx`/arm 内
//!   预绑定变量（rd/rs1/__g…）为自由变量。None = 由调用方 fallback
//!   （字面量/物理寄存器/MemRef）。

use super::super::model::{OperandKind, OperandSlot};
use super::field_ctor_expr;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

/// 槽类型约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhKind {
    Reg,
    Imm,
    Cond,
    /// **内存操作数**（v21 W6 乙）：占位符自己产 `MemRef`（如 `{sret}` = 布局给的隐藏
    /// sret 指针做基址、disp 恒 0 ✗），模板里直接写 `{sret}`（不带方括号 ✗）。
    Mem,
}

/// 临时寄存器类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhTemp {
    Gpr,
    Fpr,
}

/// 单个占位符条目（注册表行）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ph {
    pub name: &'static str,
    pub kind: PhKind,
    /// true = 不约束槽类型（符号寄存器 {out}/{0}…原语义无守卫，任何槽都
    /// 按占位符处理；false = 仅匹配 kind 指定槽）。
    pub loose: bool,
    pub token_kind: &'static str,
    /// 预声明临时变量名（`__g`/`__g1`/`__f`…）；None = 无。
    pub temp: Option<&'static str>,
    /// 临时类别（仅 temp 非 None 时有意义）。
    pub temp_class: Option<PhTemp>,
    /// map_reg_field 绑定的 XReg 表达式文本；`0u32` = 不绑定（物理/立即数）。
    pub xreg: &'static str,
    /// ctor 表达式构建器（None = 调用方 fallback）。
    pub ctor: Option<fn(&OperandSlot) -> TokenStream>,
}

// ─────────────────────────── 注册表 ───────────────────────────

/// 全部占位符（唯一事实源）。新增占位符在此加一行即可。
pub(crate) fn placeholders() -> &'static [Ph] {
    &[
        // ── 符号化寄存器（loose：任何槽都按占位符处理，原语义无守卫）──
        // 注意：`{N}`（{0}/{1}/{2}/…）是**动态编号操作数**，不在此静态表——
        // 见 `numbered_operand`（支持任意多操作数指令，不只 3 个）。
        Ph {
            name: "{out}",
            kind: PhKind::Reg,
            loose: true,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "rd",
            ctor: Some(|s| field_ctor_expr(s, quote! { 0u32 })),
        },
        Ph {
            name: "{out2}",
            kind: PhKind::Reg,
            loose: true,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "rd2",
            ctor: Some(|s| field_ctor_expr(s, quote! { 0u32 })),
        },
        // ── 临时寄存器（模板级预声明）──
        // 命名：`{g}`/`{gN}` = 整数（GPR，变量 __g/__gN）、`{f}`/`{fN}` =
        // 浮点（FPR，变量 __f/__fN）——与 PhTemp::{Gpr,Fpr} 一一对应，且与
        // 其余占位符（{out}/{iconst}/{cc}…）无前缀冲突（{g}≠{global}、
        // {f}≠{fconst}：精确匹配，编号临时仅认 `{g`/`{f` + 数字）。
        Ph {
            name: "{g}",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: Some("__g"),
            temp_class: Some(PhTemp::Gpr),
            xreg: "__g",
            ctor: Some(|s| field_ctor_expr(s, quote! { 0u32 })),
        },
        Ph {
            name: "{f}",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: Some("__f"),
            temp_class: Some(PhTemp::Fpr),
            xreg: "__f",
            ctor: Some(|s| field_ctor_expr(s, quote! { 0u32 })),
        },
        // ── 帧/地址语义 ──
        Ph {
            name: "{off}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { ctx.current_offset }),
        },
        Ph {
            name: "{alloca}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { ctx.current_alloca_offset as i64 }),
        },
        Ph {
            name: "{global}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(
                |_| quote! { ctx.current_global.map(|g| -(g.index() as i64) - 1).unwrap_or(0) },
            ),
        },
        // ── 整数常量（常量池解析）──
        Ph {
            name: "{iconst}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_int(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0)
                }
            }),
        },
        // {iconst_c16}：常量的 16 位取反补值（~v & 0xFFFF）——MOVN 的 imm16。
        // A64 MOVN 语义 = NOT(imm16 << shift)，其余位全 1：对 [-0x10000,-1]
        // 的负值，imm16 = ~v 使低 16 位还原为 v，高位全 1 = 符号扩展，单条
        // movn 即得正确负常量（X/W 均成立）。
        Ph {
            name: "{iconst_c16}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_int(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0);
                       ((!__v as u64) & 0xFFFF) as i64 })
                }
            }),
        },
        // {iconst_f0}..{iconst_f3}：常量的 16 位分片（低位序：f0=[15:0]、
        // f1=[31:16]、f2=[47:32]、f3=[63:48]）——MOVZ/MOVK hw 变体的 imm16。
        // 按 u64 两补码位型右移取片：i64 负大值（高位片 0xFFFF…）同样成立，
        // movz(hw3) 置高片清零其它 + movk(hw2..hw0) 依降序覆写 = 任意位型。
        Ph {
            name: "{iconst_f0}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_int(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0) as u64;
                       ((__v >> (16 * 0)) & 0xFFFF) as i64 })
                }
            }),
        },
        Ph {
            name: "{iconst_f1}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_int(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0) as u64;
                       ((__v >> (16 * 1)) & 0xFFFF) as i64 })
                }
            }),
        },
        Ph {
            name: "{iconst_f2}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_int(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0) as u64;
                       ((__v >> (16 * 2)) & 0xFFFF) as i64 })
                }
            }),
        },
        Ph {
            name: "{iconst_f3}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_int(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0) as u64;
                       ((__v >> (16 * 3)) & 0xFFFF) as i64 })
                }
            }),
        },
        Ph {
            name: "{iconst_hi20}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_int(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0); ((__v + 0x800) >> 12) & 0xFFFFF })
                }
            }),
        },
        Ph {
            name: "{iconst_lo12}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_int(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0);
                       let __lo = (__v << 52 >> 52) as i32;
                       (if __lo >= 0x800 { __lo - 0x1000 } else { __lo }) as i64 })
                }
            }),
        },
        Ph {
            name: "{iconst_hi32_hi20}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_int(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0);
                       ((((__v >> 32) as i64) + 0x800) >> 12) & 0xFFFFF })
                }
            }),
        },
        Ph {
            name: "{iconst_hi32_lo12}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_int(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0);
                       let __hi32 = (__v >> 32) as i64;
                       let __lo = (__hi32 << 52 >> 52) as i32;
                       (if __lo >= 0x800 { __lo - 0x1000 } else { __lo }) as i64 })
                }
            }),
        },
        Ph {
            name: "{iconst_lo32_hi20}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_int(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0);
                       ((((__v as i64 & 0xFFFFFFFF) + 0x800) >> 12) & 0xFFFFF) })
                }
            }),
        },
        Ph {
            name: "{iconst_lo32_lo12}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_int(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0);
                       let __lo32 = (__v as i64 & 0xFFFFFFFF);
                       let __lo = (__lo32 << 52 >> 52) as i32;
                       (if __lo >= 0x800 { __lo - 0x1000 } else { __lo }) as i64 })
                }
            }),
        },
        // ── 浮点常量（常量池位模式）──
        Ph {
            name: "{fconst}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_float(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0) as i64
                }
            }),
        },
        Ph {
            name: "{fconst_hi32_hi20}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_float(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0);
                       ((((__v >> 32) as i64) + 0x800) >> 12) & 0xFFFFF })
                }
            }),
        },
        Ph {
            name: "{fconst_hi32_lo12}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_float(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0);
                       let __hi32 = (__v >> 32) as i64;
                       let __lo = (__hi32 << 52 >> 52) as i32;
                       (if __lo >= 0x800 { __lo - 0x1000 } else { __lo }) as i64 })
                }
            }),
        },
        Ph {
            name: "{fconst_lo32_hi20}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_float(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0);
                       ((((__v as i64 & 0xFFFFFFFF) + 0x800) >> 12) & 0xFFFFF) })
                }
            }),
        },
        Ph {
            name: "{fconst_lo32_lo12}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    ({ let __v = ctx.constant_pool.as_ref()
                        .and_then(|p| p.resolve_float(crate::prelude::ConstId::from_raw(ctx.current_const_index)))
                        .unwrap_or(0);
                       let __lo32 = (__v as i64 & 0xFFFFFFFF);
                       let __lo = (__lo32 << 52 >> 52) as i32;
                       (if __lo >= 0x800 { __lo - 0x1000 } else { __lo }) as i64 })
                }
            }),
        },
        // ── 立即数语义（imm0 / lane / shuffle）──
        Ph {
            name: "{imm0}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { ctx.current_immediates.first().copied().unwrap_or(0) as i64 }),
        },
        Ph {
            name: "{imm0_sub4}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(
                |_| quote! { ctx.current_immediates.first().copied().unwrap_or(0).saturating_sub(4) as i64 },
            ),
        },
        Ph {
            name: "{shufps_imm8}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { __shufps_imm8(ctx.current_immediates.as_slice(), 0usize) }),
        },
        Ph {
            name: "{shufps_imm8_hi}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { __shufps_imm8(ctx.current_immediates.as_slice(), 4usize) }),
        },
        // ── 向量常量（lane 恢复）──
        Ph {
            name: "{vconst_lo2}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(
                |_| quote! { __vconst_raw64(ctx.constant_pool.as_ref(), ctx.current_const_index) },
            ),
        },
        Ph {
            name: "{vconst_lo}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    __vconst_half(ctx.constant_pool.as_ref(), ctx.current_const_index, 0usize, false, __vconst_elem_bits(ctx, results) == 64)
                }
            }),
        },
        Ph {
            name: "{vconst_hi}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    __vconst_half(ctx.constant_pool.as_ref(), ctx.current_const_index, 0usize, true, __vconst_elem_bits(ctx, results) == 64)
                }
            }),
        },
        Ph {
            name: "{vconst_lo_hi}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    __vconst_half(ctx.constant_pool.as_ref(), ctx.current_const_index, 1usize, false, __vconst_elem_bits(ctx, results) == 64)
                }
            }),
        },
        Ph {
            name: "{vconst_hi_hi}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    __vconst_half(ctx.constant_pool.as_ref(), ctx.current_const_index, 1usize, true, __vconst_elem_bits(ctx, results) == 64)
                }
            }),
        },
        // 512 位常量的第 3/4 个 128 位段（half=2/3；命名沿用 `_hi` 后缀=第 2 段
        // 的惯例，`_h2`/`_h3` 标 half 序号，lo/hi 标该 128 位段内的低/高 64 位）。
        // V512 的 Vconst 规则用它们拼 4 个 128 位段 → 4 条 EVEX 插入。
        Ph {
            name: "{vconst_lo_h2}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    __vconst_half(ctx.constant_pool.as_ref(), ctx.current_const_index, 2usize, false, __vconst_elem_bits(ctx, results) == 64)
                }
            }),
        },
        Ph {
            name: "{vconst_hi_h2}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    __vconst_half(ctx.constant_pool.as_ref(), ctx.current_const_index, 2usize, true, __vconst_elem_bits(ctx, results) == 64)
                }
            }),
        },
        Ph {
            name: "{vconst_lo_h3}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    __vconst_half(ctx.constant_pool.as_ref(), ctx.current_const_index, 3usize, false, __vconst_elem_bits(ctx, results) == 64)
                }
            }),
        },
        Ph {
            name: "{vconst_hi_h3}",
            kind: PhKind::Imm,
            loose: false,
            token_kind: "imm",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    __vconst_half(ctx.constant_pool.as_ref(), ctx.current_const_index, 3usize, true, __vconst_elem_bits(ctx, results) == 64)
                }
            }),
        },
        // ── 条件码 ──
        Ph {
            name: "{cc}",
            kind: PhKind::Cond,
            loose: false,
            token_kind: "cond",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { __cc }),
        },
        // v21 W6：终结层的**块号符号值**。`Br` 有 then/else 两个后继、`Jmp` 一个目标；
        // 取值由终结 lowering 在生成代码里定义成 `__true_block`/`__false_block`/`__target_block`
        // （`dfg.term_branch`/`term_jump` 的块号）。
        // v21 W6：`[lower.Br]` 的**条件寄存器**。与 `{g}` 同形（字段先填 0，映射期由 `xreg`
        // 改写成真寄存器），但 **不分配**临时——直接用终结层算好的 `__cond`。
        Ph {
            name: "{cond}",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "__cond",
            ctor: Some(|s| field_ctor_expr(s, quote! { 0u32 })),
        },
        // v21 W6 乙：**ABI 相对的返回槽**（按 IR 语义命名，不按 ISA 寄存器名 ✗）。
        // `{ret}` / `{retf}` = "返回值要去的那个槽" ✗，由终结 lowering 从宿主约定解析成
        // 物理寄存器（`__ret_dst`/`__retf_dst` ✗）——**物理** ⇒ `xreg: "0u32"`（不参与映射 ✗）。
        // `{ret_src}` / `{retf_src}` = 承载返回值的**虚拟**寄存器 ✗，与 `{cond}` 同形：
        // 字段先填 0，`map_reg_field` 用 `xreg` 把 IR 返回值绑上去 ✗。
        Ph {
            name: "{ret}",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { __ret_dst }),
        },
        Ph {
            name: "{ret_src}",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "__ret_src",
            ctor: Some(|s| field_ctor_expr(s, quote! { 0u32 })),
        },
        Ph {
            name: "{retf}",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { __retf_dst }),
        },
        Ph {
            name: "{retf_src}",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "__retf_src",
            ctor: Some(|s| field_ctor_expr(s, quote! { 0u32 })),
        },
        // `{sret}` = **按引用返回（sret）的隐藏指针**（>16B 向量的落点 ✗），由调用布局给 ✗。
        // 物理寄存器 ⇒ `xreg: "0u32"`（不参与 map_reg_field ✗）；常用于内存操作数写法
        // `[{sret}]`（只写基址、disp 恒 0 ✗）。
        // v21 W6 乙：**多值返回的第 k 个分量**（循环内逐次绑定 ✗）——
        // `{retN}` = 第 k 个返回槽（布局的 Pair/Group 第 k 项 ✗），
        // `{retN_src}` = 第 k 个结果值（`map_reg_field` 绑 ✗）。整数/浮点分量共用这两个名字 ✗。
        Ph {
            name: "{retN}",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { __retN }),
        },
        Ph {
            name: "{retN_src}",
            kind: PhKind::Reg,
            loose: false,
            token_kind: "reg",
            temp: None,
            temp_class: None,
            xreg: "__retN_src",
            ctor: Some(|s| field_ctor_expr(s, quote! { 0u32 })),
        },
        Ph {
            name: "{sret}",
            kind: PhKind::Mem,
            loose: false,
            token_kind: "mem",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| {
                quote! {
                    MemRef {
                        base: Some(__sret),
                        disp: 0,
                        index: None,
                        scale: 1,
                    }
                }
            }),
        },
        Ph {
            name: "{true}",
            kind: PhKind::Cond,
            loose: false,
            token_kind: "label",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { __true_block }),
        },
        Ph {
            name: "{false}",
            kind: PhKind::Cond,
            loose: false,
            token_kind: "label",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { __false_block }),
        },
        Ph {
            name: "{target}",
            kind: PhKind::Cond,
            loose: false,
            token_kind: "label",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { __target_block }),
        },
        // v21 W6：**条件取反**——IR 条件的逆（eq↔ne、slt↔sge…）在本 ISA 的编码。
        // 编码不是硬编码，而是从 `[enum.cond]` 反查（见 `lowering.rs` 的 `__cc_inv`）。
        Ph {
            name: "{cc_inv}",
            kind: PhKind::Cond,
            loose: false,
            token_kind: "cond",
            temp: None,
            temp_class: None,
            xreg: "0u32",
            ctor: Some(|_| quote! { __cc_inv }),
        },
    ]
}

// ─────────────────────────── 查询 ───────────────────────────

/// 按 token 文本精确查表（{gN}/{fN} 编号临时走 `numbered_temp`）。
pub(crate) fn lookup(name: &str) -> Option<&'static Ph> {
    placeholders().iter().find(|p| p.name == name)
}

/// 编号临时 {gN}/{fN}（N ≥ 1）：**动态解析，任意编号上限**（与 `{N}`
/// 编号操作数一致——不受静态表条目数限制，指令需要几个临时就支持几个）。
/// `{gN}` → GPR `__gN`；`{fN}` → FPR `__fN`（g/f = Gpr/Fpr 的首字母，
/// 与其他占位符无前缀冲突）。
/// 返回 (临时变量名, 类别)。非编号临时（{g}/{f} 静态条目）→ None。
pub(crate) fn numbered_temp(name: &str) -> Option<(String, PhTemp)> {
    let body = name.strip_prefix('{').and_then(|r| r.strip_suffix('}'))?;
    // 区分 {gN} 与 {fN}：前缀 g → GPR、f → FPR
    let (cls, digits) = match body.strip_prefix('g') {
        Some(d) => (PhTemp::Gpr, d),
        None => {
            let d = body.strip_prefix('f')?;
            (PhTemp::Fpr, d)
        }
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    if n == 0 {
        return None; // {g0}/{f0} 无意义
    }
    let var = if cls == PhTemp::Gpr {
        format!("__g{n}")
    } else {
        format!("__f{n}")
    };
    Some((var, cls))
}

/// 编号操作数 {N}（N ≥ 0）：**动态解析，任意编号上限**——指令有 3 个以上
/// 操作数时 {3}/{4}/… 照常可用（旧静态表只注册到 {2}，超界落到 fallback
/// 编译错）。{0}→rs1、{1}→rs2、{2}→rs3、{N}→rs{N+1}。非编号 → None。
pub(crate) fn numbered_operand(name: &str) -> Option<usize> {
    let digits = name.strip_prefix('{').and_then(|r| r.strip_suffix('}'))?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: usize = digits.parse().ok()?;
    // 操作数编号上限：64（模板超界 → 编译期报错，防误用）
    if n >= 64 {
        return None;
    }
    Some(n)
}

/// lowering_token_kind 的形状签名（token → reg/imm/cond/num/mem）。
/// 注册表查不到（字面量/物理寄存器/内存表达式）时回退旧规则。
pub(crate) fn token_kind(op: &str) -> &'static str {
    // v21 W6：算术表达式占位符 ⇒ 种类按首个操作数（`{imm0 - 4}` 是 imm）。
    if is_arith_expr(op)
        && let Some(f) = first_operand(op)
    {
        return token_kind(&f);
    }
    // 剥外层方括号（内存基址写法 [{n}]）
    let inner = if let Some(op) = op.strip_circumfix('[', ']') {
        op
    } else {
        op
    };
    if let Some(p) = lookup(inner) {
        return p.token_kind;
    }
    if numbered_temp(inner).is_some() {
        return "reg";
    }
    // {N} 编号操作数 → 符号寄存器（reg）
    if numbered_operand(inner).is_some() {
        return "reg";
    }
    // 物理寄存器（RAX 等全大写/数字）
    if inner.starts_with('{')
        && inner.ends_with('}')
        && inner[1..inner.len() - 1]
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return "reg";
    }
    // 数字立即数
    if super::integration::parse_i64_lit(inner).is_ok() {
        return "num";
    }
    // 真正的内存操作数
    if inner.contains('+') || inner.contains('-') || inner.contains('(') || inner.contains(')') {
        return "mem";
    }
    "reg"
}

/// 模板中出现的全部临时寄存器声明（去重）：
/// `(变量名, 类别)` 列表——gen_lowering 按此预声明。
/// **显式临时声明**（v21 W6）：`["g1:gpr", "f2:fpr"]` ⇒ `(变量名, 类别)`。
///
/// 声明了显式集合 ⇒ `templates` 里用到的每个 `{gN}`/`{fN}` 都必须已声明，
/// 否则返回错误（点名是哪一个）——把"靠前缀猜类别"的隐式魔法变成**可选显式**。
pub(crate) fn explicit_temps(
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
    // 校验：用到的临时都得声明
    for (var, _) in collect_temps(templates) {
        if !out.iter().any(|(v, _)| *v == var) {
            let shown = var.trim_start_matches("__").to_string();
            return Err(format!(
                "temps: `insts` 用了 `{{{shown}}}`，但它不在本规则的 `temps` 里（声明了 temps 就必须显式列出）"
            ));
        }
    }
    Ok(out)
}

pub(crate) fn collect_temps(templates: &[String]) -> Vec<(String, PhTemp)> {
    let mut out: Vec<(String, PhTemp)> = Vec::new();
    for t in templates {
        for tok in brace_tokens(t) {
            let entry: Option<(String, PhTemp)> = if let Some(p) = lookup(tok) {
                p.temp
                    .map(|tv| (tv.to_string(), p.temp_class.unwrap_or(PhTemp::Gpr)))
            } else {
                numbered_temp(tok)
            };
            if let Some((var, cls)) = entry
                && !out.iter().any(|(v, _)| *v == var)
            {
                out.push((var, cls));
            }
        }
    }
    out
}

/// 提取字符串中的全部 `{...}` token（顺序出现）。
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

/// ctor 构建：查注册表 → 闭包；`{N}` 编号操作数 → 符号寄存器占位 ctor；
/// `{gN}`/`{fN}` 编号临时 → 符号寄存器占位 ctor；查不到 → None。
/// `slot` 为指令操作数槽。
pub(crate) fn ctor_for(name: &str, slot: &OperandSlot) -> Option<TokenStream> {
    if let Some(p) = lookup(name) {
        let f = p.ctor?;
        return Some(f(slot));
    }
    if numbered_operand(name).is_some() || numbered_temp(name).is_some() {
        return Some(field_ctor_expr(slot, quote! { 0u32 }));
    }
    None
}

/// xreg 绑定表达式（map_reg_field 用；`0u32` = 不绑定）。
pub(crate) fn xreg_for(name: &str) -> TokenStream {
    let x = if let Some(p) = lookup(name) {
        p.xreg
    } else if let Some((var, _)) = numbered_temp(name) {
        // {gN}/{fN} → 预声明临时变量（__gN/__fN）
        let id = format_ident!("{var}");
        return quote! { #id };
    } else if let Some(n) = numbered_operand(name) {
        // {N} → rs{N+1}（{0}→rs1、{1}→rs2、{2}→rs3 … 任意多操作数）
        let _ = n;
        return numbered_xreg(name);
    } else {
        "0u32"
    };
    if x == "0u32" {
        quote! { 0u32 }
    } else {
        let id = format_ident!("{x}");
        quote! { #id }
    }
}

/// 编号操作数 {N} → xreg 符号名 `rs{N+1}`（TokenStream）。
fn numbered_xreg(name: &str) -> TokenStream {
    let n = numbered_operand(name).expect("numbered_xreg 仅对 {N} 调用");
    let id = format_ident!("rs{}", n + 1);
    quote! { #id }
}

/// **算术表达式占位符**（v21 W6）：`{a + b}` / `{iconst >> 12 & 0xfffff}` 这类写法。
///
/// 认法：整串被一层 `{}` 包住、且**顶层**（不嵌套在更内层花括号里）出现运算符。
/// 每个操作数必须是**已注册占位符**（`{imm0}` 之类）或整数字面量。
pub(crate) fn is_arith_expr(op: &str) -> bool {
    let Some(inner) = op.strip_prefix('{').and_then(|s| s.strip_suffix('}')) else {
        return false;
    };
    let mut depth = 0i32;
    for c in inner.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            '+' | '-' | '*' | '/' | '&' | '|' | '^' if depth == 0 => return true,
            '<' | '>' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// 表达式里**首个操作数**的内层片段（去掉外层 `{}` 与结尾运算符）。
fn first_operand(op: &str) -> Option<String> {
    let inner = op.strip_prefix('{')?.strip_suffix('}')?;
    let mut out = String::new();
    let mut depth = 0i32;
    for c in inner.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            '+' | '-' | '*' | '/' | '&' | '|' | '^' | '<' | '>' if depth == 0 => break,
            _ => {}
        }
        out.push(c);
    }
    let s = out.trim().to_string();
    if s.is_empty() {
        return None;
    }
    // 注册名形如 `{imm0}` ⇒ 补回花括号再交给 lookup/kind_matches。
    if s.starts_with('{') {
        Some(s)
    } else {
        Some(format!("{{{s}}}"))
    }
}

/// 表达式 ctor：逐段把操作数换成各自 ctor 的产物（运算符原样 ❗）。
pub(crate) fn expr_ctor(op: &str, slot: &OperandSlot) -> Option<TokenStream> {
    if !is_arith_expr(op) {
        return None;
    }
    let inner = op.strip_prefix('{')?.strip_suffix('}')?;
    let mut toks: Vec<TokenStream> = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let flush = |cur: &mut String, toks: &mut Vec<TokenStream>| -> Option<()> {
        let s = cur.trim().to_string();
        cur.clear();
        if s.is_empty() {
            return Some(());
        }
        if let Ok(v) = s.parse::<i64>() {
            toks.push(quote! { (#v as i64) });
            return Some(());
        }
        let name = if s.starts_with('{') {
            s.clone()
        } else {
            format!("{{{s}}}")
        };
        let ph = lookup(&name)?;
        let ctor = ph.ctor?;
        toks.push(ctor(slot));
        Some(())
    };
    for c in inner.chars() {
        match c {
            '{' => {
                depth += 1;
                cur.push(c);
            }
            '}' => {
                depth -= 1;
                cur.push(c);
            }
            '+' | '-' | '*' | '/' | '&' | '|' | '^' | '<' | '>' if depth == 0 => {
                flush(&mut cur, &mut toks)?;
                let ops = if (c == '<' || c == '>') && inner.contains(&format!("{c}{c}")) {
                    format!("{c}{c}")
                } else {
                    c.to_string()
                };
                toks.push(match ops.as_str() {
                    "+" => quote! { + },
                    "-" => quote! { - },
                    "*" => quote! { * },
                    "/" => quote! { / },
                    "&" => quote! { & },
                    "|" => quote! { | },
                    "^" => quote! { ^ },
                    "<<" => quote! { << },
                    ">>" => quote! { >> },
                    _ => return None,
                });
            }
            _ => cur.push(c),
        }
    }
    flush(&mut cur, &mut toks)?;
    // 末尾可能是重复吃到的运算符（`<<` 被拆两次）⇒ 去掉悬挂运算符
    if toks.len() >= 2 {
        let mut expr = quote! {};
        let mut i = 0;
        while i < toks.len() {
            let tk = &toks[i];
            expr = quote! { #expr #tk };
            i += 1;
        }
        return Some(quote! { ( #expr ) });
    }
    None
}

/// 槽类型约束检查：占位符是否可用于该槽（编号临时归入 Reg；loose 不约束；
/// {N} 编号操作数同符号寄存器——loose，任何槽可用）。
pub(crate) fn kind_matches(name: &str, slot_kind: OperandKind) -> bool {
    // v21 W6：算术表达式占位符 ⇒ 按**首个操作数**的槽适配判定。
    if is_arith_expr(name) {
        return first_operand(name)
            .map(|f| kind_matches(&f, slot_kind))
            .unwrap_or(false);
    }
    // v21 W6：终结层的**块号符号值**可落 Imm/Label 两种槽——各 ISA 的目标槽种类不同
    // （x86 `target:rel`、riscv `target:off_j`、arm64 `target:off26`）。
    let bare = name.trim();
    if matches!(bare, "{true}" | "{false}" | "{target}") {
        return matches!(slot_kind, OperandKind::Imm | OperandKind::Label);
    }
    if let Some(p) = lookup(name) {
        if p.loose {
            return true;
        }
        return match p.kind {
            PhKind::Reg => slot_kind == OperandKind::Reg,
            PhKind::Mem => slot_kind == OperandKind::Mem,
            PhKind::Imm => slot_kind == OperandKind::Imm,
            PhKind::Cond => slot_kind == OperandKind::Cond,
        };
    }
    if numbered_temp(name).is_some() {
        return slot_kind == OperandKind::Reg;
    }
    // {N} 编号操作数：loose（与旧 {0}/{1}/{2} 条目一致）
    numbered_operand(name).is_some()
}

/// 该 `{…}` token 是否是本 DSL 认识的占位符：静态注册表 ∪ `{N}` 编号操作数
/// ∪ `{gN}`/`{fN}` 编号临时。**校验期用**：模板里写错名字（`{iconst_lo}`
/// 少个 `12`）今天会静默落到 fallback 分支，装出一个字面量 0 或物理寄存器，
/// 生成能编译但语义错的代码——`validate_lowering` 据本函数提前拒绝。
pub(crate) fn is_known(name: &str) -> bool {
    lookup(name).is_some() || numbered_operand(name).is_some() || numbered_temp(name).is_some()
}

/// 唯一性断言（测试用）：注册表无重名、无重复临时变量。
pub(crate) fn assert_unique() -> Result<(), String> {
    let ps = placeholders();
    for (i, p) in ps.iter().enumerate() {
        for q in &ps[i + 1..] {
            if p.name == q.name {
                return Err(format!("占位符重名: {}", p.name));
            }
            if let (Some(a), Some(b)) = (p.temp, q.temp)
                && a == b
            {
                return Err(format!("临时变量冲突: {a}（{} 与 {}）", p.name, q.name));
            }
        }
    }
    Ok(())
}

/// **仅供单测**：表达式占位符的种类（等于 `token_kind`，但名字更直白）。
#[cfg(test)]
pub(crate) fn first_operand_kind_of_test(op: &str) -> &'static str {
    token_kind(op)
}
