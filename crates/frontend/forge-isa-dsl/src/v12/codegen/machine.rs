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
    // from_index：GPR 区 → 主 GPR 视图；FPR 区 → 浮点视图。默认值类在下面
    // 用**元数据派生**（`V12Model::main_gpr_class`/`main_fpr_class`），不在此处
    // 按"已声明组最宽"手推。
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
    // 主组 = **元数据派生**（`[meta]` 显式宽度键 > 最宽已声明组；缺 GPR 组
    // 即 Err）。历史实现取 `GPR(8).or(GPR(4)).unwrap_or(8)`——1 字节寄存器
    // ISA 两者都不存在，会拿到一个不存在的类（并让名字表全空）。
    let gpr_main = model.main_gpr_class()?;
    // 无 FPR 组（arm64/demo）= FPR(8)：`from_index` 的 FPR 视图按 FPR 区映射，
    // 不需要同名宽度组（历史 `unwrap_or(8)` 语义，保留）。
    let fpr_main = model.main_fpr_class()?.unwrap_or(RegClass::FPR(8));
    // 地址类 / 值池 / 栈槽单位 / 帧开销（全部元数据驱动，缺组即 Err）。
    let addr_class = model.addr_class()?;
    let value_gpr = model.value_gpr_class()?;
    let value_fpr = model.value_fpr_class()?.unwrap_or(RegClass::FPR(8));
    let slot_bytes = model.slot_bytes()?;
    let fp_overhead = model.fp_overhead_bytes()?;
    let vector_tiers = model.vector_tiers();
    let n_tiers = vector_tiers.len();
    // 浮点寄存器**文件**是否存在（与"值池宽度"不同）：无 FPR 组的 ISA
    // （arm64/demo/demo8）没有任何浮点/向量寄存器 —— 值池门必须据此拒绝
    // FPR/VEC 类型，否则会放行一个该 ISA 根本不存在的类。
    let has_fp_file = model.main_fpr_class()?.is_some();
    let fpr_pool_opt: Option<RegClass> = if has_fp_file { Some(value_fpr) } else { None };
    let fpr_pool_toks = match fpr_pool_opt {
        Some(c) => quote! { Some(#c) },
        None => quote! { None },
    };
    // `[types]` 显式映射（B2）：类型 → 类的 ISA 数据。生成 `__TYPE_MAP` 常量，
    // `class_for_type` 先查它（显式条目**优先于**通用值池规则），`type_map()`
    // 让 lowering 的 `reg_class_for` 也走同一份数据。
    let mut type_map_entries: Vec<TokenStream> = Vec::new();
    for (ty, target) in model.explicit_type_map()? {
        let Some(rc) = target else {
            continue; // 显式 unsupported：不进映射表（由值池门返回 None）
        };
        let ident = crate::v12::model::type_id_ident(&ty)
            .ok_or_else(|| format!("[types].{ty}: 未知类型名"))?;
        let tid = format_ident!("{ident}");
        type_map_entries.push(quote! { (forge_ir::TypeId::#tid, #rc) });
    }
    let n_type_map = type_map_entries.len();

    // 默认值类：主 GPR 组宽度 / FPR 组宽度
    let gpr_class = quote! { #gpr_main };
    let fpr_class = quote! { #fpr_main };
    let addr_toks = quote! { #addr_class };
    let value_gpr_toks = quote! { #value_gpr };
    let value_fpr_toks = quote! { #value_fpr };

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
        // 组内索引越界兜底 = 该组首个寄存器（组名由 TOML 决定，**不写死 x86
        // 的 "RAX"**；组为空由 `group_names` 在生成期报错，这里到不了）。
        let Some(fb) = fb else {
            return Err(format!(
                "[reg.{reg_class}]: 组内没有寄存器名——无法生成 from_index_grp 兜底"
            ));
        };
        grp_arms.push(quote! {
            #gname_lit => match idx { #(#idx_arms,)* _ => Reg::#fb }
        });
    }

    Ok(quote! {
        /// 默认整数值类（lowering 中 alloc_xreg 的默认目标类；元数据驱动）。
        pub(crate) const __DEFAULT_GPR_CLASS: forge_ir::RegClass = #gpr_class;
        /// 默认浮点值类（ABI/SSE 占位基准）。
        pub(crate) const __DEFAULT_FPR_CLASS: forge_ir::RegClass = #fpr_class;
        /// 地址/指针类（MemRef base/index、`lea`、sp/fp、帧地址）。
        pub(crate) const __ADDR_CLASS: forge_ir::RegClass = #addr_toks;
        /// 宿主整数值寄存器池类（值 XReg/零值/临时 vreg）。
        pub(crate) const __VALUE_GPR_CLASS: forge_ir::RegClass = #value_gpr_toks;
        /// 宿主浮点值寄存器池类（f64 值池宽；≠ `__DEFAULT_FPR_CLASS`）。
        pub(crate) const __VALUE_FPR_CLASS: forge_ir::RegClass = #value_fpr_toks;
        /// 浮点值池（**None = 本 ISA 未声明任何浮点寄存器组**）：
        /// 值池门据此拒绝 FPR/VEC 类型（详见 `class_for_type_in_pool`）。
        pub(crate) const __VALUE_FPR_POOL: Option<forge_ir::RegClass> = #fpr_pool_toks;
        /// ABI 栈槽单位（字节）。
        pub(crate) const __SLOT_BYTES: u16 = #slot_bytes;
        /// 帧指针保存槽字节数。
        pub(crate) const __FP_OVERHEAD_BYTES: u16 = #fp_overhead;
        /// 向量类字节档位（升序；`TargetRegInfo::vector_tiers`）。
        pub(crate) const __VECTOR_TIERS: [u16; #n_tiers] = [#(#vector_tiers),*];
        /// `[types]` 显式类型→类映射（B2；空 = 全部走通用值池规则）。
        pub(crate) const __TYPE_MAP: [(forge_ir::TypeId, forge_ir::RegClass); #n_type_map] =
            [#(#type_map_entries),*];

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

pub(crate) fn gen_machine_inst(
    infos: &[InstInfo],
    model: &V12Model,
) -> Result<TokenStream, String> {
    // v18 S8a：8 个"每指令一条臂"的方法 → **一行形状表** + 两个紧凑字段访问器。
    // 见 `__Shape`：`uses`/`defs`（Reg 字段序号）、`def_reuse`（def 的 ReuseInput 目标）、
    // `n_regs`（Reg 字段个数，= reg_field/settable 的域）、`classes`（每 Reg 字段的
    // 槽类索引：0 = 多类槽用运行期 class，n = `__SLOT_CLASSES[n-1]`）、
    // `effects`（effect 序列索引，0xFF = 空）。
    let mut shape_rows: Vec<TokenStream> = Vec::new();
    let mut shape_index_arms: Vec<TokenStream> = Vec::new();
    let mut reg_slot_arms: Vec<TokenStream> = Vec::new();
    let mut set_reg_slot_arms: Vec<TokenStream> = Vec::new();
    // 去重的槽类表（`__SLOT_CLASSES`）与 effect 序列表（`__EFFECT_SETS`）。
    let mut slot_classes: Vec<TokenStream> = Vec::new();
    let mut slot_class_texts: Vec<String> = Vec::new();
    let mut effect_sets: Vec<TokenStream> = Vec::new();
    let mut effect_set_texts: Vec<String> = Vec::new();
    let mut branch_arms = Vec::new();
    let mut call_arms = Vec::new();
    let mut ret_arms = Vec::new();
    let mut move_arms = Vec::new();
    let mut branch_targets_arms = Vec::new();
    let mut implicit_arms: Vec<TokenStream> = Vec::new();
    // 物理寄存器名 → 索引（implicit_regs 解析用）。锚点 = **元数据派生**的
    // 主 GPR 类（`[meta].default_gpr_width` > 最宽已声明组），缺组即 Err——
    // 历史实现锚定 `GPR(8).or(GPR(4))`，1 字节寄存器 ISA 会得到空表并静默
    // 丢弃 implicit_regs（clobber 集缺失 → regalloc 分配被隐式破坏的寄存器）。
    let name_to_idx = model.main_gpr_name_to_idx()?;
    let gpr_clobber_class = model.main_gpr_class()?;
    let gpr_clobber_toks = quote! { #gpr_clobber_class };

    for info in infos {
        let vn = &info.vn;
        // effect 标签（本循环多处用：is_branch/is_call/is_ret/effects/is_move）
        let eff = &info.inst.effect;
        // implicit_regs：指令隐式破坏的物理寄存器（cqo 的 RDX、idiv 的
        // RAX/RDX）→ MachineInst::clobbers（regalloc 在本指令点避开）。
        // 名字解析 **fail-closed**：解析不到即生成期报错（历史实现 `filter_map`
        // 静默丢弃 ⇒ clobber 集缺失 ⇒ regalloc 会分配被隐式破坏的寄存器）。
        if let Some(implicit) = &info.inst.implicit_regs {
            let mut entries: Vec<TokenStream> = Vec::with_capacity(implicit.len());
            for r in implicit {
                let idx = name_to_idx.get(r.as_str()).copied().ok_or_else(|| {
                    format!(
                        "[[instructions.{}]].implicit_regs: 物理寄存器名 \"{r}\" 不在主 GPR 组 \
                         [reg.{gpr_clobber_class}] 内（生成期 fail-closed：静默丢弃会让 regalloc \
                         分配被隐式破坏的寄存器）",
                        info.inst.name
                    )
                })?;
                entries.push(quote! { (#idx, #gpr_clobber_toks) });
            }
            if !entries.is_empty() {
                implicit_arms.push(quote! { Inst::#vn { .. } => &[#(#entries),*] });
            }
        }

        // Reg 操作数按操作数序收集角色（操作数级 role 优先，缺省 In）。
        // `reg_field` 序号 = 该变体 Reg 操作数的位置序（lowering 同序 map_reg_field），
        // 因此 uses/defs 存的是**序号**（不是字段名），访问器再按序号取字段值。
        let mut use_idxs: Vec<u8> = Vec::new();
        let mut def_idxs: Vec<u8> = Vec::new();
        let mut reg_fields: Vec<(usize, &syn::Ident, &OperandSlot)> = Vec::new();
        for (_, fid, slot, role) in info.operands.iter() {
            if slot.kind != OperandKind::Reg {
                continue;
            }
            let idx = reg_fields.len();
            match role {
                OperandRole::In => use_idxs.push(idx as u8),
                OperandRole::Out => def_idxs.push(idx as u8),
                OperandRole::InOut => {
                    use_idxs.push(idx as u8);
                    def_idxs.push(idx as u8);
                }
            }
            reg_fields.push((idx, fid, slot));
        }
        let n_regs = reg_fields.len();
        let fids: Vec<&syn::Ident> = reg_fields.iter().map(|(_, f, _)| *f).collect();
        // def 的 ReuseInput：InOut 字段复用它在 use 列表里的位置（两地址指令）。
        let def_reuse: Vec<u8> = def_idxs
            .iter()
            .map(|d| {
                use_idxs
                    .iter()
                    .position(|u| u == d)
                    .map(|p| p as u8)
                    .unwrap_or(0xFF)
            })
            .collect();
        // 每 Reg 字段的槽类编码：单类槽 → 索引+1；多类槽（class=None）→ 0。
        let mut class_codes: Vec<u8> = Vec::with_capacity(n_regs);
        for (_, _, slot) in &reg_fields {
            match &slot.class {
                Some(_) => {
                    let expr = super::reg_class_expr(slot);
                    let text = expr.to_string();
                    let idx = match slot_class_texts.iter().position(|x| *x == text) {
                        Some(i) => i,
                        None => {
                            slot_classes.push(super::reg_class_expr(slot));
                            slot_class_texts.push(text);
                            slot_classes.len() - 1
                        }
                    };
                    class_codes.push(idx as u8 + 1);
                }
                None => class_codes.push(0),
            }
        }

        // effects：按 TOML 声明序取 effect 序列（0xFF = 空），同序去重。
        let eff_kinds: Vec<TokenStream> = eff
            .iter()
            .map(|e| match e {
                Effect::Pure | Effect::Move => quote! { crate::prelude::EffectKind::Pure },
                Effect::Read => quote! { crate::prelude::EffectKind::Read },
                Effect::Write => quote! { crate::prelude::EffectKind::Write },
                Effect::Branch => quote! { crate::prelude::EffectKind::Branch },
                Effect::Jump => quote! { crate::prelude::EffectKind::Jump },
                Effect::Call => quote! { crate::prelude::EffectKind::Call },
                Effect::Ret => quote! { crate::prelude::EffectKind::Ret },
                Effect::Trap => quote! { crate::prelude::EffectKind::Trap },
            })
            .collect();
        let eff_idx: u8 = if eff_kinds.is_empty() {
            0xFF
        } else {
            let text = quote! { #(#eff_kinds),* }.to_string();
            match effect_set_texts.iter().position(|x| *x == text) {
                Some(i) => i as u8,
                None => {
                    effect_sets.push(quote! { &[#(#eff_kinds),*] });
                    effect_set_texts.push(text);
                    (effect_sets.len() - 1) as u8
                }
            }
        };

        // 形状行 + 两个字段访问器（`[f1, f2][i]`：Reg 字段序 = reg_field 序号）。
        let uses_lit = use_idxs
            .iter()
            .map(|i| proc_macro2::Literal::u8_unsuffixed(*i));
        let defs_lit = def_idxs
            .iter()
            .map(|i| proc_macro2::Literal::u8_unsuffixed(*i));
        let reuse_lit = def_reuse
            .iter()
            .map(|i| proc_macro2::Literal::u8_unsuffixed(*i));
        let cls_lit = class_codes
            .iter()
            .map(|i| proc_macro2::Literal::u8_unsuffixed(*i));
        let n_regs_lit = proc_macro2::Literal::u8_unsuffixed(n_regs as u8);
        let eff_lit = proc_macro2::Literal::u8_unsuffixed(eff_idx);
        shape_rows.push(quote! {
            __Shape {
                uses: &[#(#uses_lit),*],
                defs: &[#(#defs_lit),*],
                def_reuse: &[#(#reuse_lit),*],
                n_regs: #n_regs_lit,
                classes: &[#(#cls_lit),*],
                effects: #eff_lit,
            }
        });
        let row_idx = shape_rows.len() - 1;
        shape_index_arms.push(quote! { Inst::#vn { .. } => #row_idx });
        if reg_fields.is_empty() {
            reg_slot_arms.push(quote! { Inst::#vn { .. } => None });
            set_reg_slot_arms.push(quote! { Inst::#vn { .. } => false });
        } else {
            // 注意两层解引用：`&self` 上的匹配让 `fid` 绑定成 `&Reg`，
            // 于是 `[fid, …]` 是 `[&Reg; n]`，`.get(i)` 给 `Option<&&Reg>`，
            // 两次 `.copied()` 才回到 `Option<Reg>`。
            reg_slot_arms.push(
                quote! { Inst::#vn { #(#fids),*, .. } => [#(#fids),*].get(i).copied().copied() },
            );
            // 可写侧**不能**返回 `Option<&mut Reg>`：`[fid, …]` 是临时数组，
            // 从它里面取出的 `&mut Reg` 生命周期被绑定到该临时值（E0515）。
            // 改成"就地写入 + 报告是否命中"，写动作发生在数组存活期内。
            set_reg_slot_arms.push(
                quote! { Inst::#vn { #(#fids),*, .. } => match [#(#fids),*].get_mut(i) {
                    Some(r) => { **r = reg; true }
                    None => false,
                } },
            );
        }

        // effect → is_branch/is_call/is_ret/effects
        let is_jumpy = |e: &&Effect| matches!(e, Effect::Branch | Effect::Jump);
        if eff.iter().any(|e| is_jumpy(&e)) {
            branch_arms.push(quote! { Inst::#vn { .. } => true });
        }
        if eff.contains(&Effect::Call) {
            call_arms.push(quote! { Inst::#vn { .. } => true });
        }
        if eff.contains(&Effect::Ret) {
            ret_arms.push(quote! { Inst::#vn { .. } => true });
        }
        // branch_targets：effect Branch/Jump 且有 Label 槽 → 提取为 Block
        if eff.iter().any(|e| is_jumpy(&e))
            && let Some(fid) = info
                .operands
                .iter()
                .find(|(_, _, s, _)| s.kind == OperandKind::Label)
                .map(|(_, fid, _, _)| fid)
        {
            branch_targets_arms.push(quote! {
                Inst::#vn { #fid, .. } => smallvec::smallvec![crate::prelude::Block::new(*#fid as u32)]
            });
        }
        // is_move：effect 含 "Move"（纯寄存器移动，TOML 显式声明）且
        // 1 def + 1 use。**删除指令名前缀启发式**（MOV_/MOVR）——语义由
        // effect 标签表达，与指令名解耦（LLVM TableGen flags 同思路）。
        let is_move_decl = eff.contains(&Effect::Move);
        if is_move_decl && def_idxs.len() == 1 && use_idxs.len() == 1 {
            let d = fids[usize::from(def_idxs[0])];
            let u = fids[usize::from(use_idxs[0])];
            if d != u {
                move_arms.push(
                    quote! { Inst::#vn { #d, #u, .. } => Some((#d.to_index(), #u.to_index())) },
                );
            }
        }
    }

    // `Inst::Raw` 用一行空形状（表尾）。
    shape_rows.push(quote! {
        __Shape {
            uses: &[], defs: &[], def_reuse: &[], n_regs: 0, classes: &[], effects: 0xFF,
        }
    });
    let raw_row = shape_rows.len() - 1;
    shape_index_arms.push(quote! { Inst::Raw(_) => #raw_row });
    reg_slot_arms.push(quote! { Inst::Raw(_) => None });
    set_reg_slot_arms.push(quote! { Inst::Raw(_) => false });

    Ok(quote! {
        /// 每条指令的**形状**（v18 S8a）：uses/defs/reg_field/约束/effect 的单一事实源。
        ///
        /// 此前 8 个方法各自生成一套"每指令一条臂"的 match（x86 合计 ~185 KB）；
        /// 现在只有这一张表 + 两个字段访问器，方法是通用循环。
        #[derive(Clone, Copy)]
        pub(crate) struct __Shape {
            /// Reg 字段序号（use 角色），按字段序。
            pub uses: &'static [u8],
            /// Reg 字段序号（def 角色），按字段序。
            pub defs: &'static [u8],
            /// 与 `defs` 等长：0xFF = `OperandConstraint::Any`，否则 = 复用的 use 位置。
            pub def_reuse: &'static [u8],
            /// Reg 字段个数（= `reg_field`/`is_reg_field_settable` 的域）。
            /// 可改写性只由这里决定：非 Reg 字段（立即数/内存/标签）不可改写，
            /// regalloc 对落在不可改写字段上的 spilled def 是 fail-closed 报错（WA-40）。
            pub n_regs: u8,
            /// 与 Reg 字段序等长：0 = 多类槽（用运行期 class），n = `__SLOT_CLASSES[n-1]`。
            pub classes: &'static [u8],
            /// `__EFFECT_SETS` 下标（0xFF = 无 effect）。
            pub effects: u8,
        }
        static __SHAPES: &[__Shape] = &[ #(#shape_rows),* ];
        static __SLOT_CLASSES: &[forge_ir::RegClass] = &[ #(#slot_classes),* ];
        static __EFFECT_SETS: &[&[crate::prelude::EffectKind]] = &[ #(#effect_sets),* ];
        impl Inst {
            #[inline]
            fn __shape(&self) -> &'static __Shape {
                &__SHAPES[match self { #(#shape_index_arms,)* }]
            }
            /// 第 i 个 **Reg 字段**的值（越界 = None）。i 是 `reg_field` 序号。
            #[inline]
            fn __reg_slot(&self, i: usize) -> Option<Reg> {
                match self { #(#reg_slot_arms,)* }
            }
            /// 把第 i 个 Reg 字段写成 `reg`（越界 = 不写，返回 false）。
            /// i 是 `reg_field` 序号。
            #[inline]
            fn __set_reg_slot(&mut self, i: usize, reg: Reg) -> bool {
                match self { #(#set_reg_slot_arms,)* }
            }
        }
        impl crate::prelude::MachineInst for Inst {
            fn uses(&self) -> smallvec::SmallVec<[u32; 4]> {
                let mut out = smallvec::SmallVec::new();
                for &f in self.__shape().uses {
                    if let Some(r) = self.__reg_slot(usize::from(f)) {
                        out.push(r.to_index());
                    }
                }
                out
            }
            fn defs(&self) -> smallvec::SmallVec<[u32; 2]> {
                let mut out = smallvec::SmallVec::new();
                for &f in self.__shape().defs {
                    if let Some(r) = self.__reg_slot(usize::from(f)) {
                        out.push(r.to_index());
                    }
                }
                out
            }
            fn use_constraints(&self) -> smallvec::SmallVec<[crate::machine::inst::OperandConstraint; 4]> {
                let n = usize::from(self.__shape().uses.len() as u8);
                let mut out = smallvec::SmallVec::new();
                for _ in 0..n {
                    out.push(crate::machine::inst::OperandConstraint::Any);
                }
                out
            }
            fn def_constraints(&self) -> smallvec::SmallVec<[crate::machine::inst::OperandConstraint; 2]> {
                let mut out = smallvec::SmallVec::new();
                for &r in self.__shape().def_reuse {
                    out.push(if r == 0xFF {
                        crate::machine::inst::OperandConstraint::Any
                    } else {
                        crate::machine::inst::OperandConstraint::ReuseInput(usize::from(r))
                    });
                }
                out
            }
            fn effects(&self) -> smallvec::SmallVec<[crate::prelude::EffectKind; 2]> {
                let mut out = smallvec::SmallVec::new();
                if let Some(set) = __EFFECT_SETS.get(usize::from(self.__shape().effects)) {
                    out.extend_from_slice(set);
                }
                out
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
                self.__reg_slot(i).map_or(0, |r| r.to_index())
            }
            fn set_reg_field(&mut self, i: usize, preg: u32, class: forge_ir::RegClass) {
                let sh = self.__shape();
                if i >= usize::from(sh.n_regs) {
                    return;
                }
                // 单类槽：宽度由指令声明（槽 class）强制；多类槽：用运行期 class
                // （regalloc 携带 IR 值宽度），非整数类回退主 GPR 类（元数据派生）。
                let code = sh.classes.get(i).copied().unwrap_or(0);
                let cls = if code == 0 {
                    if class.is_int() {
                        class
                    } else {
                        __DEFAULT_GPR_CLASS
                    }
                } else {
                    __SLOT_CLASSES[usize::from(code) - 1]
                };
                self.__set_reg_slot(i, <Reg as forge_ir::PhysReg>::from_index(preg, cls));
            }
            fn is_reg_field_settable(&self, i: usize) -> bool {
                i < usize::from(self.__shape().n_regs)
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
    // fixed 与 mixed 的 label 槽都散布在位段中（fixup 从指令起始算），
    // 只有 prefix_scan 的 label 是尾部连续 imm。
    let fixed = !model.is_prefix_scan();
    // 定宽/混合字长 ISA 的 fixup 宽度 = **该指令**的字长（v18 S4：fixed 全 ISA 相同、
    // mixed 逐指令）；变长 ISA 固定 4。位段重排（riscv JAL/B 型、arm64 imm26…）由
    // 该 ISA 的 `RelocPatcher` 做。label 与 global 两类 fixup 共用这个宽度。
    let fixed_bytes_of = |inst: &Instruction| -> Result<u32, String> {
        if fixed {
            model.inst_width_bytes(inst)
        } else {
            Ok(4)
        }
    };
    // 含 Label 槽的指令：encode 后对 fixup 位置 use_label_at（块号 → 实际偏移）。
    let mut label_arms: Vec<TokenStream> = Vec::new();
    for info in infos {
        let vn = &info.vn;
        // fixup 宽度 = 本指令字长（fixed/mixed；变长 ISA 恒 4）
        let fixed_bytes_lit = proc_macro2::Literal::u32_unsuffixed(fixed_bytes_of(&info.inst)?);
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
                // ISA 重编码位段；Relative(字长,0) = 相对指令地址本身）。
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
                                crate::RelocKind::Relative(#fixed_bytes_lit, 0),
                                &format!("@{}", -rel - 1),
                                0,
                            );
                        } else {
                            sink.use_label_at(__base, crate::emit::LabelRef::from_id(rel as u32), crate::RelocKind::Relative(#fixed_bytes_lit, 0));
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
                            sink.use_label_at(fixup, crate::emit::LabelRef::from_id(rel as u32), crate::RelocKind::REL4);
                        }
                        Ok(())
                    }
                });
            }
        }
    }
    let label_arms = label_arms;
    // GlobalAddr 专用指令（TOML 声明 `reloc = "<名>"`，v18 S3d 起查 `[[reloc]]` 表）：
    // imm 槽 < 0 时编码 GlobalId（-(id+1)）→ 重定位 "G{id}"（JIT/QEMU 打包按全局
    // 变量注册）；>= 0 时是普通立即数。语义来自表项：
    // - `absolute`（x86 MOVABS_GLOBAL，槽 imm64）：fixup = 指令末尾该槽的字节区间
    //   （槽宽即补丁宽度），reloc = Absolute(槽字节数)；
    // - `pc_relative`（riscv AUIPC_GLOBAL/ADDI_GLOBAL）：fixup = 指令起始，
    //   补丁宽度 = 指令字长（`[encoding].bits`），
    //   reloc = Relative(字长, 0)；patcher 按 opcode 0x17/0x13 分写 hi20/lo12 位段。
    let mut global_arms: Vec<TokenStream> = Vec::new();
    for info in infos {
        let Some(reloc_name) = info.inst.reloc.as_deref() else {
            continue;
        };
        let def = model
            .reloc
            .iter()
            .find(|r| r.name == reloc_name)
            .ok_or_else(|| {
                format!(
                    "[[instructions.{}]]: reloc '{reloc_name}' 未在 [[reloc]] 声明",
                    info.inst.name
                )
            })?;
        let vn = &info.vn;
        // 绑定的槽 → Inst 字段名 + 槽字节数（absolute 的补丁宽度）
        let slot_fid = info
            .operands
            .iter()
            .find(|(_, _, s, _)| s.name == def.slot)
            .map(|(_, fid, _, _)| fid.clone())
            .ok_or_else(|| {
                format!(
                    "[[instructions.{}]]: reloc '{reloc_name}' 绑定的槽 '{}' 不在该指令的操作数里",
                    info.inst.name, def.slot
                )
            })?;
        let slot_bits = model
            .operand_slots
            .iter()
            .find(|s| s.name == def.slot)
            .and_then(|s| s.width)
            .unwrap_or(64);
        let slot_bytes_lit = proc_macro2::Literal::u32_unsuffixed(slot_bits.div_ceil(8));
        let reloc_bytes_lit = proc_macro2::Literal::u32_unsuffixed(fixed_bytes_of(&info.inst)?);
        let addend = def.addend.unwrap_or(0);
        let body = match def.semantics {
            RelocSemantics::Absolute => quote! {
                let bytes = encode(inst).map_err(|e| crate::EncodeError::Other(e))?;
                let imm = match inst {
                    Inst::#vn { #slot_fid, .. } => *#slot_fid,
                    _ => unreachable!(),
                };
                let fixup = sink.offset() + bytes.len() - #slot_bytes_lit as usize;
                sink.put_bytes(&bytes);
                if imm < 0 {
                    sink.add_reloc(
                        fixup,
                        crate::RelocKind::Absolute(#slot_bytes_lit),
                        &format!("G{}", -imm - 1),
                        #addend,
                    );
                }
                Ok(())
            },
            RelocSemantics::PcRelative => quote! {
                let bytes = encode(inst).map_err(|e| crate::EncodeError::Other(e))?;
                let imm = match inst {
                    Inst::#vn { #slot_fid, .. } => *#slot_fid,
                    _ => unreachable!(),
                };
                let __base = sink.offset();
                sink.put_bytes(&bytes);
                if imm < 0 {
                    sink.add_reloc(
                        __base,
                        crate::RelocKind::Relative(#reloc_bytes_lit, 0),
                        &format!("G{}", -imm - 1),
                        #addend,
                    );
                }
                Ok(())
            },
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

/// `[[pseudo]]` 的展开助手（v18 S3e）。
///
/// 生成两个函数：
///
/// - `__pseudo_expand(line, depth, out)`：行首词命中某个伪指令名 ⇒ 按 `params` 位置
///   切分实参、逐行把 `{参数}` 替换成实参文本 push 进 `out`，返回 `true`；不是伪指令
///   返回 `false`。展开出的行**递归**再判一次（emit 行可以是别的伪指令），
///   深度上限 16（自引用即报错，不会栈溢出）。
/// - `__split_args(s)`：**顶层**逗号切分（`()`/`[]` 内的逗号不算——`li x1, (a + b)`
///   与 `lw x1, [x2, #4]` 都按 2 个参数切）。
///
/// 没有 `[[pseudo]]` 的 ISA 返回空 token（生成的代码与引入本能力之前逐字相同）。
pub(crate) fn gen_pseudo_helpers(model: &V12Model) -> TokenStream {
    if model.pseudo.is_empty() {
        return quote! {};
    }
    let mut arms: Vec<TokenStream> = Vec::new();
    for p in &model.pseudo {
        let name_lit = syn::LitStr::new(&p.name, proc_macro2::Span::call_site());
        let params_lit = syn::LitStr::new(&p.params.join(", "), proc_macro2::Span::call_site());
        let n = p.params.len();
        // 实参绑定：__a0..__a{n-1}
        let binds: Vec<TokenStream> = (0..n)
            .map(|i| {
                let id = format_ident!("__a{i}");
                quote! { let #id = args[#i].trim(); }
            })
            .collect();
        // 每行 emit：String::from(模板) + 逐个参数 replace
        let emits: Vec<TokenStream> = p
            .emit
            .iter()
            .map(|line| {
                let lit = syn::LitStr::new(line.trim(), proc_macro2::Span::call_site());
                let reps: Vec<TokenStream> = p
                    .params
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let ph =
                            syn::LitStr::new(&format!("{{{a}}}"), proc_macro2::Span::call_site());
                        let id = format_ident!("__a{i}");
                        quote! { __l = __l.replace(#ph, #id); }
                    })
                    .collect();
                quote! {
                    {
                        let mut __l = String::from(#lit);
                        #(#reps)*
                        out.push(__l);
                    }
                }
            })
            .collect();
        arms.push(quote! {
            if head == #name_lit {
                let args = __split_args(rest);
                if args.len() != #n {
                    return Err(format!(
                        "伪指令 '{}' 需要 {} 个参数（{}），实际 {}",
                        #name_lit, #n, #params_lit, args.len()
                    ));
                }
                #(#binds)*
                #(#emits)*
                return Ok(true);
            }
        });
    }
    quote! {
        /// 顶层逗号切分（尊重 `()`/`[]`：`li x1, (a + b)`、`lw x1, [x2, #4]` 都算 2 段）。
        fn __split_args(s: &str) -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            let mut depth = 0i32;
            let mut cur = String::new();
            for ch in s.chars() {
                match ch {
                    '(' | '[' => { depth += 1; cur.push(ch); }
                    ')' | ']' => { depth -= 1; cur.push(ch); }
                    ',' if depth == 0 => { out.push(cur.trim().to_string()); cur.clear(); }
                    _ => cur.push(ch),
                }
            }
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
            }
            out
        }

        /// 伪指令展开：`Ok(true)` = 该行是伪指令（展开结果已 push 进 `out`）。
        ///
        /// 递归展开（emit 行可以是别的伪指令），`depth` 上限 16 防自引用成环。
        fn __pseudo_expand(line: &str, depth: usize, out: &mut Vec<String>)
            -> Result<bool, String>
        {
            if depth > 16 {
                return Err("伪指令展开超过 16 层（emit 里嵌套了自身?）".into());
            }
            let (head, rest) = match line.trim().split_once(char::is_whitespace) {
                Some((h, r)) => (h, r.trim()),
                None => (line.trim(), ""),
            };
            #(#arms)*
            Ok(false)
        }
    }
}

pub(crate) fn gen_assembler(model: &V12Model) -> TokenStream {
    let comment = model.meta.comment_char.chars().next().unwrap_or('#');
    let label_suf = model.meta.label_suffix.clone();
    let label_suf_lit = syn::LitStr::new(&label_suf, proc_macro2::Span::call_site());
    let label_suf_len = label_suf.len();
    let dir_pre = model.meta.directive_prefix.clone();
    let dir_pre_lit = syn::LitStr::new(&dir_pre, proc_macro2::Span::call_site());
    let align_pad = model.emit.as_ref().and_then(|e| e.align_pad).unwrap_or(0);
    // `[[pseudo]]` 展开（v18 S3e）：按名生成匹配臂 + 逐行文本替换。没有伪指令的 ISA
    // 不生成任何东西（生成的代码逐字不变）。
    let pseudo_helpers = gen_pseudo_helpers(model);
    // 调用点也只在有伪指令时生成（否则会引用不存在的助手函数）。
    let pseudo_call: TokenStream = if model.pseudo.is_empty() {
        quote! {}
    } else {
        quote! {
            {
                let mut __plines: Vec<String> = Vec::new();
                if __pseudo_expand(rest, 0, &mut __plines)
                    .map_err(|e| line_err(AsmError::Other(e)))?
                {
                    for __pl in &__plines {
                        let (inst, syms) =
                            __assemble(__pl).map_err(|e| line_err(AsmError::Other(e)))?;
                        __offset += encode(&inst)
                            .map_err(|e| line_err(AsmError::Other(e)))?
                            .len() as u64;
                        for (oi, s) in syms {
                            pending.push((insts.len(), oi, s, __line_no + 1));
                        }
                        insts.push(inst);
                    }
                    continue;
                }
            }
        }
    };
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
                    // 伪指令展开（v18 S3e）：行首词命中 `[[pseudo]]` 名 → 按 params
                    // 位置切分实参、代入 emit 模板，展开出的每一行再走同一套展开
                    //（emit 行可以是别的伪指令；深度上限 16 防环）。
                    #pseudo_call
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

        #pseudo_helpers

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
