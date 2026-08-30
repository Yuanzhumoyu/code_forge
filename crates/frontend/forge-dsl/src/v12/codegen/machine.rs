//! Reg 枚举 / MachineInst / TargetEncoder / Decoder / Disassembler /
//! Assembler 生成（TargetMachine 集成层的一部分）。
//!
//! 从 `integration.rs` 拆分（原 236-1257 行）。复用父模块的 `InstInfo` 与
//! `shared::group_names`、model 类型。

use super::super::model::*;
use super::super::shared::group_names;
use super::InstInfo;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

// ─────────────────────── Reg 枚举（物理寄存器）───────────────────────

/// 从 v12 `[reg.*]` 生成物理寄存器枚举 + PhysReg impl + 默认值类常量。
/// 变体 = 全部寄存器组的全部寄存器名；物理编号 = 组内索引 + base_index
/// （多宽度视图组可 base_index=0 共享同一物理编号——x86 的 EAX/RAX 等）。
/// GPR 组按宽度降序排列（64 位主组在前），FPR 组在后。
pub(crate) fn gen_reg_enum(model: &V12Model) -> Result<TokenStream, String> {
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

// ─────────────────────── MachineInst impl ───────────────────────

pub(crate) fn gen_machine_inst(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
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

pub(crate) fn gen_encoder(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
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

pub(crate) fn gen_decoder() -> TokenStream {
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

pub(crate) fn gen_disasm(_infos: &[InstInfo]) -> Result<TokenStream, String> {
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

pub(crate) fn gen_assembler(model: &V12Model) -> TokenStream {
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

