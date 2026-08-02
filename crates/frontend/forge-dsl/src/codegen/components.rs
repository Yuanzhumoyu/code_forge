//! Codegen v19 — generates componentized TargetMachine + trait impls.
//!
//! Generates each ISA module with simple, module-scoped names:
//! - `TargetMachine` (not `X8664TargetMachine`)
//! - `RegInfo` (not `X8664RegInfo`)
//! - `ABI`, `Lowering`, `Encoder`, `FrameLowering`, `IsaInfo`
//!
//! The module name (e.g., `x86_64`) provides the namespace.

use crate::model::*;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

/// Generate all v19 components: TargetRegInfo, TargetABI, TargetLowering,
/// TargetEncoder, TargetFrameLowering, and TargetMachine.
pub fn gen_v19_components(model: &IsaModel) -> TokenStream {
    let reg_info = gen_target_reg_info(model);
    let abi = gen_target_abi(model);
    let lowering = gen_target_lowering(model);
    let encoder = gen_target_encoder(model);
    let frame_lowering = gen_target_frame_lowering(model);
    let isa_info = gen_v19_isa_info(model);
    let disasm = gen_disasm_impl(model);
    let asm = gen_asm_impl(model);
    let target_machine = gen_target_machine_struct(model);

    quote! {
        #reg_info
        #abi
        #lowering
        #encoder
        #frame_lowering
        #isa_info
        #disasm
        #asm
        #target_machine
    }
}

// ============================================================
// Helper: get GPR group name
// ============================================================

fn gpr_group_name(model: &IsaModel) -> &str {
    if model.reg.contains_key("gpr64") {
        "gpr64"
    } else {
        "gpr"
    }
}

/// Resolve a physical register name (e.g. "XMM0", "RAX") to its (is_float, index) pair.
pub(crate) fn resolve_reg_index(model: &IsaModel, reg_name: &str) -> (bool, u8) {
    for (group_name, group) in &model.reg {
        let is_float = group_name.contains("xmm") || group_name.contains("float");
        // Named registers: RAX, RCX, R10, R11, etc.
        if let Some(names) = &group.names
            && let Some(pos) = names.iter().position(|n| n.eq_ignore_ascii_case(reg_name))
        {
            return (is_float, pos as u8);
        }
        // Prefix-based: XMM0, XMM1, XMM15, etc.
        if let Some(ref prefix) = group.prefix
            && let Some(stripped) = reg_name.to_uppercase().strip_prefix(&prefix.to_uppercase())
            && let Ok(n) = stripped.parse::<u32>()
        {
            return (is_float, n as u8);
        }
    }
    (false, 0)
}

// ============================================================
// IsaInfo (v19 version — instance methods)
// ============================================================

fn gen_v19_isa_info(model: &IsaModel) -> TokenStream {
    let meta = &model.meta;
    let name_str = &meta.name;
    let version_str = &meta.version;
    let mode_val = meta.mode;
    let cap = &meta.capabilities;
    let vl = cap.variable_length;
    let pl = cap.prefix_layers;
    let n_insts = model.inst.len();
    let mask_regs = cap.mask_registers;
    let bcast = cap.broadcast;
    let round = cap.rounding_mode;
    let max_inst_len = meta.max_inst_len;
    let min_inst_len: u8 = if vl { 1 } else { 4 };
    let endian = if meta.endian == "big" {
        quote! { forge_ir::Endianness::Big }
    } else {
        quote! { forge_ir::Endianness::Little }
    };

    // Generate static SIMD widths slice from model
    let simd_widths = &cap.simd_widths;

    quote! {
        pub struct IsaInfo;

        impl crate::machine::isa_info::IsaInfo for IsaInfo {
            fn name(&self) -> &'static str { #name_str }
            fn version(&self) -> &'static str { #version_str }
            fn address_size(&self) -> u8 { #mode_val }
            fn endianness(&self) -> forge_ir::Endianness { #endian }
            fn capabilities(&self) -> crate::machine::isa_info::IsaCapabilities {
                crate::machine::isa_info::IsaCapabilities {
                    variable_length: #vl,
                    fixed_inst_size: 0,
                    prefix_layers: #pl,
                    addressing_modes: &[],
                    simd_widths: &[#(#simd_widths),*],
                    mask_registers: #mask_regs,
                    broadcast: #bcast,
                    rounding_mode: #round,
                    endianness: #endian,
                    min_inst_len: #min_inst_len,
                    max_inst_len: #max_inst_len,
                }
            }
            fn num_instructions(&self) -> usize { #n_insts }
        }
    }
}

// ============================================================
// TargetRegInfo
// ============================================================

fn gen_target_reg_info(model: &IsaModel) -> TokenStream {
    let gpr = model.reg.get(gpr_group_name(model));
    let ngpr = gpr.map(|g| g.count as u8).unwrap_or(16);
    let nfpr = model
        .reg
        .get("xmm")
        .or_else(|| model.reg.get("float"))
        .map(|g| g.count as u8)
        .unwrap_or(16);

    // ── 从 [reg_classes] 读取 allocatable 寄存器列表 ──
    // 如果定义了则使用；否则回退到全量 range
    let gpr_allocatable: Vec<u8> = model
        .reg_classes
        .get("GPR")
        .map(|rc| rc.allocatable.clone())
        .unwrap_or_else(|| (0..ngpr).collect());
    let fpr_allocatable: Vec<u8> = model
        .reg_classes
        .get("FPR")
        .map(|rc| rc.allocatable.clone())
        .unwrap_or_else(|| (0..nfpr).collect());

    // ── 从 [reg_classes] 读取寄存器宽度 ──
    let gpr_width: u8 = model.reg_classes.get("GPR").map(|rc| rc.width).unwrap_or(8);
    let fpr_width: u8 = model.reg_classes.get("FPR").map(|rc| rc.width).unwrap_or(8);

    // SP register
    let sp_tokens = if let Some(ref abi) = model.abi {
        if let Some(ref frame) = abi.frame {
            let sp_ident = format_ident!("{}", frame.sp);
            quote! { forge_ir::FrameAccess::Register(Reg::#sp_ident) }
        } else {
            quote! { forge_ir::FrameAccess::Register(Reg::from_index(4, forge_ir::RegClass::Int)) }
        }
    } else {
        quote! { forge_ir::FrameAccess::Register(Reg::from_index(4, forge_ir::RegClass::Int)) }
    };

    // FP register
    let fp_tokens = if let Some(ref abi) = model.abi {
        if let Some(ref frame) = abi.frame {
            if let Some(ref fp) = frame.fp {
                let fp_ident = format_ident!("{fp}");
                quote! { Some(Reg::#fp_ident) }
            } else {
                quote! { None::<Reg> }
            }
        } else {
            quote! { None::<Reg> }
        }
    } else {
        quote! { None::<Reg> }
    };

    // Callee-saved register indices
    let callee_saved: Vec<TokenStream> = model
        .abi
        .as_ref()
        .map(|a| {
            a.callee_saved
                .gpr
                .iter()
                .chain(a.callee_saved.xmm.iter())
                .map(|n| {
                    let i = format_ident!("{n}");
                    quote! { Reg::#i as u8 }
                })
                .collect()
        })
        .unwrap_or_default();

    // Scratch registers — use explicit [abi.scratch] PReg numbers when available.
    // These are the physical register numbers reserved for spill loads/stores.
    // Fallback: derive from callee-saved inversion (exclude saved regs + SP).

    // 从 model 解析 SP 寄存器索引（用于 fallback 分支，替代硬编码 4）
    let sp_index_for_scratch: u8 = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .map(|f| {
            let (_, idx) = resolve_reg_index(model, &f.sp);
            idx
        })
        .unwrap_or(4); // 最终回退到 4（x86 RSP）

    let scratch_regs: Vec<u8> = {
        let explicit: Option<Vec<u8>> = model.abi.as_ref().and_then(|a| {
            if a.scratch.is_empty() {
                None
            } else {
                Some(a.scratch.values().map(|&v| v as u8).collect())
            }
        });
        if let Some(regs) = explicit {
            regs
        } else {
            let saved: Vec<u8> = model
                .abi
                .as_ref()
                .map(|a| {
                    a.callee_saved
                        .gpr
                        .iter()
                        .map(|n| {
                            n.trim_start_matches(|c: char| !c.is_ascii_digit())
                                .parse::<u8>()
                                .unwrap_or(0)
                        })
                        .collect()
                })
                .unwrap_or_default();
            (0..ngpr)
                .filter(|n| !saved.contains(n) && *n != sp_index_for_scratch)
                .collect()
        }
    };

    // Precolored VReg→PReg mappings from [abi.precolor] (e.g., VReg0=RAX, VReg100=XMM0)
    let precolored_vregs: Vec<TokenStream> =
        model
            .abi
            .as_ref()
            .map(|a| {
                a.precolor.iter().map(|(vreg_name, reg_name)| {
                // Parse "VReg100" → 100
                let vreg_idx: u32 = vreg_name
                    .trim_start_matches(|c: char| !c.is_ascii_digit())
                    .parse().unwrap_or(0);
                let (is_float, reg_idx) = resolve_reg_index(model, reg_name);
                let class_tok = if is_float {
                    quote! { forge_ir::RegClass::Float }
                } else {
                    quote! { forge_ir::RegClass::Int }
                };
                quote! {
                    (crate::prelude::XReg::new(#vreg_idx, #class_tok, 8), forge_ir::PReg::new(#reg_idx, #class_tok))
                }
            }).collect()
            })
            .unwrap_or_default();

    quote! {
        pub struct RegInfo;

        impl crate::machine::reg_info::TargetRegInfo for RegInfo {
            type Reg = Reg;

            fn num_gp_regs(&self) -> u8 { #ngpr }
            fn num_fp_regs(&self) -> u8 { #nfpr }

            fn reg_class_width(&self, class: forge_ir::RegClass) -> u8 {
                match class {
                    forge_ir::RegClass::GPR => #gpr_width,
                    forge_ir::RegClass::FPR => #fpr_width,
                    _ => class.default_width(),
                }
            }

            fn sp_reg(&self) -> forge_ir::FrameAccess<Self::Reg> { #sp_tokens }
            fn fp_reg(&self) -> Option<Self::Reg> { #fp_tokens }

            fn allocatable_gp_order(&self) -> Vec<u8> {
                vec![#(#gpr_allocatable),*]
            }
            fn allocatable_fp_order(&self) -> Vec<u8> {
                vec![#(#fpr_allocatable),*]
            }
            fn scratch_regs(&self) -> Vec<u8> {
                vec![#(#scratch_regs),*]
            }
            fn callee_saved(&self) -> Vec<u8> {
                vec![#(#callee_saved),*]
            }
            fn precolored_xregs(&self) -> Vec<(crate::prelude::XReg, forge_ir::PReg)> {
                vec![#(#precolored_vregs),*]
            }
        }
    }
}

// ============================================================
// TargetABI
// ============================================================

fn gen_target_abi(model: &IsaModel) -> TokenStream {
    let stack_align = model.abi.as_ref().map(|a| a.stack_align).unwrap_or(16);

    let red_zone = match model.abi.as_ref().and_then(|a| a.red_zone) {
        Some(n) => quote! { Some(#n) },
        None => quote! { None::<u32> },
    };

    // arg_regs
    let arg_regs: Vec<TokenStream> = model
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
    let ret_regs: Vec<TokenStream> = model
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

    quote! {
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
            fn red_zone(&self) -> Option<u32> { #red_zone }
        }
    }
}

// ============================================================
// TargetLowering — delegates to existing lower_impl functions
// ============================================================

fn gen_target_lowering(model: &IsaModel) -> TokenStream {
    let _ = model;
    quote! {
        pub struct Lowering;

        impl crate::machine::lowering::TargetLowering for Lowering {
            type Inst = Inst;

            fn lower_inst(
                &self,
                op: &crate::prelude::Opcode,
                args: &[crate::prelude::XReg],
                results: &[crate::prelude::XReg],
                ctx: &mut crate::prelude::LowerCtx,
            ) -> Result<crate::prelude::InstPacket<Self::Inst>, crate::prelude::CompileError> {
                lower_impl(op, args, results, ctx)
            }

            fn lower_terminator(
                &self,
                term: &crate::prelude::Terminator,
                value_to_xreg: &std::collections::HashMap<crate::prelude::Value, crate::prelude::XReg>,
                _block_to_vblock: &std::collections::HashMap<crate::prelude::Block, crate::prelude::VBlockId>,
                ctx: &mut crate::prelude::LowerCtx,
            ) -> Result<crate::prelude::InstPacket<Self::Inst>, crate::prelude::CompileError> {
                lower_terminator_impl(term, value_to_xreg, ctx)
            }

            fn lower_pattern(
                &self,
                pattern_name: &str,
                args: &[crate::prelude::XReg],
                results: &[crate::prelude::XReg],
                ctx: &mut crate::prelude::LowerCtx,
            ) -> Result<crate::prelude::InstPacket<Self::Inst>, crate::prelude::CompileError> {
                lower_pattern_impl(pattern_name, args, results, ctx)
            }
        }
    }
}

// ============================================================
// TargetEncoder — delegates to existing emit_inst function
// ============================================================

fn gen_target_encoder(model: &IsaModel) -> TokenStream {
    let _ = model;
    quote! {
        pub struct Encoder;

        impl crate::machine::encoder::TargetEncoder for Encoder {
            type Inst = Inst;

            fn encode(
                &self,
                inst: &Self::Inst,
                reg_map: &crate::AllocResult,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::EncodeError> {
                emit_inst(inst, reg_map, sink)
                    .map_err(|e| crate::EncodeError::Other(format!("{e}")))
            }

            fn encoded_size(&self, inst: &Self::Inst) -> Result<usize, crate::EncodeError> {
                // Build dummy AllocResult (AllocResult) from instruction's uses/defs plus safety net.
                let mut rm = crate::AllocResult::default();
                for &vreg in inst.uses().iter().chain(inst.defs().iter()) {
                    let xreg = crate::prelude::XReg::new(vreg as u32, crate::prelude::RegClass::GPR, 8);
                    if !rm.assignments.contains_key(&xreg) {
                        rm.insert(xreg, forge_ir::PReg::new(vreg % 16, forge_ir::RegClass::GPR));
                    }
                }
                for n in 0..16u32 {
                    let v = crate::prelude::XReg::new(n as u32, crate::prelude::RegClass::GPR, 8);
                    if !rm.assignments.contains_key(&v) {
                        rm.insert(v, forge_ir::PReg::new((n % 16) as u8, forge_ir::RegClass::GPR));
                    }
                }
                self.encode_to_bytes(inst, &rm).map(|v| v.len())
            }
        }
    }
}

// ============================================================
// TargetFrameLowering — delegates to emit_prologue/emit_epilogue_impl
// ============================================================

fn gen_target_frame_lowering(model: &IsaModel) -> TokenStream {
    let needs_epilogue_label = !model.meta.no_epilogue_label;
    let is_variable = model.meta.capabilities.variable_length;

    // ── 从 [spill] section 解析 spill 模板数据 ──
    // 使用 [spill.GPR] 或 [spill] 中的第一个条目
    let gpr_spill = model.spill.get("GPR");
    let fpr_spill = model.spill.get("FPR");

    // 解析 GPR spill 的 base 寄存器索引
    let gpr_base_reg: u8 = gpr_spill
        .map(|s| {
            let (_, idx) = resolve_reg_index(model, &s.load.base);
            idx
        })
        .or_else(|| {
            // 回退：从 abi.frame.fp 解析
            model
                .abi
                .as_ref()
                .and_then(|a| a.frame.as_ref())
                .and_then(|f| f.fp.as_ref())
                .map(|fp| {
                    let (_, idx) = resolve_reg_index(model, fp);
                    idx
                })
        })
        .unwrap_or(5); // 最后回退到 x86 RBP=5

    // 解析 FPR spill 的 base 寄存器索引（如果存在）
    let fpr_base_reg: u8 = fpr_spill
        .map(|s| {
            let (_, idx) = resolve_reg_index(model, &s.load.base);
            idx
        })
        .unwrap_or(gpr_base_reg); // 回退到 GPR base

    // 解析 store 路径的 base 寄存器（从 s.store.base，回退到 load.base）
    let gpr_store_base_reg: u8 = gpr_spill
        .map(|s| {
            let (_, idx) = resolve_reg_index(model, &s.store.base);
            idx
        })
        .unwrap_or(gpr_base_reg);
    let fpr_store_base_reg: u8 = fpr_spill
        .map(|s| {
            let (_, idx) = resolve_reg_index(model, &s.store.base);
            idx
        })
        .unwrap_or(fpr_base_reg);

    // 判断 GPR spill 是否为浮点 MOVSD（从 load 和 store inst 名称推断）
    let gpr_is_movsd = gpr_spill
        .map(|s| {
            s.load.inst.contains("MOVSD")
                || s.load.inst.contains("movsd")
                || s.store.inst.contains("MOVSD")
                || s.store.inst.contains("movsd")
        })
        .unwrap_or(false);

    // 收集已定义的 spill 类名称（用于非 x86 错误消息）
    let spill_class_names: Vec<&str> = model.spill.keys().map(|s| s.as_str()).collect();
    let spill_err_detail = if spill_class_names.is_empty() {
        "No [spill] section found in ISA TOML. Add e.g. [spill.GPR] with load/store templates."
            .to_string()
    } else {
        format!(
            "emit_spill_load not implemented for this ISA. Found [spill] classes: [{}]. Implement in gen_target_frame_lowering.",
            spill_class_names.join(", ")
        )
    };

    // Generate spill/jump overrides: x86 gets the real implementations;
    // other ISAs get unimplemented!() stubs.
    let (spill_load_body, spill_store_body, epilogue_jump_body) = if is_variable {
        // x86: 使用 [spill] 数据生成宽度感知 spill 编码
        let spill_load = quote! {
            fn emit_spill_load(&self, dst_reg: u8, offset: i32, _width: u8, sink: &mut crate::CodeSink) -> Result<(), crate::prelude::CompileError> {
                const GPR_BASE: u8 = #gpr_base_reg;
                const FPR_BASE: u8 = #fpr_base_reg;
                // 宽度 > 8 使用 MOVUPS (FPR)；否则使用 MOV (GPR)
                let is_fp = _width > 8 || #gpr_is_movsd;
                let base = if is_fp { FPR_BASE } else { GPR_BASE };
                if is_fp {
                    // MOVUPS xmm, [base+disp]: 0x0F 0x10 /r
                    let mut rex = 0x40u8;
                    if (dst_reg & 0x8) != 0 { rex |= 0x04; }
                    if (base & 0x8) != 0 { rex |= 0x01; }
                    if rex != 0x40 { sink.put1(rex); }
                    sink.put1(0x0F);
                    sink.put1(0x10);
                    if (-128i32..=127i32).contains(&offset) {
                        sink.put1(((1u8) << 6) | ((dst_reg & 0x7) << 3) | (base & 0x7));
                        sink.put1(offset as u8);
                    } else {
                        sink.put1(((2u8) << 6) | ((dst_reg & 0x7) << 3) | (base & 0x7));
                        sink.put4(offset as u32);
                    }
                } else {
                    // MOV r64, [base+disp]: REX.W + 0x8B /r
                    let mut rex = 0x48u8;
                    if (dst_reg & 0x8) != 0 { rex |= 0x04; }
                    if (base & 0x8) != 0 { rex |= 0x01; }
                    sink.put1(rex);
                    sink.put1(0x8B);
                    if (-128i32..=127i32).contains(&offset) {
                        sink.put1(((1u8) << 6) | ((dst_reg & 0x7) << 3) | (base & 0x7));
                        sink.put1(offset as u8);
                    } else {
                        sink.put1(((2u8) << 6) | ((dst_reg & 0x7) << 3) | (base & 0x7));
                        sink.put4(offset as u32);
                    }
                }
                Ok(())
            }
        };
        let spill_store = quote! {
            fn emit_spill_store(&self, src_reg: u8, offset: i32, _width: u8, sink: &mut crate::CodeSink) -> Result<(), crate::prelude::CompileError> {
                const GPR_STORE_BASE: u8 = #gpr_store_base_reg;
                const FPR_STORE_BASE: u8 = #fpr_store_base_reg;
                let is_fp = _width > 8 || #gpr_is_movsd;
                let store_base = if is_fp { FPR_STORE_BASE } else { GPR_STORE_BASE };
                if is_fp {
                    // MOVUPS [base+disp], xmm: 0x0F 0x11 /r
                    let mut rex = 0x40u8;
                    if (src_reg & 0x8) != 0 { rex |= 0x04; }
                    if (store_base & 0x8) != 0 { rex |= 0x01; }
                    if rex != 0x40 { sink.put1(rex); }
                    sink.put1(0x0F);
                    sink.put1(0x11);
                    if (-128i32..=127i32).contains(&offset) {
                        sink.put1(((1u8) << 6) | ((src_reg & 0x7) << 3) | (store_base & 0x7));
                        sink.put1(offset as u8);
                    } else {
                        sink.put1(((2u8) << 6) | ((src_reg & 0x7) << 3) | (store_base & 0x7));
                        sink.put4(offset as u32);
                    }
                } else {
                    // MOV [base+disp], r64: REX.W + 0x89 /r
                    let mut rex = 0x48u8;
                    if (src_reg & 0x8) != 0 { rex |= 0x04; }
                    if (store_base & 0x8) != 0 { rex |= 0x01; }
                    sink.put1(rex);
                    sink.put1(0x89);
                    if (-128i32..=127i32).contains(&offset) {
                        sink.put1(((1u8) << 6) | ((src_reg & 0x7) << 3) | (store_base & 0x7));
                        sink.put1(offset as u8);
                    } else {
                        sink.put1(((2u8) << 6) | ((src_reg & 0x7) << 3) | (store_base & 0x7));
                        sink.put4(offset as u32);
                    }
                }
                Ok(())
            }
        };
        let epilogue_jump = quote! {
            fn emit_epilogue_jump(
                &self,
                _encoder: &std::sync::Arc<dyn crate::machine::encoder::TargetEncoder<Inst = Self::Inst>>,
                _reg_map: &crate::AllocResult,
                epilogue_block: forge_ir::Block,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::prelude::CompileError> {
                sink.put1(0xE9u8);
                let fixup = sink.offset();
                sink.put4(0u32);
                sink.use_label_at(fixup, epilogue_block, crate::RelocKind::REL4);
                Ok(())
            }
        };
        (spill_load, spill_store, epilogue_jump)
    } else {
        // 非 x86: 使用 [spill] 数据生成信息丰富的错误消息
        let spill_err = &spill_err_detail;
        let spill_load = quote! {
            fn emit_spill_load(&self, _dst_reg: u8, _offset: i32, _width: u8, _sink: &mut crate::CodeSink) -> Result<(), crate::prelude::CompileError> {
                Err(crate::prelude::CompileError::Unimplemented(#spill_err.into()))
            }
        };
        let spill_store = quote! {
            fn emit_spill_store(&self, _src_reg: u8, _offset: i32, _width: u8, _sink: &mut crate::CodeSink) -> Result<(), crate::prelude::CompileError> {
                Err(crate::prelude::CompileError::Unimplemented(#spill_err.into()))
            }
        };
        let epilogue_jump = quote! {
            fn emit_epilogue_jump(
                &self,
                _encoder: &std::sync::Arc<dyn crate::machine::encoder::TargetEncoder<Inst = Self::Inst>>,
                _reg_map: &crate::AllocResult,
                _epilogue_block: forge_ir::Block,
                _sink: &mut crate::CodeSink,
            ) -> Result<(), crate::prelude::CompileError> {
                Ok(())
            }
        };
        (spill_load, spill_store, epilogue_jump)
    };

    quote! {
        pub struct FrameLowering;

        impl crate::machine::frame::TargetFrameLowering for FrameLowering {
            type Inst = Inst;

            fn needs_epilogue_label(&self) -> bool { #needs_epilogue_label }

            fn emit_prologue(
                &self,
                frame_size: u32,
                reg_map: &crate::AllocResult,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::prelude::CompileError> {
                emit_prologue_impl(frame_size, reg_map, sink)
            }

            fn emit_epilogue(
                &self,
                frame_size: u32,
                reg_map: &crate::AllocResult,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::prelude::CompileError> {
                emit_epilogue_impl(frame_size, reg_map, sink)
            }

            #spill_load_body
            #spill_store_body
            #epilogue_jump_body
        }
    }
}

// ============================================================
// TargetMachine — composes all components + ensure_registered()
// ============================================================

// ============================================================
// TargetDisassembler (optional)
// ============================================================

fn gen_disasm_impl(model: &IsaModel) -> TokenStream {
    let mut arms: Vec<TokenStream> = Vec::new();

    for (inst_name, inst) in &model.inst {
        let vn = super::pascal_ident(inst_name);
        let asm_template = &inst.asm;

        let field_idents: Vec<_> = inst
            .fields
            .iter()
            .map(|f| quote::format_ident!("{}", f.name))
            .collect();

        if field_idents.is_empty() {
            arms.push(quote::quote! {
                Inst::#vn => #asm_template.to_string()
            });
        } else {
            // Build a format! call using the asm template.
            // Placeholders like {dest}, {imm:x} are replaced with format args.
            // We map :x suffix to lowercase hex, and default to Debug formatting.
            let mut fmt_str = String::new();
            let mut format_args: Vec<proc_macro2::TokenStream> = Vec::new();
            let mut i = 0;
            let template_chars: Vec<char> = asm_template.chars().collect();
            let mut pos = 0;
            while pos < template_chars.len() {
                if template_chars[pos] == '{' {
                    let end = template_chars[pos..]
                        .iter()
                        .position(|&c| c == '}')
                        .unwrap();
                    let placeholder: String = template_chars[pos + 1..pos + end].iter().collect();
                    let (field_name, fmt_spec) = if let Some(colon_pos) = placeholder.find(':') {
                        (&placeholder[..colon_pos], &placeholder[colon_pos + 1..])
                    } else {
                        (&placeholder[..], "")
                    };
                    // Find field index
                    if let Some(field_idx) = inst.fields.iter().position(|f| f.name == field_name) {
                        fmt_str.push('{');
                        fmt_str.push_str(&i.to_string());
                        fmt_str.push('}');
                        let f_ident = &field_idents[field_idx];
                        match fmt_spec {
                            "x" => format_args.push(quote::quote! { format!("{:x}", #f_ident) }),
                            _ => format_args.push(quote::quote! { format!("{:?}", #f_ident) }),
                        }
                        i += 1;
                    }
                    pos += end + 1;
                } else {
                    fmt_str.push(template_chars[pos]);
                    pos += 1;
                }
            }
            let format_str = fmt_str;
            arms.push(quote::quote! {
                Inst::#vn { #(#field_idents),*, .. } => format!(#format_str, #(#format_args),*)
            });
        }
    }

    arms.push(quote::quote! {
        Inst::Unknown(_) => "<unknown>".to_string()
    });

    quote::quote! {
        pub struct Disassembler;

        impl crate::machine::disasm::TargetDisassembler for Disassembler {
            type Inst = Inst;

            fn disassemble(&self, inst: &Self::Inst) -> String {
                match inst {
                    #(#arms),*
                }
            }
        }
    }
}

fn gen_asm_impl(model: &IsaModel) -> TokenStream {
    let _ = model;
    quote::quote! {
        pub struct Assembler;

        impl crate::machine::assembler::TargetAssembler for Assembler {
            type Inst = Inst;

            fn parse_insts(&self, _source: &str) -> Result<Vec<Self::Inst>, crate::AsmError> {
                Err(crate::AsmError::Other("assembler not yet implemented".into()))
            }
        }
    }
}
fn gen_target_machine_struct(model: &IsaModel) -> TokenStream {
    let isa_name_str = &model.meta.name;

    quote! {
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
                }
            }

            /// Create a new TargetMachine wrapped in Arc (for Registry).
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
        }

        crate::impl_erased_target_machine!(TargetMachine);

        /// Register this ISA backend in the global registry.
        pub fn ensure_registered() {
            static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
            INIT.get_or_init(|| {
                crate::machine::reloc_patcher::register_default_reloc_patcher(#isa_name_str);
                if !crate::Registry::global().contains(#isa_name_str) {
                    crate::Registry::global().register_backend(TargetMachine::new_arc());
                }
            });
        }
    }
}
