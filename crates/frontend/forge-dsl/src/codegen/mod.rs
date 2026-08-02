//! 代码生成器 — 从 IsaModel 生成 Rust TokenStream。
//!
//! 所有生成内容来自 TOML 模型字段，零硬编码。

use crate::model::*;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::HashSet;

/// Get the canonical GPR register group, preferring `gpr64` with fallback to `gpr`.
fn get_gpr_group(model: &IsaModel) -> &RegGroup {
    model
        .reg
        .get("gpr64")
        .or_else(|| model.reg.get("gpr"))
        .expect("missing [reg.gpr64] or [reg.gpr]")
}

mod components;
#[path = "../standard_insts.rs"]
mod standard_insts;

/// Generate x86 encoding helper functions (modrm, sib, REX, etc.).
/// These are generated as private functions in the ISA module.
/// For non-x86 ISAs they compile to dead code (suppressed by #[allow(dead_code)]).
fn gen_x86_helpers() -> TokenStream {
    quote::quote! {
        #[allow(dead_code)]
        pub fn modrm(mod_bits: u8, reg: u8, rm: u8) -> u8 {
            (mod_bits << 6) | ((reg & 0x7) << 3) | (rm & 0x7)
        }

        #[allow(dead_code)]
        pub fn sib(scale: u8, index: u8, base: u8) -> u8 {
            let scale_bits = match scale { 1 => 0, 2 => 1, 4 => 2, 8 => 3, _ => 0 };
            (scale_bits << 6) | ((index & 0x7) << 3) | (base & 0x7)
        }

        #[allow(dead_code)]
        pub fn enc_rr_0f(sink: &mut crate::prelude::CodeSink, opcode: u8, reg: u8, rm: u8, w: bool) {
            let base: u8 = if w { 0x48 } else { 0x40 };
            let rex = base | if reg >= 8 { 0x04 } else { 0 } | if rm >= 8 { 0x01 } else { 0 };
            if rex != 0x40 || w { sink.put1(rex); }
            sink.put1(0x0F);
            sink.put1(opcode);
            sink.put1(modrm(3, reg, rm));
        }

        #[allow(dead_code)]
        pub fn enc_sse_rr(sink: &mut crate::prelude::CodeSink, prefix: u8, opcode: u8, w: bool, reg: u8, rm: u8) {
            sink.put1(prefix);
            let base: u8 = if w { 0x48 } else { 0x40 };
            let rex = base | if reg >= 8 { 0x04 } else { 0 } | if rm >= 8 { 0x01 } else { 0 };
            if rex != 0x40 || w { sink.put1(rex); }
            sink.put1(0x0F);
            sink.put1(opcode);
            sink.put1(modrm(3, reg, rm));
        }

        #[allow(dead_code)]
        /// LEA rd, [RBP+disp32]（StackAddr 栈帧地址；RBP=5 架构常量）
        pub fn enc_lea_rbp(sink: &mut crate::prelude::CodeSink, rd: u8, offset: i32) {
            let rex = 0x48u8 | if (rd & 0x8) != 0 { 0x04 } else { 0 };
            sink.put1(rex);
            sink.put1(0x8D);
            const RBP: u8 = 5;
            if (-128i32..=127).contains(&offset) {
                sink.put1(modrm(1, rd, RBP));
                sink.put1(offset as u8);
            } else {
                sink.put1(modrm(2, rd, RBP));
                sink.put4(offset as u32);
            }
        }
        pub fn enc_lea_sib(sink: &mut crate::prelude::CodeSink, rd: u8, base: u8, index: u8, scale: u8, offset: i32) {
            let rex = 0x48u8
                | if (rd & 0x8) != 0 { 0x04 } else { 0 }
                | if (index & 0x8) != 0 { 0x02 } else { 0 }
                | if (base & 0x8) != 0 { 0x01 } else { 0 };
            sink.put1(rex);
            sink.put1(0x8D);
            // RBP=5 是 x86 ISA 架构常量：当 base=RBP 且 mod=00 时，x86 强制要求
            // SIB 寻址带有 8/32 位位移。这不是寄存器分配硬编码。
            const RBP: u8 = 5; // x86 architectural constant (RBP encoding)
            let need_disp = offset != 0 || (base & 0x7) == RBP;
            if !need_disp {
                sink.put1(modrm(0, rd, 0x04));
                sink.put1(sib(scale, index, base));
            } else if (-128i32..=127).contains(&offset) {
                sink.put1(modrm(1, rd, 0x04));
                sink.put1(sib(scale, index, base));
                sink.put1(offset as u8);
            } else {
                sink.put1(modrm(2, rd, 0x04));
                sink.put1(sib(scale, index, base));
                sink.put4(offset as u32);
            }
        }
    }
}

pub fn generate(model: &IsaModel) -> Result<TokenStream, String> {
    let reg_enum = gen_reg_enum(model);
    let inst_enum = gen_inst_enum(model);
    let machine_inst = gen_machine_inst(model);

    // Shared free functions — used by v19 component impls
    let emit_func = gen_emit_func(model)?;
    let lower_func = gen_lower_func(model)?;
    let lower_term_func = gen_lower_term_func(model)?;
    let lower_pattern_func = gen_lower_pattern_func(model)?;

    // x86 encoding helpers — only generated for variable-length ISAs
    let x86_helpers = if model.meta.capabilities.variable_length {
        gen_x86_helpers()
    } else {
        quote! {}
    };

    // v19: Componentized TargetMachine + trait impls
    let v19_components = components::gen_v19_components(model);

    Ok(quote! {
        #reg_enum
        #inst_enum
        #machine_inst
        // ── ISA encoding helpers ──
        #x86_helpers
        // ── shared lowering / emit functions ──
        #emit_func
        #lower_func
        #lower_term_func
        #lower_pattern_func
        // ── v19 componentized API ──
        #v19_components
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

/// Result of looking up a register name in the ISA model.
enum RegLookup {
    /// Named register found at index, with its exact name.
    Named { name: String, index: u8 },
    /// Prefix-based register found at index (e.g. XMM0 with prefix "XMM").
    Prefixed { name: String, index: u8 },
    /// Not found in any register group.
    NotFound,
}

/// Look up a register name in the ISA model's register groups.
/// Searches both named registers and prefix-based registers.
/// Returns the register's exact name and its index in the group.
fn lookup_reg_in_model(val: &str, model: &IsaModel) -> RegLookup {
    for group in model.reg.values() {
        // Named registers: "RAX", "RCX", "R10", "XMM0", ...
        if let Some(names) = &group.names
            && let Some(pos) = names.iter().position(|n| n.eq_ignore_ascii_case(val))
        {
            return RegLookup::Named {
                name: names[pos].clone(),
                index: pos as u8,
            };
        }
        // Prefix-based names: "XMM0" with prefix "XMM", "R5" with prefix "R"
        if let Some(ref prefix) = group.prefix {
            let upper = val.to_uppercase();
            if let Some(stripped) = upper.strip_prefix(&prefix.to_uppercase())
                && let Ok(n) = stripped.parse::<u32>()
            {
                let full_name = format!("{prefix}{n}");
                return RegLookup::Prefixed {
                    name: full_name,
                    index: n as u8,
                };
            }
        }
    }
    RegLookup::NotFound
}

/// Check if a field is a def (write) register, respecting explicit TOML role.
fn is_field_def(field: &crate::model::InstField) -> bool {
    match field.role.as_deref() {
        Some("def") | Some("both") => true,
        Some("use") => false,
        _ => field.name == "dest" || field.name == "reg",
    }
}

/// Check if a field is a use (read) register, respecting explicit TOML role.
fn is_field_use(field: &crate::model::InstField) -> bool {
    match field.role.as_deref() {
        Some("use") | Some("both") => true,
        Some("def") => false,
        _ => field.name.starts_with("src") || field.name == "base",
    }
}

// ============================================================
// Reg 枚举
// ============================================================

fn gen_reg_enum(model: &IsaModel) -> TokenStream {
    let gpr = get_gpr_group(model);
    let gpr_count = gpr.count as usize;

    // Collect GPR variants
    let gpr_variants: Vec<TokenStream> = match &gpr.names {
        Some(names) if !names.is_empty() => names
            .iter()
            .map(|n| {
                let ident = format_ident!("{n}");
                quote! { #ident }
            })
            .collect(),
        _ => {
            let prefix = gpr.prefix.as_deref().unwrap_or("R");
            (0..gpr_count)
                .map(|i| {
                    let ident = format_ident!("{prefix}{i}");
                    quote! { #ident }
                })
                .collect()
        }
    };

    // Collect float register variants from xmm/float group
    let fpr_group = model.reg.get("xmm").or_else(|| model.reg.get("float"));
    let fpr_offset = gpr_variants.len() as u8;
    let fpr_variants: Vec<TokenStream> = match fpr_group.and_then(|g| g.names.as_ref()) {
        Some(names) if !names.is_empty() => names
            .iter()
            .map(|n| {
                let ident = format_ident!("{n}");
                quote! { #ident }
            })
            .collect(),
        _ => Vec::new(),
    };
    let fpr_count = fpr_variants.len();

    // Combine all variants
    let mut all_variants = gpr_variants.clone();
    all_variants.extend(fpr_variants.clone());

    // GPR index arms
    let gpr_idx_arms: Vec<TokenStream> = gpr_variants
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let i = i as u8;
            quote! { #i => Reg::#v }
        })
        .collect();

    // FPR index arms
    let fpr_idx_arms: Vec<TokenStream> = fpr_variants
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let i = fpr_offset + i as u8;
            quote! { #i => Reg::#v }
        })
        .collect();

    let first = &all_variants[0];

    quote! {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum Reg { #(#all_variants),* }

        impl forge_ir::PhysReg for Reg {
            fn to_index(self) -> u8 { self as u8 }
            fn class(self) -> forge_ir::RegClass {
                let idx = self as u8;
                if idx >= #fpr_offset && #fpr_count > 0 {
                    forge_ir::RegClass::Float
                } else {
                    forge_ir::RegClass::Int
                }
            }
            fn from_index(idx: u8, cls: forge_ir::RegClass) -> Self {
                match cls {
                    forge_ir::RegClass::Float => match idx.wrapping_add(#fpr_offset) {
                        #(#fpr_idx_arms ,)*
                        _ => Reg::#first,
                    },
                    _ => match idx {
                        #(#gpr_idx_arms ,)*
                        _ => Reg::#first,
                    },
                }
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
        // 指令寄存器字段为用户定义的物理寄存器类型（Reg）；XReg 为 API/lowering 层
        // 临时寄存器，经 InstPacket.xreg_map 映射到字段，分配器分配后回写。
        FieldType::Ireg | FieldType::Freg | FieldType::GprReg | FieldType::XmmReg => {
            quote! { Reg }
        }
        FieldType::MemRef => quote! { crate::prelude::MemRef },
        FieldType::I8 => quote! { i8 },
        FieldType::I16 => quote! { i16 },
        FieldType::I32 => quote! { i32 },
        FieldType::I64 => quote! { i64 },
        FieldType::U8 => quote! { u8 },
        FieldType::U16 => quote! { u16 },
        FieldType::U32 => quote! { u32 },
        FieldType::U64 => quote! { u64 },
        FieldType::F32 => quote! { f32 },
        FieldType::F64 => quote! { f64 },
        FieldType::BlockTarget => quote! { i64 },
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
    let mut use_c_arms: Vec<TokenStream> = Vec::new();
    let mut def_c_arms: Vec<TokenStream> = Vec::new();
    let mut reg_field_arms: Vec<TokenStream> = Vec::new();
    let mut set_reg_field_arms: Vec<TokenStream> = Vec::new();
    let mut branch_arms: Vec<TokenStream> = Vec::new();
    let mut call_arms: Vec<TokenStream> = Vec::new();
    let mut ret_arms: Vec<TokenStream> = Vec::new();
    let mut move_arms: Vec<TokenStream> = Vec::new();
    let mut side_effects_arms: Vec<TokenStream> = Vec::new();
    let mut effects_arms: Vec<TokenStream> = Vec::new();
    let mut branch_targets_arms: Vec<TokenStream> = Vec::new();

    for (inst_name, inst) in &model.inst {
        let vn = pascal_ident(inst_name);

        // uses: VReg type + use role (explicit or inferred from name)
        let ufs: Vec<_> = inst
            .fields
            .iter()
            .filter(|f| {
                (f.field_type == FieldType::Ireg || f.field_type == FieldType::Freg)
                    && is_field_use(f)
            })
            .map(|f| format_ident!("{}", f.name))
            .collect();
        if ufs.is_empty() {
            use_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
            use_c_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
        } else {
            let clones: Vec<_> = ufs.iter().map(|f| quote! { #f.to_index() }).collect();
            use_arms
                .push(quote! { Inst::#vn { #(#ufs),*, .. } => smallvec::smallvec![#(#clones),*] });
            let any_uses: Vec<TokenStream> = ufs
                .iter()
                .map(|_| quote! { crate::machine::inst::OperandConstraint::Any })
                .collect();
            use_c_arms.push(quote! {
                Inst::#vn { #(#ufs),*, .. } => smallvec::smallvec![#(#any_uses),*]
            });
        }

        // defs: VReg type + def role (explicit or inferred from name)
        let dfs: Vec<_> = inst
            .fields
            .iter()
            .filter(|f| {
                (f.field_type == FieldType::Ireg || f.field_type == FieldType::Freg)
                    && is_field_def(f)
            })
            .map(|f| format_ident!("{}", f.name))
            .collect();
        if dfs.is_empty() {
            def_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
            def_c_arms.push(quote! { Inst::#vn { .. } => smallvec::smallvec![] });
        } else {
            let clones: Vec<_> = dfs.iter().map(|f| quote! { #f.to_index() }).collect();
            def_arms
                .push(quote! { Inst::#vn { #(#dfs),*, .. } => smallvec::smallvec![#(#clones),*] });
            let any_defs: Vec<TokenStream> = dfs
                .iter()
                .map(|_| quote! { crate::machine::inst::OperandConstraint::Any })
                .collect();
            def_c_arms.push(quote! {
                Inst::#vn { #(#dfs),*, .. } => smallvec::smallvec![#(#any_defs),*]
            });
        }

        // reg_field/set_reg_field：按 asm 模板的字段占位符序读写物理索引
        // （与指令包 xreg_map 的 map 调用序一致——lowering 也按模板序生成；
        // 不能用 inst.fields：serde 把 inline table 解析成 BTreeMap 字母序，
        // 导致 dest/base 等字段序错位，分配器回写与编码对不上）
        let field_tuples: Vec<(String, crate::model::FieldType)> = inst
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.field_type.clone()))
            .collect();
        let valid_names: std::collections::HashSet<&str> =
            inst.fields.iter().map(|f| f.name.as_str()).collect();
        let template_order =
            crate::asm_resolver::ParsedTemplate::parse(&inst.asm, &field_tuples).field_order();
        let reg_fields: Vec<_> = template_order
            .iter()
            .filter(|(name, ty)| {
                // 排除 asm 模板里的显示别名（如 riscv64 `sd rs2, {imm}({base})` 的 imm≠offset）
                matches!(ty, FieldType::Ireg | FieldType::Freg)
                    && valid_names.contains(name.as_str())
            })
            .collect();
        if reg_fields.is_empty() {
            // 无寄存器字段：不 push 兜底（兜底统一在 impl 的 match 末尾）
        } else {
            let mut rf_arms: Vec<TokenStream> = Vec::new();
            let mut sf_arms: Vec<TokenStream> = Vec::new();
            for (i, (fname, ftype)) in reg_fields.iter().enumerate() {
                let fi = format_ident!("{fname}");
                let cls = if *ftype == FieldType::Freg {
                    quote! { forge_ir::RegClass::FPR }
                } else {
                    quote! { forge_ir::RegClass::GPR }
                };
                rf_arms.push(quote! { (#i, Inst::#vn { #fi, .. }) => #fi.to_index() });
                sf_arms.push(quote! {
                    (#i, Inst::#vn { #fi, .. }) => {
                        *#fi = <Reg as forge_ir::PhysReg>::from_index(idx, #cls);
                    }
                });
            }
            reg_field_arms.push(quote! { #(#rf_arms),* });
            set_reg_field_arms.push(quote! { #(#sf_arms),* });
        }

        // is_move
        let upper = inst_name.to_uppercase();
        if (upper.starts_with("MOV_") || upper == "SD_MOV" || upper == "SD_FMOV")
            && dfs.len() == 1
            && ufs.len() == 1
        {
            let d = &dfs[0];
            let u = &ufs[0];
            move_arms
                .push(quote! { Inst::#vn { #d, #u, .. } => Some((#d.to_index(), #u.to_index())) });
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
            // clobbers() 保持恒空（有意的权衡）：
            // call 破坏 caller-saved（RAX/RCX/RDX/RSI/RDI/R8-R11/XMM0-15），但启用
            // clobber 处理（process_block 1.5 步 spill 被破坏的活跃 XReg）曾导致
            // test_jit_call_external_function AV——根因：发参 mov（arg_regs 的
            // RCX/RDX/R8/R9）的 XReg 在 call 点仍被视为活跃，clobber spill/reload
            // 时序与参数传递冲突。安全启用需要「参数传递 XReg 在 call 点不活跃」
            // 的活区间语义（发参 mov 后值已传 callee，无需保留）。
            // 当前无测试覆盖「call 后 caller-saved 中仍活跃 XReg」的场景，
            // 恒空不影响现有正确性；待 ABI 配置细化后启用。
        }
        if effects.iter().any(|e| e.as_str() == "Ret") {
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
                "Ret" => quote! { crate::prelude::EffectKind::Ret },
                "Call" => quote! { crate::prelude::EffectKind::Call },
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
                    quote! { crate::prelude::Block(*#f as u32) }
                })
                .collect();
            branch_targets_arms.push(
                quote! { Inst::#vn { #(#bt_fields),*, .. } => smallvec::smallvec![#(#bts),*] },
            );
        }
    }

    quote! {
        impl crate::prelude::MachineInst for Inst {
            fn uses(&self) -> smallvec::SmallVec<[u8;4]> {
                match self { #(#use_arms),* , Inst::Unknown(_) => smallvec::smallvec![] }
            }
            fn defs(&self) -> smallvec::SmallVec<[u8;2]> {
                match self { #(#def_arms),* , Inst::Unknown(_) => smallvec::smallvec![] }
            }
            fn use_constraints(&self) -> smallvec::SmallVec<[crate::machine::inst::OperandConstraint;4]> {
                match self { #(#use_c_arms),* , Inst::Unknown(_) => smallvec::smallvec![] }
            }
            fn def_constraints(&self) -> smallvec::SmallVec<[crate::machine::inst::OperandConstraint;2]> {
                match self { #(#def_c_arms),* , Inst::Unknown(_) => smallvec::smallvec![] }
            }
            fn effects(&self) -> smallvec::SmallVec<[crate::prelude::EffectKind;2]> {
                match self { #(#effects_arms),* , Inst::Unknown(_) => smallvec::smallvec![] }
            }
            fn is_branch(&self) -> bool {
                match self { #(#branch_arms,)* _ => false }
            }
            fn branch_targets(&self) -> smallvec::SmallVec<[crate::prelude::Block;2]> {
                match self { #(#branch_targets_arms),* , Inst::Unknown(_) => smallvec::smallvec![] }
            }
            fn is_call(&self) -> bool {
                match self { #(#call_arms,)* _ => false }
            }
            fn is_ret(&self) -> bool {
                match self { #(#ret_arms,)* _ => false }
            }
            fn clobbers(&self) -> &[u8] {
                match self { _ => &[] }
            }
            fn is_move(&self) -> Option<(u8, u8)> {
                match self { #(#move_arms,)* _ => None }
            }
            fn reg_field(&self, i: usize) -> u8 {
                match (i, self) { #(#reg_field_arms,)* _ => 0 }
            }
            fn set_reg_field(&mut self, i: usize, idx: u8) {
                match (i, self) { #(#set_reg_field_arms,)* _ => {} }
            }
            fn has_side_effects(&self) -> bool {
                self.is_call() || self.is_ret() || self.is_branch() || self.memory_access().is_some()
                    || match self { #(#side_effects_arms,)* _ => false }
            }
        }
    }

    // ============================================================
}
// emit_inst — instruction-level encoder
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
            Inst::#vn { .. } => {
                return Err(crate::prelude::CompileError::Emit("no encoding for ".into()));
            }
        });
    }

    arms.push(quote! {
        Inst::Unknown(_) => {
            return Err(crate::prelude::CompileError::Emit("unknown instruction".into()));
        }
    });

    // Prologue / Epilogue
    let prologue_body = gen_prologue_epilogue(model, true);
    let epilogue_body = gen_prologue_epilogue(model, false);

    Ok(quote! {
        pub fn emit_inst(
            inst: &Inst,
            rm: &crate::prelude::AllocResult,
            sink: &mut crate::prelude::CodeSink,
        ) -> Result<(), crate::prelude::CompileError> {
            use crate::encode::{BitField, pack_bits};
            let preg = |v: crate::prelude::XReg, m: &crate::prelude::AllocResult|
                m.resolve(v).map(|p| p.num).map_err(|e| crate::prelude::CompileError::RegAlloc(format!("{}", e)));
            match inst { #(#arms),* }
            Ok(())
        }

        pub fn emit_prologue_impl(fs: u32, rm: &crate::prelude::AllocResult, sink: &mut crate::prelude::CodeSink)
            -> Result<(), crate::prelude::CompileError>
        {
            let frame_size = fs;
            #prologue_body
            Ok(())
        }

        pub fn emit_epilogue_impl(fs: u32, rm: &crate::prelude::AllocResult, sink: &mut crate::prelude::CodeSink)
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
                    #(#inst_toks)*;
                    Ok(__pack)
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
                    #(#inst_toks)*;
                    Ok(__pack)
                }
            });
        }
    }
    Ok(cond_arms)
}

fn gen_lower_func(model: &IsaModel) -> Result<TokenStream, String> {
    if std::env::var("CF_LOWER_DBG").is_ok() {
        eprintln!(
            "LOWER KEYS ({}): {:?}",
            model.meta.name,
            model.lower.keys().collect::<Vec<_>>()
        );
    }
    let mut arms: Vec<TokenStream> = Vec::new();

    // 收集 Icmp 和 Fcmp 的子规则
    let mut icmp_cases: Vec<(String, &LowerRule)> = Vec::new();
    let mut fcmp_cases: Vec<(String, &LowerRule)> = Vec::new();
    let mut other_rules: Vec<(&String, &LowerRule)> = Vec::new();
    // Template-generated cases (owned, so we can expand $CC)
    let mut icmp_template_arms: Vec<(String, Vec<String>)> = Vec::new();
    let mut fcmp_template_arms: Vec<(String, Vec<String>)> = Vec::new();
    let mut atomic_cases: Vec<(String, LowerRule)> = Vec::new();

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
        } else if let Some(op) = key.strip_prefix("AtomicRmw.") {
            atomic_cases.push((op.to_string(), rule.clone()));
        } else {
            other_rules.push((key, rule));
        }
    }

    // 其他 opcode 规则
    let mut handled_opcodes: HashSet<String> = HashSet::new();
    for (key, rule) in &other_rules {
        // AtomicRmw is dispatched by its op via the AtomicRmw.<Op> sub-rules
        // (generated below), not by a single template.
        if *key == "AtomicRmw" && !atomic_cases.is_empty() {
            continue;
        }
        handled_opcodes.insert((*key).clone());
        let op_ident = format_ident!("{key}");

        // Type-classified Call lowering: when `[abi.call]` is configured,
        // generate a special arm that moves args into ABI registers by type
        // (integer → gpr[k], float → xmm[m]), emits the relocatable call, and
        // moves the return value back. This replaces the fixed arg0-arg7
        // template approach.
        if *key == "Call"
            && let Some(abi) = model.abi.as_ref()
            && let Some(call_cfg) = abi.call.as_ref()
            && let (Some(arg_mov), Some(arg_mov_f), Some(ret_mov), Some(ret_mov_f), Some(call_inst)) = (
                call_cfg.arg_mov.as_ref(),
                call_cfg.arg_mov_f.as_ref(),
                call_cfg.ret_mov.as_ref(),
                call_cfg.ret_mov_f.as_ref(),
                call_cfg.call_inst.as_ref(),
            )
        {
            let arg_mov = format_ident!("{arg_mov}");
            let arg_mov_f = format_ident!("{arg_mov_f}");
            let ret_mov = format_ident!("{ret_mov}");
            let ret_mov_f = format_ident!("{ret_mov_f}");
            let call_inst = format_ident!("{call_inst}");
            let call_field =
                format_ident!("{}", call_cfg.call_field.as_deref().unwrap_or("target"));
            let gpr_idents: Vec<_> = abi
                .arg_regs
                .gpr
                .iter()
                .map(|n| format_ident!("{n}"))
                .collect();
            let xmm_idents: Vec<_> = abi
                .arg_regs
                .xmm
                .iter()
                .map(|n| format_ident!("{n}"))
                .collect();
            let n_gpr = gpr_idents.len();
            let n_xmm = xmm_idents.len();
            let ret_gpr = abi
                .ret_regs
                .gpr
                .first()
                .map(|n| format_ident!("{n}"))
                .unwrap_or(format_ident!("RAX"));
            let ret_xmm = abi
                .ret_regs
                .xmm
                .first()
                .map(|n| format_ident!("{n}"))
                .unwrap_or(format_ident!("XMM0"));

            arms.push(quote! {
                crate::prelude::Opcode::Call { .. } => {
                    let __arg_gprs: [crate::prelude::Reg; #n_gpr] = [#(crate::prelude::Reg::#gpr_idents),*];
                    let __arg_xmms: [crate::prelude::Reg; #n_xmm] = [#(crate::prelude::Reg::#xmm_idents),*];
                    let mut __out = Vec::new();
                    let mut __gi = 0usize;
                    let mut __fi = 0usize;
                    // Move args into ABI registers by type; beyond the
                    // register count, extra args are dropped (stack args not
                    // yet lowered — recorded limitation).
                    for __arg in args.iter().copied() {
                        if __arg.class().is_fp() {
                            if __fi < #n_xmm {
                                let __reg = __arg_xmms[__fi]; __fi += 1;
                                __out.push(crate::prelude::Inst::#arg_mov_f { dest: __reg, src: __arg });
                            }
                        } else if __gi < #n_gpr {
                            let __reg = __arg_gprs[__gi]; __gi += 1;
                            __out.push(crate::prelude::Inst::#arg_mov { dest: __reg, src: __arg });
                        }
                    }
                    let __func = ctx.current_func_ref.map(|f| f.0 as i32).unwrap_or(0);
                    __out.push(crate::prelude::Inst::#call_inst { #call_field: __func });
                    let __rd = result.unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    if __rd.class().is_fp() {
                        __out.push(crate::prelude::Inst::#ret_mov_f { dest: __rd, src: crate::prelude::Reg::#ret_xmm });
                    } else {
                        __out.push(crate::prelude::Inst::#ret_mov { dest: __rd, src: crate::prelude::Reg::#ret_gpr });
                    }
                    Ok(__out)
                }
            });
            continue;
        }

        let lowering = crate::cst_codegen::gen_lowering_insts_cst(&rule.insts, model)?;
        let inst_toks = lowering.insts;
        let temp_toks = lowering.temps;

        // 检查是否需要 destructure index 字段 (Iconst/Fconst)
        let uses_const = rule
            .insts
            .iter()
            .any(|s| s.contains("iconst") || s.contains("fconst"));

        if uses_const {
            arms.push(quote! {
                crate::prelude::Opcode::#op_ident => {
                    let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let index: u32 = ctx.current_const_index;
                    #(#temp_toks);*;
                    #(#inst_toks)*;
                    Ok(__pack)
                }
            });
        } else {
            arms.push(quote! {
                crate::prelude::Opcode::#op_ident { .. } => {
                    let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    #(#temp_toks);*;
                    #(#inst_toks)*;
                    Ok(__pack)
                }
            });
        }
    }

    // ── AtomicRmw dispatched by op (AtomicRmw.<Op> sub-rules) ──
    if !atomic_cases.is_empty() {
        let mut op_arms: Vec<TokenStream> = Vec::new();
        for (op_name, rule) in &atomic_cases {
            let op_ident = format_ident!("{op_name}");
            let lowering = crate::cst_codegen::gen_lowering_insts_cst(&rule.insts, model)?;
            let temp_toks = lowering.temps;
            let inst_toks = lowering.insts;
            op_arms.push(quote! {
                crate::prelude::AtomicRmwOp::#op_ident => { #(#temp_toks);*; #(#inst_toks)*; Ok(__pack) }
            });
        }
        arms.push(quote! {
            crate::prelude::Opcode::AtomicRmw { .. } => {
                let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                match ctx.current_atomic_op.unwrap_or(crate::prelude::AtomicRmwOp::Add) {
                    #(#op_arms),*
                    _ => Err(crate::prelude::CompileError::Unsupported("atomic op".into()))
                }
            }
        });
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
            let lowering = crate::cst_codegen::gen_lowering_insts_cst(&rule.insts, model)?;
            let temp_toks = lowering.temps;
            let inst_toks = lowering.insts;
            cond_arms.push(quote! {
                crate::prelude::IntCC::#cc_ident => { #(#temp_toks);*; #(#inst_toks)*; Ok(__pack) }
            });
        }
        for (cond_name, insts) in &icmp_template_arms {
            let cc_ident = format_ident!("{cond_name}");
            let inst_toks = gen_lower_insts(insts, model)?;
            cond_arms.push(quote! {
                crate::prelude::IntCC::#cc_ident => { #(#inst_toks)*; Ok(__pack) }
            });
        }
        cond_arms.extend(default_icmp_arms);
        arms.push(quote! {
            crate::prelude::Opcode::Icmp { cond } => {
                let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
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
            let lowering = crate::cst_codegen::gen_lowering_insts_cst(&rule.insts, model)?;
            let temp_toks = lowering.temps;
            let inst_toks = lowering.insts;
            cond_arms.push(quote! {
                crate::prelude::FloatCC::#cc_ident => { #(#temp_toks);*; #(#inst_toks)*; Ok(__pack) }
            });
        }
        for (cond_name, insts) in &fcmp_template_arms {
            let cc_ident = format_ident!("{cond_name}");
            let inst_toks = gen_lower_insts(insts, model)?;
            cond_arms.push(quote! {
                crate::prelude::FloatCC::#cc_ident => { #(#inst_toks)*; Ok(__pack) }
            });
        }
        cond_arms.extend(default_fcmp_arms);
        arms.push(quote! {
            crate::prelude::Opcode::Fcmp { cond, .. } => {
                let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
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
                        crate::prelude::Opcode::#op_ident => {
                            let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                            let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                            let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                            let index: u32 = ctx.current_const_index;
                            #(#inst_toks)*;
                            Ok(__pack)
                        }
                    });
                } else {
                    arms.push(quote! {
                        crate::prelude::Opcode::#op_ident { .. } => {
                            let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                            let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                            let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                            #(#inst_toks)*;
                            Ok(__pack)
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
            args: &[crate::prelude::XReg],
            results: &[crate::prelude::XReg],
            ctx: &mut crate::prelude::LowerCtx,
        ) -> Result<crate::prelude::InstPacket<Inst>, crate::prelude::CompileError> {
            let mut __pack = crate::prelude::InstPacket::new();
            match op { #(#arms),* }
        }
    })
}

/// Expand `$CC` placeholder in template insts with the given condition value.
fn expand_cc(template: &[String], cc_val: &str) -> Vec<String> {
    template.iter().map(|s| s.replace("$CC", cc_val)).collect()
}

/// Parsed result: (instruction_name, [(field_name, arg_value)]).
/// Generate Inst TokenStreams for default lowering (SD_* instructions).
///
/// Used only for the built-in standard-instruction lowering fallback
/// (`no_default_lowering = false`). Each string is `"INST_KEY op1, op2, ..."` —
/// the key is looked up directly in `model.inst`.
fn gen_lower_insts(insts: &[String], model: &IsaModel) -> Result<Vec<TokenStream>, String> {
    let mut result = Vec::new();
    for asm_str in insts {
        let trimmed = asm_str.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Split: "SD_MOV rd, rs1" → key="SD_MOV", rest="rd, rs1"
        let (key, rest) = match trimmed.find(char::is_whitespace) {
            Some(pos) => (trimmed[..pos].trim(), trimmed[pos..].trim()),
            None => (trimmed, ""),
        };
        let operands: Vec<&str> = if rest.is_empty() {
            Vec::new()
        } else {
            rest.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect()
        };

        let inst_def = model.inst.get(key).ok_or_else(|| {
            format!(
                "default lowering: instruction '{}' not found in [inst.*]",
                key
            )
        })?;
        let ivn = pascal_ident(key);
        let mut field_exprs: Vec<TokenStream> = Vec::new();
        let scratch = model.abi.as_ref().map(|a| &a.scratch);

        let mut op_idx = 0;
        let mut reg_map_calls: Vec<TokenStream> = Vec::new();
        for field in &inst_def.fields {
            if matches!(field.field_type, crate::model::FieldType::Opsize) {
                continue; // implicit, uses default value
            }
            let arg_val = if op_idx < operands.len() {
                operands[op_idx]
            } else {
                return Err(format!(
                    "default lowering: missing operand for field '{}' in '{}'",
                    field.name, trimmed
                ));
            };
            op_idx += 1;

            let fi = format_ident!("{}", field.name);
            // 寄存器字段（Ireg/Freg）以默认物理 Reg 占位，XReg 记录到 xreg_map
            if matches!(
                field.field_type,
                crate::model::FieldType::Ireg | crate::model::FieldType::Freg
            ) {
                let xreg_expr = lowering_arg_expr(arg_val, &field.field_type, scratch, Some(model));
                let cls = if matches!(field.field_type, crate::model::FieldType::Freg) {
                    quote! { crate::prelude::RegClass::FPR }
                } else {
                    quote! { crate::prelude::RegClass::GPR }
                };
                field_exprs.push(quote! {
                    #fi: <Reg as forge_ir::PhysReg>::from_index(0, #cls)
                });
                reg_map_calls.push(quote! {
                    __pack.map_reg_field(#xreg_expr, __idx);
                });
            } else {
                let expr = lowering_arg_expr(arg_val, &field.field_type, scratch, Some(model));
                field_exprs.push(quote! { #fi: #expr });
            }
        }

        // Fill missing fields with defaults
        for field in &inst_def.fields {
            if !field_exprs.is_empty()
                && inst_def
                    .fields
                    .iter()
                    .position(|f| f.name == field.name)
                    .is_some_and(|pos| {
                        pos < op_idx
                            + inst_def
                                .fields
                                .iter()
                                .filter(|f| matches!(f.field_type, crate::model::FieldType::Opsize))
                                .count()
                    })
            {
                continue;
            }
            // Check if we already emitted this field
            let already_emitted = inst_def
                .fields
                .iter()
                .filter(|f| !matches!(f.field_type, crate::model::FieldType::Opsize))
                .take(op_idx)
                .any(|f| f.name == field.name);
            if !already_emitted {
                let fi = format_ident!("{}", field.name);
                let default = default_for_type(&field.field_type);
                field_exprs.push(quote! { #fi: #default });
            }
        }

        // 记录寄存器字段的 XReg → (指令索引, 字段顺序) 映射（与 cst 路径一致）
        result.push(quote! {
            {
                let __idx = __pack.push_inst(Inst::#ivn { #(#field_exprs),* });
                #(#reg_map_calls)*
            }
        });
    }
    Ok(result)
}

#[derive(Clone, Copy)]
pub(crate) enum GenMode {
    Lowering,
    Emit,
}

/// Resolve the float return VReg from ABI config.
///
/// Returns the VReg index precolored to the first XMM return register,
/// or 100 as a legacy fallback.
fn resolve_float_ret_vreg(model: &IsaModel) -> u32 {
    if let Some(ref abi) = model.abi
        && let Some(first_xmm) = abi.ret_regs.xmm.first()
    {
        // Walk precolor map to find the VReg bound to this physical register
        for (vreg_key, phys_name) in &abi.precolor {
            if phys_name.eq_ignore_ascii_case(first_xmm)
                && let Ok(idx) = vreg_key
                    .trim_start_matches(|c: char| !c.is_ascii_digit())
                    .parse::<u32>()
            {
                return idx;
            }
        }
    }
    // Legacy fallback
    100
}

/// Resolve a physical register name to its precolored VReg index.
///
/// Looks up `[abi.precolor]` to find which VReg is bound to the given physical register name.
/// e.g. `"RAX"` → VReg(0), `"R10"` → VReg(96)
fn resolve_precolor_vreg(reg_name: &str, model: &IsaModel) -> Option<u32> {
    model.abi.as_ref()?.precolor.iter().find_map(|(k, v)| {
        if v.eq_ignore_ascii_case(reg_name) {
            let idx: u32 = k
                .trim_start_matches(|c: char| !c.is_ascii_digit())
                .parse()
                .ok()?;
            Some(idx)
        } else {
            None
        }
    })
}

/// 将 lowering 规则中的参数字符串转为表达式。
///
/// Supports physical register names (`RAX`, `R10`, `XMM0`) — resolves to the precolored
/// VReg index for Ireg/Freg fields, or `Reg::NAME` for GprReg/XmmReg fields.
pub(crate) fn lowering_arg_expr(
    val: &str,
    ft: &FieldType,
    scratch: Option<&std::collections::BTreeMap<String, u32>>,
    model: Option<&IsaModel>,
) -> TokenStream {
    // 0. Constant pool inline: `{const 42}` or `{const 3.14}`
    //    Emits the literal value directly without constant pool lookup.
    if let Some(inner) = val
        .strip_prefix("{const ")
        .and_then(|s| s.strip_suffix('}'))
    {
        let inner = inner.trim();
        return match ft {
            FieldType::F32 | FieldType::F64 => {
                let n: f64 = inner.parse().unwrap_or(0.0);
                match ft {
                    FieldType::F32 => quote! { #n as f32 },
                    _ => quote! { #n as f64 },
                }
            }
            FieldType::I8
            | FieldType::I16
            | FieldType::I32
            | FieldType::I64
            | FieldType::U8
            | FieldType::U16
            | FieldType::U32
            | FieldType::U64
            | FieldType::CondCode
            | FieldType::Opsize
            | FieldType::BlockTarget => {
                // Support both hex (0xFF) and decimal
                let n: i64 = if inner.starts_with("0x") || inner.starts_with("0X") {
                    i64::from_str_radix(&inner[2..], 16).unwrap_or(0)
                } else {
                    inner.parse::<i64>().unwrap_or(0)
                };
                match ft {
                    FieldType::I8 | FieldType::U8 | FieldType::CondCode | FieldType::Opsize => {
                        quote! { #n as u8 }
                    }
                    FieldType::I16 | FieldType::U16 => quote! { #n as i16 },
                    FieldType::I32 => quote! { #n as i32 },
                    FieldType::U32 => quote! { #n as u32 },
                    _ => quote! { #n as i64 },
                }
            }
            _ => {
                // For register types etc., fall through to normal handling
                let n: i64 = inner.parse().unwrap_or(0);
                quote! { #n as i64 }
            }
        };
    }

    // 1. Scratch register aliases: symbolic name → VReg index
    //    e.g. TMP0 → VReg(96), CL → VReg(97)
    if let Some(&idx) = scratch.and_then(|map| map.get(val)) {
        return quote! { crate::prelude::XReg::new(#idx as u32, crate::prelude::RegClass::GPR, 8) };
    }

    // 2. Physical register name lookup (P3d: uses shared lookup_reg_in_model)
    if let Some(model) = model {
        match lookup_reg_in_model(val, model) {
            RegLookup::Named { ref name, .. } | RegLookup::Prefixed { ref name, .. } => {
                if matches!(ft, FieldType::GprReg | FieldType::XmmReg) {
                    let ri = format_ident!("{}", name);
                    return quote! { Reg::#ri };
                }
                // For virtual register fields (Ireg/Freg), look up the precolored VReg index
                let vreg_idx = resolve_precolor_vreg(val, model).unwrap_or_else(|| {
                    // fallback: try numeric parse from name
                    name.trim_start_matches(|c: char| !c.is_ascii_digit())
                        .parse::<u32>()
                        .unwrap_or(0)
                });
                let cls = if name.to_uppercase().contains("XMM")
                    || name.to_uppercase().contains("YMM")
                    || name.to_uppercase().contains("ZMM")
                    || name.to_uppercase().starts_with('F')
                {
                    quote! { crate::prelude::RegClass::FPR }
                } else {
                    quote! { crate::prelude::RegClass::GPR }
                };
                return quote! { crate::prelude::XReg::new(#vreg_idx as u32, #cls, 8) };
            }
            RegLookup::NotFound => {}
        }
    }

    // 3. Position-dependent lowering variables
    match val {
        "rd" => quote! { rd },
        "r2" => quote! { r2 },
        "rs1" => quote! { rs1 },
        "rs2" => quote! { rs2 },
        "rs3" => {
            quote! { args.get(2).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r }) }
        }
        // Call argument registers (fixed positions). Beyond the register
        // count the operand falls back to VReg(0) (harmless extra move).
        "arg0" => {
            quote! { args.get(0).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r }) }
        }
        "arg1" => {
            quote! { args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r }) }
        }
        "arg2" => {
            quote! { args.get(2).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r }) }
        }
        "arg3" => {
            quote! { args.get(3).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r }) }
        }
        "arg4" => {
            quote! { args.get(4).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r }) }
        }
        "arg5" => {
            quote! { args.get(5).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r }) }
        }
        "arg6" => {
            quote! { args.get(6).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r }) }
        }
        "arg7" => {
            quote! { args.get(7).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r }) }
        }
        // Cross-function call target: the FuncRef number from the IR Call's
        // `Immediate::Func`; the emitted machine instruction carries it and a
        // `@call_reloc` encodes a relocation against the "@N" symbol.
        "func" => {
            let cast = match ft {
                FieldType::I8 | FieldType::U8 | FieldType::CondCode => quote! { as u8 },
                FieldType::I16 | FieldType::U16 => quote! { as i16 },
                FieldType::I32 | FieldType::U32 => quote! { as i32 },
                _ => quote! { as i64 },
            };
            quote! { ctx.current_func_ref.map(|f| f.0 #cast).unwrap_or(0) }
        }
        "global" => {
            let cast = match ft {
                FieldType::I8 | FieldType::U8 | FieldType::CondCode => quote! { as u8 },
                FieldType::I16 | FieldType::U16 => quote! { as i16 },
                FieldType::I32 | FieldType::U32 => quote! { as i32 },
                _ => quote! { as i64 },
            };
            quote! { ctx.current_global.map(|g| g.0 #cast).unwrap_or(0) }
        }
        // StackAddr 的帧偏移（Immediate::Int）。
        "offset" => quote! { ctx.current_offset },
        "zero" => quote! { ctx.alloc_zero_vreg() },
        "iconst" => {
            quote! { ctx.constant_pool.as_ref().and_then(|p| p.resolve_int(crate::prelude::ConstId(index))).unwrap_or(0) as i64 }
        }
        "fconst" => {
            quote! { ctx.constant_pool.as_ref().and_then(|p| p.resolve_float(crate::prelude::ConstId(index))).map(|b| b as i64).unwrap_or(0) }
        }
        // 4. %name temporary VReg — expands to `__vreg_<name>` variable (declared by gen_lowering_insts_cst)
        s if s.starts_with('%') => {
            let name = &s[1..];
            let vi = format_ident!("__vreg_{name}");
            quote! { #vi }
        }
        // 5. VReg(N) literal (retained for transitional compatibility, deprecated)
        s if s.starts_with("VReg(") || s.starts_with("vreg(") => {
            let n: u32 = s
                .trim_start_matches(|c: char| !c.is_ascii_digit())
                .trim_end_matches(')')
                .parse()
                .unwrap_or(0);
            let cls = match ft {
                FieldType::Freg => quote! { crate::prelude::RegClass::FPR },
                _ => quote! { crate::prelude::RegClass::GPR },
            };
            quote! { crate::prelude::XReg::new(#n as u32, #cls, 8) }
        }
        // 5. Numeric literals
        s if s.starts_with("0x") || s.starts_with("0X") => {
            let n = u64::from_str_radix(&s[2..], 16).unwrap_or(0);
            match ft {
                FieldType::I8 | FieldType::U8 | FieldType::CondCode => quote! { #n as u8 },
                FieldType::I16 | FieldType::U16 => quote! { #n as i16 },
                FieldType::I32 => quote! { #n as i32 },
                FieldType::U32 => quote! { #n as u32 },
                FieldType::I64 => quote! { #n as i64 },
                FieldType::U64 => quote! { #n as i64 },
                FieldType::F32 => quote! { #n as f32 },
                FieldType::F64 => quote! { #n as f64 },
                _ => quote! { #n as i64 },
            }
        }
        s => {
            // 尝试解析为十进制数值
            if let Ok(n) = s.parse::<i64>() {
                match ft {
                    FieldType::I8 | FieldType::U8 | FieldType::CondCode => quote! { #n as u8 },
                    FieldType::I16 | FieldType::U16 => quote! { #n as i16 },
                    FieldType::I32 => quote! { #n as i32 },
                    FieldType::U32 => quote! { #n as u32 },
                    FieldType::I64 => quote! { #n as i64 },
                    FieldType::U64 => quote! { #n as i64 },
                    _ => quote! { #n as i64 },
                }
            } else {
                // 当作变量引用 (val, val2, cond, target, true_block, false_block)
                let vi = format_ident!("{s}");
                quote! { #vi }
            }
        }
    }
}

pub(crate) fn default_for_type(ft: &FieldType) -> TokenStream {
    match ft {
        FieldType::Ireg | FieldType::Freg => {
            quote! { <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::GPR) }
        }
        FieldType::I8 => quote! { 0i8 },
        FieldType::I16 => quote! { 0i16 },
        FieldType::I32 => quote! { 0i32 },
        FieldType::I64 => quote! { 0i64 },
        FieldType::U8 => quote! { 0u8 },
        FieldType::U16 => quote! { 0u16 },
        FieldType::U32 => quote! { 0u32 },
        FieldType::U64 => quote! { 0u64 },
        FieldType::F32 => quote! { 0.0f32 },
        FieldType::F64 => quote! { 0.0f64 },
        FieldType::GprReg | FieldType::XmmReg => {
            quote! { Reg::from_index(0, forge_ir::RegClass::Int) }
        }
        FieldType::MemRef => quote! { crate::prelude::MemRef::default() },
        FieldType::BlockTarget => quote! { 0i64 },
        FieldType::CondCode => quote! { 0u8 },
        FieldType::Opsize => {
            // 默认 64 位（emit_prologue/epilogue 无 ctx；x86 默认操作数宽度）
            quote! { 64u8 }
        }
    }
}

/// 将 emit 上下文中的参数字符串转为表达式。
/// 与 `lowering_arg_expr` 不同：emit 上下文支持寄存器名 (RBP, RSP, ...) 和 `frame_size`。
pub(crate) fn emit_arg_expr(val: &str, ft: &FieldType, model: &IsaModel) -> TokenStream {
    // frame_size: the local variable bound in emit_prologue/emit_epilogue impl
    if val == "frame_size" {
        return match ft {
            FieldType::I8 | FieldType::U8 => quote! { frame_size as u8 },
            FieldType::I16 | FieldType::U16 => quote! { frame_size as i16 },
            FieldType::I32 => quote! { frame_size as i32 },
            FieldType::U32 => quote! { frame_size },
            FieldType::I64 => quote! { frame_size as i64 },
            FieldType::U64 => quote! { frame_size as i64 },
            _ => quote! { frame_size as i64 },
        };
    }

    // Named registers: use shared lookup helper (P3d dedup)
    match lookup_reg_in_model(val, model) {
        RegLookup::Named { ref name, index } | RegLookup::Prefixed { ref name, index } => {
            if matches!(ft, FieldType::GprReg | FieldType::XmmReg) {
                let ri = syn::Ident::new(name, proc_macro2::Span::call_site());
                return quote! { Reg::#ri };
            }
            let cls = if matches!(ft, FieldType::Freg) {
                quote! { forge_ir::RegClass::FPR }
            } else {
                quote! { forge_ir::RegClass::GPR }
            };
            return quote! { <Reg as forge_ir::PhysReg>::from_index(#index as u8, #cls) };
        }
        RegLookup::NotFound => {}
    }

    // Scratch register aliases
    if let Some(scratch) = model.abi.as_ref().and_then(|a| a.scratch.get(val)) {
        return quote! { <Reg as forge_ir::PhysReg>::from_index(#scratch as u8, forge_ir::RegClass::GPR) };
    }

    // Hex literal
    if val.starts_with("0x") || val.starts_with("0X") {
        let n = u64::from_str_radix(&val[2..], 16).unwrap_or(0);
        return match ft {
            FieldType::I8 | FieldType::U8 | FieldType::CondCode => quote! { #n as u8 },
            FieldType::I16 | FieldType::U16 => quote! { #n as i16 },
            FieldType::I32 => quote! { #n as i32 },
            FieldType::U32 => quote! { #n as u32 },
            FieldType::I64 => quote! { #n as i64 },
            FieldType::U64 => quote! { #n as i64 },
            FieldType::F32 => quote! { #n as f32 },
            FieldType::F64 => quote! { #n as f64 },
            _ => quote! { #n as i64 },
        };
    }

    // Decimal literal
    if let Ok(n) = val.parse::<i64>() {
        return match ft {
            FieldType::I8 | FieldType::U8 | FieldType::CondCode => quote! { #n as u8 },
            FieldType::I16 | FieldType::U16 => quote! { #n as i16 },
            FieldType::I32 => quote! { #n as i32 },
            FieldType::U32 => quote! { #n as u32 },
            FieldType::I64 => quote! { #n as i64 },
            FieldType::U64 => quote! { #n as i64 },
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

        // Regular instruction — parse "INST_KEY op1, op2, ..." into (key, [(field, value)])
        let (inst_name, field_args) = {
            let (key, rest) = match trimmed.find(char::is_whitespace) {
                Some(pos) => (trimmed[..pos].trim(), trimmed[pos..].trim()),
                None => (trimmed, ""),
            };
            let operands: Vec<&str> = if rest.is_empty() {
                Vec::new()
            } else {
                rest.split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect()
            };
            let inst_def = match model.inst.get(key) {
                Some(d) => d,
                None => return quote! { const _PARSE_ERROR: &str = "emit: unknown instruction"; },
            };
            let mut field_args: Vec<(&str, &str)> = Vec::new();
            let mut op_idx = 0;
            for field in &inst_def.fields {
                if matches!(field.field_type, crate::model::FieldType::Opsize) {
                    continue;
                }
                if op_idx < operands.len() {
                    field_args.push((field.name.as_str(), operands[op_idx]));
                    op_idx += 1;
                }
            }
            (key.to_string(), field_args)
        };
        let inst_def = &model.inst[&inst_name];
        let ivn = pascal_ident(&inst_name);
        let mut field_exprs: Vec<TokenStream> = Vec::new();

        for (field_name, arg_val) in &field_args {
            let field = inst_def
                .fields
                .iter()
                .find(|f| &f.name == field_name)
                .expect("field from field_args should exist in inst_def.fields");
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

    let gpr = get_gpr_group(model);
    let names = gpr.names.as_ref();

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
        pascal_ident("PUSH_Gpr")
    } else {
        pascal_ident("POP_Gpr")
    };

    // For-loop using Reg enum values — bypasses AllocResult entirely
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

    let opsize_default: u8 = model
        .dyn_types
        .get("opsize")
        .and_then(|d| d.default)
        .unwrap_or(64);

    let gpr = get_gpr_group(model);
    let names = gpr.names.as_ref();

    let mut stmts: Vec<TokenStream> = Vec::new();

    // Type-classified parameter receipt (requires `[abi.call].ret_mov_f`):
    // integer params move from gpr[k], float params from xmm[m].
    let fp_mov = abi
        .call
        .as_ref()
        .and_then(|c| c.ret_mov_f.as_ref())
        .map(|n| pascal_ident(n));
    let xmm_names: &[String] = &abi.arg_regs.xmm;

    if let (Some(fp_mov), false) = (&fp_mov, xmm_names.is_empty()) {
        let fp_mov = fp_mov.clone();
        let xmm_group = model.reg.get("xmm");
        let xmm_src_idents: Vec<_> = xmm_names
            .iter()
            .map(|reg_name| {
                let src_name = xmm_group
                    .and_then(|g| g.names.as_ref())
                    .and_then(|nlist| nlist.iter().find(|n| n.eq_ignore_ascii_case(reg_name)))
                    .cloned()
                    .unwrap_or_else(|| reg_name.to_string());
                syn::Ident::new(&src_name, proc_macro2::Span::call_site())
            })
            .collect();
        let gpr_src_idents: Vec<_> = arg_reg_names
            .iter()
            .map(|reg_name| {
                let src_name = names
                    .and_then(|nlist| nlist.iter().find(|n| n.eq_ignore_ascii_case(reg_name)))
                    .cloned()
                    .unwrap_or_else(|| reg_name.to_string());
                syn::Ident::new(&src_name, proc_macro2::Span::call_site())
            })
            .collect();
        let n_gpr = gpr_src_idents.len();
        let n_xmm = xmm_src_idents.len();

        stmts.push(quote! {
            let mut __gi = 0usize;
            let mut __fi = 0usize;
            for (__i, &pv) in rm.param_vregs.iter().enumerate() {
                // Skip if dest XReg was dead-code eliminated (not in allocated regs)
                if !rm.assignments.contains_key(&pv) {
                    continue;
                }
                // 分配后：dest 字段为物理 Reg（用户定义寄存器类型）
                let __dest = match rm.preg(pv) {
                    Some(p) => <Reg as forge_ir::PhysReg>::from_index(p.num, p.class),
                    None => continue,
                };
                let __is_float = rm.param_is_float.get(__i).copied().unwrap_or(false);
                if __is_float {
                    if __fi < #n_xmm {
                        let __reg = [#(Reg::#xmm_src_idents),*][__fi];
                        __fi += 1;
                        if let Err(e) = emit_inst(&Inst::#fp_mov { dest: __dest, src: __reg }, rm, sink) {
                            return Err(e);
                        }
                    }
                } else if __gi < #n_gpr {
                    let __reg = [#(Reg::#gpr_src_idents),*][__gi];
                    __gi += 1;
                    // i32/u32 参数符号扩展收参（movsxd）；i64 直接 mov。
                    let __is_32 = rm.param_is_32.get(__i).copied().unwrap_or(false);
                    if __is_32 {
                        if let Err(e) = emit_inst(&Inst::MovsxdRGpr { dest: __dest, src: __reg }, rm, sink) {
                            return Err(e);
                        }
                    } else if let Err(e) = emit_inst(&Inst::MovRm8R64 { dest: __dest, src: __reg, opsize: #opsize_default }, rm, sink) {
                        return Err(e);
                    }
                }
            }
        });
    } else {
        // Original GPR-only parameter receipt (no `[abi.call]` float config).
        for (i, reg_name) in arg_reg_names.iter().enumerate() {
            let src_name = names
                .and_then(|nlist| nlist.iter().find(|n| n.eq_ignore_ascii_case(reg_name)))
                .cloned()
                .unwrap_or_else(|| reg_name.to_string());
            let src_ident = syn::Ident::new(&src_name, proc_macro2::Span::call_site());

            stmts.push(quote! {
                if let Some(&pv) = rm.param_vregs.get(#i) {
                    // Skip if dest XReg was dead-code eliminated (not in allocated regs)
                    if let Some(__preg) = rm.preg(pv) {
                        let __dest = <Reg as forge_ir::PhysReg>::from_index(__preg.num, __preg.class);
                        if let Err(e) = emit_inst(&Inst::MovRm8R64 { dest: __dest, src: Reg::#src_ident, opsize: #opsize_default }, rm, sink) {
                            return Err(e);
                        }
                    }
                }
            });
        }
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
        // P0a: Removed hardcoded x86 fallback. Every ISA must define encoding for frame alloc/free.
        return quote! {{
            let _ = (frame_size, rm, sink);
            compile_error!(concat!("ISA \"", #inst_name, "\" has no encoding — required for @frame_alloc/@frame_free. Add [inst.", #inst_name, "] with an encoding string."));
        }};
    }

    // Look up SP register name from ABI config (e.g., "RSP" for x86, "SP" for AArch64).
    let sp_name = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .map(|f| f.sp.as_str())
        .unwrap_or("RSP");
    let sp_ident = syn::Ident::new(sp_name, proc_macro2::Span::call_site());

    // Check if this ISA needs the immediate negated for SUB (e.g., RISC-V ADDI with signed imm12).
    let neg_alloc = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .map(|f| f.neg_alloc_imm)
        .unwrap_or(false);

    let imm_expr = if is_alloc && neg_alloc {
        // RISC-V: ADDI uses 12-bit signed immediate; negate for subtract.
        quote! { frame_size.wrapping_neg() }
    } else {
        quote! { frame_size }
    };

    quote! {
        if frame_size > 0 {
            if let Err(e) = emit_inst(&Inst::#ivn {
                dest: Reg::#sp_ident,
                imm: #imm_expr,
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

    // Resolve float return VReg from ABI (was hardcoded VReg(100))
    // 具体数值在使用处（gen_default_lowering/用户 Return 臂）经 resolve_float_ret_vreg 取用
    let _float_ret_vreg = resolve_float_ret_vreg(model);

    for (term_name, rule) in &model.lower_term {
        let lowering = crate::cst_codegen::gen_lowering_insts_cst(&rule.insts, model)?;
        let temp_toks = lowering.temps;
        let inst_toks = lowering.insts;

        match term_name.as_str() {
            "Return" => {
                // float return: use ABI-derived float return VReg (resolved from [abi.ret_regs.xmm])
                let has_sd_fmov = model.inst.contains_key("SD_FMOV");
                let val_code = quote! {
                    let val = values.first().copied()
                        .and_then(|x| v.get(&x)).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let val2 = values.get(1).copied()
                        .and_then(|x| v.get(&x)).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                };
                if has_sd_fmov {
                    let ret_idx = resolve_float_ret_vreg(model);
                    let ret_idx_lit =
                        syn::LitInt::new(&ret_idx.to_string(), proc_macro2::Span::call_site());
                    arms.push(quote! {
                        crate::prelude::Terminator::Return { values } => {
                            #val_code
                            if ctx.is_float_return {
                                {
                                    // SdFmov dest=返回FPR（字段0 预着色 XReg）、src=val（字段1）
                                    let __idx = __pack.push_inst(Inst::SdFmov { dest: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::FPR), src: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::FPR) });
                                    __pack.map_reg_field(crate::prelude::XReg::new(#ret_idx_lit, forge_ir::RegClass::Float, 8), __idx);
                                    __pack.map_reg_field(val, __idx);
                                    #(#inst_toks)*;
                                    Ok(__pack)
                                }
                            } else {
                                #(#temp_toks);*;
                                #(#inst_toks)*;
                                Ok(__pack)
                            }
                        }
                    });
                } else {
                    arms.push(quote! {
                        crate::prelude::Terminator::Return { values } => {
                            #val_code
                            #(#temp_toks);*;
                            #(#inst_toks)*;
                            Ok(__pack)
                        }
                    });
                }
            }
            "Jump" => {
                arms.push(quote! {
                    crate::prelude::Terminator::Jump { target, .. } => {
                        let target = target.0 as i64;
                        #(#temp_toks);*;
                        #(#inst_toks)*;
                        Ok(__pack)
                    }
                });
            }
            "Branch" => {
                arms.push(quote! {
                    crate::prelude::Terminator::Branch { cond: cond_val, then_block, else_block, .. } => {
                        let cond = v.get(cond_val).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                        let true_block = then_block.0 as i64;
                        let false_block = else_block.0 as i64;
                        #(#temp_toks);*;
                        #(#inst_toks)*;
                        Ok(__pack)
                    }
                });
            }
            "Unreachable" => {
                arms.push(quote! {
                    crate::prelude::Terminator::Unreachable => {
                        #(#temp_toks);*;
                        #(#inst_toks)*;
                        Ok(__pack)
                    }
                });
            }
            "Switch" => {
                arms.push(quote! {
                    crate::prelude::Terminator::Switch { discriminant, default_block, .. } => {
                        let discriminant = v.get(discriminant).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                        let default_block = default_block.0 as i64;
                        #(#temp_toks);*;
                        #(#inst_toks)*;
                        Ok(__pack)
                    }
                });
            }
            _ => {
                let tn = format_ident!("{term_name}");
                arms.push(quote! {
                    crate::prelude::Terminator::#tn { .. } => {
                        #(#temp_toks);*;
                        #(#inst_toks)*;
                        Ok(__pack)
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
                    let ret_idx = resolve_float_ret_vreg(model);
                    let ret_idx_lit =
                        syn::LitInt::new(&ret_idx.to_string(), proc_macro2::Span::call_site());
                    arms.push(quote! {
                        crate::prelude::Terminator::Return { values } => {
                            let val = values.first().copied()
                                .and_then(|x| v.get(&x)).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                            if ctx.is_float_return {
                                {
                                    // SdFmov dest=返回FPR（字段0 预着色 XReg）、src=val（字段1）
                                    let __idx = __pack.push_inst(Inst::SdFmov { dest: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::FPR), src: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::FPR) });
                                    __pack.map_reg_field(crate::prelude::XReg::new(#ret_idx_lit, forge_ir::RegClass::Float, 8), __idx);
                                    __pack.map_reg_field(val, __idx);
                                    #(#ret_toks)*;
                                    Ok(__pack)
                                }
                            } else {
                                { #(#body_toks)* #(#ret_toks)* Ok(__pack) }
                            }
                        }
                    });
                } else if *term_name == "Jump" {
                    let inst_toks = gen_lower_insts(&default_insts, model)?;
                    arms.push(quote! {
                        crate::prelude::Terminator::Jump { target, .. } => {
                            let target = target.0 as i64;
                            #(#inst_toks)*;
                            Ok(__pack)
                        }
                    });
                } else if *term_name == "Branch" {
                    let inst_toks = gen_lower_insts(&default_insts, model)?;
                    arms.push(quote! {
                        crate::prelude::Terminator::Branch { cond: cond_val, then_block, else_block, .. } => {
                            let cond = v.get(cond_val).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                            let true_block = then_block.0 as i64;
                            let false_block = else_block.0 as i64;
                            #(#inst_toks)*;
                            Ok(__pack)
                        }
                    });
                } else if *term_name == "Unreachable" {
                    let inst_toks = gen_lower_insts(&default_insts, model)?;
                    arms.push(quote! {
                        crate::prelude::Terminator::Unreachable => {
                            #(#inst_toks)*;
                            Ok(__pack)
                        }
                    });
                } else if *term_name == "Switch" {
                    // Switch: if-else 链展开 — 对每个 case 生成 CMP + JCC Equal
                    // 字段已物理化（Reg）：临时 XReg 经 ctx.alloc_xreg 分配，push 时 map_reg_field 记录
                    arms.push(quote! {
                        crate::prelude::Terminator::Switch { discriminant, default_block, default_args: _, cases } => {
                            let disc = v.get(discriminant).copied().unwrap_or_else(|| ctx.alloc_xreg(crate::prelude::RegClass::GPR));
                            for &(case_val, case_block, _) in cases.iter() {
                                // 每个 case 分配一个临时 XReg 装载常量
                                let tmp = ctx.alloc_xreg(crate::prelude::RegClass::GPR);
                                let __idx = __pack.push_inst(Inst::SdMovImm { dest: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::GPR), imm: case_val as i64 });
                                __pack.map_reg_field(tmp, __idx);
                                // SD_CMP disc, tmp
                                let __idx = __pack.push_inst(Inst::SdCmp { src1: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::GPR), src2: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::GPR) });
                                __pack.map_reg_field(disc, __idx);
                                __pack.map_reg_field(tmp, __idx);
                                // SD_JCC Equal (0), case_block
                                __pack.push_inst(Inst::SdJcc { cond: 0u8, rel: case_block.0 as i64 });
                            }
                            // SD_JMP default_block
                            __pack.push_inst(Inst::SdJmp { rel: default_block.0 as i64 });
                            Ok(__pack)
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
                    if let Some(&val) = values.first() {
                        if let Some(&vr) = v.get(&val) {
                            __pack.push_inst(Inst::MovR8Rm { dest: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::GPR), src: vr });
                        }
                    }
                    Ok(__pack)
                }
            });
        }
        if !model.lower_term.contains_key("Unreachable") {
            arms.push(quote! {
                crate::prelude::Terminator::Unreachable => { { __pack.push_inst(Inst::Nop {}); Ok(__pack) } }
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
            v: &std::collections::HashMap<crate::prelude::Value, crate::prelude::XReg>,
            ctx: &mut crate::prelude::LowerCtx,
        ) -> Result<crate::prelude::InstPacket<Inst>, crate::prelude::CompileError> {
            let mut __pack = crate::prelude::InstPacket::new();
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
                _args: &[crate::prelude::XReg],
                _results: &[crate::prelude::XReg],
                _ctx: &mut crate::prelude::LowerCtx,
            ) -> Result<crate::prelude::InstPacket<Inst>, crate::prelude::CompileError> {
                Ok(crate::prelude::InstPacket::new())
            }
        });
    }

    let mut arms: Vec<TokenStream> = Vec::new();
    for (pattern_name, rule) in &model.lower_pattern {
        let lowering = crate::cst_codegen::gen_lowering_insts_cst(&rule.insts, model)?;
        let temp_toks = lowering.temps;
        let inst_toks = lowering.insts;
        arms.push(quote! {
            #pattern_name => {
                let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR); _r });
                #(#temp_toks);*;
                #(#temp_toks);*;
                #(#inst_toks)*;
                Ok(__pack)
            }
        });
    }

    arms.push(quote! {
        _ => Ok(crate::prelude::InstPacket::new())
    });

    Ok(quote! {
        pub fn lower_pattern_impl(
            pattern_name: &str,
            args: &[crate::prelude::XReg],
            results: &[crate::prelude::XReg],
            ctx: &mut crate::prelude::LowerCtx,
        ) -> Result<crate::prelude::InstPacket<Inst>, crate::prelude::CompileError> {
            let _ = ctx;
            let mut __pack = crate::prelude::InstPacket::new();
            match pattern_name { #(#arms),* }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn make_minimal_model() -> IsaModel {
        let mut reg = BTreeMap::new();
        reg.insert(
            "gpr".into(),
            RegGroup {
                count: 8,
                width: 64,
                names: Some(vec!["R0".into(), "R1".into(), "R2".into(), "R3".into()]),
                prefix: None,
            },
        );
        reg.insert(
            "xmm".into(),
            RegGroup {
                count: 8,
                width: 128,
                names: Some(vec![
                    "XMM0".into(),
                    "XMM1".into(),
                    "XMM2".into(),
                    "XMM3".into(),
                ]),
                prefix: None,
            },
        );
        IsaModel {
            reg_classes: BTreeMap::new(),
            spill: BTreeMap::new(),
            meta: Meta {
                name: "minimal".into(),
                version: "1".into(),
                endian: "little".into(),
                mode: 64,
                max_inst_len: 15,
                capabilities: Default::default(),
                no_default_lowering: true,
                no_epilogue_label: false,
            },
            reg,
            abi: None,
            inst: BTreeMap::new(),
            lower: BTreeMap::new(),
            lower_term: BTreeMap::new(),
            lower_pattern: BTreeMap::new(),
            emit: None,
            enc_macros: BTreeMap::new(),
            enc_scatters: BTreeMap::new(),
            enc_constants: BTreeMap::new(),
            cc_names: BTreeMap::new(),
            lang_tokens: None,
            lang_keywords: None,
            lang_rules: None,
            dyn_types: BTreeMap::new(),
        }
    }

    #[test]
    fn test_validate_missing_gpr_reg_group() {
        let model = IsaModel {
            reg_classes: BTreeMap::new(),
            spill: BTreeMap::new(),
            meta: Meta {
                name: "test".into(),
                version: "1".into(),
                endian: "little".into(),
                mode: 64,
                max_inst_len: 15,
                capabilities: Default::default(),
                no_default_lowering: true,
                no_epilogue_label: false,
            },
            reg: BTreeMap::new(),
            abi: None,
            inst: BTreeMap::new(),
            lower: BTreeMap::new(),
            lower_term: BTreeMap::new(),
            lower_pattern: BTreeMap::new(),
            emit: None,
            enc_macros: BTreeMap::new(),
            enc_scatters: BTreeMap::new(),
            enc_constants: BTreeMap::new(),
            cc_names: BTreeMap::new(),
            lang_tokens: None,
            lang_keywords: None,
            lang_rules: None,
            dyn_types: BTreeMap::new(),
        };
        let err = model.validate().unwrap_err();
        assert!(
            err.contains("missing [reg.gpr64] or [reg.gpr]"),
            "Expected missing reg error, got: {err}"
        );
    }

    #[test]
    fn test_validate_inst_with_ireg_but_no_gpr() {
        let mut inst = BTreeMap::new();
        inst.insert(
            "ADD".into(),
            Instruction {
                fields: vec![InstField {
                    name: "dest".into(),
                    field_type: FieldType::Ireg,
                    role: None,
                }],
                encoding: None,
                asm: "add {dest}".into(),
                variants: None,
                effect: None,
            },
        );
        let model = IsaModel {
            reg_classes: BTreeMap::new(),
            spill: BTreeMap::new(),
            meta: Meta {
                name: "test".into(),
                version: "1".into(),
                endian: "little".into(),
                mode: 64,
                max_inst_len: 15,
                capabilities: Default::default(),
                no_default_lowering: true,
                no_epilogue_label: false,
            },
            reg: BTreeMap::new(),
            abi: None,
            inst,
            lower: BTreeMap::new(),
            lower_term: BTreeMap::new(),
            lower_pattern: BTreeMap::new(),
            emit: None,
            enc_macros: BTreeMap::new(),
            enc_scatters: BTreeMap::new(),
            enc_constants: BTreeMap::new(),
            cc_names: BTreeMap::new(),
            lang_tokens: None,
            lang_keywords: None,
            lang_rules: None,
            dyn_types: BTreeMap::new(),
        };
        let err = model.validate().unwrap_err();
        assert!(
            err.to_lowercase().contains("gpr"),
            "Expected error about missing GPR register group, got: {err}"
        );
    }

    #[test]
    fn test_validate_inst_with_freg_but_no_xmm() {
        let mut reg = BTreeMap::new();
        reg.insert(
            "gpr".into(),
            RegGroup {
                count: 2,
                width: 64,
                names: Some(vec!["R0".into(), "R1".into()]),
                prefix: None,
            },
        );
        let mut inst = BTreeMap::new();
        inst.insert(
            "FADD".into(),
            Instruction {
                fields: vec![InstField {
                    name: "dest".into(),
                    field_type: FieldType::Freg,
                    role: None,
                }],
                encoding: None,
                asm: "fadd {dest}".into(),
                variants: None,
                effect: None,
            },
        );
        let model = IsaModel {
            reg_classes: BTreeMap::new(),
            spill: BTreeMap::new(),
            meta: Meta {
                name: "test".into(),
                version: "1".into(),
                endian: "little".into(),
                mode: 64,
                max_inst_len: 15,
                capabilities: Default::default(),
                no_default_lowering: true,
                no_epilogue_label: false,
            },
            reg,
            abi: None,
            inst,
            lower: BTreeMap::new(),
            lower_term: BTreeMap::new(),
            lower_pattern: BTreeMap::new(),
            emit: None,
            enc_macros: BTreeMap::new(),
            enc_scatters: BTreeMap::new(),
            enc_constants: BTreeMap::new(),
            cc_names: BTreeMap::new(),
            lang_tokens: None,
            lang_keywords: None,
            lang_rules: None,
            dyn_types: BTreeMap::new(),
        };
        let err = model.validate().unwrap_err();
        assert!(
            err.contains("requires [reg.xmm]"),
            "Expected xmm requirement error, got: {err}"
        );
    }

    #[test]
    fn test_validate_passes_with_valid_model() {
        let model = make_minimal_model();
        model
            .validate()
            .expect("Valid model should pass validation");
    }

    #[test]
    fn test_generate_fails_with_invalid_encoding_macro() {
        let mut model = make_minimal_model();
        model.inst.insert(
            "ADD".into(),
            Instruction {
                fields: vec![
                    InstField {
                        name: "dest".into(),
                        field_type: FieldType::Ireg,
                        role: None,
                    },
                    InstField {
                        name: "src".into(),
                        field_type: FieldType::Ireg,
                        role: None,
                    },
                ],
                encoding: Some("$nonexistent_macro dest src".into()),
                asm: "add {dest}, {src}".into(),
                variants: None,
                effect: None,
            },
        );
        let err = generate(&model).unwrap_err();
        assert!(
            err.contains("unknown macro"),
            "Expected unknown macro error, got: {err}"
        );
    }

    #[test]
    fn test_duplicate_reg_names_in_group() {
        let mut reg = BTreeMap::new();
        reg.insert(
            "gpr".into(),
            RegGroup {
                count: 3,
                width: 64,
                names: Some(vec!["R0".into(), "R1".into(), "R1".into()]),
                prefix: None,
            },
        );
        let model = IsaModel {
            reg_classes: BTreeMap::new(),
            spill: BTreeMap::new(),
            meta: Meta {
                name: "test".into(),
                version: "1".into(),
                endian: "little".into(),
                mode: 64,
                max_inst_len: 15,
                capabilities: Default::default(),
                no_default_lowering: true,
                no_epilogue_label: false,
            },
            reg,
            abi: None,
            inst: BTreeMap::new(),
            lower: BTreeMap::new(),
            lower_term: BTreeMap::new(),
            lower_pattern: BTreeMap::new(),
            emit: None,
            enc_macros: BTreeMap::new(),
            enc_scatters: BTreeMap::new(),
            enc_constants: BTreeMap::new(),
            cc_names: BTreeMap::new(),
            lang_tokens: None,
            lang_keywords: None,
            lang_rules: None,
            dyn_types: BTreeMap::new(),
        };
        let gpr_group = model.reg.get("gpr").unwrap();
        let names = gpr_group.names.as_ref().unwrap();
        assert_eq!(names.len(), 3);
        let r1_count = names.iter().filter(|n| n.as_str() == "R1").count();
        assert_eq!(r1_count, 2, "Expected R1 to appear twice (duplicate)");
    }
}

#[cfg(test)]
mod determinism_tests {
    /// Generation must be deterministic: the same model parsed twice from
    /// source must produce byte-identical generated tokens (guards against
    /// HashMap iteration order creeping back in).
    #[test]
    fn test_generate_is_deterministic() {
        let root = env!("CARGO_MANIFEST_DIR");
        for path in [
            "isa/x86_v10.toml",
            "isa/aarch64_v10.toml",
            "isa/riscv64_v10.toml",
            "isa/wasm32_v10.toml",
        ] {
            let full = std::path::Path::new(root).join("../../..").join(path);
            let source = std::fs::read_to_string(&full).expect(path);
            let mut model = crate::parser::parse(&source).expect("parse");
            model.validate().expect("validate");
            model.expand_variants();
            let t1 = crate::codegen::generate(&model).expect("generate 1");
            let t2 = crate::codegen::generate(&model).expect("generate 2");
            assert_eq!(
                t1.to_string(),
                t2.to_string(),
                "non-deterministic generation for {path}"
            );
        }
    }
}
