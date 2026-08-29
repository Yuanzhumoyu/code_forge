//! 变长（VEX/EVEX/前缀扫描）encode/decode——x86 专用机制。
//!
//! 从 `codegen/mod.rs` 拆分（原 818-3010 行）：变长 ISA（x86）的字段语义、
//! VEX/EVEX/前缀扫描与字节前缀决策树集中于此；定宽 ISA（riscv）不经过本
//! 模块（`generate()` 按 `meta.variable_length` 分派）。复用父模块的
//! `field_ctor_expr`/`sign_extend_ts`（位域工具）与 `shared::parse_u64`。

use super::super::model::*;
use super::super::shared::parse_u64;
use super::{field_ctor_expr, field_ctor_expr_view, sign_extend_ts, InstInfo};
use proc_macro2::TokenStream;
use quote::quote;

// ─────────────────────── 变长 encode/decode（迭代 3）───────────────────────

/// 变长 form 的字段语义解析结果。
struct VlenCtx {
    /// 是否 opsize 语义（有 Reg 操作数 → 由寄存器宽度推导）。
    has_opsize: bool,
    /// opsize 值表达式（u64）：操作数 Reg 的 width()。
    opsize_expr: Option<TokenStream>,
    /// 固定前缀字节表达式（u8）：fields.prefix / 数字 / 0。
    prefix_expr: TokenStream,
    /// REX.W 位表达式（u64）：auto（opsize==64）/ fields.w / 0。
    rex_w_expr: TokenStream,
    /// ModRM 语义（`+r` 形式为 None）。
    modrm: Option<ModrmKind>,
    /// 固定 ModRM 字节（无操作数：MFENCE 0F AE F0）；与 modrm 互斥。
    modrm_fixed: Option<u64>,
    /// modrm reg 字段值表达式（u64）：rr → 操作数 0；ext → fields.ext。
    reg_expr: Option<TokenStream>,
    /// modrm rm 字段值表达式（u64；内存形式 = 基址）。
    rm_expr: Option<TokenStream>,
    /// modrm reg 操作数是否 8 位寄存器（sil/dil/spl/bpl 需 REX 前缀才能编码）。
    reg_is_byte: bool,
    /// modrm rm 操作数是否 8 位寄存器（同上）。
    rm_is_byte: bool,
    /// 内存形式位移表达式（i64；rr/ext 为 None）。
    disp_expr: Option<TokenStream>,
    /// 尾部立即数字节数（form.imm / 8）。
    imm_bytes: usize,
    /// VEX 语义（form.vex 存在时）：map/pp/w/l 值表达式（u64）。
    vex: Option<VexCtx>,
    /// EVEX 语义（form.evex 存在时；AVX-512 最小集）。
    evex: Option<EvexCtx>,
    /// `+r` 形式 opcode 基值（50/58/B8/C8...）；None = 普通 opcode。
    opcode_reg: Option<u64>,
    /// rex_w = "always"：恒发 REX.W（+r 的 mov_imm64/bswap）。
    rex_w_always: bool,
    /// 每个操作数的寄存器视图：(槽 class 组名, 固定宽度位)。固定宽度 =
    /// 槽 class 组的宽度（如 [gpr32] → 32）；class=None → 多态（None）。
    reg_view: Vec<Option<u16>>,
    /// decode 宽度 guard：固定宽度 Reg 操作数（16/32/64）→ `__opsize == w`。
    decode_width_guard: Option<u16>,
    /// decode 反向宽度 guard：SSE field-w 系 w=0 → `__opsize != 64`。
    decode_width_neq64: bool,
}

/// VEX 编码上下文（迭代 4）。
struct VexCtx {
    map_expr: TokenStream,
    pp_expr: TokenStream,
    w_expr: TokenStream,
    l_expr: TokenStream,
    /// vvvv 语义：操作数 ≥3 且第 3 个是 reg 槽 → ~op2（有源）；否则 0x0F。
    has_src: bool,
}

/// EVEX 编码上下文（AVX-512：reg-reg 与内存形式、压缩位移、opmask/broadcast/z）。
struct EvexCtx {
    /// opcode map（mm，0/1/2 → 0F/0F38/0F3A）。
    map_expr: TokenStream,
    /// 前缀（pp：0 无 / 1 66 / 2 F3 / 3 F2）。
    pp_expr: TokenStream,
    /// REX.W 位。
    w_expr: TokenStream,
    /// 向量长度 L'L（0/1/2 → 128/256/512 位）。
    l_val: u64,
    /// broadcast 位（P2 bit4；0/1）。
    b_val: u64,
    /// 零掩码位 z（P2 bit7；0/1）。
    z_val: u64,
    /// opmask aaa 表达式（u8；P2 bit2-0）：掩码操作数（kreg 类槽）的寄存器
    /// 索引 & 7；无掩码操作数 → 0。
    aaa_expr: Option<TokenStream>,
    /// 压缩位移缩放 N（disp8 × N；1 = 不缩放）。
    disp_scale: u64,
    /// 三操作数有源（vvvv=~op1/op2）。
    has_src: bool,
}

/// ModRM 语义键（迭代 3/3b）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModrmKind {
    /// mod=11：reg=op0, rm=op1。
    RR,
    /// mod=11：reg=op1, rm=op0（0F 7E 系：MOVD/MOVQ r, xmm 的 reg=源 xmm）。
    RRRev,
    /// mod=11：reg=op0, rm=op2（VEX imm 系：VINSERTF128 的 reg=dest、rm=src2）。
    RRSrc2,
    /// mod=11：reg=fields.ext（扩展码），rm=op0。
    Ext,
    /// 内存：reg=op0, base=op1（reg 槽），disp=0。
    MemReg,
    /// 内存：reg=op0, mem=op1（mem 槽 → base/disp）。
    MemRefOp,
}

impl ModrmKind {
    fn parse(s: Option<&str>) -> Option<ModrmKind> {
        match s {
            Some("rr") => Some(ModrmKind::RR),
            Some("rr_rev") => Some(ModrmKind::RRRev),
            Some("rr_src2") => Some(ModrmKind::RRSrc2),
            Some("ext") => Some(ModrmKind::Ext),
            Some("rm_mem") => Some(ModrmKind::MemReg),
            Some("rm_memref") => Some(ModrmKind::MemRefOp),
            _ => None,
        }
    }
    fn is_mem(self) -> bool {
        matches!(self, ModrmKind::MemReg | ModrmKind::MemRefOp)
    }
}

fn vlen_ctx(info: &InstInfo, _m: &V12Model) -> Result<VlenCtx, String> {
    let form = info.form;
    let fields = info.inst.fields.as_ref();
    let field_val = |k: &str| fields.and_then(|f| f.get(k)).copied().unwrap_or(0);
    // 每个 Reg 操作数的固定宽度（槽 class 组宽度；class=None → 多态 None）
    let reg_view: Vec<Option<u16>> = info
        .operands
        .iter()
        .map(|(_, _, s, _)| {
            if s.kind == OperandKind::Reg {
                s.class.as_ref().map(|c| c.width())
            } else {
                None
            }
        })
        .collect();
    // opsize（v12.1）：数字 ≥8 → 固定宽度；数字 <8 → 操作数序号（宽度由该
    // 操作数寄存器推导，如 opsize=0 取 op0 的 RAX→64/EAX→32）；缺省 = 第一
    // 个 Reg 槽操作数。指令级覆盖（Instruction.opsize）优先于 form 级。
    let form_opsize = info.inst.opsize.or(form.opsize);
    let rex_w_override = info.inst.rex_w.as_deref().or(form.rex_w.as_deref());
    let (has_opsize, opsize_expr) = match form_opsize {
        Some(o) => match o {
            Opsize::Reg(width) => (true, Some(quote! { #width })),
            Opsize::Slot(idx) => {
                let (_, fid, slot, _) = info.operands.get(idx as usize).ok_or_else(|| {
                    format!(
                        "[[instructions.{}]]: form opsize slot ={} references missing operand {:?}",
                        info.inst.name,
                        idx,
                        info.operands.len()
                    )
                })?;
                if slot.kind != OperandKind::Reg {
                    return Err(format!(
                        "[[instructions.{}]]: form opsize slot ={} must reference a reg operand (got {})",
                        info.inst.name,
                        idx,
                        slot.kind.kind_name()
                    ));
                }
                (true, Some(quote! { #fid.width() }))
            }
        },
        // 无 opsize 键 → 无 opsize 语义（SSE 等宽度由 fields.w 固定，不参与
        // 66/REX.W 前缀推导）。需要推导的 form 显式声明 opsize = 序号。
        None => (false, None),
    };
    // decode 宽度 guard：`__opsize == w`。仅对有 opsize 语义且非 SSE field-w
    // 系（w 字段固定，REX.W 与 opsize 无关）的指令；w=0 的 SSE 系需要反向
    // `__opsize != 64`（MOVD w=0 vs MOVQ w=1 同 opcode 区分）。
    // 指令级覆盖优先（cvtsi2sd 合并：opsize=s1 驱动 REX.W）。
    // 多类槽（reg_view=None）且 opsize=Slot：w 位 = REX.W = (__opsize==8)。
    let rex_w_key = rex_w_override;
    let (decode_width_guard, decode_width_neq64) = if rex_w_key == Some("field") {
        let w0 = fields.and_then(|f| f.get("w")).copied() == Some(0);
        (None, w0)
    } else if !has_opsize {
        (None, false)
    } else if let Some(n) = form_opsize {
        match n {
            Opsize::Reg(width) => (Some(width), false),
            Opsize::Slot(idx) => match reg_view.get(idx as usize).copied().flatten() {
                // 多类槽：REX.W 由前缀扫描的 __opsize 判定（REX.W=1 → 8）
                None => (Some(8), false),
                Some(w) => (Some(w), false),
            },
        }
    } else {
        (
            reg_view
                .iter()
                .flatten()
                .copied()
                .find(|w| *w == 2 || *w == 4 || *w == 8),
            false,
        )
    };
    // 前缀
    let prefix_expr: TokenStream = match form.prefix.as_deref() {
        None => quote! { 0u8 },
        Some("field") => {
            let v = field_val("prefix");
            quote! { #v as u8 }
        }
        Some(p) => {
            let v = parse_u64(p).unwrap_or(0);
            quote! { #v as u8 }
        }
    };
    // REX.W：显式声明优先；有 opsize 语义且未声明 → 默认 opsize==64 驱动。
    // 指令级覆盖（Instruction.rex_w）优先于 form 级。
    let rex_w_expr: TokenStream = match rex_w_override {
        Some("auto") => quote! { if __opsize == 8 { 1u64 } else { 0u64 } },
        Some("field") => {
            let v = field_val("w");
            quote! { #v as u64 }
        }
        None if has_opsize => quote! { if __opsize == 8 { 1u64 } else { 0u64 } },
        _ => quote! { 0u64 },
    };
    // modrm reg/rm/disp：`+r` 形式与无 ModRM 形式（REL32/NOOP）为 None；
    // modrm_fixed（无操作数固定 ModRM 字节）与 modrm 互斥。
    let (modrm, reg_expr, rm_expr, disp_expr, reg_is_byte, rm_is_byte) =
        if form.opcode_reg.is_some() || form.modrm.is_none() || form.modrm_fixed.is_some() {
            (None, None, None, None, false, false)
        } else {
            let modrm = ModrmKind::parse(form.modrm.as_deref()).ok_or_else(|| {
            format!(
                "[[instructions.{}]]: form modrm key {:?} unsupported (rr/ext/rm_mem/rm_memref)",
                info.inst.name, form.modrm
            )
        })?;
            let (reg_expr, rm_expr, disp_expr): (TokenStream, TokenStream, Option<TokenStream>) =
                match modrm {
                    ModrmKind::RR => {
                        let r0 = info.operands[0].1.clone();
                        let r1 = info.operands[1].1.clone();
                        (
                            quote! { #r0.to_index() as u64 },
                            quote! { #r1.to_index() as u64 },
                            None,
                        )
                    }
                    ModrmKind::RRRev => {
                        // 0F 7E 系（MOVD/MOVQ r, xmm）：reg=op1（源 xmm）、rm=op0（目标 gpr）
                        let r0 = info.operands[0].1.clone();
                        let r1 = info.operands[1].1.clone();
                        (
                            quote! { #r1.to_index() as u64 },
                            quote! { #r0.to_index() as u64 },
                            None,
                        )
                    }
                    ModrmKind::RRSrc2 => {
                        // VEX imm 系（VINSERTF128）：reg=op0（dest）、rm=op2（src2）
                        let r0 = info.operands[0].1.clone();
                        let r2 = info.operands[2].1.clone();
                        (
                            quote! { #r0.to_index() as u64 },
                            quote! { #r2.to_index() as u64 },
                            None,
                        )
                    }
                    ModrmKind::Ext => {
                        let ext = field_val("ext");
                        let r0 = info.operands[0].1.clone();
                        (quote! { #ext }, quote! { #r0.to_index() as u64 }, None)
                    }
                    ModrmKind::MemReg => {
                        let r0 = info.operands[0].1.clone();
                        let b = info.operands[1].1.clone();
                        (
                            quote! { #r0.to_index() as u64 },
                            quote! { #b.to_index() as u64 },
                            Some(quote! { 0i64 }),
                        )
                    }
                    ModrmKind::MemRefOp => {
                        let r0 = info.operands[0].1.clone();
                        let m = info.operands[1].1.clone();
                        (
                            quote! { #r0.to_index() as u64 },
                            quote! { #m.base.to_index() as u64 },
                            Some(quote! { #m.disp }),
                        )
                    }
                };
            // 8 位寄存器标记：reg=op0（RR/Mem*/MemRefOp）、rm=op1（RR）或 op0（Ext）。
            // x86 8 位寄存器索引 4-7（spl/bpl/sil/dil）无 REX 前缀编码为 ah/ch/dh/bh，
            // 故这类操作数必须强制 REX（即使索引 <8）。
            let slot_byte =
                |s: &OperandSlot| s.kind == OperandKind::Reg && s.byte_reg == Some(true);
            let (reg_is_byte, rm_is_byte) = match modrm {
                ModrmKind::RR => (slot_byte(info.operands[0].2), slot_byte(info.operands[1].2)),
                ModrmKind::Ext => (false, slot_byte(info.operands[0].2)),
                _ => (false, false),
            };
            (
                Some(modrm),
                Some(reg_expr),
                Some(rm_expr),
                disp_expr,
                reg_is_byte,
                rm_is_byte,
            )
        };
    let imm_bytes = (form.imm.unwrap_or(0) / 8) as usize;
    let vex = if let Some(vs) = &form.vex {
        let f = |k: &str, key: &str| -> TokenStream {
            match vs_value(vs, k) {
                Some(v) => quote! { #v as u64 },
                None => {
                    let v = field_val(key);
                    quote! { #v as u64 }
                }
            }
        };
        // vvvv：操作数 ≥3 且第 3 个是 reg 槽 → 有源（~op2）
        let has_src = info.operands.len() >= 3 && info.operands[2].2.kind == OperandKind::Reg;
        Some(VexCtx {
            map_expr: f("map", "vex_map"),
            pp_expr: f("pp", "vex_pp"),
            w_expr: f("w", "vex_w"),
            l_expr: f("l", "vex_l"),
            has_src,
        })
    } else {
        None
    };
    // EVEX（AVX-512）：复用 VexSpec 数据键；l（0/1/2）为编译期 L'L 值。
    let evex = if let Some(es) = &form.evex {
        let f = |k: &str, key: &str| -> TokenStream {
            match vs_value(es, k) {
                Some(v) => quote! { #v as u64 },
                None => {
                    let v = field_val(key);
                    quote! { #v as u64 }
                }
            }
        };
        let has_src = info.operands.len() >= 3 && info.operands[2].2.kind == OperandKind::Reg;
        let l_val = match vs_value(es, "l") {
            Some(v) => v,
            None => field_val("evex_l"),
        };
        if l_val > 2 {
            return Err(format!(
                "[[instructions.{}]]: evex l must be 0/1/2 (128/256/512), got {l_val}",
                info.inst.name
            ));
        }
        let b_val = match vs_value(es, "b") {
            Some(v) => v,
            None => field_val("evex_b"),
        };
        if b_val > 1 {
            return Err(format!(
                "[[instructions.{}]]: evex b must be 0/1, got {b_val}",
                info.inst.name
            ));
        }
        let z_val = match vs_value(es, "z") {
            Some(v) => v,
            None => field_val("evex_z"),
        };
        if z_val > 1 {
            return Err(format!(
                "[[instructions.{}]]: evex z must be 0/1, got {z_val}",
                info.inst.name
            ));
        }
        // opmask aaa：操作数中第一个 kreg 类槽（掩码寄存器）的索引 & 7；
        // 无 → 0（无掩码）。
        let aaa_expr = info
            .operands
            .iter()
            .find(|(_, _, s, _)| {
                s.kind == OperandKind::Reg
                    && s.class
                        .as_ref()
                        .is_some_and(|c| matches!(c, RegClass::KReg(_)))
            })
            .map(|(_, fid, _, _)| quote! { (#fid.to_index() & 7) as u8 });
        let disp_scale = match vs_value(es, "disp_scale") {
            Some(v) => v,
            None => field_val("evex_disp_scale"),
        };
        let disp_scale = if disp_scale == 0 { 1 } else { disp_scale };
        if disp_scale > 64 || disp_scale & (disp_scale - 1) != 0 {
            return Err(format!(
                "[[instructions.{}]]: evex disp_scale must be a power of 2 ≤ 64, got {disp_scale}",
                info.inst.name
            ));
        }
        Some(EvexCtx {
            map_expr: f("map", "evex_map"),
            pp_expr: f("pp", "evex_pp"),
            w_expr: f("w", "evex_w"),
            l_val,
            b_val,
            z_val,
            aaa_expr,
            disp_scale,
            has_src,
        })
    } else {
        None
    };
    Ok(VlenCtx {
        has_opsize,
        opsize_expr,
        prefix_expr,
        rex_w_expr,
        modrm,
        modrm_fixed: form.modrm_fixed,
        reg_expr,
        rm_expr,
        reg_is_byte,
        rm_is_byte,
        disp_expr,
        imm_bytes,
        vex,
        evex,
        opcode_reg: form.opcode_reg,
        rex_w_always: form.rex_w.as_deref() == Some("always"),
        reg_view,
        decode_width_guard,
        decode_width_neq64,
    })
}

/// VEX/EVEX 字段来源：`"field"` → None（取 fields.vex_*/evex_*）；数字 → 固定值。
fn vs_value(vs: &VexSpec, key: &str) -> Option<u64> {
    let s = match key {
        "map" => vs.map.as_deref(),
        "pp" => vs.pp.as_deref(),
        "w" => vs.w.as_deref(),
        "l" => vs.l.as_deref(),
        "b" => vs.b.as_deref(),
        "z" => vs.z.as_deref(),
        "disp_scale" => vs.disp_scale.as_deref(),
        _ => None,
    }?;
    if s == "field" { None } else { parse_u64(s) }
}

/// 变长 encode：字节流（prefix → REX → escape → opcode → ModRM → imm）。
/// `pub(crate)`：由 `codegen::generate()` 按 `variable_length` 分派调用。
pub(crate) fn gen_vlen_encode(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    let mut arms = Vec::new();
    for info in infos {
        let vn = &info.vn;
        let ctx = vlen_ctx(info, model)?;
        let mut stmts: Vec<TokenStream> = Vec::new();
        // opsize → __opsize 局部（必须先于 66/REX 检查）
        let opsize_bind = if let Some(oe) = &ctx.opsize_expr {
            quote! { let __opsize:u16 = #oe; }
        } else {
            quote! { let __opsize:u16 = 0; }
        };
        // 固定宽度组校验（严格类型检测）：槽声明 [gpr32] 等 → 操作数寄存器
        // 宽度必须匹配，否则报错（不静默截断）。
        let mut width_checks: Vec<TokenStream> = Vec::new();
        for (i, (_, fid, slot, _)) in info.operands.iter().enumerate() {
            if slot.kind != OperandKind::Reg {
                continue;
            }
            if let Some(Some(w)) = ctx.reg_view.get(i) {
                let name_lit = syn::LitStr::new(&info.inst.name, proc_macro2::Span::call_site());
                width_checks.push(quote! {
                    if #fid.width() != #w {
                        return Err(format!(
                            "{}: operand {} must be a {}-bit register (got {}-bit)",
                            #name_lit, #i, #w, #fid.width()
                        ));
                    }
                });
            }
        }
        // `+r` 形式（50+r/push、58+r/pop、B8+r/mov_imm64、C8+r/bswap）：
        // REX（always → 0x48|B；否则 reg≥8 → 0x41）+ [escape] + opcode|reg&7 + imm
        if let Some(base) = ctx.opcode_reg {
            let reg0 = info.operands[0].1.clone();
            let rex_always = ctx.rex_w_always;
            stmts.push(quote! {
                let __reg = #reg0.to_index() as u64;
                if #rex_always {
                    __bytes.push((0x48u64 | ((__reg >> 3) & 1)) as u8);
                } else if (__reg & 8) != 0 {
                    __bytes.push(0x41u8);
                }
            });
            if let Some(esc) = &info.form.escape {
                for e in esc {
                    stmts.push(quote! { __bytes.push(#e as u8); });
                }
            }
            stmts.push(quote! { __bytes.push((#base | (__reg & 7)) as u8); });
            if ctx.imm_bytes > 0 {
                let imm_fid = info
                    .operands
                    .iter()
                    .find(|(_, _, s, _)| s.kind == OperandKind::Imm)
                    .map(|(_, fid, _, _)| fid.clone())
                    .ok_or_else(|| {
                        format!(
                            "[[instructions.{}]]: form imm requires an imm operand",
                            info.inst.name
                        )
                    })?;
                let le_bytes = match ctx.imm_bytes {
                    1 => quote! { (*#imm_fid as u8).to_le_bytes().to_vec() },
                    2 => quote! { (*#imm_fid as u16).to_le_bytes().to_vec() },
                    4 => quote! { (*#imm_fid as u32).to_le_bytes().to_vec() },
                    8 => quote! { (*#imm_fid as u64).to_le_bytes().to_vec() },
                    n => return Err(format!("unsupported imm width {n} bytes")),
                };
                stmts.push(quote! { __bytes.extend_from_slice(&#le_bytes); });
            }
        } else if let Some(evex) = &ctx.evex {
            // EVEX（AVX-512）：62 + P0/P1/P2 + opcode + ModRM（reg-reg / 内存）。
            let vform = info.form;
            let reg = ctx.reg_expr.as_ref().unwrap();
            let rm = ctx.rm_expr.as_ref().unwrap();
            let vopcode = info.inst.opcode.unwrap();
            let map = &evex.map_expr;
            let pp = &evex.pp_expr;
            let w = &evex.w_expr;
            let l_val = evex.l_val;
            let b_val = evex.b_val;
            let z_val = evex.z_val;
            let aaa_expr: TokenStream = match &evex.aaa_expr {
                Some(e) => e.clone(),
                None => quote! { 0u8 },
            };
            let scale = evex.disp_scale as i64;
            let has_src = evex.has_src;
            let is_mem = ctx.modrm.map(|k| k.is_mem()).unwrap_or(false);
            // 内存：MemRef 操作数 fid（SIB index/scale 用）
            let mem_fid = if ctx.modrm == Some(ModrmKind::MemRefOp) {
                info.operands
                    .iter()
                    .find(|(_, _, s, _)| s.kind == OperandKind::Mem)
                    .map(|(_, fid, _, _)| fid.clone())
            } else {
                None
            };
            let idx_bind: TokenStream = if let Some(mf) = &mem_fid {
                quote! {
                    let __idx: u8 = match #mf.index {
                        Some(r) => r.to_index() as u8,
                        None => 4,
                    };
                    let __sc: u8 = match #mf.scale {
                        2 => 1, 4 => 2, 8 => 3, _ => 0,
                    };
                }
            } else {
                quote! { let __idx: u8 = 4; let __sc: u8 = 0; }
            };
            let vv_idx = if vform.modrm.as_deref() == Some("rr_src2") {
                1
            } else {
                2
            };
            let vv_expr: TokenStream = if has_src {
                let vv_fid = info.operands[vv_idx].1.clone();
                quote! { (!((#vv_fid.to_index() as u64) as u8 & 0x0F)) & 0x0F }
            } else {
                quote! { 0x0F }
            };
            // V' = ~(vvvv bit4)；无源 → 1
            let vp_expr: TokenStream = if has_src {
                let vv_fid = info.operands[vv_idx].1.clone();
                quote! { if ((#vv_fid.to_index() as u64) & 0x10) != 0 { 0u8 } else { 1u8 } }
            } else {
                quote! { 1u8 }
            };
            // ModRM 发射：reg-reg（mod=11）或内存（mod≠3 + 压缩位移 disp/scale）
            let modrm_emit: TokenStream = if is_mem {
                let force = mem_force_disp(model, info)?;
                let force_toks: Vec<TokenStream> = force.iter().map(|&b| quote! { #b }).collect();
                let disp = ctx.disp_expr.as_ref().unwrap();
                quote! {
                    let __disp = #disp;
                    const FORCE_DISP: &[u8] = &[#(#force_toks),*];
                    let __force = FORCE_DISP.contains(&(__rm as u8));
                    // 压缩位移：mod=1 的 disp8 = 实际位移/scale（须整除且缩放后
                    // 在 i8 范围）；mod=2 的 disp32 不缩放（x86 EVEX 规则）。
                    let __mod: u8 = if __disp == 0 && !__force {
                        0
                    } else if __disp % #scale == 0
                        && (-128i64 <= __disp / (#scale as i64)
                            && __disp / (#scale as i64) <= 127i64)
                    {
                        1
                    } else {
                        2
                    };
                    let __sib = (__rm & 7) == 4 || __idx != 4;
                    if __sib {
                        __bytes.push((((__mod as u64) << 6) | ((__reg & 7) << 3) | 4) as u8);
                        __bytes.push((((__sc as u64) << 6)
                            | (((__idx & 7) as u64) << 3) | (__rm & 7)) as u8);
                    } else {
                        __bytes.push((((__mod as u64) << 6) | ((__reg & 7) << 3) | (__rm & 7)) as u8);
                    }
                    if __mod == 1 {
                        __bytes.push((__disp / (#scale as i64)) as u8);
                    } else if __mod == 2 {
                        __bytes.extend_from_slice(&(__disp as u32).to_le_bytes());
                    }
                }
            } else {
                quote! { __bytes.push(((3u64 << 6) | ((__reg & 7) << 3) | (__rm & 7)) as u8); }
            };
            stmts.push(quote! {
                let __reg = #reg;
                let __rm = #rm;
                #idx_bind
                __bytes.push(0x62u8);
                // P0: R'(7) X'(6) B'(5) R(4) 0(3) 0(2) mm(1-0)；X' = ~index bit3（无索引 → 1）
                let __b: u8 = if (__rm & 8) != 0 { 0 } else { 1 };
                let __x: u8 = if (__idx & 8) != 0 { 0 } else { 1 };
                let __r: u8 = if (__reg & 8) != 0 { 0 } else { 1 };
                let __r4: u8 = if (__reg & 16) != 0 { 0 } else { 1 };
                __bytes.push(((__r << 7) | (__x << 6) | (__b << 5) | (__r4 << 4)
                    | ((#map as u8) & 0x3)) as u8);
                // P1: W(7) vvvv(6-3) 1(2) pp(1-0)
                let __w = #w;
                let __vvvv: u8 = #vv_expr;
                __bytes.push((((__w as u8) << 7) | (__vvvv << 3) | (1u8 << 2)
                    | (#pp as u8 & 0x3)) as u8);
                // P2: z(7) L'L(6-5) b(4) V'(3) aaa(2-0)——z 零掩码 / aaa 掩码索引
                let __vp: u8 = #vp_expr;
                let __aaa: u8 = #aaa_expr;
                __bytes.push((((#z_val as u8) << 7) | ((#l_val as u8) << 5)
                    | ((#b_val as u8) << 4) | (__vp << 3) | (__aaa & 7)) as u8);
                __bytes.push(#vopcode as u8);
                #modrm_emit
            });
            // 尾部立即数（EVEX imm8 等）
            if ctx.imm_bytes > 0 {
                let imm_fid = info
                    .operands
                    .iter()
                    .find(|(_, _, s, _)| s.kind == OperandKind::Imm)
                    .map(|(_, fid, _, _)| fid.clone())
                    .ok_or_else(|| {
                        format!(
                            "[[instructions.{}]]: form imm requires an imm operand",
                            info.inst.name
                        )
                    })?;
                let le_bytes = match ctx.imm_bytes {
                    1 => quote! { (*#imm_fid as u8).to_le_bytes().to_vec() },
                    2 => quote! { (*#imm_fid as u16).to_le_bytes().to_vec() },
                    4 => quote! { (*#imm_fid as u32).to_le_bytes().to_vec() },
                    8 => quote! { (*#imm_fid as u64).to_le_bytes().to_vec() },
                    n => return Err(format!("unsupported imm width {n} bytes")),
                };
                stmts.push(quote! { __bytes.extend_from_slice(&#le_bytes); });
            }
        } else if let Some(vex) = &ctx.vex {
            let vform = info.form;
            let reg = ctx.reg_expr.as_ref().unwrap();
            let rm = ctx.rm_expr.as_ref().unwrap();
            // VEX 内存（rm_memref）：MemRef 操作数 fid（SIB index/scale 用）。
            let vex_mem_fid = if ctx.modrm == Some(ModrmKind::MemRefOp) {
                info.operands
                    .iter()
                    .find(|(_, _, s, _)| s.kind == OperandKind::Mem)
                    .map(|(_, fid, _, _)| fid.clone())
            } else {
                None
            };
            let vex_idx_bind: TokenStream = if let Some(mf) = &vex_mem_fid {
                quote! {
                    let __idx: u8 = match #mf.index {
                        Some(r) => r.to_index() as u8,
                        None => 4,
                    };
                    let __sc: u8 = match #mf.scale {
                        2 => 1, 4 => 2, 8 => 3, _ => 0,
                    };
                }
            } else {
                quote! { let __idx: u8 = 4; let __sc: u8 = 0; }
            };
            let vopcode = info.inst.opcode.unwrap();
            let map = &vex.map_expr;
            let pp = &vex.pp_expr;
            let w = &vex.w_expr;
            let l = &vex.l_expr;
            let has_src = vex.has_src;
            // vvvv 源：rr_src2（VINSERTF128 等）→ ~op1（src1）；其余 → ~op2（src2）。
            // x86：VINSERTF128 的 vvvv=~src1、VADDPS 的 vvvv=~src2。
            let vv_idx = if vform.modrm.as_deref() == Some("rr_src2") {
                1
            } else {
                2
            };
            let vv_expr: TokenStream = if has_src {
                let vv_fid = info.operands[vv_idx].1.clone();
                quote! { (!((#vv_fid.to_index() as u64) as u8 & 0x0F)) & 0x0F }
            } else {
                quote! { 0x0F }
            };
            // VEX 内存形式（mod≠3）：base=rm，mod/disp 与 SIB 逻辑与非 VEX 内存一致。
            let mem_emit: TokenStream = if ctx.modrm.map(|k| k.is_mem()).unwrap_or(false) {
                let force = mem_force_disp(model, info)?;
                let force_toks: Vec<TokenStream> = force.iter().map(|&b| quote! { #b }).collect();
                let disp = ctx.disp_expr.as_ref().unwrap();
                quote! {
                    let __disp = #disp;
                    const FORCE_DISP: &[u8] = &[#(#force_toks),*];
                    let __force = FORCE_DISP.contains(&(__rm as u8));
                    let __mod: u8 = if __disp == 0 && !__force {
                        0
                    } else if (-128i64..=127i64).contains(&__disp) {
                        1
                    } else {
                        2
                    };
                    let __sib = (__rm & 7) == 4 || __idx != 4;
                    if __sib {
                        __bytes.push((((__mod as u64) << 6) | ((__reg & 7) << 3) | 4) as u8);
                        __bytes.push((((__sc as u64) << 6)
                            | (((__idx & 7) as u64) << 3) | (__rm & 7)) as u8);
                    } else {
                        __bytes.push((((__mod as u64) << 6) | ((__reg & 7) << 3) | (__rm & 7)) as u8);
                    }
                    if __mod == 1 {
                        __bytes.push(__disp as u8);
                    } else if __mod == 2 {
                        __bytes.extend_from_slice(&(__disp as u32).to_le_bytes());
                    }
                }
            } else {
                quote! { __bytes.push(((3u64 << 6) | ((__reg & 7) << 3) | (__rm & 7)) as u8); }
            };
            stmts.push(quote! {
                let __reg = #reg;
                let __rm = #rm;
                #vex_idx_bind
                __bytes.push(0xC4u8);
                let __b: u8 = if (__rm & 8) != 0 { 0 } else { 1 };
                let __x: u8 = 1;
                let __r: u8 = if (__reg & 8) != 0 { 0 } else { 1 };
                __bytes.push(((__r << 7) | (__x << 6) | (__b << 5) | ((#map as u8) & 0x1F)) as u8);
                let __w = #w;
                let __vvvv: u8 = #vv_expr;
                __bytes.push((((__w as u8) << 7) | (__vvvv << 3)
                    | ((#l as u8 & 0x1) << 2) | (#pp as u8 & 0x3)) as u8);
                __bytes.push(#vopcode as u8);
                #mem_emit
            });
            // 尾部立即数（VEX imm8 等）
            if ctx.imm_bytes > 0 {
                let imm_fid = info
                    .operands
                    .iter()
                    .find(|(_, _, s, _)| s.kind == OperandKind::Imm)
                    .map(|(_, fid, _, _)| fid.clone())
                    .ok_or_else(|| {
                        format!(
                            "[[instructions.{}]]: form imm requires an imm operand",
                            info.inst.name
                        )
                    })?;
                let le_bytes = match ctx.imm_bytes {
                    1 => quote! { (*#imm_fid as u8).to_le_bytes().to_vec() },
                    2 => quote! { (*#imm_fid as u16).to_le_bytes().to_vec() },
                    4 => quote! { (*#imm_fid as u32).to_le_bytes().to_vec() },
                    8 => quote! { (*#imm_fid as u64).to_le_bytes().to_vec() },
                    n => return Err(format!("unsupported imm width {n} bytes")),
                };
                stmts.push(quote! { __bytes.extend_from_slice(&#le_bytes); });
            }
        } else if ctx.modrm.is_none() {
            // 无 ModRM 的变长形式（JMP/CALL rel32、RET、CQO 等）：
            // [REX.W] + prefix + escape + opcode + imm。
            stmts.push(opsize_bind);
            stmts.extend(width_checks.iter().cloned());
            if ctx.rex_w_always {
                stmts.push(quote! { __bytes.push(0x48u8); });
            }
            let prefix_expr = &ctx.prefix_expr;
            stmts.push(quote! {
                let __p = #prefix_expr;
                if __p != 0 { __bytes.push(__p); }
            });
            if let Some(esc) = &info.form.escape {
                for e in esc {
                    stmts.push(quote! { __bytes.push(#e as u8); });
                }
            }
            let opcode = info.inst.opcode.unwrap();
            if let Some((_, cond_fid, _, _)) = info
                .operands
                .iter()
                .find(|(_, _, s, _)| s.kind == OperandKind::Cond)
            {
                stmts.push(quote! { __bytes.push((#opcode | (*#cond_fid as u64 & 0xF)) as u8); });
            } else {
                stmts.push(quote! { __bytes.push(#opcode as u8); });
            }
            // 固定 ModRM 字节（无操作数指令：MFENCE 0F AE F0）
            if let Some(fm) = ctx.modrm_fixed {
                stmts.push(quote! { __bytes.push(#fm as u8); });
            }
            // 尾部立即数（form.imm：rel32 等）
            if ctx.imm_bytes > 0 {
                let imm_fid = info
                    .operands
                    .iter()
                    .find(|(_, _, s, _)| s.kind == OperandKind::Imm || s.kind == OperandKind::Label)
                    .map(|(_, fid, _, _)| fid.clone())
                    .ok_or_else(|| {
                        format!(
                            "[[instructions.{}]]: form imm requires an imm/label operand",
                            info.inst.name
                        )
                    })?;
                let le_bytes = match ctx.imm_bytes {
                    1 => quote! { (*#imm_fid as u8).to_le_bytes().to_vec() },
                    2 => quote! { (*#imm_fid as u16).to_le_bytes().to_vec() },
                    4 => quote! { (*#imm_fid as u32).to_le_bytes().to_vec() },
                    8 => quote! { (*#imm_fid as u64).to_le_bytes().to_vec() },
                    n => return Err(format!("unsupported imm width {n} bytes")),
                };
                stmts.push(quote! { __bytes.extend_from_slice(&#le_bytes); });
            }
        } else if let Some(modrm) = ctx.modrm {
            let reg = ctx.reg_expr.as_ref().unwrap();
            let rm = ctx.rm_expr.as_ref().unwrap();
            // MemRefOp 形式的 MemRef 操作数 fid（SIB index/scale 用）。
            let mem_fid = if modrm == ModrmKind::MemRefOp {
                info.operands
                    .iter()
                    .find(|(_, _, s, _)| s.kind == OperandKind::Mem)
                    .map(|(_, fid, _, _)| fid.clone())
            } else {
                None
            };
            stmts.push(opsize_bind);
            stmts.extend(width_checks.iter().cloned());
            if ctx.has_opsize {
                stmts.push(quote! { if __opsize == 2 { __bytes.push(0x66u8); } });
            }
            let prefix_expr = &ctx.prefix_expr;
            let prefix_push = quote! {
                let __p = #prefix_expr;
                if __p != 0 { __bytes.push(__p); }
            };
            stmts.push(prefix_push);
            let rex_w = &ctx.rex_w_expr;
            // REX 发出条件：有 opsize → opsize==64 或扩展寄存器；无 opsize（SSE）→
            // rex_w 显式（ROUNDSD 等 w=1 恒需 REX.W）或扩展寄存器。
            // 8 位寄存器索引 4-7（spl/bpl/sil/dil）无 REX 编码为 ah/ch/dh/bh → 强制 REX。
            // SIB index 高半（R12 索引）→ REX.X。
            let rex_cond = if ctx.has_opsize {
                let b8 = if ctx.reg_is_byte || ctx.rm_is_byte {
                    quote! { || (#reg & 7) >= 4 || (#rm & 7) >= 4 }
                } else {
                    quote! {}
                };
                quote! { __opsize == 8 || (#reg & 8) != 0 || (#rm & 8) != 0 #b8 || (__idx & 8) != 0 }
            } else {
                let b8 = if ctx.reg_is_byte || ctx.rm_is_byte {
                    quote! { || (#reg & 7) >= 4 || (#rm & 7) >= 4 }
                } else {
                    quote! {}
                };
                quote! { __rex_w != 0 || (#reg & 8) != 0 || (#rm & 8) != 0 #b8 || (__idx & 8) != 0 }
            };
            // SIB index/scale 绑定（MemRefOp：来自 MemRef.index/scale；其余：无索引）
            let idx_bind: TokenStream = if let Some(mf) = &mem_fid {
                quote! {
                    let __idx: u8 = match #mf.index {
                        Some(r) => r.to_index() as u8,
                        None => 4,
                    };
                    let __sc: u8 = match #mf.scale {
                        2 => 1, 4 => 2, 8 => 3, _ => 0,
                    };
                }
            } else {
                quote! {
                    let __idx: u8 = 4;
                    let __sc: u8 = 0;
                }
            };
            stmts.push(quote! {
                let __reg = #reg;
                let __rm = #rm;
                #idx_bind
                let __rex_w = #rex_w;
                if #rex_cond {
                    let __rex: u8 = ((0x40u64 | (__rex_w << 3)
                        | ((__reg >> 3) & 1) << 2 | (((__idx as u64) >> 3) & 1) << 1
                        | ((__rm >> 3) & 1)) & 0xFF) as u8;
                    __bytes.push(__rex);
                }
            });
            // escape + opcode（cond 操作数 → opcode 低 4 位：JCC/SETCC/CMOVCC）
            if let Some(esc) = &info.form.escape {
                for e in esc {
                    stmts.push(quote! { __bytes.push(#e as u8); });
                }
            }
            let opcode = info.inst.opcode.unwrap();
            if let Some((_, cond_fid, _, _)) = info
                .operands
                .iter()
                .find(|(_, _, s, _)| s.kind == OperandKind::Cond)
            {
                stmts.push(quote! { __bytes.push((#opcode | (*#cond_fid as u64 & 0xF)) as u8); });
            } else {
                stmts.push(quote! { __bytes.push(#opcode as u8); });
            }
            // ModRM：mod=11（rr/ext）或内存（rm_mem/rm_memref）
            if modrm.is_mem() {
                let force = mem_force_disp(model, info)?;
                let force_toks: Vec<TokenStream> = force.iter().map(|&b| quote! { #b }).collect();
                let disp = ctx.disp_expr.as_ref().unwrap();
                stmts.push(quote! {
                    let __disp = #disp;
                    const FORCE_DISP: &[u8] = &[#(#force_toks),*];
                    let __force = FORCE_DISP.contains(&(__rm as u8));
                    let __mod: u8 = if __disp == 0 && !__force {
                        0
                    } else if (-128i64..=127i64).contains(&__disp) {
                        1
                    } else {
                        2
                    };
                    let __sib = (__rm & 7) == 4 || __idx != 4;
                    if __sib {
                        __bytes.push((((__mod as u64) << 6) | ((__reg & 7) << 3) | 4) as u8);
                        __bytes.push((((__sc as u64) << 6)
                            | (((__idx & 7) as u64) << 3) | (__rm & 7)) as u8);
                    } else {
                        __bytes.push((((__mod as u64) << 6) | ((__reg & 7) << 3) | (__rm & 7)) as u8);
                    }
                    if __mod == 1 {
                        __bytes.push(__disp as u8);
                    } else if __mod == 2 {
                        __bytes.extend_from_slice(&(__disp as u32).to_le_bytes());
                    }
                });
            } else {
                stmts.push(quote! {
                    __bytes.push(((3u64 << 6) | ((__reg & 7) << 3) | (__rm & 7)) as u8);
                });
            }
            // 尾部立即数（form.imm）
            if ctx.imm_bytes > 0 {
                let imm_fid = info
                    .operands
                    .iter()
                    .find(|(_, _, s, _)| s.kind == OperandKind::Imm)
                    .map(|(_, fid, _, _)| fid.clone())
                    .ok_or_else(|| {
                        format!(
                            "[[instructions.{}]]: form imm requires an imm operand",
                            info.inst.name
                        )
                    })?;
                let le_bytes = match ctx.imm_bytes {
                    1 => quote! { (*#imm_fid as u8).to_le_bytes().to_vec() },
                    2 => quote! { (*#imm_fid as u16).to_le_bytes().to_vec() },
                    4 => quote! { (*#imm_fid as u32).to_le_bytes().to_vec() },
                    8 => quote! { (*#imm_fid as u64).to_le_bytes().to_vec() },
                    n => return Err(format!("unsupported imm width {n} bytes")),
                };
                stmts.push(quote! { __bytes.extend_from_slice(&#le_bytes); });
            }
        }
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
        arms.push(quote! {
            #pat => {
                let mut __bytes: Vec<u8> = Vec::new();
                #(#stmts)*
                Ok(__bytes)
            }
        });
    }
    Ok(quote! {
        /// 编码单条指令为字节序列（变长 ISA）。
        pub fn encode(inst: &Inst) -> Result<Vec<u8>, String> {
            match inst {
                #(#arms,)*
                Inst::Raw(bytes) => Ok(bytes.clone()),
            }
        }
    })
}

/// `+r` 形式解码 arm：REX 扫描后 [escape] `opcode_reg|reg&7` [imm]。
fn gen_vlen_opcode_reg_decode_arm(
    info: &InstInfo,
    ctx: &VlenCtx,
    base: u64,
    endian: Endian,
) -> Result<TokenStream, String> {
    let vn = &info.vn;
    let imm_bytes = ctx.imm_bytes;
    let mut off = 0usize;
    let mut conds: Vec<TokenStream> = Vec::new();
    if let Some(esc) = &info.form.escape {
        for e in esc {
            conds.push(quote! { bytes[__o + #off] == #e as u8 });
            off += 1;
        }
    }
    // opcode 高 5 位匹配（低 3 位 = reg）
    conds.push(quote! { (bytes[__o + #off] & 0xF8) == ((#base & 0xF8) as u8) });
    let opcode_off = off;
    let total = off + 1 + imm_bytes;
    // 字段提取：op0 = reg（opcode&7 | REX.B<<3）；后续 imm 操作数
    let mut binds: Vec<TokenStream> = Vec::new();
    let mut ctor_fields: Vec<TokenStream> = Vec::new();
    for (i, (_, fid, slot, _)) in info.operands.iter().enumerate() {
        let expr: TokenStream = match slot.kind {
            OperandKind::Reg if i == 0 => field_ctor_expr(
                slot,
                quote! { ((bytes[__o + #opcode_off] & 7) as u32) | (__rex_b << 3) },
            ),
            OperandKind::Imm => {
                let raw = imm_read_ts(opcode_off + 1, imm_bytes, endian);
                let signed = slot.signed.unwrap_or(false);
                let w = slot.width.unwrap_or(64);
                if signed {
                    sign_extend_ts(raw, w)
                } else {
                    quote! { #raw as i64 }
                }
            }
            _ => {
                return Err(format!(
                    "[[instructions.{}]]: +r operand {i} kind {:?} unsupported",
                    info.inst.name, slot.kind
                ));
            }
        };
        binds.push(quote! { let #fid = #expr; });
        ctor_fields.push(quote! { #fid });
    }
    let ctor = if info.operands.is_empty() {
        quote! { Inst::#vn }
    } else {
        quote! { Inst::#vn { #(#ctor_fields),* } }
    };
    let cond = if conds.is_empty() {
        quote! { true }
    } else {
        quote! { #(#conds)&&* }
    };
    Ok(quote! {
        if __o + #total <= bytes.len() && #cond {
            #(#binds)*
            return Some((#ctor, __o + #total));
        }
    })
}

/// VEX 解码 arm：C4 + vex2/vex3（map/pp/w/l guard）→ opcode → ModRM（mod=11）。
fn gen_vlen_vex_decode_arm(
    info: &InstInfo,
    ctx: &VlenCtx,
    endian: Endian,
) -> Result<TokenStream, String> {
    let vn = &info.vn;
    let form = info.form;
    let vex = ctx.vex.as_ref().unwrap();
    let opcode = info.inst.opcode.unwrap();
    let imm_bytes = ctx.imm_bytes;
    let total = 5 + imm_bytes;
    let is_mem = ctx.modrm.map(|k| k.is_mem()).unwrap_or(false);
    // vex guard 常量：数字或 fields.vex_*（与 encode 的 vs_value 一致）
    let vex_val = |vs_key: &str, field_key: &str| -> u64 {
        match vs_value(info.form.vex.as_ref().unwrap(), vs_key) {
            Some(v) => v,
            None => info
                .inst
                .fields
                .as_ref()
                .and_then(|f| f.get(field_key))
                .copied()
                .unwrap_or(0),
        }
    };
    let map_v = vex_val("map", "vex_map");
    let pp_v = vex_val("pp", "vex_pp");
    let w_v = vex_val("w", "vex_w");
    let l_v = vex_val("l", "vex_l");
    let has_src = vex.has_src;
    // 字段提取：reg=op0（R 来自 vex2）、rm=op1（B 来自 vex2）、vvvv=op2（有源）。
    // rr_src2（VINSERTF128）：rm=op2（B）、vvvv=op1。
    // 内存形式（rm_memref）：op0=reg、op1=MemRef（base=rm|B、disp）。
    let is_src2 = form.modrm.as_deref() == Some("rr_src2");
    let mut binds: Vec<TokenStream> = Vec::new();
    let mut ctor_fields: Vec<TokenStream> = Vec::new();
    for (i, (_, fid, slot, _)) in info.operands.iter().enumerate() {
        let expr: TokenStream = match slot.kind {
            OperandKind::Reg => match i {
                0 => field_ctor_expr(slot, quote! { ((__modrm >> 3) & 7) as u32 | (__r << 3) }),
                1 if is_src2 => field_ctor_expr(slot, quote! { __vvvv as u32 }),
                1 if is_mem => {
                    // VEX 内存：op1 是 Mem 槽（MemRefOp），由 __base/__disp 构造
                    return Err(format!(
                        "[[instructions.{}]]: VEX mem form with reg operand at position 1 (expected mem slot)",
                        info.inst.name
                    ));
                }
                1 => field_ctor_expr(slot, quote! { (__modrm & 7) as u32 | (__b << 3) }),
                2 if is_src2 => field_ctor_expr(slot, quote! { (__modrm & 7) as u32 | (__b << 3) }),
                2 if has_src => field_ctor_expr(slot, quote! { __vvvv as u32 }),
                _ => {
                    return Err(format!(
                        "[[instructions.{}]]: VEX reg operand {i} position unsupported",
                        info.inst.name
                    ));
                }
            },
            OperandKind::Mem => {
                if !is_mem {
                    return Err(format!(
                        "[[instructions.{}]]: VEX mem operand {i} requires a mem modrm form",
                        info.inst.name
                    ));
                }
                quote! { MemRef {
                    base: <Reg as TryFrom<RegRef>>::try_from(RegRef::new(RegClass::GPR(8), __base)).unwrap(),
                    disp: __disp,
                    index: __index_reg,
                    scale: __scale,
                } }
            }
            OperandKind::Imm => {
                let raw = imm_read_ts(5, imm_bytes, endian);
                let signed = slot.signed.unwrap_or(false);
                let w = slot.width.unwrap_or(32);
                if signed {
                    sign_extend_ts(raw, w)
                } else {
                    quote! { #raw as i64 }
                }
            }
            _ => {
                return Err(format!(
                    "[[instructions.{}]]: VEX operand kind {:?} unsupported",
                    info.inst.name, slot.kind
                ));
            }
        };
        binds.push(quote! { let #fid = #expr; });
        ctor_fields.push(quote! { #fid });
    }
    let ctor = if info.operands.is_empty() {
        quote! { Inst::#vn }
    } else {
        quote! { Inst::#vn { #(#ctor_fields),* } }
    };
    // 寄存器形式（mod=11）与内存形式（mod≠3）共用 vex guard；长度不同。
    let body: TokenStream = if is_mem {
        quote! {
            let __modrm = bytes[__o + 4];
            let __mod = __modrm >> 6;
            if __mod != 3 && !(__mod == 0 && (__modrm & 7) == 5) {
                let __r: u32 = ((!((__vex2 >> 7) & 1)) & 1) as u32;
                let __b: u32 = ((!((__vex2 >> 5) & 1)) & 1) as u32;
                let __x: u32 = ((!((__vex2 >> 6) & 1)) & 1) as u32;
                let mut __o2 = __o + 5;
                let mut __base: u32 = ((__modrm & 7) as u32) | (__b << 3);
                let mut __index_reg: Option<Reg> = None;
                let mut __scale: u8 = 1;
                let mut __sib_ok = true;
                if (__modrm & 7) == 4 {
                    if __o2 < bytes.len() {
                        let __sib_byte = bytes[__o2];
                        __o2 += 1;
                        let __idx4: u32 = ((__sib_byte >> 3) & 7) as u32;
                        let __idx_full: u32 = __idx4 | (__x << 3);
                        let __sc_bits = (__sib_byte >> 6) & 3;
                        __scale = match __sc_bits { 1 => 2, 2 => 4, 3 => 8, _ => 1 };
                        __base = ((__sib_byte & 7) as u32) | (__b << 3);
                        if __idx_full == 4 {
                            __index_reg = None;
                        } else {
                            __index_reg = Some(
                                <Reg as TryFrom<RegRef>>::try_from(
                                    RegRef::new(RegClass::GPR(8), __idx_full),
                                ).unwrap(),
                            );
                        }
                    } else {
                        __sib_ok = false;
                    }
                }
                let mut __disp: i64 = 0;
                let mut __disp_ok = true;
                if __mod == 1 {
                    if __o2 < bytes.len() {
                        __disp = (bytes[__o2] as i8) as i64;
                        __o2 += 1;
                    } else {
                        __disp_ok = false;
                    }
                } else if __mod == 2 {
                    if __o2 + 3 < bytes.len() {
                        __disp = i32::from_le_bytes([
                            bytes[__o2],
                            bytes[__o2 + 1],
                            bytes[__o2 + 2],
                            bytes[__o2 + 3],
                        ]) as i64;
                        __o2 += 4;
                    } else {
                        __disp_ok = false;
                    }
                }
                if __sib_ok && __disp_ok {
                    #(#binds)*
                    return Some((#ctor, __o2 + #imm_bytes));
                }
            }
        }
    } else {
        quote! {
            let __modrm = bytes[__o + 4];
            if (__modrm >> 6) == 3 {
                let __r: u32 = ((!((__vex2 >> 7) & 1)) & 1) as u32;
                let __b: u32 = ((!((__vex2 >> 5) & 1)) & 1) as u32;
                let __vvvv: u32 = ((!((__vex3 >> 3) & 0xF)) & 0xF) as u32;
                #(#binds)*
                return Some((#ctor, __o + #total));
            }
        }
    };
    Ok(quote! {
        if __o + #total <= bytes.len() && bytes[__o] == 0xC4 {
            let __vex2 = bytes[__o + 1];
            let __vex3 = bytes[__o + 2];
            if ((__vex2 & 0x1F) as u64) == #map_v
                && ((__vex3 & 0x3) as u64) == #pp_v
                && (((__vex3 >> 7) & 1) as u64) == #w_v
                && (((__vex3 >> 2) & 1) as u64) == #l_v
                && bytes[__o + 3] == #opcode as u8
            {
                #body
            }
        }
    })
}

// ─────────────────── 变长 decode：字节前缀决策树 ───────────────────

/// EVEX 解码 arm：62 + P0/P1/P2（mm/pp/W/L'L/aaa guard）+ opcode + ModRM（mod=11）。
fn gen_vlen_evex_decode_arm(
    info: &InstInfo,
    ctx: &VlenCtx,
    endian: Endian,
) -> Result<TokenStream, String> {
    let vn = &info.vn;
    let form = info.form;
    let evex = ctx.evex.as_ref().unwrap();
    let opcode = info.inst.opcode.unwrap();
    let imm_bytes = ctx.imm_bytes;
    let total = 6 + imm_bytes;
    let vex_val = |vs_key: &str, field_key: &str| -> u64 {
        match vs_value(info.form.evex.as_ref().unwrap(), vs_key) {
            Some(v) => v,
            None => info
                .inst
                .fields
                .as_ref()
                .and_then(|f| f.get(field_key))
                .copied()
                .unwrap_or(0),
        }
    };
    let map_v = vex_val("map", "evex_map");
    let pp_v = vex_val("pp", "evex_pp");
    let w_v = vex_val("w", "evex_w");
    let l_v = evex.l_val;
    let b_v = evex.b_val;
    let scale = evex.disp_scale;
    let has_src = evex.has_src;
    let is_mem = ctx.modrm.map(|k| k.is_mem()).unwrap_or(false);
    let is_src2 = form.modrm.as_deref() == Some("rr_src2");
    // 是否有 opmask 操作数（kreg 类槽）：有 → 不要求 aaa==0，且提取 __aaa
    let has_mask = info.operands.iter().any(|(_, _, s, _)| {
        s.kind == OperandKind::Reg
            && s.class
                .as_ref()
                .is_some_and(|c| matches!(c, RegClass::KReg(_)))
    });
    // 零掩码 z 位（仅掩码形式语义化；无掩码时 z 必须为 0）
    let z_v = evex.z_val;
    // aaa guard：无掩码操作数 → 要求 aaa==0；有 → 不检查（接受 k0-k7）
    let aaa_guard: TokenStream = if has_mask {
        quote! { true }
    } else {
        quote! { ((__p2 as u64) & 0x7) == 0 }
    };
    // z guard：指令声明 z=1 → 要求 P2 z 位为 1；否则要求为 0（z 位与掩码
    // 形式绑定，由指令声明决定，避免 decode 歧义）。
    let z_guard: TokenStream = if z_v == 1 {
        quote! { ((((__p2 as u64) >> 7) & 1) == 1) }
    } else {
        quote! { ((((__p2 as u64) >> 7) & 1) == 0) }
    };
    let mut binds: Vec<TokenStream> = Vec::new();
    let mut ctor_fields: Vec<TokenStream> = Vec::new();
    for (i, (_, fid, slot, _)) in info.operands.iter().enumerate() {
        let expr: TokenStream = match slot.kind {
            OperandKind::Reg
                if slot
                    .class
                    .as_ref()
                    .is_some_and(|c| matches!(c, RegClass::KReg(_))) =>
            {
                // opmask 掩码寄存器：取自 P2 的 aaa（bit2-0）
                field_ctor_expr(slot, quote! { __aaa as u32 })
            }
            OperandKind::Reg => match i {
                0 => field_ctor_expr(
                    slot,
                    quote! { ((__modrm >> 3) & 7) as u32 | (__r << 3) | (__r4 << 4) },
                ),
                1 if is_src2 => field_ctor_expr(slot, quote! { __vvvv as u32 }),
                1 if is_mem => {
                    return Err(format!(
                        "[[instructions.{}]]: EVEX mem form with reg operand at position 1 (expected mem slot)",
                        info.inst.name
                    ));
                }
                1 => field_ctor_expr(slot, quote! { (__modrm & 7) as u32 | (__b << 3) }),
                2 if is_src2 => field_ctor_expr(slot, quote! { (__modrm & 7) as u32 | (__b << 3) }),
                2 if has_src => field_ctor_expr(slot, quote! { __vvvv as u32 }),
                _ => {
                    return Err(format!(
                        "[[instructions.{}]]: EVEX reg operand {i} position unsupported",
                        info.inst.name
                    ));
                }
            },
            OperandKind::Mem => {
                quote! { MemRef {
                    base: <Reg as TryFrom<RegRef>>::try_from(RegRef::new(RegClass::GPR(8), __base)).unwrap(),
                    disp: __disp,
                    index: __index_reg,
                    scale: __scale,
                } }
            }
            OperandKind::Imm => {
                let raw = imm_read_ts(6, imm_bytes, endian);
                let signed = slot.signed.unwrap_or(false);
                let w = slot.width.unwrap_or(32);
                if signed {
                    sign_extend_ts(raw, w)
                } else {
                    quote! { #raw as i64 }
                }
            }
            _ => {
                return Err(format!(
                    "[[instructions.{}]]: EVEX operand kind {:?} unsupported",
                    info.inst.name, slot.kind
                ));
            }
        };
        binds.push(quote! { let #fid = #expr; });
        ctor_fields.push(quote! { #fid });
    }
    let ctor = if info.operands.is_empty() {
        quote! { Inst::#vn }
    } else {
        quote! { Inst::#vn { #(#ctor_fields),* } }
    };
    // 寄存器形式（mod=11）与内存形式（mod≠3 + SIB + 压缩位移回乘 scale）
    let body: TokenStream = if is_mem {
        quote! {
            let __modrm = bytes[__o + 5];
            let __mod = __modrm >> 6;
            if __mod != 3 && !(__mod == 0 && (__modrm & 7) == 5) {
                let __r: u32 = ((!((__p0 >> 7) & 1)) & 1) as u32;
                let __b: u32 = ((!((__p0 >> 5) & 1)) & 1) as u32;
                let __x: u32 = ((!((__p0 >> 6) & 1)) & 1) as u32;
                let __r4: u32 = ((!((__p0 >> 4) & 1)) & 1) as u32;
                let mut __o2 = __o + 6;
                let mut __base: u32 = ((__modrm & 7) as u32) | (__b << 3);
                let mut __index_reg: Option<Reg> = None;
                let mut __scale: u8 = 1;
                let mut __sib_ok = true;
                if (__modrm & 7) == 4 {
                    if __o2 < bytes.len() {
                        let __sib_byte = bytes[__o2];
                        __o2 += 1;
                        let __idx4: u32 = ((__sib_byte >> 3) & 7) as u32;
                        let __idx_full: u32 = __idx4 | (__x << 3);
                        let __sc_bits = (__sib_byte >> 6) & 3;
                        __scale = match __sc_bits { 1 => 2, 2 => 4, 3 => 8, _ => 1 };
                        __base = ((__sib_byte & 7) as u32) | (__b << 3);
                        if __idx_full == 4 {
                            __index_reg = None;
                        } else {
                            __index_reg = Some(
                                <Reg as TryFrom<RegRef>>::try_from(
                                    RegRef::new(RegClass::GPR(8), __idx_full),
                                ).unwrap(),
                            );
                        }
                    } else {
                        __sib_ok = false;
                    }
                }
                let mut __disp: i64 = 0;
                let mut __disp_ok = true;
                // 压缩位移：mod=1 disp8 × scale；mod=2 disp32 不缩放
                if __mod == 1 {
                    if __o2 < bytes.len() {
                        __disp = ((bytes[__o2] as i8) as i64) * (#scale as i64);
                        __o2 += 1;
                    } else {
                        __disp_ok = false;
                    }
                } else if __mod == 2 {
                    if __o2 + 3 < bytes.len() {
                        __disp = i32::from_le_bytes([
                            bytes[__o2],
                            bytes[__o2 + 1],
                            bytes[__o2 + 2],
                            bytes[__o2 + 3],
                        ]) as i64;
                        __o2 += 4;
                    } else {
                        __disp_ok = false;
                    }
                }
                if __sib_ok && __disp_ok {
                    #(#binds)*
                    return Some((#ctor, __o2 + #imm_bytes));
                }
            }
        }
    } else {
        quote! {
            let __modrm = bytes[__o + 5];
            if (__modrm >> 6) == 3 {
                let __r: u32 = ((!((__p0 >> 7) & 1)) & 1) as u32;
                let __b: u32 = ((!((__p0 >> 5) & 1)) & 1) as u32;
                let __r4: u32 = ((!((__p0 >> 4) & 1)) & 1) as u32;
                let __vvvv: u32 = ((!(((__p1 >> 3) & 0xF) as u32)) & 0xF)
                    | (((!((__p2 >> 3) & 1)) & 1) as u32) << 4;
                #(#binds)*
                return Some((#ctor, __o + #total));
            }
        }
    };
    Ok(quote! {
        if __o + #total <= bytes.len() && bytes[__o] == 0x62 {
            let __p0 = bytes[__o + 1];
            let __p1 = bytes[__o + 2];
            let __p2 = bytes[__o + 3];
            let __aaa: u32 = (__p2 & 7) as u32;
            if (((__p0 as u64) & 0x3) == #map_v)
                && (((__p1 as u64) & 0x3) == #pp_v)
                && ((((__p1 as u64) >> 7) & 1) == #w_v)
                && ((((__p2 as u64) >> 5) & 0x3) == #l_v)
                && ((((__p2 as u64) >> 4) & 1) == #b_v)
                && #aaa_guard
                && #z_guard
                && bytes[__o + 4] == #opcode as u8
            {
                #body
            }
        }
    })
}

/// 一条解码指令的常数字节 key：`(mask, value)` 序列（隐式 byte offset = level 序号，
/// 相对 `__o`，即共享前缀扫描结束后的位置）。last 条目可带掩码（+r 低 3 位 / 条件码低 4 位）。
fn vlen_decode_key(info: &InstInfo, ctx: &VlenCtx) -> Vec<(u8, u8)> {
    // EVEX：62 前缀（P0/P1/P2/opcode guard 留在 arm 内）。
    if ctx.evex.is_some() {
        return vec![(0xFF, 0x62)];
    }
    // VEX：C4 前缀（vex2/3/opcode guard 留在 arm 内）。
    if ctx.vex.is_some() {
        return vec![(0xFF, 0xC4)];
    }
    // `+r` 形式：escape + opcode 高 5 位（低 3 位 = 寄存器）。
    if let Some(base) = ctx.opcode_reg {
        let mut key = Vec::new();
        if let Some(esc) = &info.form.escape {
            for e in esc {
                key.push((0xFF, *e));
            }
        }
        key.push((0xF8, (base as u8) & 0xF8));
        return key;
    }
    let mut key = Vec::new();
    // 显式非扫描前缀字节（x86_v12 恒为 0；通用 ISA 兼容）。
    if let Some(p) = info.form.prefix.as_deref() {
        let v = if p == "field" {
            info.inst
                .fields
                .as_ref()
                .and_then(|f| f.get("prefix"))
                .copied()
                .unwrap_or(0)
        } else {
            parse_u64(p).unwrap_or(0)
        };
        // 扫描标志前缀（66/F0/F2/F3/无）不进 key；其他显式字节为首字节。
        if !matches!(v, 0 | 0x66 | 0xF0 | 0xF2 | 0xF3) {
            key.push((0xFF, v as u8));
        }
    }
    if let Some(esc) = &info.form.escape {
        for e in esc {
            key.push((0xFF, *e));
        }
    }
    let opcode = info.inst.opcode.unwrap();
    if info
        .operands
        .iter()
        .any(|(_, _, s, _)| s.kind == OperandKind::Cond)
    {
        // 条件码在 opcode 低 4 位：key 匹配高 4 位（JCC/SETCC/CMOVCC）。
        key.push((0xF0, (opcode as u8) & 0xF0));
    } else {
        key.push((0xFF, opcode as u8));
    }
    // 固定 ModRM 字节（MFENCE 0F AE F0 等）：作为最后一个 key 字节。
    if let Some(fm) = ctx.modrm_fixed {
        key.push((0xFF, fm as u8));
    }
    key
}

/// 解码决策树节点：leaf 带一/多条 arm（同 key 的 opcode 家族，声明序），内部节点为 `(mask,value)` 边。
struct DecTrieNode {
    arms: Vec<TokenStream>,
    edges: Vec<(u8, u8, usize)>,
}

fn build_dec_trie(groups: &[(Vec<(u8, u8)>, TokenStream)]) -> Vec<DecTrieNode> {
    let mut nodes = vec![DecTrieNode {
        arms: Vec::new(),
        edges: Vec::new(),
    }];
    for (key, arm) in groups {
        let mut idx = 0usize;
        for (mask, value) in key {
            let found = nodes[idx]
                .edges
                .iter()
                .position(|(m, v, _)| *m == *mask && *v == *value);
            idx = if let Some(e) = found {
                nodes[idx].edges[e].2
            } else {
                let child = nodes.len();
                nodes.push(DecTrieNode {
                    arms: Vec::new(),
                    edges: Vec::new(),
                });
                nodes[idx].edges.push((*mask, *value, child));
                child
            };
        }
        nodes[idx].arms.push(arm.clone());
    }
    nodes
}

/// 校验决策树各节点边的 (mask,value) 两两不相交。若两条边能同时命中同一字节
/// （且至少一条带掩码），则 else-if 链的次序可能与声明序首匹配不一致 → 拒绝。
/// 无重叠时每个字节至多命中一条边，next-match 语义与旧线性 if 链完全一致。
fn check_dec_trie_overlaps(nodes: &[DecTrieNode]) -> Result<(), String> {
    for (ni, node) in nodes.iter().enumerate() {
        for (i, (m1, v1, _)) in node.edges.iter().enumerate() {
            for (j, (m2, v2, _)) in node.edges.iter().enumerate().skip(i + 1) {
                // 存在字节 b 使 (b&m1)==v1 且 (b&m2)==v2 当且仅当两值在共同掩码位一致。
                let overlaps = (*m1 != 0xFF || *m2 != 0xFF) && ((*v1 & *m2) == (*v2 & *m1));
                if overlaps {
                    return Err(format!(
                        "v12 decode trie: node {ni} edges [{i}]({m1:02x}/{v1:02x}) and [{j}]({m2:02x}/{v2:02x}) overlap — ambiguous byte dispatch; make the encodings disjoint"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// 将 trie 展开为嵌套 `if`（else-if 链）。`off` = 当前字节相对 `__o` 的偏移。
/// x86_v12 各节点边无重叠掩码（由 `check_dec_trie_overlaps` 保证），故 else-if 链安全。
fn emit_dec_trie(nodes: &[DecTrieNode], idx: usize, off: usize) -> TokenStream {
    let node = &nodes[idx];
    let mut body = TokenStream::new();
    // leaf arm 先试（声明序）；arm 内部 return，未命中的自然落到子节点。
    for arm in &node.arms {
        body.extend(arm.clone());
    }
    if !node.edges.is_empty() {
        let depth = off + 1;
        let mut chain = quote! {};
        for (mask, value, child) in node.edges.iter().rev() {
            let sub = emit_dec_trie(nodes, *child, off + 1);
            chain = quote! {
                if (__b & #mask) == #value {
                    #sub
                } else {
                    #chain
                }
            };
        }
        body.extend(quote! {
            if __o + #depth <= bytes.len() {
                let __b = bytes[__o + #off];
                #chain
            }
        });
    }
    body
}

/// 变长 decode：共享前缀扫描 + 字节前缀决策树（替代线性声明序 if 链）。
/// `pub(crate)`：由 `codegen::generate()` 按 `variable_length` 分派调用。
pub(crate) fn gen_vlen_decode(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    let endian = model.meta.endian;
    let mut groups: Vec<(Vec<(u8, u8)>, TokenStream)> = Vec::new();
    for info in infos {
        let vn = &info.vn;
        let ctx = vlen_ctx(info, model)?;
        let key = vlen_decode_key(info, &ctx);
        // EVEX 指令走独立 arm（62 + P0/P1/P2 guard）
        if ctx.evex.is_some() {
            groups.push((key, gen_vlen_evex_decode_arm(info, &ctx, endian)?));
            continue;
        }
        // VEX 指令走独立 arm（C4 + vex2/vex3 guard）
        if ctx.vex.is_some() {
            groups.push((key, gen_vlen_vex_decode_arm(info, &ctx, endian)?));
            continue;
        }
        // `+r` 形式：opcode 含 reg 低 3 位（50/58/B8/C8...）
        if let Some(base) = ctx.opcode_reg {
            groups.push((
                key,
                gen_vlen_opcode_reg_decode_arm(info, &ctx, base, endian)?,
            ));
            continue;
        }
        let form = info.form;
        let mut conds: Vec<TokenStream> = Vec::new();
        // 前缀匹配条件：66/F2/F3 → 扫描标志（无长度）；其他（LOCK 0xF0 等）→
        // 显式字节检查（占 1 字节，escape/opcode 偏移 +1）
        let (prefix_cond, prefix_len) = match form.prefix.as_deref() {
            None | Some("opsize") => (None, 0),
            Some("field") => {
                let v = info
                    .inst
                    .fields
                    .as_ref()
                    .and_then(|f| f.get("prefix"))
                    .copied()
                    .unwrap_or(0);
                prefix_cond_ts(v)
            }
            Some(p) => prefix_cond_ts(parse_u64(p).unwrap_or(0)),
        };
        if let Some(c) = prefix_cond {
            conds.push(c);
        }
        // 宽度 guard：固定宽度 Reg 操作数（16/32/64）→ `__opsize == w`
        //（取代旧 opsize/w 位区分条件——MOVD [gpr32] 与 MOVQ [gpr64] 同
        // opcode 仅 REX.W 不同，靠此区分）。
        if let Some(w) = ctx.decode_width_guard {
            conds.push(quote! { __opsize == #w });
        }
        if ctx.decode_width_neq64 {
            conds.push(quote! { __opsize != 8 });
        }
        // escape + opcode 匹配（prefix 显式字节已占 prefix_len）
        let mut off = prefix_len;
        if let Some(esc) = &form.escape {
            for e in esc {
                conds.push(quote! { bytes[__o + #off] == #e as u8 });
                off += 1;
            }
        }
        let opcode = info.inst.opcode.unwrap();
        if info
            .operands
            .iter()
            .any(|(_, _, s, _)| s.kind == OperandKind::Cond)
        {
            // 条件码在 opcode 低 4 位：guard 匹配高 4 位（JCC/SETCC/CMOVCC）
            conds.push(quote! { (bytes[__o + #off] & 0xF0) == (#opcode & 0xF0) as u8 });
        } else {
            conds.push(quote! { bytes[__o + #off] == #opcode as u8 });
        }
        off += 1;
        // 固定 ModRM 字节（MFENCE 0F AE F0 等无操作数指令）占 1 字节并精确匹配。
        if let Some(fm) = ctx.modrm_fixed {
            conds.push(quote! { bytes[__o + #off] == #fm as u8 });
            off += 1;
        }
        // 无 ModRM 形式（JMP/CALL rel32、RET、MFENCE 固定字节）：imm/label
        // 紧接 opcode；有 ModRM 形式：modrm 在 opcode 之后。
        let no_modrm = ctx.modrm.is_none();
        let modrm_idx = off; // modrm 在 bytes[__o + modrm_idx]（无 modrm 时无用）
        if !no_modrm {
            off += 1;
        }
        let total = off + ctx.imm_bytes;
        let len = total;
        let cond = if conds.is_empty() {
            quote! { true }
        } else {
            quote! { #(#conds)&&* }
        };
        // 字段提取表达式：modrm 语义决定 reg/rm/base/mem 的来源
        let imm_start = total - ctx.imm_bytes;
        let field_expr = |i: usize, slot: &OperandSlot| -> Result<TokenStream, String> {
            let reg_field = quote! { ((__modrm >> 3) & 7) as u32 | (__rex_r << 3) };
            let rm_field = quote! { ((__modrm & 7) as u32) | (__rex_b << 3) };
            // Reg 构造按宽度视图（固定宽度组 / 多态 __opsize）
            match slot.kind {
                OperandKind::Reg => {
                    let view = ctx.reg_view.get(i).copied().flatten();
                    let fe = |v: TokenStream| field_ctor_expr_view(slot, v, view);
                    match (ctx.modrm.unwrap(), i) {
                        (ModrmKind::RR, 0) | (ModrmKind::MemReg, 0) | (ModrmKind::MemRefOp, 0) => {
                            Ok(fe(reg_field))
                        }
                        (ModrmKind::RR, 1) | (ModrmKind::Ext, 0) => Ok(fe(rm_field)),
                        (ModrmKind::RRRev, 1) => Ok(fe(reg_field)),
                        (ModrmKind::RRRev, 0) => Ok(fe(rm_field)),
                        (ModrmKind::RRSrc2, 0) => Ok(fe(reg_field)),
                        (ModrmKind::RRSrc2, 1) => Ok(fe(quote! { __vvvv as u32 })),
                        (ModrmKind::RRSrc2, 2) => Ok(fe(rm_field)),
                        (ModrmKind::MemReg, 1) => Ok(fe(quote! { __base })),
                        _ => Err(format!(
                            "[[instructions.{}]]: reg operand {i} position unsupported for modrm {:?}",
                            info.inst.name, ctx.modrm
                        )),
                    }
                }
                OperandKind::Mem => match (ctx.modrm.unwrap(), i) {
                    (ModrmKind::MemRefOp, 1) => Ok(
                        quote! { MemRef { base: <Reg as TryFrom<RegRef>>::try_from(RegRef::new(RegClass::GPR(8),__base)).unwrap(),  disp: __disp, index: __index_reg, scale: __scale } },
                    ),
                    _ => Err(format!(
                        "[[instructions.{}]]: mem operand {i} position unsupported",
                        info.inst.name
                    )),
                },
                OperandKind::Cond => {
                    // 条件码 = opcode 字节低 4 位（JCC/SETCC/CMOVCC）；
                    // opcode 在 modrm_idx - 1（modrm_idx = opcode 后位置）
                    let cond_off = modrm_idx.saturating_sub(1);
                    Ok(quote! { (bytes[__o + #cond_off] & 0x0F) as u8 })
                }
                OperandKind::Imm | OperandKind::Label => {
                    let raw = imm_read_ts(imm_start, ctx.imm_bytes, endian);
                    let signed = slot.signed.unwrap_or(false);
                    let w = slot.width.unwrap_or(32);
                    if signed {
                        Ok(sign_extend_ts(raw, w))
                    } else {
                        Ok(quote! { #raw as i64 })
                    }
                }
            }
        };
        let mut binds: Vec<TokenStream> = Vec::new();
        let mut ctor_fields: Vec<TokenStream> = Vec::new();
        for (i, (_, fid, slot, _)) in info.operands.iter().enumerate() {
            let expr = field_expr(i, slot)?;
            binds.push(quote! { let #fid = #expr; });
            ctor_fields.push(quote! { #fid });
        }
        let ctor = if info.operands.is_empty() {
            quote! { Inst::#vn }
        } else {
            quote! { Inst::#vn { #(#ctor_fields),* } }
        };
        // ext 形式：ModRM.reg 必须等于固定扩展码
        let modrm_guard: TokenStream = if no_modrm {
            quote! {}
        } else if ctx.modrm.unwrap() == ModrmKind::Ext {
            let ext = info
                .inst
                .fields
                .as_ref()
                .and_then(|f| f.get("ext"))
                .copied()
                .unwrap_or(0);
            quote! { && ((__modrm >> 3) & 7) as u64 == #ext }
        } else {
            quote! {}
        };
        if no_modrm {
            // 无 ModRM：opcode（+imm）直接匹配，无 reg/rm 提取。
            groups.push((
                key,
                quote! {
                    if __o + #total <= bytes.len() && #cond {
                        #(#binds)*
                        return Some((#ctor, __o + #len));
                    }
                },
            ));
        } else if ctx.modrm.unwrap().is_mem() {
            // 内存形式：mod≠3 + SIB（index=4 无 index）+ disp8/disp32 + RIP-rel 拒绝。
            // MemReg（无 disp 语义）只接受 mod∈{0,1} 且 mod=1 时 disp==0
            //（force_disp_base 的 disp8=0）；MemRefOp 接受 mod∈{0,1,2}。
            let mod_guard: TokenStream = if ctx.modrm.unwrap() == ModrmKind::MemReg {
                quote! { __mod != 3 && __mod != 2 }
            } else {
                quote! { __mod != 3 }
            };
            // MemReg（rm_mem：[base] 仅基址语义）不接受 SIB index——否则会吞掉
            // MemRefOp 的索引寻址字节（如 8B 04 0B：mov_mem 先于 mov64rm 声明）。
            let idx_reject: TokenStream = if ctx.modrm.unwrap() == ModrmKind::MemRefOp {
                quote! {}
            } else {
                quote! {
                    if __index_reg.is_some() {
                        __sib_ok = false;
                    }
                }
            };
            let disp_zero_check: TokenStream = if ctx.modrm.unwrap() == ModrmKind::MemReg {
                quote! {
                    if __mod == 1 {
                        __disp = (bytes[__o2] as i8) as i64;
                        __o2 += 1;
                        if __disp != 0 {
                            __disp_ok = false;
                        }
                    }
                }
            } else {
                quote! {
                    if __mod == 1 {
                        if __o2 < bytes.len() {
                            __disp = (bytes[__o2] as i8) as i64;
                            __o2 += 1;
                        } else {
                            __disp_ok = false;
                        }
                    } else if __mod == 2 {
                        if __o2 + 3 < bytes.len() {
                            __disp = i32::from_le_bytes([
                                bytes[__o2],
                                bytes[__o2 + 1],
                                bytes[__o2 + 2],
                                bytes[__o2 + 3],
                            ]) as i64;
                            __o2 += 4;
                        } else {
                            __disp_ok = false;
                        }
                    }
                }
            };
            groups.push((
                key,
                quote! {
                    if __o + #total <= bytes.len() && #cond {
                        let __modrm = bytes[__o + #modrm_idx];
                        let __mod = __modrm >> 6;
                        if #mod_guard && !(__mod == 0 && (__modrm & 7) == 5) {
                            let mut __o2 = __o + #len;
                            let mut __base: u32 = ((__modrm & 7) as u32) | (__rex_b << 3);
                            let mut __index_reg: Option<Reg> = None;
                            let mut __scale: u8 = 1;
                            let mut __sib_ok = true;
                            if (__modrm & 7) == 4 {
                                if __o2 < bytes.len() {
                                    let __sib_byte = bytes[__o2];
                                    __o2 += 1;
                                    let __idx4: u32 = ((__sib_byte >> 3) & 7) as u32;
                                    let __idx_full: u32 = __idx4 | (__rex_x << 3);
                                    let __sc_bits = (__sib_byte >> 6) & 3;
                                    __scale = match __sc_bits { 1 => 2, 2 => 4, 3 => 8, _ => 1 };
                                    __base = ((__sib_byte & 7) as u32) | (__rex_b << 3);
                                    if __idx_full == 4 {
                                        __index_reg = None;
                                    } else {
                                        __index_reg = Some(
                                            <Reg as TryFrom<RegRef>>::try_from(
                                                RegRef::new(RegClass::GPR(8), __idx_full),
                                            ).unwrap(),
                                        );
                                    }
                                } else {
                                    __sib_ok = false;
                                }
                            }
                            #idx_reject
                            let mut __disp: i64 = 0;
                            let mut __disp_ok = true;
                            #disp_zero_check
                            if __sib_ok && __disp_ok {
                                #(#binds)*
                                return Some((#ctor, __o2));
                            }
                        }
                    }
                },
            ));
        } else {
            groups.push((
                key,
                quote! {
                    if __o + #total <= bytes.len() && #cond {
                        let __modrm = bytes[__o + #modrm_idx];
                        if (__modrm >> 6) == 3 #modrm_guard {
                            #(#binds)*
                            return Some((#ctor, __o + #len));
                        }
                    }
                },
            ));
        }
    }
    let nodes = build_dec_trie(&groups);
    check_dec_trie_overlaps(&nodes)?;
    let dispatch = emit_dec_trie(&nodes, 0, 0);
    let scan_loop = gen_prefix_scan_loop(model)?;
    // E-②：default_opsize（[meta].default_opsize，位）作为 decode 的 __opsize
    // 初始值（无前缀时的缺省宽度；缺省 4 = 32 位——现状语义）。
    let default_opsize: u16 = model
        .meta
        .default_opsize
        .map(|b| (b / 8) as u16)
        .unwrap_or(4);
    Ok(quote! {
        /// 解码字节流开头的单条指令；共享前缀扫描（`[conventions.prefix_scan]`
        /// 声明驱动）+ 字节前缀决策树，无匹配 → None。返回 (指令, 消费字节数)。
        #[allow(clippy::int_plus_one)]
        pub fn decode(bytes: &[u8]) -> Option<(Inst, usize)> {
            let mut __o = 0usize;
            let mut __opsize: u16 = #default_opsize;
            let mut __p66 = false;
            let mut __pF0 = false;
            let mut __pF2 = false;
            let mut __pF3 = false;
            let mut __rex_r: u32 = 0;
            let mut __rex_x: u32 = 0;
            let mut __rex_b: u32 = 0;
            #scan_loop
            #dispatch
            None
        }

        /// 解码并报告部分匹配偏移：变长 ISA 失败 → Err(已消费前缀字节数)。
        /// 实现：先调 `decode(bytes)`（完整路径，含前缀扫描）；成功返回
        /// 完整结果。失败时单独扫描前缀得到 `__o`（已消费的前缀字节数）
        /// 作为 Err 偏移——指令字节不匹配但前缀已被消费。
        pub fn decode_partial(bytes: &[u8]) -> Result<(Inst, usize), usize> {
            if let Some(r) = decode(bytes) {
                return Ok(r);
            }
            // 失败：扫描前缀（与 decode 内部一致），报告部分匹配偏移
            let mut __o = 0usize;
            let mut __opsize: u16 = #default_opsize;
            let mut __p66 = false;
            let mut __pF0 = false;
            let mut __pF2 = false;
            let mut __pF3 = false;
            let mut __rex_r: u32 = 0;
            let mut __rex_x: u32 = 0;
            let mut __rex_b: u32 = 0;
            #scan_loop
            let _ = (__opsize, __p66, __pF0, __pF2, __pF3, __rex_r, __rex_x, __rex_b);
            Err(__o)
        }
    })
}

/// 解析 "0x40..0x4F" / "0x40..=0x4F"（闭区间）→ (lo, hi)。
fn parse_scan_range(s: &str) -> Option<(u64, u64)> {
    let (lo_s, hi_s) = if let Some((a, b)) = s.split_once("..=") {
        (a, b)
    } else if let Some((a, b)) = s.split_once("..") {
        (a, b)
    } else {
        return None;
    };
    let lo = parse_u64(lo_s.trim())?;
    let hi = if let Some((a, b)) = hi_s.split_once("..=") {
        let h = parse_u64(a.trim())?;
        let e = parse_u64(b.trim())?;
        // 罕见：双层 ..；直接取第一个
        let _ = e;
        h
    } else {
        parse_u64(hi_s.trim())?
    };
    // ".." 半开区间 → hi 减一；"..=" 闭区间 → 原值
    let hi = if s.contains("..=") {
        hi
    } else {
        hi.checked_sub(1)?
    };
    if lo > hi || hi > 0xFF {
        return None;
    }
    Some((lo, hi))
}

/// x86 缺省前缀扫描集（`[conventions.prefix_scan]` 未声明时使用）。
fn x86_scan_default() -> Vec<PrefixScanEntry> {
    vec![
        PrefixScanEntry {
            byte: Some(0x66),
            range: None,
            effects: vec!["opsize16".into()],
        },
        PrefixScanEntry {
            byte: Some(0xF0),
            range: None,
            effects: vec!["lock".into()],
        },
        PrefixScanEntry {
            byte: Some(0xF2),
            range: None,
            effects: vec!["repne".into()],
        },
        PrefixScanEntry {
            byte: Some(0xF3),
            range: None,
            effects: vec!["repe".into()],
        },
        PrefixScanEntry {
            byte: Some(0x67),
            range: None,
            effects: vec!["addr16".into()],
        },
        PrefixScanEntry {
            byte: None,
            range: Some("0x40..0x4F".into()),
            effects: vec!["rex".into()],
        },
    ]
}

/// 生成变长解码的前缀扫描循环：每条目一个 `else if`，效果集驱动
/// `__opsize`/前缀标志/REX 位。非 x86 ISA 声明空表 → 立即 break（零分支）。
fn gen_prefix_scan_loop(model: &V12Model) -> Result<TokenStream, String> {
    let entries: Vec<PrefixScanEntry> = model
        .conventions
        .prefix_scan
        .clone()
        .unwrap_or_else(x86_scan_default);
    let mut stmts: Vec<TokenStream> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let ctx = || format!("[conventions.prefix_scan][{i}]");
        let mut eff: Vec<TokenStream> = Vec::new();
        for fx in &e.effects {
            eff.push(match fx.as_str() {
                "opsize16" => quote! { __p66 = true; __opsize = 2; },
                "lock" => quote! { __pF0 = true; },
                "repe" => quote! { __pF3 = true; },
                "repne" => quote! { __pF2 = true; },
                "addr16" => quote! {},
                "rex" => quote! {
                    __rex_r = ((__b >> 2) & 1) as u32;
                    __rex_x = ((__b >> 1) & 1) as u32;
                    __rex_b = (__b & 1) as u32;
                    if (__b & 0x08) != 0 { __opsize = 8; }
                },
                other => {
                    return Err(format!(
                        "{}: unknown effect '{other}' (opsize16/lock/repe/repne/addr16/rex)",
                        ctx()
                    ));
                }
            });
        }
        let guard = if let Some(b) = e.byte {
            quote! { __b == #b as u8 }
        } else if let Some(r) = &e.range {
            let (lo, hi) = parse_scan_range(r).ok_or_else(|| {
                format!(
                    "{}: bad range '{r}' (expect '0x40..0x4F' or '0x40..=0x4F')",
                    ctx()
                )
            })?;
            quote! { ((#lo as u8)..=(#hi as u8)).contains(&__b) }
        } else {
            return Err(format!("{}: entry needs `byte` or `range`", ctx()));
        };
        let kw = if i == 0 {
            quote! { if }
        } else {
            quote! { else if }
        };
        stmts.push(quote! { #kw #guard { #(#eff)* __o += 1; } });
    }
    Ok(quote! {
        while __o < bytes.len() {
            let __b = bytes[__o];
            #(#stmts)*
            else { break; }
        }
    })
}

/// 固定前缀的匹配条件：(条件, 占用的指令长度偏移)。
/// 66/F0/F2/F3 由前缀扫描消费（条件用标志，无长度）；其他前缀字节不在
/// 扫描集 → 显式检查 bytes[__o]（占 1 字节）。
fn prefix_cond_ts(v: u64) -> (Option<TokenStream>, usize) {
    match v {
        0 => (Some(quote! { !__p66 && !__pF0 && !__pF2 && !__pF3 }), 0),
        0x66 => (Some(quote! { __p66 }), 0),
        0xF0 => (Some(quote! { __pF0 }), 0),
        0xF2 => (Some(quote! { __pF2 }), 0),
        0xF3 => (Some(quote! { __pF3 }), 0),
        other => (Some(quote! { bytes[__o] == #other as u8 }), 1),
    }
}

/// 内存寻址的强制位移基址（conventions.modrm.force_disp_base，缺省空）。
fn mem_force_disp(model: &V12Model, info: &InstInfo) -> Result<Vec<u8>, String> {
    if let Some(modrm) = &model.conventions.modrm {
        return Ok(modrm.force_disp_base.clone());
    }
    let _ = info;
    Ok(Vec::new())
}

/// 读取尾部立即数（从 `__o + start` 起读 `bytes` 字节 → u64 原始值）。
/// 端序由 `[meta].endian` 决定（little = 现状；big → 大端装配）。
fn imm_read_ts(start: usize, bytes: usize, endian: Endian) -> TokenStream {
    let le = endian == Endian::Little;
    match bytes {
        1 => quote! { bytes[__o + #start] as u64 },
        2 => {
            let (a, b) = (
                quote! { bytes[__o + #start] },
                quote! { bytes[__o + #start + 1] },
            );
            if le {
                quote! { u16::from_le_bytes([#a, #b]) as u64 }
            } else {
                quote! { u16::from_be_bytes([#a, #b]) as u64 }
            }
        }
        4 => {
            let (a, b, c, d) = (
                quote! { bytes[__o + #start] },
                quote! { bytes[__o + #start + 1] },
                quote! { bytes[__o + #start + 2] },
                quote! { bytes[__o + #start + 3] },
            );
            if le {
                quote! { u32::from_le_bytes([#a, #b, #c, #d]) as u64 }
            } else {
                quote! { u32::from_be_bytes([#a, #b, #c, #d]) as u64 }
            }
        }
        8 => {
            let (a, b, c, d, e, f, g, h) = (
                quote! { bytes[__o + #start] },
                quote! { bytes[__o + #start + 1] },
                quote! { bytes[__o + #start + 2] },
                quote! { bytes[__o + #start + 3] },
                quote! { bytes[__o + #start + 4] },
                quote! { bytes[__o + #start + 5] },
                quote! { bytes[__o + #start + 6] },
                quote! { bytes[__o + #start + 7] },
            );
            if le {
                quote! { u64::from_le_bytes([#a, #b, #c, #d, #e, #f, #g, #h]) }
            } else {
                quote! { u64::from_be_bytes([#a, #b, #c, #d, #e, #f, #g, #h]) }
            }
        }
        _ => quote! { 0u64 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(edges: Vec<(u8, u8, usize)>) -> DecTrieNode {
        DecTrieNode {
            arms: Vec::new(),
            edges,
        }
    }

    /// 同一节点的两条边能命中同一字节（精确 0x80 与掩码 0x80..0x8F）→ 拒绝。
    #[test]
    fn trie_overlap_detected() {
        let nodes = vec![
            node(vec![(0xFF, 0x80, 1), (0xF8, 0x80, 2)]),
            node(vec![]),
            node(vec![]),
        ];
        let err = check_dec_trie_overlaps(&nodes).unwrap_err();
        assert!(err.contains("overlap"), "err: {err}");
    }

    /// 不同节点/不相交边 → 通过。
    #[test]
    fn trie_disjoint_ok() {
        // root 边不相交：(0x80 exact) 与 (0x00..0x07 masked)。
        let nodes = vec![
            node(vec![(0xFF, 0x80, 1), (0xF8, 0x00, 2)]),
            node(vec![]),
            node(vec![]),
        ];
        assert!(check_dec_trie_overlaps(&nodes).is_ok());
    }
}
