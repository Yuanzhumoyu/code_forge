//! ISA-DSL codegen — 迭代 2：定宽（32 位）ISA 自包含模块生成。
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
//!   有符号槽按槽宽度符号扩展（v11 不扩展，ISA-DSL 规范修正）。
//!
//! - `disassemble(&Inst) -> String` / `assemble(&str) -> Result<Inst, String>`：
//!   asm 模板驱动（`{0}`/`{1}` 占位符，mnemonic 与模板分离），支持 simple、
//!   memory（`{I}({J})`）与 memory0（`({J})`）三种操作数形状。
//!
//! 迭代 2 范围：`[encoding].bits == 32` 的定宽 ISA（riscv64 试点）；
//! 变长 form（modrm/vex 语义键）与 64/16 位定宽在迭代 3+ 支持。

use super::model::*;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::BTreeSet;

pub(crate) mod asm;
/// TargetFrameLowering/TargetABI/emit（integration.rs 拆分）。
pub(crate) mod frame;
pub(crate) mod integration;
/// TargetLowering 生成（integration.rs 拆分；依赖 integration 的工具函数）。
pub(crate) mod lowering;
/// Reg 枚举 / MachineInst / Encoder / Decoder / Disasm / Assembler
/// （integration.rs 拆分）。
pub(crate) mod machine;
/// 内存操作数文本模板（v16/S9）：`__render_mem`/`__mem` 从同一份模板派生。
pub(crate) mod mem;
/// 搬运族派生（v20 V8）：`data_width` + 操作数结构 → 方向/寄存器族/宽度。
pub(crate) mod moves;
/// 占位符注册表——lowering 模板 `{...}` token 的唯一事实源（第三轮重构）。
pub(crate) mod placeholder;
/// v18 S6：生成期自测（`#[cfg(test)] mod __spec_tests`）。
pub(crate) mod spec;
/// 变长（VEX/EVEX/前缀扫描）encode/decode——x86 专用机制，独立文件组织。
pub(crate) mod vlen;

// ─────────────────────────────── 共享工具 ───────────────────────────────

/// 字符串 → PascalCase 标识符文本（v11 `codegen` 模块的同名工具，随语法层
/// 删除迁移至 ISA-DSL 生成器；`_`/`-`/空格 为分隔符，数字开头补 "Inst" 前缀）。
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
/// 槽位 class 缺失（多类槽 gprx）→ 主 GPR 类常量 `__DEFAULT_GPR_CLASS`
/// （元数据派生；历史实现写死 `GPR(8)`——1 字节寄存器 ISA 下不存在该类）。
fn reg_class_expr(slot: &OperandSlot) -> TokenStream {
    match slot.class.as_ref() {
        Some(c) => match c {
            RegClass::GPR(w) => quote! { forge_ir::RegClass::GPR(#w) },
            RegClass::FPR(w) => quote! { forge_ir::RegClass::FPR(#w) },
            RegClass::VEC(w) => quote! { forge_ir::RegClass::VEC(#w) },
            RegClass::KReg(w) => quote! { forge_ir::RegClass::KReg(#w) },
        },
        _ => quote! { __DEFAULT_GPR_CLASS },
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
            let ctor = match slot.class.as_ref() {
                Some(c) => quote! { #c },
                // 槽 class 缺失（多类槽 gprx）→ 主 GPR 类常量（元数据派生；
                // 历史实现写死 `GPR(8)`）。
                None => quote! { __DEFAULT_GPR_CLASS },
            };
            quote! { <Reg as TryFrom<RegRef>>::try_from(RegRef::new(#ctor,#v)).unwrap() }
        }
        None => {
            // 多类 GPR（宽度视图）：按 decode 扫描出的 `__opsize`（**字节**，
            // 由 `[encoding].default_opsize` 提供缺省）选择视图——assemble/encode
            // 侧已按实际寄存器宽度还原，decode 反向。
            quote! {
                <Reg as TryFrom<RegRef>>::try_from(RegRef::new(RegClass::GPR(__opsize),#v)).unwrap()
            }
        }
    }
}

/// DSL 操作数名 → Rust 字段标识符（v18 S7d）。
///
/// 名字全部来自 `ops = ["dst:r:out", …]` 的 `dst`——作者写什么，用户面就看到什么。
/// 归一化只处理"不能直接当标识符"的情况（数字开头/含 `-` → `_`；Rust 关键字 →
/// 原始标识符 `r#…`；`self`/`Self`/`super`/`crate` 不能原始化 → 末尾加 `_`），
/// 不做任何语义改名。
fn operand_field_ident(name: &str, inst_name: &str, i: usize) -> Result<syn::Ident, String> {
    let span = proc_macro2::Span::call_site();
    let mut s = String::with_capacity(name.len() + 1);
    for (n, ch) in name.chars().enumerate() {
        let ch = if ch.is_ascii_alphanumeric() || ch == '_' {
            ch
        } else {
            '_'
        };
        if n == 0 && ch.is_ascii_digit() {
            s.push('_');
        }
        s.push(ch);
    }
    if s.is_empty() {
        return Err(format!(
            "[[instructions.{inst_name}]].ops[{i}]: 操作数名 '{name}' 不能作为 Rust 字段名"
        ));
    }
    // 普通标识符 → 直接用；关键字 → 原始标识符；不可原始化 → 加后缀。
    if let Ok(id) = syn::parse_str::<syn::Ident>(&s) {
        return Ok(id);
    }
    if !matches!(s.as_str(), "self" | "Self" | "super" | "crate" | "extern")
        && syn::parse_str::<syn::Ident>(&format!("r#{s}")).is_ok()
    {
        return Ok(syn::Ident::new_raw(&s, span));
    }
    Ok(syn::Ident::new(&format!("{s}_"), span))
}

/// 变长 ISA 的语义化操作数名（替代位置名 op{i}，恢复 v11 可读性）：
/// Reg out→dest、Reg in→src/src2/src3、cond→cond、
/// mem→mem、imm→imm、label→target；重名时追加序号。
///
/// **只是编码键名**（查 `[conventions.bitfields]` / modrm 角色 / 立即数编码表），
/// 与生成的 Rust 字段名无关（后者见 [`operand_field_ident`]）。
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
        OperandKind::Bits => "fence",
    }
    .to_string()
}

// ─────────────────────────────── 入口 ───────────────────────────────

pub fn generate(model: &IsaModel) -> Result<TokenStream, String> {
    generate_with(model, true)
}

/// [`generate`] 的可选项版本：`spec_tests` 控制是否生成 `#[cfg(test)] mod
/// __spec_tests`（v18 S6）。夹具谱在 `tests/common/mod.rs` 里会被多个测试二进制
/// 反复展开，故那里显式关掉、由专门的用例二进制打开（见 `isa_from_file!` 参数）。
pub fn generate_with(model: &IsaModel, spec_tests: bool) -> Result<TokenStream, String> {
    generate_with_parts(model, spec_tests, crate::Parts::all())
}

/// [`generate_with`] 的**部件级**版本（v18 S7d）：`parts` 控制四块可选件是否发射。
/// `Inst` 枚举 / 寄存器表 / 内存支撑恒定发射（任何部件都依赖它们）；位域助手
/// `__place`/`__bits` 跟着 encode/decode 走。
pub fn generate_with_parts(
    model: &IsaModel,
    spec_tests: bool,
    parts: crate::Parts,
) -> Result<TokenStream, String> {
    // 只有 `prefix_scan`（x86 风格前缀链）走 `vlen.rs` 的定长切片路径；
    // `fixed` 与 `mixed` 都按位域编解码（`mixed` 逐指令取字长）。
    let prefix_scan = model.is_prefix_scan();
    // 定宽 ISA 的指令字长是 ISA 数据（`[encoding].bits`）；
    // `mixed` 逐指令取 `width`，无单一字长。字长非法/缺失在此报错（不再只认 32）。
    if !model.is_variable_length() {
        model.inst_bytes()?;
    }
    let infos = collect_inst_infos(model)?;
    let reg_tables = gen_reg_tables(model)?;
    let mem_support = gen_mem_support(model, &infos)?;
    let inst_enum = gen_inst_enum(&infos);
    let (encode_fn, decode_fn) = if prefix_scan {
        (
            vlen::gen_vlen_encode(&infos, model)?,
            vlen::gen_vlen_decode(&infos, model)?,
        )
    } else {
        (gen_encode(&infos, model)?, gen_decode(&infos, model)?)
    };
    // 定宽字位域助手（`__place`/`__bits`，字节数组字，字长任意）——fixed/mixed 路径用。
    let bit_helpers = if prefix_scan {
        quote! {}
    } else {
        gen_bit_helpers()
    };
    // 逻辑立即数助手（`encode = "logical_imm"` 的槽才要）：值 ↔ (N, immr, imms)。
    // 与位域助手一样**与 `parts` 无关**——encode/decode 哪天被切开，助手也还在。
    let logic_imm_helpers = if !prefix_scan
        && model
            .operand_slots
            .iter()
            .any(|s| s.encode == Some(SlotEncode::LogicalImm))
    {
        gen_logic_imm_helpers()
    } else {
        quote! {}
    };
    // 命名位集合助手（`kind = "bits"` 的槽才要）：与 parts 无关（同位域助手）。
    let bitset_helpers = if !prefix_scan
        && model
            .operand_slots
            .iter()
            .any(|s| s.kind == OperandKind::Bits)
    {
        gen_bitset_helpers()
    } else {
        quote! {}
    };
    // ── 可选部件（`parts = [...]`，v18 S7d）──
    let (disasm_fn, asm_fn) = if parts.asm {
        (
            asm::gen_disassemble(&infos)?,
            // `gen_assemble` 同时发射线性扫描探针 `could_be_instruction`（给下游 crate 的
            // "真实汇编语料解析档"用：诊断"这段是不是本 ISA 的指令"，不假设首词 = 助记符）。
            asm::gen_assemble(&infos, model)?,
        )
    } else {
        (quote! {}, quote! {})
    };
    let encode_fn = if parts.encode {
        encode_fn
    } else {
        quote! {}
    };
    let decode_fn = if parts.decode {
        decode_fn
    } else {
        quote! {}
    };
    let bit_helpers = if parts.encode || parts.decode {
        bit_helpers
    } else {
        quote! {}
    };
    // `asm` 部件的 `assemble` 会调 `__pseudo_expand`（`[[pseudo]]` 展开），而这个助手平时
    // 由 `tm` 部件里的 `gen_assembler` 发射。**只开 asm、不开 tm** 时必须在这里补发，
    // 否则生成物引用未定义函数（v19 V3a 由 `forge-isa test` 在 riscv64 上抓到：谱里的
    // `li` 伪指令 + `parts = ["encode","decode","asm"]` → `E0425: cannot find function
    // __pseudo_expand`）。`tm` 在时不发，避免重复定义。
    let pseudo_helpers = if parts.asm && !parts.tm {
        machine::gen_pseudo_helpers(model)
    } else {
        quote! {}
    };
    // `Reg` 枚举/`PhysReg` impl/类常量是 `Inst` 字段类型的前提：`tm` 部件不在时
    // 必须单独发射（`tm` 在时由集成层发射，生成物因此逐字节不变）。
    let reg_enum = if parts.tm {
        quote! {}
    } else {
        integration::gen_reg_enum_only(model)?
    };
    // v18 S6：生成期自测（每条指令的规格往返 + 边界）。
    let spec_tests_ts = if spec_tests {
        spec::gen_spec_tests(&infos, model)?
    } else {
        quote! {}
    };
    // 迭代 5：TargetMachine 集成层（MachineInst/Encoder/Decoder/ABI/
    // FrameLowering/Lowering/TargetMachine 组装）。
    let integration = if parts.tm {
        integration::gen_integration(&infos, model)?
    } else {
        quote! {}
    };
    Ok(quote! {
        // ── ISA-DSL 生成模块（迭代 2/3/3b：自包含 encode/decode/asm）──
        #reg_tables
        #mem_support
        #inst_enum
        #bit_helpers
        #logic_imm_helpers
        #bitset_helpers
        #pseudo_helpers
        #encode_fn
        #decode_fn
        #disasm_fn
        #asm_fn
        #reg_enum
        // ── TargetMachine 集成层（迭代 5）──
        #integration
        // ── 生成期自测（v18 S6，`cfg(test)`）──
        #spec_tests_ts
    })
}

/// 有 mem 操作数时生成 `MemRef` 结构 + 反汇编渲染 `__render_mem`（自包含）。
/// `__render_mem` 从 `[conventions.mem] templates` 的**第 0 条**派生（v16；v20 V10
/// 起模板是列表，渲染只用第 0 条——渲染必须唯一）；缺省 = x86 `[...]`。
fn gen_mem_support(model: &IsaModel, _infos: &[InstInfo]) -> Result<TokenStream, String> {
    let tpls = mem::effective_templates(&model.conventions.mem);
    let first = tpls
        .first()
        .ok_or("[conventions.mem]: `templates` 不能为空".to_string())?;
    let items = mem::parse_mem_template(first)?;
    let render_mem = mem::gen_render_mem(&items);
    Ok(quote! {
        /// 内存操作数（自包含；base 为物理寄存器索引，disp 为字节位移；
        /// index 为可选索引寄存器（x86 SIB index），scale 为缩放 1/2/4/8）。
        ///
        /// `base` 是 **Option**（v20 V10）：Intel 系 ISA 有"没有基址"的写法
        /// （x86 `mov rax, [0x1234]`），它由 [`[conventions.mem]`] 的模板形状声明
        /// ——**模板里写了 `{base}` 就是"这条写法必须有基址"**（缺了照样解析失败），
        /// 不写 `{base}` 的模板产出的就是 `base: None`（绝对地址）。这样"要不要基址"
        /// 只有一处声明（模板本身），既不需要新键，也没有第二个开关可与之矛盾。
        ///
        /// [`[conventions.mem]`]: ../../../docs/reference/isa-dsl.md
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub struct MemRef {
            /// 基址寄存器；`None` = 无基址（绝对地址 / 立即数地址）。
            pub base: Option<Reg>,
            pub disp: i64,
            /// 索引寄存器（None = 无索引）。
            pub index: Option<Reg>,
            /// 索引缩放（1/2/4/8；缺省 1）。
            pub scale: u8,
        }
        #render_mem
    })
}

/// 单条指令的生成信息：form 解析 + 操作数→位域绑定 + mnemonic/asm 模板。
/// `inst` 为 owned（`[[templates]]` 展开在解析期完成，这里就是完整指令表）。
///
/// 字段 `pub(crate)`：`report`（CLI 的 `insts`/`explain`/`diff`，v18 S7b）直接读它，
/// 不重复实现"form 预设 ⊕ 指令级逐键覆盖"的判定（单一事实源）。
pub(crate) struct InstInfo<'a> {
    pub(crate) inst: Instruction,
    /// **已解析的编码键**：`form` 预设 ⊕ 指令级逐键覆盖（`EncKeys::over`）。
    /// 下游只读这一份，不再各自 `inst.x.or(form.x)`——覆盖语义单点实现。
    pub(crate) form: EncKeys,
    pub(crate) vn: syn::Ident,
    /// 操作数绑定：(位域名, 字段标识, 槽, 角色)。
    pub(crate) operands: Vec<(String, syn::Ident, &'a OperandSlot, OperandRole)>,
}

pub(crate) fn collect_inst_infos<'a>(m: &'a IsaModel) -> Result<Vec<InstInfo<'a>>, String> {
    // `[[templates]]` 已在解析期展开进 `m.instructions`（`v18 S2`），此处直接用。
    let insts: Vec<Instruction> = m.instructions.clone();
    let mut out = Vec::new();
    for inst in &insts {
        // 编码键 = form 预设（可省略）⊕ 指令级逐键覆盖（指令优先）
        let preset: EncKeys = match &inst.form {
            None => EncKeys::default(),
            Some(name) => m
                .forms
                .iter()
                .find(|f| &f.name == name)
                .ok_or_else(|| format!("[[instructions.{}]]: form '{name}' missing", inst.name))?
                .keys
                .clone(),
        };
        let mut enc = inst.enc.over(&preset);
        // 从 asm 模板解析操作数声明（命名形态：ops 声明 + asm 引用；v17 起不再拆助记符）。
        let asm = inst.asm.clone();
        let (uses, norm_asm) = parse_asm_decl(
            &asm,
            inst.ops.as_deref(),
            &inst.name,
            &m.variant_param_names(),
        )?;
        // `opsize = "<操作数名>"` → 按声明序解析成位置索引（下游只见索引）
        if let Some(Opsize::Named(n)) = &enc.opsize {
            let names = inst.ops.as_deref().unwrap_or(&[]);
            let idx = names
                .iter()
                .position(|e| e.split(':').next().map(str::trim) == Some(n.as_str()))
                .ok_or_else(|| {
                    format!(
                        "[[instructions.{}]].opsize: '{n}' 不是已声明的操作数名（ops: {}）",
                        inst.name,
                        names.join(", ")
                    )
                })?;
            enc.opsize = Some(Opsize::Slot(idx as u16));
        }
        // `opsize = "out"` → 唯一 out/inout 角色的 Reg 操作数索引（下游只见 Slot）
        if let Some(Opsize::Out) = &enc.opsize {
            let mut out_idxs: Vec<usize> = Vec::new();
            for (i, u) in uses.iter().enumerate() {
                if !matches!(u.role, Some(OperandRole::Out) | Some(OperandRole::InOut)) {
                    continue;
                }
                let is_reg = m
                    .operand_slots
                    .iter()
                    .find(|s| s.name == u.slot)
                    .map(|s| s.kind == OperandKind::Reg)
                    .unwrap_or(false);
                if is_reg {
                    out_idxs.push(i);
                }
            }
            match out_idxs.len() {
                1 => enc.opsize = Some(Opsize::Slot(out_idxs[0] as u16)),
                0 => {
                    return Err(format!(
                        "[[instructions.{}]].opsize = \"out\": 没有 out/inout 的 Reg 操作数（改用 \"max\" 或显式操作数名）",
                        inst.name
                    ));
                }
                n => {
                    return Err(format!(
                        "[[instructions.{}]].opsize = \"out\": {n} 个 out/inout 操作数，无法唯一确定（显式写操作数名）",
                        inst.name
                    ));
                }
            }
        }
        let form = &enc;
        // 变长（无 opcode_field）不需要 operand_fields；定宽需要
        if form.opcode_field.is_none() && form.operand_fields.is_some() {
            return Err(format!(
                "[[instructions.{}]]: operand_fields declared without opcode_field (vlen 形式用 modrm 语义键)",
                inst.name
            ));
        }
        if inst.opcode.is_none() && form.opcode_reg.is_none() {
            return Err(format!(
                "[[instructions.{}]]: opcode required (or form opcode_reg for +r forms)",
                inst.name
            ));
        }
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
                        inst.name, op.slot, asm
                    )
                })?;
            // ① **编码键名**（`fname`）：定宽 → 位域名（`operand_fields[i]`，查
            // `[conventions.bitfields]` 用）；变长 → 语义角色名（dest/src/cond/mem/imm/
            // target，查 modrm 角色与立即数编码表用）。**只用于编码**，改名会影响编码。
            let fname = if fixed {
                of.and_then(|f| f.get(i)).cloned().ok_or_else(|| {
                    format!(
                        "[[instructions.{}]]: operand {} missing operand_fields entry",
                        inst.name, i
                    )
                })?
            } else {
                let mut name = semantic_operand_name(op, slot, i);
                // 重名去重：src→src2/src3、dest→dest2、imm→imm2 …
                let mut n = 2;
                while operands.iter().any(|(f, _, _, _)| f == &name) {
                    name = format!("{}{}", semantic_operand_name(op, slot, i), n);
                    n += 1;
                }
                name
            };
            // ② **Rust 字段名**（`fid`）：**就是 `ops` 里作者声明的名字**（v18 S7d）。
            // 定宽 ISA 不再拿位域名（rd/rs1）冒充字段名，变长 ISA 也不再用
            // dest/src 之类语义名覆盖作者写下的 dst/src。
            let fid = operand_field_ident(&op.name, &inst.name, i)?;
            let role = op.role.unwrap_or(OperandRole::In);
            operands.push((fname, fid, slot, role));
        }
        out.push(InstInfo {
            // asm 规范化：命名形态的 `{dst}` 已换成 `{1}`，下游（asm/machine/
            // decode 生成）只认索引形态，无需感知命名。
            inst: Instruction {
                asm: norm_asm,
                ..inst.clone()
            },
            form: enc.clone(),
            vn: pascal_ident(&inst.name),
            operands,
        });
    }
    Ok(out)
}

/// 从 asm 模板解析：首词 = 助记符；操作数占位符 `{i:[槽:角色]}` 内联声明。
/// 返回 (助记符, 操作数声明表，按序号排序且连续)。
/// validate.rs 也调用本函数做操作数校验（槽存在/角色合法/序号连续）。
/// 解析 `ops = ["名字:槽[:角色]", …]`：返回操作数用法表（含**声明名**），序即编码序。
fn parse_ops_list(ops: &[String], inst_name: &str) -> Result<Vec<OperandUse>, String> {
    let ctx = || format!("[[instructions.{inst_name}]].ops");
    if ops.is_empty() {
        return Err(format!("{}: 不能为空数组（省略该键即可）", ctx()));
    }
    let mut uses: Vec<OperandUse> = Vec::with_capacity(ops.len());
    for (i, entry) in ops.iter().enumerate() {
        let mut parts = entry.split(':').map(str::trim);
        let name = parts.next().unwrap_or("");
        let slot = parts.next().unwrap_or("");
        let role = parts.next();
        if parts.next().is_some() {
            return Err(format!(
                "{}[{i}] '{entry}': 形如 \"名字:槽\" 或 \"名字:槽:角色\"",
                ctx()
            ));
        }
        if name.is_empty() || slot.is_empty() {
            return Err(format!("{}[{i}] '{entry}': 名字与槽都不能为空", ctx()));
        }
        if uses.iter().any(|u| u.name == name) {
            return Err(format!("{}[{i}]: 操作数名 '{name}' 重复", ctx()));
        }
        let role = match role {
            None | Some("in") => OperandRole::In,
            Some("out") => OperandRole::Out,
            Some("inout") => OperandRole::InOut,
            Some(other) => {
                return Err(format!(
                    "{}[{i}]: 角色 '{other}' 非法（in/out/inout）",
                    ctx()
                ));
            }
        };
        uses.push(OperandUse {
            name: name.to_string(),
            slot: slot.to_string(),
            role: Some(role),
        });
    }
    Ok(uses)
}

/// 命名 asm 模板 → 索引形态：`{dst}` → `{1}`（按 `ops` 声明序）。
///
/// 规范化让下游（asm/machine/decode 生成）只认索引，完全不必感知命名——
/// 命名只是**作者面**的语法。未知名字在此报错（拼错的占位符不会静默变字面量）。
///
/// `variant_params` = `[meta].variants` 声明的参数名（v19 V5）：模板里写了 `{参数名}`
/// 却**本次没传值**时，这里给出可操作的错误——参数占位符由投影期替换
/// （`validate::apply_variants` 在校验之前跑），走到这里还剩参数占位符 = 没传参。
/// 默认档不许留参数占位符（否则生成的汇编里会打印出字面 `{width}`）。
fn normalize_named_template(
    rest: &str,
    names: &[String],
    inst_name: &str,
    variant_params: &BTreeSet<String>,
) -> Result<String, String> {
    let ctx = || format!("[[instructions.{inst_name}]] asm");
    let mut out = String::with_capacity(rest.len());
    let mut chars = rest.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '{' {
            out.push(c);
            continue;
        }
        let Some(end) = rest[i..].find('}') else {
            return Err(format!("{}: 未闭合的 '{{'", ctx()));
        };
        let inner = rest[i + 1..i + end].trim();
        let idx = names.iter().position(|n| n == inner).ok_or_else(|| {
            if variant_params.contains(inner) {
                format!(
                    "{}: 占位符 '{{{inner}}}' 是**变体参数**（`[meta].variants`）但本次没有传值——\
                     参数化模板必须显式传参（CLI `--params {inner}=<取值>`、\
                     宏 `params = {{ {inner} = <取值> }}`）；默认档不许留参数占位符",
                    ctx()
                )
            } else {
                format!(
                    "{}: 占位符 '{{{inner}}}' 不是已声明的操作数名（ops: {}）",
                    ctx(),
                    names.join(", ")
                )
            }
        })?;
        out.push_str(&format!("{{{idx}}}"));
        for _ in 0..end - 1 {
            chars.next();
        }
        chars.next(); // '}'
    }
    Ok(out)
}

/// 解析指令的 asm 模板：`ops` 给声明，`asm` 只引用名字。
///
/// 返回 (操作数用法表, **规范化后的 asm**)。规范化 = 把 `{名字}` 换成
/// `{序号}`，于是下游（asm/machine/decode 生成）只认索引，完全不必感知命名——
/// 命名只是作者面的语法。v17 起**不再拆分「助记符」**：整条 asm（前导字面 +
/// 操作数）原样规范化，助记符只是前导字面段，不再是独立分派键。v14 的内联
/// 声明（`{i:[槽:角色]}`）已删除。
pub(crate) fn parse_asm_decl(
    asm: &str,
    ops: Option<&[String]>,
    inst_name: &str,
    variant_params: &BTreeSet<String>,
) -> Result<(Vec<OperandUse>, String), String> {
    let ctx = || format!("[[instructions.{inst_name}]] asm");
    if asm.trim().is_empty() {
        return Err(format!("{}: asm must not be empty", ctx()));
    }
    let Some(list) = ops else {
        // 无操作数指令（ret/nop/…）不需要 ops；有占位符却没 ops = 缺声明
        if asm.contains('{') {
            return Err(format!(
                "{}: asm 引用了操作数但没有 `ops` 声明（v15：声明写在 ops，模板只引用名字）",
                ctx()
            ));
        }
        return Ok((Vec::new(), asm.to_string()));
    };
    let uses = parse_ops_list(list, inst_name)?;
    let names: Vec<String> = uses.iter().map(|u| u.name.clone()).collect();
    let norm_asm = normalize_named_template(asm, &names, inst_name, variant_params)?;
    let segs = asm::parse_template(&norm_asm)?;
    for (n, name) in names.iter().enumerate() {
        let refs = segs
            .iter()
            .filter(|s| matches!(s, asm::Seg::Op(k) if *k == n))
            .count();
        if refs > 1 {
            return Err(format!(
                "{}: 操作数 '{name}' 在模板里被引用 {refs} 次（歧义）",
                ctx()
            ));
        }
    }
    Ok((uses, norm_asm))
}

// ─────────────────────────────── 寄存器表 ───────────────────────────────

/// 每使用到的寄存器组生成 `{group}_name(i)` 与 `__parse_reg_{group}(s)`；
/// 存在多态 Reg 槽（class=None）时另生成 `__parse_reg_any`（任意 GPR 名）。
fn gen_reg_tables(model: &IsaModel) -> Result<TokenStream, String> {
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
        // 别名（`aliases = { a0 = 10 }`）：**解析认、渲染不认**——反汇编仍出主名。
        // 下标已由 `validate_regs` 校验过（越界/重名在那里报错）。
        if let Some(aliases) = &g.aliases {
            for (alias, idx) in aliases {
                let Some(name) = names.get(*idx as usize) else {
                    continue;
                };
                let reg_lit = syn::Ident::new(name, proc_macro2::Span::call_site());
                from_str.push(match case_insensitive_regs {
                    true => {
                        let alias = alias.to_lowercase();
                        quote! { #alias => Ok(Reg::#reg_lit) }
                    }
                    false => quote! { #alias => Ok(Reg::#reg_lit) },
                });
            }
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

fn gen_encode(infos: &[InstInfo], m: &IsaModel) -> Result<TokenStream, String> {
    let little = m.meta.endian == Endian::Little;
    // 指令字长（字节，= ceil(位宽/8)）——**逐指令**取 ISA 数据（v18 S4：fixed 全 ISA
    // 同一个字长、mixed 逐指令 `width`），**无宽度白名单/上限**。
    // 字表示为**字节数组**（`[u8; n]`，LE 位序：bit 0 = 第 0 字节 LSB），位域写入
    // 走生成的 `__place` 助手；因此字长不受 u64/u128 限制（任意位宽，含非 8 倍数）。
    // 大端 ISA：内存序 = 字节数组反转（bit 0 落在最后一个字节的 LSB）。
    let out = if little {
        quote! { Ok(__word.to_vec()) }
    } else {
        quote! { Ok({ let mut __out = __word.to_vec(); __out.reverse(); __out }) }
    };
    let mut arms = Vec::new();
    for info in infos {
        let vn = &info.vn;
        let inst_len_lit = proc_macro2::Literal::u32_unsuffixed(m.inst_width_bytes(&info.inst)?);
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
        for (fname, fid, slot, _) in &info.operands {
            let bf = get_bf(m, fname)?;
            // P0-16：立即数槽 encode 前范围检查——原实现 `value & mask`
            // 静默截断溢出（riscv imm12 传 -3000 → 掩码后错值，大帧栈错位）。
            // 判据（重定位指令 / 预移位散布位域 / `prefix_scan` 一律不检查）与
            // 生成期自测 `__spec_tests` 共享 [`spec::imm_encode_checked`]——
            // 同一份规则不在两处各写一遍（自测不做假保证）。
            if let Some((lo, hi)) = spec::imm_encode_checked(m, info, slot, fname) {
                stmts.push(quote! {
                    let __v = *#fid as i64;
                    if !(#lo..=#hi).contains(&__v) {
                        return Err(format!(
                            "{}: immediate {} out of range [{}, {}]",
                            stringify!(#vn), __v, #lo, #hi
                        ));
                    }
                });
            }
            // 多字段落点（`encode` + `fields`）：一个值摊到多个位域。值域检查由方案负责
            // （`slice` 是纯切片；`logical_imm` 走下面的非线性编码）。
            if let (Some(enc), Some(sfields)) = (slot.encode, &slot.fields) {
                match enc {
                    SlotEncode::Slice => {
                        stmts.push(quote! {
                            let __mv = *#fid as u64;
                        });
                        let mut sh: u32 = 0;
                        for sname in sfields {
                            let sbf = single_field(m, sname, "fields")?;
                            let (_so, sw) = single(sbf);
                            let smask = if sw >= 64 { u64::MAX } else { (1u64 << sw) - 1 };
                            let shl = proc_macro2::Literal::u32_unsuffixed(sh);
                            let smask = proc_macro2::Literal::u64_unsuffixed(smask);
                            stmts.extend(place_ts(quote! { (__mv >> #shl) & #smask }, sbf));
                            sh += sw;
                        }
                    }
                    SlotEncode::LogicalImm => {
                        let width = proc_macro2::Literal::u32_unsuffixed(slot.width.unwrap_or(0));
                        stmts.push(quote! {
                            let (__lin, __lir, __lis) =
                                match __encode_logical_imm(*#fid as u64, #width) {
                                    Some(t) => t,
                                    None => return Err(format!(
                                        "{}: 立即数 {:#x} 不是 {}-bit 的合法逻辑立即数（要求\"一段连续 1 循环填充\"）",
                                        stringify!(#vn), *#fid as u64, #width
                                    )),
                                };
                        });
                        for (i, sname) in sfields.iter().enumerate() {
                            let sbf = single_field(m, sname, "fields")?;
                            let v = match i {
                                0 => quote! { __lin },
                                1 => quote! { __lir },
                                _ => quote! { __lis },
                            };
                            stmts.extend(place_ts(v, sbf));
                        }
                    }
                }
                continue;
            }
            stmts.extend(place_ts(
                if slot.kind == OperandKind::Reg {
                    // 多宽度视图组（base_index=0 共享物理号）时 Reg 枚举判别值
                    // 是全局序号，不是物理编号——编码必须用 to_index()
                    //（如 X5/W5 判别值不同但物理号同为 5）
                    quote! { <Reg as forge_ir::PhysReg>::to_index(*#fid) as u64 }
                } else if slot.unit() > 1 {
                    // 源值单位（`unit`）：字段值 = 源值 / unit。
                    //
                    // **对齐检查只对 imm 槽做**：汇编期的文本路径（`__imm`/`__label`）已经
                    // 拒掉非整数倍，这里是防"直接构造 Inst"的兜底；而 label 槽在编译器
                    // 路径上塞的是**标签 id 占位**（`LabelRef::EPILOGUE.id()`，不是字节偏移），
                    // 随后由 reloc patcher 整体改写——对它查对齐只会误报。
                    let sh = proc_macro2::Literal::u32_unsuffixed(slot.unit_shift());
                    let unit_lit = proc_macro2::Literal::i64_suffixed(slot.unit());
                    if slot.kind == OperandKind::Imm {
                        quote! {{
                            let __raw = *#fid as i64;
                            if __raw % #unit_lit != 0 {
                                return Err(format!(
                                    "{}: immediate {} 不是 {} 的整数倍（该槽以字节为单位：{} 字节 = 1 个字段单位）",
                                    stringify!(#vn), __raw, #unit_lit, #unit_lit
                                ));
                            }
                            (__raw >> #sh) as u64
                        }}
                    } else {
                        quote! { ((*#fid as i64) >> #sh) as u64 }
                    }
                } else {
                    quote! { *#fid as u64 }
                },
                bf,
            ));
        }
        // 未覆盖位域天然为 0（__word 初始 0）——无需显式置零
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
        let out = out.clone();
        arms.push(quote! {
            #pat => {
                let mut __word = [0u8; #inst_len_lit as usize];
                #(#stmts)*
                #out
            }
        });
    }
    Ok(quote! {
        /// 编码单条指令为字节（定宽：`ceil([encoding].bits / 8)` 字节，
        /// 字长任意；`[meta].endian` 决定内存字节序）。
        pub fn encode(inst: &Inst) -> Result<Vec<u8>, String> {
            match inst {
                #(#arms,)*
                Inst::Raw(bytes) => Ok(bytes.clone()),
            }
        }
    })
}

/// **命名位集合**助手（`kind = "bits"` 的槽才要）。
///
/// 源文本是一个 ident，由表里若干**名字拼接**而成（RISC-V `fence` 的 `iorw` = i|o|r|w）；
/// 编码值 = 各位的**按位或**。解析按**最长匹配优先**贪心（表里若同时有 `rw` 与 `r`，
/// 先吃长的），整串必须吃干净才算命中——拼错一个字母就不匹配，不静默当空集。
/// 渲染反向：按表的名字**字典序**把置位的名字拼起来（确定性，与 `cond` 的"同码取字母序
/// 最小名"同一口径）。表是**数据**（`[conventions.bitsets.<table>]`），DSL 不认识任何名字。
fn gen_bitset_helpers() -> TokenStream {
    quote! {
        /// 命名位集合解析：贪心最长匹配拼名，返回按位或；整串吃不干净 ⇒ `None`。
        #[allow(dead_code)]
        fn __bitset(it: &mut __Iter, names: &[(&str, u64)]) -> Option<i64> {
            let Some(__Tok::Ident(text)) = it.toks.get(it.pos) else {
                return None;
            };
            let mut rest: &str = text;
            let mut val: u64 = 0;
            while !rest.is_empty() {
                let hit = names
                    .iter()
                    .filter(|(n, _)| !n.is_empty() && rest.starts_with(*n))
                    .max_by_key(|(n, _)| n.len());
                match hit {
                    Some((n, b)) => {
                        val |= *b;
                        rest = &rest[n.len()..];
                    }
                    None => return None,
                }
            }
            it.pos += 1;
            Some(val as i64)
        }

        /// 命名位集合渲染：按表的名字**字典序**拼出置位的名字（全 0 ⇒ 空串）。
        #[allow(dead_code)]
        fn __render_bitset(v: i64, names: &[(&str, u64)]) -> String {
            let mut out = String::new();
            for (n, b) in names {
                if *b != 0 && (v as u64) & *b == *b {
                    out.push_str(n);
                }
            }
            out
        }
    }
}

/// **逻辑立即数**助手（`encode = "logical_imm"`，每个用到它的模块生成一次）。
///
/// ARM 系逻辑运算的立即数不是任意值：它是"一段连续 1 循环填充整个宽度"的位模式，编码成
/// `N`(1) / `immr`(6) / `imms`(6) 三个分量（元素尺寸 + 旋转量 + 1 的个数）。这是**非线性**
/// 映射（值 → 分量），所以它不走位切片，而由这两个助手实现；谱只声明"这个槽用这套方案、
/// 三个分量落到哪些位域"。
///
/// 算法（与 ARM ARM 的 `DecodeBitMasks` 互为逆）：
/// ① 找**最小的重复元素**尺寸 `esize = 2^len`（值在 `esize` 上循环）；
/// ② 把元素里那段 1 旋到最低位，检查它确实是**连续一段**（否则不可编码）；
/// ③ `immr` = 反向的旋转量、`imms` = 元素尺寸与 1 的个数按规范拼进 6 位。
///
/// `width` ∈ {32, 64}（32 位形式 `N` 恒 0）。
fn gen_logic_imm_helpers() -> TokenStream {
    quote! {
        /// 值的位模式按 `width` 位循环右移。
        #[inline]
        fn __ror(v: u64, r: u32, width: u32) -> u64 {
            let r = r % width;
            let mask: u64 = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
            if r == 0 { v & mask } else { ((v >> r) | (v << (width - r))) & mask }
        }

        /// 逻辑立即数编码：值 → `(N, immr, imms)`；不是合法位掩码 ⇒ `None`。
        fn __encode_logical_imm(value: u64, width: u32) -> Option<(u64, u64, u64)> {
            let mask: u64 = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
            let v = value & mask;
            // 全 0 / 全 1 没有"一段 1"可分（它们由 MOV 而不是逻辑立即数表达）。
            if v == 0 || v == mask { return None; }
            // ① 最小重复元素尺寸：两半不同 ⇒ 元素 = 2 × 半宽。
            let mut size = width;
            loop {
                size /= 2;
                let em: u64 = if size >= 64 { u64::MAX } else { (1u64 << size) - 1 };
                if (v & em) != ((v >> size) & em) { size *= 2; break; }
                if size <= 2 { break; }
            }
            let em: u64 = if size >= 64 { u64::MAX } else { (1u64 << size) - 1 };
            let pat = v & em;
            let ones = pat.count_ones();
            if ones == 0 || ones == size { return None; }
            // ② 元素内必须是一段**连续的 1**（可环绕元素边界）。
            let rot = pat.trailing_zeros() % size;
            let rotated = __ror(pat, rot, size);
            if rotated != (1u64 << ones) - 1 { return None; }
            // ③ 分量：`immr` 是把那段 1 转回原位所需的右旋量；
            //    `imms` = 元素尺寸取反左移一位（低 6 位）| (1 的个数 - 1)。
            let immr = (size - rot) % size;
            let len = size.trailing_zeros();
            let n = if len == 6 { 1u64 } else { 0 };
            let sz = size as u64;
            let imms = (((!(sz - 1)) << 1) & 0x3F) | (ones as u64 - 1);
            Some((n, immr as u64, imms))
        }

        /// 逻辑立即数解码：`(N, immr, imms)` → 值的位模式（`width` 位）。
        fn __decode_logical_imm(n: u64, immr: u64, imms: u64, width: u32) -> u64 {
            // `len` = `N:NOT(imms)` 的最高位；元素尺寸 = 2^len。
            let t = ((n & 1) << 6) | ((!imms) & 0x3F);
            let len = 63 - t.leading_zeros();
            let size = 1u32 << len;
            let levels: u64 = if len >= 6 { 0x3F } else { (1u64 << len) - 1 };
            let ones = (imms & levels) as u32 + 1;
            let welem: u64 = if ones >= 64 { u64::MAX } else { (1u64 << ones) - 1 };
            let elem = __ror(welem, (immr & 0x3F) as u32, size);
            if width <= size { return elem; }
            // 元素循环填充到整宽。
            let mut out = 0u64;
            let mut sh = 0u32;
            while sh < width {
                out |= elem << sh;
                sh += size;
            }
            out
        }
    }
}

/// 定宽字位域助手（每个定宽模块生成一次）：/// `__word: [u8; n]` 是 LE 位序的字（bit 0 = 第 0 字节 LSB），字长任意。
/// `__place` 写 [off, off+width)、`__bits` 读 [off, off+width)（width ≤ 64）。
/// 用 u128 中间量覆盖"跨字节 + 非字节对齐"的位段（shift ≤ 7，width ≤ 64 → ≤ 71 位）。
fn gen_bit_helpers() -> TokenStream {
    quote! {
        /// 位域写入：`value` 的低 `width` 位写到字的 [off, off+width)。
        /// `width ≤ 64`（位域值是 u64/i64）；字长任意（不越界——位域由 validate
        /// 约束在字内）。
        #[inline]
        fn __place(word: &mut [u8], value: u64, off: usize, width: u32) {
            let mask: u64 = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
            let shifted = ((value & mask) as u128) << (off % 8);
            let start = off / 8;
            let nbytes = ((off % 8) + width as usize).div_ceil(8);
            let mut i = 0usize;
            while i < nbytes {
                word[start + i] |= (shifted >> (8 * i)) as u8;
                i += 1;
            }
        }

        /// 位域读取：字的 [off, off+width) 值（`width ≤ 64`；字长任意）。
        #[inline]
        fn __bits(word: &[u8], off: usize, width: u32) -> u64 {
            let start = off / 8;
            let shift = (off % 8) as u32;
            let nbytes = ((off % 8) + width as usize).div_ceil(8);
            let mut acc: u128 = 0;
            let mut i = 0usize;
            while i < nbytes {
                acc |= (word[start + i] as u128) << (8 * i);
                i += 1;
            }
            let mask: u64 = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
            ((acc >> shift) as u64) & mask
        }
    }
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
                        "ISA-DSL decode bit-trie: node {ni} edges [{i}](bit {b1} w {w1} v {v1:#x}) and [{j}](bit {b2} w {w2} v {v2:#x}) overlap at bits [{lo},{hi}) — ambiguous; make the encodings disjoint"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// 将位 trie 展开：**叶 arm 先试，随后按声明序逐条试边**。
///
/// 边之间**必须平铺成互不嵌套的 `if`，不能写成 `else if` 链**：同一节点上两条边的位区间
/// **可以不相交却同时匹配同一个字**（例：A64 逻辑族 `shift` 在 [22,24)、`bics` 的 N 位在
/// [21,22)——`and …` 的移位边与 `bics` 的 N 边能同时命中）。`else if` 只在**前一条边的
/// 条件为假**时才试下一条，于是"前一条边的子树匹配上了条件、但叶子 guard 没过而返回不了"
/// 会直接跳出整条链，把后面的边变成**死代码**——实测 `bics x0, x1, x2` 编得出、解不回
/// （同字能被 `ands` 的移位边接住，`bics` 那条永远到不了）。平铺则"试一条、没返回就试下一条"，
/// 与定宽 decode 的文档语义（叶优先 + 声明序）一致。
///
/// 与变长（字节）decode 的 `emit_dec_trie` 不同：那里同一节点的掩码由
/// `check_dec_trie_overlaps` 保证**互斥**，`else if` 链既安全又完整。
fn emit_bit_trie(nodes: &[BitTrieNode], idx: usize) -> TokenStream {
    let node = &nodes[idx];
    let mut body = TokenStream::new();
    for arm in &node.arms {
        body.extend(arm.clone());
    }
    for (bit, w, value, child) in &node.edges {
        let sub = emit_bit_trie(nodes, *child);
        body.extend(quote! {
            if __bits(&__word, #bit as usize, #w) == #value {
                #sub
            }
        });
    }
    body
}

/// 一个"宽度组"的解码体：`(读字表达式, 分派语句)`。
///
/// 组 = 字长相同的全部指令（fixed ISA 只有一组；mixed 每个 `widths` 一项一组）。
/// 组内建**位级 trie**（常量位段），叶子里做补集零 guard + 字段提取。
fn gen_decode_group(
    infos: &[&InstInfo],
    m: &IsaModel,
    inst_bytes: u32,
) -> Result<(TokenStream, TokenStream), String> {
    let little = m.meta.endian == Endian::Little;
    let inst_len_lit = proc_macro2::Literal::u32_unsuffixed(inst_bytes);
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
        // 操作数位域（其位不被零 guard 约束）——**多字段落点要把该槽的每个字段都算上**：
        // 只算 `operand_fields` 里那一个（首字段）会把其余字段（如 `tbz` 的 `b5`[31]）
        // 误当"保留位"，于是 guard 要求它们为 0，而它们恰恰是操作数的一部分（实测
        // `TBZX[bit=63]` ⇒ bit31 = 1 ⇒ 解不回）。
        for (fname, _, slot, _) in &info.operands {
            match (&slot.encode, &slot.fields) {
                (Some(_), Some(sfields)) => {
                    for sname in sfields {
                        covered_ranges.extend(bf_ranges(get_bf(m, sname)?));
                    }
                }
                _ => covered_ranges.extend(bf_ranges(get_bf(m, fname)?)),
            }
        }
        // 补集零 guard：**按字节**生成（字长任意，不能用 u64 掩码）——
        // 未覆盖的位（含非 8 倍数位宽在末字节的填充位）必须为 0。
        let guard = complement_zero_guard(&covered_ranges, inst_bytes);
        // 字段提取
        let mut binds: Vec<TokenStream> = Vec::new();
        let mut ctor_fields: Vec<TokenStream> = Vec::new();
        for (fname, fid, slot, _) in &info.operands {
            let bf = get_bf(m, fname)?;
            let raw = extract_ts(bf);
            // 多字段落点（`encode` + `fields`）：把各段拼回源值（与编码侧对称）。
            if let (Some(enc), Some(sfields)) = (slot.encode, &slot.fields) {
                let signed = slot.signed.unwrap_or(false) || slot.kind == OperandKind::Label;
                let expr: TokenStream = match enc {
                    SlotEncode::Slice => {
                        let mut parts: Vec<TokenStream> = Vec::new();
                        let mut sh: u32 = 0;
                        for sname in sfields {
                            let sbf = get_bf(m, sname)?;
                            let piece = extract_ts(sbf);
                            let shl = proc_macro2::Literal::u32_unsuffixed(sh);
                            parts.push(quote! { ((#piece) << #shl) });
                            sh += single(sbf).1;
                        }
                        let combined = quote! { (#(#parts)|*) };
                        if signed {
                            let w = slot.width.unwrap_or(64);
                            sign_extend_ts(quote! { (#combined) as u64 }, w)
                        } else {
                            quote! { (#combined) as i64 }
                        }
                    }
                    SlotEncode::LogicalImm => {
                        let width = proc_macro2::Literal::u32_unsuffixed(slot.width.unwrap_or(0));
                        let mut pieces: Vec<TokenStream> = Vec::new();
                        for sname in sfields {
                            pieces.push(extract_ts(get_bf(m, sname)?));
                        }
                        quote! {
                            __decode_logical_imm(#(#pieces),*, #width) as i64
                        }
                    }
                };
                binds.push(quote! { let #fid = #expr; });
                ctor_fields.push(quote! { #fid });
                continue;
            }
            let expr: TokenStream = match slot.kind {
                OperandKind::Reg => field_ctor_expr(slot, quote! { #raw as u32 }),
                // 条件码槽在 `Inst` 里是 `u8`（与编码侧 `*fid as u64` 对称）——
                // 定宽 ISA 的第一个 cond 槽用例（arm64 `cond4`，v18 S3c）。
                OperandKind::Cond => quote! { #raw as u8 },
                _ => {
                    let signed = slot.signed.unwrap_or(false) || slot.kind == OperandKind::Label;
                    let base = if signed {
                        let w = slot.width.unwrap_or(64);
                        sign_extend_ts(raw, w)
                    } else {
                        quote! { #raw as i64 }
                    };
                    // 源值单位（`unit`）：`Inst` 字段是**源单位**（字节），字段值乘回去
                    // （A64 `imm26 = 7` ⇒ 目标 = 28 字节），`disassemble` 因此照旧闭合。
                    if slot.unit() > 1 {
                        let sh = proc_macro2::Literal::u32_unsuffixed(slot.unit_shift());
                        quote! { (#base) << #sh }
                    } else {
                        base
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
        let ctor_len = inst_len_lit.clone();
        groups.push((
            key,
            quote! {
                if #guard {
                    #(#binds)*
                    return Some((#ctor, #ctor_len as usize));
                }
            },
        ));
    }
    let nodes = build_bit_trie(&groups);
    check_bit_trie_overlaps(&nodes)?;
    let dispatch = emit_bit_trie(&nodes, 0);
    let read = if little {
        quote! { bytes[..#inst_len_lit as usize].to_vec() }
    } else {
        quote! {{
            let mut __be = bytes[..#inst_len_lit as usize].to_vec();
            __be.reverse();
            __be
        }}
    };
    Ok((read, dispatch))
}

/// `decode` 的生成：`fixed` 单组；`mixed` 按 `widths` 升序分组，**首个完整匹配即停**。
fn gen_decode(infos: &[InstInfo], m: &IsaModel) -> Result<TokenStream, String> {
    match m.encoding.kind {
        EncodingKind::Fixed => {
            // 指令字长（字节）：ISA 数据（`[encoding].bits`），**无白名单/上限**。
            // 字表示为字节数组（LE 位序），各 arm 的常量位段比较与字段提取都走生成的
            // `__bits` 助手——字长不受 u64/u128 限制。
            let inst_bytes = m.inst_bytes()?;
            let inst_len_lit = proc_macro2::Literal::u32_unsuffixed(inst_bytes);
            let all: Vec<&InstInfo> = infos.iter().collect();
            let (read, dispatch) = gen_decode_group(&all, m, inst_bytes)?;
            Ok(quote! {
                /// 解码定宽指令字（`ceil([encoding].bits / 8)` 字节，字长任意）；
                /// 常量位段位级决策树（叶节点优先，叶内声明序），无匹配 → None。
                /// 返回 (指令, 消费字节数) = 字长的字节数。
                pub fn decode(bytes: &[u8]) -> Option<(Inst, usize)> {
                    if bytes.len() < #inst_len_lit as usize {
                        return None;
                    }
                    let __word: Vec<u8> = #read;
                    #dispatch
                    None
                }

                /// 解码并报告部分匹配偏移：定宽 ISA 失败 → Err(bytes.len() 不足 ? len : 0)。
                /// 供 `TargetDecoder::decode` 的 `DecodeError::InvalidBytes(n)` 使用。
                pub fn decode_partial(bytes: &[u8]) -> Result<(Inst, usize), usize> {
                    match decode(bytes) {
                        Some(r) => Ok(r),
                        None => Err(if bytes.len() < #inst_len_lit as usize {
                            bytes.len()
                        } else {
                            0
                        }),
                    }
                }
            })
        }
        EncodingKind::Mixed => {
            // 按字长分组：同一字长的指令一个 trie；`decode` 按 widths **升序**依次尝试
            //（16 位指令在前——RVC/Thumb 类短编码优先），首个完整匹配即停。
            let mut widths: Vec<u32> = Vec::new();
            for info in infos {
                let w = m.inst_width_bytes(&info.inst)?;
                if !widths.contains(&w) {
                    widths.push(w);
                }
            }
            widths.sort_unstable();
            let mut fns: Vec<TokenStream> = Vec::new();
            let mut calls: Vec<TokenStream> = Vec::new();
            for w in &widths {
                let subset: Vec<&InstInfo> = infos
                    .iter()
                    .filter(|i| m.inst_width_bytes(&i.inst) == Ok(*w))
                    .collect();
                let (read, dispatch) = gen_decode_group(&subset, m, *w)?;
                let fn_name = format_ident!("__decode_w{}", w);
                let len_lit = proc_macro2::Literal::u32_unsuffixed(*w);
                fns.push(quote! {
                    fn #fn_name(bytes: &[u8]) -> Option<(Inst, usize)> {
                        if bytes.len() < #len_lit as usize {
                            return None;
                        }
                        let __word: Vec<u8> = #read;
                        #dispatch
                        None
                    }
                });
                calls.push(quote! {
                    if let Some(__r) = #fn_name(bytes) {
                        return Some(__r);
                    }
                });
            }
            let min_w = *widths.first().unwrap_or(&1);
            let min_lit = proc_macro2::Literal::u32_unsuffixed(min_w);
            Ok(quote! {
                #(#fns)*

                /// 解码混合字长指令字（v18 S4）：按 `[encoding].widths` **升序**尝试，
                /// 首个完整匹配即停。返回 (指令, 消费字节数 = 该指令字长)。
                pub fn decode(bytes: &[u8]) -> Option<(Inst, usize)> {
                    #(#calls)*
                    None
                }

                /// 解码并报告部分匹配偏移：短于**最短**字长 ⇒ 截断（Err(len)），否则 0。
                pub fn decode_partial(bytes: &[u8]) -> Result<(Inst, usize), usize> {
                    match decode(bytes) {
                        Some(r) => Ok(r),
                        None => Err(if bytes.len() < #min_lit as usize {
                            bytes.len()
                        } else {
                            0
                        }),
                    }
                }
            })
        }
        EncodingKind::PrefixScan => Err(format!(
            "prefix_scan ISA 走变长解码路径（widths={:?} 不适用）",
            m.encoding.widths
        )),
    }
}

/// 补集零 guard：未覆盖位必须为 0——按字节生成 `(__word[i] & mask) == 0` 合取
/// （字长任意，不能用 u64 掩码；非 8 倍数位宽的末字节填充位天然未覆盖 → 必须为 0）。
fn complement_zero_guard(covered: &[(u32, u32)], inst_bytes: u32) -> TokenStream {
    let mut uncovered = vec![0xFFu8; inst_bytes as usize];
    for (s, e) in covered {
        for bit in *s..*e {
            let byte = (bit / 8) as usize;
            if byte < uncovered.len() {
                uncovered[byte] &= !(1u8 << (bit % 8));
            }
        }
    }
    let terms: Vec<TokenStream> = uncovered
        .iter()
        .enumerate()
        .filter(|(_, m)| **m != 0)
        .map(|(i, m)| quote! { (__word[#i] & #m) == 0 })
        .collect();
    if terms.is_empty() {
        quote! { true }
    } else {
        quote! { #(#terms)&&* }
    }
}

// ─────────────────────────────── 位域助手 ───────────────────────────────

fn get_bf<'a>(m: &'a IsaModel, name: &str) -> Result<&'a Bitfield, String> {
    m.conventions
        .bitfields
        .get(name)
        .ok_or_else(|| format!("bitfield '{name}' not declared in [conventions.bitfields]"))
}

/// 单一位段（散布位段在此报错——迭代 2 中 opcode/fields/隐式 0 不支持散布）。
fn single_field<'a>(m: &'a IsaModel, name: &str, ctx: &str) -> Result<&'a Bitfield, String> {
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

/// encode：`__place(&mut __word, value, offset, width)`（单）或逐 piece
/// `__place(&mut __word, value >> shift, offset, width)`。
/// 字是字节数组（字长任意），位段跨字节/非字节对齐由 `__place` 处理。
fn place_ts(value: TokenStream, bf: &Bitfield) -> Vec<TokenStream> {
    match &bf.pieces {
        None => {
            let (off, w) = single(bf);
            vec![quote! { __place(&mut __word, #value, #off as usize, #w); }]
        }
        Some(ps) => ps
            .iter()
            .map(|p| {
                let sh = p.shift;
                let off = p.offset;
                let w = p.width;
                quote! { __place(&mut __word, (#value >> #sh), #off as usize, #w); }
            })
            .collect(),
    }
}

/// decode：字段原始提取（u64）→ 整体加括号（防 `|` 与后续 `<<`/`as` 优先级问题）。
/// 单 → `__bits(&__word, off, w)`；散布 → pieces OR 累加。
/// 字是字节数组（字长任意），故统一走 `__bits` 助手而非 `__w >> off`。
fn extract_ts(bf: &Bitfield) -> TokenStream {
    match &bf.pieces {
        None => {
            let (off, w) = single(bf);
            quote! { (__bits(&__word, #off as usize, #w)) }
        }
        Some(ps) => {
            let parts: Vec<_> = ps
                .iter()
                .map(|p| {
                    let sh = p.shift;
                    let off = p.offset;
                    let w = p.width;
                    quote! { ((__bits(&__word, #off as usize, #w)) << #sh) }
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
