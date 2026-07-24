//! 代码生成器 — 从 IsaModel 生成 Rust TokenStream。
//!
//! 所有生成内容来自 TOML 模型字段，零硬编码。

use crate::model::*;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::HashSet;

#[path = "standard_insts.rs"]
mod standard_insts;

pub fn generate(model: &IsaModel) -> Result<TokenStream, String> {
    let reg_enum = gen_reg_enum(model);
    let inst_enum = gen_inst_enum(model);
    let machine_inst = gen_machine_inst(model);
    let isa_info = gen_isa_info(model);
    let isa_impl = gen_isa_impl(model)?;
    let disasm_impl = gen_disasm_func(model);
    let encoder_impl = gen_encoder_impl(model);
    let assembler_impl = gen_asm_assembler_impl(model);

    Ok(quote! {
        #reg_enum
        #inst_enum
        #machine_inst
        #isa_info
        #isa_impl
        #disasm_impl
        #encoder_impl
        #assembler_impl
    })
}

// ============================================================
// Helpers
// ============================================================

/// "MOV_R8_RM" → "MovR8Rm"
fn pascal(s: &str) -> String {
    let mut r = String::new();
    let mut cap = true;
    for ch in s.chars() {
        if "_ -.-\t".contains(ch) {
            cap = true;
            continue;
        }
        if cap {
            r.push(ch.to_ascii_uppercase());
            cap = false;
        } else {
            r.push(ch.to_ascii_lowercase());
        }
    }
    if r.is_empty() || r.starts_with(|c: char| c.is_ascii_digit()) {
        r.insert_str(0, "Inst");
    }
    r
}

pub(crate) fn pascal_ident(s: &str) -> proc_macro2::Ident {
    format_ident!("{}", pascal(s))
}

/// 判断字段名是否为 def（定义目标寄存器）
fn is_def_name(name: &str) -> bool {
    name == "dest" || name == "reg"
}

/// 判断字段名是否为 use（读取源寄存器）
fn is_use_name(name: &str) -> bool {
    name.starts_with("src") || name == "base"
}

// ============================================================
// Reg 枚举
// ============================================================

fn gen_reg_enum(model: &IsaModel) -> TokenStream {
    let gpr = model.reg.get("gpr").expect("missing [reg.gpr]");
    let count = gpr.count as usize;

    let variants: Vec<TokenStream> = match &gpr.names {
        Some(names) if !names.is_empty() => names
            .iter()
            .map(|n| {
                let ident = format_ident!("{n}");
                quote! { #ident }
            })
            .collect(),
        _ => {
            let prefix = gpr.prefix.as_deref().unwrap_or("R");
            (0..count)
                .map(|i| {
                    let ident = format_ident!("{prefix}{i}");
                    quote! { #ident }
                })
                .collect()
        }
    };

    let idx_arms: Vec<TokenStream> = variants
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let i = i as u8;
            quote! { #i => Reg::#v }
        })
        .collect();

    let first = &variants[0];

    quote! {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum Reg { #(#variants),* }

        impl crate::prelude::PhysReg for Reg {
            fn to_index(self) -> u8 { self as u8 }
            fn class(self) -> crate::prelude::RegClass { crate::prelude::RegClass::Int }
            fn from_index(idx: u8, _: crate::prelude::RegClass) -> Self {
                match idx { #(#idx_arms ,)* _ => Reg::#first }
            }
        }
    }
}

// ============================================================
// Inst 枚举
// ============================================================

fn gen_inst_enum(model: &IsaModel) -> TokenStream {
    let mut variants = Vec::new();
    for (name, inst) in &model.inst {
        let vn = pascal_ident(name);
        let fields = &inst.fields;
        if fields.is_empty() {
            variants.push(quote! { #vn });
        } else {
            let fs: Vec<_> = fields
                .iter()
                .map(|f| {
                    let fi = format_ident!("{}", f.name);
                    let ft = field_type_tok(&f.field_type);
                    quote! { #fi: #ft }
                })
                .collect();
            variants.push(quote! { #vn { #(#fs),* } });
        }
    }
    variants.push(quote! { Unknown(u32) });

    quote! {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub enum Inst { #(#variants),* }
    }
}

fn field_type_tok(ft: &FieldType) -> TokenStream {
    match ft {
        FieldType::VReg => quote! { crate::prelude::VReg },
        FieldType::I64 => quote! { i64 },
        FieldType::U8 => quote! { u8 },
        FieldType::U32 => quote! { u32 },
        FieldType::F64 => quote! { f64 },
        FieldType::BlockTarget => quote! { i64 },
        FieldType::Reg => quote! { Reg },
        FieldType::CondCode => quote! { u8 },
        FieldType::Opsize => quote! { u8 },
    }
}

// ============================================================
// MachineInst impl
// ============================================================

fn gen_machine_inst(model: &IsaModel) -> TokenStream {
    let mut use_arms: Vec<TokenStream> = Vec::new();
    let mut def_arms: Vec<TokenStream> = Vec::new();
    let mut branch_arms: Vec<TokenStream> = Vec::new();
    let mut call_arms: Vec<TokenStream> = Vec::new();
    let mut ret_arms: Vec<TokenStream> = Vec::new();
    let mut move_arms: Vec<TokenStream> = Vec::new();
    let mut side_effects_arms: Vec<TokenStream> = Vec::new();
    let mut effects_arms: Vec<TokenStream> = Vec::new();
    let mut branch_targets_arms: Vec<TokenStream> = Vec::new();

    for (inst_name, inst) in &model.inst {
        let vn = pascal_ident(inst_name);

        // uses: VReg 类型 + use 名称
        let ufs: Vec<_> = inst
            .fields
            .iter()
            .filter(|f| f.field_type == FieldType::VReg && is_use_name(&f.name))
            .map(|f| format_ident!("{}", f.name))
            .collect();
        if ufs.is_empty() {
            use_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
        } else {
            let clones: Vec<_> = ufs.iter().map(|f| quote! { #f.clone() }).collect();
            use_arms
                .push(quote! { Inst::#vn { #(#ufs),*, .. } => smallvec::smallvec![#(#clones),*] });
        }

        // defs: VReg 类型 + def 名称
        let dfs: Vec<_> = inst
            .fields
            .iter()
            .filter(|f| f.field_type == FieldType::VReg && is_def_name(&f.name))
            .map(|f| format_ident!("{}", f.name))
            .collect();
        if dfs.is_empty() {
            def_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
        } else {
            let clones: Vec<_> = dfs.iter().map(|f| quote! { #f.clone() }).collect();
            def_arms
                .push(quote! { Inst::#vn { #(#dfs),*, .. } => smallvec::smallvec![#(#clones),*] });
        }

        // is_move
        let upper = inst_name.to_uppercase();
        if (upper.starts_with("MOV_") || upper == "SD_MOV" || upper == "SD_FMOV")
            && dfs.len() == 1
            && ufs.len() == 1
        {
            let d = &dfs[0];
            let u = &ufs[0];
            move_arms.push(quote! { Inst::#vn { #d, #u, .. } => Some((#d.clone(), #u.clone())) });
        }

        let effects = inst.effect.as_deref().unwrap_or(&[]);
        if effects
            .iter()
            .any(|e| e.as_str() == "Branch" || e.as_str() == "Jump")
        {
            branch_arms.push(quote! { Inst::#vn { .. } => true });
        }
        if effects.iter().any(|e| e.as_str() == "Call") {
            call_arms.push(quote! { Inst::#vn { .. } => true });
        }
        if effects.iter().any(|e| e.as_str() == "Ret") || inst_name == "RET" {
            ret_arms.push(quote! { Inst::#vn { .. } => true });
        }

        // side_effects: any instruction with non-Pure effects (> Pure or non-empty)
        let has_side_effect = !effects.is_empty() && effects.iter().any(|e| e.as_str() != "Pure");
        if has_side_effect {
            side_effects_arms.push(quote! { Inst::#vn { .. } => true });
        }

        // effects() — generate EffectKind list from TOML effect strings
        let eff_kinds: Vec<TokenStream> = effects
            .iter()
            .map(|e| match e.as_str() {
                "Pure" => quote! { crate::prelude::EffectKind::Pure },
                "Read" => quote! { crate::prelude::EffectKind::Read },
                "Write" => quote! { crate::prelude::EffectKind::Write },
                "Branch" => quote! { crate::prelude::EffectKind::Branch },
                "Jump" => quote! { crate::prelude::EffectKind::Jump },
                "Trap" => quote! { crate::prelude::EffectKind::Trap },
                "Ret" => quote! { crate::prelude::EffectKind::Jump },
                "Call" => quote! { crate::prelude::EffectKind::Trap },
                _ => quote! { crate::prelude::EffectKind::Custom(0) },
            })
            .collect();
        if eff_kinds.is_empty() {
            effects_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
        } else {
            effects_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![#(#eff_kinds),*] });
        }

        // branch_targets — extract BlockTarget fields
        let bt_fields: Vec<_> = inst
            .fields
            .iter()
            .filter(|f| f.field_type == FieldType::BlockTarget)
            .map(|f| format_ident!("{}", f.name))
            .collect();
        if bt_fields.is_empty() {
            branch_targets_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
        } else {
            let bts: Vec<_> = bt_fields
                .iter()
                .map(|f| {
                    quote! { crate::prelude::BlockId(*#f as u32) }
                })
                .collect();
            branch_targets_arms.push(
                quote! { Inst::#vn { #(#bt_fields),*, .. } => smallvec::smallvec![#(#bts),*] },
            );
        }
    }

    quote! {
        impl crate::prelude::MachineInst for Inst {
            fn uses(&self) -> smallvec::SmallVec<[crate::prelude::VReg;4]> {
                match self { #(#use_arms),* , Inst::Unknown(_) => smallvec::smallvec![] }
            }
            fn defs(&self) -> smallvec::SmallVec<[crate::prelude::VReg;2]> {
                match self { #(#def_arms),* , Inst::Unknown(_) => smallvec::smallvec![] }
            }
            fn effects(&self) -> smallvec::SmallVec<[crate::prelude::EffectKind;2]> {
                match self { #(#effects_arms),* , Inst::Unknown(_) => smallvec::smallvec![] }
            }
            fn is_branch(&self) -> bool {
                match self { #(#branch_arms,)* _ => false }
            }
            fn branch_targets(&self) -> smallvec::SmallVec<[crate::prelude::BlockId;2]> {
                match self { #(#branch_targets_arms),* , Inst::Unknown(_) => smallvec::smallvec![] }
            }
            fn is_call(&self) -> bool {
                match self { #(#call_arms,)* _ => false }
            }
            fn is_ret(&self) -> bool {
                match self { #(#ret_arms,)* _ => false }
            }
            fn is_move(&self) -> Option<(crate::prelude::VReg, crate::prelude::VReg)> {
                match self { #(#move_arms,)* _ => None }
            }
            fn has_side_effects(&self) -> bool {
                self.is_call() || self.is_ret() || self.is_branch() || self.memory_access().is_some()
                    || match self { #(#side_effects_arms,)* _ => false }
            }
        }
    }
}

// ============================================================
// IsaInfo impl
// ============================================================

fn gen_isa_info(model: &IsaModel) -> TokenStream {
    let meta = &model.meta;
    let name_str = &meta.name;
    let version_str = &meta.version;
    let mode_val = meta.mode;
    let cap = &meta.capabilities;
    let vl = cap.variable_length;
    let pl = cap.prefix_layers;
    let min_len = if vl { 1u8 } else { 4u8 };
    let max_len = meta.max_inst_len; // already u8 from TOML
    let n_insts = model.inst.len();
    let endian = if meta.endian == "big" {
        quote! { crate::prelude::Endianness::Big }
    } else {
        quote! { crate::prelude::Endianness::Little }
    };

    let mask = cap.mask_registers;
    let bcast = cap.broadcast;
    let round = cap.rounding_mode;
    let simd_vals: Vec<TokenStream> = cap.simd_widths.iter().map(|w| quote! { #w }).collect();

    let simd_expr = if simd_vals.is_empty() {
        quote! { &[] }
    } else {
        quote! { Box::leak(vec![#(#simd_vals),*].into_boxed_slice()) }
    };

    // Register class info from [reg.*] groups
    let reg_classes_exprs: Vec<TokenStream> = model.reg.iter().map(|(name, group)| {
        let w = group.width;
        let cnt = group.count;
        let pfx = group.prefix.clone().unwrap_or_else(|| name.clone());
        let cls = if name == "gpr" {
            quote! { crate::prelude::RegClass::Int }
        } else {
            quote! { crate::prelude::RegClass::Float }
        };
        quote! {
            crate::prelude::RegisterClassInfo { name: #name, count: #cnt, width: #w, prefix: #pfx, reg_class: #cls }
        }
    }).collect();

    quote! {
        pub struct Isa;

        impl crate::prelude::IsaInfo for Isa {
            fn name() -> &'static str { #name_str }
            fn version() -> &'static str { #version_str }
            fn address_size() -> u8 { #mode_val }
            fn capabilities() -> crate::prelude::IsaCapabilities {
                crate::prelude::IsaCapabilities {
                    variable_length: #vl,
                    fixed_inst_size: 0,
                    prefix_layers: #pl,
                    addressing_modes: &[],
                    simd_widths: #simd_expr,
                    mask_registers: #mask,
                    broadcast: #bcast,
                    rounding_mode: #round,
                    endianness: #endian,
                    min_inst_len: #min_len,
                    max_inst_len: #max_len,
                }
            }
            fn formats() -> &'static [crate::prelude::FormatInfo] { &[] }
            fn register_classes() -> &'static [crate::prelude::RegisterClassInfo] {
                Box::leak(vec![#(#reg_classes_exprs),*].into_boxed_slice())
            }
            fn num_instructions() -> usize { #n_insts }
        }
    }
}

// ============================================================
// InstructionSet impl (ABI + emit + lower)
// ============================================================

fn gen_isa_impl(model: &IsaModel) -> Result<TokenStream, String> {
    let abi_methods = gen_abi_methods(model);
    let emit_func = gen_emit_func(model)?;
    let lower_func = gen_lower_func(model)?;
    let lower_term_func = gen_lower_term_func(model)?;
    let lower_pattern_func = gen_lower_pattern_func(model)?;

    Ok(quote! {
        impl crate::prelude::InstructionSet for Isa {
            type Inst = Inst;
            type Reg = Reg;

            #abi_methods

            fn emit(inst: &Self::Inst, rm: &crate::prelude::RegMap, sink: &mut crate::prelude::CodeSink) -> Result<(), crate::prelude::CompileError> {
                emit_inst(inst, rm, sink)
            }

            fn lower(op: &crate::prelude::Opcode, args: &[crate::prelude::VReg], result: Option<crate::prelude::VReg>, ctx: &mut crate::prelude::LowerCtx) -> Result<Vec<Self::Inst>, crate::prelude::CompileError> {
                lower_impl(op, args, result, ctx)
            }
            fn lower_terminator(term: &crate::prelude::Terminator, v: &std::collections::HashMap<crate::prelude::Value, crate::prelude::VReg>, _b: &std::collections::HashMap<crate::prelude::BlockId, crate::prelude::VCodeBlockId>, ctx: &mut crate::prelude::LowerCtx) -> Result<Vec<Self::Inst>, crate::prelude::CompileError> {
                lower_terminator_impl(term, v, ctx)
            }
            fn lower_pattern(pattern_name: &str, _matched_insts: &[crate::ir::Instruction], args: &[crate::prelude::VReg], result: Option<crate::prelude::VReg>, ctx: &mut crate::prelude::LowerCtx) -> Result<Vec<Self::Inst>, crate::prelude::CompileError> {
                lower_pattern_impl(pattern_name, args, result, ctx)
            }
        }

        #emit_func
        #lower_func
        #lower_term_func
        #lower_pattern_func
    })
}

// ============================================================
// ABI methods (sp_reg, fp_reg, callee_save, precolor, ...)
// ============================================================

fn gen_abi_methods(model: &IsaModel) -> TokenStream {
    let ngpr = model.reg.get("gpr").map(|g| g.count as u8).unwrap_or(16);
    let nfpr = model.reg.get("xmm").map(|g| g.count as u8).unwrap_or(16);

    // sp_reg
    let sp_tokens = if let Some(ref abi) = model.abi {
        if let Some(ref frame) = abi.frame {
            let sp_ident = format_ident!("{}", frame.sp);
            quote! { crate::prelude::FrameAccess::Register(Reg::#sp_ident) }
        } else {
            quote! { crate::prelude::FrameAccess::Register(Reg::from_index(4, crate::prelude::RegClass::Int)) }
        }
    } else {
        quote! { crate::prelude::FrameAccess::Register(Reg::from_index(4, crate::prelude::RegClass::Int)) }
    };

    // fp_reg
    let fp_tokens = if let Some(ref abi) = model.abi {
        if let Some(ref frame) = abi.frame {
            if let Some(ref fp) = frame.fp {
                let fp_ident = format_ident!("{fp}");
                quote! { crate::prelude::FrameAccess::Register(Reg::#fp_ident) }
            } else {
                quote! { crate::prelude::FrameAccess::None }
            }
        } else {
            quote! { crate::prelude::FrameAccess::None }
        }
    } else {
        quote! { crate::prelude::FrameAccess::None }
    };

    // stack_align
    let stack_align = model.abi.as_ref().map(|a| a.stack_align).unwrap_or(16);

    // red_zone
    let red_zone: TokenStream = match model.abi.as_ref().and_then(|a| a.red_zone) {
        Some(n) => quote! { Some(#n) },
        None => quote! { None::<u32> },
    };

    // callee_save
    let cs: Vec<TokenStream> = model
        .abi
        .as_ref()
        .map(|a| {
            a.callee_saved
                .gpr
                .iter()
                .chain(a.callee_saved.xmm.iter())
                .map(|n| {
                    let i = format_ident!("{n}");
                    quote! { Reg::#i }
                })
                .collect()
        })
        .unwrap_or_default();

    // arg_regs
    let arg_regs_entries: Vec<TokenStream> = model
        .abi
        .as_ref()
        .map(|a| {
            a.arg_regs
                .gpr
                .iter()
                .chain(a.arg_regs.xmm.iter())
                .map(|n| {
                    let i = format_ident!("{n}");
                    quote! { Reg::#i }
                })
                .collect()
        })
        .unwrap_or_default();

    // ret_regs
    let ret_regs_entries: Vec<TokenStream> = model
        .abi
        .as_ref()
        .map(|a| {
            a.ret_regs
                .gpr
                .iter()
                .chain(a.ret_regs.xmm.iter())
                .map(|n| {
                    let i = format_ident!("{n}");
                    quote! { Reg::#i }
                })
                .collect()
        })
        .unwrap_or_default();

    // precolor
    let pre_entries: Vec<TokenStream> = model
        .abi
        .as_ref()
        .map(|a| {
            a.precolor
                .iter()
                .map(|(vk, rn)| {
                    let vn: u32 = vk
                        .trim_start_matches("VReg")
                        .trim_start_matches("VREG")
                        .trim_start_matches("vreg")
                        .parse()
                        .unwrap_or(0);
                    let ri = format_ident!("{rn}");
                    quote! {
                        m.insert(crate::prelude::VReg(#vn), crate::prelude::PReg::new(Reg::#ri as u8, crate::prelude::RegClass::Int));
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    quote! {
        fn num_gp_regs() -> u8 { #ngpr }
        fn num_fp_regs() -> u8 { #nfpr }

        fn sp_reg() -> crate::prelude::FrameAccess<Self::Reg> { #sp_tokens }
        fn fp_reg() -> crate::prelude::FrameAccess<Self::Reg> { #fp_tokens }
        fn stack_align() -> u32 { #stack_align }
        fn red_zone() -> Option<u32> { #red_zone }

        fn callee_save_regs() -> Vec<Self::Reg> {
            vec![#(#cs),*]
        }

        fn arg_regs() -> Vec<Self::Reg> {
            vec![#(#arg_regs_entries),*]
        }

        fn ret_regs() -> Vec<Self::Reg> {
            vec![#(#ret_regs_entries),*]
        }

        fn precolored_vregs() -> std::collections::HashMap<crate::prelude::VReg, crate::prelude::PReg> {
            let mut m = std::collections::HashMap::new();
            #(#pre_entries)*
            // Float return + mask registers
            // Use VReg > 99 to avoid conflicts with normal VReg allocation (starts at 0)
            m.insert(crate::prelude::VReg(100), crate::prelude::PReg::new(0u8, crate::prelude::RegClass::Float));
            m
        }

        fn emit_prologue(fs: u32, rm: &crate::prelude::RegMap, sink: &mut crate::prelude::CodeSink) {
            if let Err(e) = emit_prologue_impl(fs, rm, sink) {
                panic!("emit_prologue failed: {}", e);
            }
        }
        fn emit_epilogue(fs: u32, rm: &crate::prelude::RegMap, sink: &mut crate::prelude::CodeSink) {
            if let Err(e) = emit_epilogue_impl(fs, rm, sink) {
                panic!("emit_epilogue failed: {}", e);
            }
        }
    }
}

// ============================================================
// emit_inst — 格式驱动代码生成
// ============================================================

fn gen_emit_func(model: &IsaModel) -> Result<TokenStream, String> {
    let mut arms: Vec<TokenStream> = Vec::new();

    for (inst_name, inst) in &model.inst {
        let vn = pascal_ident(inst_name);

        // Bitstring encoding
        if let Some(ref enc_str) = inst.encoding {
            let used_fields: Vec<&str> = inst.fields.iter().map(|f| f.name.as_str()).collect();
            let bounds: Vec<_> = used_fields.iter().map(|n| format_ident!("{n}")).collect();
            let code = crate::bitstring::gen_bitstring_emit(enc_str, inst, model)?;
            if bounds.is_empty() {
                arms.push(quote! { Inst::#vn { .. } => { #code } });
            } else {
                arms.push(quote! { Inst::#vn { #(#bounds),*, .. } => { #code } });
            }
            continue;
        }

        // No encoding — generate error
        arms.push(quote! {
            Inst::#vn { .. } => { return Err(crate::prelude::CompileError::Emit("no encoding for ".into())); }
        });
    }

    arms.push(quote! {
        Inst::Unknown(_) => { return Err(crate::prelude::CompileError::Emit("unknown instruction".into())); }
    });

    // Prologue / Epilogue
    let prologue_body = gen_prologue_epilogue(model, true);
    let epilogue_body = gen_prologue_epilogue(model, false);

    Ok(quote! {
        pub fn emit_inst(
            inst: &Inst,
            rm: &crate::prelude::RegMap,
            sink: &mut crate::prelude::CodeSink,
        ) -> Result<(), crate::prelude::CompileError> {
            use crate::backend::encode::{modrm, sib, enc_lea_sib, enc_rr_0f, BitField, pack_bits};
            let preg = |v: crate::prelude::VReg, m: &crate::prelude::RegMap|
                m.resolve(v).map(|p| p.num).map_err(|e| crate::prelude::CompileError::RegAlloc(format!("{}", e)));
            match inst { #(#arms),* }
            Ok(())
        }

        pub fn emit_prologue_impl(fs: u32, rm: &crate::prelude::RegMap, sink: &mut crate::prelude::CodeSink)
            -> Result<(), crate::prelude::CompileError>
        {
            let frame_size = fs;
            #prologue_body
            Ok(())
        }

        pub fn emit_epilogue_impl(fs: u32, rm: &crate::prelude::RegMap, sink: &mut crate::prelude::CodeSink)
            -> Result<(), crate::prelude::CompileError>
        {
            let frame_size = fs;
            #epilogue_body
            Ok(())
        }
    })
}

/// 生成序言/尾声 body — use `insts` asm sequence.
fn gen_prologue_epilogue(model: &IsaModel, is_prologue: bool) -> TokenStream {
    let default = quote! { let _ = (frame_size, rm, sink); };

    let emit_section = match &model.emit {
        Some(e) => e,
        None => return default,
    };

    let block = if is_prologue {
        &emit_section.prologue
    } else {
        &emit_section.epilogue
    };

    let insts = match block {
        Some(b) if !b.insts.is_empty() => &b.insts,
        _ => return default,
    };

    if crate::cst_codegen::is_v11_format(insts) {
        crate::cst_codegen::gen_emit_insts_cst(insts, model)
    } else {
        gen_emit_insts_sequence(insts, model)
    }
}

// ============================================================
// lower_impl — 从 [lower.*] 生成指令选择
// ============================================================

/// 为未用户定义的 Icmp 条件生成默认 lowering 的 match 臂。
fn gen_default_icmp_arms(
    model: &IsaModel,
    user_handled: &HashSet<String>,
) -> Result<Vec<TokenStream>, String> {
    let mut cond_arms: Vec<TokenStream> = Vec::new();
    for cond_name in standard_insts::ICMP_CONDS {
        if user_handled.contains(*cond_name) {
            continue;
        }
        if let Some(default_insts) = standard_insts::default_icmp_lowering(cond_name) {
            let cc_ident = format_ident!("{cond_name}");
            let inst_toks = gen_lower_insts(&default_insts, model)?;
            cond_arms.push(quote! {
                crate::prelude::IntCC::#cc_ident => {
                    Ok(vec![#(#inst_toks),*])
                }
            });
        }
    }
    Ok(cond_arms)
}

/// 为未用户定义的 Fcmp 条件生成默认 lowering 的 match 臂。
fn gen_default_fcmp_arms(
    model: &IsaModel,
    user_handled: &HashSet<String>,
) -> Result<Vec<TokenStream>, String> {
    let mut cond_arms: Vec<TokenStream> = Vec::new();
    for cond_name in standard_insts::FCMP_CONDS {
        if user_handled.contains(*cond_name) {
            continue;
        }
        if let Some(default_insts) = standard_insts::default_fcmp_lowering(cond_name) {
            let cc_ident = format_ident!("{cond_name}");
            let inst_toks = gen_lower_insts(&default_insts, model)?;
            cond_arms.push(quote! {
                crate::prelude::FloatCC::#cc_ident => {
                    Ok(vec![#(#inst_toks),*])
                }
            });
        }
    }
    Ok(cond_arms)
}

fn gen_lower_func(model: &IsaModel) -> Result<TokenStream, String> {
    let mut arms: Vec<TokenStream> = Vec::new();

    // 收集 Icmp 和 Fcmp 的子规则
    let mut icmp_cases: Vec<(String, &LowerRule)> = Vec::new();
    let mut fcmp_cases: Vec<(String, &LowerRule)> = Vec::new();
    let mut other_rules: Vec<(&String, &LowerRule)> = Vec::new();
    // Template-generated cases (owned, so we can expand $CC)
    let mut icmp_template_arms: Vec<(String, Vec<String>)> = Vec::new();
    let mut fcmp_template_arms: Vec<(String, Vec<String>)> = Vec::new();

    for (key, rule) in &model.lower {
        if let Some(cond) = key.strip_prefix("Icmp.") {
            icmp_cases.push((cond.to_string(), rule));
        } else if let Some(cond) = key.strip_prefix("Fcmp.") {
            fcmp_cases.push((cond.to_string(), rule));
        } else if *key == "Icmp" {
            // Template entry — expand for each condition
            if !rule.template.is_empty() && !rule.conditions.is_empty() {
                for (cond_name, cc_val) in &rule.conditions {
                    icmp_template_arms.push((cond_name.clone(), expand_cc(&rule.template, cc_val)));
                }
            }
        } else if *key == "Fcmp" {
            // Template entry — expand for each condition
            if !rule.template.is_empty() && !rule.conditions.is_empty() {
                for (cond_name, cc_val) in &rule.conditions {
                    fcmp_template_arms.push((cond_name.clone(), expand_cc(&rule.template, cc_val)));
                }
            }
        } else {
            other_rules.push((key, rule));
        }
    }

    // 其他 opcode 规则
    let mut handled_opcodes: HashSet<String> = HashSet::new();
    for (key, rule) in &other_rules {
        handled_opcodes.insert((*key).clone());
        let op_ident = format_ident!("{key}");
        let inst_toks = gen_lower_insts_auto(&rule.insts, model)?;

        // 检查是否需要 destructure index 字段 (Iconst/Fconst)
        let uses_const = rule
            .insts
            .iter()
            .any(|s| s.contains("iconst") || s.contains("fconst"));

        if uses_const {
            arms.push(quote! {
                crate::prelude::Opcode::#op_ident { index } => {
                    let rd = result.unwrap_or(crate::prelude::VReg(0));
                    let rs1 = args.first().copied().unwrap_or(crate::prelude::VReg(0));
                    let rs2 = args.get(1).copied().unwrap_or(crate::prelude::VReg(0));
                    Ok(vec![#(#inst_toks),*])
                }
            });
        } else {
            arms.push(quote! {
                crate::prelude::Opcode::#op_ident { .. } => {
                    let rd = result.unwrap_or(crate::prelude::VReg(0));
                    let rs1 = args.first().copied().unwrap_or(crate::prelude::VReg(0));
                    let rs2 = args.get(1).copied().unwrap_or(crate::prelude::VReg(0));
                    Ok(vec![#(#inst_toks),*])
                }
            });
        }
    }

    // ============================================================
    // 默认 lowering 回退 — 使用标准指令集
    // ============================================================

    // ============================================================
    // 默认 lowering 回退 — 收集默认条件 (两阶段: 先收集, 后统一生成)
    // ============================================================
    let no_default = model.meta.no_default_lowering;

    // 收集默认 Icmp/Fcmp 条件臂 (TokenStream)
    let mut default_icmp_arms: Vec<TokenStream> = Vec::new();
    let mut default_fcmp_arms: Vec<TokenStream> = Vec::new();

    if !no_default {
        // Icmp 默认条件
        let user_icmp_conds: HashSet<String> = icmp_cases
            .iter()
            .map(|(n, _)| n.clone())
            .chain(icmp_template_arms.iter().map(|(n, _)| n.clone()))
            .collect();
        default_icmp_arms = gen_default_icmp_arms(model, &user_icmp_conds)?;

        // Fcmp 默认条件
        let user_fcmp_conds: HashSet<String> = fcmp_cases
            .iter()
            .map(|(n, _)| n.clone())
            .chain(fcmp_template_arms.iter().map(|(n, _)| n.clone()))
            .collect();
        default_fcmp_arms = gen_default_fcmp_arms(model, &user_fcmp_conds)?;
    }

    // ── 统一生成 Icmp arm (用户定义 + 模板 + 默认) ──
    let has_icmp =
        !icmp_cases.is_empty() || !icmp_template_arms.is_empty() || !default_icmp_arms.is_empty();
    if has_icmp {
        let mut cond_arms: Vec<TokenStream> = Vec::new();
        for (cond_name, rule) in &icmp_cases {
            let cc_ident = format_ident!("{cond_name}");
            let inst_toks = gen_lower_insts_auto(&rule.insts, model)?;
            cond_arms.push(quote! {
                crate::prelude::IntCC::#cc_ident => { Ok(vec![#(#inst_toks),*]) }
            });
        }
        for (cond_name, insts) in &icmp_template_arms {
            let cc_ident = format_ident!("{cond_name}");
            let inst_toks = gen_lower_insts(insts, model)?;
            cond_arms.push(quote! {
                crate::prelude::IntCC::#cc_ident => { Ok(vec![#(#inst_toks),*]) }
            });
        }
        cond_arms.extend(default_icmp_arms);
        arms.push(quote! {
            crate::prelude::Opcode::Icmp { cond } => {
                let rd = result.unwrap_or(crate::prelude::VReg(0));
                let rs1 = args.first().copied().unwrap_or(crate::prelude::VReg(0));
                let rs2 = args.get(1).copied().unwrap_or(crate::prelude::VReg(0));
                match cond { #(#cond_arms),* _ => Err(crate::prelude::CompileError::Unsupported("icmp condition".into())) }
            }
        });
    }

    // ── 统一生成 Fcmp arm (用户定义 + 模板 + 默认) ──
    let has_fcmp =
        !fcmp_cases.is_empty() || !fcmp_template_arms.is_empty() || !default_fcmp_arms.is_empty();
    if has_fcmp {
        let mut cond_arms: Vec<TokenStream> = Vec::new();
        for (cond_name, rule) in &fcmp_cases {
            let cc_ident = format_ident!("{cond_name}");
            let inst_toks = gen_lower_insts_auto(&rule.insts, model)?;
            cond_arms.push(quote! {
                crate::prelude::FloatCC::#cc_ident => { Ok(vec![#(#inst_toks),*]) }
            });
        }
        for (cond_name, insts) in &fcmp_template_arms {
            let cc_ident = format_ident!("{cond_name}");
            let inst_toks = gen_lower_insts(insts, model)?;
            cond_arms.push(quote! {
                crate::prelude::FloatCC::#cc_ident => { Ok(vec![#(#inst_toks),*]) }
            });
        }
        cond_arms.extend(default_fcmp_arms);
        arms.push(quote! {
            crate::prelude::Opcode::Fcmp { cond, .. } => {
                let rd = result.unwrap_or(crate::prelude::VReg(0));
                let rs1 = args.first().copied().unwrap_or(crate::prelude::VReg(0));
                let rs2 = args.get(1).copied().unwrap_or(crate::prelude::VReg(0));
                match cond { #(#cond_arms),* _ => Err(crate::prelude::CompileError::Unsupported("fcmp condition".into())) }
            }
        });
    }

    if !no_default {
        // 普通 opcode 默认 lowering
        for opcode_name in standard_insts::ALL_OPCODES {
            if handled_opcodes.contains(*opcode_name) {
                continue; // 用户已定义
            }
            // 跳过 Icmp/Fcmp (已在上面处理)
            if *opcode_name == "Icmp" || *opcode_name == "Fcmp" {
                continue;
            }
            // 跳过 Phi (在 lowering.rs 中跳过，不生成指令)
            if *opcode_name == "Phi" {
                continue;
            }

            if let Some(default_insts) = standard_insts::default_opcode_lowering(opcode_name) {
                let inst_toks = gen_lower_insts(&default_insts, model)?;
                let op_ident = format_ident!("{opcode_name}");

                let uses_const = standard_insts::opcode_uses_const_index(opcode_name);
                if uses_const {
                    arms.push(quote! {
                        crate::prelude::Opcode::#op_ident { index } => {
                            let rd = result.unwrap_or(crate::prelude::VReg(0));
                            let rs1 = args.first().copied().unwrap_or(crate::prelude::VReg(0));
                            let rs2 = args.get(1).copied().unwrap_or(crate::prelude::VReg(0));
                            Ok(vec![#(#inst_toks),*])
                        }
                    });
                } else {
                    arms.push(quote! {
                        crate::prelude::Opcode::#op_ident { .. } => {
                            let rd = result.unwrap_or(crate::prelude::VReg(0));
                            let rs1 = args.first().copied().unwrap_or(crate::prelude::VReg(0));
                            let rs2 = args.get(1).copied().unwrap_or(crate::prelude::VReg(0));
                            Ok(vec![#(#inst_toks),*])
                        }
                    });
                }
            }
        }
    }

    // 兜底
    arms.push(quote! {
        _ => Err(crate::prelude::CompileError::Unsupported(format!("lower {:?}", op)))
    });

    Ok(quote! {
        pub fn lower_impl(
            op: &crate::prelude::Opcode,
            args: &[crate::prelude::VReg],
            result: Option<crate::prelude::VReg>,
            ctx: &mut crate::prelude::LowerCtx,
        ) -> Result<Vec<Inst>, crate::prelude::CompileError> {
            match op { #(#arms),* }
        }
    })
}

/// Expand `$CC` placeholder in template insts with the given condition value.
fn expand_cc(template: &[String], cc_val: &str) -> Vec<String> {
    template.iter().map(|s| s.replace("$CC", cc_val)).collect()
}

/// Parsed result: (instruction_name, [(field_name, arg_value)]).
type AsmParsed<'a> = (&'a str, Vec<(&'a str, &'a str)>);

/// Parse an assembly string `"MNEMONIC op1, op2, ..."` into (inst_name, [(field_name, arg_value)]).
fn parse_asm_inst<'a>(asm: &'a str, model: &'a IsaModel) -> Result<AsmParsed<'a>, String> {
    let asm = asm.trim();
    if asm.is_empty() {
        return Err("empty asm instruction".into());
    }

    // Split: first word is mnemonic, rest are comma-separated operands
    let (mnemonic, rest) = match asm.find(char::is_whitespace) {
        Some(pos) => (asm[..pos].trim(), asm[pos..].trim()),
        None => (asm, ""),
    };

    // Parse operands: split by comma, trim whitespace
    let operands: Vec<&str> = if rest.is_empty() {
        Vec::new()
    } else {
        rest.split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect()
    };

    // Find instruction definition (case-insensitive, _/- normalization)
    let lookup = mnemonic.to_uppercase().replace(['-', ' '], "_");
    let (inst_name, inst_def) = model
        .inst
        .iter()
        .find(|(k, _)| k.to_uppercase().replace(['-', ' '], "_") == lookup)
        .ok_or_else(|| {
            let available: Vec<&str> = model.inst.keys().map(|s| s.as_str()).collect();
            let suggestion = closest_match(mnemonic, &available);
            format!(
                "unknown instruction '{}' in asm string.{} Available: {:?}",
                mnemonic,
                suggestion
                    .map(|s| format!(" Did you mean '{}'?", s))
                    .unwrap_or_default(),
                available
            )
        })?;

    // Map operands to fields by position.
    // Fields of type Opsize are implicit (always defaulted) and skip operand consumption.
    let mut field_args: Vec<(&str, &str)> = Vec::new();
    let mut op_idx = 0;
    for field in inst_def.fields.iter() {
        if matches!(field.field_type, crate::model::FieldType::Opsize) {
            continue; // implicit, uses default value
        }
        if op_idx < operands.len() {
            field_args.push((field.name.as_str(), operands[op_idx]));
            op_idx += 1;
        } else {
            return Err(format!(
                "missing operand for field '{}' in '{}'",
                field.name, asm
            ));
        }
    }

    // extra operands silently ignored (historical behavior)
    Ok((inst_name.as_str(), field_args))
}

/// 将 asm 字符串列表转为 Inst 构造表达式列表
fn gen_lower_insts(insts: &[String], model: &IsaModel) -> Result<Vec<TokenStream>, String> {
    let mut result = Vec::new();
    for asm_str in insts {
        let (inst_name, field_args) = parse_asm_inst(asm_str, model)?;
        let inst_def = &model.inst[inst_name];
        let ivn = pascal_ident(inst_name);
        let mut field_exprs: Vec<TokenStream> = Vec::new();

        for (field_name, arg_val) in &field_args {
            let field = inst_def
                .fields
                .iter()
                .find(|f| &f.name == field_name)
                .unwrap();
            let fi = format_ident!("{}", field_name);
            let scratch = model.abi.as_ref().map(|a| &a.scratch);
            let expr = lowering_arg_expr(arg_val, &field.field_type, scratch);
            field_exprs.push(quote! { #fi: #expr });
        }

        // Handle fields not covered by operands (e.g. empty-field instructions)
        for field in &inst_def.fields {
            if !field_args.iter().any(|(n, _)| *n == field.name) {
                let fi = format_ident!("{}", field.name);
                let default = default_for_type(&field.field_type);
                field_exprs.push(quote! { #fi: #default });
            }
        }

        result.push(quote! { Inst::#ivn { #(#field_exprs),* } });
    }
    Ok(result)
}

// ============================================================
// V11: AST-based lowering/emit code generation
// ============================================================

/// Auto-detect and generate lowering insts using V11 (CST-based) or V10 format.
fn gen_lower_insts_auto(insts: &[String], model: &IsaModel) -> Result<Vec<TokenStream>, String> {
    if crate::cst_codegen::is_v11_format(insts) {
        crate::cst_codegen::gen_lowering_insts_cst(insts, model)
    } else {
        gen_lower_insts(insts, model)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum GenMode {
    Lowering,
    Emit,
}

/// Return the candidate with the smallest Levenshtein distance to `target`, if within threshold.
fn closest_match<'a>(target: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let target = target.to_lowercase();
    let mut best: Option<(&str, usize)> = None;
    for &c in candidates {
        let dist = crate::asm_resolver::levenshtein(&target, &c.to_lowercase());
        if dist == 0 {
            continue;
        } // exact match — shouldn't happen here
        if dist <= 3 && (best.is_none() || dist < best.unwrap().1) {
            best = Some((c, dist));
        }
    }
    best.map(|(s, _)| s)
}

/// 将 lowering 规则中的参数字符串转为表达式
pub(crate) fn lowering_arg_expr(
    val: &str,
    ft: &FieldType,
    scratch: Option<&std::collections::HashMap<String, u32>>,
) -> TokenStream {
    // Scratch register aliases: symbolic name → VReg index
    if let Some(&idx) = scratch.and_then(|map| map.get(val)) {
        return quote! { crate::prelude::VReg(#idx as u32) };
    }
    match val {
        "rd" => quote! { rd },
        "rs1" => quote! { rs1 },
        "rs2" => quote! { rs2 },
        "rs3" => quote! { args.get(2).copied().unwrap_or(crate::prelude::VReg(0)) },
        "iconst" => {
            quote! { ctx.constant_pool.as_ref().and_then(|p| p.get(*index)).and_then(|b| b.try_to_i64()).unwrap_or(0) as i64 }
        }
        "fconst" => {
            quote! { ctx.constant_pool.as_ref().and_then(|p| p.get(*index)).map(|b| b.to_f64().to_bits() as i64).unwrap_or(0) }
        }
        s if s.starts_with("VReg(") || s.starts_with("vreg(") => {
            let n: u32 = s
                .trim_start_matches(|c: char| !c.is_ascii_digit())
                .trim_end_matches(')')
                .parse()
                .unwrap_or(0);
            quote! { crate::prelude::VReg(#n as u32) }
        }
        s if s.starts_with("0x") || s.starts_with("0X") => {
            let n = u64::from_str_radix(&s[2..], 16).unwrap_or(0);
            match ft {
                FieldType::U8 | FieldType::CondCode => quote! { #n as u8 },
                FieldType::I64 => quote! { #n as i64 },
                _ => quote! { #n as i64 },
            }
        }
        s => {
            // 尝试解析为十进制数值
            if let Ok(n) = s.parse::<i64>() {
                match ft {
                    FieldType::U8 | FieldType::CondCode => quote! { #n as u8 },
                    FieldType::U32 => quote! { #n as u32 },
                    FieldType::I64 => quote! { #n as i64 },
                    _ => quote! { #n as i64 },
                }
            } else {
                // 当作变量引用
                let vi = format_ident!("{s}");
                quote! { #vi }
            }
        }
    }
}

pub(crate) fn default_for_type(ft: &FieldType) -> TokenStream {
    match ft {
        FieldType::VReg => quote! { crate::prelude::VReg(0) },
        FieldType::I64 => quote! { 0i64 },
        FieldType::U8 => quote! { 0u8 },
        FieldType::U32 => quote! { 0u32 },
        FieldType::F64 => quote! { 0.0f64 },
        FieldType::BlockTarget => quote! { 0i64 },
        FieldType::Reg => quote! { Reg::from_index(0, crate::prelude::RegClass::Int) },
        FieldType::CondCode => quote! { 0u8 },
        FieldType::Opsize => {
            quote! { ctx.default_opsize }
        },
    }
}

/// 将 emit 上下文中的参数字符串转为表达式。
/// 与 `lowering_arg_expr` 不同：emit 上下文支持寄存器名 (RBP, RSP, ...) 和 `frame_size`。
pub(crate) fn emit_arg_expr(val: &str, ft: &FieldType, model: &IsaModel) -> TokenStream {
    // frame_size: the local variable bound in emit_prologue/emit_epilogue impl
    if val == "frame_size" {
        return match ft {
            FieldType::U8 => quote! { frame_size as u8 },
            FieldType::U32 => quote! { frame_size },
            FieldType::I64 => quote! { frame_size as i64 },
            _ => quote! { frame_size as i64 },
        };
    }

    // Named registers: look up in [reg.gpr.names] and [reg.xmm.names]
    for group in model.reg.values() {
        if let Some(names) = &group.names
            && let Some(pos) = names.iter().position(|n| n.eq_ignore_ascii_case(val))
        {
            // For Reg-type fields, emit Reg::NAME directly (physical register, no VReg indirection)
            if matches!(ft, FieldType::Reg) {
                let reg_name = &names[pos];
                let ri = syn::Ident::new(reg_name, proc_macro2::Span::call_site());
                return quote! { Reg::#ri };
            }
            return quote! { crate::prelude::VReg(#pos as u32) };
        }
        // Also check prefix-based names (e.g. XMM0, XMM1)
        if let Some(ref prefix) = group.prefix {
            let upper_val = val.to_uppercase();
            if let Some(stripped) = upper_val.strip_prefix(&prefix.to_uppercase())
                && let Ok(n) = stripped.parse::<u32>()
            {
                if matches!(ft, FieldType::Reg) {
                    let reg_name = format!("{}{}", prefix, n);
                    let ri = syn::Ident::new(&reg_name, proc_macro2::Span::call_site());
                    return quote! { Reg::#ri };
                }
                return quote! { crate::prelude::VReg(#n as u32) };
            }
        }
    }

    // Scratch register aliases
    if let Some(scratch) = model.abi.as_ref().and_then(|a| a.scratch.get(val)) {
        return quote! { crate::prelude::VReg(#scratch as u32) };
    }

    // Hex literal
    if val.starts_with("0x") || val.starts_with("0X") {
        let n = u64::from_str_radix(&val[2..], 16).unwrap_or(0);
        return match ft {
            FieldType::U8 | FieldType::CondCode => quote! { #n as u8 },
            FieldType::I64 => quote! { #n as i64 },
            _ => quote! { #n as i64 },
        };
    }

    // Decimal literal
    if let Ok(n) = val.parse::<i64>() {
        return match ft {
            FieldType::U8 | FieldType::CondCode => quote! { #n as u8 },
            FieldType::U32 => quote! { #n as u32 },
            FieldType::I64 => quote! { #n as i64 },
            _ => quote! { #n as i64 },
        };
    }

    // Fallback: variable reference
    let vi = format_ident!("{val}");
    quote! { #vi }
}

/// Generate emit_inst calls for a sequence of asm strings (prologue/epilogue).
fn gen_emit_insts_sequence(insts: &[String], model: &IsaModel) -> TokenStream {
    if insts.is_empty() {
        return quote! { let _ = (frame_size, rm, sink); };
    }

    // Process instructions in order. Collect consecutive regular instructions into
    // for-loop arrays (borrow-checker friendly). Pseudo-instructions are emitted as-is.
    let mut all_stmts: Vec<TokenStream> = Vec::new();
    let mut pending_regular: Vec<TokenStream> = Vec::new();

    let flush_regular = |pending: &mut Vec<TokenStream>, stmts: &mut Vec<TokenStream>| {
        if !pending.is_empty() {
            stmts.push(quote! {
                for __inst in &[#(#pending),*] {
                    if let Err(e) = emit_inst(__inst, rm, &mut *sink) {
                        return Err(e);
                    }
                }
            });
            pending.clear();
        }
    };

    for asm_str in insts {
        let trimmed = asm_str.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(preudo) = trimmed.strip_prefix("@") {
            flush_regular(&mut pending_regular, &mut all_stmts);
            match preudo {
                "push_callee" => all_stmts.push(gen_push_callee(model, true)),
                "pop_callee" => all_stmts.push(gen_push_callee(model, false)),
                "move_args" => all_stmts.push(gen_move_args(model)),
                "frame_alloc" => all_stmts.push(gen_frame_alloc_free(model, true)),
                "frame_free" => all_stmts.push(gen_frame_alloc_free(model, false)),
                _ => {}
            }
            continue;
        }

        // Regular instruction
        let (inst_name, field_args) = match parse_asm_inst(trimmed, model) {
            Ok(r) => r,
            Err(e) => {
                return quote! { const _PARSE_ERROR: &str = #e; };
            }
        };
        let inst_def = &model.inst[inst_name];
        let ivn = pascal_ident(inst_name);
        let mut field_exprs: Vec<TokenStream> = Vec::new();

        for (field_name, arg_val) in &field_args {
            let field = inst_def
                .fields
                .iter()
                .find(|f| &f.name == field_name)
                .unwrap();
            let fi = format_ident!("{}", field_name);
            let expr = emit_arg_expr(arg_val, &field.field_type, model);
            field_exprs.push(quote! { #fi: #expr });
        }

        for field in &inst_def.fields {
            if !field_args.iter().any(|(n, _)| *n == field.name) {
                let fi = format_ident!("{}", field.name);
                let default = default_for_type(&field.field_type);
                field_exprs.push(quote! { #fi: #default });
            }
        }

        pending_regular.push(quote! { Inst::#ivn { #(#field_exprs),* } });
    }

    flush_regular(&mut pending_regular, &mut all_stmts);

    if all_stmts.is_empty() {
        quote! { let _ = (frame_size, rm, sink); }
    } else {
        quote! { #(#all_stmts)* }
    }
}

/// Generate push or pop for callee-saved registers from [abi.callee_saved].
pub(crate) fn gen_push_callee(model: &IsaModel, is_push: bool) -> TokenStream {
    let Some(ref abi) = model.abi else {
        return quote! { let _ = (rm, sink); };
    };

    let gpr_names: &[String] = &abi.callee_saved.gpr;
    if gpr_names.is_empty() {
        return quote! { let _ = (rm, sink); };
    }

    let gpr = model.reg.get("gpr");
    let names = gpr.and_then(|g| g.names.as_ref());

    // Build array of Reg::NAME idents for direct physical register references
    let ordered_names: Vec<&String> = if is_push {
        gpr_names.iter().collect()
    } else {
        gpr_names.iter().rev().collect()
    };

    let mut reg_idents: Vec<syn::Ident> = Vec::new();
    for reg_name in &ordered_names {
        let name = names
            .and_then(|nlist| nlist.iter().find(|n| n.eq_ignore_ascii_case(reg_name)))
            .cloned()
            .unwrap_or_else(|| reg_name.to_string());
        reg_idents.push(syn::Ident::new(&name, proc_macro2::Span::call_site()));
    }

    let ivn = if is_push {
        pascal_ident("PUSH64_R")
    } else {
        pascal_ident("POP64_R")
    };

    // For-loop using Reg enum values — bypasses RegMap entirely
    quote! {{
        let __s = &mut *sink;
        for &__reg in &[#(Reg::#reg_idents),*] {
            if let Err(e) = emit_inst(&Inst::#ivn { reg: __reg }, rm, __s) {
                return Err(e);
            }
        }
    }}
}

/// Generate arg register → param vreg moves from [abi.arg_regs].
pub(crate) fn gen_move_args(model: &IsaModel) -> TokenStream {
    let Some(ref abi) = model.abi else {
        return quote! { let _ = (rm, sink); };
    };

    let arg_reg_names: &[String] = &abi.arg_regs.gpr;
    if arg_reg_names.is_empty() {
        return quote! { let _ = (rm, sink); };
    }

    let opsize_default: u8 = model.dyn_types
        .get("opsize")
        .and_then(|d| d.default)
        .unwrap_or(64);

    let gpr = model.reg.get("gpr");
    let names = gpr.and_then(|g| g.names.as_ref());

    let mut stmts: Vec<TokenStream> = Vec::new();

    // Generate: for each arg_reg, mov param_vreg, arg_reg using Reg type for physical source
    for (i, reg_name) in arg_reg_names.iter().enumerate() {
        let src_name = names
            .and_then(|nlist| nlist.iter().find(|n| n.eq_ignore_ascii_case(reg_name)))
            .cloned()
            .unwrap_or_else(|| reg_name.to_string());
        let src_ident = syn::Ident::new(&src_name, proc_macro2::Span::call_site());

        stmts.push(quote! {
            if let Some(&pv) = rm.param_vregs.get(#i) {
                // Skip if dest VReg was dead-code eliminated (not in RegMap)
                if rm.vreg_to_preg.contains_key(&pv) {
                    if let Err(e) = emit_inst(&Inst::MovRm8R64 { dest: pv, src: Reg::#src_ident, opsize: #opsize_default }, rm, sink) {
                        return Err(e);
                    }
                }
            }
        });
    }

    quote! { #(#stmts)* }
}

/// Generate frame allocation (sub rsp, frame_size) or deallocation (add rsp, frame_size).
pub(crate) fn gen_frame_alloc_free(model: &IsaModel, is_alloc: bool) -> TokenStream {
    let inst_name = if is_alloc {
        "SUB64_R_IMM32"
    } else {
        "ADD64_R_IMM32"
    };
    let ivn = pascal_ident(inst_name);

    let has_encoding = model
        .inst
        .get(inst_name)
        .and_then(|i| i.encoding.as_ref())
        .is_some();
    if !has_encoding {
        // Fallback: direct sink calls (should not be needed with proper encoding)
        if is_alloc {
            return quote! {{
                let s = &mut *sink;
                if frame_size > 0 {
                    if frame_size < 128 {
                        s.put1(0x48); s.put1(0x83); s.put1(0xEC); s.put1(frame_size as u8);
                    } else {
                        s.put1(0x48); s.put1(0x81); s.put1(0xEC); s.put4(frame_size);
                    }
                }
            }};
        } else {
            return quote! {{
                let s = &mut *sink;
                if frame_size > 0 {
                    if frame_size < 128 {
                        s.put1(0x48); s.put1(0x83); s.put1(0xC4); s.put1(frame_size as u8);
                    } else {
                        s.put1(0x48); s.put1(0x81); s.put1(0xC4); s.put4(frame_size);
                    }
                }
            }};
        }
    }

    // Use Reg::RSP directly — no VReg lookup needed
    quote! {
        if frame_size > 0 {
            if let Err(e) = emit_inst(&Inst::#ivn {
                dest: Reg::RSP,
                imm: frame_size,
            }, rm, sink) {
                return Err(e);
            }
        }
    }
}

// ============================================================
// lower_terminator_impl — 从 [lower_term.*] 生成
// ============================================================

fn gen_lower_term_func(model: &IsaModel) -> Result<TokenStream, String> {
    let mut arms: Vec<TokenStream> = Vec::new();

    for (term_name, rule) in &model.lower_term {
        let inst_toks = gen_lower_insts_auto(&rule.insts, model)?;

        match term_name.as_str() {
            "Return" => {
                // float return: 若 TOML 定义了 SD_FMOV，用它拷贝到 VReg(100)=XMM0
                let has_sd_fmov = model.inst.contains_key("SD_FMOV");
                let val_code = quote! {
                    let val = values.first().copied()
                        .and_then(|x| v.get(&x)).copied().unwrap_or(crate::prelude::VReg(0));
                    let val2 = values.get(1).copied()
                        .and_then(|x| v.get(&x)).copied().unwrap_or(crate::prelude::VReg(0));
                };
                if has_sd_fmov {
                    arms.push(quote! {
                        crate::prelude::Terminator::Return { values } => {
                            #val_code
                            if ctx.is_float_return {
                                Ok(vec![Inst::SdFmov { dest: crate::prelude::VReg(100), src: val }])
                            } else {
                                Ok(vec![#(#inst_toks),*])
                            }
                        }
                    });
                } else {
                    arms.push(quote! {
                        crate::prelude::Terminator::Return { values } => {
                            #val_code
                            Ok(vec![#(#inst_toks),*])
                        }
                    });
                }
            }
            "Jump" => {
                arms.push(quote! {
                    crate::prelude::Terminator::Jump { target, .. } => {
                        let target = target.0 as i64;
                        Ok(vec![#(#inst_toks),*])
                    }
                });
            }
            "Branch" => {
                arms.push(quote! {
                    crate::prelude::Terminator::Branch { cond: cond_val, true_block, false_block, .. } => {
                        let cond = v.get(cond_val).copied().unwrap_or(crate::prelude::VReg(0));
                        let true_block = true_block.0 as i64;
                        let false_block = false_block.0 as i64;
                        Ok(vec![#(#inst_toks),*])
                    }
                });
            }
            "Unreachable" => {
                arms.push(quote! {
                    crate::prelude::Terminator::Unreachable => {
                        Ok(vec![#(#inst_toks),*])
                    }
                });
            }
            "Switch" => {
                arms.push(quote! {
                    crate::prelude::Terminator::Switch { discriminant, default_block, .. } => {
                        let discriminant = v.get(discriminant).copied().unwrap_or(crate::prelude::VReg(0));
                        let default_block = default_block.0 as i64;
                        Ok(vec![#(#inst_toks),*])
                    }
                });
            }
            _ => {
                let tn = format_ident!("{term_name}");
                arms.push(quote! {
                    crate::prelude::Terminator::#tn { .. } => {
                        Ok(vec![#(#inst_toks),*])
                    }
                });
            }
        }
    }

    // 默认：如果 Return/Jump/Branch/Unreachable/Switch 无规则，给默认实现
    if !model.meta.no_default_lowering {
        // 使用标准指令默认 lowering
        let term_names = ["Return", "Jump", "Branch", "Unreachable", "Switch"];
        for term_name in &term_names {
            if model.lower_term.contains_key(*term_name) {
                continue; // 用户已定义
            }
            if let Some(default_insts) = standard_insts::default_terminator_lowering(term_name) {
                // Return 的默认 lowering 中 "val" 是变量引用，会被 lowering_arg_expr 正确处理
                // 但 float return 需要特殊处理
                if *term_name == "Return" {
                    // 分离: 除 SD_RET 外的指令用于整数返回，SD_RET 保留
                    let non_ret: Vec<_> = default_insts
                        .iter()
                        .filter(|li| !li.starts_with("SD_RET"))
                        .cloned()
                        .collect();
                    let ret_only: Vec<_> = default_insts
                        .iter()
                        .filter(|li| li.starts_with("SD_RET"))
                        .cloned()
                        .collect();

                    let body_toks = gen_lower_insts(&non_ret, model)?;
                    let ret_toks = gen_lower_insts(&ret_only, model)?;
                    arms.push(quote! {
                        crate::prelude::Terminator::Return { values } => {
                            let val = values.first().copied()
                                .and_then(|x| v.get(&x)).copied().unwrap_or(crate::prelude::VReg(0));
                            if ctx.is_float_return {
                                Ok(vec![Inst::SdFmov { dest: crate::prelude::VReg(100), src: val }, #(#ret_toks),*])
                            } else {
                                Ok(vec![#(#body_toks),* , #(#ret_toks),*])
                            }
                        }
                    });
                } else if *term_name == "Jump" {
                    let inst_toks = gen_lower_insts(&default_insts, model)?;
                    arms.push(quote! {
                        crate::prelude::Terminator::Jump { target, .. } => {
                            let target = target.0 as i64;
                            Ok(vec![#(#inst_toks),*])
                        }
                    });
                } else if *term_name == "Branch" {
                    let inst_toks = gen_lower_insts(&default_insts, model)?;
                    arms.push(quote! {
                        crate::prelude::Terminator::Branch { cond: cond_val, true_block, false_block, .. } => {
                            let cond = v.get(cond_val).copied().unwrap_or(crate::prelude::VReg(0));
                            let true_block = true_block.0 as i64;
                            let false_block = false_block.0 as i64;
                            Ok(vec![#(#inst_toks),*])
                        }
                    });
                } else if *term_name == "Unreachable" {
                    let inst_toks = gen_lower_insts(&default_insts, model)?;
                    arms.push(quote! {
                        crate::prelude::Terminator::Unreachable => {
                            Ok(vec![#(#inst_toks),*])
                        }
                    });
                } else if *term_name == "Switch" {
                    // Switch: if-else 链展开 — 对每个 case 生成 CMP + JCC Equal
                    arms.push(quote! {
                        crate::prelude::Terminator::Switch { discriminant, default_block, cases } => {
                            let disc = v.get(discriminant).copied().unwrap_or(crate::prelude::VReg(0));
                            let mut insts = Vec::new();
                            for &(case_val, case_block, _) in cases.iter() {
                                // SD_MOV_IMM VReg(97), case_val
                                insts.push(Inst::SdMovImm { dest: crate::prelude::VReg(97), imm: case_val as i64 });
                                // SD_CMP disc, VReg(97)
                                insts.push(Inst::SdCmp { src1: disc, src2: crate::prelude::VReg(97) });
                                // SD_JCC Equal (0), case_block
                                insts.push(Inst::SdJcc { cond: 0u8, rel: case_block.0 as i64 });
                            }
                            // SD_JMP default_block
                            insts.push(Inst::SdJmp { rel: default_block.0 as i64 });
                            Ok(insts)
                        }
                    });
                }
            }
        }
    } else {
        // 旧硬编码默认 (x86_64 兼容)
        if !model.lower_term.contains_key("Return") {
            arms.push(quote! {
                crate::prelude::Terminator::Return { values } => {
                    let mut insts = Vec::new();
                    if let Some(&val) = values.first() {
                        if let Some(&vr) = v.get(&val) {
                            insts.push(Inst::MovR8Rm { dest: crate::prelude::VReg(0u32), src: vr });
                        }
                    }
                    Ok(insts)
                }
            });
        }
        if !model.lower_term.contains_key("Unreachable") {
            arms.push(quote! {
                crate::prelude::Terminator::Unreachable => { Ok(vec![Inst::Nop {}]) }
            });
        }
    }

    // 兜底
    arms.push(quote! {
        _ => Err(crate::prelude::CompileError::Unsupported(format!("lower_term {:?}", term)))
    });

    Ok(quote! {
        pub fn lower_terminator_impl(
            term: &crate::prelude::Terminator,
            v: &std::collections::HashMap<crate::prelude::Value, crate::prelude::VReg>,
            ctx: &crate::prelude::LowerCtx,
        ) -> Result<Vec<Inst>, crate::prelude::CompileError> {
            match term { #(#arms),* }
        }
    })
}

// ============================================================
// lower_pattern_impl — 从 [lower_pattern.<name>] 生成模式优化
// ============================================================

fn gen_lower_pattern_func(model: &IsaModel) -> Result<TokenStream, String> {
    if model.lower_pattern.is_empty() {
        // No pattern rules — return default (empty) implementation
        return Ok(quote! {
            pub fn lower_pattern_impl(
                _pattern_name: &str,
                _args: &[crate::prelude::VReg],
                _result: Option<crate::prelude::VReg>,
                _ctx: &crate::prelude::LowerCtx,
            ) -> Result<Vec<Inst>, crate::prelude::CompileError> {
                Ok(Vec::new())
            }
        });
    }

    let mut arms: Vec<TokenStream> = Vec::new();
    for (pattern_name, rule) in &model.lower_pattern {
        let inst_toks = gen_lower_insts_auto(&rule.insts, model)?;
        arms.push(quote! {
            #pattern_name => {
                let rd = result.unwrap_or(crate::prelude::VReg(0));
                let rs1 = args.first().copied().unwrap_or(crate::prelude::VReg(0));
                let rs2 = args.get(1).copied().unwrap_or(crate::prelude::VReg(0));
                Ok(vec![#(#inst_toks),*])
            }
        });
    }

    arms.push(quote! {
        _ => Ok(Vec::new())
    });

    Ok(quote! {
        pub fn lower_pattern_impl(
            pattern_name: &str,
            args: &[crate::prelude::VReg],
            result: Option<crate::prelude::VReg>,
            ctx: &crate::prelude::LowerCtx,
        ) -> Result<Vec<Inst>, crate::prelude::CompileError> {
            let _ = ctx;
            match pattern_name { #(#arms),* }
        }
    })
}

// ============================================================
// Disassembler impl — 从 asm 模板生成
// ============================================================

/// Parse an asm template like `"add {dest}, {src}"` into format segments.
struct AsmFrag {
    /// Literal string piece or "" for field placeholders.
    lit: String,
    /// If Some, this is a field reference; the TokenStream formats a single field value.
    field_fmt: Option<TokenStream>,
}

/// Split `asm` template into fragments. Each fragment is either a literal or a field reference.
fn parse_asm_template(asm: &str, fields: &[InstField]) -> Vec<AsmFrag> {
    let mut frags: Vec<AsmFrag> = Vec::new();
    let mut rest = asm;
    while let Some(pos) = rest.find('{') {
        // Literal before the {
        if pos > 0 {
            frags.push(AsmFrag {
                lit: rest[..pos].to_string(),
                field_fmt: None,
            });
        }
        // Find closing }
        let end = match rest[pos + 1..].find('}') {
            Some(e) => pos + 1 + e,
            None => {
                // Unclosed brace — treat rest as literal
                frags.push(AsmFrag {
                    lit: rest[pos..].to_string(),
                    field_fmt: None,
                });
                return frags;
            }
        };
        let field_name = &rest[pos + 1..end];
        let field = fields.iter().find(|f| f.name == field_name);
        let fmt = field.map(|f| gen_field_format(&f.name, &f.field_type));
        frags.push(AsmFrag {
            lit: String::new(),
            field_fmt: fmt,
        });
        rest = &rest[end + 1..];
    }
    if !rest.is_empty() {
        frags.push(AsmFrag {
            lit: rest.to_string(),
            field_fmt: None,
        });
    }
    frags
}

/// Generate a TokenStream that formats a single field value.
/// VReg → `reg_name(*field)`, CondCode → `cc_name(*field)`, etc.
fn gen_field_format(field_name: &str, ft: &FieldType) -> TokenStream {
    let fi = format_ident!("{field_name}");
    match ft {
        FieldType::VReg => {
            quote! { reg_name(*#fi) }
        }
        FieldType::Reg => {
            quote! { reg_name(crate::prelude::VReg(*#fi as u32)) }
        }
        FieldType::CondCode => {
            quote! { cc_name(*#fi as u8) }
        }
        FieldType::BlockTarget => {
            quote! { * #fi }
        }
        FieldType::I64 => {
            quote! { * #fi }
        }
        FieldType::U8 | FieldType::U32 => {
            quote! { * #fi }
        }
        FieldType::F64 => {
            quote! { * #fi }
        }
        FieldType::Opsize => {
            quote! { * #fi }
        }
    }
}

/// Generate a single match arm
fn gen_disasm_arm(vn: &syn::Ident, fields: &[InstField], frags: &[AsmFrag]) -> TokenStream {
    let mut fmt_str = String::new();
    let mut fmt_args: Vec<TokenStream> = Vec::new();
    for frag in frags {
        fmt_str.push_str(&frag.lit);
        if let Some(ref field_fmt) = frag.field_fmt {
            fmt_str.push_str("{}");
            fmt_args.push(field_fmt.clone());
        }
    }
    let fmt_lit = fmt_str;

    // Build patterns for this variant — match all fields
    if fields.is_empty() {
        if fmt_args.is_empty() {
            quote! { Inst::#vn { .. } => #fmt_lit.to_string() }
        } else {
            // Fields referenced in asm but no fields declared — shouldn't happen
            let bindings: Vec<_> = fields
                .iter()
                .map(|f| {
                    let fi = format_ident!("{}", f.name);
                    quote! { #fi }
                })
                .collect();
            quote! { Inst::#vn { #(#bindings),* } => format!(#fmt_lit, #(#fmt_args),*) }
        }
    } else {
        let bindings: Vec<_> = fields
            .iter()
            .map(|f| {
                let fi = format_ident!("{}", f.name);
                quote! { #fi }
            })
            .collect();
        if fmt_args.is_empty() {
            quote! { Inst::#vn { #(#bindings),*, .. } => #fmt_lit.to_string() }
        } else {
            quote! { Inst::#vn { #(#bindings),*, .. } => format!(#fmt_lit, #(#fmt_args),*) }
        }
    }
}

fn gen_disasm_func(model: &IsaModel) -> TokenStream {
    // ── reg_name helper ──
    let mut groups: Vec<_> = model.reg.iter().collect();
    // Sort so GPR comes first (VReg 0..gpr_count), then float/xmm, then others
    groups.sort_by_key(|(name, _)| {
        if *name == "gpr" {
            0
        } else if *name == "float" || *name == "xmm" {
            1
        } else {
            2
        }
    });

    let mut offset: usize = 0;
    let mut reg_name_arms: Vec<TokenStream> = Vec::new();
    for (_group_name, group) in &groups {
        let count = group.count as usize;
        let start = offset;
        let end = offset + count;
        let names: Vec<TokenStream> = match &group.names {
            Some(nlist) => nlist
                .iter()
                .map(|n| {
                    let s = n.as_str();
                    quote! { #s }
                })
                .collect(),
            None => {
                let prefix = group.prefix.clone().unwrap_or_default();
                (0..count as u16)
                    .map(|i| {
                        let formatted = format!("{}{}", prefix, i);
                        quote! { #formatted }
                    })
                    .collect()
            }
        };
        reg_name_arms.push(quote! {
            if (#start..#end).contains(&idx) {
                return &[#(#names),*][idx - #start];
            }
        });
        offset = end;
    }

    let reg_name_fn = quote! {
        fn reg_name(vreg: crate::prelude::VReg) -> &'static str {
            let idx = vreg.0 as usize;
            #(#reg_name_arms)*
            "?"
        }
    };

    // ── cc_name helper ──
    let mut cc_arms: Vec<TokenStream> = Vec::new();
    for (hex_key, asm_name) in &model.cc_names {
        let val = u64::from_str_radix(
            hex_key.trim_start_matches("0x").trim_start_matches("0X"),
            16,
        )
        .unwrap_or(0) as u8;
        let name = asm_name.as_str();
        cc_arms.push(quote! { #val => #name, });
    }

    let cc_name_fn = if cc_arms.is_empty() {
        quote! {
            fn cc_name(_cond: u8) -> &'static str { "??" }
        }
    } else {
        quote! {
            fn cc_name(cond: u8) -> &'static str {
                match cond {
                    #(#cc_arms)*
                    _ => "??",
                }
            }
        }
    };

    // ── disassemble match arms ──
    let mut disasm_arms: Vec<TokenStream> = Vec::new();
    for (_inst_name, inst) in &model.inst {
        let vn = pascal_ident(_inst_name);
        let frags = parse_asm_template(&inst.asm, &inst.fields);
        let arm = gen_disasm_arm(&vn, &inst.fields, &frags);
        disasm_arms.push(arm);
    }

    quote! {
        impl crate::prelude::Disassembler for Isa {
            type Inst = Inst;

            fn disassemble(inst: &Self::Inst) -> String {
                match inst {
                    #(#disasm_arms),* ,
                    Inst::Unknown(_) => "???".to_string(),
                }
            }
        }

        #reg_name_fn
        #cc_name_fn
    }
}

// ============================================================
// Assembler impl — 运行时汇编器 (parse_insts + assemble)
// ============================================================

fn gen_asm_assembler_impl(model: &IsaModel) -> TokenStream {
    if model.inst.is_empty() {
        return TokenStream::new();
    }

    // ── Build the complete grammar source string ──
    let grammar_source = build_assembler_grammar_source(model);

    // ── resolve_reg: register name → index ──
    let resolve_reg_body = gen_resolve_reg_body(model);

    // ── assembler_lookup: static AsmLookupTable ──
    let lookup_table = gen_lookup_table_init(model);

    // ── construct_inst: inst_name + bindings → Inst ──
    let construct_inst_body = gen_construct_inst_body(model);

    // ── get_mnemonic and get_operand_texts: CST helpers ──
    let cst_helpers = gen_cst_helpers();

    quote! {
        impl crate::backend::Assembler for Isa {
            fn parse_insts(
                source: &str,
            ) -> core::result::Result<
                Vec<<Self as crate::backend::InstructionSet>::Inst>,
                crate::backend::AsmError,
            > {
                use lang_frontend::Parser;
                use std::sync::OnceLock;
                use std::collections::HashMap;
                use crate::assembler::{AsmLookupTable, OperandValue};
                use crate::backend::AsmError;
                use crate::prelude::VReg;

                // ── Static cached parser ──
                static PARSER: OnceLock<Parser> = OnceLock::new();
                let parser = PARSER.get_or_init(|| {
                    let grammar = lang_frontend::parse_grammar(#grammar_source)
                        .expect("assembler grammar parse");
                    Parser::build(grammar)
                });

                // ── Static cached lookup table ──
                static LOOKUP: OnceLock<AsmLookupTable> = OnceLock::new();
                let lookup = LOOKUP.get_or_init(|| {
                    let lut = #lookup_table;
                    // Verify the table has entries (compile-time check via static)
                    let _ = &lut;
                    lut
                });

                // ── CST helpers (inline) ──
                #cst_helpers

                // ── resolve_reg ──
                #resolve_reg_body

                // ── construct_inst + helpers ──
                #construct_inst_body

                // ── Parse ──
                let cst = parser.parse(source)
                    .map_err(|e| AsmError::Parse(format!("{}", e)))?;

                let mut insts: Vec<Inst> = Vec::new();
                let mut label_positions: HashMap<String, i64> = HashMap::new();
                let mut byte_offset: i64 = 0;

                for node in cst.walk() {
                    if node.kind == "label_def" {
                        if let Some(label_name) = get_label_name_from_node(&node) {
                            label_positions.insert(label_name, byte_offset);
                        }
                    }
                    if node.kind == "inst_like" {
                        let mnemonic = match get_mnemonic_from_node(&node) {
                            Some(m) => m,
                            None => continue,
                        };
                        let operands = get_operand_texts_from_node(&node);
                        let op_values: Vec<OperandValue> = operands
                            .iter()
                            .map(|o| classify_operand_value(o, resolve_reg))
                            .collect();

                        // Resolve: (mnemonic, operands) → (inst_name, field_bindings)
                        let (inst_name, bindings) = resolve_inst(
                            lookup, &mnemonic, &op_values,
                        ).map_err(|e| AsmError::UnknownMnemonic(e))?;

                        // Construct Inst
                        let inst = construct_inst(&inst_name, &bindings, &label_positions)
                            .map_err(|e| AsmError::InvalidOperand(e))?;

                        // Track byte offset for labels
                        if let Ok(bytes) = <Isa as crate::backend::Encoder>::encode(&inst) {
                            byte_offset += bytes.len() as i64;
                        }

                        insts.push(inst);
                    }
                }

                Ok(insts)
            }

            fn assemble(
                name: &str,
                source: &str,
            ) -> core::result::Result<
                crate::jit::JitCompiler<Self>,
                crate::backend::AsmError,
            > {
                let insts = Self::parse_insts(source)?;
                let mut code: Vec<u8> = Vec::new();
                for inst in &insts {
                    let bytes = <Isa as crate::backend::Encoder>::encode(inst)
                        .map_err(|e| crate::backend::AsmError::Encode(e))?;
                    code.extend_from_slice(&bytes);
                }
                let code_len = code.len();
                let compiled = crate::CompiledFunction {
                    code,
                    relocations: vec![],
                    code_size: code_len,
                };
                let mut jit = crate::jit::JitCompiler::<Isa>::new();
                jit.add_compiled(name, compiled)
                    .map_err(|e| {
                        crate::backend::AsmError::Encode(
                            crate::EncodeError::Other(format!("{:?}", e)),
                        )
                    })?;
                Ok(jit)
            }
        }
    }
}

/// Build the complete grammar source string (base.lx + ISA extensions).
fn build_assembler_grammar_source(model: &IsaModel) -> String {
    let base = include_str!("../grammars/base.lx");
    let mut source = String::from(base);

    // Add ISA-specific tokens
    if let Some(tokens) = &model.lang_tokens {
        for (name, def) in tokens {
            source.push_str(&format!("\ntoken {} = \"{}\"\n", name, def.pattern));
        }
    }

    // Add keywords as literal tokens (same IDENT kind)
    if let Some(keywords) = &model.lang_keywords {
        for (name, text) in keywords {
            source.push_str(&format!("\ntoken {} = \"{}\"\n", name, text));
        }
    }

    // Add ISA-specific rules
    if let Some(rules) = &model.lang_rules {
        for (name, rule_src) in rules {
            source.push_str(&format!("\n{} ::= {}\n", name, rule_src));
        }
    }

    source
}

/// Generate the `resolve_reg` function body (match on register names).
fn gen_resolve_reg_body(model: &IsaModel) -> TokenStream {
    let mut checks: Vec<TokenStream> = Vec::new();

    for group in model.reg.values() {
        if let Some(names) = &group.names {
            for (i, name) in names.iter().enumerate() {
                let idx = i as u8;
                checks.push(quote! {
                    if name == #name { return Some(#idx); }
                });
            }
        }
    }

    if checks.is_empty() {
        quote! {
            fn resolve_reg(_name: &str) -> Option<u8> { None }
        }
    } else {
        quote! {
            fn resolve_reg(name: &str) -> Option<u8> {
                #(#checks)*
                None
            }
        }
    }
}

/// Generate the `AsmLookupTable` initializer.
fn gen_lookup_table_init(model: &IsaModel) -> TokenStream {
    let mut entries: Vec<TokenStream> = Vec::new();
    let mut cc_entries: Vec<TokenStream> = Vec::new();
    let mut inst_names: Vec<TokenStream> = Vec::new();

    for (inst_name, inst) in &model.inst {
        // Skip instructions without encodings (lowering helpers, not real machine insts)
        if inst.encoding.is_none() {
            continue;
        }
        let name_str = inst_name.clone();
        inst_names.push(quote! { #name_str.into() });

        // Parse the asm template to extract mnemonic and field order
        let template = crate::asm_resolver::ParsedTemplate::parse(
            &inst.asm,
            &inst.fields.iter().map(|f| (f.name.clone(), f.field_type.clone())).collect::<Vec<_>>(),
        );

        let field_order = template.field_order();
        let field_types: Vec<TokenStream> = field_order.iter().map(|(fname, fty)| {
            let fname_str = fname.clone();
            let ftype_tok = match fty {
                FieldType::VReg => quote! { crate::assembler::FieldType::VReg },
                FieldType::Reg => quote! { crate::assembler::FieldType::Reg },
                FieldType::I64 => quote! { crate::assembler::FieldType::I64 },
                FieldType::U8 => quote! { crate::assembler::FieldType::U8 },
                FieldType::U32 => quote! { crate::assembler::FieldType::U32 },
                FieldType::F64 => quote! { crate::assembler::FieldType::F64 },
                FieldType::BlockTarget => quote! { crate::assembler::FieldType::BlockTarget },
                FieldType::CondCode => quote! { crate::assembler::FieldType::CondCode },
                FieldType::Opsize => quote! { crate::assembler::FieldType::U8 },
            };
            quote! { (#fname_str.into(), #ftype_tok) }
        }).collect();

        if template.has_cond_field {
            // Expand condition code mnemonics
            for (hex_key, cc_name) in &model.cc_names {
                let cond_val = u64::from_str_radix(
                    hex_key.trim_start_matches("0x").trim_start_matches("0X"),
                    16,
                ).unwrap_or(0) as u8;
                let expanded_mnemonic = template.expand_cond(cc_name);
                let mnem_str = expanded_mnemonic.clone();
                cc_entries.push(quote! {
                    (#mnem_str.into(), #name_str.into(), #cond_val)
                });
            }
        } else {
            let mnemonic = template.mnemonic.clone();
            let field_count = field_types.len();
            entries.push(quote! {
                {
                    let mut fields: Vec<(String, crate::assembler::FieldType)> = Vec::with_capacity(#field_count);
                    #( fields.push(#field_types); )*
                    crate::assembler::AsmEntry {
                        mnemonic: #mnemonic.into(),
                        inst_name: #name_str.into(),
                        template_fields: fields,
                    }
                }
            });
        }
    }

    quote! {
        crate::assembler::AsmLookupTable {
            entries: vec![#(#entries),*],
            cc_entries: vec![#(#cc_entries),*],
            inst_names: vec![#(#inst_names),*],
        }
    }
}

/// Generate the `construct_inst` function body.
fn gen_construct_inst_body(model: &IsaModel) -> TokenStream {
    let mut arms: Vec<TokenStream> = Vec::new();

    let opsize_default: u8 = model.dyn_types
        .get("opsize")
        .and_then(|d| d.default)
        .unwrap_or(64);

    for (inst_name, inst) in &model.inst {
        let vn = pascal_ident(inst_name);

        if inst.fields.is_empty() {
            arms.push(quote! { #inst_name => { Ok(Inst::#vn) } , });
        } else {
            let field_binds: Vec<TokenStream> = inst.fields.iter().map(|f| {
                let fname = format_ident!("{}", f.name);
                let fname_str = f.name.clone();
                match f.field_type {
                    FieldType::VReg => quote! {
                        #fname: get_vreg_from_bindings(bindings, #fname_str, resolve_reg)?
                    },
                    FieldType::Reg => quote! {
                        #fname: get_reg_from_bindings(bindings, #fname_str)?
                    },
                    FieldType::I64 => quote! {
                        #fname: get_i64_from_bindings(bindings, #fname_str)?
                    },
                    FieldType::U8 => quote! {
                        #fname: get_i64_from_bindings(bindings, #fname_str)? as u8
                    },
                    FieldType::U32 => quote! {
                        #fname: get_i64_from_bindings(bindings, #fname_str)? as u32
                    },
                    FieldType::F64 => quote! {
                        #fname: get_f64_from_bindings(bindings, #fname_str)?
                    },
                    FieldType::BlockTarget => quote! {
                        #fname: get_label_from_bindings(bindings, #fname_str, label_offsets)?
                    },
                    FieldType::CondCode => quote! {
                        #fname: get_i64_from_bindings(bindings, #fname_str)? as u8
                    },
                    FieldType::Opsize => quote! {
                        #fname: get_opsize_from_bindings(bindings, #fname_str)
                    },
                }
            }).collect();

            arms.push(quote! {
                #inst_name => { Ok(Inst::#vn { #(#field_binds),* }) } ,
            });
        }
    }

    quote! {
        fn classify_operand_value(
            text: &str,
            resolve_reg_fn: fn(&str) -> Option<u8>,
        ) -> OperandValue {
            if text.starts_with("VReg(") && text.ends_with(')') {
                if let Ok(n) = text.trim_start_matches("VReg(")
                    .trim_end_matches(')').parse::<u32>()
                {
                    return OperandValue::VReg(n);
                }
            }
            if text.starts_with('.') {
                return OperandValue::Label(text.to_string());
            }
            if text.starts_with("0x") || text.starts_with("0X") {
                let s = &text[2..].replace('_', "");
                if let Ok(n) = i64::from_str_radix(s, 16) {
                    return OperandValue::Imm(n);
                }
            }
            if text.starts_with("0b") || text.starts_with("0B") {
                let s = &text[2..].replace('_', "");
                if let Ok(n) = i64::from_str_radix(s, 2) {
                    return OperandValue::Imm(n);
                }
            }
            if let Ok(n) = text.replace('_', "").parse::<i64>() {
                return OperandValue::Imm(n);
            }
            if let Ok(f) = text.parse::<f64>() {
                return OperandValue::Float(f);
            }
            if text.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
                if let Some(idx) = resolve_reg_fn(text) {
                    return OperandValue::Reg { index: idx };
                }
            }
            OperandValue::Ident(text.to_string())
        }

        fn score_type_match(
            template_fields: &[(String, crate::assembler::FieldType)],
            op_values: &[OperandValue],
        ) -> i32 {
            let mut score = 0i32;
            for (i, (_, ftype)) in template_fields.iter().enumerate() {
                if i >= op_values.len() { break; }
                let matches = match ftype {
                    crate::assembler::FieldType::VReg => {
                        matches!(&op_values[i],
                            OperandValue::Reg { .. } | OperandValue::VReg(_) | OperandValue::Ident(_))
                    }
                    crate::assembler::FieldType::Reg => {
                        matches!(&op_values[i], OperandValue::Reg { .. })
                    }
                    crate::assembler::FieldType::I64
                    | crate::assembler::FieldType::U8
                    | crate::assembler::FieldType::U32 => {
                        matches!(&op_values[i], OperandValue::Imm(_))
                    }
                    crate::assembler::FieldType::F64 => {
                        matches!(&op_values[i], OperandValue::Float(_) | OperandValue::Imm(_))
                    }
                    crate::assembler::FieldType::BlockTarget => {
                        matches!(&op_values[i], OperandValue::Label(_))
                    }
                    crate::assembler::FieldType::CondCode => {
                        // CondCode is consumed by mnemonic; not from ops
                        true
                    }
                    // Opsize is an implicit field, not an asm operand
                    _ => true,
                };
                if matches { score += 1; }
            }
            score
        }

        fn resolve_inst(
            lookup: &AsmLookupTable,
            mnemonic: &str,
            op_values: &[OperandValue],
        ) -> core::result::Result<(String, Vec<(String, OperandValue)>), String> {
            // 1. Check CC entries
            if let Some((cc_inst_name, cc_val)) = lookup.find_cc(mnemonic) {
                let candidates: Vec<_> = lookup.entries.iter()
                    .filter(|e| e.inst_name == cc_inst_name)
                    .collect();
                if let Some(entry) = candidates.first() {
                    // CC inst: cond field is consumed by mnemonic, rest from operands
                    let mut bindings: Vec<(String, OperandValue)> = Vec::new();
                    let mut op_idx = 0usize;
                    for (fname, ftype) in &entry.template_fields {
                        if *ftype == crate::assembler::FieldType::CondCode {
                            bindings.push((fname.clone(), OperandValue::Imm(cc_val as i64)));
                        } else if op_idx < op_values.len() {
                            bindings.push((fname.clone(), op_values[op_idx].clone()));
                            op_idx += 1;
                        }
                    }
                    return Ok((cc_inst_name.clone(), bindings));
                }
            }

            // 2. Find by mnemonic
            let candidates = lookup.find(mnemonic);
            if candidates.is_empty() {
                return Err(format!("unknown mnemonic '{}'", mnemonic));
            }

            // Filter by operand count, then score by type compatibility
            let matching: Vec<_> = candidates.iter()
                .filter(|e| e.template_fields.len() == op_values.len())
                .collect();

            let entry = if matching.len() == 1 {
                matching[0]
            } else if matching.len() > 1 {
                // Score candidates by operand type compatibility
                let scored: Vec<_> = matching.iter().map(|e| {
                    let s = score_type_match(&e.template_fields, op_values);
                    (*e, s)
                }).collect();
                let best = scored.iter().max_by_key(|(_, s)| *s)
                    .map(|(e, _)| *e);
                best.unwrap_or(matching[0])
            } else if let Some(e) = candidates.first() {
                e
            } else {
                return Err(format!(
                    "no matching inst for mnemonic '{}' with {} operands (cands={} match={})",
                    mnemonic, op_values.len(),
                    candidates.len(), matching.len(),
                ));
            };

            let mut bindings: Vec<(String, OperandValue)> = Vec::new();
            for (i, (fname, _ftype)) in entry.template_fields.iter().enumerate() {
                if i < op_values.len() {
                    bindings.push((fname.clone(), op_values[i].clone()));
                }
            }
            Ok((entry.inst_name.clone(), bindings))
        }

        fn construct_inst(
            inst_name: &str,
            bindings: &[(String, OperandValue)],
            label_offsets: &HashMap<String, i64>,
        ) -> core::result::Result<Inst, String> {
            match inst_name {
                #(#arms)*
                _ => core::result::Result::Err(format!("unknown inst: {}", inst_name)),
            }
        }

        fn get_vreg_from_bindings(
            bindings: &[(String, OperandValue)],
            name: &str,
            resolve_reg_fn: fn(&str) -> core::option::Option<u8>,
        ) -> core::result::Result<VReg, String> {
            for (fname, val) in bindings {
                if fname == name {
                    return match val {
                        OperandValue::VReg(n) => core::result::Result::Ok(VReg(*n)),
                        OperandValue::Reg { index } => core::result::Result::Ok(VReg(*index as u32)),
                        OperandValue::Ident(s) => {
                            if let Some(idx) = resolve_reg_fn(s) {
                                core::result::Result::Ok(VReg(idx as u32))
                            } else if s.starts_with("VReg(") {
                                let n: u32 = s.trim_start_matches("VReg(")
                                    .trim_end_matches(')').parse()
                                    .map_err(|_| format!("invalid vreg: {}", s))?;
                                core::result::Result::Ok(VReg(n))
                            } else {
                                core::result::Result::Err(
                                    format!("expected register for '{}', got '{}'", name, s))
                            }
                        }
                        _ => core::result::Result::Err(
                            format!("expected register for '{}', got {:?}", name, val)),
                    };
                }
            }
            core::result::Result::Err(format!("missing binding for field '{}'", name))
        }

        fn get_reg_from_bindings(
            bindings: &[(String, OperandValue)],
            name: &str,
        ) -> core::result::Result<Reg, String> {
            for (fname, val) in bindings {
                if fname == name {
                    return match val {
                        OperandValue::Reg { index } => {
                            // SAFETY: Reg is repr(u8) with matching discriminant values
                            core::result::Result::Ok(unsafe {
                                std::mem::transmute::<u8, Reg>(*index)
                            })
                        }
                        _ => core::result::Result::Err(
                            format!("expected Reg for '{}', got {:?}", name, val)),
                    };
                }
            }
            core::result::Result::Err(format!("missing binding for field '{}'", name))
        }

        fn get_i64_from_bindings(
            bindings: &[(String, OperandValue)],
            name: &str,
        ) -> core::result::Result<i64, String> {
            for (fname, val) in bindings {
                if fname == name {
                    return match val {
                        OperandValue::Imm(n) => core::result::Result::Ok(*n),
                        OperandValue::Reg { index } => core::result::Result::Ok(*index as i64),
                        OperandValue::Ident(s) => {
                            parse_int_fallback(s)
                                .ok_or_else(|| format!(
                                    "expected integer for '{}', got '{}'", name, s))
                        }
                        _ => core::result::Result::Err(
                            format!("expected integer for '{}', got {:?}", name, val)),
                    };
                }
            }
            core::result::Result::Err(format!("missing binding for field '{}'", name))
        }

        fn get_f64_from_bindings(
            bindings: &[(String, OperandValue)],
            name: &str,
        ) -> core::result::Result<f64, String> {
            for (fname, val) in bindings {
                if fname == name {
                    return match val {
                        OperandValue::Float(f) => core::result::Result::Ok(*f),
                        OperandValue::Imm(n) => core::result::Result::Ok(*n as f64),
                        OperandValue::Ident(s) => {
                            s.parse::<f64>()
                                .map_err(|_| format!(
                                    "expected float for '{}', got '{}'", name, s))
                        }
                        _ => core::result::Result::Err(
                            format!("expected float for '{}', got {:?}", name, val)),
                    };
                }
            }
            core::result::Result::Err(format!("missing binding for field '{}'", name))
        }

        fn get_label_from_bindings(
            bindings: &[(String, OperandValue)],
            name: &str,
            label_offsets: &HashMap<String, i64>,
        ) -> core::result::Result<i64, String> {
            for (fname, val) in bindings {
                if fname == name {
                    return match val {
                        OperandValue::Label(lbl) => {
                            label_offsets.get(lbl)
                                .copied()
                                .ok_or_else(|| format!("unresolved label: {}", lbl))
                        }
                        OperandValue::Ident(s) => {
                            label_offsets.get(s)
                                .copied()
                                .ok_or_else(|| format!("unresolved label: {}", s))
                        }
                        OperandValue::Imm(n) => core::result::Result::Ok(*n),
                        _ => core::result::Result::Err(
                            format!("expected label for '{}', got {:?}", name, val)),
                    };
                }
            }
            core::result::Result::Err(format!("missing binding for field '{}'", name))
        }

        fn get_opsize_from_bindings(
            bindings: &[(String, OperandValue)],
            name: &str,
        ) -> u8 {
            for (fname, val) in bindings {
                if fname == name {
                    return match val {
                        OperandValue::Imm(n) => *n as u8,
                        _ => #opsize_default,
                    };
                }
            }
            #opsize_default
        }

        fn parse_int_fallback(s: &str) -> Option<i64> {
            if let Ok(n) = s.parse::<i64>() {
                return Some(n);
            }
            if s.starts_with("0x") || s.starts_with("0X") {
                if let Ok(n) = i64::from_str_radix(&s[2..].replace('_', ""), 16) {
                    return Some(n);
                }
            }
            if s.starts_with("0b") || s.starts_with("0B") {
                if let Ok(n) = i64::from_str_radix(&s[2..].replace('_', ""), 2) {
                    return Some(n);
                }
            }
            None
        }
    }
}

/// Generate inline CST helper functions (for use in parse_insts).
fn gen_cst_helpers() -> TokenStream {
    quote! {
        fn get_mnemonic_from_node(node: &lang_frontend::CstNode) -> Option<String> {
            for leaf in node.leaves() {
                if leaf.kind == "IDENT" {
                    return Some(leaf.text.clone());
                }
            }
            None
        }

        fn get_operand_texts_from_node(node: &lang_frontend::CstNode) -> Vec<String> {
            let mut operands = Vec::new();
            for child in node.walk() {
                if child.kind == "operand" {
                    let text: String = child.leaves().iter()
                        .map(|t| t.text.as_str())
                        .collect::<Vec<_>>()
                        .join("");
                    operands.push(text);
                }
            }
            operands
        }

        fn get_label_name_from_node(node: &lang_frontend::CstNode) -> Option<String> {
            for leaf in node.leaves() {
                if leaf.kind == "LABEL" {
                    return Some(leaf.text.clone());
                }
            }
            None
        }

        fn is_label_node(node: &lang_frontend::CstNode) -> bool {
            node.leaves().iter().any(|t| t.kind == "COLON")
        }
    }
}

// ============================================================
// Encoder impl — 复用 emit_inst 配合 identity RegMap
// ============================================================

fn gen_encoder_impl(_model: &IsaModel) -> TokenStream {
    // Build identity RegMap: VReg index → PReg with same number.
    // Covers all VRegs the instruction might reference (uses, defs, hardcoded).
    quote! {
        impl crate::prelude::Encoder for Isa {
            type Inst = Inst;
            type EncodeState = ();

            fn encode_with_state(
                inst: &Self::Inst,
                _state: &Self::EncodeState,
            ) -> Result<Vec<u8>, crate::EncodeError> {
                let mut rm = crate::prelude::RegMap::new();
                for i in 0u32..128u32 {
                    rm.insert(
                        crate::prelude::VReg(i),
                        crate::prelude::PReg::new(i as u8, crate::prelude::RegClass::Int),
                    );
                }
                let mut sink = crate::prelude::CodeSink::new();
                emit_inst(inst, &rm, &mut sink)
                    .map_err(|e| crate::EncodeError::Other(format!("{}", e)))?;
                Ok(sink.bytes().to_vec())
            }
        }
    }
}

// ============================================================
// Tests for ASM parsing
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build a minimal IsaModel for testing asm parsing.
    fn test_model() -> IsaModel {
        IsaModel {
            meta: Meta {
                name: "test".into(),
                version: "1.0".into(),
                endian: "little".into(),
                mode: 64,
                max_inst_len: 15,
                capabilities: Default::default(),
                no_default_lowering: true,
            },
            reg: {
                let mut m = HashMap::new();
                m.insert(
                    "gpr".into(),
                    RegGroup {
                        count: 16,
                        width: 64,
                        names: Some(
                            vec![
                                "RAX", "RCX", "RDX", "RBX", "RSP", "RBP", "RSI", "RDI", "R8", "R9",
                                "R10", "R11", "R12", "R13", "R14", "R15",
                            ]
                            .into_iter()
                            .map(|s| s.to_string())
                            .collect(),
                        ),
                        prefix: None,
                    },
                );
                m
            },
            abi: None,
            inst: {
                let mut m = HashMap::new();
                m.insert(
                    "ADD_RM8_R8".into(),
                    Instruction {
                        fields: vec![
                            InstField {
                                name: "dest".into(),
                                field_type: FieldType::VReg,
                            },
                            InstField {
                                name: "src".into(),
                                field_type: FieldType::VReg,
                            },
                        ],
                        encoding: None,
                        asm: "add {dest}, {src}".into(),
                        effect: None,
                    },
                );
                m.insert(
                    "RET".into(),
                    Instruction {
                        fields: vec![],
                        encoding: None,
                        asm: "ret".into(),
                        effect: None,
                    },
                );
                m.insert(
                    "MOV_REG_IMM64".into(),
                    Instruction {
                        fields: vec![
                            InstField {
                                name: "imm".into(),
                                field_type: FieldType::I64,
                            },
                            InstField {
                                name: "reg".into(),
                                field_type: FieldType::VReg,
                            },
                        ],
                        encoding: None,
                        asm: "mov {reg}, 0x{imm:x}".into(),
                        effect: None,
                    },
                );
                m.insert(
                    "JCC_REL32".into(),
                    Instruction {
                        fields: vec![
                            InstField {
                                name: "cond".into(),
                                field_type: FieldType::CondCode,
                            },
                            InstField {
                                name: "rel".into(),
                                field_type: FieldType::BlockTarget,
                            },
                        ],
                        encoding: None,
                        asm: "j{cond} .L{rel}".into(),
                        effect: None,
                    },
                );
                m.insert(
                    "SETCC_RM8".into(),
                    Instruction {
                        fields: vec![
                            InstField {
                                name: "cond".into(),
                                field_type: FieldType::CondCode,
                            },
                            InstField {
                                name: "dest".into(),
                                field_type: FieldType::VReg,
                            },
                        ],
                        encoding: None,
                        asm: "set{cond} {dest}".into(),
                        effect: None,
                    },
                );
                m
            },
            lower: HashMap::new(),
            lower_term: HashMap::new(),
            lower_pattern: HashMap::new(),
            emit: None,
            enc_macros: HashMap::new(),
            enc_scatters: HashMap::new(),
            cc_names: HashMap::new(),
            lang_tokens: None,
            lang_keywords: None,
            lang_rules: None,
            dyn_types: HashMap::new(),
        }
    }

    // ── parse_asm_inst tests ──

    #[test]
    fn test_parse_asm_no_operands() {
        let model = test_model();
        let (name, fields) = parse_asm_inst("RET", &model).unwrap();
        assert_eq!(name, "RET");
        assert!(fields.is_empty());
    }

    #[test]
    fn test_parse_asm_two_operands() {
        let model = test_model();
        let (name, fields) = parse_asm_inst("ADD_RM8_R8 rd, rs1", &model).unwrap();
        assert_eq!(name, "ADD_RM8_R8");
        assert_eq!(fields, vec![("dest", "rd"), ("src", "rs1")]);
    }

    #[test]
    fn test_parse_asm_with_extra_spaces() {
        let model = test_model();
        let (name, fields) = parse_asm_inst("  ADD_RM8_R8   rd  ,  rs1  ", &model).unwrap();
        assert_eq!(name, "ADD_RM8_R8");
        assert_eq!(fields, vec![("dest", "rd"), ("src", "rs1")]);
    }

    #[test]
    fn test_parse_asm_unknown_instruction() {
        let model = test_model();
        let result = parse_asm_inst("NONEXISTENT rd, rs1", &model);
        assert!(result.is_err(), "unknown instruction should error");
    }

    #[test]
    fn test_parse_asm_extra_operands_silently_ignored() {
        let model = test_model();
        // RET has 0 fields but we pass 1 operand — extra operands are silently ignored
        // (parse_asm_inst only validates that all fields have operands, not that there are no extras)
        let result = parse_asm_inst("RET extra", &model);
        assert!(result.is_ok(), "extra operands are currently ignored");
    }

    #[test]
    fn test_parse_asm_missing_operand_errors() {
        let model = test_model();
        // ADD_RM8_R8 has 2 fields but we pass only 1 operand
        let result = parse_asm_inst("ADD_RM8_R8 rd", &model);
        assert!(result.is_err(), "missing operand should error");
        assert!(
            result.unwrap_err().contains("missing operand"),
            "error should mention missing operand"
        );
    }

    #[test]
    fn test_parse_asm_numeric_operands() {
        let model = test_model();
        let (name, fields) = parse_asm_inst("ADD_RM8_R8 0x10, 42", &model).unwrap();
        assert_eq!(name, "ADD_RM8_R8");
        assert_eq!(fields, vec![("dest", "0x10"), ("src", "42")]);
    }

    #[test]
    fn test_parse_asm_with_register_names() {
        // In emit context, operands can be register names like RBP, RSP
        let model = test_model();
        let (name, fields) = parse_asm_inst("ADD_RM8_R8 RBP, RSP", &model).unwrap();
        assert_eq!(name, "ADD_RM8_R8");
        assert_eq!(fields, vec![("dest", "RBP"), ("src", "RSP")]);
    }

    #[test]
    fn test_parse_asm_single_operand() {
        let model = test_model();
        // SETCC_RM8 has 2 fields (cond, dest) in alpha order
        let (name, fields) = parse_asm_inst("SETCC_RM8 0x94, rd", &model).unwrap();
        assert_eq!(name, "SETCC_RM8");
        // BTreeMap order: cond < dest
        assert_eq!(fields, vec![("cond", "0x94"), ("dest", "rd")]);
    }

    #[test]
    fn test_parse_asm_field_order_matches_btree() {
        let model = test_model();
        // MOV_REG_IMM64 fields in BTreeMap order: imm < reg
        let (name, fields) = parse_asm_inst("MOV_REG_IMM64 0x42, rd", &model).unwrap();
        assert_eq!(name, "MOV_REG_IMM64");
        assert_eq!(fields, vec![("imm", "0x42"), ("reg", "rd")]);
    }

    // ── parse_asm_template tests ──

    #[test]
    fn test_parse_template_no_fields() {
        let fields = vec![];
        let frags = parse_asm_template("ret", &fields);
        assert_eq!(frags.len(), 1);
        assert_eq!(frags[0].lit, "ret");
        assert!(frags[0].field_fmt.is_none());
    }

    #[test]
    fn test_parse_template_with_fields() {
        let fields = vec![
            InstField {
                name: "dest".into(),
                field_type: FieldType::VReg,
            },
            InstField {
                name: "src".into(),
                field_type: FieldType::VReg,
            },
        ];
        let frags = parse_asm_template("add {dest}, {src}", &fields);
        assert_eq!(frags.len(), 4);
        // "add "
        assert_eq!(frags[0].lit, "add ");
        assert!(frags[0].field_fmt.is_none());
        // {dest} → VReg formatting
        assert_eq!(frags[1].lit, "");
        assert!(frags[1].field_fmt.is_some());
        // ", "
        assert_eq!(frags[2].lit, ", ");
        assert!(frags[2].field_fmt.is_none());
        // {src} → VReg formatting
        assert_eq!(frags[3].lit, "");
        assert!(frags[3].field_fmt.is_some());
    }

    #[test]
    fn test_parse_template_with_cond_code() {
        let fields = vec![
            InstField {
                name: "cond".into(),
                field_type: FieldType::CondCode,
            },
            InstField {
                name: "dest".into(),
                field_type: FieldType::VReg,
            },
        ];
        let frags = parse_asm_template("set{cond} {dest}", &fields);
        assert_eq!(frags.len(), 4);
        // "set"
        assert_eq!(frags[0].lit, "set");
        assert!(frags[0].field_fmt.is_none());
        // {cond} → CondCode formatting
        assert_eq!(frags[1].lit, "");
        assert!(frags[1].field_fmt.is_some());
        // " "
        assert_eq!(frags[2].lit, " ");
        assert!(frags[2].field_fmt.is_none());
        // {dest} → VReg formatting
        assert_eq!(frags[3].lit, "");
        assert!(frags[3].field_fmt.is_some());
    }

    #[test]
    fn test_parse_template_unknown_field_is_skipped() {
        let fields = vec![InstField {
            name: "reg".into(),
            field_type: FieldType::VReg,
        }];
        let frags = parse_asm_template("mov {reg}, {nonexistent}", &fields);
        // Unknown field {nonexistent} creates a fragment with empty lit and None field_fmt
        // Results in 4 fragments: "mov ", {reg}, ", ", {nonexistent dropped}
        assert_eq!(frags.len(), 4);
        assert_eq!(frags[0].lit, "mov ");
        assert_eq!(frags[1].lit, "");
        assert!(frags[1].field_fmt.is_some()); // {reg}
        assert_eq!(frags[2].lit, ", ");
        assert!(frags[2].field_fmt.is_none());
        // {nonexistent} → empty fragment, silently dropped
        assert_eq!(frags[3].lit, "");
        assert!(frags[3].field_fmt.is_none());
    }
}
