//! v12 codegen — 迭代 2：定宽（32 位）ISA 自包含模块生成。
//!
//! 生成模块（仅依赖 std，不耦合 forge-codegen 内部）：
//!
//! - `Inst` 枚举：变体按 v11 同款 pascal 命名；操作数按绑定位域名命名
//!   （reg → u32 物理索引，imm/label → i64）。
//!
//! - `encode(&Inst) -> Result<[u8; 4], String>`：opcode、`fields` 固定值与
//!   操作数经位段放置（散布 pieces：`((value >> shift) & mask) << offset`）。
//!
//! - `decode(&[u8]) -> Option<Inst>`：常量 guard（opcode/fields 值 +
//!   覆盖补集零位）+ 字段提取（散布反向 OR）；声明序首匹配（与 v11 一致）；
//!   有符号槽按槽宽度符号扩展（v11 不扩展，v12 规范修正）。
//!
//! - `disassemble(&Inst) -> String` / `assemble(&str) -> Result<Inst, String>`：
//!   asm 模板驱动（`{0}`/`{1}` 占位符，mnemonic 与模板分离），支持 simple、
//!   memory（`{I}({J})`）与 memory0（`({J})`）三种操作数形状。
//!
//! 迭代 2 范围：`meta.default_inst_width == 32` 的定宽 ISA（riscv64 试点）；
//! 变长 form（modrm/vex 语义键）与 64/16 位定宽在迭代 3+ 支持。

use super::model::*;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

pub(crate) mod asm;
/// TargetFrameLowering/TargetABI/emit（integration.rs 拆分）。
pub(crate) mod frame;
pub(crate) mod integration;
/// TargetLowering 生成（integration.rs 拆分；依赖 integration 的工具函数）。
pub(crate) mod lowering;
/// Reg 枚举 / MachineInst / Encoder / Decoder / Disasm / Assembler
/// （integration.rs 拆分）。
pub(crate) mod machine;
/// 变长（VEX/EVEX/前缀扫描）encode/decode——x86 专用机制，独立文件组织。
pub(crate) mod vlen;

// ─────────────────────────────── 共享工具 ───────────────────────────────

/// 字符串 → PascalCase 标识符文本（v11 `codegen` 模块的同名工具，随语法层
/// 删除迁移至 v12 生成器；`_`/`-`/空格 为分隔符，数字开头补 "Inst" 前缀）。
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

/// 指令名 → Rust 标识符（`Inst` 枚举变体名）。
pub(crate) fn pascal_ident(s: &str) -> proc_macro2::Ident {
    format_ident!("{}", pascal(s))
}

// ─────────────────────────── 类型化字段辅助 ───────────────────────────

/// Inst 字段的 Rust 类型：Reg 槽 → `Reg` 枚举（类型安全，非裸 u32）；
/// cond → u8；mem → MemRef；imm/label → i64。
fn field_ty(slot: &OperandSlot) -> TokenStream {
    match slot.kind {
        OperandKind::Reg => quote! { Reg },
        OperandKind::Cond => quote! { u8 },
        OperandKind::Mem => quote! { MemRef },
        _ => quote! { i64 },
    }
}

/// Reg 槽的 RegClass 表达式（`Reg::from_index` 消歧 GPR/FPR 用；
/// 槽位 class 为浮点组 → FPR64，否则 GPR64）。
fn reg_class_expr(slot: &OperandSlot) -> TokenStream {
    match slot.class.as_ref() {
        Some(c) => match c {
            RegClass::GPR(w) => quote! { forge_ir::RegClass::GPR(#w) },
            RegClass::FPR(w) => quote! { forge_ir::RegClass::FPR(#w) },
            RegClass::VEC(w) => quote! { forge_ir::RegClass::VEC(#w) },
            RegClass::KReg(w) => quote! { forge_ir::RegClass::KReg(#w) },
        },
        _ => quote! { forge_ir::RegClass::GPR(8) },
    }
}

/// 字段构造表达式：Reg 槽 → `Reg::from_index(v, class)`（原始索引 →
/// 类型化枚举，主视图）；其余 → 原值。
/// `pub(crate)`：变长模块（vlen.rs）复用。
pub(crate) fn field_ctor_expr(slot: &OperandSlot, v: TokenStream) -> TokenStream {
    if slot.kind == OperandKind::Reg {
        let cls = reg_class_expr(slot);
        quote! { Reg::from_index(#v, #cls) }
    } else {
        v
    }
}

/// 字段构造表达式（decode 宽度视图）：Reg 槽按固定宽度组/多态选择视图。
/// `view` = 槽 class 组的固定宽度（Some）；None = 多态（按 __opsize 选
/// gpr16/32/64——assemble 与 encode 已按实际寄存器宽度还原，decode 反向）。
/// `pub(crate)`：变长模块（vlen.rs）复用。
pub(crate) fn field_ctor_expr_view(
    slot: &OperandSlot,
    v: TokenStream,
    view: Option<u16>,
) -> TokenStream {
    if slot.kind != OperandKind::Reg {
        return v;
    }
    match view {
        Some(_) => {
            // 固定宽度组：from_index_grp(v, 组名)
            let class = slot.class.as_ref().unwrap_or(&RegClass::GPR(8));
            quote! { <Reg as TryFrom<RegRef>>::try_from(RegRef::new(#class,#v)).unwrap() }
        }
        None => {
            // 多态 GPR：按 decode 扫描的前缀宽度选择视图
            quote! {
                <Reg as TryFrom<RegRef>>::try_from(RegRef::new(RegClass::GPR(__opsize),#v)).unwrap()
            }
        }
    }
}

/// 变长 ISA 的语义化操作数名（替代位置名 op{i}，恢复 v11 可读性）：
/// Reg out→dest、Reg in→src/src2/src3、cond→cond、
/// mem→mem、imm→imm、label→target；重名时追加序号。
fn semantic_operand_name(op: &OperandUse, slot: &OperandSlot, _i: usize) -> String {
    match slot.kind {
        OperandKind::Reg => match op.role {
            Some(OperandRole::Out) | Some(OperandRole::InOut) => "dest",
            _ => "src",
        },
        OperandKind::Cond => "cond",
        OperandKind::Mem => "mem",
        OperandKind::Imm => "imm",
        OperandKind::Label => "target",
    }
    .to_string()
}

// ─────────────────────────────── 入口 ───────────────────────────────

pub fn generate(model: &V12Model) -> Result<TokenStream, String> {
    let variable = model.meta.variable_length;
    if !variable && model.meta.default_inst_width != Some(32) {
        return Err(format!(
            "v12 codegen (iteration 2/3) supports default_inst_width = 32 or variable_length, got {:?}",
            model.meta.default_inst_width
        ));
    }
    let infos = collect_inst_infos(model)?;
    let reg_tables = gen_reg_tables(model)?;
    let mem_support = gen_mem_support(&infos)?;
    let inst_enum = gen_inst_enum(&infos);
    let (encode_fn, decode_fn) = if variable {
        (
            vlen::gen_vlen_encode(&infos, model)?,
            vlen::gen_vlen_decode(&infos, model)?,
        )
    } else {
        (gen_encode(&infos, model)?, gen_decode(&infos, model)?)
    };
    let disasm_fn = asm::gen_disassemble(&infos)?;
    let asm_fn = asm::gen_assemble(&infos, model)?;
    // 迭代 5：TargetMachine 集成层（MachineInst/Encoder/Decoder/ABI/
    // FrameLowering/Lowering/TargetMachine 组装）。
    let integration = integration::gen_integration(&infos, model)?;
    Ok(quote! {
        // ── v12 生成模块（迭代 2/3/3b：自包含 encode/decode/asm）──
        #reg_tables
        #mem_support
        #inst_enum
        #encode_fn
        #decode_fn
        #disasm_fn
        #asm_fn
        // ── v12 TargetMachine 集成层（迭代 5）──
        #integration
    })
}

/// 有 mem 操作数时生成 `MemRef` 结构 + 渲染/解析 helpers（自包含）。
fn gen_mem_support(_infos: &[InstInfo]) -> Result<TokenStream, String> {
    // 找 mem 槽的 base 寄存器组（class）
    Ok(quote! {
        /// 内存操作数（自包含；base 为物理寄存器索引，disp 为字节位移；
        /// index 为可选索引寄存器（x86 SIB index），scale 为缩放 1/2/4/8）。
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub struct MemRef {
            pub base: Reg,
            pub disp: i64,
            /// 索引寄存器（None = 无索引）。
            pub index: Option<Reg>,
            /// 索引缩放（1/2/4/8；缺省 1）。
            pub scale: u8,
        }
        fn __render_mem(m: &MemRef) -> String {
            let base = <&'static str as From<Reg>>::from(m.base);
            let mut s = format!("[{base}");
            if let Some(idx) = &m.index {
                let iname = <&'static str as From<Reg>>::from(*idx);
                if m.scale == 1 {
                    s.push_str(&format!("+{iname}"));
                } else {
                    s.push_str(&format!("+{iname}*{}", m.scale));
                }
            }
            if m.disp != 0 {
                if m.disp > 0 {
                    s.push_str(&format!("+{}", m.disp));
                } else {
                    s.push_str(&format!("{}", m.disp));
                }
            }
            s.push(']');
            s
        }
    })
}

/// 单条指令的生成信息：form 解析 + 操作数→位域绑定 + mnemonic/asm 模板。
/// `inst` 为 owned（families 展开后每个 variant 是一条合成指令）。
pub(crate) struct InstInfo<'a> {
    inst: Instruction,
    form: &'a Form,
    vn: syn::Ident,
    mnemonic: String,
    /// 操作数绑定：(位域名, 字段标识, 槽, 角色)。
    operands: Vec<(String, syn::Ident, &'a OperandSlot, OperandRole)>,
}

fn collect_inst_infos<'a>(m: &'a V12Model) -> Result<Vec<InstInfo<'a>>, String> {
    // 展开 families：每个 variant 合成一条指令（family.fields 共享 +
    // variant.fields 覆盖；form 共享；asm = 家族模板 {name} → 变体名小写）
    let mut insts: Vec<Instruction> = m.instructions.clone();
    for fam in &m.families {
        for var in &fam.variants {
            let mut fields = fam.fields.clone().unwrap_or_default();
            if let Some(vf) = &var.fields {
                for (k, v) in vf {
                    fields.insert(k.clone(), *v);
                }
            }
            let asm = var
                .asm
                .clone()
                .unwrap_or_else(|| fam.asm.replace("{name}", &var.name.to_lowercase()));
            insts.push(Instruction {
                name: var.name.clone(),
                form: fam.form.clone(),
                opcode: var.opcode,
                fields: if fields.is_empty() {
                    None
                } else {
                    Some(fields)
                },
                asm,
                when: var.when.clone(),
                vex: None,
                opsize: None,
                rex_w: None,
                effect: Vec::new(),
                implicit_regs: None,
                global_reloc: None,
            });
        }
    }
    let mut out = Vec::new();
    for inst in &insts {
        let form = m
            .forms
            .iter()
            .find(|f| f.name == inst.form)
            .ok_or_else(|| {
                format!(
                    "[[instructions.{}]]: form '{}' missing",
                    inst.name, inst.form
                )
            })?;
        // 变长 form（无 opcode_field）不需要 operand_fields；定宽需要
        if form.opcode_field.is_none() && form.operand_fields.is_some() {
            return Err(format!(
                "[[instructions.{}]]: form '{}' declares operand_fields without opcode_field (vlen forms use modrm semantic keys)",
                inst.name, inst.form
            ));
        }
        if inst.opcode.is_none() && form.opcode_reg.is_none() {
            return Err(format!(
                "[[instructions.{}]]: opcode required (or form opcode_reg for +r forms)",
                inst.name
            ));
        }
        // 从 asm 模板解析助记符 + 操作数声明（新语法：占位符内联槽/角色）
        let (mnemonic, uses) = parse_asm_decl(&inst.asm, &inst.name)?;
        let of = form.operand_fields.as_deref();
        let fixed = form.opcode_field.is_some();
        let mut operands = Vec::new();
        for (i, op) in uses.iter().enumerate() {
            let slot = m
                .operand_slots
                .iter()
                .find(|s| s.name == op.slot)
                .ok_or_else(|| {
                    format!(
                        "[[instructions.{}]]: operand slot '{}' missing (asm '{}')",
                        inst.name, op.slot, inst.asm
                    )
                })?;
            // 字段名：定宽 → 位域名（operand_fields[i] 或 field 覆盖）；
            // 变长 → 语义名（dest/src/cond/mem/imm/target；field 覆盖优先）。
            let field = if fixed {
                if let Some(f) = &op.field {
                    f.clone()
                } else {
                    of.and_then(|f| f.get(i)).cloned().ok_or_else(|| {
                        format!(
                            "[[instructions.{}]]: operand {} missing operand_fields entry",
                            inst.name, i
                        )
                    })?
                }
            } else {
                op.field.clone().unwrap_or_else(|| {
                    let mut name = semantic_operand_name(op, slot, i);
                    // 重名去重：src→src2/src3、dest→dest2、imm→imm2 …
                    let mut n = 2;
                    while operands.iter().any(|(f, _, _, _)| f == &name) {
                        name = format!("{}{}", semantic_operand_name(op, slot, i), n);
                        n += 1;
                    }
                    name
                })
            };
            let fid = format_ident!("{}", field);
            let role = op.role.unwrap_or(OperandRole::In);
            operands.push((field, fid, slot, role));
        }
        out.push(InstInfo {
            inst: inst.clone(),
            form,
            vn: pascal_ident(&inst.name),
            mnemonic,
            operands,
        });
    }
    Ok(out)
}

/// 从 asm 模板解析：首词 = 助记符；操作数占位符 `{i:[槽:角色]}` 内联声明。
/// 返回 (助记符, 操作数声明表，按序号排序且连续)。
/// validate.rs 也调用本函数做操作数校验（槽存在/角色合法/序号连续）。
pub(crate) fn parse_asm_decl(
    asm: &str,
    inst_name: &str,
) -> Result<(String, Vec<OperandUse>), String> {
    let ctx = || format!("[[instructions.{inst_name}]] asm");
    let (mnemonic, rest) = asm
        .split_once(char::is_whitespace)
        .map_or_else(|| (asm.trim(), ""), |(m, r)| (m.trim(), r.trim()));
    if mnemonic.is_empty() {
        return Err(format!("{}: asm must not be empty", ctx()));
    }
    if mnemonic.contains('{') || mnemonic.contains('}') {
        return Err(format!(
            "{}: asm mnemonic '{mnemonic}' must not contain '{{'/'}}'",
            ctx()
        ));
    }
    let (segs, decls) = asm::parse_template_full(rest)?;
    // 序号连续性校验：操作数 0..k 全部出现且有声明
    let mut sorted: Vec<(usize, String, Option<String>)> = decls;
    sorted.sort_by_key(|(n, _, _)| *n);
    for (i, (n, slot, _role)) in sorted.iter().enumerate() {
        if *n != i {
            return Err(format!(
                "{}: operand indices must be contiguous 0..k (found {{{}}})",
                ctx(),
                n
            ));
        }
        if slot.is_empty() {
            return Err(format!(
                "{}: operand {{{n}}} missing slot declaration",
                ctx()
            ));
        }
        // 占位符在模板中出现次数 ≤1（重复引用同一操作数 → 歧义）
        if segs
            .iter()
            .filter(|s| matches!(s, asm::Seg::Op(k) if *k == *n))
            .count()
            > 1
        {
            return Err(format!(
                "{}: operand {{{n}}} referenced more than once in asm template",
                ctx()
            ));
        }
    }
    let mut uses = Vec::new();
    for (n, slot, role) in sorted {
        let role = match role.as_deref() {
            None | Some("in") => OperandRole::In,
            Some("out") => OperandRole::Out,
            Some("inout") => OperandRole::InOut,
            Some(other) => {
                return Err(format!(
                    "{}: operand {{{n}}} role '{other}' invalid (in/out/inout)",
                    ctx()
                ));
            }
        };
        uses.push(OperandUse {
            slot,
            role: Some(role),
            field: None,
        });
        let _ = n;
    }
    Ok((mnemonic.to_string(), uses))
}

// ─────────────────────────────── 寄存器表 ───────────────────────────────

/// 每使用到的寄存器组生成 `{group}_name(i)` 与 `__parse_reg_{group}(s)`；
/// 存在多态 Reg 槽（class=None）时另生成 `__parse_reg_any`（任意 GPR 名）。
fn gen_reg_tables(model: &V12Model) -> Result<TokenStream, String> {
    // 只生成指令操作数实际引用的组，避免无用函数警告
    let mut fns = Vec::new();
    let mut try_ref = Vec::new();
    let mut from_str = Vec::new();
    let mut to_str = Vec::new();
    let case_insensitive_regs: bool = model.meta.case_insensitive_regs.unwrap_or_default();
    for (reg_class, g) in &model.reg {
        let names = group_names(g)?;
        let mut reg_class_ref = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let reg_lit = syn::Ident::new(name, proc_macro2::Span::call_site());

            // TryFrom<RegClass> for Reg 的类型模式分支
            let i = i as u32;
            reg_class_ref.push(quote! { #i => Ok(Reg::#reg_lit) });

            // FromStr for Reg 的模式分支
            from_str.push(match case_insensitive_regs {
                true => {
                    let name = name.to_lowercase();
                    quote! { #name => Ok(Reg::#reg_lit) }
                }
                false => quote! { #name => Ok(Reg::#reg_lit) },
            });

            // ToString for Reg 的模式分支
            to_str.push(quote! { Reg::#reg_lit => #name });
        }

        try_ref.push(quote! {
            RegRef{class,id} if class == #reg_class => match id {
                #(#reg_class_ref,)*
                _ => Err(format!("Invalid register id {} for class {}", id, class))
            }
        });
    }

    let lowercase = match case_insensitive_regs {
        true => quote! {s.trim().to_lowercase().as_str()},
        false => quote! {s.trim()},
    };
    fns.push(quote! {
        use ::forge_ir::{RegRef,RegClass};
        use ::std::str::FromStr;

        impl TryFrom<RegRef> for Reg{
            type Error = String;

            fn try_from(reg_ref: RegRef) -> Result<Self, Self::Error> {
                match reg_ref {
                    #(#try_ref,)*
                    _=> Err(format!("invalid register reference: {reg_ref:?}"))
                }
            }
        }

        impl FromStr for Reg {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match #lowercase {
                    #(#from_str,)*
                    _=> Err(format!("invalid register name: {s}"))
                }
            }
        }

        impl From<Reg> for &'static str {
            fn from(reg: Reg) -> Self {
                match reg {
                    #(#to_str),*
                }
            }
        }
    });
    Ok(quote! { #(#fns)* })
}

/// 组寄存器名列表（共享实现见 `super::shared::group_names`；
/// 此处 re-export 保持调用点不变）。
use super::shared::group_names;

// ─────────────────────────────── Inst 枚举 ───────────────────────────────

fn gen_inst_enum(infos: &[InstInfo]) -> TokenStream {
    let variants: Vec<_> = infos
        .iter()
        .map(|info| {
            let vn = &info.vn;
            if info.operands.is_empty() {
                quote! { #vn }
            } else {
                let fs: Vec<_> = info
                    .operands
                    .iter()
                    .map(|(_, fid, slot, _)| {
                        let ty = field_ty(slot);
                        quote! { #fid: #ty }
                    })
                    .collect();
                quote! { #vn { #(#fs),* } }
            }
        })
        .collect();
    quote! {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub enum Inst {
            #(#variants),*,
            /// 伪指令原始字节（`.byte`/`.align` 展开；encode 原样输出）。
            Raw(Vec<u8>),
        }
    }
}

// ─────────────────────────────── encode ───────────────────────────────

fn gen_encode(infos: &[InstInfo], m: &V12Model) -> Result<TokenStream, String> {
    let little = m.meta.endian == Endian::Little;
    let mut arms = Vec::new();
    for info in infos {
        let vn = &info.vn;
        let opcode_field = single_field(
            m,
            info.form.opcode_field.as_deref().unwrap(),
            "opcode_field",
        )?;
        let opcode = info.inst.opcode.unwrap();
        let mut stmts: Vec<TokenStream> = Vec::new();
        // 主 opcode
        stmts.extend(place_ts(quote! { #opcode }, opcode_field));
        // fields 固定值
        if let Some(fields) = &info.inst.fields {
            for (fname, val) in fields {
                let bf = single_field(m, fname, "fields")?;
                stmts.extend(place_ts(quote! { #val }, bf));
            }
        }
        // 操作数
        for (fname, fid, _, _) in &info.operands {
            let bf = get_bf(m, fname)?;
            stmts.extend(place_ts(quote! { *#fid as u64 }, bf));
        }
        // 未覆盖位域天然为 0（__w 初始 0）——无需显式置零
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
        let out = if little {
            quote! { Ok((__w as u32).to_le_bytes().to_vec()) }
        } else {
            quote! { Ok((__w as u32).to_be_bytes().to_vec()) }
        };
        arms.push(quote! {
            #pat => {
                let mut __w: u64 = 0;
                #(#stmts)*
                #out
            }
        });
    }
    Ok(quote! {
        /// 编码单条指令为字节（定宽：小端 4 字节；32 位定宽）。
        pub fn encode(inst: &Inst) -> Result<Vec<u8>, String> {
            match inst {
                #(#arms,)*
                Inst::Raw(bytes) => Ok(bytes.clone()),
            }
        }
    })
}

// ─────────────────────────────── decode ───────────────────────────────

// ─────────────────────── 定宽 decode：位级决策树 ───────────────────────
//
// 与变长 decode 的字节前缀 trie 对称：定宽按**常量位段**（opcode_field +
// fields 固定值）构建位级决策树（LLVM DecoderEmitter 形态），边 =
// (bit, width, value)；叶 = 指令的补集零 guard + 字段提取。
// 语义：叶节点（短常量键）优先尝试，叶内按声明序——与变长 trie 一致；
// 常量位段重叠且取值一致（歧义）→ 生成期硬报错。

/// 定宽位 trie 节点：叶 arm（声明序）+ 位段边 (bit, width, value, child)。
struct BitTrieNode {
    arms: Vec<TokenStream>,
    edges: Vec<(u32, u32, u64, usize)>,
}

/// decode 决策树的叶分组：(位段条件, 匹配 arm)。
type BitTrieGroup = (Vec<(u32, u32, u64)>, TokenStream);

fn build_bit_trie(groups: &[BitTrieGroup]) -> Vec<BitTrieNode> {
    let mut nodes = vec![BitTrieNode {
        arms: Vec::new(),
        edges: Vec::new(),
    }];
    for (key, arm) in groups {
        let mut idx = 0usize;
        for (bit, width, value) in key {
            let found = nodes[idx]
                .edges
                .iter()
                .position(|(b, w, v, _)| *b == *bit && *w == *width && *v == *value);
            idx = if let Some(e) = found {
                nodes[idx].edges[e].3
            } else {
                let child = nodes.len();
                nodes.push(BitTrieNode {
                    arms: Vec::new(),
                    edges: Vec::new(),
                });
                nodes[idx].edges.push((*bit, *width, *value, child));
                child
            };
        }
        nodes[idx].arms.push(arm.clone());
    }
    nodes
}

/// 校验位 trie 各节点边的 (bit,width,value) 两两不相交：位区间相交且共享区
/// 取值一致 → 同一字可同时命中两条边 → 歧义 → 拒绝。
fn check_bit_trie_overlaps(nodes: &[BitTrieNode]) -> Result<(), String> {
    for (ni, node) in nodes.iter().enumerate() {
        for (i, (b1, w1, v1, _)) in node.edges.iter().enumerate() {
            for (j, (b2, w2, v2, _)) in node.edges.iter().enumerate().skip(i + 1) {
                let lo = (*b1).max(*b2);
                let hi = (*b1 + *w1).min(*b2 + *w2);
                if lo >= hi {
                    continue; // 位区间不相交 → 无歧义
                }
                let m = if hi - lo >= 64 {
                    u64::MAX
                } else {
                    (1u64 << (hi - lo)) - 1
                };
                let va = (v1 >> (lo - b1)) & m;
                let vb = (v2 >> (lo - b2)) & m;
                if va == vb {
                    return Err(format!(
                        "v12 decode bit-trie: node {ni} edges [{i}](bit {b1} w {w1} v {v1:#x}) and [{j}](bit {b2} w {w2} v {v2:#x}) overlap at bits [{lo},{hi}) — ambiguous; make the encodings disjoint"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// 将位 trie 展开为嵌套 `if`（叶 arm 先试，随后按边 descend）。
fn emit_bit_trie(nodes: &[BitTrieNode], idx: usize) -> TokenStream {
    let node = &nodes[idx];
    let mut body = TokenStream::new();
    for arm in &node.arms {
        body.extend(arm.clone());
    }
    if !node.edges.is_empty() {
        let mut chain = quote! {};
        for (bit, w, value, child) in node.edges.iter().rev() {
            let sub = emit_bit_trie(nodes, *child);
            let mask = if *w >= 64 {
                quote! { u64::MAX }
            } else {
                let m = (1u64 << *w) - 1;
                quote! { #m }
            };
            chain = quote! {
                if ((__w >> #bit) & #mask) == #value {
                    #sub
                } else {
                    #chain
                }
            };
        }
        body.extend(chain);
    }
    body
}

fn gen_decode(infos: &[InstInfo], m: &V12Model) -> Result<TokenStream, String> {
    let little = m.meta.endian == Endian::Little;
    let mut groups: Vec<BitTrieGroup> = Vec::new();
    for info in infos {
        let vn = &info.vn;
        // 常量路径：opcode_field + fields 固定值（单一位段；散布在单上下文报错）
        let mut key: Vec<(u32, u32, u64)> = Vec::new();
        // 覆盖位域（opcode + fields + 操作数位域）：其**补集**必须为 0。
        // 这统一处理了 form 的隐式 0 位域（纯 opcode 形式如 NOP/ECALL 的
        // 其余位、操作数不足时多余位域位置、fields 之外的固定 0 位）。
        let mut covered_ranges: Vec<(u32, u32)> = Vec::new();
        let opcode_field = single_field(
            m,
            info.form.opcode_field.as_deref().unwrap(),
            "opcode_field",
        )?;
        let opcode = info.inst.opcode.unwrap();
        let (bo, bw) = single(opcode_field);
        let bmask = if bw == 64 { u64::MAX } else { (1u64 << bw) - 1 };
        key.push((bo, bw, opcode & bmask));
        covered_ranges.extend(bf_ranges(opcode_field));
        if let Some(fields) = &info.inst.fields {
            for (fname, val) in fields {
                let bf = single_field(m, fname, "fields")?;
                let (fo, fw) = single(bf);
                let fmask = if fw == 64 { u64::MAX } else { (1u64 << fw) - 1 };
                key.push((fo, fw, *val & fmask));
                covered_ranges.extend(bf_ranges(bf));
            }
        }
        // 操作数位域（其位不被零 guard 约束）
        for (fname, _, _, _) in &info.operands {
            let bf = get_bf(m, fname)?;
            covered_ranges.extend(bf_ranges(bf));
        }
        // 补集零 guard：`(w & !covered_mask) == 0`
        let mut covered_mask: u64 = 0;
        for (s, e) in &covered_ranges {
            let w = e - s;
            let mask = if w == 64 { u64::MAX } else { (1u64 << w) - 1 };
            covered_mask |= mask << s;
        }
        let guard = quote! { ((__w & !(#covered_mask)) == 0) };
        // 字段提取
        let mut binds: Vec<TokenStream> = Vec::new();
        let mut ctor_fields: Vec<TokenStream> = Vec::new();
        for (fname, fid, slot, _) in &info.operands {
            let bf = get_bf(m, fname)?;
            let raw = extract_ts(bf);
            let expr: TokenStream = match slot.kind {
                OperandKind::Reg => field_ctor_expr(slot, quote! { #raw as u32 }),
                _ => {
                    let signed = slot.signed.unwrap_or(false) || slot.kind == OperandKind::Label;
                    if signed {
                        let w = slot.width.unwrap_or(64);
                        sign_extend_ts(raw, w)
                    } else {
                        quote! { #raw as i64 }
                    }
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
        groups.push((
            key,
            quote! {
                if #guard {
                    #(#binds)*
                    return Some((#ctor, 4));
                }
            },
        ));
    }
    let nodes = build_bit_trie(&groups);
    check_bit_trie_overlaps(&nodes)?;
    let dispatch = emit_bit_trie(&nodes, 0);
    let read = if little {
        quote! { u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64 }
    } else {
        quote! { u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64 }
    };
    Ok(quote! {
        /// 解码 4 字节为指令；常量位段位级决策树（叶节点优先，叶内声明序），
        /// 无匹配 → None。返回 (指令, 消费字节数)。
        pub fn decode(bytes: &[u8]) -> Option<(Inst, usize)> {
            if bytes.len() < 4 {
                return None;
            }
            let __w = #read;
            #dispatch
            None
        }

        /// 解码并报告部分匹配偏移：定宽 ISA 失败 → Err(bytes.len() 不足 ? len : 0)。
        /// 供 `TargetDecoder::decode` 的 `DecodeError::InvalidBytes(n)` 使用。
        pub fn decode_partial(bytes: &[u8]) -> Result<(Inst, usize), usize> {
            match decode(bytes) {
                Some(r) => Ok(r),
                None => Err(if bytes.len() < 4 { bytes.len() } else { 0 }),
            }
        }
    })
}

// ─────────────────────────────── 位域助手 ───────────────────────────────

fn get_bf<'a>(m: &'a V12Model, name: &str) -> Result<&'a Bitfield, String> {
    m.conventions
        .bitfields
        .get(name)
        .ok_or_else(|| format!("bitfield '{name}' not declared in [conventions.bitfields]"))
}

/// 单一位段（散布位段在此报错——迭代 2 中 opcode/fields/隐式 0 不支持散布）。
fn single_field<'a>(m: &'a V12Model, name: &str, ctx: &str) -> Result<&'a Bitfield, String> {
    let bf = get_bf(m, name)?;
    if bf.pieces.is_some() {
        return Err(format!(
            "bitfield '{name}' ({ctx}) is scattered — iteration 2 requires single {{offset, width}} here"
        ));
    }
    Ok(bf)
}

fn single(bf: &Bitfield) -> (u32, u32) {
    (bf.offset.unwrap(), bf.width.unwrap())
}

fn mask_ts(w: u32) -> TokenStream {
    if w == 64 {
        quote! { u64::MAX }
    } else {
        let m = (1u64 << w) - 1;
        quote! { #m }
    }
}

/// encode：`__w |= (value & mask) << offset`（单）或逐 piece `((value >> shift) & mask) << offset`。
fn place_ts(value: TokenStream, bf: &Bitfield) -> Vec<TokenStream> {
    match &bf.pieces {
        None => {
            let (off, w) = single(bf);
            let mask = mask_ts(w);
            vec![quote! { __w |= (#value & #mask) << #off; }]
        }
        Some(ps) => ps
            .iter()
            .map(|p| {
                let mask = mask_ts(p.width);
                let sh = p.shift;
                let off = p.offset;
                quote! { __w |= ((#value >> #sh) & #mask) << #off; }
            })
            .collect(),
    }
}

/// decode：字段原始提取（u64）→ 整体加括号（防 `|` 与后续 `<<`/`as` 优先级问题）。
/// 单 → `(w >> off) & mask`；散布 → pieces OR 累加。
fn extract_ts(bf: &Bitfield) -> TokenStream {
    match &bf.pieces {
        None => {
            let (off, w) = single(bf);
            let mask = mask_ts(w);
            quote! { ((__w >> #off) & #mask) }
        }
        Some(ps) => {
            let parts: Vec<_> = ps
                .iter()
                .map(|p| {
                    let mask = mask_ts(p.width);
                    let sh = p.shift;
                    let off = p.offset;
                    quote! { (((__w >> #off) & #mask) << #sh) }
                })
                .collect();
            quote! { (#(#parts)|*) }
        }
    }
}

/// 符号扩展：`((raw) << (64-w)) as i64 >> (64-w)`（raw 已整体加括号）。
/// `pub(crate)`：变长模块（vlen.rs）复用。
pub(crate) fn sign_extend_ts(raw: TokenStream, width: u32) -> TokenStream {
    let sh = 64 - width;
    quote! { (((#raw) << #sh) as i64) >> #sh }
}

/// 位域的字位域范围列表（piece 的 [offset, offset+width)）。
fn bf_ranges(bf: &Bitfield) -> Vec<(u32, u32)> {
    match &bf.pieces {
        None => {
            let (off, w) = single(bf);
            vec![(off, off + w)]
        }
        Some(ps) => ps.iter().map(|p| (p.offset, p.offset + p.width)).collect(),
    }
}
