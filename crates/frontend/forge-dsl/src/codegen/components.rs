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
/// 判断名字是否解析为物理寄存器（大小写不敏感，支持 named/prefix 两种组）。
/// 与 resolve_reg_index 不同：未匹配时返回 false（而非兜底 0 号）。
pub(crate) fn is_phys_reg_name(model: &IsaModel, reg_name: &str) -> bool {
    for group in model.reg.values() {
        if let Some(names) = &group.names
            && names.iter().any(|n| n.eq_ignore_ascii_case(reg_name))
        {
            return true;
        }
        if let Some(prefix) = &group.prefix {
            let upper = reg_name.to_uppercase();
            let p = prefix.to_uppercase();
            if let Some(stripped) = upper.strip_prefix(&p)
                && !stripped.is_empty()
                && stripped.chars().all(|c| c.is_ascii_digit())
            {
                return true;
            }
        }
    }
    false
}

pub(crate) fn resolve_reg_index(model: &IsaModel, reg_name: &str) -> (bool, u32) {
    for (group_name, group) in &model.reg {
        let is_float = group_name.contains("xmm") || group_name.contains("float");
        // Named registers: RAX, RCX, R10, R11, etc.
        if let Some(names) = &group.names
            && let Some(pos) = names.iter().position(|n| n.eq_ignore_ascii_case(reg_name))
        {
            // 物理编号 = 组内索引 + 组声明的 base_index（如 x86 gpr8h = 4）。
            let idx = pos as u32 + group.base_index.unwrap_or(0);
            return (is_float, idx);
        }
        // Prefix-based: XMM0, XMM1, XMM15, etc.
        if let Some(ref prefix) = group.prefix
            && let Some(stripped) = reg_name.to_uppercase().strip_prefix(&prefix.to_uppercase())
            && let Ok(n) = stripped.parse::<u32>()
        {
            return (is_float, n);
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
    let ngpr = gpr.map(|g| g.count as u32).unwrap_or(16);
    let nfpr = model
        .reg
        .get("xmm")
        .or_else(|| model.reg.get("float"))
        .map(|g| g.count as u32)
        .unwrap_or(16);

    // ── 从 [reg_classes] 读取 allocatable 寄存器列表 ──
    // 如果定义了则使用；否则回退到全量 range
    let gpr_allocatable: Vec<u32> = model
        .reg_classes
        .get("GPR")
        .map(|rc| rc.allocatable.iter().map(|&x| x as u32).collect())
        .unwrap_or_else(|| (0..ngpr).collect());
    let fpr_allocatable: Vec<u32> = model
        .reg_classes
        .get("FPR")
        .map(|rc| rc.allocatable.iter().map(|&x| x as u32).collect())
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
                    quote! { Reg::#i as u32 }
                })
                .collect()
        })
        .unwrap_or_default();

    // Prologue bytes pushed above the frame pointer (fp save slot).
    let fp_push_bytes = model.abi.as_ref().map(|a| a.fp_push_bytes).unwrap_or(8);

    // Scratch registers — use explicit [abi.scratch] PReg numbers when available.
    // These are the physical register numbers reserved for spill loads/stores.
    // Fallback: derive from callee-saved inversion (exclude saved regs + SP).

    // 从 model 解析 SP 寄存器索引（用于 fallback 分支排除 SP；无 [abi.frame] 时为 None，
    // 此时不排除任何寄存器——不做 x86 RSP 假设）。
    let sp_index_for_scratch: Option<u32> =
        model.abi.as_ref().and_then(|a| a.frame.as_ref()).map(|f| {
            let (_, idx) = resolve_reg_index(model, &f.sp);
            idx
        });

    let scratch_regs: Vec<u32> = {
        let explicit: Option<Vec<u32>> = model.abi.as_ref().and_then(|a| {
            if a.scratch.is_empty() {
                None
            } else {
                Some(a.scratch.values().copied().collect())
            }
        });
        if let Some(regs) = explicit {
            regs
        } else {
            let saved: Vec<u32> = model
                .abi
                .as_ref()
                .map(|a| {
                    a.callee_saved
                        .gpr
                        .iter()
                        .map(|n| {
                            n.trim_start_matches(|c: char| !c.is_ascii_digit())
                                .parse::<u32>()
                                .unwrap_or(0)
                        })
                        .collect()
                })
                .unwrap_or_default();
            (0..ngpr)
                .filter(|n| !saved.contains(n) && (sp_index_for_scratch != Some(*n)))
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

    // ── 多宽度寄存器类收集（[reg_classes.*] 全类 + 主类 fallback）──
    // 每个类生成 RegisterClassInfo（name/count/width/reg_class/allocatable），
    // 供分配器按类型分派（如 I32 → GPR(4) 池）。
    let mut class_infos: Vec<TokenStream> = Vec::new();
    let mut width_arms: Vec<TokenStream> = Vec::new();
    let mut seen_classes: Vec<(String, u16)> = Vec::new(); // (kind, width) 去重
    for (name, rc) in &model.reg_classes {
        let kind = if name.starts_with("FPR") {
            "FPR"
        } else if name.starts_with("VEC") {
            "VEC"
        } else {
            "GPR"
        };
        let kind_ident = format_ident!("{}", kind);
        let width = rc.width as u16;
        let alloc: Vec<u32> = rc.allocatable.iter().map(|&x| x as u32).collect();
        let count = alloc.len() as u16;
        let key = (kind.to_string(), width);
        if seen_classes.contains(&key) {
            continue;
        }
        seen_classes.push(key.clone());
        let name_lit = name.clone();
        let reg_class_tok = quote! { forge_ir::RegClass::#kind_ident(#width) };
        class_infos.push(quote! {
            crate::machine::isa_info::RegisterClassInfo {
                name: #name_lit,
                count: #count,
                width: #width as u16,
                prefix: "",
                reg_class: #reg_class_tok,
                allocatable: vec![#(#alloc),*],
            }
        });
        let width_u8 = width as u8;
        width_arms.push(quote! { #reg_class_tok => #width_u8 });
    }
    // 主类 fallback：reg_classes 完全未定义 GPR/FPR 时补默认（全量可分配）。
    // 注意：不生成 width_arms——payload 即宽度，未定义类由 `_ => class.default_width()`
    // 覆盖（如 x86 的 FPR(8) x87 视图 → 8，而 FPR(16) XMM 由 [reg_classes.FPR] 定义）。
    if !seen_classes.iter().any(|(k, _)| k == "GPR") {
        class_infos.insert(
            0,
            quote! {
                crate::machine::isa_info::RegisterClassInfo {
                    name: "GPR",
                    count: #ngpr as u16,
                    width: #gpr_width as u16,
                    prefix: "",
                    reg_class: forge_ir::RegClass::GPR(8),
                    allocatable: vec![#(#gpr_allocatable),*],
                }
            },
        );
    }
    if !seen_classes.iter().any(|(k, _)| k == "FPR") {
        class_infos.push(quote! {
            crate::machine::isa_info::RegisterClassInfo {
                name: "FPR",
                count: #nfpr as u16,
                width: #fpr_width as u16,
                prefix: "",
                reg_class: forge_ir::RegClass::FPR(8),
                allocatable: vec![#(#fpr_allocatable),*],
            }
        });
    }

    // 默认整数值类（alloc_xreg 默认目标类）：由共享 helper 推导
    //（gpr64 组 → GPR(8)；仅 [reg.gpr] 按其声明宽度）。
    let (gpr_default_class, fpr_default_class) = super::default_class_tokens(model);

    quote! {
        pub struct RegInfo;

        impl crate::machine::reg_info::TargetRegInfo for RegInfo {
            type Reg = Reg;

            fn num_gp_regs(&self) -> u32 { #ngpr }
            fn num_fp_regs(&self) -> u32 { #nfpr }

            fn default_gpr_class(&self) -> forge_ir::RegClass {
                #gpr_default_class
            }

            fn default_fpr_class(&self) -> forge_ir::RegClass {
                #fpr_default_class
            }

            fn reg_class_width(&self, class: forge_ir::RegClass) -> u8 {
                match class {
                    #(#width_arms,)*
                    _ => class.default_width(),
                }
            }

            fn register_classes(&self) -> &[crate::machine::isa_info::RegisterClassInfo] {
                static CLASSES: std::sync::OnceLock<
                    Vec<crate::machine::isa_info::RegisterClassInfo>,
                > = std::sync::OnceLock::new();
                CLASSES.get_or_init(|| vec![#(#class_infos),*])
            }

            fn sp_reg(&self) -> forge_ir::FrameAccess<Self::Reg> { #sp_tokens }
            fn fp_reg(&self) -> Option<Self::Reg> { #fp_tokens }

            fn allocatable_gp_order(&self) -> Vec<u32> {
                vec![#(#gpr_allocatable),*]
            }
            fn allocatable_fp_order(&self) -> Vec<u32> {
                vec![#(#fpr_allocatable),*]
            }
            fn scratch_regs(&self) -> Vec<u32> {
                vec![#(#scratch_regs),*]
            }
            fn callee_saved(&self) -> Vec<u32> {
                vec![#(#callee_saved),*]
            }
            fn frame_pointer_overhead(&self) -> u32 {
                #fp_push_bytes
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
    let frame_padding = model.abi.as_ref().map(|a| a.frame_padding).unwrap_or(0);

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
            fn frame_padding(&self) -> i32 { #frame_padding }
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
            ) -> Result<crate::prelude::InstPacket<Self::Inst>, crate::prelude::IrError> {
                lower_impl(op, args, results, ctx)
            }

            fn lower_terminator(
                &self,
                term: &crate::prelude::Terminator,
                value_to_xreg: &std::collections::HashMap<crate::prelude::Value, crate::prelude::XReg>,
                _block_to_vblock: &std::collections::HashMap<crate::prelude::Block, crate::prelude::VBlockId>,
                ctx: &mut crate::prelude::LowerCtx,
            ) -> Result<crate::prelude::InstPacket<Self::Inst>, crate::prelude::IrError> {
                lower_terminator_impl(term, value_to_xreg, ctx)
            }

            fn lower_pattern(
                &self,
                pattern_name: &str,
                args: &[crate::prelude::XReg],
                results: &[crate::prelude::XReg],
                ctx: &mut crate::prelude::LowerCtx,
            ) -> Result<crate::prelude::InstPacket<Self::Inst>, crate::prelude::IrError> {
                lower_pattern_impl(pattern_name, args, results, ctx)
            }
        }
    }
}

// ============================================================
// TargetEncoder — delegates to existing emit_inst function
// ============================================================

fn gen_target_encoder(model: &IsaModel) -> TokenStream {
    // ISA 声明的 GPR 数（encoded_size 的 dummy 分配用；不再假设 x86 16 个）。
    let ngpr = model
        .reg
        .get("gpr64")
        .or_else(|| model.reg.get("gpr"))
        .map(|g| g.count as u32)
        .unwrap_or(16);
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
                // 寄存器数取 ISA 声明的 GPR 数（#ngpr），不再假设 x86 的 16 个。
                let mut rm = crate::AllocResult::default();
                for &vreg in inst.uses().iter().chain(inst.defs().iter()) {
                    let xreg = crate::prelude::XReg::new(vreg as u32, crate::prelude::RegClass::GPR64, 8);
                    if !rm.assignments.contains_key(&xreg) {
                        rm.insert(xreg, forge_ir::PReg::new((vreg % #ngpr) as u32, crate::prelude::RegClass::GPR64));
                    }
                }
                for n in 0..#ngpr {
                    let v = crate::prelude::XReg::new(n as u32, crate::prelude::RegClass::GPR64, 8);
                    if !rm.assignments.contains_key(&v) {
                        rm.insert(v, forge_ir::PReg::new((n % #ngpr) as u32, crate::prelude::RegClass::GPR64));
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

    // 解析 GPR spill 的 base 寄存器索引（[spill.GPR].load.base，或回退 [abi.frame].fp）。
    // 缺失时保持 None，由调用方生成编译错误——不假设 x86 RBP。
    let gpr_base_reg: Option<u32> = gpr_spill
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
        });

    // 解析 FPR spill 的 base 寄存器索引（如果存在）
    let fpr_base_reg: Option<u32> = fpr_spill
        .map(|s| {
            let (_, idx) = resolve_reg_index(model, &s.load.base);
            idx
        })
        .or(gpr_base_reg); // 回退到 GPR base

    // 解析 store 路径的 base 寄存器（从 s.store.base，回退到 load.base）
    let gpr_store_base_reg: Option<u32> = gpr_spill
        .map(|s| {
            let (_, idx) = resolve_reg_index(model, &s.store.base);
            idx
        })
        .or(gpr_base_reg);
    let fpr_store_base_reg: Option<u32> = fpr_spill
        .map(|s| {
            let (_, idx) = resolve_reg_index(model, &s.store.base);
            idx
        })
        .or(fpr_base_reg);

    // 收集已定义的 spill 类名称（用于错误消息）
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

    // Generate spill/jump overrides. variable-length ISAs get real
    // implementations driven by [spill.*] templates (template instruction +
    // base register, emitted via emit_inst — no ISA-specific bytes here).
    // ISAs without complete spill templates get no override (trait default).
    let (spill_load_body, spill_store_body, epilogue_jump_body) = if is_variable {
        let gpr_load_inst = gpr_spill.map(|s| super::pascal_ident(&s.load.inst));
        let gpr_store_inst = gpr_spill.map(|s| super::pascal_ident(&s.store.inst));
        let fpr_load_inst = fpr_spill.map(|s| super::pascal_ident(&s.load.inst));
        let fpr_store_inst = fpr_spill.map(|s| super::pascal_ident(&s.store.inst));
        match (
            gpr_base_reg,
            gpr_store_base_reg,
            fpr_base_reg,
            fpr_store_base_reg,
            gpr_load_inst,
            gpr_store_inst,
            fpr_load_inst,
            fpr_store_inst,
        ) {
            (
                Some(gpr_b),
                Some(gpr_sb),
                Some(fpr_b),
                Some(fpr_sb),
                Some(gpr_li),
                Some(gpr_si),
                Some(fpr_li),
                Some(fpr_si),
            ) => {
                let spill_load = quote! {
                    fn emit_spill_load(&self, dst_reg: u32, offset: i32, width: u8, is_fp: bool, sink: &mut crate::CodeSink) -> Result<(), crate::prelude::IrError> {
                        let __rm = crate::prelude::AllocResult::default();
                        if is_fp || width > 8 {
                            let __dest = <Reg as forge_ir::PhysReg>::from_index(dst_reg, crate::prelude::RegClass::FPR64);
                            emit_inst(&Inst::#fpr_li { dest: __dest, mem: crate::prelude::MemRef::new(#fpr_b, offset as i32, 16) }, &__rm, sink)?;
                        } else {
                            let __dest = <Reg as forge_ir::PhysReg>::from_index(dst_reg, crate::prelude::RegClass::GPR64);
                            emit_inst(&Inst::#gpr_li { dest: __dest, mem: crate::prelude::MemRef::new(#gpr_b, offset as i32, 8) }, &__rm, sink)?;
                        }
                        Ok(())
                    }
                };
                let spill_store = quote! {
                    fn emit_spill_store(&self, src_reg: u32, offset: i32, width: u8, is_fp: bool, sink: &mut crate::CodeSink) -> Result<(), crate::prelude::IrError> {
                        let __rm = crate::prelude::AllocResult::default();
                        if is_fp || width > 8 {
                            let __src = <Reg as forge_ir::PhysReg>::from_index(src_reg, crate::prelude::RegClass::FPR64);
                            emit_inst(&Inst::#fpr_si { src: __src, mem: crate::prelude::MemRef::new(#fpr_sb, offset as i32, 16) }, &__rm, sink)?;
                        } else {
                            let __src = <Reg as forge_ir::PhysReg>::from_index(src_reg, crate::prelude::RegClass::GPR64);
                            emit_inst(&Inst::#gpr_si { src: __src, mem: crate::prelude::MemRef::new(#gpr_sb, offset as i32, 8) }, &__rm, sink)?;
                        }
                        Ok(())
                    }
                };
                // epilogue jump：opcode 由 [meta].epilogue_jump_opcode 声明（无架构常量）。
                let epilogue_jump = match model.meta.epilogue_jump_opcode {
                    Some(op) => quote! {
                        fn emit_epilogue_jump(
                            &self,
                            _encoder: &std::sync::Arc<dyn crate::machine::encoder::TargetEncoder<Inst = Self::Inst>>,
                            _reg_map: &crate::AllocResult,
                            epilogue_block: forge_ir::Block,
                            sink: &mut crate::CodeSink,
                        ) -> Result<(), crate::prelude::IrError> {
                            sink.put1(#op);
                            let fixup = sink.offset();
                            sink.put4(0u32);
                            sink.use_label_at(fixup, epilogue_block, crate::RelocKind::REL4);
                            Ok(())
                        }
                    },
                    None => quote! {},
                };
                (spill_load, spill_store, epilogue_jump)
            }
            _ => {
                // 无完整 [spill.*] 模板：不生成 override（trait 默认返回
                // Unimplemented 错误），避免任何架构假设的编码。
                (quote! {}, quote! {}, quote! {})
            }
        }
    } else {
        // 非 variable-length ISA：生成信息丰富的运行时错误 stub。
        let spill_err = &spill_err_detail;
        let spill_load = quote! {
            fn emit_spill_load(&self, _dst_reg: u32, _offset: i32, _width: u8, _is_fp: bool, _sink: &mut crate::CodeSink) -> Result<(), crate::prelude::IrError> {
                Err(crate::prelude::IrError::Unimplemented(#spill_err.into()))
            }
        };
        let spill_store = quote! {
            fn emit_spill_store(&self, _src_reg: u32, _offset: i32, _width: u8, _is_fp: bool, _sink: &mut crate::CodeSink) -> Result<(), crate::prelude::IrError> {
                Err(crate::prelude::IrError::Unimplemented(#spill_err.into()))
            }
        };
        let epilogue_jump = quote! {
            fn emit_epilogue_jump(
                &self,
                _encoder: &std::sync::Arc<dyn crate::machine::encoder::TargetEncoder<Inst = Self::Inst>>,
                _reg_map: &crate::AllocResult,
                _epilogue_block: forge_ir::Block,
                _sink: &mut crate::CodeSink,
            ) -> Result<(), crate::prelude::IrError> {
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
            ) -> Result<(), crate::prelude::IrError> {
                emit_prologue_impl(frame_size, reg_map, sink)
            }

            fn emit_epilogue(
                &self,
                frame_size: u32,
                reg_map: &crate::AllocResult,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::prelude::IrError> {
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
    // 生成每 ISA 专属汇编器：Token 枚举（logos）+ lalrpop parser（OUT_DIR 缓存）+ Assembler
    match gen_assembler(model) {
        Ok(ts) => ts,
        Err(e) => syn::Error::new(proc_macro2::Span::call_site(), e).to_compile_error(),
    }
}

fn gen_assembler(model: &IsaModel) -> Result<TokenStream, String> {
    let isa_name = model.meta.name.to_lowercase().replace('-', "_");
    let token_enum_text = crate::asm_grammar::gen_token_enum_text(model)?;
    let grammar_text = crate::asm_grammar::gen_grammar_text(model)?;

    // lalrpop 编译（写临时目录 → 读回代码）。proc-macro 展开时 OUT_DIR 不可用
    //（cargo 只把它传给 build script），故 parser 代码直接嵌入 token stream，
    // 不依赖 include!/环境变量。语法文本仍写入临时目录供 lalrpop 处理。
    let gen_dir = std::env::temp_dir().join("forge_asm_gen").join(&isa_name);
    let parser_code = crate::asm_grammar::compile_grammar(&isa_name, &grammar_text, &gen_dir)?;

    // 调试：导出生成的语法文本与 parser 代码
    if std::env::var("FGE_DEBUG_GEN").is_ok() {
        let _ = std::fs::write(
            std::env::temp_dir().join(format!("forge_asm_{isa_name}.lalrpop")),
            &grammar_text,
        );
        let _ = std::fs::write(
            std::env::temp_dir().join(format!("forge_asm_{isa_name}_parser.rs")),
            &parser_code,
        );
    }

    let token_enum: TokenStream = token_enum_text.parse().map_err(|e| {
        let _ = std::fs::write(
            std::env::temp_dir().join(format!("forge_asm_{isa_name}_token_enum.rs")),
            &token_enum_text,
        );
        format!("token enum parse: {e}")
    })?;
    let parser_ts: TokenStream = parser_code
        .parse()
        .map_err(|e| format!("parser code parse: {e}"))?;

    // bind 层（类型签名驱动 + 候选组消歧 + 两遍标签解析）
    let (_, meta) = crate::asm_grammar::build_meta(model)?;
    let bind_impl = gen_bind_impl(model, &meta)?;

    Ok(quote::quote! {
        // ── 汇编 token 枚举（logos，该 ISA 保留字 + 通用 token）──
        #token_enum

        // ── lalrpop 生成的每 ISA parser（嵌入 token stream）──
        // 结构：parser 在 `mod asm_parser` 内；语法 use 的 `super::Token`
        // 指向本模块的 Token，`crate::assembler::*` 指向 forge-asm 共享类型。
        mod asm_parser {
            #parser_ts
        }

        /// 汇编器：`TargetAssembler` 实现（parse_insts = parse_lines + bind）。
        pub struct Assembler;

        impl crate::machine::assembler::TargetAssembler for Assembler {
            type Inst = Inst;

            fn parse_lines(
                &self,
                source: &str,
            ) -> Result<Vec<crate::assembler::AsmLine>, crate::machine::assembler::AsmError> {
                let parser = asm_parser::ProgramParser::new();
                let tokens = crate::assembler::TokenStream::<Token>::new(source);
                parser
                    .parse(tokens)
                    .map_err(|e| crate::machine::assembler::AsmError::ParseError(format!("{e}")))
            }

            fn bind(
                &self,
                lines: Vec<crate::assembler::AsmLine>,
            ) -> Result<Vec<Self::Inst>, crate::machine::assembler::AsmError> {
                #bind_impl
                __bind_lines(lines)
            }
        }
    })
}
fn gen_target_machine_struct(model: &IsaModel) -> TokenStream {
    let isa_name_str = &model.meta.name;

    // Stage 3 模式融合门控：[meta].enable_pattern_isel = true 的 ISA
    // 通过 TargetMachine::pattern_matcher() 暴露全局标准模式匹配器。
    let pattern_matcher_fn = if model.meta.enable_pattern_isel.unwrap_or(false) {
        quote! {
            fn pattern_matcher(&self) -> Option<&crate::ext::pattern_isel::PatternMatcher> {
                Some(crate::ext::pattern_isel::standard_matcher())
            }
        }
    } else {
        quote! {}
    };

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
            fn decoder(&self) -> Option<&std::sync::Arc<dyn crate::machine::decoder::TargetDecoder<Inst = Self::Inst>>> { Some(&self.decoder) }

            #pattern_matcher_fn
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

// ============================================================
// Assembler bind 层生成（类型签名驱动的指令绑定 + 消歧）
// ============================================================

/// 字段类型签名编码（生成期常量，供 __match 谓词使用）。
#[derive(Clone, Copy, PartialEq)]
enum TyCode {
    /// Ireg/Freg：任意 Reg 类操作数（物理或虚拟）
    AnyReg,
    /// GprReg：物理 GPR 名
    Gpr,
    /// XmmReg：物理 FPR 名
    Fpr,
    /// 整型立即数（宽度/符号在绑定时校验）
    Imm,
    /// 浮点立即数
    Float,
    /// 内存引用
    Mem,
    /// 块标签
    Label,
    /// 条件码
    Cond,
}

fn ty_code(ft: &FieldType) -> TyCode {
    match ft {
        FieldType::Ireg | FieldType::Freg => TyCode::AnyReg,
        FieldType::GprReg => TyCode::Gpr,
        FieldType::XmmReg => TyCode::Fpr,
        FieldType::I8
        | FieldType::I16
        | FieldType::I32
        | FieldType::I64
        | FieldType::U8
        | FieldType::U16
        | FieldType::U32
        | FieldType::U64 => TyCode::Imm,
        FieldType::F32 | FieldType::F64 => TyCode::Float,
        FieldType::MemRef => TyCode::Mem,
        FieldType::BlockTarget => TyCode::Label,
        FieldType::CondCode => TyCode::Cond,
        FieldType::Opsize => TyCode::Imm,
    }
}

/// 生成 bind 层：两遍标签解析 + 组内候选语义消歧 + 字段绑定。
fn gen_bind_impl(
    model: &IsaModel,
    meta: &crate::asm_grammar::AsmMeta,
) -> Result<TokenStream, String> {
    let reg_lookup = gen_reg_lookup(model);
    let cc_lookup = gen_cc_lookup(model);

    // 每候选组的绑定 arm
    let mut arms: Vec<TokenStream> = Vec::new();
    for (gi, cands) in meta.groups.iter().enumerate() {
        if cands.len() == 1 {
            arms.push(bind_single_arm(gi, cands[0], meta)?);
        } else {
            arms.push(bind_group_arm(gi, cands, meta)?);
        }
    }

    Ok(quote! {
        // ── bind 辅助：寄存器/cc 查表 ──
        #reg_lookup
        #cc_lookup

        fn __reg_field(
            op: &crate::assembler::OperandValue,
            mn: &str,
            idx: usize,
            want: u8,
        ) -> Result<Reg, crate::machine::assembler::AsmError> {
            use crate::machine::assembler::AsmError::TypeMismatch;
            let name = match op {
                crate::assembler::OperandValue::Ident(n) => n.as_str(),
                crate::assembler::OperandValue::TmpReg(n) => n.as_str(),
                _ => {
                    return Err(TypeMismatch {
                        mnemonic: mn.to_string(),
                        index: idx,
                        expected: "register".to_string(),
                        got: format!("{:?}", op),
                    })
                }
            };
            match __asm_reg(name) {
                Some(reg) => {
                    let is_fpr = reg.class() == __DEFAULT_FPR_CLASS;
                    if want == 1 && is_fpr {
                        return Err(TypeMismatch {
                            mnemonic: mn.to_string(),
                            index: idx,
                            expected: "GPR register".to_string(),
                            got: name.to_string(),
                        });
                    }
                    if want == 2 && !is_fpr {
                        return Err(TypeMismatch {
                            mnemonic: mn.to_string(),
                            index: idx,
                            expected: "FPR register".to_string(),
                            got: name.to_string(),
                        });
                    }
                    Ok(reg)
                }
                None => Err(TypeMismatch {
                    mnemonic: mn.to_string(),
                    index: idx,
                    expected: "physical register".to_string(),
                    got: name.to_string(),
                }),
            }
        }

        fn __int_field(
            op: &crate::assembler::OperandValue,
            mn: &str,
            idx: usize,
            bits: u8,
            signed: bool,
        ) -> Result<i64, crate::machine::assembler::AsmError> {
            use crate::machine::assembler::AsmError::TypeMismatch;
            let v = match op {
                crate::assembler::OperandValue::Imm(v) | crate::assembler::OperandValue::Const(v) => *v,
                _ => {
                    return Err(TypeMismatch {
                        mnemonic: mn.to_string(),
                        index: idx,
                        expected: "immediate".to_string(),
                        got: format!("{:?}", op),
                    })
                }
            };
            let (lo, hi): (i64, i64) = if signed {
                match bits {
                    8 => (-128, 127),
                    16 => (-32768, 32767),
                    32 => (i32::MIN as i64, i32::MAX as i64),
                    _ => (i64::MIN, i64::MAX),
                }
            } else {
                match bits {
                    8 => (0, 255),
                    16 => (0, 65535),
                    32 => (0, u32::MAX as i64),
                    64 => (0, i64::MAX),
                    _ => (i64::MIN, i64::MAX),
                }
            };
            if v < lo || v > hi {
                return Err(TypeMismatch {
                    mnemonic: mn.to_string(),
                    index: idx,
                    expected: format!("{}-bit {} immediate", bits, if signed { "signed" } else { "unsigned" }),
                    got: v.to_string(),
                });
            }
            Ok(v)
        }

        fn __float_field(
            op: &crate::assembler::OperandValue,
            mn: &str,
            idx: usize,
        ) -> Result<f64, crate::machine::assembler::AsmError> {
            use crate::machine::assembler::AsmError::TypeMismatch;
            match op {
                crate::assembler::OperandValue::Float(v) => Ok(*v),
                _ => Err(TypeMismatch {
                    mnemonic: mn.to_string(),
                    index: idx,
                    expected: "float immediate".to_string(),
                    got: format!("{:?}", op),
                }),
            }
        }

        fn __cc_field(
            op: &crate::assembler::OperandValue,
            mn: &str,
            idx: usize,
        ) -> Result<u8, crate::machine::assembler::AsmError> {
            use crate::machine::assembler::AsmError::TypeMismatch;
            match op {
                crate::assembler::OperandValue::Ident(n) => __asm_cc(n).ok_or_else(|| TypeMismatch {
                    mnemonic: mn.to_string(),
                    index: idx,
                    expected: "condition code".to_string(),
                    got: n.clone(),
                }),
                _ => Err(TypeMismatch {
                    mnemonic: mn.to_string(),
                    index: idx,
                    expected: "condition code".to_string(),
                    got: format!("{:?}", op),
                }),
            }
        }

        fn __label_field(
            op: &crate::assembler::OperandValue,
            labels: &std::collections::HashMap<String, i64>,
            mn: &str,
            idx: usize,
            allow_missing: bool,
        ) -> Result<i64, crate::machine::assembler::AsmError> {
            use crate::machine::assembler::AsmError::{TypeMismatch, UndefinedLabel};
            match op {
                crate::assembler::OperandValue::Label(l) => {
                    // 数字标签（`jmp 8` / `jmp 0x10`）直接作为字节偏移
                    if let Ok(v) = l.parse::<i64>() {
                        return Ok(v);
                    }
                    if let Some(hex) = l.strip_prefix("0x") {
                        if let Ok(v) = i64::from_str_radix(hex, 16) {
                            return Ok(v);
                        }
                    }
                    match labels.get(l) {
                        Some(v) => Ok(*v),
                        None if allow_missing => Ok(0),
                        None => Err(UndefinedLabel(l.clone())),
                    }
                }
                _ => Err(TypeMismatch {
                    mnemonic: mn.to_string(),
                    index: idx,
                    expected: "label".to_string(),
                    got: format!("{:?}", op),
                }),
            }
        }

        fn __mem_field(
            op: &crate::assembler::OperandValue,
            mn: &str,
            idx: usize,
        ) -> Result<MemRef, crate::machine::assembler::AsmError> {
            use crate::machine::assembler::AsmError::TypeMismatch;
            match op {
                crate::assembler::OperandValue::Mem { base, disp } => {
                    let base_idx = match base {
                        Some(b) => __asm_reg(b).ok_or_else(|| TypeMismatch {
                            mnemonic: mn.to_string(),
                            index: idx,
                            expected: "base register".to_string(),
                            got: b.clone(),
                        })?.to_index(),
                        None => 0,
                    };
                    Ok(MemRef::new(base_idx, *disp as i32, 8))
                }
                _ => Err(TypeMismatch {
                    mnemonic: mn.to_string(),
                    index: idx,
                    expected: "memory reference".to_string(),
                    got: format!("{:?}", op),
                }),
            }
        }

        // 候选组内语义消歧：字段类型签名 → 匹配谓词
        fn __match(ty: u8, op: &crate::assembler::OperandValue) -> bool {
            match ty {
                0 => matches!(op, crate::assembler::OperandValue::Ident(_)
                    | crate::assembler::OperandValue::TmpReg(_)
                    | crate::assembler::OperandValue::VReg(_)),
                1 => matches!(op, crate::assembler::OperandValue::Ident(n) if __asm_reg(n).is_some_and(|r| r.class() == __DEFAULT_GPR_CLASS)),
                2 => matches!(op, crate::assembler::OperandValue::Ident(n) if __asm_reg(n).is_some_and(|r| r.class() == __DEFAULT_FPR_CLASS)),
                3 => matches!(op, crate::assembler::OperandValue::Imm(_) | crate::assembler::OperandValue::Const(_)),
                4 => matches!(op, crate::assembler::OperandValue::Float(_)),
                5 => matches!(op, crate::assembler::OperandValue::Mem { .. }),
                6 => matches!(op, crate::assembler::OperandValue::Label(_)),
                7 => matches!(op, crate::assembler::OperandValue::Ident(n) if __asm_cc(n).is_some()),
                _ => true,
            }
        }

        fn __match_all(tys: &[u8], ops: &[crate::assembler::OperandValue]) -> bool {
            tys.len() == ops.len() && tys.iter().zip(ops.iter()).all(|(t, o)| __match(*t, o))
        }

        // ── 单指令绑定：组索引 → Inst ──
        fn __bind_one(
            raw: &crate::assembler::RawInst,
            labels: &std::collections::HashMap<String, i64>,
            allow_missing: bool,
        ) -> Result<Inst, crate::machine::assembler::AsmError> {
            if raw.inst_idx == usize::MAX {
                return Err(crate::machine::assembler::AsmError::Other(format!(
                    "pseudo-op `{}` has no standalone encoding (emit macro)",
                    raw.mnemonic
                )));
            }
            let ops = &raw.operands;
            let mn = raw.mnemonic.as_str();
            match raw.inst_idx {
                #(#arms)*
                _ => Err(crate::machine::assembler::AsmError::Other(format!(
                    "internal: unknown inst group {}",
                    raw.inst_idx
                ))),
            }
        }

        // ── TargetAssembler::bind：标签→字节偏移两遍解析 + 绑定 ──
        // 第一遍收集标签→指令序号；占位绑定（rel=0）后经 Encoder 计算每条
        // 指令的字节长度（rel 为固定宽度，占位不影响长度），得标签的字节偏移；
        // 第二遍用真实偏移重新绑定 BlockTarget。
        fn __bind_lines(
            lines: Vec<crate::assembler::AsmLine>,
        ) -> Result<Vec<Inst>, crate::machine::assembler::AsmError> {
            let mut label_order: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            let mut n = 0usize;
            for line in &lines {
                if let Some(l) = &line.label {
                    label_order.insert(l.clone(), n);
                }
                if line.inst.is_some() {
                    n += 1;
                }
            }
            let empty: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
            let placeholder: Vec<Inst> = lines
                .iter()
                .filter_map(|l| l.inst.as_ref())
                .map(|raw| __bind_one(raw, &empty, true))
                .collect::<Result<_, _>>()?;
            let encoder = Encoder;
            // offsets[i] = 第 i 条指令的起始字节偏移；offsets[n] = 代码末尾
            // （末尾标签如 `jmp .Lend\n.Lend:` 的序号为指令总数）。
            let mut offsets: Vec<i64> = Vec::with_capacity(placeholder.len() + 1);
            offsets.push(0);
            let mut off: i64 = 0;
            for inst in &placeholder {
                off += crate::machine::encoder::TargetEncoder::encoded_size(&encoder, inst)
                    .map_err(|e| {
                        crate::machine::assembler::AsmError::Other(format!(
                            "encode for label offset: {e}"
                        ))
                    })? as i64;
                offsets.push(off);
            }
            let mut labels: std::collections::HashMap<String, i64> =
                std::collections::HashMap::new();
            for (name, i) in label_order {
                labels.insert(name, offsets[i]);
            }
            let mut out = Vec::with_capacity(lines.len());
            for line in lines {
                if let Some(raw) = line.inst {
                    out.push(__bind_one(&raw, &labels, false)?);
                }
            }
            Ok(out)
        }
    })
}

/// 单候选组绑定 arm。
fn bind_single_arm(
    gi: usize,
    inst_idx: usize,
    meta: &crate::asm_grammar::AsmMeta,
) -> Result<TokenStream, String> {
    let inst_name = &meta.inst_names[inst_idx];
    let variant = crate::codegen::pascal_ident(inst_name);
    let fields = &meta.inst_fields[inst_idx];
    let operands = &meta.inst_operands[inst_idx];
    let assigns = field_assigns(fields, operands, inst_name, meta.group_mnemonic_cc[gi])?;
    let arity = operands.len();
    Ok(quote! {
        #gi => {
            if ops.len() != #arity {
                return Err(crate::machine::assembler::AsmError::TypeMismatch {
                    mnemonic: mn.to_string(),
                    index: ops.len(),
                    expected: format!("{} operands", #arity),
                    got: ops.len().to_string(),
                });
            }
            Ok(Inst::#variant { #(#assigns),* })
        }
    })
}

/// 多候选组绑定 arm：类型签名过滤 → 唯一候选绑定，否则 Ambiguous。
fn bind_group_arm(
    gi: usize,
    cands: &[usize],
    meta: &crate::asm_grammar::AsmMeta,
) -> Result<TokenStream, String> {
    // 每候选的匹配谓词 + 绑定代码
    let mut checks: Vec<TokenStream> = Vec::new();
    let mut bindings: Vec<TokenStream> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for &c in cands {
        let inst_name = &meta.inst_names[c];
        let variant = crate::codegen::pascal_ident(inst_name);
        let fields = &meta.inst_fields[c];
        // 组内候选共享语法形状（字段名顺序），但字段类型可能不同（Ireg vs
        // GprReg）——语义消歧按各自 OperandSpec.ty 的类型签名计算。
        let operands = &meta.inst_operands[c];
        let tys: Vec<u8> = operands.iter().map(|o| ty_code(&o.ty) as u8).collect();
        let tys_lit = quote! { &[#(#tys),*] };
        // 特异度权重：Gpr/Fpr 物理约束比 AnyReg 更精确（如 add RAX,5 →
        // GprReg,u32 候选优先于 Ireg,i64 候选）
        let weight: usize = operands
            .iter()
            .map(|o| match ty_code(&o.ty) {
                TyCode::Gpr | TyCode::Fpr => 2,
                _ => 1,
            })
            .sum();
        let assigns = field_assigns(fields, operands, inst_name, meta.group_mnemonic_cc[gi])?;
        let c_lit = c;
        let w_lit = weight;
        checks.push(quote! {
            if __match_all(#tys_lit, ops) {
                if #w_lit > best_w {
                    best_w = #w_lit;
                    best = #c_lit;
                    ties = 1;
                } else if #w_lit == best_w {
                    ties += 1;
                }
            }
        });
        bindings.push(quote! {
            #c_lit => {
                Ok(Inst::#variant { #(#assigns),* })
            }
        });
        names.push(inst_name.clone());
    }
    let arity = meta.inst_operands[cands[0]].len();
    let names_str = names.join(", ");
    Ok(quote! {
        #gi => {
            if ops.len() != #arity {
                return Err(crate::machine::assembler::AsmError::TypeMismatch {
                    mnemonic: mn.to_string(),
                    index: ops.len(),
                    expected: format!("{} operands", #arity),
                    got: ops.len().to_string(),
                });
            }
            let mut best: usize = usize::MAX;
            let mut best_w: usize = 0;
            let mut ties: usize = 0;
            #(#checks)*
            match (best, ties) {
                (usize::MAX, _) => Err(crate::machine::assembler::AsmError::TypeMismatch {
                    mnemonic: mn.to_string(),
                    index: 0,
                    expected: "any of [".to_string() + #names_str + "]",
                    got: format!("{:?}", ops),
                }),
                (c, 1) => match c {
                    #(#bindings)*
                    _ => unreachable!(),
                },
                (_, _) => Err(crate::machine::assembler::AsmError::Ambiguous(format!(
                    "`{}` matches {} overloads: [{}]",
                    mn, ties, #names_str
                ))),
            }
        }
    })
}

/// 生成字段赋值表达式：
/// - 候选操作数字段 → ops[k] 绑定（k 按候选操作数顺序）
/// - 隐式 Opsize → 默认 64
/// - mnemonic 内嵌 CondCode（set{cond} 展开）→ 生成期内联 cc 值
fn field_assigns(
    fields: &[(String, FieldType)],
    operands: &[crate::asm_grammar::OperandSpec],
    inst_name: &str,
    mnemonic_cc: Option<u8>,
) -> Result<Vec<TokenStream>, String> {
    let _mn = inst_name.to_lowercase();
    let mut assigns = Vec::new();
    for (name, ft) in fields {
        let fi = format_ident!("{}", name);
        // 操作数位置 = operands 中的原始顺序（与 parser ops 顺序一致）；
        // 绑定分支的类型直接取自 OperandSpec.ty。
        if let Some((idx, ospec)) = operands.iter().enumerate().find(|(_, o)| o.field == *name) {
            match &ospec.ty {
                FieldType::Ireg | FieldType::Freg => {
                    assigns.push(quote! { #fi: __reg_field(&ops[#idx], mn, #idx, 0)? });
                }
                FieldType::GprReg => {
                    assigns.push(quote! { #fi: __reg_field(&ops[#idx], mn, #idx, 1)? });
                }
                FieldType::XmmReg => {
                    assigns.push(quote! { #fi: __reg_field(&ops[#idx], mn, #idx, 2)? });
                }
                FieldType::I8 | FieldType::U8 => {
                    let signed = matches!(ft, FieldType::I8);
                    let ty_tok = crate::codegen::field_type_tok(ft);
                    assigns.push(
                        quote! { #fi: __int_field(&ops[#idx], mn, #idx, 8, #signed)? as #ty_tok },
                    );
                }
                FieldType::I16 | FieldType::U16 => {
                    let signed = matches!(ft, FieldType::I16);
                    let ty_tok = crate::codegen::field_type_tok(ft);
                    assigns.push(
                        quote! { #fi: __int_field(&ops[#idx], mn, #idx, 16, #signed)? as #ty_tok },
                    );
                }
                FieldType::I32 | FieldType::U32 => {
                    let signed = matches!(ft, FieldType::I32);
                    let ty_tok = crate::codegen::field_type_tok(ft);
                    assigns.push(
                        quote! { #fi: __int_field(&ops[#idx], mn, #idx, 32, #signed)? as #ty_tok },
                    );
                }
                FieldType::I64 | FieldType::U64 => {
                    let signed = matches!(ft, FieldType::I64);
                    let ty_tok = crate::codegen::field_type_tok(ft);
                    assigns.push(
                        quote! { #fi: __int_field(&ops[#idx], mn, #idx, 64, #signed)? as #ty_tok },
                    );
                }
                FieldType::F32 | FieldType::F64 => {
                    let ty_tok = crate::codegen::field_type_tok(ft);
                    assigns.push(quote! { #fi: __float_field(&ops[#idx], mn, #idx)? as #ty_tok });
                }
                FieldType::MemRef => {
                    assigns.push(quote! { #fi: __mem_field(&ops[#idx], mn, #idx)? });
                }
                FieldType::BlockTarget => {
                    assigns.push(
                        quote! { #fi: __label_field(&ops[#idx], labels, mn, #idx, allow_missing)? },
                    );
                }
                FieldType::CondCode => {
                    assigns.push(quote! { #fi: __cc_field(&ops[#idx], mn, #idx)? });
                }
                FieldType::Opsize => {
                    assigns.push(quote! { #fi: 64u8 });
                }
            }
            continue;
        }
        // 非操作数位置字段：隐式
        match ft {
            FieldType::Opsize => {
                // Opsize 默认 64（parse 场景无 ctx）
                assigns.push(quote! { #fi: 64u8 });
            }
            FieldType::CondCode => {
                // mnemonic 内嵌 cond（set{cond}/j{cond} 展开）→ 内联 cc 值
                let cc_val = mnemonic_cc.unwrap_or(0);
                assigns.push(quote! { #fi: #cc_val });
            }
            _ => {
                // 其它隐式字段：寄存器 → 索引 0 物理寄存器；MemRef → unresolved；
                // 整数/浮点 → Default
                match ft {
                    FieldType::Ireg | FieldType::Freg | FieldType::GprReg | FieldType::XmmReg => {
                        assigns.push(quote! {
                            #fi: <Reg as forge_ir::PhysReg>::from_index(0, __DEFAULT_GPR_CLASS)
                        });
                    }
                    FieldType::MemRef => {
                        assigns.push(quote! { #fi: MemRef::unresolved(8) });
                    }
                    _ => {
                        let ty_tok = crate::codegen::field_type_tok(ft);
                        assigns.push(quote! { #fi: <#ty_tok as Default>::default() });
                    }
                }
            }
        }
    }
    let _ = _mn;
    Ok(assigns)
}

/// 生成运行时寄存器名解析：`__asm_reg(name) -> Option<Reg>`。
///
/// named 组（含带 base_index 的视图组如 x86 gpr8h 的 AH/CH/DH/BH）直接返回
/// Reg 变体值——`from_index` 无法还原高字节寄存器视图，必须按名字匹配。
/// prefix 组（minimal_sd R0、wasm32 L0）用 `from_index(n, class)`。
fn gen_reg_lookup(model: &IsaModel) -> TokenStream {
    let mut named_arms: Vec<TokenStream> = Vec::new();
    let mut prefix_arms: Vec<TokenStream> = Vec::new();
    for (group_name, group) in &model.reg {
        let is_fpr = group_name.contains("xmm") || group_name.contains("float");
        let cls = if is_fpr {
            quote! { __DEFAULT_FPR_CLASS }
        } else {
            quote! { __DEFAULT_GPR_CLASS }
        };
        if let Some(names) = &group.names {
            for n in names.iter() {
                let up = n.to_uppercase();
                let variant = format_ident!("{}", n);
                named_arms.push(quote! {
                    #up => Some(Reg::#variant),
                });
            }
        }
        if let Some(prefix) = &group.prefix {
            let up_prefix = prefix.to_uppercase();
            prefix_arms.push(quote! {
                if let Some(rest) = name.strip_prefix(#up_prefix) {
                    if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                        if let Ok(n) = rest.parse::<u32>() {
                            return Some(<Reg as forge_ir::PhysReg>::from_index(n, #cls));
                        }
                    }
                }
            });
        }
    }
    quote! {
        fn __asm_reg(name: &str) -> Option<Reg> {
            let name = name.to_uppercase();
            match name.as_str() {
                #(#named_arms)*
                _ => {
                    #(#prefix_arms)*
                    return None;
                }
            }
        }
    }
}

/// 生成运行时条件码查表：`__asm_cc(name) -> Option<u8>`。
fn gen_cc_lookup(model: &IsaModel) -> TokenStream {
    let mut arms: Vec<TokenStream> = Vec::new();
    for (hex_key, cc_name) in &model.cc_names {
        let val = u64::from_str_radix(
            hex_key.trim_start_matches("0x").trim_start_matches("0X"),
            16,
        )
        .unwrap_or(0) as u8;
        let cc = cc_name.clone();
        let v = val;
        arms.push(quote! {
            #cc => Some(#v),
        });
    }
    quote! {
        fn __asm_cc(name: &str) -> Option<u8> {
            match name {
                #(#arms)*
                _ => None,
            }
        }
    }
}
