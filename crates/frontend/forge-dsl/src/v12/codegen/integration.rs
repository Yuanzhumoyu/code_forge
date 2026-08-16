//! v12 codegen — TargetMachine 集成层（迭代 5）。
//!
//! 自包含模块（encode/decode/disassemble/assemble）之上生成 forge-codegen
//! 组件：MachineInst impl、TargetEncoder/Decoder/Disassembler/Assembler、
//! TargetABI、TargetFrameLowering、TargetLowering、TargetMachine 组装。
//!
//! 关键语义（与 v11 一致）：
//! - Inst 寄存器字段为 u32 **物理索引**：lowering 构造时占位 0，regalloc 经
//!   `xreg_map` 分配后 `set_reg_field` 回填物理索引，encode 直接读字段。
//! - `uses()/defs()` 返回寄存器字段值（回填后即物理索引）；regalloc 的活区间
//!   由 pipeline 聚合的 `xreg_map` 驱动（见 pipeline/liverange.rs），
//!   MachineInst::uses/defs 仅作辅助查询。
//! - effect 标签（指令 `effect` 键）驱动 is_branch/is_call/is_ret/effects。

use super::super::model::*;
use super::InstInfo;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
/// 生成集成层组件（Reg 枚举 + MachineInst + Encoder + Decoder + Disasm +
/// Assembler + ABI + FrameLowering + Lowering + TargetMachine）。
pub fn gen_integration(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    let reg_enum = gen_reg_enum(model)?;
    let machine_inst = gen_machine_inst(infos)?;
    let encoder = gen_encoder(infos, model)?;
    let decoder = gen_decoder();
    let disasm = gen_disasm(infos)?;
    let assembler = gen_assembler();
    let abi = gen_abi(model)?;
    let frame = gen_frame_lowering(infos, model)?;
    let lowering = gen_lowering(infos, model)?;
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
/// 变体 = 全部寄存器组的全部寄存器名（GPR 在前，浮点在后）；物理编号 =
/// 组内索引 + base_index（与 v12 编码索引一致——Inst 字段直接存该编号）。
fn gen_reg_enum(model: &V12Model) -> Result<TokenStream, String> {
    // 收集 (组名, 寄存器名列表, base_index, 是否浮点)
    let mut groups: Vec<(String, Vec<String>, u32, bool)> = Vec::new();
    for (gname, g) in &model.reg {
        let names = group_names(g)?;
        let is_fp = gname.contains("xmm") || gname.contains("fpr") || gname.contains("float");
        groups.push((gname.clone(), names, g.base_index.unwrap_or(0), is_fp));
    }
    // GPR 在前（保持组声明序），FPR 在后
    groups.sort_by_key(|(_, _, _, is_fp)| *is_fp);

    let mut variants: Vec<syn::Ident> = Vec::new();
    let mut to_index_arms: Vec<TokenStream> = Vec::new();
    let mut class_arms: Vec<TokenStream> = Vec::new();
    // from_index：GPR 区 → 主 GPR 视图（第一个 GPR 组的第 i 个）；FPR 区 → XMM 视图
    let mut gpr_main: Option<(String, Vec<String>)> = None;
    let mut fpr_main: Option<(String, Vec<String>)> = None;
    for (gname, names, base, is_fp) in &groups {
        if !*is_fp && gpr_main.is_none() {
            gpr_main = Some((gname.clone(), names.clone()));
        }
        if *is_fp && fpr_main.is_none() {
            fpr_main = Some((gname.clone(), names.clone()));
        }
        for (i, n) in names.iter().enumerate() {
            let v = format_ident!("{n}");
            let idx = i as u32 + base;
            variants.push(v.clone());
            to_index_arms.push(quote! { Reg::#v => #idx });
            let cls = if *is_fp {
                quote! { forge_ir::RegClass::FPR64 }
            } else {
                quote! { forge_ir::RegClass::GPR64 }
            };
            class_arms.push(quote! { Reg::#v => #cls });
        }
    }
    // from_index 兜底：按物理编号段（GPR 0..N、FPR N..）——为简单起见按
    // 主组前缀生成（RAX.. / XMM0..）。无法精确映射时回退主组 0 号。
    let gpr_names = gpr_main.map(|(_, n)| n).unwrap_or_default();
    let fpr_names = fpr_main.map(|(_, n)| n).unwrap_or_default();
    let ngpr = gpr_names.len() as u32;
    let gpr_fi: Vec<TokenStream> = gpr_names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let v = format_ident!("{n}");
            let i = i as u32;
            quote! { #i => Reg::#v }
        })
        .collect();
    let fpr_fi: Vec<TokenStream> = fpr_names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let v = format_ident!("{n}");
            let i = ngpr + i as u32;
            quote! { #i => Reg::#v }
        })
        .collect();
    let gpr_fallback = gpr_names.first().map(|n| format_ident!("{n}"));
    let fpr_fallback = fpr_names.first().map(|n| format_ident!("{n}"));
    let from_idx_body = if variants.is_empty() {
        quote! { 0 }
    } else {
        let f1 = gpr_fallback.clone().unwrap_or_else(|| variants[0].clone());
        let f2 = fpr_fallback.clone().unwrap_or_else(|| variants[0].clone());
        quote! {
            match idx {
                #(#gpr_fi,)*
                #(#fpr_fi,)*
                _ if idx < #ngpr => Reg::#f1,
                _ => Reg::#f2,
            }
        }
    };

    // 默认值类：主 GPR 组宽度 / FPR 组宽度
    let gpr_width: u8 = model
        .reg
        .get("gpr64")
        .or_else(|| model.reg.get("gpr"))
        .map(|g| (g.width / 8) as u8)
        .unwrap_or(8);
    let fpr_width: u8 = model
        .reg
        .get("xmm")
        .or_else(|| model.reg.get("fpr"))
        .map(|g| (g.width / 8) as u8)
        .unwrap_or(8);
    let gpr_class = quote! { forge_ir::RegClass::GPR(#gpr_width as u16) };
    let fpr_class = quote! { forge_ir::RegClass::FPR(#fpr_width as u16) };

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
            fn from_index(idx: u32, _class: forge_ir::RegClass) -> Self {
                #from_idx_body
            }
            fn class(self) -> forge_ir::RegClass {
                match self { #(#class_arms,)* }
            }
        }
    })
}

/// 组寄存器名列表（names 或 prefix+count 生成；count-only → 默认 "R" 前缀，
/// 与 codegen/mod.rs 的 group_names 一致）。
fn group_names(g: &RegGroup) -> Result<Vec<String>, String> {
    if let Some(names) = &g.names {
        if names.is_empty() {
            return Err("reg group must have at least one name".into());
        }
        return Ok(names.clone());
    }
    let prefix = g.prefix.clone().unwrap_or_else(|| "R".to_string());
    let count = g
        .count
        .ok_or_else(|| "reg group needs `names` or `count`".to_string())?;
    Ok((0..count as usize)
        .map(|i| format!("{prefix}{i}"))
        .collect())
}

// ─────────────────────── MachineInst impl ───────────────────────

fn gen_machine_inst(infos: &[InstInfo]) -> Result<TokenStream, String> {
    let mut use_arms = Vec::new();
    let mut def_arms = Vec::new();
    let mut use_c_arms = Vec::new();
    let mut def_c_arms = Vec::new();
    let mut reg_field_arms = Vec::new();
    let mut set_reg_field_arms = Vec::new();
    let mut branch_arms = Vec::new();
    let mut call_arms = Vec::new();
    let mut ret_arms = Vec::new();
    let mut move_arms = Vec::new();
    let mut effects_arms = Vec::new();

    for info in infos {
        let vn = &info.vn;

        // Reg 操作数按操作数序收集角色（操作数级 role 优先，缺省 In）。
        // 与 v11 一致：每变体一条 arm，模式列出全部 use/def 字段 + `..`；
        // reg_field 序号 = 该变体 Reg 操作数的位置序（lowering 同序 map_reg_field）。
        let mut use_fids: Vec<&syn::Ident> = Vec::new();
        let mut def_fids: Vec<&syn::Ident> = Vec::new();
        let mut reg_field_entries: Vec<(usize, &syn::Ident)> = Vec::new();
        let mut reg_field_idx = 0usize;
        for (i, (_, fid, slot)) in info.operands.iter().enumerate() {
            if slot.kind != OperandKind::Reg {
                continue;
            }
            let role = info
                .inst
                .operands
                .get(i)
                .and_then(|u| u.role)
                .unwrap_or(OperandRole::In);
            match role {
                OperandRole::In => use_fids.push(fid),
                OperandRole::Out => def_fids.push(fid),
                OperandRole::InOut => {
                    use_fids.push(fid);
                    def_fids.push(fid);
                }
            }
            reg_field_entries.push((reg_field_idx, fid));
            reg_field_idx += 1;
        }

        // uses/defs：列出全部字段的 arm（无字段变体用 `..` 兜底）
        if use_fids.is_empty() {
            use_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
            use_c_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
        } else {
            let vals: Vec<_> = use_fids.iter().map(|f| quote! { *#f }).collect();
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
            let vals: Vec<_> = def_fids.iter().map(|f| quote! { *#f }).collect();
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

        // reg_field/set_reg_field：按 Reg 位置序
        if reg_field_entries.is_empty() {
            // 无 Reg 字段：不 push（match 末尾 _ => 兜底）
        } else {
            let mut rf = Vec::new();
            let mut sf = Vec::new();
            for (idx, fid) in &reg_field_entries {
                rf.push(quote! { (#idx, Inst::#vn { #fid, .. }) => *#fid });
                sf.push(quote! { (#idx, Inst::#vn { #fid, .. }) => { *#fid = idx; } });
            }
            reg_field_arms.push(quote! { #(#rf),* });
            set_reg_field_arms.push(quote! { #(#sf),* });
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
                move_arms.push(quote! { Inst::#vn { #d, #u, .. } => Some((*#d, *#u)) });
            }
        }
    }

    Ok(quote! {
        impl crate::prelude::MachineInst for Inst {
            fn uses(&self) -> smallvec::SmallVec<[u32; 4]> {
                match self { #(#use_arms,)* }
            }
            fn defs(&self) -> smallvec::SmallVec<[u32; 2]> {
                match self { #(#def_arms,)* }
            }
            fn use_constraints(&self) -> smallvec::SmallVec<[crate::machine::inst::OperandConstraint; 4]> {
                match self { #(#use_c_arms,)* }
            }
            fn def_constraints(&self) -> smallvec::SmallVec<[crate::machine::inst::OperandConstraint; 2]> {
                match self { #(#def_c_arms,)* }
            }
            fn effects(&self) -> smallvec::SmallVec<[crate::prelude::EffectKind; 2]> {
                match self { #(#effects_arms,)* }
            }
            fn is_branch(&self) -> bool {
                match self { #(#branch_arms,)* _ => false }
            }
            fn branch_targets(&self) -> smallvec::SmallVec<[crate::prelude::Block; 2]> {
                smallvec::smallvec![]
            }
            fn is_call(&self) -> bool {
                match self { #(#call_arms,)* _ => false }
            }
            fn is_ret(&self) -> bool {
                match self { #(#ret_arms,)* _ => false }
            }
            fn is_move(&self) -> Option<(u32, u32)> {
                match self { #(#move_arms,)* _ => None }
            }
            fn reg_field(&self, i: usize) -> u32 {
                match (i, self) { #(#reg_field_arms,)* _ => 0 }
            }
            fn set_reg_field(&mut self, i: usize, idx: u32) {
                match (i, self) { #(#set_reg_field_arms,)* _ => {} }
            }
        }
    })
}

// ─────────────────────── TargetEncoder ───────────────────────

fn gen_encoder(_infos: &[InstInfo], _model: &V12Model) -> Result<TokenStream, String> {
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
                let bytes = encode(inst).map_err(|e| crate::EncodeError::Other(e))?;
                sink.put_bytes(&bytes);
                Ok(())
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
                decode(bytes).ok_or(crate::machine::decoder::DecodeError::InvalidBytes(0))
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

fn gen_assembler() -> TokenStream {
    quote! {
        pub struct Assembler;

        impl crate::machine::assembler::TargetAssembler for Assembler {
            type Inst = Inst;

            fn parse_insts(&self, source: &str) -> Result<Vec<Self::Inst>, crate::machine::assembler::AsmError> {
                let mut out = Vec::new();
                for line in source.lines() {
                    let t = line.trim();
                    if t.is_empty() || t.starts_with('#') {
                        continue;
                    }
                    out.push(assemble(t).map_err(crate::machine::assembler::AsmError::Other)?);
                }
                Ok(out)
            }
        }
    }
}

// ─────────────────────── TargetABI ───────────────────────

fn gen_abi(model: &V12Model) -> Result<TokenStream, String> {
    // [abi] → arg_regs（按 arg_class 顺序：int 类在前，其余 class 依次）。
    // ret_regs：缺省空（v12 声明层暂不区分返回寄存器——后续迭代扩展）。
    let stack_align = model.abi.as_ref().and_then(|a| a.stack_align).unwrap_or(16);
    let mut arg_regs: Vec<TokenStream> = Vec::new();
    if let Some(abi) = &model.abi {
        for ac in &abi.arg_class {
            for r in &ac.regs {
                let i = format_ident!("{r}");
                arg_regs.push(quote! { Reg::#i });
            }
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
                vec![]
            }
            fn stack_align(&self) -> u32 { #stack_align }
        }
    })
}

// ─────────────────────── TargetFrameLowering ───────────────────────

fn gen_frame_lowering(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    // 指令名 → InstInfo（[emit]/[spill] 模板用大写指令名引用）。
    let prologue_toks = gen_emit_block(infos, model, true)?;
    let epilogue_toks = gen_emit_block(infos, model, false)?;

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

            fn needs_epilogue_label(&self) -> bool { true }

            fn emit_epilogue_jump(
                &self,
                _encoder: &std::sync::Arc<dyn crate::machine::encoder::TargetEncoder<Inst = Self::Inst>>,
                _reg_map: &crate::AllocResult,
                epilogue_block: forge_ir::Block,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::IrError> {
                // JMP rel32（0xE9 + 占位）+ label 记录（链接期回填）。
                sink.put1(0xE9);
                let fixup = sink.offset();
                sink.put4(0u32);
                sink.use_label_at(fixup, epilogue_block, crate::RelocKind::REL4);
                Ok(())
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
                width: u8,
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
                width: u8,
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
            out.push(gen_emit_pseudo(model, name)?);
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
                .map(|(_, fid, _)| fid.clone())
                .ok_or_else(|| format!("emit 模板操作数 {i} 超出指令 {inst_name}（行: {line}）"))?;
            let slot = &info.operands[i].2;
            let expr: TokenStream = match op {
                other if slot.kind == OperandKind::Reg => {
                    // 物理寄存器名 → 物理索引（emit 模式直接写死）
                    let reg = format_ident!("{other}");
                    quote! { Reg::#reg.to_index() }
                }
                other if slot.kind == OperandKind::Imm || slot.kind == OperandKind::Label => {
                    let v: i64 = other
                        .parse()
                        .map_err(|_| format!("emit 模板立即数 '{other}' 无法解析"))?;
                    quote! { #v }
                }
                other if slot.kind == OperandKind::Opsize => {
                    let v: u64 = other
                        .parse()
                        .map_err(|_| format!("emit 模板 opsize '{other}' 无法解析"))?;
                    quote! { #v as u8 }
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
    // 非文本操作数（Opsize）缺省 64
    for (fid, _, slot) in info.operands.iter() {
        if slot.kind == OperandKind::Opsize && !bindings.iter().any(|(f, _)| f == &fid.to_string())
        {
            bindings.push((fid.to_string(), quote! { 64u8 }));
        }
    }
    let fields: Vec<TokenStream> = info
        .operands
        .iter()
        .map(|(fid, _, _)| {
            let fid_ident = format_ident!("{fid}");
            let expr = bindings
                .iter()
                .find(|(f, _)| f == &fid.to_string())
                .map(|(_, e)| e.clone())
                .unwrap_or_else(|| quote! { 0 });
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
fn gen_emit_pseudo(model: &V12Model, name: &str) -> Result<TokenStream, String> {
    match name {
        "push_callee" | "pop_callee" => {
            let Some(abi) = &model.abi else {
                return Ok(quote! {});
            };
            let Some(callee) = &abi.callee_saved else {
                return Ok(quote! {});
            };
            let regs: Vec<String> = if name == "push_callee" {
                callee.gpr.clone()
            } else {
                callee.gpr.iter().rev().cloned().collect()
            };
            let push_pop = if name == "push_callee" { "PUSH" } else { "POP" };
            let vn = crate::codegen::pascal_ident(push_pop);
            let mut stmts = Vec::new();
            for r in &regs {
                let reg = format_ident!("{r}");
                stmts.push(quote! {
                    let __bytes = encode(&Inst::#vn { op0: Reg::#reg.to_index() })
                        .map_err(|e| crate::IrError::Emit(e))?;
                    __sink.put_bytes(&__bytes);
                });
            }
            Ok(quote! { #(#stmts)* })
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
            // 帧分配指令：SUB RSP, frame_size —— sp 是第 0 操作数、imm 是第 1。
            let sp = format_ident!("{}", frame.sp);
            let vn = crate::codegen::pascal_ident(inst);
            Ok(quote! {
                if __frame_size != 0 {
                    let __bytes = encode(&Inst::#vn {
                        op0: Reg::#sp.to_index(),
                        op1: __frame_size as i64,
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
            let vn = crate::codegen::pascal_ident(inst);
            Ok(quote! {
                if __frame_size != 0 {
                    let __bytes = encode(&Inst::#vn {
                        op0: Reg::#sp.to_index(),
                        op1: __frame_size as i64,
                    }).map_err(|e| crate::IrError::Emit(e))?;
                    __sink.put_bytes(&__bytes);
                }
            })
        }
        other => Err(format!("unknown emit pseudo '@{other}'")),
    }
}

/// spill 模板 → 单条 load/store 语句（`{0}` = 寄存器、`{1}` = 偏移、基址 = base）。
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
                .map(|(_, fid, _)| fid.clone())
                .ok_or_else(|| format!("spill 模板操作数 {i} 超出指令 {inst_name}"))?;
            let slot = &info.operands[i].2;
            let expr: TokenStream = match op {
                "{0}" if slot.kind == OperandKind::Reg => quote! { __dst },
                "{1}" if slot.kind == OperandKind::Mem => {
                    quote! { MemRef { base: Reg::#base.to_index(), disp: __off } }
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
        .map(|(fid, _, _)| {
            let fid_ident = format_ident!("{fid}");
            let expr = bindings
                .iter()
                .find(|(f, _)| f == &fid.to_string())
                .map(|(_, e)| e.clone())
                .unwrap_or_else(|| quote! { 0 });
            quote! { #fid_ident: #expr }
        })
        .collect();
    let vn = crate::codegen::pascal_ident(inst_name);
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

// ─────────────────────── TargetLowering ───────────────────────

fn gen_lowering(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    // 指令名 → 变体名（lowering 模板用大写指令名引用）。
    let name_to_vn: std::collections::HashMap<&str, syn::Ident> = infos
        .iter()
        .map(|i| (i.inst.name.as_str(), i.vn.clone()))
        .collect();

    let mut arms: Vec<TokenStream> = Vec::new();
    for rule in &model.lowering {
        let op_ident = format_ident!("{}", rule.op);
        // 谓词接入：when → 生成求值闭包（attrs 由调用上下文提供；未知 false）
        let pred = match &rule.when {
            Some(v) => Some(
                super::super::pred::parse(v)
                    .map_err(|e| format!("[[lowering.{}]] when: {e}", rule.op))?,
            ),
            None => None,
        };
        let _ = pred; // 迭代 5 生成阶段保留解析校验；求值接线在 arm 内

        // 展开 insts 模板 → 指令构造序列（符号化操作数 {out}/{0}/{1}）
        let inst_toks = gen_lowering_insts(&rule.insts, infos, &name_to_vn)?;
        let when_guard: TokenStream = match &rule.when {
            Some(_) => quote! { /* when 谓词：由 pipeline 上下文求值（迭代 5 接线） */ },
            None => quote! {},
        };
        arms.push(quote! {
            crate::prelude::Opcode::#op_ident { .. } => {
                let mut __pack = crate::prelude::InstPacket::new();
                #when_guard
                #(#inst_toks)*
                Ok(__pack)
            }
        });
    }

    // terminator 指令引用（按名查找，缺失 → 该终结符 Unsupported）
    let has_ret = infos.iter().any(|i| i.inst.name == "RET");
    let has_jmp = infos.iter().any(|i| i.inst.name == "JMP_REL32");
    let has_mov_rax = infos.iter().any(|i| i.inst.name == "MOV_RM8_R64");

    // Return：MOV_RM8_R64 val, RAX（reg=src=val、rm=dest=RAX → 0x89 方向）
    // + RET。val 是返回值 XReg（占位 0 + map 到字段 0）。
    let return_body: TokenStream = if has_ret && has_mov_rax {
        quote! {
            let val = values.first().copied()
                .and_then(|x| value_to_xreg.get(&x)).copied()
                .unwrap_or_else(|| ctx.alloc_xreg(__DEFAULT_GPR_CLASS));
            let __idx = __pack.push_inst(Inst::MovRm8R64 {
                op0: 0,
                op1: 0,
                op2: 64,
            });            __pack.map_reg_field(val, __idx, 0u8, false);
            __pack.push_inst(Inst::Ret);
            Ok(__pack)
        }
    } else {
        quote! {
            let _ = __pack;
            Err(crate::prelude::IrError::Unsupported("v12 return lowering (RET/MOV_RM8_R64 missing)".into()))
        }
    };
    // Jump：JMP_REL32 target（块号 → rel 字段）。
    let jump_body: TokenStream = if has_jmp {
        quote! {
            __pack.push_inst(Inst::JmpRel32 { op0: target.0 as i64 });
            Ok(__pack)
        }
    } else {
        quote! {
            let _ = __pack;
            Err(crate::prelude::IrError::Unsupported("v12 jump lowering (JMP_REL32 missing)".into()))
        }
    };

    // 无任何 terminator 指令（riscv 等）→ 直接 Err 版本（避免 __pack 未使用/不可达）
    let term_impl: TokenStream = if has_ret || has_jmp {
        quote! {
            #[allow(unreachable_code)]
            fn lower_terminator(
                &self,
                term: &crate::prelude::Terminator,
                value_to_xreg: &std::collections::HashMap<crate::prelude::Value, crate::prelude::XReg>,
                _block_to_vblock: &std::collections::HashMap<crate::prelude::Block, crate::prelude::VBlockId>,
                ctx: &mut crate::prelude::LowerCtx,
            ) -> Result<crate::prelude::InstPacket<Self::Inst>, crate::prelude::IrError> {
                let mut __pack: crate::prelude::InstPacket<Self::Inst> =
                    crate::prelude::InstPacket::new();
                match term {
                    crate::prelude::Terminator::Return { values, .. } => {
                        #return_body
                    }
                    crate::prelude::Terminator::Jump { target, .. } => {
                        #jump_body
                    }
                    _ => Err(crate::prelude::IrError::Unsupported("v12 terminator lowering".into())),
                }
            }
        }
    } else {
        quote! {
            fn lower_terminator(
                &self,
                _term: &crate::prelude::Terminator,
                _value_to_xreg: &std::collections::HashMap<crate::prelude::Value, crate::prelude::XReg>,
                _block_to_vblock: &std::collections::HashMap<crate::prelude::Block, crate::prelude::VBlockId>,
                _ctx: &mut crate::prelude::LowerCtx,
            ) -> Result<crate::prelude::InstPacket<Self::Inst>, crate::prelude::IrError> {
                Err(crate::prelude::IrError::Unsupported("v12 terminator lowering".into()))
            }
        }
    };

    Ok(quote! {
        pub struct Lowering;

        impl crate::machine::lowering::TargetLowering for Lowering {
            type Inst = Inst;

            fn lower_inst(
                &self,
                op: &crate::prelude::Opcode,
                args: &[crate::prelude::XReg],
                results: &[crate::prelude::XReg],
                ctx: &mut crate::prelude::LowerCtx,
            ) -> Result<crate::prelude::InstPacket<Self::Inst>, crate::prelude::IrError> {
                let rd = results.first().copied().unwrap_or_else(|| ctx.alloc_xreg(__DEFAULT_GPR_CLASS));
                let rs1 = args.first().copied().unwrap_or_else(|| ctx.alloc_xreg(__DEFAULT_GPR_CLASS));
                let rs2 = args.get(1).copied().unwrap_or_else(|| ctx.alloc_xreg(__DEFAULT_GPR_CLASS));
                let rs3 = args.get(2).copied().unwrap_or_else(|| ctx.alloc_xreg(__DEFAULT_GPR_CLASS));
                match op { #(#arms,)* _ => Err(crate::prelude::IrError::Unsupported("v12 lowering".into())) }
            }

            #term_impl
        }
    })
}

/// 展开一条 lowering 模板（符号化操作数）为指令构造语句序列。
fn gen_lowering_insts(
    templates: &[String],
    infos: &[InstInfo],
    name_to_vn: &std::collections::HashMap<&str, syn::Ident>,
) -> Result<Vec<TokenStream>, String> {
    let mut out = Vec::new();
    for t in templates {
        let trimmed = t.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // `{out} = INST op0, op1` 或 `INST op0, op1`
        let (lhs, rhs) = match trimmed.split_once('=') {
            Some((l, r)) => (Some(l.trim()), r.trim()),
            None => (None, trimmed),
        };
        let (inst_name, ops) = match rhs.split_once(char::is_whitespace) {
            Some((n, rest)) => (n.trim(), rest.trim()),
            None => (rhs.trim(), ""),
        };
        let vn = name_to_vn
            .get(inst_name)
            .ok_or_else(|| format!("lowering 模板引用了未知指令 '{inst_name}'（行: {trimmed}）"))?;
        let info = infos
            .iter()
            .find(|i| &i.vn == vn)
            .expect("vn from name_to_vn");
        // 操作数符号 → (ctor 表达式, map_reg_field 的 XReg 表达式)
        // ctor 用占位 0（regalloc 经 xreg_map 分配后 set_reg_field 回填），
        // 与 v11 的 from_index(0) 占位语义一致；XReg 表达式供 map_reg_field。
        let mut bindings: Vec<(String, TokenStream, TokenStream)> = Vec::new();
        if !ops.is_empty() {
            for (i, op) in ops.split(',').map(|s| s.trim()).enumerate() {
                if op.is_empty() {
                    continue;
                }
                let fid = info
                    .operands
                    .get(i)
                    .map(|(_, fid, _)| fid.clone())
                    .ok_or_else(|| {
                        format!(
                            "lowering 模板操作数 {i} 超出指令 {inst_name} 的操作数（行: {trimmed}）"
                        )
                    })?;
                let slot = &info.operands[i].2;
                let (ctor_expr, xreg_expr) = match op {
                    "{out}" => {
                        let _ = lhs;
                        (quote! { 0u32 }, quote! { rd })
                    }
                    "{0}" => (quote! { 0u32 }, quote! { rs1 }),
                    "{1}" => (quote! { 0u32 }, quote! { rs2 }),
                    "{2}" => (quote! { 0u32 }, quote! { rs3 }),
                    other if slot.kind == OperandKind::Reg => {
                        // 物理寄存器名（RAX 等）→ Reg 枚举 → 物理索引（立即写死）
                        let reg = format_ident!("{other}");
                        (quote! { Reg::#reg.to_index() }, quote! { 0u32 })
                    }
                    other if slot.kind == OperandKind::Imm => {
                        let v: i64 = other
                            .parse()
                            .map_err(|_| format!("lowering 模板立即数 '{other}' 无法解析"))?;
                        (quote! { #v }, quote! { 0u32 })
                    }
                    other if slot.kind == OperandKind::Opsize => {
                        let v: u64 = other
                            .parse()
                            .map_err(|_| format!("lowering 模板 opsize '{other}' 无法解析"))?;
                        (quote! { #v as u8 }, quote! { 0u32 })
                    }
                    other => {
                        return Err(format!(
                            "lowering 模板操作数 '{other}' 不支持（指令 {inst_name} 槽 {}）",
                            slot.kind.kind_name()
                        ));
                    }
                };
                bindings.push((fid.to_string(), ctor_expr, xreg_expr));
            }
        }
        // 非文本操作数（Opsize）缺省 64（v11 约定 "所有 lowering 规则默认为 opsize=64"）
        for (fid, _, slot) in info.operands.iter() {
            if slot.kind == OperandKind::Opsize
                && !bindings.iter().any(|(f, _, _)| f == &fid.to_string())
            {
                bindings.push((fid.to_string(), quote! { 64u8 }, quote! { 0u32 }));
            }
        }
        // 构造 Inst 变体（Reg 字段占位 0；物理寄存器/imm 直接写死）
        let fields: Vec<TokenStream> = info
            .operands
            .iter()
            .map(|(fid, _, _)| {
                let fid_ident = format_ident!("{fid}");
                let expr = bindings
                    .iter()
                    .find(|(f, _, _)| f == &fid.to_string())
                    .map(|(_, e, _)| e.clone())
                    .unwrap_or_else(|| quote! { 0 });
                quote! { #fid_ident: #expr }
            })
            .collect();
        let ctor = if info.operands.is_empty() {
            quote! { Inst::#vn }
        } else {
            quote! { Inst::#vn { #(#fields),* } }
        };
        out.push(quote! {
            let __idx = __pack.push_inst(#ctor);
        });
        // map_reg_field：Reg 操作数（跳过 Mem 基址等非 Reg 字段；与 v11 一致，
        // 仅 Reg 槽参与 regalloc 映射）
        let mut reg_field_i = 0usize;
        for (i, (_, fid, slot)) in info.operands.iter().enumerate() {
            if slot.kind != OperandKind::Reg {
                continue;
            }
            let role = info
                .inst
                .operands
                .get(i)
                .and_then(|u| u.role)
                .unwrap_or(OperandRole::In);
            let is_def = matches!(role, OperandRole::Out | OperandRole::InOut);
            let xreg_expr = bindings
                .iter()
                .find(|(f, _, _)| f == &fid.to_string())
                .map(|(_, _, x)| x.clone())
                .unwrap_or_else(|| quote! { 0u32 });
            // 物理寄存器占位（xreg_expr = 0u32 字面量）不参与 map_reg_field——
            // 只有 XReg 符号（rd/rs1/rs2/rs3）映射到 regalloc
            if xreg_expr.to_string() == "0u32" {
                continue;
            }
            out.push(quote! {
                __pack.map_reg_field(#xreg_expr, __idx, #reg_field_i as u8, #is_def);
            });
            reg_field_i += 1;
        }
    }
    Ok(out)
}

// ─────────────────────── IsaInfo / RegInfo ───────────────────────

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
        .get("gpr64")
        .or_else(|| model.reg.get("gpr"))
        .map(|g| {
            g.names
                .as_ref()
                .map(|n| n.len() as u32)
                .or(g.count.map(|c| c as u32))
                .unwrap_or(16)
        })
        .unwrap_or(16);
    let fpr_count = model
        .reg
        .get("xmm")
        .or_else(|| model.reg.get("fpr"))
        .map(|g| {
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
        .get("gpr64")
        .or_else(|| model.reg.get("gpr"))
        .map(group_names)
        .transpose()?
        .unwrap_or_default();
    let name_to_idx: std::collections::HashMap<&str, u32> = gpr_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i as u32))
        .collect();
    // SP/FP：按名称解析（"RSP"/"RBP"/"SP"/"FP" 等）；缺失回退索引
    //（x86 RSP=4/RBP=5；riscv 无显式声明时用 2/8——但组内索引可能不同，
    // 缺省用 0 号避免越界，后续 [abi.frame] 声明完善）。
    let sp_idx = name_to_idx
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("rsp") || n.eq_ignore_ascii_case("sp"))
        .map(|(_, &i)| i)
        .unwrap_or(0);
    let fp_idx = name_to_idx
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("rbp") || n.eq_ignore_ascii_case("fp"))
        .map(|(_, &i)| i)
        .unwrap_or(0);
    // sp/fp 引用：能解析到名字 → Reg::NAME；否则用 from_index（避免不存在的变体）
    let sp_ref = name_to_idx
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("rsp") || n.eq_ignore_ascii_case("sp"))
        .map(|(n, &i)| {
            let ident = format_ident!("{n}");
            (quote! { Reg::#ident }, i)
        })
        .unwrap_or_else(|| {
            (
                quote! { <Reg as forge_ir::PhysReg>::from_index(#sp_idx, forge_ir::RegClass::GPR64) },
                sp_idx,
            )
        });
    let fp_ref = name_to_idx
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("rbp") || n.eq_ignore_ascii_case("fp"))
        .map(|(n, &i)| {
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
    // allocatable：全量 0..count（排除 SP/FP）
    let gp_alloc: Vec<TokenStream> = (0..gpr_count)
        .filter(|&i| i != sp_idx && i != fp_idx)
        .map(|i| quote! { #i })
        .collect();
    let fp_alloc: Vec<TokenStream> = (0..fpr_count).map(|i| quote! { #i }).collect();
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
                vec![]
            }
            fn callee_saved(&self) -> Vec<u32> {
                vec![]
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
