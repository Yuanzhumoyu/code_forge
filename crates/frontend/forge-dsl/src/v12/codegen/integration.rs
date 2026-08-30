//! v12 codegen — TargetMachine 集成层（迭代 5）。
//!
//! 自包含模块（encode/decode/disassemble/assemble）之上生成 forge-codegen
//! 组件：MachineInst impl、TargetEncoder/Decoder/Disassembler/Assembler、
//! TargetABI、TargetFrameLowering、TargetLowering、TargetMachine 组装。
//!
//! 关键语义（与 v11 一致）：
//! - Inst 寄存器字段为类型化 `Reg` 枚举（v12 类型化重构：不再裸 u32）：
//!   lowering 构造时占位 `Reg::from_index(0, class)`，regalloc 经 `xreg_map`
//!   分配后 `set_reg_field` 回填物理索引（`Reg::from_index` 重建枚举），
//!   encode 读 `to_index()`。
//! - `uses()/defs()` 返回寄存器字段的物理索引（回填后即分配结果）；regalloc
//!   的活区间由 pipeline 聚合的 `xreg_map` 驱动（见 pipeline/liverange.rs），
//!   MachineInst::uses/defs 仅作辅助查询。
//! - effect 标签（指令 `effect` 键）驱动 is_branch/is_call/is_ret/effects。

use super::super::model::*;
use super::super::pred::{self, CmpOp, Pred};
use super::InstInfo;
use super::field_ctor_expr;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

/// 解析 lowering 模板立即数字面量：支持十进制、`0x`/`0X` 十六进制、
/// 负号（含 `-0x…`），超过 i64 正范围的 64 位 hex 按 u64 解析后转
/// 两补码（如 `0x8000000000000000` → i64::MIN）。
pub(crate) fn parse_i64_lit(text: &str) -> Result<i64, ()> {
    let t = text.trim();
    let (neg, body) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t),
    };
    let v = if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).map_err(|_| ()).or_else(|_| {
            // 超出 i64 正范围 → 按 u64 取两补码
            u64::from_str_radix(hex, 16)
                .map(|u| u as i64)
                .map_err(|_| ())
        })?
    } else {
        body.parse::<i64>().map_err(|_| ())?
    };
    Ok(if neg { v.wrapping_neg() } else { v })
}

/// 结构化谓词 → Rust 布尔表达式（`__attr(name) -> Option<i64>` 求值；
/// 未知属性 → false，与 pred.rs eval 语义一致）。
pub(crate) fn compile_pred_guard(pred: &Pred, attr: &syn::Ident) -> TokenStream {
    match pred {
        Pred::And(ps) => {
            let gs: Vec<_> = ps.iter().map(|p| compile_pred_guard(p, attr)).collect();
            quote! { (#(#gs)&&*) }
        }
        Pred::Or(ps) => {
            let gs: Vec<_> = ps.iter().map(|p| compile_pred_guard(p, attr)).collect();
            quote! { (#(#gs)||*) }
        }
        Pred::Not(p) => {
            let g = compile_pred_guard(p, attr);
            quote! { !(#g) }
        }
        Pred::Cmp(op, name, want) => {
            let n = syn::LitStr::new(name, proc_macro2::Span::call_site());
            let f = match op {
                CmpOp::Eq => quote! { == },
                CmpOp::Ne => quote! { != },
                CmpOp::Lt => quote! { < },
                CmpOp::Le => quote! { <= },
                CmpOp::Gt => quote! { > },
                CmpOp::Ge => quote! { >= },
            };
            quote! { #attr(#n).map_or(false, |__g| __g #f #want) }
        }
    }
}

/// lowering 谓词的运行时属性源：预计算到 Option<i64> 局部（避免闭包捕获 ctx）。
/// `cond` = Fcmp/Icmp 条件 id（fcmp_id/icmp_id；非比较 op 恒 0）。
/// `elem` = 结果类型 id（向量 → 元素类型 id；F32=1/F64=2/I32=3/I64=4）。
pub(crate) fn gen_lowering_attrs() -> TokenStream {
    quote! {
        let __a_rd = results.first().and_then(|x| ctx.xreg_types.get(x)).map(|t| {
            if ctx.type_ctx.as_ref().is_some_and(|tc| tc.is_vector(*t)) {
                (ctx.type_ctx.as_ref().map(|tc| tc.size_bytes(*t)).unwrap_or(0) * 8) as i64
            } else {
                t.bits() as i64
            }
        });
        let __a_rs1 = args.first().and_then(|x| ctx.xreg_types.get(x)).map(|t| {
            if ctx.type_ctx.as_ref().is_some_and(|tc| tc.is_vector(*t)) {
                (ctx.type_ctx.as_ref().map(|tc| tc.size_bytes(*t)).unwrap_or(0) * 8) as i64
            } else {
                t.bits() as i64
            }
        });
        let __a_rs2 = args.get(1).and_then(|x| ctx.xreg_types.get(x)).map(|t| {
            if ctx.type_ctx.as_ref().is_some_and(|tc| tc.is_vector(*t)) {
                (ctx.type_ctx.as_ref().map(|tc| tc.size_bytes(*t)).unwrap_or(0) * 8) as i64
            } else {
                t.bits() as i64
            }
        });
        let __a_elem = results.first().and_then(|x| ctx.xreg_types.get(x)).map(|t| {
            if ctx.type_ctx.as_ref().is_some_and(|tc| tc.is_vector(*t)) {
                ctx.type_ctx
                    .as_ref()
                    .and_then(|tc| tc.element_type(*t))
                    .map(elem_id_of)
                    .unwrap_or(0)
            } else {
                elem_id_of(*t)
            }
        });
        let __a_cond = match op {
            crate::prelude::Opcode::Fcmp { cond } => Some(fcmp_id(cond)),
            crate::prelude::Opcode::Icmp { cond } => Some(icmp_id(cond)),
            _ => None,
        };
        let __a_imm0 = ctx.current_immediates.first().copied().map(|v| v as i64);
        let __attr = |name: &str| -> Option<i64> {
            match name {
                "rd" => __a_rd,
                "rs1_width" => __a_rs1,
                "rs2_width" => __a_rs2,
                "elem" => __a_elem,
                "cond" => __a_cond,
                "imm0" => __a_imm0,
                _ => None,
            }
        };
    }
}

/// 按指令名取操作数序字段 ident（集成层硬编码构造 Inst 用；字段名随
/// 类型化重构变化，避免各处硬编码 op{i}）。
pub(crate) fn inst_fids<'a>(infos: &'a [InstInfo], name: &str) -> Vec<&'a syn::Ident> {
    infos
        .iter()
        .find(|i| i.inst.name == name)
        .map(|i| i.operands.iter().map(|(_, fid, _, _)| fid).collect())
        .unwrap_or_default()
}

/// 按角色解析移动指令字段：返回 (src 字段名, src 操作数序号, dest 字段名,
/// dest 操作数序号)。src = 第一个 In 角色 Reg 槽；dest = 第一个 Out/InOut
/// 角色 Reg 槽。该角色语义对两种操作数序都成立：x86 的 `MOV_RM8_R64`
///（op0=In=src、op1=InOut=dest）与 riscv 的 `mv`（op0=Out=dest、op1=In=src）。
/// 调用方用 src/dest **序号**做 map_reg_field（vreg 绑定字段），不可硬编码
/// 0/1——那按 x86 方向写死在定宽 ISA（dest=op0）上会把参数移动反成
/// `mv vreg, x0`、返回值接收反成 `mv x0, src`。
pub(crate) fn inst_move_role(infos: &[InstInfo], name: &str) -> Option<(syn::Ident, u8, syn::Ident, u8)> {
    let info = infos.iter().find(|i| i.inst.name == name)?;
    let src = info
        .operands
        .iter()
        .enumerate()
        .find(|(_, (_, _, s, r))| *r == OperandRole::In && s.kind == OperandKind::Reg)?;
    let dest = info.operands.iter().enumerate().find(|(_, (_, _, s, r))| {
        matches!(r, OperandRole::Out | OperandRole::InOut) && s.kind == OperandKind::Reg
    })?;
    Some((src.1.1.clone(), src.0 as u8, dest.1.1.clone(), dest.0 as u8))
}

/// 按指令名取 (所有 Reg 槽字段, 所有 Imm/Label 槽字段)——@frame_alloc 等
/// 需要把"全部 reg 槽填 sp、全部 imm 槽填 frame_size"的指令（x86 的
/// `SUB64_R_IMM32` 是 2 操作数、demo 的 `ADDI16` 是 rd/rs1/imm 3 操作数）。
/// `pub(crate)`：lowering 模块（lowering.rs）复用。
pub(crate) fn inst_reg_imm_fids<'a>(
    infos: &'a [InstInfo],
    name: &str,
) -> Option<(Vec<&'a syn::Ident>, Vec<&'a syn::Ident>)> {
    let info = infos.iter().find(|i| i.inst.name == name)?;
    let regs: Vec<&syn::Ident> = info
        .operands
        .iter()
        .filter(|(_, _, s, _)| s.kind == OperandKind::Reg)
        .map(|(_, f, _, _)| f)
        .collect();
    let imms: Vec<&syn::Ident> = info
        .operands
        .iter()
        .filter(|(_, _, s, _)| matches!(s.kind, OperandKind::Imm | OperandKind::Label))
        .map(|(_, f, _, _)| f)
        .collect();
    Some((regs, imms))
}
/// 生成集成层组件（Reg 枚举 + MachineInst + Encoder + Decoder + Disasm +
/// Assembler + ABI + FrameLowering + Lowering + TargetMachine）。
pub fn gen_integration(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    let reg_enum = gen_reg_enum(model)?;
    let machine_inst = gen_machine_inst(infos, model)?;
    let encoder = gen_encoder(infos, model)?;
    let decoder = gen_decoder();
    let disasm = gen_disasm(infos)?;
    let assembler = gen_assembler(model);
    let abi = gen_abi(model)?;
    let frame = gen_frame_lowering(infos, model)?;
    let lowering = super::lowering::gen_lowering(infos, model)?;
    let isa_info = gen_isa_info(model, infos)?;
    let reg_info = gen_reg_info(model)?;
    let target_machine = gen_target_machine(model)?;
    Ok(quote! {
        // ── v12 TargetMachine 集成层（迭代 5/6）──
        use forge_ir::PhysReg;
        #reg_enum
        #machine_inst
        #encoder
        #decoder
        #disasm
        #assembler
        #abi
        #frame
        #lowering
        #isa_info
        #reg_info
        #target_machine
    })
}

// ─────────────────────── Reg 枚举（物理寄存器）───────────────────────

/// 从 v12 `[reg.*]` 生成物理寄存器枚举 + PhysReg impl + 默认值类常量。
/// 变体 = 全部寄存器组的全部寄存器名；物理编号 = 组内索引 + base_index
/// （多宽度视图组可 base_index=0 共享同一物理编号——x86 的 EAX/RAX 等）。
/// GPR 组按宽度降序排列（64 位主组在前），FPR 组在后。
fn gen_reg_enum(model: &V12Model) -> Result<TokenStream, String> {
    // 收集 (组名, 寄存器名列表, base_index)
    let mut groups: Vec<(RegClass, Vec<String>, u32)> = Vec::new();
    for (reg_class, g) in &model.reg {
        let names = group_names(g)?;
        groups.push((*reg_class, names, g.base_index.unwrap_or(0)));
    }
    // GPR 在前（按宽度降序：64 → 32 → 16 → 8），FPR 在后（保持组序）
    groups.sort_by(|(a, ..), (b, ..)| a.cmp(b).reverse());

    let mut variants: Vec<syn::Ident> = Vec::new();
    let mut to_index_arms: Vec<TokenStream> = Vec::new();
    let mut class_arms: Vec<TokenStream> = Vec::new();
    let mut width_arms: Vec<TokenStream> = Vec::new();
    // from_index：GPR 区 → 主 GPR 视图（"gpr64"/"gpr" 组）；FPR 区 → XMM 视图
    let mut gpr_main: Option<RegClass> = None;
    let mut fpr_main: Option<RegClass> = None;
    for (reg_class, names, base) in &groups {
        for (i, n) in names.iter().enumerate() {
            let v = format_ident!("{n}");
            let idx = i as u32 + base;
            variants.push(v.clone());
            to_index_arms.push(quote! { Reg::#v => #idx });
            let (cls, width) = match reg_class {
                RegClass::GPR(w) => (quote! { forge_ir::RegClass::GPR(#w)}, w),
                RegClass::FPR(w) => (quote! { forge_ir::RegClass::FPR(#w)}, w),
                RegClass::VEC(w) => (quote! { forge_ir::RegClass::VEC(#w)}, w),
                RegClass::KReg(w) => (quote! { forge_ir::RegClass::KReg(#w)}, w),
            };
            class_arms.push(quote! { Reg::#v => #cls });
            // 宽度（位）：opsize 推导与 decode 宽度视图用
            width_arms.push(quote! { Reg::#v => #width });
        }
    }
    // 主组兜底：宽 GPR 组 / 宽 FPR 组（`__DEFAULT_GPR_CLASS`/`__DEFAULT_FPR_CLASS`）。
    // 此前两值都取 `groups.first()`（按 RegClass 降序 → 最高类，x86 即 FPR/XMM），
    // 使 `__DEFAULT_GPR_CLASS` 误成 GPR(16)（无效类）——修正为各自的宽度最大组。
    if gpr_main.is_none() {
        gpr_main = groups
            .iter()
            .filter(|(rc, ..)| matches!(rc, RegClass::GPR(_)))
            .max_by_key(|(rc, ..)| rc.width())
            .map(|(g, ..)| *g);
    }
    if fpr_main.is_none() {
        // 优先 XMM（16 字节）作为默认浮点类——ABI/SSE 占位以 XMM 为基准；
        // ZMM（32 字节）等 EVEX 组不改变默认（否则 SSE 占位变成 ZMM 视图）。
        fpr_main = groups
            .iter()
            .filter(|(rc, ..)| matches!(rc, RegClass::FPR(_)))
            .find(|(rc, ..)| rc.width() == 16)
            .or_else(|| {
                groups
                    .iter()
                    .filter(|(rc, ..)| matches!(rc, RegClass::FPR(_)))
                    .max_by_key(|(rc, ..)| rc.width())
            })
            .map(|(g, ..)| *g);
    }
    // from_index：按传入 RegClass 消歧（is_fp → FPR 视图、否则 GPR 视图）。
    // v12 物理编号 = 组内索引（XMM0=0），故 FPR 分支按组内索引映射（不偏移）。
    let gpr_width = gpr_main.map(|r| r.width()).unwrap_or_else(|| 8);
    let fpr_width = fpr_main.map(|r| r.width()).unwrap_or_else(|| 8);

    // 默认值类：主 GPR 组宽度 / FPR 组宽度
    let gpr_class = quote! { forge_ir::RegClass::GPR(#gpr_width) };
    let fpr_class = quote! { forge_ir::RegClass::FPR(#fpr_width) };

    // from_index_grp：按组名 + 索引构造 Reg（decode 宽度视图 / assemble 组视图）。
    // 覆盖全部组（含宽度视图组 gpr8/16/32/64 与浮点组）。
    let mut grp_arms: Vec<TokenStream> = Vec::new();
    for (reg_class, names, base) in &groups {
        let gname_lit = syn::LitStr::new(&reg_class.to_string(), proc_macro2::Span::call_site());
        let mut idx_arms: Vec<TokenStream> = Vec::new();
        let mut fb: Option<syn::Ident> = None;
        for (i, n) in names.iter().enumerate() {
            let v = format_ident!("{n}");
            let idx = i as u32 + base;
            idx_arms.push(quote! { #idx => Reg::#v });
            if fb.is_none() {
                fb = Some(v);
            }
        }
        let fb = fb.unwrap_or_else(|| format_ident!("RAX"));
        grp_arms.push(quote! {
            #gname_lit => match idx { #(#idx_arms,)* _ => Reg::#fb }
        });
    }

    Ok(quote! {
        /// 默认整数值类（lowering 中 alloc_xreg 的默认目标类；元数据驱动）。
        pub(crate) const __DEFAULT_GPR_CLASS: forge_ir::RegClass = #gpr_class;
        /// 默认浮点值类。
        pub(crate) const __DEFAULT_FPR_CLASS: forge_ir::RegClass = #fpr_class;

        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Reg { #(#variants),* }

        impl forge_ir::PhysReg for Reg {
            fn to_index(self) -> u32 {
                match self { #(#to_index_arms,)* }
            }
            fn from_index(idx: u32, class: forge_ir::RegClass) -> Self {
                <Self as TryFrom<RegRef>>::try_from(RegRef::new(class,idx)).unwrap()
            }
            fn class(self) -> forge_ir::RegClass {
                match self { #(#class_arms,)* }
            }
            /// 寄存器宽度（位）：opsize 前缀推导与 decode 宽度视图选择。
            fn width(self) -> u16 {
                match self { #(#width_arms,)* }
            }
        }
    })
}

/// 组寄存器名列表（共享实现见 `super::super::shared::group_names`；
/// 此处 re-export 保持调用点不变）。
use super::super::shared::group_names;

// ─────────────────────── MachineInst impl ───────────────────────

fn gen_machine_inst(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    let mut use_arms = Vec::new();
    let mut def_arms = Vec::new();
    let mut use_c_arms = Vec::new();
    let mut def_c_arms = Vec::new();
    let mut branch_arms = Vec::new();
    let mut call_arms = Vec::new();
    let mut ret_arms = Vec::new();
    let mut move_arms = Vec::new();
    let mut effects_arms = Vec::new();
    let mut branch_targets_arms = Vec::new();
    let mut implicit_arms: Vec<TokenStream> = Vec::new();
    let mut reg_field_arms: Vec<TokenStream> = Vec::new();
    let mut set_reg_field_arms: Vec<TokenStream> = Vec::new();
    // 物理寄存器名 → 索引（implicit_regs 解析用）。
    let gpr_names: Vec<String> = model
        .reg
        .get(&RegClass::GPR(8))
        .or_else(|| model.reg.get(&RegClass::GPR(4)))
        .map(group_names)
        .transpose()?
        .unwrap_or_default();
    let name_to_idx: std::collections::HashMap<&str, u32> = gpr_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i as u32))
        .collect();

    for info in infos {
        let vn = &info.vn;
        // implicit_regs：指令隐式破坏的物理寄存器（cqo 的 RDX、idiv 的
        // RAX/RDX）→ MachineInst::clobbers（regalloc 在本指令点避开）。
        if let Some(implicit) = &info.inst.implicit_regs {
            let entries: Vec<TokenStream> = implicit
                .iter()
                .filter_map(|r| name_to_idx.get(r.as_str()).copied())
                .map(|idx| quote! { (#idx, forge_ir::RegClass::GPR64) })
                .collect();
            if !entries.is_empty() {
                implicit_arms.push(quote! { Inst::#vn { .. } => &[#(#entries),*] });
            }
        }

        // Reg 操作数按操作数序收集角色（操作数级 role 优先，缺省 In）。
        // 与 v11 一致：每变体一条 arm，模式列出全部 use/def 字段 + `..`；
        // reg_field 序号 = 该变体 Reg 操作数的位置序（lowering 同序 map_reg_field）。
        let mut use_fids: Vec<&syn::Ident> = Vec::new();
        let mut def_fids: Vec<&syn::Ident> = Vec::new();
        let mut reg_field_entries: Vec<(usize, &syn::Ident, &OperandSlot)> = Vec::new();
        let mut reg_field_idx = 0usize;
        for (_, fid, slot, role) in info.operands.iter() {
            if slot.kind != OperandKind::Reg {
                continue;
            }
            match role {
                OperandRole::In => use_fids.push(fid),
                OperandRole::Out => def_fids.push(fid),
                OperandRole::InOut => {
                    use_fids.push(fid);
                    def_fids.push(fid);
                }
            }
            reg_field_entries.push((reg_field_idx, fid, slot));
            reg_field_idx += 1;
        }

        // uses/defs：列出全部字段的 arm（无字段变体用 `..` 兜底）
        if use_fids.is_empty() {
            use_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
            use_c_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
        } else {
            let vals: Vec<_> = use_fids.iter().map(|f| quote! { #f.to_index() }).collect();
            let any: Vec<_> = use_fids
                .iter()
                .map(|_| quote! { crate::machine::inst::OperandConstraint::Any })
                .collect();
            use_arms.push(
                quote! { Inst::#vn { #(#use_fids),*, .. } => smallvec::smallvec![#(#vals),*] },
            );
            use_c_arms.push(
                quote! { Inst::#vn { #(#use_fids),*, .. } => smallvec::smallvec![#(#any),*] },
            );
        }
        if def_fids.is_empty() {
            def_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
            def_c_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
        } else {
            let vals: Vec<_> = def_fids.iter().map(|f| quote! { #f.to_index() }).collect();
            let any: Vec<_> = def_fids
                .iter()
                .map(|_| quote! { crate::machine::inst::OperandConstraint::Any })
                .collect();
            def_arms.push(
                quote! { Inst::#vn { #(#def_fids),*, .. } => smallvec::smallvec![#(#vals),*] },
            );
            def_c_arms.push(
                quote! { Inst::#vn { #(#def_fids),*, .. } => smallvec::smallvec![#(#any),*] },
            );
        }

        // reg_field/set_reg_field：按该变体 Reg 操作数的位置序（与 map_reg_field 的
        // reg_field_i 一致）。reg_field(i) 返回第 i 个 Reg 操作数的物理索引；
        // set_reg_field(i, preg) 用 `Reg::from_index` 回填（regalloc 后）。
        if reg_field_entries.is_empty() {
            reg_field_arms.push(quote! { Inst::#vn { .. } => 0 });
            set_reg_field_arms.push(quote! { Inst::#vn { .. } => {} });
        } else {
            let fids: Vec<_> = reg_field_entries.iter().map(|(_, fid, _)| fid).collect();
            let rf_arms: Vec<_> = reg_field_entries
                .iter()
                .map(|(idx, fid, _)| quote! { #idx => #fid.to_index() })
                .collect();
            let srf_arms: Vec<_> = reg_field_entries
                .iter()
                .map(|(idx, fid, slot)| {
                    let cls = super::reg_class_expr(slot);
                    quote! { #idx => *#fid = <Reg as forge_ir::PhysReg>::from_index(preg, #cls) }
                })
                .collect();
            reg_field_arms.push(quote! {
                Inst::#vn { #(#fids),*, .. } => match i { #(#rf_arms,)* _ => 0 }
            });
            set_reg_field_arms.push(quote! {
                Inst::#vn { #(#fids),*, .. } => match i { #(#srf_arms,)* _ => {} }
            });
        }

        // effect → is_branch/is_call/is_ret/effects
        let eff = &info.inst.effect;
        if eff.iter().any(|e| e == "Branch" || e == "Jump") {
            branch_arms.push(quote! { Inst::#vn { .. } => true });
        }
        if eff.iter().any(|e| e == "Call") {
            call_arms.push(quote! { Inst::#vn { .. } => true });
        }
        if eff.iter().any(|e| e == "Ret") {
            ret_arms.push(quote! { Inst::#vn { .. } => true });
        }
        // branch_targets：effect Branch/Jump 且有 Label 槽 → 提取为 Block
        if (eff.iter().any(|e| e == "Branch" || e == "Jump"))
            && let Some(fid) = info
                .operands
                .iter()
                .find(|(_, _, s, _)| s.kind == OperandKind::Label)
                .map(|(_, fid, _, _)| fid)
        {
            branch_targets_arms.push(quote! {
                Inst::#vn { #fid, .. } => smallvec::smallvec![crate::prelude::Block(*#fid as u32)]
            });
        }
        let eff_kinds: Vec<TokenStream> = eff
            .iter()
            .map(|e| match e.as_str() {
                "Pure" => quote! { crate::prelude::EffectKind::Pure },
                "Read" => quote! { crate::prelude::EffectKind::Read },
                "Write" => quote! { crate::prelude::EffectKind::Write },
                "Branch" => quote! { crate::prelude::EffectKind::Branch },
                "Jump" => quote! { crate::prelude::EffectKind::Jump },
                "Call" => quote! { crate::prelude::EffectKind::Call },
                "Ret" => quote! { crate::prelude::EffectKind::Ret },
                _ => quote! { crate::prelude::EffectKind::Custom(0) },
            })
            .collect();
        if eff_kinds.is_empty() {
            effects_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
        } else {
            effects_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![#(#eff_kinds),*] });
        }

        // is_move：MOV 类（1 use + 1 def Reg）
        let upper = info.inst.name.to_uppercase();
        if (upper.starts_with("MOV_") || upper.starts_with("MOVR"))
            && def_fids.len() == 1
            && use_fids.len() == 1
        {
            let d = def_fids[0];
            let u = use_fids[0];
            if d != u {
                move_arms.push(
                    quote! { Inst::#vn { #d, #u, .. } => Some((#d.to_index(), #u.to_index())) },
                );
            }
        }
    }

    Ok(quote! {
        impl crate::prelude::MachineInst for Inst {
            fn uses(&self) -> smallvec::SmallVec<[u32; 4]> {
                match self { #(#use_arms,)* Inst::Raw(_) => smallvec::smallvec![] }
            }
            fn defs(&self) -> smallvec::SmallVec<[u32; 2]> {
                match self { #(#def_arms,)* Inst::Raw(_) => smallvec::smallvec![] }
            }
            fn use_constraints(&self) -> smallvec::SmallVec<[crate::machine::inst::OperandConstraint; 4]> {
                match self { #(#use_c_arms,)* Inst::Raw(_) => smallvec::smallvec![] }
            }
            fn def_constraints(&self) -> smallvec::SmallVec<[crate::machine::inst::OperandConstraint; 2]> {
                match self { #(#def_c_arms,)* Inst::Raw(_) => smallvec::smallvec![] }
            }
            fn effects(&self) -> smallvec::SmallVec<[crate::prelude::EffectKind; 2]> {
                match self { #(#effects_arms,)* Inst::Raw(_) => smallvec::smallvec![] }
            }
            fn is_branch(&self) -> bool {
                match self { #(#branch_arms,)* _ => false }
            }
            fn branch_targets(&self) -> smallvec::SmallVec<[crate::prelude::Block; 2]> {
                match self { #(#branch_targets_arms,)* _ => smallvec::smallvec![] }
            }
            fn is_call(&self) -> bool {
                match self { #(#call_arms,)* _ => false }
            }
            fn is_ret(&self) -> bool {
                match self { #(#ret_arms,)* _ => false }
            }
            fn clobbers(&self) -> &[(u32, forge_ir::RegClass)] {
                match self { #(#implicit_arms,)* _ => &[] }
            }
            fn is_move(&self) -> Option<(u32, u32)> {
                match self { #(#move_arms,)* _ => None }
            }
            fn reg_field(&self, i: usize) -> u32 {
                match self { #(#reg_field_arms,)* Inst::Raw(_) => 0 }
            }
            fn set_reg_field(&mut self, i: usize, preg: u32) {
                match self { #(#set_reg_field_arms,)* Inst::Raw(_) => {} }
            }
        }
    })
}

// ─────────────────────── TargetEncoder ───────────────────────

fn gen_encoder(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    // 变长（x86）：label 槽是尾部连续 imm（REL32 form.imm），fixup 位置 =
    // 指令末尾 - imm_bytes，reloc = Relative(4,-4)（相对下一条指令）。
    // 定宽（riscv）：label 槽（off_j/off_b）是散布位段，fixup 位置 = 指令
    // 起始，reloc = Relative(4,0)（相对指令地址本身；位段重排由
    // RiscvRelocPatcher 编码）。
    let fixed = !model.meta.variable_length;
    // 含 Label 槽的指令：encode 后对 fixup 位置 use_label_at（块号 → 实际偏移）。
    let mut label_arms: Vec<TokenStream> = Vec::new();
    for info in infos {
        let vn = &info.vn;
        // 找 Label 槽的操作数 (位置, fid)
        if let Some(fid) = info
            .operands
            .iter()
            .find(|(_, _, s, _)| s.kind == OperandKind::Label)
            .map(|(_, fid, _, _)| fid)
        {
            let imm_bytes = (info.form.imm.unwrap_or(0) / 8) as usize;
            if fixed {
                // 定宽：label 槽散布在位段中，fixup = 指令起始（patcher 按
                // ISA 重编码位段；Relative(4,0) = 相对指令地址本身）。
                label_arms.push(quote! {
                    Inst::#vn { .. } => {
                        let bytes = encode(inst).map_err(|e| crate::EncodeError::Other(e))?;
                        let rel = match inst {
                            Inst::#vn { #fid, .. } => *#fid,
                            _ => unreachable!(),
                        };
                        let __base = sink.offset();
                        sink.put_bytes(&bytes);
                        if rel < 0 {
                            // 函数符号调用（Call lowering 把 FuncRef 编码为 -(f+1)）：
                            // 记录 "@N" 符号重定位（JIT 符号表按 FuncRef 序注册）。
                            sink.add_reloc(
                                __base,
                                crate::RelocKind::Relative(4, 0),
                                &format!("@{}", -rel - 1),
                                0,
                            );
                        } else {
                            sink.use_label_at(__base, forge_ir::Block(rel as u32), crate::RelocKind::Relative(4, 0));
                        }
                        Ok(())
                    }
                });
            } else {
                label_arms.push(quote! {
                    Inst::#vn { .. } => {
                        let bytes = encode(inst).map_err(|e| crate::EncodeError::Other(e))?;
                        let rel = match inst {
                            Inst::#vn { #fid, .. } => *#fid,
                            _ => unreachable!(),
                        };
                        let __base = sink.offset();
                        let total = bytes.len();
                        let fixup = __base + total - #imm_bytes;
                        sink.put_bytes(&bytes);
                        if rel < 0 {
                            // 函数符号调用（Call lowering 把 FuncRef 编码为 -(f+1)）：
                            // 记录 "@N" 符号重定位（JIT 符号表按 FuncRef 序注册）。
                            sink.add_reloc(
                                fixup,
                                crate::RelocKind::CALL,
                                &format!("@{}", -rel - 1),
                                0,
                            );
                        } else {
                            sink.use_label_at(fixup, forge_ir::Block(rel as u32), crate::RelocKind::REL4);
                        }
                        Ok(())
                    }
                });
            }
        }
    }
    let label_arms = label_arms;
    // GlobalAddr 专用指令（TOML 声明 `global_reloc`）：imm 槽 < 0 时编码
    // GlobalId（-(id+1)）→ 重定位 "G{id}"（JIT/QEMU 打包按全局变量注册）；
    // >= 0 时是普通立即数。三种语义：
    // - "abs8"（x86 MOVABS_GLOBAL）：符号在指令末尾 8 字节（imm64），
    //   reloc = ABS8；
    // - "pcrel_hi"/"pcrel_lo"（riscv AUIPC_GLOBAL/ADDI_GLOBAL）：定宽 4 字节，
    //   fixup = 指令起始，reloc = Relative(4,0)；patcher 按 opcode 0x17/0x13
    //   分写 hi20/lo12 位段。
    let mut global_arms: Vec<TokenStream> = Vec::new();
    for info in infos {
        let Some(kind) = info.inst.global_reloc.as_deref() else {
            continue;
        };
        let vn = &info.vn;
        // 指令的 imm 槽字段（MOVABS_GLOBAL 是第 2 操作数；riscv 对是唯一 imm）
        let imm_fid = info
            .operands
            .iter()
            .find(|(_, _, s, _)| s.kind == OperandKind::Imm)
            .map(|(_, fid, _, _)| fid.clone())
            .unwrap_or_else(|| format_ident!("imm"));
        let body = match kind {
            "abs8" => quote! {
                let bytes = encode(inst).map_err(|e| crate::EncodeError::Other(e))?;
                let imm = match inst {
                    Inst::#vn { #imm_fid, .. } => *#imm_fid,
                    _ => unreachable!(),
                };
                let fixup = sink.offset() + bytes.len() - 8;
                sink.put_bytes(&bytes);
                if imm < 0 {
                    sink.add_reloc(fixup, crate::RelocKind::ABS8, &format!("G{}", -imm - 1), 0);
                }
                Ok(())
            },
            "pcrel_hi" | "pcrel_lo" => quote! {
                let bytes = encode(inst).map_err(|e| crate::EncodeError::Other(e))?;
                let imm = match inst {
                    Inst::#vn { #imm_fid, .. } => *#imm_fid,
                    _ => unreachable!(),
                };
                let __base = sink.offset();
                sink.put_bytes(&bytes);
                if imm < 0 {
                    sink.add_reloc(
                        __base,
                        crate::RelocKind::Relative(4, 0),
                        &format!("G{}", -imm - 1),
                        0,
                    );
                }
                Ok(())
            },
            _ => unreachable!("validated: {kind}"),
        };
        global_arms.push(quote! { Inst::#vn { .. } => { #body } });
    }
    let global_arms = global_arms;
    Ok(quote! {
        pub struct Encoder;

        impl crate::machine::encoder::TargetEncoder for Encoder {
            type Inst = Inst;

            fn encode(
                &self,
                inst: &Self::Inst,
                _reg_map: &crate::AllocResult,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::EncodeError> {
                match inst {
                    #(#label_arms)*
                    #(#global_arms)*
                    _ => {
                        let bytes = encode(inst).map_err(|e| crate::EncodeError::Other(e))?;
                        sink.put_bytes(&bytes);
                        Ok(())
                    }
                }
            }

            fn encode_to_bytes(
                &self,
                inst: &Self::Inst,
                _reg_map: &crate::AllocResult,
            ) -> Result<Vec<u8>, crate::EncodeError> {
                encode(inst).map_err(|e| crate::EncodeError::Other(e))
            }
        }
    })
}

// ─────────────────────── TargetDecoder ───────────────────────

fn gen_decoder() -> TokenStream {
    quote! {
        pub struct Decoder;

        impl crate::machine::decoder::TargetDecoder for Decoder {
            type Inst = Inst;

            fn decode(&self, bytes: &[u8]) -> Result<(Self::Inst, usize), crate::machine::decoder::DecodeError> {
                // E：decode 错误带**部分匹配偏移**（变长 ISA 的前缀已消费字节数；
                // 定宽失败 → 0）。硬契约：成功路径 decode(encode(x)) == x。
                decode_partial(bytes).map_err(|n| {
                    crate::machine::decoder::DecodeError::InvalidBytes(n)
                })
            }
        }
    }
}

// ─────────────────────── TargetDisassembler ───────────────────────

fn gen_disasm(_infos: &[InstInfo]) -> Result<TokenStream, String> {
    Ok(quote! {
        pub struct Disassembler;

        impl crate::machine::disasm::TargetDisassembler for Disassembler {
            type Inst = Inst;

            fn disassemble(&self, inst: &Self::Inst) -> String {
                disassemble(inst)
            }
        }
    })
}

// ─────────────────────── TargetAssembler ───────────────────────

fn gen_assembler(model: &V12Model) -> TokenStream {
    let comment = model.meta.comment_char.chars().next().unwrap_or('#');
    let label_suf = model.meta.label_suffix.clone();
    let label_suf_lit = syn::LitStr::new(&label_suf, proc_macro2::Span::call_site());
    let label_suf_len = label_suf.len();
    let dir_pre = model.meta.directive_prefix.clone();
    let dir_pre_lit = syn::LitStr::new(&dir_pre, proc_macro2::Span::call_site());
    let align_pad = model.emit.as_ref().and_then(|e| e.align_pad).unwrap_or(0);
    quote! {
        pub struct Assembler;

        impl crate::machine::assembler::TargetAssembler for Assembler {
            type Inst = Inst;

            /// 两遍布局：行解析（标签定义 / 指令 / 符号引用）→ 符号回填。
            ///
            /// label 槽语义与 encode 契约一致：分支 label 操作数 = Block 索引
            /// （`use_label_at` 在链接期把 Block 解析为实际偏移）；未定义标签 →
            /// `AsmError::UndefinedLabel`。符号引用在前向/后向标签均可解析。
            fn parse_insts(
                &self,
                source: &str,
            ) -> Result<Vec<Self::Inst>, crate::machine::assembler::AsmError> {
                use crate::machine::assembler::AsmError;
                // ── 预处理：.macro/.endm 展开（文本级）──
                let expanded = __expand_macros(source).map_err(AsmError::Other)?;
                // ── 第一遍：行解析（含伪指令展开与字节偏移跟踪）──
                let mut insts: Vec<Inst> = Vec::new();
                let mut labels: std::collections::BTreeMap<String, u32> =
                    std::collections::BTreeMap::new();
                // .equ 符号常量（立即数求值用；thread_local 表）
                __EQU.with(|m| m.borrow_mut().clear());
                // (指令序号, 操作数序号, 符号名, 行号)
                let mut pending: Vec<(usize, usize, String, usize)> = Vec::new();
                let mut __offset: u64 = 0;
                for (__line_no, line) in expanded.lines().enumerate() {
                    let line = strip_comment(line, #comment);
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    // 行号辅助：错误消息带行号（汇编器结构化错误）
                    let line_err = |e: AsmError| -> AsmError {
                        match e {
                            AsmError::ParseError(m) => {
                                AsmError::ParseError(format!("line {}: {m}", __line_no + 1))
                            }
                            AsmError::UndefinedLabel(m) => {
                                AsmError::UndefinedLabel(format!("line {}: {m}", __line_no + 1))
                            }
                            AsmError::Ambiguous(m) => {
                                AsmError::Ambiguous(format!("line {}: {m}", __line_no + 1))
                            }
                            AsmError::TypeMismatch {
                                mnemonic,
                                index,
                                expected,
                                got,
                            } => AsmError::TypeMismatch {
                                mnemonic: format!("line {}: {mnemonic}", __line_no + 1),
                                index,
                                expected,
                                got,
                            },
                            AsmError::Other(m) => {
                                AsmError::Other(format!("line {}: {m}", __line_no + 1))
                            }
                        }
                    };
                    // 标签定义：`name + label_suffix`
                    let rest = if let Some(pos) = line.find(#label_suf_lit) {
                        let name = line[..pos].trim();
                        if name.is_empty() {
                            return Err(line_err(AsmError::ParseError("empty label name".into())));
                        }
                        labels.insert(name.to_string(), insts.len() as u32);
                        line[pos + #label_suf_len..].trim()
                    } else {
                        line
                    };
                    if rest.is_empty() {
                        continue;
                    }
                    // 伪指令：.byte / .word / .align / .global / .extern / .equ / ...
                    if let Some(dir) = rest.strip_prefix(#dir_pre_lit) {
                        let (name, args) = match dir.split_once(char::is_whitespace) {
                            Some((n, a)) => (n, a.trim()),
                            None => (dir, ""),
                        };
                        let dir_err = |msg: String| line_err(AsmError::Other(msg));
                        match name {
                            "equ" => {
                                // .equ name, expr —— 符号常量（立即数求值用）
                                let (sym, expr) = args.split_once(',').ok_or_else(|| {
                                    dir_err("`.equ` needs `name, expr`".into())
                                })?;
                                let sym = sym.trim();
                                let expr = expr.trim();
                                if sym.is_empty() || expr.is_empty() {
                                    return Err(dir_err("`.equ` needs `name, expr`".into()));
                                }
                                let toks = __lex(expr).map_err(AsmError::Other)?;
                                let mut it = __Iter { toks: &toks, pos: 0 };
                                let v = __expr(&mut it, false).ok_or_else(|| {
                                    dir_err(format!("`.equ {sym}`: 表达式求值失败 '{expr}'"))
                                })?;
                                if !it.eof() {
                                    return Err(dir_err(format!(
                                        "`.equ {sym}`: 表达式尾部多余 token '{expr}'"
                                    )));
                                }
                                __EQU.with(|m| m.borrow_mut().insert(sym.to_string(), v));
                            }
                            "byte" => {
                                let mut bytes = Vec::new();
                                for tok in args.split(',') {
                                    let t = tok.trim();
                                    if t.is_empty() {
                                        return Err(dir_err("`.byte` needs at least one value".into()));
                                    }
                                    bytes.push(__eval_byte(t).ok_or_else(|| {
                                        dir_err(format!("`.byte` 表达式求值失败 '{t}'"))
                                    })?);
                                }
                                __offset += bytes.len() as u64;
                                insts.push(Inst::Raw(bytes));
                            }
                            "word" | "hword" | "dword" => {
                                // 数据伪指令：按 endian 写多字节（word=4/hword=2/dword=8）
                                let width: usize = match name {
                                    "word" => 4,
                                    "hword" => 2,
                                    _ => 8,
                                };
                                let mut bytes = Vec::new();
                                for tok in args.split(',') {
                                    let t = tok.trim();
                                    if t.is_empty() {
                                        return Err(dir_err(format!(
                                            "`.{name}` needs at least one value"
                                        )));
                                    }
                                    let v = __eval_word(t).ok_or_else(|| {
                                        dir_err(format!("`.{name}` 表达式求值失败 '{t}'"))
                                    })?;
                                    // 小端：低字节在前（[meta].endian 当前全小端）
                                    bytes.extend_from_slice(&v.to_le_bytes()[..width]);
                                }
                                __offset += bytes.len() as u64;
                                insts.push(Inst::Raw(bytes));
                            }
                            "ascii" | "asciz" => {
                                let s = parse_string_lit(args).ok_or_else(|| {
                                    dir_err(format!("`.{name}` 需要字符串字面量"))
                                })?;
                                let mut bytes = s.into_bytes();
                                if name == "asciz" {
                                    bytes.push(0);
                                }
                                __offset += bytes.len() as u64;
                                insts.push(Inst::Raw(bytes));
                            }
                            "zero" => {
                                let n: usize = args
                                    .trim()
                                    .parse()
                                    .map_err(|_| dir_err(format!("`.zero` bad size '{args}'")))?;
                                __offset += n as u64;
                                if n > 0 {
                                    insts.push(Inst::Raw(vec![0u8; n]));
                                }
                            }
                            "align" => {
                                let n: usize = args
                                    .trim()
                                    .parse()
                                    .map_err(|_| dir_err(format!("`.align` bad size '{args}'")))?;
                                if n == 0 {
                                    return Err(dir_err("`.align` size must be > 0".into()));
                                }
                                let pad = (n - (__offset as usize % n)) % n;
                                __offset += pad as u64;
                                if pad > 0 {
                                    insts.push(Inst::Raw(vec![#align_pad as u8; pad]));
                                }
                            }
                            "global" | "extern" => {
                                // 符号登记（当前无消费方，接受并忽略）
                                if args.trim().is_empty() {
                                    return Err(dir_err(format!("`.{name}` needs a symbol name")));
                                }
                            }
                            other => {
                                return Err(dir_err(format!("unknown directive '.{other}'")));
                            }
                        }
                        continue;
                    }
                    let (inst, syms) = __assemble(rest).map_err(|e| line_err(AsmError::Other(e)))?;
                    __offset += encode(&inst).map_err(|e| line_err(AsmError::Other(e)))?.len() as u64;
                    for (oi, s) in syms {
                        pending.push((insts.len(), oi, s, __line_no + 1));
                    }
                    insts.push(inst);
                }
                // ── 第二遍：符号回填 ──
                for (i, oi, sym, line_no) in &pending {
                    let block = labels.get(sym).copied().ok_or_else(|| {
                        AsmError::UndefinedLabel(format!("line {line_no}: {sym}"))
                    })?;
                    if !__set_label_operand(&mut insts[*i], *oi, block as i64) {
                        return Err(AsmError::Other(format!(
                            "line {line_no}: cannot resolve label '{sym}' at operand {oi}"
                        )));
                    }
                }
                Ok(insts)
            }
        }

        /// 行内注释剥离（`[meta].comment_char`）。
        fn strip_comment(line: &str, c: char) -> &str {
            line.split_once(c).map_or(line, |(head, _)| head)
        }

        /// `.byte` 值求值：表达式（数字/符号常量/算术）→ u8。
        /// 越界（<0 或 >255）返回 None（调用方报错，不静默截断）。
        fn __eval_byte(t: &str) -> Option<u8> {
            let toks = __lex(t).ok()?;
            let mut it = __Iter { toks: &toks, pos: 0 };
            let v = __expr(&mut it, false)?;
            if !it.eof() {
                return None;
            }
            if !(0..=255).contains(&v) {
                return None;
            }
            Some(v as u8)
        }

        /// `.word/.hword/.dword` 值求值：表达式 → i64（截断由调用方按宽度）。
        fn __eval_word(t: &str) -> Option<i64> {
            let toks = __lex(t).ok()?;
            let mut it = __Iter { toks: &toks, pos: 0 };
            let v = __expr(&mut it, false)?;
            if !it.eof() {
                return None;
            }
            Some(v)
        }

        /// 字符串字面量解析：`"..."`（支持 \\n \\t \\\" \\\\ 转义）。
        fn parse_string_lit(t: &str) -> Option<String> {
            let t = t.trim();
            let body = t.strip_prefix('"').and_then(|x| x.strip_suffix('"'))?;
            let mut out = String::new();
            let mut cs = body.chars();
            while let Some(c) = cs.next() {
                if c == '\\' {
                    match cs.next()? {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '0' => out.push('\0'),
                        other => {
                            out.push('\\');
                            out.push(other);
                        }
                    }
                } else {
                    out.push(c);
                }
            }
            Some(out)
        }

        /// `.macro name args...` / `.endm` 展开：收集宏定义，调用点按参数
        /// 文本替换展开。嵌套调用支持（展开深度上限 16 防递归死循环）。
        /// 宏体中的参数以 `\arg` 或 `%arg` 引用（简化：`\arg`）。
        fn __expand_macros(source: &str) -> Result<String, String> {
            #[derive(Clone)]
            struct Macro {
                params: Vec<String>,
                body: Vec<String>,
            }
            let mut macros: std::collections::HashMap<String, Macro> =
                std::collections::HashMap::new();
            let mut out_lines: Vec<String> = Vec::new();
            let mut lines = source.lines().peekable();
            let mut depth = 0usize;
            while let Some(line) = lines.next() {
                let trimmed = line.trim();
                if let Some(dir) = trimmed.strip_prefix(#dir_pre_lit) {
                    let (name, args) = match dir.split_once(char::is_whitespace) {
                        Some((n, a)) => (n.trim(), a.trim()),
                        None => (dir.trim(), ""),
                    };
                    if name == "macro" {
                        let (mname, params) = match args.split_once(char::is_whitespace) {
                            Some((m, p)) => (m, p),
                            None => (args, ""),
                        };
                        let params: Vec<String> = if params.is_empty() {
                            Vec::new()
                        } else {
                            params.split(',').map(|s| s.trim().to_string()).collect()
                        };
                        if macros.contains_key(mname) {
                            return Err(format!("macro '{mname}' 重复定义"));
                        }
                        let mut body: Vec<String> = Vec::new();
                        let mut found_end = false;
                        for bline in lines.by_ref() {
                            let bt = bline.trim();
                            if bt.strip_prefix(#dir_pre_lit)
                                .is_some_and(|d| d.trim() == "endm")
                            {
                                found_end = true;
                                break;
                            }
                            body.push(bline.to_string());
                        }
                        if !found_end {
                            return Err(format!("macro '{mname}' 缺少 .endm"));
                        }
                        macros.insert(
                            mname.to_string(),
                            Macro { params, body },
                        );
                        continue;
                    }
                    if name == "endm" {
                        return Err("orphan .endm（无 .macro）".into());
                    }
                }
                // 宏调用：`mname arg1, arg2`（首词是宏名）
                if let Some((head, rest)) = trimmed.split_once(char::is_whitespace) {
                    if let Some(m) = macros.get(head.trim()) {
                        let arg_strs: Vec<String> = if rest.trim().is_empty() {
                            Vec::new()
                        } else {
                            rest.split(',').map(|s| s.trim().to_string()).collect()
                        };
                        if arg_strs.len() != m.params.len() {
                            return Err(format!(
                                "宏 '{head}' 参数数不匹配：期望 {}、实际 {}",
                                m.params.len(),
                                arg_strs.len()
                            ));
                        }
                        // 参数替换（多轮直到无变化；`\arg`/`%arg` 引用）
                        let mut body = m.body.clone();
                        let mut rounds = 0;
                        loop {
                            rounds += 1;
                            if rounds > 16 {
                                return Err(format!("宏 '{head}' 展开超过 16 轮（递归?）"));
                            }
                            let mut changed = false;
                            let mut nb: Vec<String> = Vec::new();
                            for bl in &body {
                                let mut nl = bl.clone();
                                for (i, p) in m.params.iter().enumerate() {
                                    let rep = arg_strs.get(i).cloned().unwrap_or_default();
                                    for pat in [format!("\\{p}"), format!("%{p}")] {
                                        if nl.contains(&pat) {
                                            nl = nl.replace(&pat, &rep);
                                            changed = true;
                                        }
                                    }
                                }
                                nb.push(nl);
                            }
                            body = nb;
                            if !changed {
                                break;
                            }
                        }
                        // 嵌套宏：展开后的行若仍是宏调用，递归展开（模拟宏内宏）。
                        // 嵌套宏体应**就地**插入当前位置（保持源顺序）——用 Vec
                        // 索引扫描而非队列（队列会把嵌套体放到队尾，颠倒顺序）。
                        let mut pending_lines: Vec<String> = body;
                        let mut expanded_out: Vec<String> = Vec::new();
                        let mut guard = 0;
                        while !pending_lines.is_empty() {
                            let pl = pending_lines.remove(0);
                            guard += 1;
                            if guard > 64 {
                                return Err(format!("宏 '{head}' 嵌套展开超过 64 行（递归?）"));
                            }
                            let pt = pl.trim();
                            if let Some((ph, pr)) = pt.split_once(char::is_whitespace) {
                                if let Some(pm) = macros.get(ph.trim()) {
                                    let pargs: Vec<String> = if pr.trim().is_empty() {
                                        Vec::new()
                                    } else {
                                        pr.split(',').map(|s| s.trim().to_string()).collect()
                                    };
                                    if pargs.len() != pm.params.len() {
                                        return Err(format!(
                                            "宏 '{ph}' 参数数不匹配：期望 {}、实际 {}",
                                            pm.params.len(),
                                            pargs.len()
                                        ));
                                    }
                                    let mut pbody = pm.body.clone();
                                    let mut prounds = 0;
                                    loop {
                                        prounds += 1;
                                        if prounds > 16 {
                                            return Err(format!("宏 '{ph}' 展开超过 16 轮"));
                                        }
                                        let mut pchanged = false;
                                        let mut pnb: Vec<String> = Vec::new();
                                        for bl in &pbody {
                                            let mut nl = bl.clone();
                                            for (i, p) in pm.params.iter().enumerate() {
                                                let rep =
                                                    pargs.get(i).cloned().unwrap_or_default();
                                                for pat in [format!("\\{p}"), format!("%{p}")] {
                                                    if nl.contains(&pat) {
                                                        nl = nl.replace(&pat, &rep);
                                                        pchanged = true;
                                                    }
                                                }
                                            }
                                            pnb.push(nl);
                                        }
                                        pbody = pnb;
                                        if !pchanged {
                                            break;
                                        }
                                    }
                                    // 就地插入：嵌套体插到当前扫描位置
                                    for bl in pbody.into_iter().rev() {
                                        pending_lines.insert(0, bl);
                                    }
                                    continue;
                                }
                            }
                            expanded_out.push(pl);
                        }
                        out_lines.extend(expanded_out);
                        continue;
                    }
                }
                out_lines.push(line.to_string());
            }
            Ok(out_lines.join("\n"))
        }
    }
}

// ─────────────────────── TargetABI ───────────────────────

fn gen_abi(model: &V12Model) -> Result<TokenStream, String> {
    // [abi] → arg_regs（按 arg_class 顺序：int 类在前，其余 class 依次）。
    // ret_regs：缺省空（v12 声明层暂不区分返回寄存器——后续迭代扩展）。
    let stack_align = model.abi.as_ref().and_then(|a| a.stack_align).unwrap_or(16);
    let mut arg_regs: Vec<TokenStream> = Vec::new();
    let mut by_ref_limit: Option<u32> = None;
    if let Some(abi) = &model.abi {
        for ac in &abi.arg_class {
            for r in &ac.regs {
                let i = format_ident!("{r}");
                arg_regs.push(quote! { Reg::#i });
            }
            // by-ref 策略：`strategy = "by-ref"` + `limit`（位）→ 超过该位宽的
            // 向量按引用传参（阈值字节 = limit/8；x86 声明 128 位 → 16 字节）。
            if ac.strategy.as_deref() == Some("by-ref")
                && let Some(bits) = ac.limit
            {
                if bits % 8 != 0 {
                    return Err(format!(
                        "[abi.arg_class.{}]: by-ref limit {bits} 不是 8 的倍数（应为位宽）",
                        ac.class
                    ));
                }
                by_ref_limit = Some(bits / 8);
            }
        }
    }
    let by_ref_toks: TokenStream = match by_ref_limit {
        Some(b) => quote! { Some(#b) },
        None => quote! { None },
    };
    let min_frame = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .and_then(|f| f.min_frame_bytes)
        .unwrap_or(0);
    let csb_override = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .and_then(|f| f.callee_saved_bytes_override);
    let csb_toks: TokenStream = match csb_override {
        Some(v) => quote! { Some(#v) },
        None => quote! { None },
    };
    let sss = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .and_then(|f| f.stack_slot_shift);
    let sss_toks: TokenStream = match sss {
        Some(v) => quote! { Some(#v) },
        None => quote! { None },
    };
    // 返回寄存器：[abi].ret_regs（物理名）→ Reg::NAME；缺省空 = index 0
    //（x86 RAX 语义，由 Return/Call lowering 的 from_index(0) 兜底）。
    let mut ret_regs: Vec<TokenStream> = Vec::new();
    if let Some(abi) = &model.abi {
        for r in &abi.ret_regs {
            let i = format_ident!("{r}");
            ret_regs.push(quote! { Reg::#i });
        }
    }
    // 若没有 arg_class（或 regs 空）→ 空 arg_regs（集成层仍可组装）。
    Ok(quote! {
        pub struct ABI;

        impl crate::machine::abi::TargetABI for ABI {
            type Reg = Reg;

            fn arg_regs(&self) -> Vec<Self::Reg> {
                vec![#(#arg_regs),*]
            }
            fn ret_regs(&self) -> Vec<Self::Reg> {
                vec![#(#ret_regs),*]
            }
            fn stack_align(&self) -> u32 { #stack_align }
            fn vector_by_ref_limit(&self) -> Option<u32> { #by_ref_toks }
            fn min_frame_bytes(&self) -> u32 { #min_frame }
            fn callee_saved_bytes_override(&self) -> Option<u32> { #csb_toks }
            fn stack_slot_shift(&self) -> Option<i32> { #sss_toks }
        }
    })
}

// ─────────────────────── TargetFrameLowering ───────────────────────

fn gen_frame_lowering(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    // 指令名 → InstInfo（[emit]/[spill] 模板用大写指令名引用）。
    let prologue_toks = gen_emit_block(infos, model, true)?;
    let epilogue_toks = gen_emit_block(infos, model, false)?;

    // 独立尾声标签：缺省 true（x86 语义：return block 经 epilogue_jump 跳到
    // 统一尾声）。定宽 ISA 无 JMP 指令时可设 false（[emit].epilogue_label）：
    // return block 不发 jump，尾声直接顺序发在 return block 之后——仅单
    // return block 函数安全（多 return block 会 fall-through 错序）。
    let needs_epilogue_label = model
        .emit
        .as_ref()
        .and_then(|e| e.epilogue_label)
        .unwrap_or(true);

    // 尾声跳转指令选择：x86 JMP_REL32（0xE9 rel32）手写；定宽（riscv）
    // JAL x0, epilogue_label（label 槽 = 块号 → encoder 定宽 fixup
    // Relative(4,0)，位段由 RiscvRelocPatcher 编码）。
    let has_jmp_f = infos.iter().any(|i| i.inst.name == "JMP_REL32");
    let has_jal_f = infos.iter().any(|i| i.inst.name == "JAL");
    let jal_f = inst_fids(infos, "JAL");
    let (jal_dest, jal_target) = if jal_f.len() >= 2 {
        (jal_f[0].clone(), jal_f[1].clone())
    } else {
        (format_ident!("dest"), format_ident!("target"))
    };
    let epilogue_jump_body: TokenStream = if has_jmp_f {
        quote! {
            // JMP rel32（0xE9 + 占位）+ label 记录（链接期回填）。
            sink.put1(0xE9);
            let fixup = sink.offset();
            sink.put4(0u32);
            sink.use_label_at(fixup, epilogue_block, crate::RelocKind::REL4);
            Ok(())
        }
    } else if has_jal_f {
        quote! {
            // jal x0, epilogue_block——label 槽 = 块号 → encoder 编码时
            // use_label_at（定宽 fixup = 指令起始；位段重排由 patcher）。
            let inst = Inst::Jal {
                #jal_dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
                #jal_target: epilogue_block.0 as i64,
            };
            encoder.encode(&inst, reg_map, sink).map_err(|e| {
                crate::IrError::Internal(format!("epilogue jump encode: {e}"))
            })?;
            Ok(())
        }
    } else {
        quote! {
            Err(crate::IrError::Unsupported(
                "epilogue jump: need JMP_REL32 or JAL".into(),
            ))
        }
    };

    // spill load/store：`{0}` = 寄存器、`{1}` = 帧偏移、基址来自模板 base。
    let spill_gpr = model.spill.get("GPR");
    let spill_fpr = model.spill.get("FPR");
    let gpr_load = gen_spill_stmt(infos, spill_gpr, true)?;
    let gpr_store = gen_spill_stmt(infos, spill_gpr, false)?;
    let fpr_load = gen_spill_stmt(infos, spill_fpr, true)?;
    let fpr_store = gen_spill_stmt(infos, spill_fpr, false)?;

    Ok(quote! {
        pub struct FrameLowering;

        impl crate::machine::frame::TargetFrameLowering for FrameLowering {
            type Inst = Inst;

            fn needs_epilogue_label(&self) -> bool { #needs_epilogue_label }

            fn emit_epilogue_jump(
                &self,
                encoder: &std::sync::Arc<dyn crate::machine::encoder::TargetEncoder<Inst = Self::Inst>>,
                reg_map: &crate::AllocResult,
                epilogue_block: forge_ir::Block,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::IrError> {
                #epilogue_jump_body
            }

            fn emit_prologue(
                &self,
                frame_size: u32,
                reg_map: &crate::AllocResult,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::IrError> {
                let __frame_size = frame_size;
                let __rm = reg_map;
                let __sink = sink;
                #prologue_toks
                Ok(())
            }

            fn emit_epilogue(
                &self,
                frame_size: u32,
                reg_map: &crate::AllocResult,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::IrError> {
                let __frame_size = frame_size;
                let __rm = reg_map;
                let __sink = sink;
                #epilogue_toks
                Ok(())
            }

            fn emit_spill_load(
                &self,
                dst_reg: u32,
                offset: i32,
                width: u16,
                is_fp: bool,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::IrError> {
                let __dst = dst_reg;
                let __off = offset as i64;
                let __sink = sink;
                if is_fp {
                    #fpr_load
                } else {
                    #gpr_load
                }
                let _ = width;
                Ok(())
            }

            fn emit_spill_store(
                &self,
                src_reg: u32,
                offset: i32,
                width: u16,
                is_fp: bool,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::IrError> {
                let __dst = src_reg;
                let __off = offset as i64;
                let __sink = sink;
                if is_fp {
                    #fpr_store
                } else {
                    #gpr_store
                }
                let _ = width;
                Ok(())
            }
        }
    })
}

/// 展开 [emit] prologue/epilogue 模板为编码语句序列。
///
/// 模板行：`INST op0, op1, ...`（物理寄存器/数字立即数）或 `@伪指令`
/// （@push_callee/@pop_callee/@frame_alloc/@frame_free）。emit 模式：
/// 无 XReg 映射——物理寄存器写死物理索引，imm 写死值，直接编码进 sink。
fn gen_emit_block(
    infos: &[InstInfo],
    model: &V12Model,
    is_prologue: bool,
) -> Result<TokenStream, String> {
    let Some(emit) = &model.emit else {
        return Ok(quote! {});
    };
    let block = if is_prologue {
        emit.prologue.as_ref()
    } else {
        emit.epilogue.as_ref()
    };
    let Some(block) = block else {
        return Ok(quote! {});
    };
    let mut out: Vec<TokenStream> = Vec::new();
    for t in &block.insts {
        let trimmed = t.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // @ 伪指令
        if let Some(name) = trimmed.strip_prefix('@') {
            out.push(gen_emit_pseudo(infos, model, name)?);
            continue;
        }
        let stmts = gen_emit_inst(infos, trimmed)?;
        out.extend(stmts);
    }
    Ok(quote! { #(#out)* })
}

/// 单个 emit 指令行 → 构造 + 编码语句（emit 模式）。
fn gen_emit_inst(infos: &[InstInfo], line: &str) -> Result<Vec<TokenStream>, String> {
    let (inst_name, ops) = match line.split_once(char::is_whitespace) {
        Some((n, rest)) => (n.trim(), rest.trim()),
        None => (line.trim(), ""),
    };
    let info = inst_info_by_name(infos, inst_name)
        .ok_or_else(|| format!("emit 模板引用了未知指令 '{inst_name}'（行: {line}）"))?;
    let vn = &info.vn;
    let mut bindings: Vec<(String, TokenStream)> = Vec::new();
    if !ops.is_empty() {
        for (i, op) in ops.split(',').map(|s| s.trim()).enumerate() {
            if op.is_empty() {
                continue;
            }
            let fid = info
                .operands
                .get(i)
                .map(|(_, fid, _, _)| fid.clone())
                .ok_or_else(|| format!("emit 模板操作数 {i} 超出指令 {inst_name}（行: {line}）"))?;
            let slot = &info.operands[i].2;
            let expr: TokenStream = match op {
                other if slot.kind == OperandKind::Reg => {
                    // 物理寄存器名 → Reg 枚举（emit 模式直接写死）
                    let reg = format_ident!("{other}");
                    quote! { Reg::#reg }
                }
                other if slot.kind == OperandKind::Imm || slot.kind == OperandKind::Label => {
                    // 占位符：{frame_size} / {frame_size_neg} / {frame_size_mN}
                    //（emit 模式下由运行时 frame_size 派生；riscv 的 ra/fp
                    // 保存槽偏移 = frame_size - N）
                    let expr: TokenStream = match other {
                        "{frame_size}" => quote! { __frame_size as i64 },
                        "{frame_size_neg}" => quote! { -(__frame_size as i64) },
                        _ if other.starts_with("{frame_size_m") && other.ends_with('}') => {
                            let n: i64 = other["{frame_size_m".len()..other.len() - 1]
                                .parse()
                                .map_err(|_| format!("emit 模板占位符 '{other}' 无法解析"))?;
                            quote! { (__frame_size as i64) - #n }
                        }
                        _ => {
                            let v: i64 = other
                                .parse()
                                .map_err(|_| format!("emit 模板立即数 '{other}' 无法解析"))?;
                            quote! { #v }
                        }
                    };
                    expr
                }
                other => {
                    return Err(format!(
                        "emit 模板操作数 '{other}' 不支持（指令 {inst_name} 槽 {}）",
                        slot.kind.kind_name()
                    ));
                }
            };
            bindings.push((fid.to_string(), expr));
        }
    }
    let fields: Vec<TokenStream> = info
        .operands
        .iter()
        .map(|(_, fid, slot, _)| {
            let fid_ident = format_ident!("{fid}");
            let expr = bindings
                .iter()
                .find(|(f, _)| f == &fid.to_string())
                .map(|(_, e)| e.clone())
                .unwrap_or_else(|| field_ctor_expr(slot, quote! { 0u32 }));
            quote! { #fid_ident: #expr }
        })
        .collect();
    let ctor = if info.operands.is_empty() {
        quote! { Inst::#vn }
    } else {
        quote! { Inst::#vn { #(#fields),* } }
    };
    Ok(vec![quote! {
        let __bytes = encode(&#ctor).map_err(|e| crate::IrError::Emit(e))?;
        __sink.put_bytes(&__bytes);
    }])
}

/// 按指令名查找 InstInfo（emit/spill 模板用）。
fn inst_info_by_name<'a>(infos: &'a [InstInfo<'a>], name: &str) -> Option<&'a InstInfo<'a>> {
    infos.iter().find(|i| i.inst.name == name)
}

/// emit @ 伪指令（push_callee/pop_callee/frame_alloc/frame_free）。
fn gen_emit_pseudo(
    infos: &[InstInfo],
    model: &V12Model,
    name: &str,
) -> Result<TokenStream, String> {
    match name {
        "push_callee" | "pop_callee" => {
            let Some(abi) = &model.abi else {
                return Ok(quote! {});
            };
            let Some(callee) = &abi.callee_saved else {
                return Ok(quote! {});
            };
            if callee.gpr.is_empty() {
                return Ok(quote! {});
            }
            // x86：PUSH/POP（+r 形式，硬件递减 sp）。定宽 ISA 无 push/pop
            // 指令（riscv）→ 用 [spill.GPR] store/load 模板指令（SD/LD）存到
            // 帧槽 [sp + frame - fp_push - (k+1)*8]（frame_alloc 之后执行，
            // 槽在已分配帧顶部 fp_push 区域之下；min_frame_bytes 需覆盖）。
            if inst_fids(infos, "PUSH").is_empty() && inst_fids(infos, "POP").is_empty() {
                let Some(frame) = &abi.frame else {
                    return Err(
                        "@push_callee: 定宽 ISA 需要 [abi.frame]（sp/fp_push_bytes）".into(),
                    );
                };
                let sp = format_ident!("{}", frame.sp);
                let fp_push = frame.fp_push_bytes.unwrap_or(8) as i64;
                let spill_tpl = model
                    .spill
                    .get("GPR")
                    .ok_or_else(|| "@push_callee: 定宽 ISA 需要 [spill.GPR]".to_string())?;
                let template = if name == "push_callee" {
                    spill_tpl.store.as_str()
                } else {
                    spill_tpl.load.as_str()
                };
                let inst_name = template
                    .split(char::is_whitespace)
                    .next()
                    .ok_or_else(|| "spill 模板为空".to_string())?;
                let vn = crate::v12::codegen::pascal_ident(inst_name);
                let fid_list = inst_fids(infos, inst_name);
                let (f_reg, f_base, f_imm) = match fid_list.len() {
                    3 => (
                        fid_list[0].clone(),
                        fid_list[1].clone(),
                        fid_list[2].clone(),
                    ),
                    _ => {
                        return Err(format!(
                            "@push_callee: [{inst_name}] 需 3 操作数 (reg, base, imm)"
                        ));
                    }
                };
                // **按需保存**（阶段 G）：只 push/pop 实际分配到的 callee-saved
                //（regalloc 的 callee_saved_to_save；不再全量保存 TOML 列表）。
                // 槽偏移按正向声明序：push 正向遍历（saved[k]→槽 k）；
                // pop **逆序遍历 + 槽 n-1-k**：恢复的寄存器 = 正向[n-1-k]，
                // 槽也必须 = n-1-k（reg 与槽同源）——否则 pop 读错槽
                //（实测返回值错/挂起）。
                let push = name == "push_callee";
                let (iter, k_expr) = if push {
                    (quote! { __saved.iter() }, quote! { __k as i64 })
                } else {
                    (
                        quote! { __saved.iter().rev() },
                        quote! { __n as i64 - 1 - __k as i64 },
                    )
                };
                return Ok(quote! {
                    let __saved = __rm.callee_saved_to_save.clone();
                    let __n = __saved.len();
                    for (__k, __preg) in #iter.enumerate() {
                        let __reg = Reg::from_index(__preg.num, __preg.class);
                        let __off_val: i64 = (#k_expr + 1) * 8;
                        let __off = __frame_size as i64 - #fp_push - __off_val;
                        let __bytes = encode(&Inst::#vn {
                            #f_reg: __reg,
                            #f_base: Reg::#sp,
                            #f_imm: __off,
                        })
                        .map_err(|e| crate::IrError::Emit(e))?;
                        __sink.put_bytes(&__bytes);
                    }
                });
            }
            let regs: Vec<String> = if name == "push_callee" {
                callee.gpr.clone()
            } else {
                callee.gpr.iter().rev().cloned().collect()
            };
            let push_pop = if name == "push_callee" { "PUSH" } else { "POP" };
            let vn = crate::v12::codegen::pascal_ident(push_pop);
            // 单操作数 +r 形式：PUSH→src（in）、POP→dest（out）——字段名取操作数 0
            let f0 = inst_fids(infos, push_pop)
                .first()
                .cloned()
                .ok_or_else(|| format!("[{push_pop}] missing operand"))?;
            let mut stmts = Vec::new();
            for r in &regs {
                let reg = format_ident!("{r}");
                stmts.push(quote! {
                    let __bytes = encode(&Inst::#vn { #f0: Reg::#reg })
                        .map_err(|e| crate::IrError::Emit(e))?;
                    __sink.put_bytes(&__bytes);
                });
            }
            Ok(quote! { #(#stmts)* })
        }
        "move_args" => {
            // 收参：把 [abi.arg_class].int 类的寄存器值 mov 到参数 XReg 的
            // 分配物理寄存器。整数参数按序取 int 类 regs[gi]（v11 语义）。
            let Some(abi) = &model.abi else {
                return Ok(quote! {});
            };
            let int_regs: Vec<&String> = abi
                .arg_class
                .iter()
                .find(|ac| ac.class == "int")
                .map(|ac| ac.regs.iter().collect())
                .unwrap_or_default();
            if int_regs.is_empty() {
                return Ok(quote! {});
            }
            let regs: Vec<syn::Ident> = int_regs.iter().map(|r| format_ident!("{r}")).collect();
            let n = regs.len();
            // 浮点参数类（XMM0-3）——Windows x64 浮点参数寄存器
            let float_regs: Vec<syn::Ident> = abi
                .arg_class
                .iter()
                .find(|ac| ac.class == "float")
                .map(|ac| ac.regs.iter().map(|r| format_ident!("{r}")).collect())
                .unwrap_or_default();
            let fn_ = float_regs.len();
            // MOV_RM8_R64 src=arg_reg、dest=param 分配寄存器（0x89: reg=src、rm=dest）
            // 指令名可配置（[abi].move_inst，缺省 "MOV_RM8_R64"）——demo 等
            // 定宽 ISA 声明自己的 mov（如 "MOV64"）；字段按角色解析（In=src、
            // Out/InOut=dest），两种操作数序（x86 src/dest 与 demo dest/src）皆可。
            let move_inst = model
                .abi
                .as_ref()
                .and_then(|a| a.move_inst.clone())
                .unwrap_or_else(|| "MOV_RM8_R64".to_string());
            let mov_vn = crate::v12::codegen::pascal_ident(&move_inst);
            let (m_src, _m_src_idx, m_dest, _m_dest_idx) = inst_move_role(infos, &move_inst)
                .ok_or_else(|| {
                    format!("[{move_inst}] must have In(src)/Out|InOut(dest) reg operands")
                })?;
            // 浮点参数移动：MOVSD/MOVSS（fpr out, fpr in；reg=dest、rm=src）
            let fpr_fids = inst_fids(infos, "MOVSD");
            let has_fpr_mov = fpr_fids.len() >= 2 && inst_fids(infos, "MOVSS").len() >= 2;
            let (f_dest, f_src) = if fpr_fids.len() >= 2 {
                (fpr_fids[0].clone(), fpr_fids[1].clone())
            } else {
                (format_ident!("dest"), format_ident!("src"))
            };
            // by-ref 向量 load：宽向量参数（>16 字节）按引用传参——ABI 传 GPR
            // 指针（int 槽位），收参时从 [ptr] load 到目标向量寄存器。
            // 优先非对齐 VMOVUPS（指针未必 32 字节对齐）；缺省回退 VMOVAPS。
            // 宽度由参数 XReg 决定：32 字节（V256）→ VMOVUPS_RM（VEX ymm）；
            // 64 字节（V512）→ VMOVUPS_ZMM_MEM（EVEX zmm）。
            let byref_loads: Vec<(&str, &str, u16)> = vec![
                ("VMOVUPS_RM", "VMOVAPS_RM", 32),
                ("VMOVUPS_ZMM_MEM", "VMOVAPS_ZMM_MEM", 64),
            ];
            let mut byref_stmt: TokenStream = quote! {
                return Err(crate::IrError::Emit(
                    "v12 by-ref vector arg load missing (VMOVUPS_RM/VMOVUPS_ZMM_MEM)".into(),
                ));
            };
            for (inst_name, fallback, width) in byref_loads {
                let fids = if inst_fids(infos, inst_name).len() == 2 {
                    inst_fids(infos, inst_name)
                } else {
                    inst_fids(infos, fallback)
                };
                if fids.len() != 2 {
                    continue;
                }
                let d_fid = fids[0].clone();
                let m_fid = fids[1].clone();
                let vn = crate::v12::codegen::pascal_ident(inst_name);
                let w = width;
                byref_stmt = quote! {
                    if __pv.width() == #w {
                        let __bytes = encode(&Inst::#vn {
                            #d_fid: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                            #m_fid: MemRef {
                                base: Reg::from_index(__src.to_index(), forge_ir::RegClass::GPR64),
                                disp: 0,
                                index: None,
                                scale: 1,
                            },
                        }).map_err(|e| crate::IrError::Emit(e))?;
                        __sink.put_bytes(&__bytes);
                    } else {
                        #byref_stmt
                    }
                };
            }
            let byref_stmt = byref_stmt;
            let fpr_stmt: TokenStream = if has_fpr_mov {
                quote! {
                    if __fi < #fn_ {
                        let __src = [#(Reg::#float_regs),*][__fi];
                        __fi += 1;
                        let __bytes = encode(&if __rm.param_is_32.get(__i) == Some(&true) {
                            Inst::Movss {
                                #f_dest: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                                #f_src: __src,
                            }
                        } else {
                            Inst::Movsd {
                                #f_dest: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                                #f_src: __src,
                            }
                        }).map_err(|e| crate::IrError::Emit(e))?;
                        __sink.put_bytes(&__bytes);
                    }
                }
            } else {
                quote! {
                    let _ = __fi;
                    return Err(crate::IrError::Emit("v12 float args (MOVSD/MOVSS missing)".into()));
                }
            };
            Ok(quote! {
                let mut __gi = 0usize;
                let mut __fi = 0usize;
                for (__i, &__pv) in __rm.param_vregs.iter().enumerate() {
                    if !__rm.assignments.contains_key(&__pv) {
                        continue;
                    }
                    let __dest = match __rm.preg(__pv) {
                        Some(p) => p.num,
                        None => continue,
                    };
                    if __rm.param_by_ref.get(__i) == Some(&true) {
                        // by-ref：宽向量参数按引用传——GPR 槽位是数据指针，
                        // 从 [ptr] load 到向量寄存器（VMOVAPS_RM/ZMM_MEM）。
                        if __gi < #n {
                            let __src = [#(Reg::#regs),*][__gi];
                            __gi += 1;
                            #byref_stmt
                        }
                    } else if __rm.param_is_float.get(__i) == Some(&true) {
                        #fpr_stmt
                    } else if __gi < #n {
                        let __src = [#(Reg::#regs),*][__gi];
                        __gi += 1;
                        let __bytes = encode(&Inst::#mov_vn {
                            #m_src: __src,
                            #m_dest: Reg::from_index(__dest, forge_ir::RegClass::GPR64),

                        }).map_err(|e| crate::IrError::Emit(e))?;
                        __sink.put_bytes(&__bytes);
                    }
                }
            })
        }
        "frame_alloc" => {
            let Some(abi) = &model.abi else {
                return Ok(quote! {});
            };
            let Some(frame) = &abi.frame else {
                return Ok(quote! {});
            };
            let inst = frame
                .alloc_inst
                .as_deref()
                .ok_or_else(|| "[abi.frame].alloc_inst required for @frame_alloc".to_string())?;
            // 帧分配：所有 Reg 槽填 sp、所有 Imm/Label 槽填 frame_size。
            // x86 `SUB64_R_IMM32`（inout reg + imm32）；demo `SUBI16`
            //（out rd + in rs1 + imm8）两种形态皆可。
            let sp = format_ident!("{}", frame.sp);
            let vn = crate::v12::codegen::pascal_ident(inst);
            let (reg_fids, imm_fids) = inst_reg_imm_fids(infos, inst)
                .ok_or_else(|| format!("[{inst}] missing operands"))?;
            if imm_fids.is_empty() {
                return Err(format!(
                    "[{inst}] must have an imm operand for @frame_alloc"
                ));
            }
            let alloc_neg = frame.alloc_neg;
            let frame_size_expr: TokenStream = if alloc_neg {
                quote! { -(__frame_size as i64) }
            } else {
                quote! { __frame_size as i64 }
            };
            let reg_binds: Vec<TokenStream> = reg_fids
                .iter()
                .map(|f| {
                    quote! { #f: Reg::#sp }
                })
                .collect();
            let imm_binds: Vec<TokenStream> = imm_fids
                .iter()
                .map(|f| {
                    quote! { #f: #frame_size_expr }
                })
                .collect();
            Ok(quote! {
                if __frame_size != 0 {
                    let __bytes = encode(&Inst::#vn {
                        #(#reg_binds,)*
                        #(#imm_binds,)*
                    }).map_err(|e| crate::IrError::Emit(e))?;
                    __sink.put_bytes(&__bytes);
                }
            })
        }
        "frame_free" => {
            let Some(abi) = &model.abi else {
                return Ok(quote! {});
            };
            let Some(frame) = &abi.frame else {
                return Ok(quote! {});
            };
            let inst = frame
                .free_inst
                .as_deref()
                .ok_or_else(|| "[abi.frame].free_inst required for @frame_free".to_string())?;
            let sp = format_ident!("{}", frame.sp);
            let vn = crate::v12::codegen::pascal_ident(inst);
            let (reg_fids, imm_fids) = inst_reg_imm_fids(infos, inst)
                .ok_or_else(|| format!("[{inst}] missing operands"))?;
            if imm_fids.is_empty() {
                return Err(format!("[{inst}] must have an imm operand for @frame_free"));
            }
            let reg_binds: Vec<TokenStream> = reg_fids
                .iter()
                .map(|f| {
                    quote! { #f: Reg::#sp }
                })
                .collect();
            let imm_binds: Vec<TokenStream> = imm_fids
                .iter()
                .map(|f| {
                    quote! { #f: __frame_size as i64 }
                })
                .collect();
            Ok(quote! {
                if __frame_size != 0 {
                    let __bytes = encode(&Inst::#vn {
                        #(#reg_binds,)*
                        #(#imm_binds,)*
                    }).map_err(|e| crate::IrError::Emit(e))?;
                    __sink.put_bytes(&__bytes);
                }
            })
        }
        other => Err(format!("unknown emit pseudo '@{other}'")),
    }
}

/// spill 模板 → 单条 load/store 语句。
///
/// 两种形态：
/// - x86 式 2 操作数 + Mem 槽：`MOV64_RM {0}, {1}`（{0}=寄存器、{1}=MemRef
///   {base, __off}，base 来自模板 base 键）。
/// - riscv 式 3 操作数 reg+reg+imm：`LD {0}, X2, {1}`（{0}=寄存器、字面
///   物理寄存器 = 基址、{1}=__off 立即数）。
fn gen_spill_stmt(
    infos: &[InstInfo],
    tpl: Option<&SpillTemplate>,
    is_load: bool,
) -> Result<TokenStream, String> {
    let Some(t) = tpl else {
        return Ok(quote! {
            let _ = (__dst, __off);
        });
    };
    let template = if is_load { &t.load } else { &t.store };
    // 指令名 = 模板首词；其余操作数按位置绑定。
    let (inst_name, ops) = match template.split_once(char::is_whitespace) {
        Some((n, rest)) => (n.trim(), rest.trim()),
        None => (template.trim(), ""),
    };
    let info = inst_info_by_name(infos, inst_name)
        .ok_or_else(|| format!("spill 模板引用了未知指令 '{inst_name}'"))?;
    let base_name = t.base.clone().unwrap_or_else(|| "RBP".to_string());
    let base = format_ident!("{base_name}");
    let mut bindings: Vec<(String, TokenStream)> = Vec::new();
    if !ops.is_empty() {
        for (i, op) in ops.split(',').map(|s| s.trim()).enumerate() {
            if op.is_empty() {
                continue;
            }
            let fid = info
                .operands
                .get(i)
                .map(|(_, fid, _, _)| fid.clone())
                .ok_or_else(|| format!("spill 模板操作数 {i} 超出指令 {inst_name}"))?;
            let slot = &info.operands[i].2;
            let expr: TokenStream = match op {
                "{0}" if slot.kind == OperandKind::Reg => field_ctor_expr(slot, quote! { __dst }),
                "{1}" if slot.kind == OperandKind::Mem => {
                    quote! { MemRef { base: Reg::#base, disp: __off, index: None, scale: 1 } }
                }
                "{1}" if slot.kind == OperandKind::Imm || slot.kind == OperandKind::Label => {
                    // riscv 式：`LD {0}, X2, {1}`——基址在字面操作数、偏移在 {1}
                    quote! { __off as i64 }
                }
                other if slot.kind == OperandKind::Reg => {
                    // 字面物理寄存器名 = 基址（riscv `LD {0}, X2, {1}`）
                    let reg = format_ident!("{other}");
                    quote! { Reg::#reg }
                }
                other => {
                    return Err(format!(
                        "spill 模板操作数 '{other}' 不支持（指令 {inst_name} 槽 {}）",
                        slot.kind.kind_name()
                    ));
                }
            };
            bindings.push((fid.to_string(), expr));
        }
    }
    let fields: Vec<TokenStream> = info
        .operands
        .iter()
        .map(|(_, fid, slot, _)| {
            let fid_ident = format_ident!("{fid}");
            let expr = bindings
                .iter()
                .find(|(f, _)| f == &fid.to_string())
                .map(|(_, e)| e.clone())
                .unwrap_or_else(|| field_ctor_expr(slot, quote! { 0u32 }));
            quote! { #fid_ident: #expr }
        })
        .collect();
    let vn = crate::v12::codegen::pascal_ident(inst_name);
    let ctor = if info.operands.is_empty() {
        quote! { Inst::#vn }
    } else {
        quote! { Inst::#vn { #(#fields),* } }
    };
    Ok(quote! {
        let __bytes = encode(&#ctor).map_err(|e| crate::IrError::Emit(e))?;
        __sink.put_bytes(&__bytes);
    })
}

// ─────────────────────── IsaInfo / RegInfo ───────────────────────

/// 剥离 asm 模板中的占位符声明 `{n:[slot:role]}` → `{n}`（字面段检查用；
/// 声明里的 `[`/`]` 不是内存形状，字面段的括号保留）。
pub(crate) fn strip_placeholder_decls(asm: &str) -> String {
    let mut out = String::with_capacity(asm.len());
    let mut chars = asm.chars().peekable();
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
            chars.next(); // '}'
            // 仅保留序号部分
            let idx = inner.split(':').next().unwrap_or("").trim();
            out.push('{');
            out.push_str(idx);
            out.push('}');
        } else {
            out.push(c);
        }
    }
    out
}

/// lowering 模板操作数 token → 槽类型签名（形状消歧用）。
/// `[{n}]` 方括号内的寄存器是内存基址（槽为 reg），按内部 token 判定。
pub(crate) fn lowering_token_kind(op: &str) -> &'static str {
    // 剥掉外层方括号(内存基址写法 [{n}])
    let inner = if let Some(op) = op.strip_circumfix('[', ']') {
        op
    } else {
        op
    };
    match inner {
        // 符号化寄存器
        "{out}" | "{out2}" | "{t}" | "{0}" | "{1}" | "{2}" => "reg",
        // 编号临时
        _ if inner.starts_with("{t") && inner.ends_with('}') => "reg",
        // 物理寄存器（RAX 等）
        _ if inner.starts_with('{')
            && inner.ends_with('}')
            && inner[1..inner.len() - 1]
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) =>
        {
            "reg"
        }
        // 常量/立即数 token
        "{iconst}" | "{fconst}" | "{off}" | "{alloca}" | "{global}" | "{imm0}" | "{imm0_sub4}"
        | "{iconst_hi20}" | "{iconst_lo12}" | "{iconst_hi32_hi20}" | "{iconst_hi32_lo12}"
        | "{iconst_lo32_hi20}" | "{iconst_lo32_lo12}"
        | "{fconst_hi32_hi20}" | "{fconst_hi32_lo12}"
        | "{fconst_lo32_hi20}" | "{fconst_lo32_lo12}" => "imm",
        _ if inner.starts_with("{shufps") || inner.starts_with("{vconst") => "imm",
        // 条件码
        "{cc}" => "cond",
        // 数字立即数（含 0x 十六进制；兼容 imm/cond 槽——setcc/cmovcc 条件码是数字）
        _ if parse_i64_lit(inner).is_ok() => "num",
        // 真正的内存操作数（MemRef 槽模板 [base+off]）
        _ if inner.contains('+')
            || inner.contains('-')
            || inner.contains('(')
            || inner.contains(')') =>
        {
            "mem"
        }
        // 其他（字面量等）→ 视为 reg（物理寄存器名）
        _ => "reg",
    }
}

fn gen_isa_info(model: &V12Model, infos: &[InstInfo]) -> Result<TokenStream, String> {
    let name_str = &model.meta.name;
    let version_str = model.meta.version.as_deref().unwrap_or("");
    let mode = model.meta.mode;
    let vl = model.meta.variable_length;
    let n_insts = infos.len();
    let max_inst_len = model.meta.max_inst_len.unwrap_or(if vl { 15 } else { 4 });
    let min_inst_len: u8 = if vl { 1 } else { 4 };
    let endian = if model.meta.endian == Endian::Big {
        quote! { forge_ir::Endianness::Big }
    } else {
        quote! { forge_ir::Endianness::Little }
    };
    Ok(quote! {
        pub struct IsaInfo;

        impl crate::machine::isa_info::IsaInfo for IsaInfo {
            fn name(&self) -> &'static str { #name_str }
            fn version(&self) -> &'static str { #version_str }
            fn address_size(&self) -> u8 { #mode }
            fn endianness(&self) -> forge_ir::Endianness { #endian }
            fn capabilities(&self) -> crate::machine::isa_info::IsaCapabilities {
                crate::machine::isa_info::IsaCapabilities {
                    variable_length: #vl,
                    fixed_inst_size: 0,
                    prefix_layers: 1,
                    addressing_modes: &[],
                    simd_widths: &[],
                    mask_registers: false,
                    broadcast: false,
                    rounding_mode: false,
                    endianness: #endian,
                    min_inst_len: #min_inst_len,
                    max_inst_len: #max_inst_len,
                }
            }
            fn num_instructions(&self) -> usize { #n_insts }
        }
    })
}

fn gen_reg_info(model: &V12Model) -> Result<TokenStream, String> {
    // 主 GPR 组（gpr/gpr64）与浮点组（xmm/fpr）数量（names.len() 优先，
    // 缺省 count 字段）。
    let gpr_count = model
        .reg
        .get(&RegClass::GPR(8))
        .or_else(|| model.reg.get(&RegClass::GPR(4)))
        .map(|g| {
            g.names
                .as_ref()
                .map(|n| n.len() as u32)
                .or(g.count.map(|c| c as u32))
                .unwrap_or(16)
        })
        .unwrap_or(16);
    // 主浮点/向量组数量：与默认 FPR 类一致（优先 16 字节 XMM 组——ABI/SSE
    // 占位以 XMM 为基准；ZMM 等 EVEX 组不改变 num_fp_regs，否则 regalloc 会
    // 用 XMM 类分配 16-31 号越界）。riscv F32/F64 同为 32 号组。
    let fpr_count = model
        .reg
        .iter()
        .filter(|(rc, _)| matches!(rc, RegClass::FPR(_)))
        .find(|(rc, _)| **rc == RegClass::FPR(16))
        .or_else(|| {
            model
                .reg
                .iter()
                .filter(|(rc, _)| matches!(rc, RegClass::FPR(_)))
                .max_by_key(|(rc, _)| rc.width())
        })
        .map(|(_, g)| {
            g.names
                .as_ref()
                .map(|n| n.len() as u32)
                .or(g.count.map(|c| c as u32))
                .unwrap_or(0)
        })
        .unwrap_or(0);
    // 主 GPR 组寄存器名（用于 sp/fp 引用解析）。
    let gpr_names: Vec<String> = model
        .reg
        .get(&RegClass::GPR(8))
        .or_else(|| model.reg.get(&RegClass::GPR(4)))
        .map(group_names)
        .transpose()?
        .unwrap_or_default();
    let name_to_idx: std::collections::HashMap<&str, u32> = gpr_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i as u32))
        .collect();
    // SP/FP：优先 [abi.frame].sp/.fp 声明的名字（demo "X7"/"X6" 等自定义
    // 寄存器名），否则按惯例名（"RSP"/"SP"、"RBP"/"FP"）解析；缺失回退
    // 索引（x86 RSP=4/RBP=5；riscv 无显式声明时用 2/8——但组内索引可能
    // 不同，缺省用 0 号避免越界）。
    let frame_sp_name = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .map(|f| f.sp.clone());
    let frame_fp_name = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .and_then(|f| f.fp.clone());
    let sp_idx = frame_sp_name
        .as_ref()
        .and_then(|n| name_to_idx.get(n.as_str()).copied())
        .or_else(|| {
            name_to_idx
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("rsp") || n.eq_ignore_ascii_case("sp"))
                .map(|(_, &i)| i)
        })
        .unwrap_or(0);
    let fp_idx = frame_fp_name
        .as_ref()
        .and_then(|n| name_to_idx.get(n.as_str()).copied())
        .or_else(|| {
            name_to_idx
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("rbp") || n.eq_ignore_ascii_case("fp"))
                .map(|(_, &i)| i)
        })
        .unwrap_or(0);
    // sp/fp 引用：能解析到名字 → Reg::NAME；否则用 from_index（避免不存在的变体）
    let sp_ref = frame_sp_name
        .as_ref()
        .and_then(|n| name_to_idx.get(n.as_str()).map(|&i| (n.clone(), i)))
        .or_else(|| {
            name_to_idx
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("rsp") || n.eq_ignore_ascii_case("sp"))
                .map(|(n, &i)| ((*n).to_string(), i))
        })
        .map(|(n, i)| {
            let ident = format_ident!("{n}");
            (quote! { Reg::#ident }, i)
        })
        .unwrap_or_else(|| {
            (
                quote! { <Reg as forge_ir::PhysReg>::from_index(#sp_idx, forge_ir::RegClass::GPR64) },
                sp_idx,
            )
        });
    let fp_ref = frame_fp_name
        .as_ref()
        .and_then(|n| name_to_idx.get(n.as_str()).map(|&i| (n.clone(), i)))
        .or_else(|| {
            name_to_idx
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("rbp") || n.eq_ignore_ascii_case("fp"))
                .map(|(n, &i)| ((*n).to_string(), i))
        })
        .map(|(n, i)| {
            let ident = format_ident!("{n}");
            (quote! { Reg::#ident }, i)
        })
        .unwrap_or_else(|| {
            (
                quote! { <Reg as forge_ir::PhysReg>::from_index(#fp_idx, forge_ir::RegClass::GPR64) },
                fp_idx,
            )
        });
    let sp_expr = sp_ref.0;
    let fp_expr = fp_ref.0;
    let sp_idx = sp_ref.1;
    let fp_idx = fp_ref.1;
    // callee_saved：从 [abi].callee_saved.gpr 解析物理索引（顺序 = prologue push 序）。
    let callee_saved: Vec<TokenStream> = model
        .abi
        .as_ref()
        .and_then(|a| a.callee_saved.as_ref())
        .map(|cs| {
            cs.gpr
                .iter()
                .filter_map(|n| name_to_idx.get(n.as_str()).copied())
                .map(|i| quote! { #i })
                .collect()
        })
        .unwrap_or_default();
    // allocatable：全量 0..count（排除 SP/FP + spill scratch + [abi].reserved）。
    // reserved：不可分配寄存器（riscv X0=zero 写入无效、X1=ra 被 prologue/
    // call 占用、X3/X4=gp/tp）——不排除会分配出垃圾（实测 subw x0 结果丢失）。
    // spill scratch（[abi].scratch = R10/R11）必须排除——emission 的 spill
    // load/store 用 scratch 寄存器，若 regalloc 把活跃 XReg 分配到 scratch，
    // spill 重写会覆盖其值（t+= 循环崩溃：i 地址在 R11 被 spill load 覆盖）。
    // 6i 曾尝试排除但触发循环 spill 暴露 v12 spill bug；6k/6m/6n 修复后
    // 重新排除（scatch 全时保留给 spill 机制）。
    let scratch_idx: std::collections::HashSet<u32> = model
        .abi
        .as_ref()
        .map(|a| {
            a.scratch
                .iter()
                .filter_map(|n| name_to_idx.get(n.as_str()).copied())
                .collect()
        })
        .unwrap_or_default();
    let reserved_idx: std::collections::HashSet<u32> = model
        .abi
        .as_ref()
        .map(|a| {
            a.reserved
                .iter()
                .filter_map(|n| name_to_idx.get(n.as_str()).copied())
                .collect()
        })
        .unwrap_or_default();
    let gp_alloc: Vec<TokenStream> = (0..gpr_count)
        .filter(|&i| {
            i != sp_idx && i != fp_idx && !scratch_idx.contains(&i) && !reserved_idx.contains(&i)
        })
        .map(|i| quote! { #i })
        .collect();
    let fp_alloc: Vec<TokenStream> = (0..fpr_count).map(|i| quote! { #i }).collect();
    // scratch：从 [abi].scratch 解析物理索引（spill load/store 用）。
    let scratch: Vec<TokenStream> = model
        .abi
        .as_ref()
        .map(|a| {
            a.scratch
                .iter()
                .filter_map(|n| name_to_idx.get(n.as_str()).copied())
                .map(|i| quote! { #i })
                .collect()
        })
        .unwrap_or_default();
    Ok(quote! {
        pub struct RegInfo;

        impl crate::machine::reg_info::TargetRegInfo for RegInfo {
            type Reg = Reg;

            fn num_gp_regs(&self) -> u32 { #gpr_count }
            fn num_fp_regs(&self) -> u32 { #fpr_count }
            fn default_gpr_class(&self) -> forge_ir::RegClass { __DEFAULT_GPR_CLASS }
            fn default_fpr_class(&self) -> forge_ir::RegClass { __DEFAULT_FPR_CLASS }
            fn sp_reg(&self) -> forge_ir::FrameAccess<Self::Reg> {
                forge_ir::FrameAccess::Register(#sp_expr)
            }
            fn fp_reg(&self) -> Option<Self::Reg> {
                Some(#fp_expr)
            }
            fn allocatable_gp_order(&self) -> Vec<u32> {
                vec![#(#gp_alloc),*]
            }
            fn allocatable_fp_order(&self) -> Vec<u32> {
                vec![#(#fp_alloc),*]
            }
            fn scratch_regs(&self) -> Vec<u32> {
                vec![#(#scratch),*]
            }
            fn callee_saved(&self) -> Vec<u32> {
                vec![#(#callee_saved),*]
            }
            fn frame_pointer_overhead(&self) -> u32 { 8 }
        }
    })
}

// ─────────────────────── TargetMachine ───────────────────────

fn gen_target_machine(model: &V12Model) -> Result<TokenStream, String> {
    let isa_name_str = &model.meta.name;
    Ok(quote! {
        #[derive(Clone)]
        pub struct TargetMachine {
            isa_info: std::sync::Arc<dyn crate::machine::isa_info::IsaInfo>,
            reg_info: std::sync::Arc<dyn crate::machine::reg_info::TargetRegInfo<Reg = Reg>>,
            abi: std::sync::Arc<dyn crate::machine::abi::TargetABI<Reg = Reg>>,
            lowering: std::sync::Arc<dyn crate::machine::lowering::TargetLowering<Inst = Inst>>,
            encoder: std::sync::Arc<dyn crate::machine::encoder::TargetEncoder<Inst = Inst>>,
            frame_lowering: std::sync::Arc<dyn crate::machine::frame::TargetFrameLowering<Inst = Inst>>,
            disassembler: std::sync::Arc<dyn crate::machine::disasm::TargetDisassembler<Inst = Inst>>,
            assembler: std::sync::Arc<dyn crate::machine::assembler::TargetAssembler<Inst = Inst>>,
            decoder: std::sync::Arc<dyn crate::machine::decoder::TargetDecoder<Inst = Inst>>,
        }

        impl TargetMachine {
            pub fn new() -> Self {
                Self {
                    isa_info: std::sync::Arc::new(IsaInfo),
                    reg_info: std::sync::Arc::new(RegInfo),
                    abi: std::sync::Arc::new(ABI),
                    lowering: std::sync::Arc::new(Lowering),
                    encoder: std::sync::Arc::new(Encoder),
                    frame_lowering: std::sync::Arc::new(FrameLowering),
                    disassembler: std::sync::Arc::new(Disassembler),
                    assembler: std::sync::Arc::new(Assembler),
                    decoder: std::sync::Arc::new(Decoder),
                }
            }

            pub fn new_arc() -> std::sync::Arc<Self> {
                std::sync::Arc::new(Self::new())
            }
        }

        impl crate::machine::target::TargetMachine for TargetMachine {
            type Inst = Inst;
            type Reg = Reg;

            fn isa_info(&self) -> &std::sync::Arc<dyn crate::machine::isa_info::IsaInfo> { &self.isa_info }
            fn reg_info(&self) -> &std::sync::Arc<dyn crate::machine::reg_info::TargetRegInfo<Reg = Self::Reg>> { &self.reg_info }
            fn abi(&self) -> &std::sync::Arc<dyn crate::machine::abi::TargetABI<Reg = Self::Reg>> { &self.abi }
            fn lowering(&self) -> &std::sync::Arc<dyn crate::machine::lowering::TargetLowering<Inst = Self::Inst>> { &self.lowering }
            fn encoder(&self) -> &std::sync::Arc<dyn crate::machine::encoder::TargetEncoder<Inst = Self::Inst>> { &self.encoder }
            fn frame_lowering(&self) -> &std::sync::Arc<dyn crate::machine::frame::TargetFrameLowering<Inst = Self::Inst>> { &self.frame_lowering }
            fn disassembler(&self) -> Option<&std::sync::Arc<dyn crate::machine::disasm::TargetDisassembler<Inst = Self::Inst>>> { Some(&self.disassembler) }
            fn assembler(&self) -> Option<&std::sync::Arc<dyn crate::machine::assembler::TargetAssembler<Inst = Self::Inst>>> { Some(&self.assembler) }
            fn decoder(&self) -> Option<&std::sync::Arc<dyn crate::machine::decoder::TargetDecoder<Inst = Self::Inst>>> { Some(&self.decoder) }
        }

        crate::impl_erased_target_machine!(TargetMachine);

        /// 注册该 ISA 后端到全局 Registry。
        pub fn ensure_registered() {
            static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
            INIT.get_or_init(|| {
                crate::machine::reloc_patcher::register_default_reloc_patcher(#isa_name_str);
                if !crate::Registry::global().contains(#isa_name_str) {
                    crate::Registry::global().register_backend(TargetMachine::new_arc());
                }
            });
        }
    })
}

/// 解析 lowering/emit 模板中的内存操作数文本 → MemRef 构造表达式。
/// 支持 `{base}+{off}`（base = 物理寄存器名或 `{off}` 偏移符号）。
pub(crate) fn parse_mem_template(text: &str, ctx: &str) -> Result<TokenStream, String> {
    let t = text.trim().trim_start_matches('[').trim_end_matches(']');
    let (base_part, disp_part) = match t.split_once('+') {
        Some((b, d)) => (b.trim().to_string(), Some(d.trim().to_string())),
        None => match t.split_once('-') {
            Some((b, d)) => (b.trim().to_string(), Some(format!("-{d}"))),
            None => (t.trim().to_string(), None),
        },
    };
    let base: TokenStream = if base_part == "{off}" {
        quote! { ctx.current_offset }
    } else {
        let reg = format_ident!("{base_part}");
        quote! { Reg::#reg }
    };
    let disp: TokenStream = match disp_part {
        Some(d) if d == "{off}" => quote! { ctx.current_offset },
        Some(d) if d == "{alloca}" => quote! { ctx.current_alloca_offset as i64 },
        Some(d) => {
            let v: i64 = d
                .parse()
                .map_err(|_| format!("{ctx}: mem disp '{d}' 无法解析"))?;
            quote! { #v }
        }
        None => quote! { 0i64 },
    };
    Ok(quote! { MemRef { base: #base, disp: #disp, index: None, scale: 1 } })
}

/// 从 lowering 模板收集写死的物理寄存器（RAX/RDX 等，非 {占位符}）→
/// clobber 列表（物理索引, RegClass）。regalloc 在本指令点避开。
pub(crate) fn collect_phys_clobbers(
    insts: &[String],
    infos: &[InstInfo],
    model: &V12Model,
) -> Result<Vec<TokenStream>, String> {
    // 主 GPR 组名 → 物理索引
    let gpr_names: Vec<String> = model
        .reg
        .get(&RegClass::GPR(8))
        .or_else(|| model.reg.get(&RegClass::GPR(4)))
        .map(group_names)
        .transpose()?
        .unwrap_or_default();
    let name_to_idx: std::collections::HashMap<&str, u32> = gpr_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i as u32))
        .collect();
    let mut out: Vec<TokenStream> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for t in insts {
        for part in t.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')') {
            let part = part.trim();
            if part.is_empty() || part.starts_with('{') || part.starts_with('@') {
                continue;
            }
            if let Some(&idx) = name_to_idx.get(part)
                && seen.insert(part.to_uppercase())
            {
                out.push(quote! { (#idx, forge_ir::RegClass::GPR64) });
            }
        }
    }
    let _ = infos;
    Ok(out)
}
