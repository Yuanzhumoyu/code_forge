//! 代码生成器 — 从 IsaModel 生成 Rust TokenStream。
//!
//! 所有生成内容来自 TOML 模型字段，零硬编码。

use crate::model::*;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::HashSet;

/// 批量 emit 的 flush 共享骨架（双轨生成器共用——cst_codegen.rs 与
/// codegen/mod.rs 的 flush 闭包逐字重复,第四十五轮提取）。
/// `emit_inst`/`rm`/`sink` 为生成代码内的标识符/表达式（TokenStream 拼接）。
pub(crate) fn flush_emit_stmts(
    pending: &mut Vec<TokenStream>,
    stmts: &mut Vec<TokenStream>,
    emit_inst: &TokenStream,
    rm: &TokenStream,
    sink: &TokenStream,
) {
    if !pending.is_empty() {
        stmts.push(quote! {
            for __inst in &[#(#pending),*] {
                if let Err(e) = #emit_inst(__inst, #rm, #sink) {
                    return Err(e);
                }
            }
        });
        pending.clear();
    }
}

/// 伪指令生成共享骨架（双轨生成器共用——cst_codegen.rs 与 codegen/mod.rs
/// 的 5 分支 match 逐字重复,第四十五轮提取）。
pub(crate) fn gen_pseudo_inst(
    name: &str,
    model: &IsaModel,
    stmts: &mut Vec<TokenStream>,
) {
    match name {
        "push_callee" => stmts.push(gen_push_callee(model, true)),
        "pop_callee" => stmts.push(gen_push_callee(model, false)),
        "move_args" => stmts.push(gen_move_args(model)),
        "frame_alloc" => stmts.push(gen_frame_alloc_free(model, true)),
        "frame_free" => stmts.push(gen_frame_alloc_free(model, false)),
        _ => {}
    }
}

/// Collect GPR width-view groups in canonical order.
/// Order comes from `[meta].gpr_bank_order` when declared (e.g. x86
/// `["gpr64","gpr32","gpr16","gpr8l","gpr8h"]`) — only the listed groups are
/// collected. Without `gpr_bank_order`, falls back to the classic sub-register
/// order for backward compatibility, then to a single `gpr` group (old ISA
/// definitions).
fn collect_gpr_groups(model: &IsaModel) -> Vec<(&str, &RegGroup)> {
    let declared = &model.meta.gpr_bank_order;
    let mut groups: Vec<(&str, &RegGroup)> = Vec::new();
    if !declared.is_empty() {
        for name in declared {
            if let Some(g) = model.reg.get(name.as_str()) {
                groups.push((name.as_str(), g));
            }
        }
    } else {
        for name in ["gpr64", "gpr32", "gpr16", "gpr8l", "gpr8h"] {
            if let Some(g) = model.reg.get(name) {
                groups.push((name, g));
            }
        }
    }
    if groups.is_empty()
        && let Some(g) = model.reg.get("gpr")
    {
        groups.push(("gpr", g));
    }
    groups
}

/// Get the canonical GPR register group (gpr64 优先，回退 gpr) — 供 ABI/push-pop 使用。
fn get_gpr_group(model: &IsaModel) -> &RegGroup {
    collect_gpr_groups(model)
        .into_iter()
        .next()
        .map(|(_, g)| g)
        .expect("missing [reg.gpr64] or [reg.gpr]")
}

/// 生成一个寄存器组的所有变体（优先 names，否则按 prefix/count 生成）。
fn group_variants(group: &RegGroup) -> Vec<TokenStream> {
    match &group.names {
        Some(names) if !names.is_empty() => names
            .iter()
            .map(|n| {
                let ident = format_ident!("{n}");
                quote! { #ident }
            })
            .collect(),
        _ => {
            let prefix = group.prefix.as_deref().unwrap_or("R");
            (0..group.count as usize)
                .map(|i| {
                    let ident = format_ident!("{prefix}{i}");
                    quote! { #ident }
                })
                .collect()
        }
    }
}

pub(crate) mod components;
#[path = "../standard_insts.rs"]
mod standard_insts;

/// Generate shared encoding helper functions (ModRM/SIB byte assembly).
/// These are generic encoding concepts (no ISA-specific constants); they are
/// emitted only for variable-length ISAs that opt in via
/// `[meta.capabilities].variable_length`. ISA-specific encodings (REX prefix
/// layout, RBP=5-style constants) live in the TOML / parameterized primitives
/// (`@modrm`, `@modrm_mem`, `@lea_sib`, …), not here.
fn gen_encoding_helpers() -> TokenStream {
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
    let encoding_helpers = if model.meta.capabilities.variable_length {
        gen_encoding_helpers()
    } else {
        quote! {}
    };

    // v19: Componentized TargetMachine + trait impls
    let v19_components = components::gen_v19_components(model);

    // P2: DSL 生成字节→指令解码器（Phase 1 定宽编码）
    let decoder = gen_decoder(model)?;

    // ISA 默认值类（元数据驱动）：lowering 中 alloc_xreg/from_index 的默认
    // 目标类。DSL 生成代码不再硬编码 RegClass::GPR64/FPR64——非 64 位主类
    // 的 ISA（如 32 位寄存器组的 wasm32）按声明的宽度推导。
    let (gpr_default_class, fpr_default_class) = default_class_tokens(model);

    let mut out = quote! {
        // ── ISA 默认值类（元数据驱动，替代生成代码中硬编码的 GPR64/FPR64）──
        #[allow(dead_code)]
        pub(crate) const __DEFAULT_GPR_CLASS: forge_ir::RegClass = #gpr_default_class;
        #[allow(dead_code)]
        pub(crate) const __DEFAULT_FPR_CLASS: forge_ir::RegClass = #fpr_default_class;

        #reg_enum
        #inst_enum
        #machine_inst
        // ── ISA encoding helpers ──
        #encoding_helpers
        // ── shared lowering / emit functions ──
        #emit_func
        #lower_func
        #lower_term_func
        #lower_pattern_func
        // ── 字节→指令解码器（P2 DSL 生成；Phase 1 定宽编码）──
        #decoder
        // ── v19 componentized API ──
        #v19_components
    };

    // 生成代码中的 RegClass::GPR64/FPR64 引用统一替换为元数据推导的默认类。
    // （x86 主类 = GPR(8)/FPR(8) = GPR64/FPR64，行为不变；其他 ISA 按声明宽度。）
    // 按最长前缀逐步替换，避免把 forge_ir::/crate::prelude:: 前缀一起吞掉。
    let s = out
        .to_string()
        .replace(
            "crate :: prelude :: RegClass :: GPR64",
            "__DEFAULT_GPR_CLASS",
        )
        .replace("forge_ir :: RegClass :: GPR64", "__DEFAULT_GPR_CLASS")
        .replace("RegClass :: GPR64", "__DEFAULT_GPR_CLASS")
        .replace(
            "crate :: prelude :: RegClass :: FPR64",
            "__DEFAULT_FPR_CLASS",
        )
        .replace("forge_ir :: RegClass :: FPR64", "__DEFAULT_FPR_CLASS")
        .replace("RegClass :: FPR64", "__DEFAULT_FPR_CLASS");
    out = s
        .parse::<TokenStream>()
        .map_err(|e| format!("post-process default-class substitution: {e}"))?;

    Ok(out)
}

/// ISA 默认整数值/浮点值类 token（元数据驱动）：
/// - GPR 主类：`[reg.gpr64]` 存在 → GPR(8)；仅 `[reg.gpr]` → 按声明宽度（位/8）；
/// - FPR 主类：`[meta].default_fpr_width`（缺省 8，即 f64）。
pub(crate) fn default_class_tokens(model: &IsaModel) -> (TokenStream, TokenStream) {
    let gpr = if model.reg.contains_key("gpr64") {
        quote! { forge_ir::RegClass::GPR(8u16) }
    } else if let Some(g) = model.reg.get("gpr") {
        let w = g.width / 8;
        quote! { forge_ir::RegClass::GPR(#w) }
    } else {
        quote! { forge_ir::RegClass::GPR64 }
    };
    let fpr = {
        let w = model.meta.default_fpr_width.unwrap_or(8) as u16;
        quote! { forge_ir::RegClass::FPR(#w) }
    };
    (gpr, fpr)
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
pub(crate) enum RegLookup {
    /// Named register found at index, with its exact name.
    Named { name: String, index: u32 },
    /// Prefix-based register found at index (e.g. XMM0 with prefix "XMM").
    Prefixed { name: String, index: u32 },
    /// Not found in any register group.
    NotFound,
}

/// Look up a register name in the ISA model's register groups.
/// Searches both named registers and prefix-based registers.
/// Returns the register's exact name and its index in the group.
pub(crate) fn lookup_reg_in_model(val: &str, model: &IsaModel) -> RegLookup {
    for group in model.reg.values() {
        // Named registers: "RAX", "RCX", "R10", "XMM0", ...
        if let Some(names) = &group.names
            && let Some(pos) = names.iter().position(|n| n.eq_ignore_ascii_case(val))
        {
            // 物理编号 = 组内索引 + 组声明的 base_index（如 x86 gpr8h 高字节组
            // AH/BH/CH/DH 声明 base_index = 4 → 物理编号 4..7）。
            let idx = pos as u32 + group.base_index.unwrap_or(0);
            return RegLookup::Named {
                name: names[pos].clone(),
                index: idx,
            };
        }
        // Prefix-based names: "XMM0" with prefix "XMM", "R5" with prefix "R"
        if let Some(ref prefix) = group.prefix {
            let upper = val.to_uppercase();
            if let Some(stripped) = upper.strip_prefix(&prefix.to_uppercase())
                && let Ok(index) = stripped.parse::<u32>()
            {
                let full_name = format!("{prefix}{index}");
                return RegLookup::Prefixed {
                    name: full_name,
                    index,
                };
            }
        }
    }
    RegLookup::NotFound
}

/// Check if a field is a def (write) register, respecting explicit TOML role.
pub(crate) fn is_field_def(field: &crate::model::InstField) -> bool {
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
    // ── GPR 位宽组（规范顺序）──
    let gpr_groups = collect_gpr_groups(model);
    let gpr_variants: Vec<TokenStream> = gpr_groups
        .iter()
        .flat_map(|(_, g)| group_variants(g))
        .collect();

    // ── FPR 组（xmm/float）──
    let fpr_group = model.reg.get("xmm").or_else(|| model.reg.get("float"));
    let fpr_variants: Vec<TokenStream> = match fpr_group {
        Some(g) => group_variants(g),
        None => Vec::new(),
    };
    let fpr_offset = gpr_variants.len() as u32;
    let fpr_count = fpr_variants.len();

    // 合并变体（判别值布局：全部 GPR 在前，FPR 在后）
    let mut all_variants = gpr_variants.clone();
    all_variants.extend(fpr_variants.clone());

    // ── to_index 显式物理编号映射 ──
    // gpr64/gpr32/gpr16/gpr8l：组内索引 = 物理编号 0..15（同族不同宽度视图共享编号）
    // gpr8h（AH/BH/CH/DH）：物理编号 4..7（x86 无 REX 高字节编码空间）
    // xmm：16+n —— 分配器 VReg id 域与 GPR(0..15) 不重叠；编码宏只用低 4 位 + bit3，
    //       16+n 的低 4 位恰为 XMM 编码编号 n（与改造前 fpr_offset=16 的行为一致）
    let mut gpr_index_arms: Vec<TokenStream> = Vec::new();
    for (_, group) in &gpr_groups {
        let variants = group_variants(group);
        // 物理编号 = 组内索引 + base_index（如 x86 gpr8h 高字节组 = 4）。
        let base = group.base_index.unwrap_or(0);
        for (i, v) in variants.iter().enumerate() {
            let idx = i as u32 + base;
            gpr_index_arms.push(quote! { Reg::#v => #idx });
        }
    }
    let fpr_index_arms: Vec<TokenStream> = fpr_variants
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let idx = 16 + i as u32;
            quote! { Reg::#v => #idx }
        })
        .collect();

    // ── from_index 臂：GPR 区返回 gpr64 主视图变体；FPR 区返回 XMM 变体 ──
    let gpr64_group = gpr_groups.first().map(|(_, g)| *g);
    let gpr_idx_arms: Vec<TokenStream> = match gpr64_group.and_then(|g| g.names.as_ref()) {
        Some(names) if !names.is_empty() => names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let v = format_ident!("{n}");
                let i = i as u32;
                quote! { #i => Reg::#v }
            })
            .collect(),
        _ => {
            let prefix = gpr64_group
                .and_then(|g| g.prefix.clone())
                .unwrap_or_else(|| "R".to_string());
            let count = gpr64_group.map(|g| g.count as u32).unwrap_or(16);
            (0..count)
                .map(|i| {
                    let v = format_ident!("{prefix}{i}");
                    quote! { #i => Reg::#v }
                })
                .collect()
        }
    };
    let fpr_from_idx_arms: Vec<TokenStream> = fpr_variants
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let i = fpr_offset + i as u32;
            quote! { #i => Reg::#v }
        })
        .collect();

    let first = &all_variants[0];

    quote! {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u32)]
        pub enum Reg { #(#all_variants),* }

        impl forge_ir::PhysReg for Reg {
            fn to_index(self) -> u32 {
                match self {
                    #(#gpr_index_arms,)*
                    #(#fpr_index_arms,)*
                }
            }
            fn class(self) -> forge_ir::RegClass {
                let idx = self as u32;
                if idx >= #fpr_offset && #fpr_count > 0 {
                    forge_ir::RegClass::Float
                } else {
                    forge_ir::RegClass::Int
                }
            }
            fn from_index(idx: u32, cls: forge_ir::RegClass) -> Self {
                match cls {
                    forge_ir::RegClass::Float => match idx.wrapping_add(#fpr_offset) {
                        #(#fpr_from_idx_arms ,)*
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

pub(crate) fn field_type_tok(ft: &FieldType) -> TokenStream {
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
    let mut implicit_arms: Vec<TokenStream> = Vec::new();

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
                    quote! { forge_ir::RegClass::FPR64 }
                } else {
                    quote! { forge_ir::RegClass::GPR64 }
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
            // clobbers() 由 TOML 的 implicit 提供（x86_v10.toml 的 CALL_RIP_REL /
            // CALL_RM 声明 caller-saved 全集）。历史上恒空（有意权衡）：
            // call 破坏 caller-saved（RAX/RCX/RDX/RSI/RDI/R8-R11/XMM0-15），启用
            // clobber 处理曾导致 test_jit_call_external_function AV——根因是发参
            // mov 的 XReg 在 call 点仍被视为活跃。现 regalloc 的 expire_dead 在
            // clobber 处理之前执行：发参 mov 是参数 XReg 的最后 use（活区间终点=
            // 发参 mov 点 < call 点），call 前已释放，不会被误 spill；clobber 只
            // spill 真正跨 call 活跃且被分配在 caller-saved 的 XReg（修复运行期
            // 值丢失，如 Vec ptr/len 槽与外部调用返回值被 call 破坏）。
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
        // implicit — 指令执行时隐式破坏的物理寄存器（cqo 的 RDX、@shift_reg 的 CL）
        if let Some(implicit) = &inst.implicit {
            let entries: Vec<TokenStream> = implicit
                .iter()
                .map(|r| {
                    let (is_fp, idx) = crate::codegen::components::resolve_reg_index(model, r);
                    if is_fp {
                        quote! { (#idx, crate::prelude::RegClass::FPR64) }
                    } else {
                        quote! { (#idx, crate::prelude::RegClass::GPR64) }
                    }
                })
                .collect();
            if !entries.is_empty() {
                implicit_arms.push(quote! { Inst::#vn { .. } => &[#(#entries),*] });
            }
        }
    }

    quote! {
        impl crate::prelude::MachineInst for Inst {
            fn uses(&self) -> smallvec::SmallVec<[u32;4]> {
                match self { #(#use_arms),* , Inst::Unknown(_) => smallvec::smallvec![] }
            }
            fn defs(&self) -> smallvec::SmallVec<[u32;2]> {
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
            fn clobbers(&self) -> &[(u32, crate::prelude::RegClass)] {
                match self { #(#implicit_arms,)* _ => &[] }
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
                return Err(crate::prelude::IrError::Emit("no encoding for ".into()));
            }
        });
    }

    arms.push(quote! {
        Inst::Unknown(_) => {
            return Err(crate::prelude::IrError::Emit("unknown instruction".into()));
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
        ) -> Result<(), crate::prelude::IrError> {
            use crate::encode::{BitField, pack_bits};
            let preg = |v: crate::prelude::XReg, m: &crate::prelude::AllocResult|
                m.resolve(v).map(|p| p.num).map_err(|e| crate::prelude::IrError::RegAlloc(format!("{}", e)));
            match inst { #(#arms),* }
            Ok(())
        }

        pub fn emit_prologue_impl(fs: u32, rm: &crate::prelude::AllocResult, sink: &mut crate::prelude::CodeSink)
            -> Result<(), crate::prelude::IrError>
        {
            let frame_size = fs;
            #prologue_body
            Ok(())
        }

        pub fn emit_epilogue_impl(fs: u32, rm: &crate::prelude::AllocResult, sink: &mut crate::prelude::CodeSink)
            -> Result<(), crate::prelude::IrError>
        {
            let frame_size = fs;
            #epilogue_body
            Ok(())
        }
    })
}

/// 生成字节→指令解码器（P2：DSL 生成 TargetDecoder，字节→Inst 反解）。
///
/// Phase 1：仅支持 `ParsedEncoding::Fixed` 且无 fixup、字段全部可逆（无
/// BlockTarget/MemRef/F32/F64——分支/内存/浮点立即数 Phase 1 不还原）的变体。
/// 定宽 ISA（riscv64/aarch64/minimal_sd）获得完整解码臂；x86 变长 @modrm、
/// wasm32 @leb128 等 Primitive/Segmented 编码 Phase 2 支持——不生成解码臂，
/// 走 unknown 错误。编码语义镜像 emit 侧：`word |= ((value >> shift) & mask) << offset`，
/// 解码 = 常量位段 guard + 字段位段提取（同名多段 OR 累加）。
fn gen_decoder(model: &IsaModel) -> Result<TokenStream, String> {
    use crate::bitstring::{BitFieldValue, ParsedEncoding};
    use crate::model::FieldType;

    let mut arms: Vec<TokenStream> = Vec::new();

    // FPR 解码表：镜像 gen_reg_enum 的 to_index（硬编码 16+i——与 x86 VReg 域
    // 约定一致；from_index 用 fpr_offset+i 对非 16 GPR 的 ISA 无法逆映射，解码
    // 不依赖它）。GPR 字段直接 from_index（to_index(Xn)=n 可逆）。
    let fpr_variants: Vec<String> = match model.reg.get("xmm").or_else(|| model.reg.get("float")) {
        Some(g) => group_variants(g).into_iter().map(|v| v.to_string()).collect(),
        None => Vec::new(),
    };
    let fpr_match_arms: Vec<TokenStream> = fpr_variants
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let idx = 16u64 + i as u64;
            let ident = format_ident!("{v}");
            quote! { #idx => Reg::#ident }
        })
        .collect();
    let fpr_expr = |bind_ident: &proc_macro2::Ident| {
        quote! {
            match #bind_ident {
                #(#fpr_match_arms,)*
                _ => <Reg as forge_ir::PhysReg>::from_index(0, __DEFAULT_FPR_CLASS),
            }
        }
    };
    // 裸索引映射（0..count → 变体）：ModRM 系原语（@modrm/@sse_*）的寄存器字段
    // 编码裸索引（3 位 + REX 扩展位），非 to_index 值——与定宽 ISA 的 FPR 字段不同。
    let fpr_raw_arms: Vec<TokenStream> = fpr_variants
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let idx = i as u64;
            let ident = format_ident!("{v}");
            quote! { #idx => Reg::#ident }
        })
        .collect();
    let raw_fpr_expr = |bind_ident: &proc_macro2::Ident| {
        quote! {
            match #bind_ident {
                #(#fpr_raw_arms,)*
                _ => <Reg as forge_ir::PhysReg>::from_index(0, __DEFAULT_FPR_CLASS),
            }
        }
    };

    // 共享字段构造：field_exprs（字段名 → u64 值表达式）→ (绑定语句, Inst 构造表达式)。
    // 非 Opsize 字段缺失 → None（该变体无法还原）。Opsize 未提供 → 0（不占编码位）。
    let construct = |vn: &syn::Ident,
                     field_map: &std::collections::BTreeMap<String, &FieldType>,
                     field_exprs: &std::collections::BTreeMap<String, TokenStream>,
                     fpr_map: &dyn Fn(&proc_macro2::Ident) -> TokenStream|
     -> Option<(Vec<TokenStream>, TokenStream)> {
        let mut field_binds: Vec<TokenStream> = Vec::new();
        let mut variant_fields: Vec<TokenStream> = Vec::new();
        for (fname, ftype) in field_map {
            let fi = format_ident!("{fname}");
            let Some(val_ts) = field_exprs.get(fname) else {
                if matches!(ftype, FieldType::Opsize) {
                    variant_fields.push(quote! { #fi: 0u8 });
                }
                continue;
            };
            let bind_ident = format_ident!("__dec_{fname}");
            field_binds.push(quote! { let #bind_ident = #val_ts; });
            let expr: TokenStream = match ftype {
                FieldType::Ireg | FieldType::GprReg => quote! {
                    <Reg as forge_ir::PhysReg>::from_index(#bind_ident as u32, __DEFAULT_GPR_CLASS)
                },
                FieldType::Freg | FieldType::XmmReg => {
                    let e = fpr_map(&bind_ident);
                    e
                }
                FieldType::I8 => quote! { #bind_ident as i8 },
                FieldType::I16 => quote! { #bind_ident as i16 },
                FieldType::I32 => quote! { #bind_ident as i32 },
                FieldType::I64 => quote! { #bind_ident as i64 },
                FieldType::U8 => quote! { #bind_ident as u8 },
                FieldType::U16 => quote! { #bind_ident as u16 },
                FieldType::U32 => quote! { #bind_ident as u32 },
                FieldType::U64 => quote! { #bind_ident as u64 },
                FieldType::CondCode | FieldType::Opsize => quote! { #bind_ident as u8 },
                // MemRef：field_exprs 条目即为完整 MemRef 表达式（base/offset/width 由原语解码填充）
                FieldType::MemRef => {
                    variant_fields.push(quote! { #fi: #val_ts });
                    continue;
                }
                _ => continue,
            };
            variant_fields.push(quote! { #fi: #expr });
        }
        // 非 Opsize 声明字段必须有值表达式，否则无法还原
        if field_map
            .iter()
            .any(|(n, t)| *t != &FieldType::Opsize && !field_exprs.contains_key(n))
        {
            return None;
        }
        let inst_expr = if variant_fields.is_empty() {
            quote! { Inst::#vn }
        } else {
            quote! { Inst::#vn { #(#variant_fields),* } }
        };
        Some((field_binds, inst_expr))
    };

    for (inst_name, inst) in &model.inst {
        let Some(enc_str) = &inst.encoding else { continue };

        let field_map: std::collections::BTreeMap<String, &FieldType> = inst
            .fields
            .iter()
            .map(|f| (f.name.clone(), &f.field_type))
            .collect();
        // 不可逆解码的字段类型 → 跳过该变体（MemRef 由 @modrm_mem 提供 base/offset 可还原）
        if field_map.values().any(|ft| {
            matches!(ft, FieldType::BlockTarget | FieldType::F32 | FieldType::F64)
        }) {
            continue;
        }

        let expanded = crate::bitstring::expand_encoding(enc_str, model)
            .map_err(|e| format!("decode: expand '{enc_str}': {e}"))?;
        let parsed = crate::bitstring::parse_encoding(&expanded)
            .map_err(|e| format!("decode: parse '{enc_str}': {e}"))?;
        let vn = pascal_ident(inst_name);
        match parsed {
            // ── Phase 1：定宽编码 ──
            ParsedEncoding::Fixed { width, fields, fixup } => {
                if fixup.is_some() || fields.is_empty() {
                    continue;
                }
                let byte_len = (width as usize).div_ceil(8);
                let read_expr: TokenStream = match width {
                    8 => quote! { bytes[0] as u64 },
                    16 => quote! { u16::from_le_bytes([bytes[0], bytes[1]]) as u64 },
                    32 => quote! { u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64 },
                    64 => quote! {
                        u64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3],
                                             bytes[4], bytes[5], bytes[6], bytes[7]]) as u64
                    },
                    _ => continue,
                };

                // 常量位段 guard + 字段位段提取（同名多段 OR 累加）
                let mut guards: Vec<TokenStream> = Vec::new();
                let mut extracts: std::collections::BTreeMap<String, Vec<TokenStream>> =
                    Default::default();
                for bf in &fields {
                    let offset = bf.offset as u64;
                    let shift = bf.shift.unwrap_or(0) as u64;
                    let mask_ts = if bf.width == 64 {
                        quote! { u64::MAX }
                    } else {
                        let m = (1u64 << bf.width) - 1;
                        quote! { #m }
                    };
                    let masked = quote! { ((__w >> #offset) & #mask_ts) };
                    match &bf.value {
                        BitFieldValue::Field(name) => {
                            let contrib = if shift == 0 {
                                masked
                            } else {
                                quote! { (#masked) << #shift }
                            };
                            extracts.entry(name.clone()).or_default().push(contrib);
                        }
                        BitFieldValue::Hex(v) => {
                            let v64 = *v as u64;
                            let expected = if shift == 0 {
                                quote! { (#v64 & #mask_ts) }
                            } else {
                                quote! { ((#v64 >> #shift) & #mask_ts) }
                            };
                            guards.push(quote! { (#masked) == (#expected) });
                        }
                        BitFieldValue::Dec(v) => {
                            let v64 = *v as u64;
                            let expected = if shift == 0 {
                                quote! { (#v64 & #mask_ts) }
                            } else {
                                quote! { ((#v64 >> #shift) & #mask_ts) }
                            };
                            guards.push(quote! { (#masked) == (#expected) });
                        }
                    }
                }
                let mut field_exprs: std::collections::BTreeMap<String, TokenStream> =
                    Default::default();
                for (fname, parts) in &extracts {
                    let val = if parts.len() == 1 {
                        parts[0].clone()
                    } else {
                        quote! { #(#parts)|* }
                    };
                    field_exprs.insert(fname.clone(), val);
                }
                let Some((binds, inst_expr)) = construct(&vn, &field_map, &field_exprs, &fpr_expr)
                else {
                    continue;
                };
                let guard_ts = if guards.is_empty() {
                    quote! { true }
                } else {
                    quote! { #(#guards)&&* }
                };
                arms.push(quote! {
                    if bytes.len() >= #byte_len {
                        let __w = #read_expr;
                        if #guard_ts {
                            #(#binds)*
                            return Ok((#inst_expr, #byte_len));
                        }
                    }
                });
            }

            // ── Phase 2：x86 变长原语子集（mod=11 寄存器形式）──
            ParsedEncoding::Primitive { name, args } => {
                let Some(arm) = gen_primitive_decode_arm(
                    &name,
                    &args,
                    &field_map,
                    &vn,
                    &construct,
                    &raw_fpr_expr,
                )? else {
                    continue;
                };
                arms.push(arm);
            }

            ParsedEncoding::Segmented { .. } => continue,
        }
    }

    Ok(quote! {
        /// DSL 生成的字节→指令解码器（Phase 1 定宽 + Phase 2 变长原语子集）。
        pub struct Decoder;

        impl crate::machine::decoder::TargetDecoder for Decoder {
            type Inst = Inst;

            fn decode(
                &self,
                bytes: &[u8],
            ) -> Result<(Inst, usize), crate::machine::decoder::DecodeError> {
                #(#arms)*
                Err(crate::machine::decoder::DecodeError::Other(
                    "no matching instruction".to_string(),
                ))
            }
        }
    })
}

/// Phase 2：x86 变长原语解码臂（mod=11 寄存器形式子集）。
///
/// 支持 @modrm/@op_rm（REX + 可选 escape + opcode + ModRM(reg-reg)）、
/// @push_reg/@pop_reg（REX.B + 50/58+r）、@mov_imm64（REX.W + B8+r + imm64）、
/// @setcc（REX + 0F 90+cc + ModRM(reg=0)）。内存寻址（mod≠3）与 SIMD/VEX/
/// leb128 等原语返回 None（Phase 2b 不支持）。生成的臂用闭包尝试解码，
/// 不匹配即返回 None 落到下一个变体。
fn gen_primitive_decode_arm<F>(
    name: &str,
    args: &[String],
    field_map: &std::collections::BTreeMap<String, &crate::model::FieldType>,
    vn: &syn::Ident,
    construct: &F,
    fpr_map: &dyn Fn(&proc_macro2::Ident) -> TokenStream,
) -> Result<Option<TokenStream>, String>
where
    F: Fn(
        &syn::Ident,
        &std::collections::BTreeMap<String, &crate::model::FieldType>,
        &std::collections::BTreeMap<String, TokenStream>,
        &dyn Fn(&proc_macro2::Ident) -> TokenStream,
    ) -> Option<(Vec<TokenStream>, TokenStream)>,
{
    let arg = |i: usize| args.get(i).map(|s| s.as_str()).unwrap_or("");
    // TOML 数字参数多为 0x 十六进制字面量（opcode/escape/ext）——i64::parse 不认，
    // 需显式按前缀解析
    let parse_num = |s: &str| -> Option<i64> {
        let t = s.trim();
        if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            i64::from_str_radix(h, 16).ok()
        } else {
            t.parse::<i64>().ok()
        }
    };
    let is_lit = |s: &str| !s.is_empty() && parse_num(s).is_some();
    let lit = |s: &str| -> i64 { parse_num(s).unwrap_or(0) };

    let mut field_exprs: std::collections::BTreeMap<String, TokenStream> = Default::default();
    let mut prelude: Vec<TokenStream> = Vec::new();

    match name {
        // @modrm opsize opcode reg rm [escape]
        "modrm" => {
            let opsize_arg = arg(0);
            let opcode_arg = arg(1);
            let reg_arg = arg(2);
            let rm_arg = arg(3);
            if !is_lit(opcode_arg)
                || (!is_lit(opsize_arg) && !field_map.contains_key(opsize_arg))
                || !field_map.contains_key(reg_arg)
                || !field_map.contains_key(rm_arg)
            {
                return Ok(None);
            }
            let opcode = lit(opcode_arg) as u8;
            let escape: u8 = if args.len() > 4 { lit(arg(4)) as u8 } else { 0 };
            let opsize_init: TokenStream = if is_lit(opsize_arg) {
                let v = lit(opsize_arg) as u32;
                quote! { #v }
            } else {
                quote! { 32u32 }
            };
            prelude.push(quote! {
                let mut __o = 0usize;
                let mut __opsize: u32 = #opsize_init;
                let mut __rex_r: u32 = 0;
                let mut __rex_b: u32 = 0;
                if __o < bytes.len() && bytes[__o] == 0x66 { __opsize = 16; __o += 1; }
                if __o < bytes.len() && (0x40..=0x4F).contains(&bytes[__o]) {
                    let __rex = bytes[__o]; __o += 1;
                    __rex_r = ((__rex >> 2) & 1) as u32;
                    __rex_b = (__rex & 1) as u32;
                    if (__rex & 0x08) != 0 { __opsize = 64; }
                }
                if #escape != 0u8 {
                    if __o >= bytes.len() || bytes[__o] != #escape { return None; }
                    __o += 1;
                }
                if __o >= bytes.len() || bytes[__o] != #opcode { return None; }
                __o += 1;
                if __o >= bytes.len() { return None; }
                let __modrm = bytes[__o]; __o += 1;
                if (__modrm >> 6) != 3 { return None; }
            });
            field_exprs.insert(
                reg_arg.to_string(),
                quote! { (((__modrm >> 3) & 7) as u64) | (__rex_r << 3) as u64 },
            );
            field_exprs.insert(
                rm_arg.to_string(),
                quote! { ((__modrm & 7) as u64) | (__rex_b << 3) as u64 },
            );
            if !is_lit(opsize_arg) {
                field_exprs.insert(opsize_arg.to_string(), quote! { __opsize as u64 });
            }
        }
        // @op_rm opsize opcode ext rm — reg 位置为常量 ext
        "op_rm" => {
            let opsize_arg = arg(0);
            let opcode_arg = arg(1);
            let ext_arg = arg(2);
            let rm_arg = arg(3);
            if !is_lit(opcode_arg)
                || !is_lit(ext_arg)
                || (!is_lit(opsize_arg) && !field_map.contains_key(opsize_arg))
                || !field_map.contains_key(rm_arg)
            {
                return Ok(None);
            }
            let opcode = lit(opcode_arg) as u8;
            let ext = lit(ext_arg) as u8;
            let opsize_init: TokenStream = if is_lit(opsize_arg) {
                let v = lit(opsize_arg) as u32;
                quote! { #v }
            } else {
                quote! { 32u32 }
            };
            prelude.push(quote! {
                let mut __o = 0usize;
                let mut __opsize: u32 = #opsize_init;
                let mut __rex_r: u32 = 0;
                let mut __rex_b: u32 = 0;
                if __o < bytes.len() && bytes[__o] == 0x66 { __opsize = 16; __o += 1; }
                if __o < bytes.len() && (0x40..=0x4F).contains(&bytes[__o]) {
                    let __rex = bytes[__o]; __o += 1;
                    __rex_r = ((__rex >> 2) & 1) as u32;
                    __rex_b = (__rex & 1) as u32;
                    if (__rex & 0x08) != 0 { __opsize = 64; }
                }
                if __o >= bytes.len() || bytes[__o] != #opcode { return None; }
                __o += 1;
                if __o >= bytes.len() { return None; }
                let __modrm = bytes[__o]; __o += 1;
                if (__modrm >> 6) != 3 { return None; }
                if ((__modrm >> 3) & 7) != #ext { return None; }
            });
            field_exprs.insert(
                rm_arg.to_string(),
                quote! { ((__modrm & 7) as u64) | (__rex_b << 3) as u64 },
            );
            if !is_lit(opsize_arg) {
                field_exprs.insert(opsize_arg.to_string(), quote! { __opsize as u64 });
            }
        }
        // @push_reg reg / @pop_reg reg
        "push_reg" | "pop_reg" => {
            let reg_arg = arg(0);
            if !field_map.contains_key(reg_arg) {
                return Ok(None);
            }
            let base: u8 = if name == "push_reg" { 0x50 } else { 0x58 };
            prelude.push(quote! {
                let mut __o = 0usize;
                let mut __rex_b: u32 = 0;
                if !bytes.is_empty() && bytes[0] == 0x41 { __rex_b = 1; __o = 1; }
                if __o >= bytes.len() { return None; }
                let __b = bytes[__o];
                if (__b & 0xF8) != #base { return None; }
                __o += 1;
            });
            field_exprs.insert(
                reg_arg.to_string(),
                quote! { ((__b & 7) as u64) | (__rex_b << 3) as u64 },
            );
        }
        // @mov_imm64 reg imm — REX.W + B8+r + imm64
        "mov_imm64" => {
            let reg_arg = arg(0);
            let imm_arg = arg(1);
            if !field_map.contains_key(reg_arg) || !field_map.contains_key(imm_arg) {
                return Ok(None);
            }
            prelude.push(quote! {
                if bytes.len() < 10 { return None; }
                let __rex = bytes[0];
                if __rex != 0x48 && __rex != 0x49 { return None; }
                let __b = bytes[1];
                if (__b & 0xF8) != 0xB8 { return None; }
                let __o = 10usize;
            });
            field_exprs.insert(
                reg_arg.to_string(),
                quote! { ((__b & 7) | ((__rex & 1) << 3)) as u64 },
            );
            field_exprs.insert(
                imm_arg.to_string(),
                quote! {
                    u64::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5],
                                        bytes[6], bytes[7], bytes[8], bytes[9]])
                },
            );
        }
        // @setcc dest cond — [REX] 0F (0x90|cc) ModRM(reg=0, rm=dest)
        "setcc" => {
            let dest_arg = arg(0);
            let cond_arg = arg(1);
            if !field_map.contains_key(dest_arg) || !is_lit(cond_arg) {
                return Ok(None);
            }
            let cc = lit(cond_arg) as u8;
            prelude.push(quote! {
                let mut __o = 0usize;
                let mut __rex_b: u32 = 0;
                if !bytes.is_empty() && bytes[0] == 0x41 { __rex_b = 1; __o = 1; }
                else if !bytes.is_empty() && bytes[0] == 0x40 { __o = 1; }
                if __o + 2 >= bytes.len() { return None; }
                if bytes[__o] != 0x0F { return None; }
                if bytes[__o + 1] != (0x90u8 | #cc) { return None; }
                let __modrm = bytes[__o + 2];
                if (__modrm >> 6) != 3 || ((__modrm >> 3) & 7) != 0 { return None; }
                let __o = __o + 3;
            });
            field_exprs.insert(
                dest_arg.to_string(),
                quote! { ((__modrm & 7) as u64) | (__rex_b << 3) as u64 },
            );
        }
        // @cmovcc opsize cc dest src — 0F 4{cc} /r 条件传送（mod=11）
        "cmovcc" => {
            let opsize_arg = arg(0);
            let cc_arg = arg(1);
            let dest_arg = arg(2);
            let src_arg = arg(3);
            if !is_lit(cc_arg)
                || (!is_lit(opsize_arg) && !field_map.contains_key(opsize_arg))
                || !field_map.contains_key(dest_arg)
                || !field_map.contains_key(src_arg)
            {
                return Ok(None);
            }
            let cc = lit(cc_arg) as u8;
            let opsize_init: TokenStream = if is_lit(opsize_arg) {
                let v = lit(opsize_arg) as u32;
                quote! { #v }
            } else {
                quote! { 32u32 }
            };
            prelude.push(quote! {
                let mut __o = 0usize;
                let mut __opsize: u32 = #opsize_init;
                let mut __rex_r: u32 = 0;
                let mut __rex_b: u32 = 0;
                if __o < bytes.len() && bytes[__o] == 0x66 { __opsize = 16; __o += 1; }
                if __o < bytes.len() && (0x40..=0x4F).contains(&bytes[__o]) {
                    let __rex = bytes[__o]; __o += 1;
                    __rex_r = ((__rex >> 2) & 1) as u32;
                    __rex_b = (__rex & 1) as u32;
                    if (__rex & 0x08) != 0 { __opsize = 64; }
                }
                if __o + 2 >= bytes.len() { return None; }
                if bytes[__o] != 0x0F { return None; }
                if bytes[__o + 1] != (0x40u8 | #cc) { return None; }
                let __modrm = bytes[__o + 2];
                if (__modrm >> 6) != 3 { return None; }
                let __o = __o + 3;
            });
            field_exprs.insert(
                dest_arg.to_string(),
                quote! { (((__modrm >> 3) & 7) as u64) | (__rex_r << 3) as u64 },
            );
            field_exprs.insert(
                src_arg.to_string(),
                quote! { ((__modrm & 7) as u64) | (__rex_b << 3) as u64 },
            );
            if !is_lit(opsize_arg) {
                field_exprs.insert(opsize_arg.to_string(), quote! { __opsize as u64 });
            }
        }
        // @modrm_mem opsize opcode reg base [disp] [prefix] [escape] — 内存寻址
        // （mod≠3；SIB 仅支持无 index 形式；MemRef 字段路径恢复 disp 为 offset）
        "modrm_mem" => {
            let opsize_arg = arg(0);
            let opcode_arg = arg(1);
            let reg_arg = arg(2);
            let rm_arg = arg(3);
            let is_memref = field_map
                .get(rm_arg)
                .map(|t| **t == crate::model::FieldType::MemRef)
                .unwrap_or(false);
            let (prefix_idx, escape_idx) = if is_memref { (4, 5) } else { (5, 6) };
            let prefix_arg = arg(prefix_idx);
            let escape_arg = arg(escape_idx);
            if !is_lit(opcode_arg)
                || (prefix_arg != "" && !is_lit(prefix_arg))
                || (escape_arg != "" && !is_lit(escape_arg))
                || (!is_lit(opsize_arg) && !field_map.contains_key(opsize_arg))
                || !field_map.contains_key(reg_arg)
                || !field_map.contains_key(rm_arg)
            {
                return Ok(None);
            }
            let opcode = lit(opcode_arg) as u8;
            let prefix: u8 = if prefix_arg != "" { lit(prefix_arg) as u8 } else { 0 };
            let escape: u8 = if escape_arg != "" { lit(escape_arg) as u8 } else { 0 };
            let opsize_init: TokenStream = if is_lit(opsize_arg) {
                let v = lit(opsize_arg) as u32;
                quote! { #v }
            } else {
                quote! { 32u32 }
            };
            prelude.push(quote! {
                let mut __o = 0usize;
                let mut __opsize: u32 = #opsize_init;
                let mut __rex_r: u32 = 0;
                let mut __rex_b: u32 = 0;
                if #prefix != 0u8 {
                    if __o >= bytes.len() || bytes[__o] != #prefix { return None; }
                    __o += 1;
                }
                if __o < bytes.len() && (0x40..=0x4F).contains(&bytes[__o]) {
                    let __rex = bytes[__o]; __o += 1;
                    __rex_r = ((__rex >> 2) & 1) as u32;
                    __rex_b = (__rex & 1) as u32;
                    if (__rex & 0x08) != 0 { __opsize = 64; }
                }
                if #escape != 0u8 {
                    if __o >= bytes.len() || bytes[__o] != #escape { return None; }
                    __o += 1;
                }
                if __o >= bytes.len() || bytes[__o] != #opcode { return None; }
                __o += 1;
                if __o >= bytes.len() { return None; }
                let __modrm = bytes[__o]; __o += 1;
                let __mod = __modrm >> 6;
                if __mod == 3 { return None; }
                let mut __base: u64 = ((__modrm & 7) as u64) | (__rex_b << 3) as u64;
                if (__modrm & 7) == 4 {
                    // SIB：仅支持 index=100（无 index，MemRef 无法表达 index/scale）
                    if __o >= bytes.len() { return None; }
                    let __sib = bytes[__o]; __o += 1;
                    if ((__sib >> 3) & 7) != 4 { return None; }
                    __base = ((__sib & 7) as u64) | (__rex_b << 3) as u64;
                }
                let mut __disp: i32 = 0;
                if __mod == 1 {
                    if __o >= bytes.len() { return None; }
                    __disp = (bytes[__o] as i8) as i32;
                    __o += 1;
                } else if __mod == 2 {
                    if __o + 3 >= bytes.len() { return None; }
                    __disp = i32::from_le_bytes([
                        bytes[__o], bytes[__o + 1], bytes[__o + 2], bytes[__o + 3],
                    ]);
                    __o += 4;
                }
            });
            field_exprs.insert(
                reg_arg.to_string(),
                quote! { (((__modrm >> 3) & 7) as u64) | (__rex_r << 3) as u64 },
            );
            if is_memref {
                // MemRef：base + offset（从 disp 恢复）+ width=0（类型宽度不在编码中）
                field_exprs.insert(
                    rm_arg.to_string(),
                    quote! { crate::prelude::MemRef::new(__base as u32, __disp, 0) },
                );
            } else {
                field_exprs.insert(rm_arg.to_string(), quote! { __base });
            }
            if !is_lit(opsize_arg) {
                field_exprs.insert(opsize_arg.to_string(), quote! { __opsize as u64 });
            }
        }
        // ── SSE 族：@sse_rr / @sse_rr_3a / @sse_rr_38 / @sse_rr_opsize /
        //    @sse_rr_imm8 / @sse_rr_w / @sse_ps_rr —— [prefix?] [REX?] 0F [3A|38] opcode ModRM [imm8]
        "sse_rr" | "sse_rr_3a" | "sse_rr_38" | "sse_rr_opsize" | "sse_rr_imm8" | "sse_rr_w"
        | "sse_ps_rr" => {
            // 参数布局：sse_rr = prefix opcode w reg rm（w=REX.W 位，解码忽略）；
            // sse_rr_3a/38/imm8/w = prefix opcode reg rm [imm]；
            // sse_rr_opsize = prefix opcode opsize reg rm；sse_ps_rr = opcode reg rm
            let (prefix_arg, opcode_arg, reg_arg, rm_arg, opsize_arg, imm_arg) = match name {
                "sse_rr" => (arg(0), arg(1), arg(3), arg(4), None, None),
                "sse_rr_opsize" => (arg(0), arg(1), arg(3), arg(4), Some(arg(2)), None),
                "sse_ps_rr" => ("", arg(0), arg(1), arg(2), None, None),
                _ => {
                    let has_imm = name == "sse_rr_3a" || name == "sse_rr_imm8";
                    (
                        arg(0),
                        arg(1),
                        arg(2),
                        arg(3),
                        None,
                        if has_imm { Some(arg(4)) } else { None },
                    )
                }
            };
            if !is_lit(opcode_arg)
                || (prefix_arg != "" && !is_lit(prefix_arg))
                || !field_map.contains_key(reg_arg)
                || !field_map.contains_key(rm_arg)
                || (imm_arg.is_some() && !field_map.contains_key(imm_arg.unwrap()))
                || (opsize_arg.is_some() && !field_map.contains_key(opsize_arg.unwrap()))
            {
                return Ok(None);
            }
            let opcode = lit(opcode_arg) as u8;
            let prefix: u8 = if prefix_arg != "" { lit(prefix_arg) as u8 } else { 0 };
            let rex_mandatory = name == "sse_rr_w";
            let is_opsize = name == "sse_rr_opsize";
            let has_3a = name == "sse_rr_3a";
            let has_38 = name == "sse_rr_38";
            prelude.push(quote! {
                let mut __o = 0usize;
                let mut __opsize: u32 = 32;
                let mut __rex_r: u32 = 0;
                let mut __rex_b: u32 = 0;
                if #prefix != 0u8 {
                    if __o >= bytes.len() || bytes[__o] != #prefix { return None; }
                    __o += 1;
                }
                if #is_opsize && __o < bytes.len() && bytes[__o] == 0x66 {
                    __opsize = 16; __o += 1;
                }
                if #rex_mandatory {
                    if __o >= bytes.len() || !(0x40..=0x4F).contains(&bytes[__o]) { return None; }
                    let __rex = bytes[__o]; __o += 1;
                    __rex_r = ((__rex >> 2) & 1) as u32;
                    __rex_b = (__rex & 1) as u32;
                    if (__rex & 0x08) != 0 { __opsize = 64; }
                } else if __o < bytes.len() && (0x40..=0x4F).contains(&bytes[__o]) {
                    let __rex = bytes[__o]; __o += 1;
                    __rex_r = ((__rex >> 2) & 1) as u32;
                    __rex_b = (__rex & 1) as u32;
                    if (__rex & 0x08) != 0 { __opsize = 64; }
                }
                if __o >= bytes.len() || bytes[__o] != 0x0F { return None; }
                __o += 1;
                if #has_3a {
                    if __o >= bytes.len() || bytes[__o] != 0x3A { return None; }
                    __o += 1;
                } else if #has_38 {
                    if __o >= bytes.len() || bytes[__o] != 0x38 { return None; }
                    __o += 1;
                }
                if __o >= bytes.len() || bytes[__o] != #opcode { return None; }
                __o += 1;
                if __o >= bytes.len() { return None; }
                let __modrm = bytes[__o]; __o += 1;
                if (__modrm >> 6) != 3 { return None; }
            });
            field_exprs.insert(
                reg_arg.to_string(),
                quote! { (((__modrm >> 3) & 7) as u64) | (__rex_r << 3) as u64 },
            );
            field_exprs.insert(
                rm_arg.to_string(),
                quote! { ((__modrm & 7) as u64) | (__rex_b << 3) as u64 },
            );
            if let Some(ia) = imm_arg {
                prelude.push(quote! {
                    if __o >= bytes.len() { return None; }
                    let __imm = bytes[__o] as u64;
                    __o += 1;
                });
                field_exprs.insert(ia.to_string(), quote! { __imm });
            }
            if let Some(oa) = opsize_arg {
                field_exprs.insert(oa.to_string(), quote! { __opsize as u64 });
            }
        }
        _ => return Ok(None),
    }

    let Some((binds, inst_expr)) = construct(vn, field_map, &field_exprs, fpr_map) else {
        return Ok(None);
    };
    Ok(Some(quote! {
        {
            let __try = |bytes: &[u8]| -> Option<(Inst, usize)> {
                #(#prelude)*
                #(#binds)*
                Some((#inst_expr, __o))
            };
            if let Some(__r) = __try(bytes) {
                return Ok(__r);
            }
        }
    }))
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
            let arg_mov = pascal_ident(arg_mov);
            let arg_mov_f = pascal_ident(arg_mov_f);
            let ret_mov = pascal_ident(ret_mov);
            let ret_mov_f = pascal_ident(ret_mov_f);
            let call_inst = pascal_ident(call_inst);
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
            let ret_gprs: Vec<_> = abi
                .ret_regs
                .gpr
                .iter()
                .map(|n| format_ident!("{n}"))
                .collect();
            // Call 专用 arm 的 clobbers 从 [abi.call] 自动推导：发参/收参寄存器
            // （arg_regs + ret_regs）在序列内写死占用，分配器必须避开。
            let call_clobber_names: Vec<String> = abi
                .arg_regs
                .gpr
                .iter()
                .chain(&abi.arg_regs.xmm)
                .chain(&abi.ret_regs.gpr)
                .chain(&abi.ret_regs.xmm)
                .map(|n| n.to_ascii_uppercase())
                .collect();
            let call_clobber_toks: Vec<TokenStream> = call_clobber_names
                .iter()
                .map(|n| {
                    let (is_fp, idx) = crate::codegen::components::resolve_reg_index(model, n);
                    if is_fp {
                        quote! { (#idx, crate::prelude::RegClass::FPR64) }
                    } else {
                        quote! { (#idx, crate::prelude::RegClass::GPR64) }
                    }
                })
                .collect();
            let call_clobber_set = if call_clobber_toks.is_empty() {
                quote! {}
            } else {
                quote! { ctx.current_clobbers = vec![#(#call_clobber_toks),*]; }
            };
            // 浮点返回寄存器必须由 [abi.ret_regs].xmm 显式声明（无 ISA 默认值）。
            let ret_xmm = abi.ret_regs.xmm.first().map(|n| format_ident!("{n}"));
            let ret_xmm_expr = match &ret_xmm {
                Some(ri) => quote! { Reg::#ri },
                None => quote! {
                    compile_error!("[abi.ret_regs].xmm is required for [abi.call] float returns — declare the float return register (e.g. XMM0)")
                },
            };

            // ── 调用骨架配置（全部来自 [abi.call]，DSL 无架构硬编码）──
            let shadow_space = call_cfg.shadow_space as usize;
            let stack_slot = call_cfg.stack_slot_size as usize;
            let reg_limit = call_cfg.reg_arg_limit as usize;
            let stack_align = abi.stack_align as usize;
            let arg_opsize = call_cfg.arg_opsize as u8;
            let stack_opsize = call_cfg.stack_opsize as u8;
            let sp_reg = match call_cfg
                .sp_reg
                .as_deref()
                .or_else(|| abi.frame.as_ref().map(|f| f.sp.as_str()))
            {
                Some(n) => format_ident!("{n}"),
                None => {
                    return Err(
                        "[abi.call] requires sp_reg (or [abi.frame].sp) for stack pointer".into(),
                    );
                }
            };
            let stack_store = match call_cfg.stack_store_inst.as_deref() {
                Some(n) => pascal_ident(n),
                None => {
                    return Err(
                        "[abi.call].stack_store_inst required — instruction that stores a stack-passed argument"
                            .into(),
                    )
                }
            };
            let stack_addr = match call_cfg.stack_addr_inst.as_deref() {
                Some(n) => pascal_ident(n),
                None => {
                    return Err(
                        "[abi.call].stack_addr_inst required — instruction that computes a stack-argument address"
                            .into(),
                    )
                }
            };
            let stack_alloc = match call_cfg.stack_alloc_inst.as_deref() {
                Some(n) => pascal_ident(n),
                None => {
                    return Err(
                        "[abi.call].stack_alloc_inst required — instruction that grows the stack for args"
                            .into(),
                    )
                }
            };
            let stack_free = match call_cfg.stack_free_inst.as_deref() {
                Some(n) => pascal_ident(n),
                None => {
                    return Err(
                        "[abi.call].stack_free_inst required — instruction that shrinks the stack after the call"
                            .into(),
                    )
                }
            };

            arms.push(quote! {
                crate::prelude::Opcode::Call { .. } => {
                    let mut __pack = crate::prelude::InstPacket::new();
                    let mut __gi = 0usize;
                    let mut __fi = 0usize;
                    // 第一遍分类：前 #reg_limit 个参数位置用寄存器（整数 → GPR、
                    // 浮点 → XMM，计数独立——按位置而非 gi/fi 判断：混合参数时第
                    // #reg_limit+1 个整数参数的 gi 可能 <#reg_limit（浮点参数不占 gi），
                    // 但 ABI 要求它压栈。
                    // 寄存器 mov 延后到栈参数 store 之后发射——arg_mov 写物理
                    // 寄存器是 regalloc 盲区（物理字段不参与分配），若先发会覆盖栈参数
                    // XReg 所在寄存器。
                    let mut __reg_moves: Vec<(bool, Reg, crate::prelude::XReg)> = Vec::new();
                    let mut __stack: Vec<(usize, crate::prelude::XReg)> = Vec::new();
                    let mut __idx = 0usize;
                    for __arg in args.iter().copied() {
                        if __arg.class().is_fp() {
                            if __idx < #reg_limit && __fi < #n_xmm {
                                let __reg = [#(Reg::#xmm_idents),*][__fi]; __fi += 1;
                                __reg_moves.push((true, __reg, __arg));
                            } else {
                                __stack.push((__idx, __arg));
                            }
                        } else if __idx < #reg_limit && __gi < #n_gpr {
                            let __reg = [#(Reg::#gpr_idents),*][__gi]; __gi += 1;
                            __reg_moves.push((false, __reg, __arg));
                        } else {
                            __stack.push((__idx, __arg));
                        }
                        __idx += 1;
                    }
                    // 栈参数区：shadow space + 按位置压栈（每槽 stack_slot 字节），
                    // 总分配按 stack_align 对齐（call 前 sp%align == 0）。
                    let __stack_bytes = __stack.len() * #stack_slot;
                    let __shadow: usize = #shadow_space;
                    let __pad: usize =
                        (#stack_align - (__shadow + __stack_bytes) % #stack_align) % #stack_align;
                    let __total = __shadow + __stack_bytes + __pad;
                    if __total > 0 {
                        __pack.push_inst(Inst::#stack_alloc {
                            dest: Reg::#sp_reg,
                            imm: __total as u32,
                        });
                    }
                    // 栈上参数：地址计算指令 + store（scratch 寄存器）
                    // 地址用虚拟 XReg（regalloc 管理）而非物理 scratch（R10）——
                    // 物理寄存器是 regalloc 盲区：参数 XReg 的活区间加载会覆盖
                    // R10 里的地址，store 写到错误地址（five_args_stack SEGV）。
                    for &(__pos, __arg) in __stack.iter() {
                        let __off =
                            (#shadow_space + #stack_slot * (__pos - #reg_limit)) as i64;
                        let __addr: crate::prelude::XReg =
                            ctx.alloc_xreg(crate::prelude::RegClass::GPR64);
                        let __ii = __pack.push_inst(Inst::#stack_addr {
                            dest: <Reg as forge_ir::PhysReg>::from_index(0, crate::prelude::RegClass::GPR64),
                            base: Reg::#sp_reg,
                            index: Reg::#sp_reg,
                            scale: 0,
                            disp: __off,
                        });
                        __pack.map_reg_field(__addr, __ii, 0, false);
                        let __ii = __pack.push_inst(Inst::#stack_store {
                            base: <Reg as forge_ir::PhysReg>::from_index(0, crate::prelude::RegClass::GPR64),
                            src: <Reg as forge_ir::PhysReg>::from_index(0, crate::prelude::RegClass::GPR64),
                            opsize: #stack_opsize,
                        });
                        __pack.map_reg_field(__addr, __ii, 0, false);
                        __pack.map_reg_field(__arg, __ii, 1, false);
                    }
                    // 寄存器参数 mov（在栈 store 之后）
                    for (__is_fp, __dest, __arg) in __reg_moves {
                        if __is_fp {
                            let __ii = __pack.push_inst(Inst::#arg_mov_f {
                                dest: __dest,
                                src: <Reg as forge_ir::PhysReg>::from_index(0, crate::prelude::RegClass::FPR64),
                            });
                            __pack.map_reg_field(__arg, __ii, 0, false);
                        } else {
                            let __ii = __pack.push_inst(Inst::#arg_mov {
                                dest: __dest,
                                src: <Reg as forge_ir::PhysReg>::from_index(0, crate::prelude::RegClass::GPR64),
                                opsize: #arg_opsize,
                            });
                            __pack.map_reg_field(__arg, __ii, 0, false);
                        }
                    }
                    let __func = ctx.current_func_ref.map(|f| f.0 as i32).unwrap_or(0);
                    __pack.push_inst(Inst::#call_inst { #call_field: __func });
                    if __total > 0 {
                        __pack.push_inst(Inst::#stack_free {
                            dest: Reg::#sp_reg,
                            imm: __total as u32,
                        });
                    }
                    for (__ri, &__r) in results.iter().enumerate() {
                        if __r.class().is_fp() {
                            let __ii = __pack.push_inst(Inst::#ret_mov_f {
                                dest: <Reg as forge_ir::PhysReg>::from_index(0, crate::prelude::RegClass::FPR64),
                                src: #ret_xmm_expr,
                            });
                            __pack.map_reg_field(__r, __ii, 0, true);
                        } else {
                            // 整数返回值按序取 ret_regs.gpr（RAX/RDX/...）——
                            // ScalarPair 如 (i32,bool) 是 RAX+RDX 两个标量
                            let __ii = __pack.push_inst(Inst::#ret_mov {
                                dest: <Reg as forge_ir::PhysReg>::from_index(0, crate::prelude::RegClass::GPR64),
                                src: [#(Reg::#ret_gprs),*][__ri],
                                opsize: ctx.default_opsize,
                            });
                            __pack.map_reg_field(__r, __ii, 0, true);
                        }
                        __pack.outputs.push(__r);
                    }
                    #call_clobber_set
                    Ok(__pack)
                }
            });
            continue;
        }

        let has_variants = !rule.variants.is_empty();
        let lowering = crate::cst_codegen::gen_lowering_insts_cst(&rule.insts, model)?;
        let inst_toks = lowering.insts;
        let temp_toks = lowering.temps;

        // variants 全不匹配时的回退：规则带默认 insts（rule.insts 非空）→
        // 执行默认 insts（同类型 bitcast 等）；纯 variants 规则 → Unsupported。
        let fallback_expr = if rule.insts.is_empty() {
            quote! { return Err(crate::prelude::IrError::Unsupported("rule variant".into())) }
        } else {
            quote! { { #(#temp_toks);*; #(#inst_toks)*; } }
        };
        // variants：按 rs1 操作数位宽运行时分派（"rs1<=16" / "rs1==32" 等）。
        let variant_arms: Vec<TokenStream> = if has_variants {
            rule.variants
                .iter()
                .map(|v| {
                    let l = crate::cst_codegen::gen_lowering_insts_cst(&v.insts, model)?;
                    let vt = l.temps;
                    let vi = l.insts;
                    let cond = parse_when_cond(v.when.as_deref().unwrap_or_default())?;
                    Ok(quote! { () if #cond => { #(#vt);*; #(#vi)*; } })
                })
                .collect::<Result<Vec<_>, String>>()?
        } else {
            vec![]
        };
        let dispatch_toks = if has_variants {
            quote! {
                // rs1 位宽：动态 vector/scalable 按 size_bytes（bits() 对 interned
                // 类型返回 0——如 <4 x f64>），否则内建位宽。
                let __sb = match (&ctx.type_ctx, ctx.xreg_types.get(&rs1)) {
                    (Some(tc), Some(t)) => {
                        let store = tc.borrow();
                        if store.is_vector(*t) || store.is_scalable_vector(*t) {
                            store.size_bytes(*t) * 8
                        } else {
                            t.bits()
                        }
                    }
                    (None, Some(t)) => t.bits(),
                    _ => 64,
                };
                // 结果位宽：动态 vector/scalable 类型按 size_bytes 计算（bits() 对
                // interned 类型返回 0），否则用内建位宽。
                let __rdb = match (&ctx.type_ctx, ctx.xreg_types.get(&rd)) {
                    (Some(tc), Some(t)) => {
                        let store = tc.borrow();
                        if store.is_vector(*t) || store.is_scalable_vector(*t) {
                            store.size_bytes(*t) * 8
                        } else {
                            t.bits()
                        }
                    }
                    (None, Some(t)) => t.bits(),
                    _ => 64,
                };
                // 元素类型：优先 rd（结果）——vconst 无 rs1 时 rd 是向量；vextract 的
                // rd 是标量（element_type 返回 None）→ fallback rs1（向量）。
                // 聚合类型（struct/array——InsertValue 的 rd/rs1）无元素类型——
                // 跳过聚合继续试 rs2（InsertValue 的字段值），标量则用自身。
                let __elem = {
                    let mut found = crate::prelude::TypeId::VOID;
                    for x in [rd, rs1, rs2] {
                        if let (Some(tc), Some(t)) = (&ctx.type_ctx, ctx.xreg_types.get(&x)) {
                            let s = tc.borrow();
                            if let Some(e) = s.element_type(*t) {
                                found = e;
                                break;
                            }
                            if !s.is_aggregate(*t) {
                                found = *t;
                                break;
                            }
                        }
                    }
                    found
                };
                // rs1 的源类型（bitcast 等需要区分源/结果方向的指令——elem 只
                // 表达 rd 优先的结果类型，无法区分 i64→f64 与 f64→i64）。
                let __rs1elem = {
                    let mut found = crate::prelude::TypeId::VOID;
                    if let (Some(tc), Some(t)) = (&ctx.type_ctx, ctx.xreg_types.get(&rs1)) {
                        let s = tc.borrow();
                        if let Some(e) = s.element_type(*t) {
                            found = e;
                        } else if !s.is_aggregate(*t) {
                            found = *t;
                        }
                    }
                    found
                };
                // 变体全不匹配时回退默认 insts（规则带默认 insts 时——如
                // Bitcast 的同类型直 mov / Fadd 的 F64 movsd+addsd）；纯
                // variants 规则（无默认 insts）保持 Unsupported（rule variant）。
                match () {
                    #(#variant_arms,)*
                    _ => #fallback_expr,
                }
            }
        } else {
            quote! { #(#temp_toks);*; #(#inst_toks)*; }
        };

        // 检查是否需要 destructure index 字段 (Iconst/Fconst/Vconst)——含 variants 分支。
        let all_insts: Vec<String> = rule
            .insts
            .iter()
            .chain(rule.variants.iter().flat_map(|v| v.insts.iter()))
            .cloned()
            .collect();
        let uses_const = all_insts.iter().any(|s| {
            s.contains("iconst")
                || s.contains("fconst")
                || s.contains("vconst_lo")
                || s.contains("vconst_hi")
                || s.contains("vconst_lo2")
                || s.contains("vconst_lo_hi")
                || s.contains("vconst_hi_hi")
                || s.contains("vconst_q0")
                || s.contains("vconst_q1")
                || s.contains("vconst_q2")
                || s.contains("vconst_q3")
        });

        // 规则 clobbers（写死物理寄存器）→ 物理编号，包内所有指令点避开。
        // 完全自动推导：写死寄存器直接写在 insts 操作数里（如 "mov RAX, rs1"），
        // 无需额外声明；隐式使用（如 x86 div 的 RDX、@shift_reg 的 CL）由
        // 指令编码原语自身保证（@shift_reg 自动搬 CL，div 编码内建）。
        let clobber_names = crate::cst_codegen::collect_phys_regs(&all_insts, model);
        let clobber_toks: Vec<TokenStream> = clobber_names
            .iter()
            .map(|r| {
                let (is_fp, idx) = crate::codegen::components::resolve_reg_index(model, r);
                if is_fp {
                    quote! { (#idx, crate::prelude::RegClass::FPR64) }
                } else {
                    quote! { (#idx, crate::prelude::RegClass::GPR64) }
                }
            })
            .collect();
        let clobber_set = if clobber_toks.is_empty() {
            quote! {}
        } else {
            quote! { ctx.current_clobbers = vec![#(#clobber_toks),*]; }
        };

        if uses_const {
            arms.push(quote! {
                crate::prelude::Opcode::#op_ident => {
                    let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let index: u32 = ctx.current_const_index;
                    #dispatch_toks
                    #clobber_set
                    Ok(__pack)
                }
            });
        } else {
            arms.push(quote! {
                crate::prelude::Opcode::#op_ident { .. } => {
                    let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    #dispatch_toks
                    #clobber_set
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
                let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                match ctx.current_atomic_op.unwrap_or(crate::prelude::AtomicRmwOp::Add) {
                    #(#op_arms),*
                    _ => Err(crate::prelude::IrError::Unsupported("atomic op".into()))
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
                let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                match cond { #(#cond_arms),* _ => Err(crate::prelude::IrError::Unsupported("icmp condition".into())) }
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
            if rule.variants.is_empty() {
                cond_arms.push(quote! {
                    crate::prelude::FloatCC::#cc_ident => { #(#temp_toks);*; #(#inst_toks)*; Ok(__pack) }
                });
            } else {
                // variants：按元素类型运行时分派（如 F32 → comiss / F64 → comisd）
                let var_arms: Vec<TokenStream> = rule
                    .variants
                    .iter()
                    .map(|v| {
                        let l = crate::cst_codegen::gen_lowering_insts_cst(&v.insts, model)?;
                        let vt = l.temps;
                        let vi = l.insts;
                        let cond = parse_when_cond(v.when.as_deref().unwrap_or_default())?;
                        Ok(quote! { () if #cond => { #(#vt);*; #(#vi)*; Ok(__pack) } })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                cond_arms.push(quote! {
                    crate::prelude::FloatCC::#cc_ident => {
                        match () { #(#var_arms),* _ => { #(#temp_toks);*; #(#inst_toks)*; Ok(__pack) } }
                    }
                });
            }
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
                let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                // 元素类型（fcmp 变体分派：F32 → comiss / F64 → comisd）
                let __elem = {
                    let mut found = crate::prelude::TypeId::VOID;
                    for x in [rd, rs1, rs2] {
                        if let (Some(tc), Some(t)) = (&ctx.type_ctx, ctx.xreg_types.get(&x)) {
                            let s = tc.borrow();
                            if let Some(e) = s.element_type(*t) {
                                found = e;
                                break;
                            }
                            if !s.is_aggregate(*t) {
                                found = *t;
                                break;
                            }
                        }
                    }
                    found
                };
                match cond { #(#cond_arms),* _ => Err(crate::prelude::IrError::Unsupported("fcmp condition".into())) }
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
                            let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                            let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                            let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                            let index: u32 = ctx.current_const_index;
                            #(#inst_toks)*;
                            Ok(__pack)
                        }
                    });
                } else {
                    arms.push(quote! {
                        crate::prelude::Opcode::#op_ident { .. } => {
                            let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                            let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                            let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
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
        _ => Err(crate::prelude::IrError::Unsupported(format!("lower {:?}", op)))
    });

    Ok(quote! {
        pub fn lower_impl(
            op: &crate::prelude::Opcode,
            args: &[crate::prelude::XReg],
            results: &[crate::prelude::XReg],
            ctx: &mut crate::prelude::LowerCtx,
        ) -> Result<crate::prelude::InstPacket<Inst>, crate::prelude::IrError> {
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
        let mut reg_field_idx: usize = 0;
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
                let is_phys = matches!(
                    crate::codegen::lookup_reg_in_model(arg_val, model),
                    crate::codegen::RegLookup::Named { .. }
                        | crate::codegen::RegLookup::Prefixed { .. }
                );
                let cls = if matches!(field.field_type, crate::model::FieldType::Freg) {
                    quote! { crate::prelude::RegClass::FPR64 }
                } else {
                    quote! { crate::prelude::RegClass::GPR64 }
                };
                if is_phys {
                    let (is_float, reg_idx) =
                        crate::codegen::components::resolve_reg_index(model, arg_val);
                    let pcls = if is_float {
                        quote! { crate::prelude::RegClass::FPR64 }
                    } else {
                        quote! { crate::prelude::RegClass::GPR64 }
                    };
                    let reg_idx_lit =
                        syn::LitInt::new(&reg_idx.to_string(), proc_macro2::Span::call_site());
                    field_exprs.push(quote! {
                        #fi: { let __v: Reg = <Reg as forge_ir::PhysReg>::from_index(#reg_idx_lit, #pcls); __v }
                    });
                } else {
                    let xreg_expr =
                        lowering_arg_expr(arg_val, &field.field_type, scratch, Some(model));
                    let field_idx_lit = syn::LitInt::new(
                        &reg_field_idx.to_string(),
                        proc_macro2::Span::call_site(),
                    );
                    let is_def = crate::codegen::is_field_def(field);
                    field_exprs.push(quote! {
                        #fi: <Reg as forge_ir::PhysReg>::from_index(0, #cls)
                    });
                    reg_map_calls.push(quote! {
                        // 旧格式 lowering 生成路径：按字段名判定 use/def（dest/reg 为 def）
                        __pack.map_reg_field(#xreg_expr, __idx, #field_idx_lit, #is_def);
                    });
                    reg_field_idx += 1;
                }
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
                // Opsize 字段在 lowering 场景取 ctx.default_opsize（按 IR 类型推断，
                // i32 → 32 位访问）；固定 64 会导致 i32 的 load/store 用 8 字节
                // 访问，槽位重叠（相邻局部变量互相污染 → mini_c 循环返回错误值）。
                if matches!(field.field_type, crate::model::FieldType::Opsize) {
                    field_exprs.push(quote! { #fi: ctx.default_opsize });
                } else {
                    let default = default_for_type(&field.field_type);
                    field_exprs.push(quote! { #fi: #default });
                }
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
/// 解析 variants 的 `when` 宽度谓词 → Rust 布尔表达式。
///
/// 支持 `"rs1<16"` / `"rs1<=16"` / `"rs1==32"` / `"rs1>=16"` / `"rs1>16"` / `"rs1!=64"`
/// 等形式（位宽单位：bit）——生成代码中 `__sb` 为 rs1 操作数的位宽；
/// `"rd==128"` / `"rd>=256"` 等结果位宽谓词——`__rdb` 为结果位宽；
/// 也支持 `"imm0==0"` / `"imm0<4"` / `"imm0>=4"` 等 immediate 谓词（全部 6 个比较符）。
fn parse_when_cond(when: &str) -> Result<TokenStream, String> {
    let s = when.trim();
    // 剥离外层括号（如 "rd<=128 && (elem==F32 || elem==I32)" 递归后的 `(elem==F32 || elem==I32)`）。
    let s = s
        .strip_prefix('(')
        .and_then(|x| x.strip_suffix(')'))
        .unwrap_or(s);
    // 复合条件：`"rd==256 && elem==F32"` / `"imm0==0 || elem==F64"` →
    // 递归解析后 &&/|| 连接（子条件整体加括号，防 `A && B || C` 的优先级歧义——
    // 如 `rd==128 && (elem==F32 || elem==I32)` 必须解析为
    // `__rdb==128 && (__elem==F32 || __elem==I32)` 而非 `(__rdb==128 && __elem==F32) || __elem==I32`）。
    if let Some((l, r)) = s.split_once("&&") {
        let lc = parse_when_cond(l.trim())?;
        let rc = parse_when_cond(r.trim())?;
        return Ok(quote! { (#lc) && (#rc) });
    }
    if let Some((l, r)) = s.split_once("||") {
        let lc = parse_when_cond(l.trim())?;
        let rc = parse_when_cond(r.trim())?;
        return Ok(quote! { (#lc) || (#rc) });
    }
    // rs1/rd 位宽谓词 + immN 数值谓词：统一在 6 个操作符上匹配。
    let ops: [(&str, TokenStream); 6] = [
        ("<=", quote! { <= }),
        ("==", quote! { == }),
        (">=", quote! { >= }),
        ("!=", quote! { != }),
        ("<", quote! { < }),
        (">", quote! { > }),
    ];
    for (op, tok) in ops {
        if let Some((lhs, rhs)) = s.split_once(op) {
            let lhs = lhs.trim();
            if let Some(idx) = lhs
                .strip_prefix("imm")
                .and_then(|d| d.parse::<usize>().ok())
            {
                let n: i64 = rhs.trim().parse().map_err(|_| {
                    format!("invalid when condition '{when}': value must be a number")
                })?;
                let n_lit = syn::LitInt::new(&n.to_string(), proc_macro2::Span::call_site());
                let imm_expr =
                    quote! { (ctx.current_immediates.get(#idx).copied().unwrap_or(0) as i64) };
                return Ok(quote! { #imm_expr #tok #n_lit });
            }
            let var = match lhs {
                "rs1" => quote! { __sb },
                "rd" => quote! { __rdb },
                "elem" => {
                    // 元素类型谓词（向量元素 TypeId，如 elem==F64 / elem==I32）：
                    // 非位宽比较——rhs 是 TypeId 名。
                    let ty_name = rhs.trim();
                    let ty_ident = syn::Ident::new(ty_name, proc_macro2::Span::call_site());
                    return Ok(quote! { __elem == crate::prelude::TypeId::#ty_ident });
                }
                "rs1elem" => {
                    // rs1 的（元素）类型谓词——区分源/结果方向的指令（bitcast）。
                    let ty_name = rhs.trim();
                    let ty_ident = syn::Ident::new(ty_name, proc_macro2::Span::call_site());
                    return Ok(quote! { __rs1elem == crate::prelude::TypeId::#ty_ident });
                }
                other => {
                    return Err(format!(
                        "invalid when condition '{when}': only 'rs1'/'rd'/'elem'/'immN' operands supported (got '{other}')"
                    ));
                }
            };
            let n: u32 = rhs
                .trim()
                .parse()
                .map_err(|_| format!("invalid when condition '{when}': width must be a number"))?;
            let n_lit = syn::LitInt::new(&n.to_string(), proc_macro2::Span::call_site());
            return Ok(quote! { #var #tok #n_lit });
        }
    }
    Err(format!(
        "invalid when condition '{when}': expected e.g. \"rs1<=16\" or \"imm0==0\" or \"elem==F64\""
    ))
}

pub(crate) fn lowering_arg_expr(
    val: &str,
    ft: &FieldType,
    scratch: Option<&std::collections::BTreeMap<String, u32>>,
    model: Option<&IsaModel>,
) -> TokenStream {
    // 0. MemRef 字段：`[RAX+8]` 内存操作数 → MemRef（base=物理编号, offset, width=8）
    if matches!(ft, FieldType::MemRef) {
        if let Ok(mem) = crate::asm_resolver::decompose_mem_operand(val) {
            let base_num = match &mem.base {
                Some(b) => model
                    .map(|m| crate::codegen::components::resolve_reg_index(m, b).1)
                    .unwrap_or(0),
                None => 0,
            };
            let disp = mem.disp;
            return quote! { crate::prelude::MemRef::new(#base_num, #disp, 8) };
        }
        return quote! { crate::prelude::MemRef::unresolved(8) };
    }
    // 0b. Constant pool inline: `{const 42}` or `{const 3.14}`
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
        return quote! { crate::prelude::XReg::new(#idx as u32, crate::prelude::RegClass::GPR64, 8) };
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
                    quote! { crate::prelude::RegClass::FPR64 }
                } else {
                    quote! { crate::prelude::RegClass::GPR64 }
                };
                return quote! { crate::prelude::XReg::new(#vreg_idx as u32, #cls, 8) };
            }
            RegLookup::NotFound => {}
        }
    }

    // 2b. 向量常量字节还原 helper：字节 → 64 位半部。
    // 元素按指定端序（常量池 get_vector_endian，默认 Little）连续存储；
    // 元素位宽由 rd 向量类型决定：
    //   - 32 位元素（f32/i32）：punpckldq 交错恢复需要"奇偶分离"——
    //     vconst_lo = 元素0|元素2<<32（旧 lanes[0]|lanes[2]<<32 等价）、
    //     vconst_hi = 元素1|元素3<<32；块 i 低 32 = bytes[8i..8i+4]、
    //     高 32 = bytes[8i+8..8i+12]（元素 2i 和 2i+2）。
    //   - 64 位元素（f64/i64）：连续块，块 i = bytes[8i..8i+8]。
    // 端序：LE → from_le_bytes、BE → from_be_bytes（32 位元素逐元素 BE 读）。
    let vc_pair = |i: usize| -> TokenStream {
        // 32 位元素奇偶分离：vconst_lo = [e0,e2]、vconst_hi = [e1,e3]、
        // vconst_lo_hi = [e4,e6]、vconst_hi_hi = [e5,e7]（punpckldq 交错恢复）。
        // 低 offset = (i&!1)*8 + (i&1)*4（i=0→0、1→4、2→16、3→20）、高 = 低+8。
        let lo_off = (i & !1) * 8 + (i & 1) * 4;
        let hi_off = lo_off + 8;
        let cont_off = i * 8;
        quote! {
            ctx.constant_pool.as_ref().and_then(|p| p.get_vector(crate::prelude::ConstId(index)).map(|b| {
                let __be = p.get_vector_endian(crate::prelude::ConstId(index))
                    == Some(crate::prelude::Endianness::Big);
                let __eb = ctx.type_ctx.as_ref()
                    .zip(ctx.xreg_types.get(&rd).copied())
                    .and_then(|(tc, t)| { let s = tc.borrow(); s.element_type(t).map(|e| s.size_bytes(e) as usize) })
                    .unwrap_or(8);
                if __eb <= 4 {
                    if __be {
                        let __lo = u32::from_be_bytes([
                            *b.get(#lo_off).unwrap_or(&0), *b.get(#lo_off + 1).unwrap_or(&0),
                            *b.get(#lo_off + 2).unwrap_or(&0), *b.get(#lo_off + 3).unwrap_or(&0),
                        ]) as u64;
                        let __hi = u32::from_be_bytes([
                            *b.get(#hi_off).unwrap_or(&0), *b.get(#hi_off + 1).unwrap_or(&0),
                            *b.get(#hi_off + 2).unwrap_or(&0), *b.get(#hi_off + 3).unwrap_or(&0),
                        ]) as u64;
                        __lo | (__hi << 32)
                    } else {
                        let __lo = u32::from_le_bytes([
                            *b.get(#lo_off).unwrap_or(&0), *b.get(#lo_off + 1).unwrap_or(&0),
                            *b.get(#lo_off + 2).unwrap_or(&0), *b.get(#lo_off + 3).unwrap_or(&0),
                        ]) as u64;
                        let __hi = u32::from_le_bytes([
                            *b.get(#hi_off).unwrap_or(&0), *b.get(#hi_off + 1).unwrap_or(&0),
                            *b.get(#hi_off + 2).unwrap_or(&0), *b.get(#hi_off + 3).unwrap_or(&0),
                        ]) as u64;
                        __lo | (__hi << 32)
                    }
                } else {
                    let mut __buf = [0u8; 8];
                    let __n = b.len().saturating_sub(#cont_off).min(8);
                    __buf[..__n].copy_from_slice(&b[#cont_off..#cont_off + __n]);
                    if __be {
                        u64::from_be_bytes(__buf)
                    } else {
                        u64::from_le_bytes(__buf)
                    }
                }
            })).unwrap_or(0) as i64
        }
    };
    // 连续 64 位块（vconst_lo2 = V64 movq 连续 8 字节；vconst_qN = 64 位元素逐元素）。
    let vc_cont = |i: usize| -> TokenStream {
        let off = i * 8;
        quote! {
            ctx.constant_pool.as_ref().and_then(|p| p.get_vector(crate::prelude::ConstId(index)).map(|b| {
                let __be = p.get_vector_endian(crate::prelude::ConstId(index))
                    == Some(crate::prelude::Endianness::Big);
                let __eb = ctx.type_ctx.as_ref()
                    .zip(ctx.xreg_types.get(&rd).copied())
                    .and_then(|(tc, t)| { let s = tc.borrow(); s.element_type(t).map(|e| s.size_bytes(e) as usize) })
                    .unwrap_or(8);
                let mut __buf = [0u8; 8];
                let __n = b.len().saturating_sub(#off).min(8);
                __buf[..__n].copy_from_slice(&b[#off..#off + __n]);
                if __eb <= 4 {
                    // V64（<2 x f32> movq 连续 8 字节）：32 位元素逐元素读。
                    // BE 时块值 = e0 | e1<<32（元素序不变，字节序按端序）。
                    let __lo = if __be {
                        u32::from_be_bytes([__buf[0], __buf[1], __buf[2], __buf[3]]) as u64
                    } else {
                        u32::from_le_bytes([__buf[0], __buf[1], __buf[2], __buf[3]]) as u64
                    };
                    let __hi = if __be {
                        u32::from_be_bytes([__buf[4], __buf[5], __buf[6], __buf[7]]) as u64
                    } else {
                        u32::from_le_bytes([__buf[4], __buf[5], __buf[6], __buf[7]]) as u64
                    };
                    __lo | (__hi << 32)
                } else if __be {
                    u64::from_be_bytes(__buf)
                } else {
                    u64::from_le_bytes(__buf)
                }
            })).unwrap_or(0) as i64
        }
    };

    // 3. Position-dependent lowering variables
    match val {
        "rd" => quote! { rd },
        "r2" => quote! { r2 },
        "rs1" => quote! { rs1 },
        "rs2" => quote! { rs2 },
        // 第三临时结果（无第三个 IR 结果时分配新 XReg，如 UmulOverflow
        // 的组合标志检测）。活区间由 regalloc 管理。
        "r3" => {
            quote! { results.get(2).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r }) }
        }
        "rs3" => {
            quote! { args.get(2).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r }) }
        }
        // Call argument registers (fixed positions). Beyond the register
        // count the operand falls back to VReg(0) (harmless extra move).
        "arg0" => {
            quote! { args.get(0).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r }) }
        }
        "arg1" => {
            quote! { args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r }) }
        }
        "arg2" => {
            quote! { args.get(2).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r }) }
        }
        "arg3" => {
            quote! { args.get(3).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r }) }
        }
        "arg4" => {
            quote! { args.get(4).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r }) }
        }
        "arg5" => {
            quote! { args.get(5).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r }) }
        }
        "arg6" => {
            quote! { args.get(6).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r }) }
        }
        "arg7" => {
            quote! { args.get(7).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r }) }
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
        // Alloca 的帧槽偏移（预扫描分配——`lea_off rd, alloca_offset` 规则用）。
        "alloca_offset" => quote! { ctx.current_alloca_offset },
        "zero" => quote! { ctx.alloc_zero_vreg() },
        "iconst" => {
            quote! { ctx.constant_pool.as_ref().and_then(|p| p.resolve_int(crate::prelude::ConstId(index))).unwrap_or(0) as i64 }
        }
        "fconst" => {
            quote! { ctx.constant_pool.as_ref().and_then(|p| p.resolve_float(crate::prelude::ConstId(index))).map(|b| b as i64).unwrap_or(0) }
        }
        // Vconst 字节还原：见 vc_pair/vc_cont 注释（32 位元素奇偶分离 / 64 位连续块）。
        //   <4 x f32> 16 字节：lo = f0|f2<<32、hi = f1|f3<<32（punpckldq 恢复）
        //   <2 x f32> 8 字节：lo2 = f0|f1<<32（movq 一次装 2×f32，连续）
        //   <8 x f32>/<8 x i32> 32 字节：lo/hi/lo_hi/hi_hi = 奇偶分离 4 组
        //   <4 x f64>/<4 x i64> 32 字节：q0-3 = 逐元素连续 64 位
        "vconst_lo" => vc_pair(0),
        "vconst_hi" => vc_pair(1),
        "vconst_lo2" => vc_cont(0),
        "vconst_lo_hi" => vc_pair(2),
        "vconst_hi_hi" => vc_pair(3),
        "vconst_q0" => vc_cont(0),
        "vconst_q1" => vc_cont(1),
        "vconst_q2" => vc_cont(2),
        "vconst_q3" => vc_cont(3),
        // ShuffleVector 的 mask lane（imm0..=imm3 = mask[0..4]）。
        "imm0" => {
            let cast = match ft {
                FieldType::I8 | FieldType::U8 | FieldType::CondCode => quote! { as u8 },
                FieldType::I16 | FieldType::U16 => quote! { as i16 },
                FieldType::I32 | FieldType::U32 => quote! { as i32 },
                _ => quote! { as i64 },
            };
            quote! { (ctx.current_immediates.get(0).copied().unwrap_or(0)) #cast }
        }
        // V256 lane 4-7：片段内选择码 = lane - 4（vextractf128 取高半后 pshufd）。
        "imm0_sub4" => {
            let cast = match ft {
                FieldType::I8 | FieldType::U8 | FieldType::CondCode => quote! { as u8 },
                FieldType::I16 | FieldType::U16 => quote! { as i16 },
                FieldType::I32 | FieldType::U32 => quote! { as i32 },
                _ => quote! { as i64 },
            };
            quote! { ((ctx.current_immediates.get(0).copied().unwrap_or(0)).saturating_sub(4)) #cast }
        }
        // ExtractValue/InsertValue 的字段位偏移：imm0 × 字段位宽（8/16/32/64）。
        "imm0_mul8" => {
            // 移位量用于 mov_imm 的 i64 imm 字段（shr 计数在寄存器——cast 无影响）
            quote! { (ctx.current_immediates.get(0).copied().unwrap_or(0)).saturating_mul(8) as i64 }
        }
        "imm0_mul16" => {
            // 移位量用于 mov_imm 的 i64 imm 字段（shr 计数在寄存器——cast 无影响）
            quote! { (ctx.current_immediates.get(0).copied().unwrap_or(0)).saturating_mul(16) as i64 }
        }
        "imm0_mul32" => {
            // 移位量用于 mov_imm 的 i64 imm 字段（shr 计数在寄存器——cast 无影响）
            quote! { (ctx.current_immediates.get(0).copied().unwrap_or(0)).saturating_mul(32) as i64 }
        }
        "imm0_mul64" => {
            // 移位量用于 mov_imm 的 i64 imm 字段（shr 计数在寄存器——cast 无影响）
            quote! { (ctx.current_immediates.get(0).copied().unwrap_or(0)).saturating_mul(64) as i64 }
        }
        "imm1" => quote! { ctx.current_immediates.get(1).copied().unwrap_or(0) },
        "imm2" => quote! { ctx.current_immediates.get(2).copied().unwrap_or(0) },
        "imm3" => quote! { ctx.current_immediates.get(3).copied().unwrap_or(0) },
        // SHUFPS imm8 编码（Intel 语义：低 4 位选 src1、高 4 位选 src2）：
        // dst[0]←src1[m0]、dst[1]←src1[m1]、dst[2]←src2[m2-4]、dst[3]←src2[m3-4]。
        "shufps_imm8" => {
            let cast = match ft {
                FieldType::I8 | FieldType::U8 | FieldType::CondCode => quote! { as u8 },
                FieldType::I16 | FieldType::U16 => quote! { as i16 },
                FieldType::I32 | FieldType::U32 => quote! { as i32 },
                _ => quote! { as i64 },
            };
            quote! {
                (
                    ((ctx.current_immediates.get(3).copied().unwrap_or(0).wrapping_sub(4)) & 0x3) << 6
                    | (((ctx.current_immediates.get(2).copied().unwrap_or(0).wrapping_sub(4)) & 0x3) << 4)
                    | ((ctx.current_immediates.get(1).copied().unwrap_or(0) & 0x3) << 2)
                    | (ctx.current_immediates.get(0).copied().unwrap_or(0) & 0x3)
                ) #cast
            }
        }
        // V256 ShuffleVector 高 128 位组：mask[4..8]（imm4-7 对应高组 4 lane）。
        "shufps_imm8_hi" => {
            let cast = match ft {
                FieldType::I8 | FieldType::U8 | FieldType::CondCode => quote! { as u8 },
                FieldType::I16 | FieldType::U16 => quote! { as i16 },
                FieldType::I32 | FieldType::U32 => quote! { as i32 },
                _ => quote! { as i64 },
            };
            quote! {
                (
                    ((ctx.current_immediates.get(7).copied().unwrap_or(0).wrapping_sub(4)) & 0x3) << 6
                    | (((ctx.current_immediates.get(6).copied().unwrap_or(0).wrapping_sub(4)) & 0x3) << 4)
                    | ((ctx.current_immediates.get(5).copied().unwrap_or(0) & 0x3) << 2)
                    | (ctx.current_immediates.get(4).copied().unwrap_or(0) & 0x3)
                ) #cast
            }
        }
        // 4. %name temporary VReg — expands to `__vreg_<name>` variable (declared by gen_lowering_insts_cst)
        //    `%name:fpr` 的类型后缀在 collect_temp_specs 已处理，这里只取 name 部分。
        s if s.starts_with('%') => {
            let (name, _) = s[1..].split_once(':').unwrap_or((&s[1..], ""));
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
                FieldType::Freg => quote! { crate::prelude::RegClass::FPR64 },
                _ => quote! { crate::prelude::RegClass::GPR64 },
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
            quote! { <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::GPR64) }
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
    // MemRef 字段：`[RAX+8]` 内存操作数 → MemRef（base=物理编号, offset, width=8）
    if matches!(ft, FieldType::MemRef) {
        if let Ok(mem) = crate::asm_resolver::decompose_mem_operand(val) {
            let base_num = match &mem.base {
                Some(b) => crate::codegen::components::resolve_reg_index(model, b).1,
                None => 0,
            };
            let disp = mem.disp;
            return quote! { crate::prelude::MemRef::new(#base_num, #disp, 8) };
        }
        return quote! { crate::prelude::MemRef::unresolved(8) };
    }
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
                quote! { forge_ir::RegClass::FPR64 }
            } else {
                quote! { forge_ir::RegClass::GPR64 }
            };
            return quote! { <Reg as forge_ir::PhysReg>::from_index(#index, #cls) };
        }
        RegLookup::NotFound => {}
    }

    // Scratch register aliases
    if let Some(scratch) = model.abi.as_ref().and_then(|a| a.scratch.get(val)) {
        return quote! { <Reg as forge_ir::PhysReg>::from_index(#scratch, forge_ir::RegClass::GPR64) };
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

    for asm_str in insts {
        let trimmed = asm_str.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(preudo) = trimmed.strip_prefix("@") {
            crate::codegen::flush_emit_stmts(
                &mut pending_regular,
                &mut all_stmts,
                &quote!(emit_inst),
                &quote!(rm),
                &quote!(&mut *sink),
            );
            crate::codegen::gen_pseudo_inst(preudo, model, &mut all_stmts);
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

    crate::codegen::flush_emit_stmts(
        &mut pending_regular,
        &mut all_stmts,
        &quote!(emit_inst),
        &quote!(rm),
        &quote!(&mut *sink),
    );

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

        // ── 收参骨架配置（全部来自 [abi.call].entry_*；缺失时编译期报错）──
        let call = abi.call.as_ref().expect("abi.call checked above");
        let (
            Some(entry_fp),
            Some(entry_load_scratch),
            Some(entry_stack_load),
            Some(entry_sext),
            Some(entry_mov),
            Some(entry_gpr_to_fp),
            Some(stack_addr),
            Some(addr_scratch),
            Some(sp_reg),
        ) = (
            call.entry_fp_reg.as_ref(),
            call.entry_load_scratch.as_ref(),
            call.entry_stack_load_inst.as_ref(),
            call.entry_sext_inst.as_ref(),
            call.entry_mov_inst.as_ref(),
            call.entry_gpr_to_fp_inst.as_ref(),
            call.stack_addr_inst.as_ref(),
            call.stack_addr_scratch.as_ref(),
            call.sp_reg
                .as_deref()
                .or_else(|| abi.frame.as_ref().map(|f| f.sp.as_str())),
        )
        else {
            return quote! {
                compile_error!("[abi.call] requires entry_fp_reg / entry_load_scratch / entry_stack_load_inst / entry_sext_inst / entry_mov_inst / entry_gpr_to_fp_inst / stack_addr_inst / stack_addr_scratch / sp_reg for @move_args parameter receipt");
            };
        };
        let entry_fp = format_ident!("{entry_fp}");
        let entry_load_scratch = format_ident!("{entry_load_scratch}");
        let entry_stack_load = pascal_ident(entry_stack_load);
        let entry_sext = pascal_ident(entry_sext);
        let entry_mov = pascal_ident(entry_mov);
        let entry_gpr_to_fp = pascal_ident(entry_gpr_to_fp);
        let stack_addr = pascal_ident(stack_addr);
        let addr_scratch = format_ident!("{addr_scratch}");
        let sp_reg = format_ident!("{sp_reg}");
        let reg_limit = call.reg_arg_limit as usize;
        let shadow_space = call.shadow_space as usize;
        let stack_slot = call.stack_slot_size as usize;
        let fp_push_bytes = abi.fp_push_bytes as usize;
        let ret_addr_bytes = call.entry_ret_addr_bytes as usize;

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
                // 寄存器/栈判定按全局位置 __i（前 reg_limit 个位置用寄存器，第
                // reg_limit+1 位置起压栈），不能按 gi/fi（类型独立计数）——
                // 混合参数时第 reg_limit+1 个整数参数的 gi 可能 < reg_limit，
                // 但调用者已按位置把它压栈（栈参数槽位按参数列表位置分配）。
                if __is_float {
                    if __i < #reg_limit && __fi < #n_xmm {
                        let __reg = [#(Reg::#xmm_src_idents),*][__fi];
                        __fi += 1;
                        if let Err(e) = emit_inst(&Inst::#fp_mov { dest: __dest, src: __reg }, rm, sink) {
                            return Err(e);
                        }
                    } else {
                        // 浮点栈参数（第 reg_limit+1 个起）：从 [fp+off] 加载。
                        // off = fp_push_bytes + 返回地址 + shadow + slot*(i-reg_limit)。
                        let __off = (#fp_push_bytes as i64 + #ret_addr_bytes as i64 + #shadow_space as i64)
                            + #stack_slot as i64 * (__i as i64 - #reg_limit as i64);
                        if let Err(e) = emit_inst(&Inst::#stack_addr {
                            dest: Reg::#addr_scratch,
                            base: Reg::#entry_fp,
                            index: Reg::#sp_reg,
                            scale: 0,
                            disp: __off,
                        }, rm, sink) {
                            return Err(e);
                        }
                        if let Err(e) = emit_inst(&Inst::#entry_stack_load { dest: Reg::#entry_load_scratch, base: Reg::#addr_scratch, opsize: 64 }, rm, sink) {
                            return Err(e);
                        }
                        if let Err(e) = emit_inst(&Inst::#entry_gpr_to_fp { dest: __dest, src: Reg::#entry_load_scratch }, rm, sink) {
                            return Err(e);
                        }
                    }
                } else if __i < #reg_limit && __gi < #n_gpr {
                    let __reg = [#(Reg::#gpr_src_idents),*][__gi];
                    __gi += 1;
                    // i32/u32 参数符号扩展收参（sext）；i64 直接 mov。
                    let __is_32 = rm.param_is_32.get(__i).copied().unwrap_or(false);
                    if __is_32 {
                        if let Err(e) = emit_inst(&Inst::#entry_sext { dest: __dest, src: __reg }, rm, sink) {
                            return Err(e);
                        }
                    } else if let Err(e) = emit_inst(&Inst::#entry_mov { dest: __dest, src: __reg, opsize: #opsize_default }, rm, sink) {
                        return Err(e);
                    }
                } else {
                    // 整数栈参数（第 reg_limit+1 个起）：从 [fp+off] 加载。
                    // i32 用 32 位加载（高 4 字节是压栈时的未定义垃圾）。
                    let __off = (#fp_push_bytes as i64 + #ret_addr_bytes as i64 + #shadow_space as i64)
                        + #stack_slot as i64 * (__i as i64 - #reg_limit as i64);
                    let __is_32 = rm.param_is_32.get(__i).copied().unwrap_or(false);
                    let __opsize = if __is_32 { 32u8 } else { 64u8 };
                    if let Err(e) = emit_inst(&Inst::#stack_addr {
                        dest: Reg::#addr_scratch,
                        base: Reg::#entry_fp,
                        index: Reg::#sp_reg,
                        scale: 0,
                        disp: __off,
                    }, rm, sink) {
                        return Err(e);
                    }
                    if let Err(e) = emit_inst(&Inst::#entry_stack_load { dest: __dest, base: Reg::#addr_scratch, opsize: __opsize }, rm, sink) {
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
    // 指令名来自 [abi.frame].alloc_inst / free_inst（默认沿用历史约定名，
    // 任何 ISA 均可通过 TOML 声明自己的帧分配指令）。
    let inst_name = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .and_then(|f| {
            if is_alloc {
                f.alloc_inst.as_deref()
            } else {
                f.free_inst.as_deref()
            }
        })
        .unwrap_or(if is_alloc {
            "SUB64_R_IMM32"
        } else {
            "ADD64_R_IMM32"
        });
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

    // SP register comes from [abi.frame].sp — required, no ISA fallback.
    let sp_name = match model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .map(|f| f.sp.clone())
    {
        Some(n) => n,
        None => {
            return quote! {{
                let _ = (frame_size, rm, sink);
                compile_error!("[abi.frame].sp is required for @frame_alloc/@frame_free — add [abi.frame] with sp = \"<SP register>\"");
            }};
        }
    };
    let sp_ident = syn::Ident::new(&sp_name, proc_macro2::Span::call_site());

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
                        .and_then(|x| v.get(&x)).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let val2 = values.get(1).copied()
                        .and_then(|x| v.get(&x)).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                };
                if has_sd_fmov {
                    arms.push(quote! {
                        crate::prelude::Terminator::Return { values, .. } => {
                            #val_code
                            if ctx.is_float_return {
                                {
                                    // SdFmov dest=返回FPR（字段0 固定 XMM0，不 map）、src=val（字段1 map）
                                    let __idx = __pack.push_inst(Inst::SdFmov { dest: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::FPR64), src: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::FPR64) });
                                    __pack.map_reg_field(val, __idx, 1u8, false);
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
                        crate::prelude::Terminator::Return { values, .. } => {
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
                        let cond = v.get(cond_val).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
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
                        let discriminant = v.get(discriminant).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
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
                    arms.push(quote! {
                        crate::prelude::Terminator::Return { values, .. } => {
                            let val = values.first().copied()
                                .and_then(|x| v.get(&x)).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                            if ctx.is_float_return {
                                {
                                    // SdFmov dest=返回FPR（字段0 固定 XMM0，不 map）、src=val（字段1 map）
                                    let __idx = __pack.push_inst(Inst::SdFmov { dest: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::FPR64), src: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::FPR64) });
                                    __pack.map_reg_field(val, __idx, 1u8, false);
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
                            let cond = v.get(cond_val).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
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
                        crate::prelude::Terminator::Switch { discriminant, default_block, default_args: _, cases, .. } => {
                            let disc = v.get(discriminant).copied().unwrap_or_else(|| ctx.alloc_xreg(crate::prelude::RegClass::GPR64));
                            for &(case_val, case_block, _) in cases.iter() {
                                // 每个 case 分配一个临时 XReg 装载常量
                                let tmp = ctx.alloc_xreg(crate::prelude::RegClass::GPR64);
                                let __idx = __pack.push_inst(Inst::SdMovImm { dest: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::GPR64), imm: case_val as i64 });
                                __pack.map_reg_field(tmp, __idx, 0u8, true);
                                // SD_CMP disc, tmp
                                let __idx = __pack.push_inst(Inst::SdCmp { src1: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::GPR64), src2: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::GPR64) });
                                __pack.map_reg_field(disc, __idx, 0u8, false);
                                __pack.map_reg_field(tmp, __idx, 1u8, false);
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
                crate::prelude::Terminator::Return { values, .. } => {
                    if let Some(&val) = values.first() {
                        if let Some(&vr) = v.get(&val) {
                            __pack.push_inst(Inst::MovR8Rm { dest: <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::GPR64), src: vr });
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
        _ => Err(crate::prelude::IrError::Unsupported(format!("lower_term {:?}", term)))
    });

    Ok(quote! {
        pub fn lower_terminator_impl(
            term: &crate::prelude::Terminator,
            v: &std::collections::HashMap<crate::prelude::Value, crate::prelude::XReg>,
            ctx: &mut crate::prelude::LowerCtx,
        ) -> Result<crate::prelude::InstPacket<Inst>, crate::prelude::IrError> {
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
            ) -> Result<crate::prelude::InstPacket<Inst>, crate::prelude::IrError> {
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
                let rd = results.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                    let r2 = results.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                let rs1 = args.first().copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
                let rs2 = args.get(1).copied().unwrap_or_else(|| { let _r: crate::prelude::XReg = ctx.alloc_xreg(crate::prelude::RegClass::GPR64); _r });
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
        ) -> Result<crate::prelude::InstPacket<Inst>, crate::prelude::IrError> {
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

    /// parse_when_cond 复合条件括号保护：`A && (B || C)` 必须解析为
    /// `(A) && (B || C)` 而非 `(A && B) || C`（&& 优先级高于 || 会致 guard 误真——
    /// 曾致 <8 x i32> vconst 错误匹配 rd==128 分支）。
    #[test]
    fn test_parse_when_cond_parentheses() {
        let ts = parse_when_cond("rd==128 && (elem==F32 || elem==I32)").unwrap();
        let s = ts.to_string();
        // 括号必须保留在子条件上：`&&` 的右侧整体成组（防 `(A && B) || C` 误真）。
        assert_eq!(
            s,
            "(__rdb == 128) && ((__elem == crate :: prelude :: TypeId :: F32) || (__elem == crate :: prelude :: TypeId :: I32))",
            "复合条件应生成带括号的子条件"
        );
    }

    #[test]
    fn test_parse_when_cond_simple() {
        let ts = parse_when_cond("imm0==0 && elem==F64").unwrap();
        let s = ts.to_string();
        assert!(s.contains("F64"), "elem 谓词应含 F64，实际: {s}");
    }

    fn make_minimal_model() -> IsaModel {
        let mut reg = BTreeMap::new();
        reg.insert(
            "gpr".into(),
            RegGroup {
                count: 8,
                width: 64,
                names: Some(vec!["R0".into(), "R1".into(), "R2".into(), "R3".into()]),
                prefix: None,
                base_index: None,
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
                base_index: None,
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
                gpr_bank_order: vec![],
                modrm_force_disp_base: vec![],
                epilogue_jump_opcode: None,
                default_fpr_width: None,
                enable_pattern_isel: None,
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
                gpr_bank_order: vec![],
                modrm_force_disp_base: vec![],
                epilogue_jump_opcode: None,
                default_fpr_width: None,
                enable_pattern_isel: None,
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
                opcodes: None,
                implicit: None,
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
                gpr_bank_order: vec![],
                modrm_force_disp_base: vec![],
                epilogue_jump_opcode: None,
                default_fpr_width: None,
                enable_pattern_isel: None,
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
                base_index: None,
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
                opcodes: None,
                implicit: None,
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
                gpr_bank_order: vec![],
                modrm_force_disp_base: vec![],
                epilogue_jump_opcode: None,
                default_fpr_width: None,
                enable_pattern_isel: None,
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
                opcodes: None,
                implicit: None,
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
                base_index: None,
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
                gpr_bank_order: vec![],
                modrm_force_disp_base: vec![],
                epilogue_jump_opcode: None,
                default_fpr_width: None,
                enable_pattern_isel: None,
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
            model.expand_opcodes();
            model.expand_variants();
            model.expand_templates();
            let t1 = crate::codegen::generate(&model).expect("generate 1");
            let t2 = crate::codegen::generate(&model).expect("generate 2");
            assert_eq!(
                t1.to_string(),
                t2.to_string(),
                "non-deterministic generation for {path}"
            );
        }
    }

    /// gpr8h（AH/BH/CH/DH）组内索引 → x86 物理编号 4..7（由 [reg.gpr8h].base_index=4 驱动）。
    #[test]
    fn test_gpr8h_phys_index_mapping() {
        let root = env!("CARGO_MANIFEST_DIR");
        let full = std::path::Path::new(root)
            .join("../../..")
            .join("isa/x86_v10.toml");
        let source = std::fs::read_to_string(&full).expect("read x86_v10.toml");
        let mut model = crate::parser::parse(&source).expect("parse");
        model.validate().expect("validate");
        model.expand_variants();

        // lookup_reg_in_model：gpr8h 组内索引 0..3 → 物理编号 4..7
        match crate::codegen::lookup_reg_in_model("AH", &model) {
            crate::codegen::RegLookup::Named { name, index } => {
                assert_eq!(name, "AH");
                assert_eq!(index, 4, "AH 物理编号应为 4（无 REX 高字节）");
            }
            _ => panic!("AH not found in model"),
        }
        match crate::codegen::lookup_reg_in_model("bh", &model) {
            crate::codegen::RegLookup::Named { name, index } => {
                assert_eq!(name, "BH");
                assert_eq!(index, 7, "BH 物理编号应为 7");
            }
            _ => panic!("BH not found in model"),
        }

        // resolve_reg_index：同映射
        let (is_float, idx) = crate::codegen::components::resolve_reg_index(&model, "DH");
        assert!(!is_float, "DH 应为整数类");
        assert_eq!(idx, 6, "DH 物理编号应为 6");

        // 普通寄存器不受影响（gpr64 组内索引 = 物理编号）
        let (_, i2) = crate::codegen::components::resolve_reg_index(&model, "RAX");
        assert_eq!(i2, 0);
        // 子寄存器 gpr32 视图同编号
        let (_, i3) = crate::codegen::components::resolve_reg_index(&model, "EAX");
        assert_eq!(i3, 0);
        // XMM prefix 解析
        let (is_f, i4) = crate::codegen::components::resolve_reg_index(&model, "XMM8");
        assert!(is_f);
        assert_eq!(i4, 8);
    }
}

#[cfg(test)]
mod call_skeleton_tests {
    /// [abi.call] 骨架必须由 TOML 配置驱动：用 riscv64 风格配置注入后，
    /// 生成代码必须引用配置的指令/寄存器名，且不得出现 x86 专有指令名
    /// 或寄存器名（回归守卫，防止调用 lowering 重新引入架构硬编码）。
    #[test]
    fn test_call_skeleton_is_config_driven() {
        let root = env!("CARGO_MANIFEST_DIR");
        let full = std::path::Path::new(root)
            .join("../../..")
            .join("isa/riscv64_v10.toml");
        let source = std::fs::read_to_string(&full).expect("read riscv64_v10.toml");
        let mut model = crate::parser::parse(&source).expect("parse");
        model.validate().expect("validate");
        model.expand_variants();

        // 注入 riscv64 风格 [abi.call]（X2=sp、X5=t0 为 RISC-V 惯例）。
        let abi = model.abi.as_mut().expect("riscv64 has [abi]");
        abi.call = Some(crate::model::AbiCall {
            arg_mov: Some("MV_RV".into()),
            arg_mov_f: Some("FMV_W_X".into()),
            ret_mov: Some("MV_VR".into()),
            ret_mov_f: Some("FMV_X_W".into()),
            call_inst: Some("JAL_IMM".into()),
            call_field: Some("offset".into()),
            shadow_space: 0,
            stack_slot_size: 8,
            reg_arg_limit: 8,
            stack_alloc_inst: Some("ADDI_R".into()),
            stack_free_inst: Some("ADDI_R".into()),
            stack_addr_inst: Some("LD_R".into()),
            stack_store_inst: Some("SD_R".into()),
            stack_addr_scratch: Some("X5".into()),
            sp_reg: Some("X2".into()),
            stack_opsize: 64,
            arg_opsize: 64,
            entry_fp_reg: Some("X8".into()),
            entry_load_scratch: Some("X5".into()),
            entry_stack_load_inst: Some("LD_R".into()),
            entry_sext_inst: Some("SextFake".into()),
            entry_mov_inst: Some("MOV_RM8_R64".into()),
            entry_gpr_to_fp_inst: Some("FMV_W_X".into()),
            entry_ret_addr_bytes: 8,
        });

        let ts = crate::codegen::generate(&model).expect("generate with [abi.call]");
        let out = ts.to_string();

        // 配置的指令/寄存器名必须出现（quote 的 token 流在 :: 两侧带空格）
        assert!(
            out.contains("MvRv"),
            "arg_mov 指令名 (MV_RV) 未进入生成代码"
        );
        assert!(out.contains("JalImm"), "call_inst (JAL_IMM) 未进入生成代码");
        assert!(out.contains("Reg :: X2"), "sp_reg (X2) 未进入生成代码");
        assert!(
            out.contains("Reg :: X5"),
            "stack_addr_scratch (X5) 未进入生成代码"
        );
        assert!(
            out.contains("Reg :: X8"),
            "entry_fp_reg (X8) 未进入生成代码"
        );
        assert!(
            out.contains("LdR"),
            "entry_stack_load_inst (LD_R) 未进入生成代码"
        );

        // 不得出现 x86 专有指令名/寄存器名
        assert!(!out.contains("Sub64RImm32"), "x86 栈分配指令泄漏");
        assert!(!out.contains("Add64RImm32"), "x86 栈释放指令泄漏");
        assert!(!out.contains("LeaR64Sib"), "x86 寻址指令泄漏");
        assert!(!out.contains("StoreMemR"), "x86 store 指令泄漏");
        assert!(!out.contains("MovRMem"), "x86 栈加载指令泄漏");
        assert!(!out.contains("MovqXmmFreg"), "x86 GPR→XMM 泄漏");
        assert!(!out.contains("MovsxdRGpr"), "x86 符号扩展指令泄漏");
        assert!(!out.contains("Reg :: RSP"), "x86 RSP 泄漏");
        assert!(!out.contains("Reg :: R10"), "x86 R10 泄漏");
        assert!(!out.contains("Reg :: RBP"), "x86 RBP 泄漏");
        assert!(!out.contains("Reg :: R11"), "x86 R11 泄漏");
    }
}

#[cfg(test)]
mod genericity_guard_tests {
    /// 守卫：DSL 生成器不得把 x86 专有指令名/寄存器名泄漏进任何非 x86 ISA
    /// 的生成代码。若未来重新引入架构硬编码，此测试失败。
    #[test]
    fn test_no_x86_specific_tokens_in_isa_outputs() {
        let root = env!("CARGO_MANIFEST_DIR");
        // 注意：Sub64RImm32/Add64RImm32 是 [abi.frame].alloc_inst/free_inst 的
        // 合法约定名（任何 ISA 均可定义同名指令），不在黑名单内。
        let bad_insts = [
            "LeaR64Sib", // 寻址（DSL 曾硬编码）
            "StoreMemR",
            "MovRMem",
            "MovqXmmFreg",
            "MovsxdRGpr",
        ];
        let bad_regs = ["Reg :: RSP", "Reg :: R10", "Reg :: RBP", "Reg :: R11"];
        for path in [
            "isa/aarch64_v10.toml",
            "isa/riscv64_v10.toml",
            "isa/wasm32_v10.toml",
            "isa/minimal_sd.toml",
        ] {
            let full = std::path::Path::new(root).join("../../..").join(path);
            let source = std::fs::read_to_string(&full).expect("read TOML");
            let mut model = crate::parser::parse(&source).expect("parse");
            model.validate().expect("validate");
            model.expand_opcodes();
            model.expand_variants();
            let ts = crate::codegen::generate(&model).expect("generate");
            let out = ts.to_string();
            for bad in bad_insts {
                assert!(!out.contains(bad), "{path} 生成代码泄漏 x86 指令名 {bad}");
            }
            for bad in bad_regs {
                assert!(!out.contains(bad), "{path} 生成代码泄漏 x86 寄存器 {bad}");
            }
            // 默认类元数据化：生成代码必须用 __DEFAULT_*_CLASS（模型推导），
            // 不得残留硬编码的 RegClass::GPR64/FPR64。
            assert!(
                out.contains("__DEFAULT_GPR_CLASS"),
                "{path} 生成代码缺失元数据驱动的默认 GPR 类"
            );
            assert!(
                !out.contains("RegClass :: GPR64"),
                "{path} 生成代码仍硬编码 RegClass::GPR64"
            );
        }
    }
}

#[cfg(test)]
mod memref_mapping_tests {
    use crate::model::{FieldType, InstField, Instruction};

    /// MemRef 字段类型必须映射到 forge-codegen 的 crate::prelude::MemRef
    /// （消除"生成即编译失败"的悬空引用）。
    #[test]
    fn test_memref_field_maps_to_prelude() {
        let root = env!("CARGO_MANIFEST_DIR");
        let full = std::path::Path::new(root)
            .join("../../..")
            .join("isa/minimal_sd.toml");
        let source = std::fs::read_to_string(&full).expect("read minimal_sd.toml");
        let mut model = crate::parser::parse(&source).expect("parse");
        model.validate().expect("validate");

        model.inst.insert(
            "SD_MEMREF_TEST".to_string(),
            Instruction {
                fields: vec![
                    InstField {
                        name: "dest".into(),
                        field_type: FieldType::Ireg,
                        role: None,
                    },
                    InstField {
                        name: "mem".into(),
                        field_type: FieldType::MemRef,
                        role: None,
                    },
                ],
                encoding: Some("{0x7F:[0;8]}".into()),
                asm: "memref {dest}".into(),
                effect: None,
                variants: None,
                opcodes: None,
                implicit: None,
            },
        );
        model.expand_variants();
        let ts = crate::codegen::generate(&model).expect("generate with MemRef field");
        let out = ts.to_string();
        assert!(
            out.contains("crate :: prelude :: MemRef"),
            "MemRef 字段类型未映射到 crate::prelude::MemRef"
        );

        // 默认值映射（直接验证 default_for_type 输出）
        let dft = crate::codegen::default_for_type(&FieldType::MemRef).to_string();
        assert!(
            dft.contains("MemRef :: default ()"),
            "MemRef 默认值未映射到 crate::prelude::MemRef::default(): {dft}"
        );
    }
}

#[cfg(test)]
mod clobber_tests {
    use super::*;

    fn load_x86_model() -> IsaModel {
        let root = env!("CARGO_MANIFEST_DIR");
        let full = std::path::Path::new(root)
            .join("../../..")
            .join("isa/x86_v10.toml");
        let source = std::fs::read_to_string(&full).expect("read x86_v10.toml");
        let mut model = crate::parser::parse(&source).expect("parse");
        model.validate().expect("validate");
        model.expand_opcodes();
        model.expand_variants();
        model.expand_templates();
        model
    }

    /// Udiv 的写死寄存器（RAX/RDX 显式在 insts）自动推导为物理编号 [0, 2]。
    #[test]
    fn test_div_clobbers_to_phys_indices() {
        let model = load_x86_model();
        let code = crate::codegen::generate(&model)
            .expect("generate")
            .to_string();
        assert!(
            code.contains("(0u32 , __DEFAULT_GPR_CLASS) , (2u32 , __DEFAULT_GPR_CLASS)"),
            "Udiv clobbers 未映射为物理编号 [0, 2]（RAX/RDX）"
        );
    }

    /// 自动推导：insts 里显式物理寄存器无需任何声明（无 clobbers 字段）。
    #[test]
    fn test_clobbers_auto_derived_from_insts() {
        let model = load_x86_model();
        let code = crate::codegen::generate(&model)
            .expect("generate")
            .to_string();
        assert!(
            code.contains("(0u32 , __DEFAULT_GPR_CLASS) , (2u32 , __DEFAULT_GPR_CLASS)"),
            "Udiv 的 RAX/RDX 应自动推导（无需手动声明）"
        );
        // Call 专用 arm：arg_regs + ret_regs 自动推导（GPR + XMM）
        assert!(
            code.contains("(1u32 , __DEFAULT_GPR_CLASS) , (2u32 , __DEFAULT_GPR_CLASS) , (8u32 , __DEFAULT_GPR_CLASS) , (9u32 , __DEFAULT_GPR_CLASS) , (0u32 , __DEFAULT_FPR_CLASS)"),
            "Call 的约定寄存器应自动推导（GPR 参数 + XMM 返回值）"
        );
    }

    /// shift 规则（@shift_reg 自动搬 CL）→ RCX 物理 1 被写死。
    #[test]
    fn test_shift_clobbers_rcx() {
        let model = load_x86_model();
        // RCX（物理 1）由 [inst.SHIFT_BIN].implicit 声明（@shift_reg 的 CL 隐式计数）
        let shl = &model.inst["SHL_RM_CL"];
        assert_eq!(
            shl.implicit.as_deref(),
            Some(&["RCX".to_string()][..]),
            "SHL_RM_CL 应声明 implicit RCX"
        );
        let code = crate::codegen::generate(&model)
            .expect("generate")
            .to_string();
        assert!(
            code.contains("(1u32 , __DEFAULT_GPR_CLASS)"),
            "shift 指令 implicit 未映射为 RCX(1)"
        );
    }

    /// 无写死寄存器/隐式破坏的规则不生成设置语句。
    #[test]
    fn test_no_clobber_rule_generates_nothing() {
        let model = load_x86_model();
        let code = crate::codegen::generate(&model)
            .expect("generate")
            .to_string();
        // Iadd 的 arm 内不应有 clobbers 设置（匹配 "mov rd, rs1" + "add rd, rs2" 的 arm）
        assert!(
            !code.contains("clobbers = vec ! []"),
            "空 clobbers 不应生成设置语句"
        );
    }
}

#[cfg(test)]
mod branch_gen_tests {
    use super::*;

    /// 分支元数据链路：effect Branch/Jump → is_branch/branch_targets 生成。
    #[test]
    fn test_branch_meta_generated() {
        let root = env!("CARGO_MANIFEST_DIR");
        let full = std::path::Path::new(root)
            .join("../../..")
            .join("isa/x86_v10.toml");
        let source = std::fs::read_to_string(&full).unwrap();
        let mut model = crate::parser::parse(&source).unwrap();
        model.validate().unwrap();
        model.expand_opcodes();
        model.expand_variants();
        model.expand_templates();
        let code = crate::codegen::generate(&model).unwrap().to_string();
        // JCC_REL32（effect Branch）→ is_branch arm
        assert!(
            code.contains("Inst :: JccRel32 { .. } => true"),
            "JCC_REL32 应生成 is_branch=true"
        );
        // JMP_REL32（effect Jump）→ branch_targets 从 BlockTarget 字段提取
        assert!(
            code.contains("crate :: prelude :: Block (* rel as u32)"),
            "JMP_REL32 的 branch_targets 应提取 rel 字段"
        );
        // JCC 的 rel 字段是 BlockTarget（Block(*rel as u32)）
        let jcc = &model.inst["JCC_REL32"];
        assert!(
            jcc.fields
                .iter()
                .any(|f| f.field_type == FieldType::BlockTarget),
            "JCC_REL32 应有 BlockTarget 字段"
        );
    }

    /// 直接/间接分支校验（validate 层）。
    #[test]
    fn test_branch_validate_direct_indirect() {
        let root = env!("CARGO_MANIFEST_DIR");
        for (path, name) in [
            ("isa/x86_v10.toml", "x86"),
            ("isa/riscv64_v10.toml", "riscv64"),
        ] {
            let full = std::path::Path::new(root).join("../../..").join(path);
            let source = std::fs::read_to_string(&full).unwrap();
            let model = crate::parser::parse(&source).unwrap();
            model
                .validate()
                .unwrap_or_else(|_| panic!("{name} validate"));
        }
    }
}
