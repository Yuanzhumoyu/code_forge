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
//!
//! 迭代 2 范围：`meta.default_inst_width == 32` 的定宽 ISA（riscv64 试点）；
//! 变长 form（modrm/vex 语义键）与 64/16 位定宽在迭代 3+ 支持。

use super::model::*;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

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
    let reg_tables = gen_reg_tables(model, &infos)?;
    let mem_support = gen_mem_support(&infos)?;
    let inst_enum = gen_inst_enum(&infos);
    let (encode_fn, decode_fn) = if variable {
        (gen_vlen_encode(&infos, model)?, gen_vlen_decode(&infos)?)
    } else {
        (gen_encode(&infos, model)?, gen_decode(&infos, model)?)
    };
    let disasm_fn = gen_disassemble(&infos)?;
    let asm_fn = gen_assemble(&infos)?;
    Ok(quote! {
        // ── v12 生成模块（迭代 2/3/3b：自包含 encode/decode/asm）──
        #reg_tables
        #mem_support
        #inst_enum
        #encode_fn
        #decode_fn
        #disasm_fn
        #asm_fn
    })
}

/// 有 mem 操作数时生成 `MemRef` 结构 + 渲染/解析 helpers（自包含）。
fn gen_mem_support(infos: &[InstInfo]) -> Result<TokenStream, String> {
    // 找 mem 槽的 base 寄存器组（class）
    let mut base_group: Option<&str> = None;
    for info in infos {
        for (_, _, slot) in &info.operands {
            if slot.kind == OperandKind::Mem
                && let Some(g) = slot.class.as_deref()
            {
                base_group = Some(g);
            }
        }
    }
    let Some(group) = base_group else {
        return Ok(quote! {});
    };
    let name_fn = format_ident!("{}_name", group);
    let reg_fn = format_ident!("__parse_reg_{}", group);
    Ok(quote! {
        /// 内存操作数（自包含；base 为物理寄存器索引，disp 为字节位移）。
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub struct MemRef {
            pub base: u32,
            pub disp: i64,
        }
        fn __render_mem(m: &MemRef) -> String {
            let base = #name_fn(m.base).unwrap_or("?");
            if m.disp == 0 {
                format!("[{base}]")
            } else if m.disp > 0 {
                format!("[{base}+{}]", m.disp)
            } else {
                format!("[{base}{}]", m.disp)
            }
        }
        fn __parse_mem_ref(s: &str) -> Result<MemRef, String> {
            let t = s.trim();
            let Some(inner) = t.strip_prefix('[').and_then(|x| x.strip_suffix(']')) else {
                return Err(format!("expected [base(+disp)], got '{s}'"));
            };
            if let Some(plus) = inner.find('+') {
                let base = #reg_fn(inner[..plus].trim())?;
                let disp = __parse_imm(inner[plus + 1..].trim())?;
                Ok(MemRef { base, disp })
            } else if let Some(minus) = inner.find('-') {
                let base = #reg_fn(inner[..minus].trim())?;
                let disp = __parse_imm(inner[minus + 1..].trim())?
                    .checked_neg()
                    .ok_or_else(|| format!("disp out of range in '{s}'"))?;
                Ok(MemRef { base, disp })
            } else {
                let base = #reg_fn(inner.trim())?;
                Ok(MemRef { base, disp: 0 })
            }
        }
    })
}

/// 单条指令的生成信息：form 解析 + 操作数→位域绑定 + mnemonic/asm 模板。
/// `inst` 为 owned（families 展开后每个 variant 是一条合成指令）。
struct InstInfo<'a> {
    inst: Instruction,
    form: &'a Form,
    vn: syn::Ident,
    mnemonic: String,
    /// 操作数绑定：(位域名, 字段标识, 槽)。
    operands: Vec<(String, syn::Ident, &'a OperandSlot)>,
}

fn collect_inst_infos<'a>(m: &'a V12Model) -> Result<Vec<InstInfo<'a>>, String> {
    // 展开 families：每个 variant 合成一条指令（family.fields 共享 +
    // variant.fields 覆盖；form/operands 共享；opcode/mnemonic/asm 取变体）
    let mut insts: Vec<Instruction> = m.instructions.clone();
    for fam in &m.families {
        for var in &fam.variants {
            let mut fields = fam.fields.clone().unwrap_or_default();
            if let Some(vf) = &var.fields {
                for (k, v) in vf {
                    fields.insert(k.clone(), *v);
                }
            }
            // asm：variant 优先；家族模板替换 {mnemonic} 为变体 mnemonic
            let asm = var.asm.clone().or_else(|| {
                fam.asm.as_ref().map(|a| {
                    let mn = var
                        .mnemonic
                        .clone()
                        .unwrap_or_else(|| var.name.to_lowercase());
                    a.replace("{mnemonic}", &mn)
                })
            });
            insts.push(Instruction {
                name: var.name.clone(),
                form: fam.form.clone(),
                opcode: var.opcode,
                fields: if fields.is_empty() {
                    None
                } else {
                    Some(fields)
                },
                operands: fam.operands.clone(),
                mnemonic: var.mnemonic.clone(),
                asm,
                when: var.when.clone(),
                vex: None,
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
        let of = form.operand_fields.as_deref();
        let fixed = form.opcode_field.is_some();
        let mut operands = Vec::new();
        for (i, op) in inst.operands.iter().enumerate() {
            let slot = m
                .operand_slots
                .iter()
                .find(|s| s.name == op.slot)
                .ok_or_else(|| {
                    format!(
                        "[[instructions.{}]]: operand slot '{}' missing",
                        inst.name, op.slot
                    )
                })?;
            // 字段名：定宽 → 位域名（operand_fields[i] 或 field 覆盖）；
            // 变长 → 位置名 op{i}（无位域绑定；ModRM 语义由 form 键驱动）
            let field = if fixed {
                op.field.clone().unwrap_or_else(|| of.unwrap()[i].clone())
            } else {
                op.field.clone().unwrap_or_else(|| format!("op{i}"))
            };
            let fid = format_ident!("{}", field);
            operands.push((field, fid, slot));
        }
        // mnemonic：asm 存在时从 asm 首词推导（与 mnemonic 字段交叉校验，
        // 单一事实来源）；否则 mnemonic 字段或指令名小写。
        let mnemonic = if let Some(a) = &inst.asm {
            let first = a
                .split_whitespace()
                .next()
                .ok_or_else(|| format!("[[instructions.{}]]: asm must not be empty", inst.name))?;
            if let Some(mf) = &inst.mnemonic
                && mf != first
            {
                return Err(format!(
                    "[[instructions.{}]]: asm mnemonic '{first}' != mnemonic field '{mf}'",
                    inst.name
                ));
            }
            first.to_string()
        } else {
            inst.mnemonic
                .clone()
                .unwrap_or_else(|| inst.name.to_lowercase())
        };
        out.push(InstInfo {
            inst: inst.clone(),
            form,
            vn: crate::codegen::pascal_ident(&inst.name),
            mnemonic,
            operands,
        });
    }
    Ok(out)
}

// ─────────────────────────────── 寄存器表 ───────────────────────────────

/// 每使用到的寄存器组生成 `{group}_name(i)` 与 `__parse_reg_{group}(s)`。
fn gen_reg_tables(model: &V12Model, infos: &[InstInfo]) -> Result<TokenStream, String> {
    // 只生成指令操作数实际引用的组，避免无用函数警告
    let mut used: Vec<String> = Vec::new();
    for info in infos {
        for (_, _, slot) in &info.operands {
            if slot.kind == OperandKind::Reg
                && let Some(class) = &slot.class
                && !used.contains(class)
            {
                used.push(class.clone());
            }
        }
    }
    let mut fns = Vec::new();
    for gname in used {
        let Some(g) = model.reg.get(&gname) else {
            return Err(format!(
                "reg group '{gname}' (used by an operand) is not declared"
            ));
        };
        let names = group_names(g)?;
        let name_fn = format_ident!("{}_name", gname);
        let parse_fn = format_ident!("__parse_reg_{}", gname);
        let mut name_arms = Vec::new();
        let mut parse_arms = Vec::new();
        for (i, n) in names.iter().enumerate() {
            let lit = syn::LitStr::new(n, proc_macro2::Span::call_site());
            let idx = i as u32;
            name_arms.push(quote! { #idx => Some(#lit) });
            let up = syn::LitStr::new(&n.to_uppercase(), proc_macro2::Span::call_site());
            parse_arms.push(quote! { #up => Ok(#idx) });
        }
        let gname_lit = syn::LitStr::new(&gname, proc_macro2::Span::call_site());
        fns.push(quote! {
            /// 组内索引 → 寄存器名（v12 物理编码索引 = 组内索引 + base_index）。
            pub fn #name_fn(i: u32) -> Option<&'static str> {
                match i { #(#name_arms,)* _ => None }
            }
            fn #parse_fn(s: &str) -> Result<u32, String> {
                match s.trim().to_ascii_uppercase().as_str() {
                    #(#parse_arms,)*
                    _ => Err(format!("expected {} register, got '{}'", #gname_lit, s)),
                }
            }
        });
    }
    Ok(quote! { #(#fns)* })
}

fn group_names(g: &RegGroup) -> Result<Vec<String>, String> {
    if let Some(ns) = &g.names {
        return Ok(ns.clone());
    }
    let prefix = g.prefix.clone().unwrap_or_else(|| "R".into());
    let count = g
        .count
        .ok_or_else(|| "reg group needs `names` or `count`".to_string())?;
    Ok((0..count as usize)
        .map(|i| format!("{prefix}{i}"))
        .collect())
}

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
                    .map(|(_, fid, slot)| {
                        let ty = match slot.kind {
                            OperandKind::Reg => quote! { u32 },
                            OperandKind::Opsize => quote! { u8 },
                            OperandKind::Mem => quote! { MemRef },
                            _ => quote! { i64 },
                        };
                        quote! { #fid: #ty }
                    })
                    .collect();
                quote! { #vn { #(#fs),* } }
            }
        })
        .collect();
    quote! {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub enum Inst { #(#variants),* }
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
        for (fname, fid, _) in &info.operands {
            let bf = get_bf(m, fname)?;
            stmts.extend(place_ts(quote! { (*#fid as u64) }, bf));
        }
        // 未覆盖位域天然为 0（__w 初始 0）——无需显式置零
        let pat = if info.operands.is_empty() {
            quote! { Inst::#vn }
        } else {
            let ids: Vec<_> = info
                .operands
                .iter()
                .map(|(_, fid, _)| fid.clone())
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
            match inst { #(#arms,)* }
        }
    })
}

// ─────────────────────────────── decode ───────────────────────────────

fn gen_decode(infos: &[InstInfo], m: &V12Model) -> Result<TokenStream, String> {
    let little = m.meta.endian == Endian::Little;
    let mut stmts: Vec<TokenStream> = Vec::new();
    for info in infos {
        let vn = &info.vn;
        let mut guards: Vec<TokenStream> = Vec::new();
        // 覆盖位域（opcode + fields + 操作数位域）：其**补集**必须为 0。
        // 这统一处理了 form 的隐式 0 位域（纯 opcode 形式如 NOP/ECALL 的
        // 其余位、操作数不足时多余位域位置、fields 之外的固定 0 位）。
        let mut covered_ranges: Vec<(u32, u32)> = Vec::new();
        // 主 opcode guard
        let opcode_field = single_field(
            m,
            info.form.opcode_field.as_deref().unwrap(),
            "opcode_field",
        )?;
        let opcode = info.inst.opcode.unwrap();
        guards.push(guard_ts(opcode_field, opcode));
        covered_ranges.extend(bf_ranges(opcode_field));
        // fields 固定值 guard
        if let Some(fields) = &info.inst.fields {
            for (fname, val) in fields {
                let bf = single_field(m, fname, "fields")?;
                guards.push(guard_ts(bf, *val));
                covered_ranges.extend(bf_ranges(bf));
            }
        }
        // 操作数位域（其位不被零 guard 约束）
        for (fname, _, _) in &info.operands {
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
        guards.push(quote! { ((__w & !(#covered_mask)) == 0) });
        // 字段提取
        let mut binds: Vec<TokenStream> = Vec::new();
        let mut ctor_fields: Vec<TokenStream> = Vec::new();
        for (fname, fid, slot) in &info.operands {
            let bf = get_bf(m, fname)?;
            let raw = extract_ts(bf);
            let expr: TokenStream = match slot.kind {
                OperandKind::Reg => quote! { #raw as u32 },
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
        let guard = if guards.is_empty() {
            quote! { true }
        } else {
            quote! { #(#guards)&&* }
        };
        let ctor = if info.operands.is_empty() {
            quote! { Inst::#vn }
        } else {
            quote! { Inst::#vn { #(#ctor_fields),* } }
        };
        stmts.push(quote! {
            if #guard {
                #(#binds)*
                return Some((#ctor, 4));
            }
        });
    }
    let read = if little {
        quote! { u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64 }
    } else {
        quote! { u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64 }
    };
    Ok(quote! {
        /// 解码 4 字节为指令；声明序首匹配（与 v11 一致），无匹配 → None。
        /// 返回 (指令, 消费字节数)。
        pub fn decode(bytes: &[u8]) -> Option<(Inst, usize)> {
            if bytes.len() < 4 {
                return None;
            }
            let __w = #read;
            #(#stmts)*
            None
        }
    })
}

// ─────────────────────── 变长 encode/decode（迭代 3）───────────────────────

/// 变长 form 的字段语义解析结果。
struct VlenCtx {
    /// 是否 opsize 语义（有 `opsize` 键：auto 或固定）。
    has_opsize: bool,
    /// opsize 值表达式（u64）：auto → 操作数；fixed → 常数。
    opsize_expr: Option<TokenStream>,
    /// 固定前缀字节表达式（u8）：fields.prefix / 数字 / 0。
    prefix_expr: TokenStream,
    /// REX.W 位表达式（u64）：auto（opsize==64）/ fields.w / 0。
    rex_w_expr: TokenStream,
    /// ModRM 语义（`+r` 形式为 None）。
    modrm: Option<ModrmKind>,
    /// modrm reg 字段值表达式（u64）：rr → 操作数 0；ext → fields.ext。
    reg_expr: Option<TokenStream>,
    /// modrm rm 字段值表达式（u64；内存形式 = 基址）。
    rm_expr: Option<TokenStream>,
    /// 内存形式位移表达式（i64；rr/ext 为 None）。
    disp_expr: Option<TokenStream>,
    /// 尾部立即数字节数（form.imm / 8）。
    imm_bytes: usize,
    /// VEX 语义（form.vex 存在时）：map/pp/w/l 值表达式（u64）。
    vex: Option<VexCtx>,
    /// `+r` 形式 opcode 基值（50/58/B8/C8...）；None = 普通 opcode。
    opcode_reg: Option<u64>,
    /// rex_w = "always"：恒发 REX.W（+r 的 mov_imm64/bswap）。
    rex_w_always: bool,
}

/// VEX 编码上下文（迭代 4）。
struct VexCtx {
    map_expr: TokenStream,
    pp_expr: TokenStream,
    w_expr: TokenStream,
    l_expr: TokenStream,
    /// vvvv 语义：操作数 ≥3 且第 3 个是 reg 槽 → ~op2（有源）；否则 0x0F。
    has_src: bool,
}

/// ModRM 语义键（迭代 3/3b）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModrmKind {
    /// mod=11：reg=op0, rm=op1。
    RR,
    /// mod=11：reg=fields.ext（扩展码），rm=op0。
    Ext,
    /// 内存：reg=op0, base=op1（reg 槽），disp=0。
    MemReg,
    /// 内存：reg=op0, mem=op1（mem 槽 → base/disp）。
    MemRefOp,
}

impl ModrmKind {
    fn parse(s: Option<&str>) -> Option<ModrmKind> {
        match s {
            Some("rr") => Some(ModrmKind::RR),
            Some("ext") => Some(ModrmKind::Ext),
            Some("rm_mem") => Some(ModrmKind::MemReg),
            Some("rm_memref") => Some(ModrmKind::MemRefOp),
            _ => None,
        }
    }
    fn is_mem(self) -> bool {
        matches!(self, ModrmKind::MemReg | ModrmKind::MemRefOp)
    }
}

fn vlen_ctx(info: &InstInfo) -> Result<VlenCtx, String> {
    let form = info.form;
    let fields = info.inst.fields.as_ref();
    let field_val = |k: &str| fields.and_then(|f| f.get(k)).copied().unwrap_or(0);
    // opsize
    let (has_opsize, opsize_expr) = match form.opsize {
        None => (false, None),
        Some(OpsizeSpec::Auto(_)) => {
            let fid = info
                .operands
                .iter()
                .find(|(_, _, s)| s.kind == OperandKind::Opsize)
                .map(|(_, fid, _)| fid.clone())
                .ok_or_else(|| {
                    format!(
                        "[[instructions.{}]]: form opsize=auto requires an opsize operand",
                        info.inst.name
                    )
                })?;
            (true, Some(quote! { *#fid as u64 }))
        }
        Some(OpsizeSpec::Fixed(v)) => (true, Some(quote! { #v })),
    };
    // 前缀
    let prefix_expr: TokenStream = match form.prefix.as_deref() {
        None => quote! { 0u8 },
        Some("field") => {
            let v = field_val("prefix");
            quote! { #v as u8 }
        }
        Some(p) => {
            let v = parse_u64(p).unwrap_or(0);
            quote! { #v as u8 }
        }
    };
    // REX.W：显式声明优先；有 opsize 语义且未声明 → 默认 opsize==64 驱动
    let rex_w_expr: TokenStream = match form.rex_w.as_deref() {
        Some("auto") => quote! { if __opsize == 64 { 1u64 } else { 0u64 } },
        Some("field") => {
            let v = field_val("w");
            quote! { #v as u64 }
        }
        None if has_opsize => quote! { if __opsize == 64 { 1u64 } else { 0u64 } },
        _ => quote! { 0u64 },
    };
    // modrm reg/rm/disp：`+r` 形式无 ModRM（opcode 内嵌 reg）
    let (modrm, reg_expr, rm_expr, disp_expr) =
        if form.opcode_reg.is_some() {
            (None, None, None, None)
        } else {
            let modrm = ModrmKind::parse(form.modrm.as_deref()).ok_or_else(|| {
                format!(
                    "[[instructions.{}]]: form modrm key {:?} unsupported (rr/ext/rm_mem/rm_memref)",
                    info.inst.name, form.modrm
                )
            })?;
            let (reg_expr, rm_expr, disp_expr): (TokenStream, TokenStream, Option<TokenStream>) =
                match modrm {
                    ModrmKind::RR => {
                        let r0 = info.operands[0].1.clone();
                        let r1 = info.operands[1].1.clone();
                        (quote! { *#r0 as u64 }, quote! { *#r1 as u64 }, None)
                    }
                    ModrmKind::Ext => {
                        let ext = field_val("ext");
                        let r0 = info.operands[0].1.clone();
                        (quote! { #ext }, quote! { *#r0 as u64 }, None)
                    }
                    ModrmKind::MemReg => {
                        let r0 = info.operands[0].1.clone();
                        let b = info.operands[1].1.clone();
                        (
                            quote! { *#r0 as u64 },
                            quote! { *#b as u64 },
                            Some(quote! { 0i64 }),
                        )
                    }
                    ModrmKind::MemRefOp => {
                        let r0 = info.operands[0].1.clone();
                        let m = info.operands[1].1.clone();
                        (
                            quote! { *#r0 as u64 },
                            quote! { #m.base as u64 },
                            Some(quote! { #m.disp }),
                        )
                    }
                };
            (Some(modrm), Some(reg_expr), Some(rm_expr), disp_expr)
        };
    let imm_bytes = (form.imm.unwrap_or(0) / 8) as usize;
    // VEX 语义（form.vex 存在时）：map/pp/w/l 来源数字或 fields.vex_*（缺省 0）
    let vex = if let Some(vs) = &form.vex {
        let f = |k: &str, key: &str| -> TokenStream {
            match vs_value(vs, k) {
                Some(v) => quote! { #v as u64 },
                None => {
                    let v = field_val(key);
                    quote! { #v as u64 }
                }
            }
        };
        // vvvv：操作数 ≥3 且第 3 个是 reg 槽 → 有源（~op2）
        let has_src = info.operands.len() >= 3 && info.operands[2].2.kind == OperandKind::Reg;
        Some(VexCtx {
            map_expr: f("map", "vex_map"),
            pp_expr: f("pp", "vex_pp"),
            w_expr: f("w", "vex_w"),
            l_expr: f("l", "vex_l"),
            has_src,
        })
    } else {
        None
    };
    Ok(VlenCtx {
        has_opsize,
        opsize_expr,
        prefix_expr,
        rex_w_expr,
        modrm,
        reg_expr,
        rm_expr,
        disp_expr,
        imm_bytes,
        vex,
        opcode_reg: form.opcode_reg,
        rex_w_always: form.rex_w.as_deref() == Some("always"),
    })
}

/// VEX 字段来源：`"field"` → None（取 fields.vex_*）；数字 → 固定值。
fn vs_value(vs: &VexSpec, key: &str) -> Option<u64> {
    let s = match key {
        "map" => vs.map.as_deref(),
        "pp" => vs.pp.as_deref(),
        "w" => vs.w.as_deref(),
        "l" => vs.l.as_deref(),
        _ => None,
    }?;
    if s == "field" { None } else { parse_u64(s) }
}

/// 变长 encode：字节流（prefix → REX → escape → opcode → ModRM → imm）。
fn gen_vlen_encode(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    let mut arms = Vec::new();
    for info in infos {
        let vn = &info.vn;
        let ctx = vlen_ctx(info)?;
        let mut stmts: Vec<TokenStream> = Vec::new();
        // opsize → __opsize 局部（必须先于 66/REX 检查）
        let opsize_bind = if let Some(oe) = &ctx.opsize_expr {
            quote! { let __opsize = #oe; }
        } else {
            quote! { let __opsize = 0u64; }
        };
        // `+r` 形式（50+r/push、58+r/pop、B8+r/mov_imm64、C8+r/bswap）：
        // REX（always → 0x48|B；否则 reg≥8 → 0x41）+ [escape] + opcode|reg&7 + imm
        if let Some(base) = ctx.opcode_reg {
            let reg0 = info.operands[0].1.clone();
            let rex_always = ctx.rex_w_always;
            stmts.push(quote! {
                let __reg = *#reg0 as u64;
                if #rex_always {
                    __bytes.push((0x48u64 | ((__reg >> 3) & 1)) as u8);
                } else if (__reg & 8) != 0 {
                    __bytes.push(0x41u8);
                }
            });
            if let Some(esc) = &info.form.escape {
                for e in esc {
                    stmts.push(quote! { __bytes.push(#e as u8); });
                }
            }
            stmts.push(quote! { __bytes.push((#base | (__reg & 7)) as u8); });
            if ctx.imm_bytes > 0 {
                let imm_fid = info
                    .operands
                    .iter()
                    .find(|(_, _, s)| s.kind == OperandKind::Imm)
                    .map(|(_, fid, _)| fid.clone())
                    .ok_or_else(|| {
                        format!("[[instructions.{}]]: form imm requires an imm operand", info.inst.name)
                    })?;
                let le_bytes = match ctx.imm_bytes {
                    1 => quote! { (*#imm_fid as u8).to_le_bytes().to_vec() },
                    2 => quote! { (*#imm_fid as u16).to_le_bytes().to_vec() },
                    4 => quote! { (*#imm_fid as u32).to_le_bytes().to_vec() },
                    8 => quote! { (*#imm_fid as u64).to_le_bytes().to_vec() },
                    n => return Err(format!("unsupported imm width {n} bytes")),
                };
                stmts.push(quote! { __bytes.extend_from_slice(&#le_bytes); });
            }
        } else if let Some(vex) = &ctx.vex {
            let reg = ctx.reg_expr.as_ref().unwrap();
            let rm = ctx.rm_expr.as_ref().unwrap();
            let vopcode = info.inst.opcode.unwrap();
            let map = &vex.map_expr;
            let pp = &vex.pp_expr;
            let w = &vex.w_expr;
            let l = &vex.l_expr;
            let has_src = vex.has_src;
            // vvvv 源（has_src 时第 3 操作数为 reg）
            let vv_expr: TokenStream = if has_src {
                let vv_fid = info.operands[2].1.clone();
                quote! { (!((*#vv_fid as u64) as u8 & 0x0F)) & 0x0F }
            } else {
                quote! { 0x0F }
            };
            stmts.push(quote! {
                let __reg = #reg;
                let __rm = #rm;
                __bytes.push(0xC4u8);
                let __b: u8 = if (__rm & 8) != 0 { 0 } else { 1 };
                let __x: u8 = 1;
                let __r: u8 = if (__reg & 8) != 0 { 0 } else { 1 };
                __bytes.push(((__r << 7) | (__x << 6) | (__b << 5) | ((#map as u8) & 0x1F)) as u8);
                let __w = #w;
                let __vvvv: u8 = #vv_expr;
                __bytes.push((((__w as u8) << 7) | (__vvvv << 3)
                    | ((#l as u8 & 0x1) << 2) | (#pp as u8 & 0x3)) as u8);
                __bytes.push(#vopcode as u8);
                __bytes.push(((3u64 << 6) | ((__reg & 7) << 3) | (__rm & 7)) as u8);
            });
            // 尾部立即数（VEX imm8 等）
            if ctx.imm_bytes > 0 {
                let imm_fid = info
                    .operands
                    .iter()
                    .find(|(_, _, s)| s.kind == OperandKind::Imm)
                    .map(|(_, fid, _)| fid.clone())
                    .ok_or_else(|| {
                        format!(
                            "[[instructions.{}]]: form imm requires an imm operand",
                            info.inst.name
                        )
                    })?;
                let le_bytes = match ctx.imm_bytes {
                    1 => quote! { (*#imm_fid as u8).to_le_bytes().to_vec() },
                    2 => quote! { (*#imm_fid as u16).to_le_bytes().to_vec() },
                    4 => quote! { (*#imm_fid as u32).to_le_bytes().to_vec() },
                    8 => quote! { (*#imm_fid as u64).to_le_bytes().to_vec() },
                    n => return Err(format!("unsupported imm width {n} bytes")),
                };
                stmts.push(quote! { __bytes.extend_from_slice(&#le_bytes); });
            }
        } else {
            let reg = ctx.reg_expr.as_ref().unwrap();
            let rm = ctx.rm_expr.as_ref().unwrap();
            stmts.push(opsize_bind);
            if ctx.has_opsize {
                stmts.push(quote! { if __opsize == 16 { __bytes.push(0x66u8); } });
            }
            let prefix_expr = &ctx.prefix_expr;
            let prefix_push = quote! {
                let __p = #prefix_expr;
                if __p != 0 { __bytes.push(__p); }
            };
            stmts.push(prefix_push);
            let rex_w = &ctx.rex_w_expr;
            // REX 发出条件：有 opsize → opsize==64 或扩展寄存器；无 opsize（SSE）→ 扩展寄存器
            let rex_cond = if ctx.has_opsize {
                quote! { __opsize == 64 || (#reg & 8) != 0 || (#rm & 8) != 0 }
            } else {
                quote! { (#reg & 8) != 0 || (#rm & 8) != 0 }
            };
            stmts.push(quote! {
                let __reg = #reg;
                let __rm = #rm;
                let __rex_w = #rex_w;
                if #rex_cond {
                    let __rex: u8 = ((0x40u64 | (__rex_w << 3)
                        | ((__reg >> 3) & 1) << 2 | ((__rm >> 3) & 1)) & 0xFF) as u8;
                    __bytes.push(__rex);
                }
            });
            // escape + opcode
            if let Some(esc) = &info.form.escape {
                for e in esc {
                    stmts.push(quote! { __bytes.push(#e as u8); });
                }
            }
            let opcode = info.inst.opcode.unwrap();
            stmts.push(quote! { __bytes.push(#opcode as u8); });
            // ModRM：mod=11（rr/ext）或内存（rm_mem/rm_memref）
            if ctx.modrm.unwrap().is_mem() {
                let force = mem_force_disp(model, info)?;
                let force_toks: Vec<TokenStream> = force.iter().map(|&b| quote! { #b }).collect();
                let disp = ctx.disp_expr.as_ref().unwrap();
                stmts.push(quote! {
                    let __disp = #disp;
                    const FORCE_DISP: &[u8] = &[#(#force_toks),*];
                    let __force = FORCE_DISP.contains(&(__rm as u8));
                    let __mod: u8 = if __disp == 0 && !__force {
                        0
                    } else if (-128i64..=127i64).contains(&__disp) {
                        1
                    } else {
                        2
                    };
                    let __sib = (__rm & 7) == 4;
                    if __sib {
                        __bytes.push((((__mod as u64) << 6) | ((__reg & 7) << 3) | 4) as u8);
                        __bytes.push((0x20u64 | (__rm & 7)) as u8);
                    } else {
                        __bytes.push((((__mod as u64) << 6) | ((__reg & 7) << 3) | (__rm & 7)) as u8);
                    }
                    if __mod == 1 {
                        __bytes.push(__disp as u8);
                    } else if __mod == 2 {
                        __bytes.extend_from_slice(&(__disp as u32).to_le_bytes());
                    }
                });
            } else {
                stmts.push(quote! {
                    __bytes.push(((3u64 << 6) | ((__reg & 7) << 3) | (__rm & 7)) as u8);
                });
            }
            // 尾部立即数（form.imm）
            if ctx.imm_bytes > 0 {
                let imm_fid = info
                    .operands
                    .iter()
                    .find(|(_, _, s)| s.kind == OperandKind::Imm)
                    .map(|(_, fid, _)| fid.clone())
                    .ok_or_else(|| {
                        format!(
                            "[[instructions.{}]]: form imm requires an imm operand",
                            info.inst.name
                        )
                    })?;
                let le_bytes = match ctx.imm_bytes {
                    1 => quote! { (*#imm_fid as u8).to_le_bytes().to_vec() },
                    2 => quote! { (*#imm_fid as u16).to_le_bytes().to_vec() },
                    4 => quote! { (*#imm_fid as u32).to_le_bytes().to_vec() },
                    8 => quote! { (*#imm_fid as u64).to_le_bytes().to_vec() },
                    n => return Err(format!("unsupported imm width {n} bytes")),
                };
                stmts.push(quote! { __bytes.extend_from_slice(&#le_bytes); });
            }
        }
        let pat = if info.operands.is_empty() {
            quote! { Inst::#vn }
        } else {
            let ids: Vec<_> = info
                .operands
                .iter()
                .map(|(_, fid, _)| fid.clone())
                .collect();
            quote! { Inst::#vn { #(#ids),* } }
        };
        arms.push(quote! {
            #pat => {
                let mut __bytes: Vec<u8> = Vec::new();
                #(#stmts)*
                Ok(__bytes)
            }
        });
    }
    Ok(quote! {
        /// 编码单条指令为字节序列（变长 ISA）。
        pub fn encode(inst: &Inst) -> Result<Vec<u8>, String> {
            match inst { #(#arms,)* }
        }
    })
}

/// `+r` 形式解码 arm：REX 扫描后 [escape] `opcode_reg|reg&7` [imm]。
fn gen_vlen_opcode_reg_decode_arm(
    info: &InstInfo,
    ctx: &VlenCtx,
    base: u64,
) -> Result<TokenStream, String> {
    let vn = &info.vn;
    let imm_bytes = ctx.imm_bytes;
    let mut off = 0usize;
    let mut conds: Vec<TokenStream> = Vec::new();
    if let Some(esc) = &info.form.escape {
        for e in esc {
            conds.push(quote! { bytes[__o + #off] == #e as u8 });
            off += 1;
        }
    }
    // opcode 高 5 位匹配（低 3 位 = reg）
    conds.push(quote! { (bytes[__o + #off] & 0xF8) == ((#base & 0xF8) as u8) });
    let opcode_off = off;
    let total = off + 1 + imm_bytes;
    // 字段提取：op0 = reg（opcode&7 | REX.B<<3）；后续 imm 操作数
    let mut binds: Vec<TokenStream> = Vec::new();
    let mut ctor_fields: Vec<TokenStream> = Vec::new();
    for (i, (_, fid, slot)) in info.operands.iter().enumerate() {
        let expr: TokenStream = match slot.kind {
            OperandKind::Reg if i == 0 => {
                quote! { ((bytes[__o + #opcode_off] & 7) as u32) | (__rex_b << 3) }
            }
            OperandKind::Imm => {
                let raw = imm_read_ts(opcode_off + 1, imm_bytes);
                let signed = slot.signed.unwrap_or(false);
                let w = slot.width.unwrap_or(64);
                if signed {
                    sign_extend_ts(raw, w)
                } else {
                    quote! { #raw as i64 }
                }
            }
            _ => {
                return Err(format!(
                    "[[instructions.{}]]: +r operand {i} kind {:?} unsupported",
                    info.inst.name, slot.kind
                ));
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
    let cond = if conds.is_empty() {
        quote! { true }
    } else {
        quote! { #(#conds)&&* }
    };
    Ok(quote! {
        if __o + #total <= bytes.len() && #cond {
            #(#binds)*
            return Some((#ctor, __o + #total));
        }
    })
}

/// VEX 解码 arm：C4 + vex2/vex3（map/pp/w/l guard）→ opcode → ModRM（mod=11）。
fn gen_vlen_vex_decode_arm(info: &InstInfo, ctx: &VlenCtx) -> Result<TokenStream, String> {
    let vn = &info.vn;
    let vex = ctx.vex.as_ref().unwrap();
    let opcode = info.inst.opcode.unwrap();
    let imm_bytes = ctx.imm_bytes;
    let total = 5 + imm_bytes;
    // vex guard 常量：数字或 fields.vex_*（与 encode 的 vs_value 一致）
    let vex_val = |vs_key: &str, field_key: &str| -> u64 {
        match vs_value(info.form.vex.as_ref().unwrap(), vs_key) {
            Some(v) => v,
            None => info
                .inst
                .fields
                .as_ref()
                .and_then(|f| f.get(field_key))
                .copied()
                .unwrap_or(0),
        }
    };
    let map_v = vex_val("map", "vex_map");
    let pp_v = vex_val("pp", "vex_pp");
    let w_v = vex_val("w", "vex_w");
    let l_v = vex_val("l", "vex_l");
    let has_src = vex.has_src;
    // 字段提取：reg=op0（R 来自 vex2）、rm=op1（B 来自 vex2）、vvvv=op2（有源）
    let mut binds: Vec<TokenStream> = Vec::new();
    let mut ctor_fields: Vec<TokenStream> = Vec::new();
    for (i, (_, fid, slot)) in info.operands.iter().enumerate() {
        let expr: TokenStream = match slot.kind {
            OperandKind::Reg => match i {
                0 => quote! { ((__modrm >> 3) & 7) as u32 | (__r << 3) },
                1 => quote! { (__modrm & 7) as u32 | (__b << 3) },
                2 if has_src => quote! { __vvvv as u32 },
                _ => {
                    return Err(format!(
                        "[[instructions.{}]]: VEX reg operand {i} position unsupported",
                        info.inst.name
                    ));
                }
            },
            OperandKind::Imm => {
                let raw = imm_read_ts(5, imm_bytes);
                let signed = slot.signed.unwrap_or(false);
                let w = slot.width.unwrap_or(32);
                if signed {
                    sign_extend_ts(raw, w)
                } else {
                    quote! { #raw as i64 }
                }
            }
            _ => {
                return Err(format!(
                    "[[instructions.{}]]: VEX operand kind {:?} unsupported",
                    info.inst.name, slot.kind
                ));
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
    Ok(quote! {
        if __o + #total <= bytes.len() && bytes[__o] == 0xC4 {
            let __vex2 = bytes[__o + 1];
            let __vex3 = bytes[__o + 2];
            if ((__vex2 & 0x1F) as u64) == #map_v
                && ((__vex3 & 0x3) as u64) == #pp_v
                && (((__vex3 >> 7) & 1) as u64) == #w_v
                && (((__vex3 >> 2) & 1) as u64) == #l_v
                && bytes[__o + 3] == #opcode as u8
            {
                let __modrm = bytes[__o + 4];
                if (__modrm >> 6) == 3 {
                    let __r: u32 = ((!((__vex2 >> 7) & 1)) & 1) as u32;
                    let __b: u32 = ((!((__vex2 >> 5) & 1)) & 1) as u32;
                    let __vvvv: u32 = ((!((__vex3 >> 3) & 0xF)) & 0xF) as u32;
                    #(#binds)*
                    return Some((#ctor, __o + #total));
                }
            }
        }
    })
}

/// 变长 decode：共享前缀扫描 + 声明序 arm 匹配。
fn gen_vlen_decode(infos: &[InstInfo]) -> Result<TokenStream, String> {
    let mut arms: Vec<TokenStream> = Vec::new();
    for info in infos {
        let vn = &info.vn;
        let ctx = vlen_ctx(info)?;
        // VEX 指令走独立 arm（C4 + vex2/vex3 guard）
        if ctx.vex.is_some() {
            arms.push(gen_vlen_vex_decode_arm(info, &ctx)?);
            continue;
        }
        // `+r` 形式：opcode 含 reg 低 3 位（50/58/B8/C8...）
        if let Some(base) = ctx.opcode_reg {
            arms.push(gen_vlen_opcode_reg_decode_arm(info, &ctx, base)?);
            continue;
        }
        let form = info.form;
        let mut conds: Vec<TokenStream> = Vec::new();
        // 前缀匹配条件：66/F2/F3 → 扫描标志（无长度）；其他（LOCK 0xF0 等）→
        // 显式字节检查（占 1 字节，escape/opcode 偏移 +1）
        let (prefix_cond, prefix_len) = match form.prefix.as_deref() {
            None | Some("opsize") => (None, 0),
            Some("field") => {
                let v = info
                    .inst
                    .fields
                    .as_ref()
                    .and_then(|f| f.get("prefix"))
                    .copied()
                    .unwrap_or(0);
                prefix_cond_ts(v)
            }
            Some(p) => prefix_cond_ts(parse_u64(p).unwrap_or(0)),
        };
        if let Some(c) = prefix_cond {
            conds.push(c);
        }
        // escape + opcode 匹配（prefix 显式字节已占 prefix_len）
        let mut off = prefix_len;
        if let Some(esc) = &form.escape {
            for e in esc {
                conds.push(quote! { bytes[__o + #off] == #e as u8 });
                off += 1;
            }
        }
        let opcode = info.inst.opcode.unwrap();
        conds.push(quote! { bytes[__o + #off] == #opcode as u8 });
        off += 1;
        let modrm_idx = off; // modrm 在 bytes[__o + modrm_idx]
        off += 1;
        let total = off + ctx.imm_bytes;
        let len = total;
        let cond = if conds.is_empty() {
            quote! { true }
        } else {
            quote! { #(#conds)&&* }
        };
        // 字段提取表达式：modrm 语义决定 reg/rm/base/mem 的来源
        let field_expr = |i: usize, slot: &OperandSlot| -> Result<TokenStream, String> {
            let reg_field = quote! { ((__modrm >> 3) & 7) as u32 | (__rex_r << 3) };
            let rm_field = quote! { ((__modrm & 7) as u32) | (__rex_b << 3) };
            match slot.kind {
                OperandKind::Reg => match (ctx.modrm.unwrap(), i) {
                    (ModrmKind::RR, 0) | (ModrmKind::MemReg, 0) | (ModrmKind::MemRefOp, 0) => {
                        Ok(reg_field)
                    }
                    (ModrmKind::RR, 1) | (ModrmKind::Ext, 0) => Ok(rm_field),
                    (ModrmKind::MemReg, 1) => Ok(quote! { __base }),
                    _ => Err(format!(
                        "[[instructions.{}]]: reg operand {i} position unsupported for modrm {:?}",
                        info.inst.name, ctx.modrm
                    )),
                },
                OperandKind::Mem => match (ctx.modrm.unwrap(), i) {
                    (ModrmKind::MemRefOp, 1) => {
                        Ok(quote! { MemRef { base: __base, disp: __disp } })
                    }
                    _ => Err(format!(
                        "[[instructions.{}]]: mem operand {i} position unsupported",
                        info.inst.name
                    )),
                },
                OperandKind::Opsize => Ok(quote! { __opsize as u8 }),
                OperandKind::Imm => {
                    let raw = imm_read_ts(total - ctx.imm_bytes, ctx.imm_bytes);
                    let signed = slot.signed.unwrap_or(false);
                    let w = slot.width.unwrap_or(32);
                    if signed {
                        Ok(sign_extend_ts(raw, w))
                    } else {
                        Ok(quote! { #raw as i64 })
                    }
                }
                _ => Err(format!(
                    "[[instructions.{}]]: operand kind {:?} unsupported in vlen decode",
                    info.inst.name, slot.kind
                )),
            }
        };
        let mut binds: Vec<TokenStream> = Vec::new();
        let mut ctor_fields: Vec<TokenStream> = Vec::new();
        for (i, (_, fid, slot)) in info.operands.iter().enumerate() {
            let expr = field_expr(i, slot)?;
            binds.push(quote! { let #fid = #expr; });
            ctor_fields.push(quote! { #fid });
        }
        let ctor = if info.operands.is_empty() {
            quote! { Inst::#vn }
        } else {
            quote! { Inst::#vn { #(#ctor_fields),* } }
        };
        // ext 形式：ModRM.reg 必须等于固定扩展码
        let modrm_guard: TokenStream = if ctx.modrm.unwrap() == ModrmKind::Ext {
            let ext = info
                .inst
                .fields
                .as_ref()
                .and_then(|f| f.get("ext"))
                .copied()
                .unwrap_or(0);
            quote! { && ((__modrm >> 3) & 7) as u64 == #ext }
        } else {
            quote! {}
        };
        if ctx.modrm.unwrap().is_mem() {
            // 内存形式：mod≠3 + SIB（index=4 无 index）+ disp8/disp32 + RIP-rel 拒绝。
            // MemReg（无 disp 语义）只接受 mod∈{0,1} 且 mod=1 时 disp==0
            //（force_disp_base 的 disp8=0）；MemRefOp 接受 mod∈{0,1,2}。
            let mod_guard: TokenStream = if ctx.modrm.unwrap() == ModrmKind::MemReg {
                quote! { __mod != 3 && __mod != 2 }
            } else {
                quote! { __mod != 3 }
            };
            let disp_zero_check: TokenStream = if ctx.modrm.unwrap() == ModrmKind::MemReg {
                quote! {
                    if __mod == 1 {
                        __disp = (bytes[__o2] as i8) as i64;
                        __o2 += 1;
                        if __disp != 0 {
                            __disp_ok = false;
                        }
                    }
                }
            } else {
                quote! {
                    if __mod == 1 {
                        if __o2 < bytes.len() {
                            __disp = (bytes[__o2] as i8) as i64;
                            __o2 += 1;
                        } else {
                            __disp_ok = false;
                        }
                    } else if __mod == 2 {
                        if __o2 + 3 < bytes.len() {
                            __disp = i32::from_le_bytes([
                                bytes[__o2],
                                bytes[__o2 + 1],
                                bytes[__o2 + 2],
                                bytes[__o2 + 3],
                            ]) as i64;
                            __o2 += 4;
                        } else {
                            __disp_ok = false;
                        }
                    }
                }
            };
            arms.push(quote! {
                if __o + #total <= bytes.len() && #cond {
                    let __modrm = bytes[__o + #modrm_idx];
                    let __mod = __modrm >> 6;
                    if #mod_guard && !(__mod == 0 && (__modrm & 7) == 5) {
                        let mut __o2 = __o + #len;
                        let mut __base: u32 = ((__modrm & 7) as u32) | (__rex_b << 3);
                        let mut __sib_ok = true;
                        if (__modrm & 7) == 4 {
                            if __o2 < bytes.len() {
                                let __sib_byte = bytes[__o2];
                                __o2 += 1;
                                if ((__sib_byte >> 3) & 7) == 4 {
                                    __base = ((__sib_byte & 7) as u32) | (__rex_b << 3);
                                } else {
                                    __sib_ok = false;
                                }
                            } else {
                                __sib_ok = false;
                            }
                        }
                        let mut __disp: i64 = 0;
                        let mut __disp_ok = true;
                        #disp_zero_check
                        if __sib_ok && __disp_ok {
                            #(#binds)*
                            return Some((#ctor, __o2));
                        }
                    }
                }
            });
        } else {
            arms.push(quote! {
                if __o + #total <= bytes.len() && #cond {
                    let __modrm = bytes[__o + #modrm_idx];
                    if (__modrm >> 6) == 3 #modrm_guard {
                        #(#binds)*
                        return Some((#ctor, __o + #len));
                    }
                }
            });
        }
    }
    Ok(quote! {
        /// 解码字节流开头的单条指令；声明序首匹配，无匹配 → None。
        /// 返回 (指令, 消费字节数)。
        pub fn decode(bytes: &[u8]) -> Option<(Inst, usize)> {
            let mut __o = 0usize;
            let mut __opsize: u32 = 32;
            let mut __p66 = false;
            let mut __pF0 = false;
            let mut __pF2 = false;
            let mut __pF3 = false;
            let mut __rex_r: u32 = 0;
            let mut __rex_b: u32 = 0;
            while __o < bytes.len() {
                let __b = bytes[__o];
                if __b == 0x66 { __p66 = true; __opsize = 16; __o += 1; }
                else if __b == 0xF0 { __pF0 = true; __o += 1; }
                else if __b == 0xF2 { __pF2 = true; __o += 1; }
                else if __b == 0xF3 { __pF3 = true; __o += 1; }
                else if __b == 0x67 { __o += 1; }
                else if (0x40..=0x4F).contains(&__b) {
                    __rex_r = ((__b >> 2) & 1) as u32;
                    __rex_b = (__b & 1) as u32;
                    if (__b & 0x08) != 0 { __opsize = 64; }
                    __o += 1;
                } else { break; }
            }
            #(#arms)*
            None
        }
    })
}

/// 固定前缀的匹配条件：(条件, 占用的指令长度偏移)。
/// 66/F0/F2/F3 由前缀扫描消费（条件用标志，无长度）；其他前缀字节不在
/// 扫描集 → 显式检查 bytes[__o]（占 1 字节）。
fn prefix_cond_ts(v: u64) -> (Option<TokenStream>, usize) {
    match v {
        0 => (Some(quote! { !__p66 && !__pF0 && !__pF2 && !__pF3 }), 0),
        0x66 => (Some(quote! { __p66 }), 0),
        0xF0 => (Some(quote! { __pF0 }), 0),
        0xF2 => (Some(quote! { __pF2 }), 0),
        0xF3 => (Some(quote! { __pF3 }), 0),
        other => (Some(quote! { bytes[__o] == #other as u8 }), 1),
    }
}

/// 内存寻址的强制位移基址（conventions.modrm.force_disp_base，缺省空）。
fn mem_force_disp(model: &V12Model, info: &InstInfo) -> Result<Vec<u8>, String> {
    if let Some(modrm) = &model.conventions.modrm {
        return Ok(modrm.force_disp_base.clone());
    }
    let _ = info;
    Ok(Vec::new())
}

/// 读取尾部立即数（LE 字节序，从 `__o + start` 起读 `bytes` 字节 → u64 原始值）。
fn imm_read_ts(start: usize, bytes: usize) -> TokenStream {
    match bytes {
        1 => quote! { bytes[__o + #start] as u64 },
        2 => quote! {
            u16::from_le_bytes([bytes[__o + #start], bytes[__o + #start + 1]]) as u64
        },
        4 => quote! {
            u32::from_le_bytes([
                bytes[__o + #start],
                bytes[__o + #start + 1],
                bytes[__o + #start + 2],
                bytes[__o + #start + 3],
            ]) as u64
        },
        8 => quote! {
            u64::from_le_bytes([
                bytes[__o + #start],
                bytes[__o + #start + 1],
                bytes[__o + #start + 2],
                bytes[__o + #start + 3],
                bytes[__o + #start + 4],
                bytes[__o + #start + 5],
                bytes[__o + #start + 6],
                bytes[__o + #start + 7],
            ])
        },
        _ => quote! { 0u64 },
    }
}

/// 解析 TOML 数值字符串（0x 十六进制或十进制）。
fn parse_u64(s: &str) -> Option<u64> {
    let t = s.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).ok()
    } else {
        t.parse::<u64>().ok()
    }
}

// ─────────────────────────────── disassemble ───────────────────────────────

fn gen_disassemble(infos: &[InstInfo]) -> Result<TokenStream, String> {
    let mut arms = Vec::new();
    for info in infos {
        let vn = &info.vn;
        let mut fmt = String::new();
        let mut locals: Vec<TokenStream> = Vec::new();
        // 完整格式 = mnemonic + 操作数模板（段序列渲染）
        fmt.push_str(&info.mnemonic);
        let ops = ops_template(info);
        if !ops.is_empty() {
            fmt.push(' ');
            let segs = parse_template(&ops)?;
            validate_segs(&segs, info)?;
            for seg in segs {
                match seg {
                    Seg::Lit(l) => fmt.push_str(&l),
                    Seg::Op(n) => {
                        let (_, fid, slot) = &info.operands[n];
                        let local = format_ident!("__o{n}");
                        let expr = render_expr(slot, fid);
                        locals.push(quote! { let #local = #expr; });
                        fmt.push_str(&format!("{{__o{n}}}"));
                    }
                }
            }
        }
        let fmt_lit = syn::LitStr::new(&fmt, proc_macro2::Span::call_site());
        let pat = if info.operands.is_empty() {
            quote! { Inst::#vn }
        } else {
            let ids: Vec<_> = info
                .operands
                .iter()
                .map(|(_, fid, _)| fid.clone())
                .collect();
            quote! { Inst::#vn { #(#ids),* } }
        };
        arms.push(quote! { #pat => { #(#locals)* format!(#fmt_lit) } });
    }
    Ok(quote! {
        /// 反汇编为汇编文本。
        pub fn disassemble(inst: &Inst) -> String {
            match inst { #(#arms,)* }
        }
    })
}

/// 操作数渲染表达式（disassemble 用）。
fn render_expr(slot: &OperandSlot, fid: &syn::Ident) -> TokenStream {
    match slot.kind {
        OperandKind::Reg => {
            let gname = format_ident!("{}_name", slot.class.as_deref().unwrap_or(""));
            quote! { #gname(*#fid).unwrap_or("?").to_string() }
        }
        OperandKind::Mem => quote! { __render_mem(#fid) },
        _ => quote! { #fid.to_string() },
    }
}

// ─────────────────────── 通用汇编模板段模型 ───────────────────────

/// 操作数模板的段：字面片段或操作数占位符（`{n}`）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    /// 字面文本（分隔符/方括号/括号/关键字等，原样匹配与输出）。
    Lit(String),
    /// 操作数占位符 `{n}`（索引 = 指令操作数位置）。
    Op(usize),
}

/// 解析操作数模板为段序列（字面与占位符交替，任意字面格式）。
///
/// 替换迭代 3b 的封闭 TokenShape（Simple/Bracket/Mem/Mem0）：`[{n}]`、
/// `{off}({base})`、`byte ptr [{n}]` 等都是字面段的自然组合，用户模板自由。
fn parse_template(tpl: &str) -> Result<Vec<Seg>, String> {
    if tpl.is_empty() {
        return Ok(Vec::new());
    }
    let mut segs = Vec::new();
    let mut lit = String::new();
    let mut chars = tpl.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut num = String::new();
            while let Some(&d) = chars.peek() {
                if d == '}' {
                    break;
                }
                num.push(d);
                chars.next();
            }
            if chars.next() != Some('}') {
                return Err(format!("unterminated '{{' in asm template '{tpl}'"));
            }
            let n: usize = num
                .parse()
                .map_err(|_| format!("bad placeholder '{{{num}}}' in asm template '{tpl}'"))?;
            if !lit.is_empty() {
                segs.push(Seg::Lit(std::mem::take(&mut lit)));
            }
            segs.push(Seg::Op(n));
        } else {
            lit.push(c);
        }
    }
    if !lit.is_empty() {
        segs.push(Seg::Lit(lit));
    }
    if segs.is_empty() {
        return Err(format!("empty asm template '{tpl}'"));
    }
    Ok(segs)
}

/// 校验段序列：占位符索引越界、非文本操作数被引用、连续占位符无字面分隔。
fn validate_segs(segs: &[Seg], info: &InstInfo) -> Result<(), String> {
    let ctx = || format!("[[instructions.{}]] asm", info.inst.name);
    let mut prev_op = false;
    for seg in segs {
        match seg {
            Seg::Lit(l) => {
                if l.is_empty() {
                    return Err(format!("{}: empty literal segment", ctx()));
                }
                prev_op = false;
            }
            Seg::Op(n) => {
                if *n >= info.operands.len() {
                    return Err(format!("{}: placeholder {{{n}}} out of range", ctx()));
                }
                if !is_text_operand(info.operands[*n].2) {
                    return Err(format!(
                        "{}: placeholder {{{n}}} references non-text operand (opsize)",
                        ctx()
                    ));
                }
                if prev_op {
                    return Err(format!(
                        "{}: adjacent placeholders need a literal separator (ambiguity)",
                        ctx()
                    ));
                }
                prev_op = true;
            }
        }
    }
    Ok(())
}

/// 完整汇编格式（含 mnemonic）：`asm` 字段或 `mnemonic + 默认操作数模板`。
fn asm_full(info: &InstInfo) -> String {
    if let Some(a) = &info.inst.asm {
        return a.clone();
    }
    let ops = info
        .operands
        .iter()
        .enumerate()
        .filter(|(_, (_, _, slot))| is_text_operand(slot))
        .map(|(i, _)| format!("{{{i}}}"))
        .collect::<Vec<_>>()
        .join(", ");
    if ops.is_empty() {
        info.mnemonic.clone()
    } else {
        format!("{} {ops}", info.mnemonic)
    }
}

/// 操作数模板（完整格式去掉 mnemonic 前缀）。
fn ops_template(info: &InstInfo) -> String {
    let full = asm_full(info);
    match full.split_once(char::is_whitespace) {
        Some((_, r)) => r.trim().to_string(),
        None => String::new(),
    }
}

// ─────────────────────────────── assemble ───────────────────────────────

fn gen_assemble(infos: &[InstInfo]) -> Result<TokenStream, String> {
    let mut arms: Vec<TokenStream> = Vec::new();
    // 按 mnemonic 分组（保持声明序）；同 mnemonic 内按模板段签名去重
    let mut handled: Vec<(String, Vec<String>)> = Vec::new();
    for info in infos {
        let mn = &info.mnemonic;
        let sig = shape_signature(info)?;
        let entry = handled.iter_mut().find(|(m, _)| m == mn);
        match entry {
            Some((_, sigs)) => {
                if sigs.contains(&sig) {
                    continue;
                }
                sigs.push(sig);
            }
            None => handled.push((mn.clone(), vec![sig])),
        }
    }
    for (mn, _) in &handled {
        let mn_lit = syn::LitStr::new(mn, proc_macro2::Span::call_site());
        let err_lit = syn::LitStr::new(
            &format!("{mn}: operand mismatch"),
            proc_macro2::Span::call_site(),
        );
        let mut tries: Vec<TokenStream> = Vec::new();
        let mut emitted: Vec<String> = Vec::new();
        for info in infos.iter().filter(|i| &i.mnemonic == mn) {
            let sig = shape_signature(info)?;
            if emitted.contains(&sig) {
                continue;
            }
            emitted.push(sig);
            tries.push(gen_assemble_try(info)?);
        }
        arms.push(quote! {
            #mn_lit => {
                #(#tries)*
                return Err(String::from(#err_lit));
            }
        });
    }

    // 共享解析助手
    let parse_imm = quote! {
        fn __parse_imm(s: &str) -> Result<i64, String> {
            let t = s.trim();
            let (neg, rest) = match t.strip_prefix('-') {
                Some(r) => (true, r),
                None => (false, t),
            };
            let v = if let Some(h) = rest
                .strip_prefix("0x")
                .or_else(|| rest.strip_prefix("0X"))
            {
                i64::from_str_radix(h, 16).map_err(|_| format!("bad hex immediate '{s}'"))?
            } else {
                rest.parse::<i64>().map_err(|_| format!("bad immediate '{s}'"))?
            };
            if neg {
                v.checked_neg().ok_or_else(|| format!("immediate out of range '{s}'"))
            } else {
                Ok(v)
            }
        }
    };

    Ok(quote! {
        /// 汇编文本 → 指令。`mnemonic` + 操作数按 asm 模板逐段解析
        /// （字面段原样匹配，占位符段按槽解析；任一失败回退下一形状）。
        pub fn assemble(text: &str) -> Result<Inst, String> {
            let s = text.trim();
            let (mnemonic, rest) = match s.split_once(|c: char| c.is_whitespace()) {
                Some((m, r)) => (m.trim(), r.trim()),
                None => (s, ""),
            };
            match mnemonic.to_ascii_lowercase().as_str() {
                #(#arms,)*
                _ => Err(format!("unknown mnemonic '{mnemonic}'")),
            }
        }
        #parse_imm
    })
}

/// 模板段签名（同 mnemonic 去重）：字面段长度 + 占位符索引与槽类型。
fn shape_signature(info: &InstInfo) -> Result<String, String> {
    let segs = parse_template(&ops_template(info))?;
    validate_segs(&segs, info)?;
    let mut s = String::new();
    for seg in &segs {
        match seg {
            Seg::Lit(l) => s.push_str(&format!("L{}", l.len())),
            Seg::Op(n) => {
                let kind = match info.operands[*n].2.kind {
                    OperandKind::Reg => "r",
                    OperandKind::Mem => "m",
                    OperandKind::Imm => "i",
                    OperandKind::Label => "l",
                    _ => "?",
                };
                s.push_str(&format!("O{n}:{kind}"));
            }
        }
    }
    Ok(s)
}

/// 单个指令的 assemble 尝试：逐段消费操作数文本 → 全部解析成功 → Ok(Inst)。
/// 任一解析失败静默跳过（同 mnemonic 的后续形状继续尝试）。
fn gen_assemble_try(info: &InstInfo) -> Result<TokenStream, String> {
    let vn = &info.vn;
    let ops = ops_template(info);
    let segs = parse_template(&ops)?;
    validate_segs(&segs, info)?;
    let defaults = default_nontext_binds(info);
    let has_op = segs.iter().any(|s| matches!(s, Seg::Op(_)));
    if !has_op {
        // 无操作数占位符：文本必须为空（或有非文本操作数默认值）
        let fids: Vec<_> = info
            .operands
            .iter()
            .map(|(_, fid, _)| fid.clone())
            .collect();
        return Ok(quote! {
            {
                let __rest = rest;
                if __rest.is_empty() {
                    #(#defaults)*
                    return Ok(Inst::#vn { #(#fids),* });
                }
            }
        });
    }
    // 逐段生成：字面段匹配 + 操作数段文本提取
    let mut stmts: Vec<TokenStream> = Vec::new();
    let mut text_vars: Vec<(usize, syn::Ident)> = Vec::new();
    for (i, seg) in segs.iter().enumerate() {
        match seg {
            Seg::Lit(l) => {
                let l_lit = syn::LitStr::new(l, proc_macro2::Span::call_site());
                stmts.push(quote! {
                    if __ok && __rest[__pos..].starts_with(#l_lit) {
                        __pos += #l_lit.len();
                    } else {
                        __ok = false;
                    }
                });
            }
            Seg::Op(n) => {
                // 操作数文本 = 到下一字面段首次出现（或末尾）为止
                let next_lit = segs[i + 1..].iter().find_map(|s| match s {
                    Seg::Lit(l) => Some(l.clone()),
                    _ => None,
                });
                let tvar = format_ident!("__text{n}");
                // Rust if/else 分支的 let 不共享作用域——先声明再赋值

                match next_lit {
                    Some(nl) => {
                        let nl_lit = syn::LitStr::new(&nl, proc_macro2::Span::call_site());
                        stmts.push(quote! {
                            let #tvar: &str;
                            if __ok {
                                let __end = __rest[__pos..]
                                    .find(#nl_lit)
                                    .map(|i| __pos + i)
                                    .unwrap_or(__rest.len());
                                #tvar = &__rest[__pos..__end];
                                __pos = __end;
                            } else {
                                #tvar = "";
                            }
                        });
                    }
                    None => {
                        stmts.push(quote! {
                            let #tvar: &str;
                            if __ok {
                                #tvar = &__rest[__pos..];
                                __pos = __rest.len();
                            } else {
                                #tvar = "";
                            }
                        });
                    }
                }
                text_vars.push((*n, tvar));
            }
        }
    }
    // 统一解析（if-let 全部 Ok 才构造）
    let mut pats: Vec<TokenStream> = Vec::new();
    let mut exprs: Vec<TokenStream> = Vec::new();
    for (n, tvar) in &text_vars {
        let (_, fid, slot) = &info.operands[*n];
        let parse = operand_parse_expr(slot, quote! { #tvar })?;
        pats.push(quote! { Ok(#fid) });
        exprs.push(parse);
    }
    let fids: Vec<_> = info
        .operands
        .iter()
        .map(|(_, fid, _)| fid.clone())
        .collect();
    let ctor = if info.operands.is_empty() {
        quote! { Inst::#vn }
    } else {
        quote! { Inst::#vn { #(#fids),* } }
    };
    Ok(quote! {
        {
            let __rest = rest;
            let mut __pos = 0usize;
            let mut __ok = true;
            #(#stmts)*
            if __ok && __pos == __rest.len() {
                if let (#(#pats),*) = (#(#exprs),*) {
                    #(#defaults)*
                    return Ok(#ctor);
                }
            }
        }
    })
}

/// 非文本操作数（opsize）的默认值绑定语句（试点：64 位操作）。
fn default_nontext_binds(info: &InstInfo) -> Vec<TokenStream> {
    info.operands
        .iter()
        .filter(|(_, _, slot)| !is_text_operand(slot))
        .map(|(_, fid, slot)| {
            let v = match slot.kind {
                OperandKind::Opsize => quote! { 64u8 },
                _ => quote! { 0 },
            };
            quote! { let #fid = #v; }
        })
        .collect()
}

/// 文本操作数（asm 中可见）：非 Opsize 槽。
fn is_text_operand(slot: &OperandSlot) -> bool {
    !matches!(slot.kind, OperandKind::Opsize)
}

/// 操作数解析表达式（按槽类别）；返回 `Result<_, String>`（不带 `?`——
/// assemble 多形状回退需要解析失败静默跳过）。
fn operand_parse_expr(slot: &OperandSlot, tok: TokenStream) -> Result<TokenStream, String> {
    match slot.kind {
        OperandKind::Reg => {
            let group = slot
                .class
                .as_deref()
                .ok_or_else(|| "reg slot missing class".to_string())?;
            let pfn = format_ident!("__parse_reg_{}", group);
            Ok(quote! { #pfn(#tok) })
        }
        OperandKind::Mem => Ok(quote! { __parse_mem_ref(#tok) }),
        OperandKind::Imm | OperandKind::Label => Ok(quote! { __parse_imm(#tok) }),
        other => Err(format!("assemble: operand kind {other:?} unsupported")),
    }
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

/// decode 常量 guard：`((w >> off) & mask) == (value & mask)`。
fn guard_ts(bf: &Bitfield, value: u64) -> TokenStream {
    let (off, w) = single(bf);
    let mask = mask_ts(w);
    quote! { ((__w >> #off) & #mask) == (#value & #mask) }
}

/// 符号扩展：`((raw) << (64-w)) as i64 >> (64-w)`（raw 已整体加括号）。
fn sign_extend_ts(raw: TokenStream, width: u32) -> TokenStream {
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
