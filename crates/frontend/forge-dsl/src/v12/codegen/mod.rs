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
    if model.meta.default_inst_width != Some(32) {
        return Err(format!(
            "v12 codegen (iteration 2) supports default_inst_width = 32 only, got {:?}",
            model.meta.default_inst_width
        ));
    }
    let infos = collect_inst_infos(model)?;
    let reg_tables = gen_reg_tables(model, &infos)?;
    let inst_enum = gen_inst_enum(&infos);
    let encode_fn = gen_encode(&infos, model)?;
    let decode_fn = gen_decode(&infos, model)?;
    let disasm_fn = gen_disassemble(&infos);
    let asm_fn = gen_assemble(&infos)?;
    Ok(quote! {
        // ── v12 生成模块（迭代 2：定宽 32 位自包含 encode/decode/asm）──
        #reg_tables
        #inst_enum
        #encode_fn
        #decode_fn
        #disasm_fn
        #asm_fn
    })
}

/// 单条指令的生成信息：form 解析 + 操作数→位域绑定 + mnemonic/asm 模板。
struct InstInfo<'a> {
    inst: &'a Instruction,
    form: &'a Form,
    vn: syn::Ident,
    mnemonic: String,
    /// 操作数绑定：(位域名, 字段标识, 槽)。
    operands: Vec<(String, syn::Ident, &'a OperandSlot)>,
}

fn collect_inst_infos<'a>(m: &'a V12Model) -> Result<Vec<InstInfo<'a>>, String> {
    let mut out = Vec::new();
    for inst in &m.instructions {
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
        if form.opcode_field.is_none() {
            return Err(format!(
                "[[instructions.{}]]: form '{}' lacks opcode_field (fixed-width required in iteration 2)",
                inst.name, inst.form
            ));
        }
        if inst.opcode.is_none() {
            return Err(format!("[[instructions.{}]]: opcode required", inst.name));
        }
        let of = form.operand_fields.as_deref().ok_or_else(|| {
            format!(
                "[[instructions.{}]]: form '{}' lacks operand_fields",
                inst.name, inst.form
            )
        })?;
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
            let field = op.field.clone().unwrap_or_else(|| of[i].clone());
            let fid = format_ident!("{}", field);
            operands.push((field, fid, slot));
        }
        let mnemonic = inst
            .mnemonic
            .clone()
            .unwrap_or_else(|| inst.name.to_lowercase());
        out.push(InstInfo {
            inst,
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
            quote! { Ok((__w as u32).to_le_bytes()) }
        } else {
            quote! { Ok((__w as u32).to_be_bytes()) }
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
        /// 编码单条指令为 4 字节（小端；32 位定宽）。
        pub fn encode(inst: &Inst) -> Result<[u8; 4], String> {
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
                return Some(#ctor);
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
        pub fn decode(bytes: &[u8]) -> Option<Inst> {
            if bytes.len() < 4 {
                return None;
            }
            let __w = #read;
            #(#stmts)*
            None
        }
    })
}

// ─────────────────────────────── disassemble ───────────────────────────────

fn gen_disassemble(infos: &[InstInfo]) -> TokenStream {
    let mut arms = Vec::new();
    for info in infos {
        let vn = &info.vn;
        let mut fmt = info.mnemonic.clone();
        let template = asm_template(info);
        let mut locals: Vec<TokenStream> = Vec::new();
        if !template.is_empty() {
            fmt.push(' ');
            let mut tpl = template.clone();
            for (i, (_, fid, slot)) in info.operands.iter().enumerate() {
                let local = format_ident!("__o{i}");
                let expr: TokenStream = match slot.kind {
                    OperandKind::Reg => {
                        let gname = format_ident!("{}_name", slot.class.as_deref().unwrap_or(""));
                        quote! { #gname(*#fid).unwrap_or("?").to_string() }
                    }
                    _ => quote! { #fid.to_string() },
                };
                locals.push(quote! { let #local = #expr; });
                tpl = tpl.replace(&format!("{{{i}}}"), &format!("{{__o{i}}}"));
            }
            fmt.push_str(&tpl);
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
    quote! {
        /// 反汇编为汇编文本。
        pub fn disassemble(inst: &Inst) -> String {
            match inst { #(#arms,)* }
        }
    }
}

// ─────────────────────────────── assemble ───────────────────────────────

/// asm 模板 token 形状（按 ", " 切分后的每个操作数 token）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenShape {
    /// 单个占位符 `{n}`。
    Simple(usize),
    /// 内存形式 `{off}({base})`。
    Mem { off: usize, base: usize },
    /// 无位移内存形式 `({base})`（offset 恒 0）。
    Mem0 { base: usize },
}

fn gen_assemble(infos: &[InstInfo]) -> Result<TokenStream, String> {
    let mut arms: Vec<TokenStream> = Vec::new();
    // 按 mnemonic 分组（保持声明序）；同 mnemonic 内按形状去重（首个形状胜出）
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
    let mem_helpers = gen_mem_helpers(infos)?;

    Ok(quote! {
        /// 汇编文本 → 指令。`mnemonic op0, op1, ...`；内存形式 `imm(reg)` / `(reg)`。
        pub fn assemble(text: &str) -> Result<Inst, String> {
            let s = text.trim();
            let (mnemonic, rest) = match s.split_once(|c: char| c.is_whitespace()) {
                Some((m, r)) => (m.trim(), r.trim()),
                None => (s, ""),
            };
            let __ops: Vec<&str> = rest
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            match mnemonic.to_ascii_lowercase().as_str() {
                #(#arms,)*
                _ => Err(format!("unknown mnemonic '{mnemonic}'")),
            }
        }
        #parse_imm
        #mem_helpers
    })
}

/// 形状签名：token 形状序列（用于同 mnemonic 去重）。
fn shape_signature(info: &InstInfo) -> Result<String, String> {
    let template = asm_template(info);
    if template.is_empty() {
        return Ok(String::new());
    }
    let mut sigs = Vec::new();
    for tok in template.split(", ") {
        sigs.push(match classify_token(tok)? {
            TokenShape::Simple(n) => format!("S{n}"),
            TokenShape::Mem { off, base } => format!("M{off},{base}"),
            TokenShape::Mem0 { base } => format!("Z{base}"),
        });
    }
    Ok(sigs.join("|"))
}

/// 单个指令的 assemble 尝试：token 数匹配 → 解析 → Ok(Inst)。
fn gen_assemble_try(info: &InstInfo) -> Result<TokenStream, String> {
    let vn = &info.vn;
    let template = asm_template(info);
    if template.is_empty() {
        return Ok(quote! { if __ops.is_empty() { return Ok(Inst::#vn); } });
    }
    let tokens: Vec<&str> = template.split(", ").collect();
    let expected = tokens.len();
    let mut let_binds: Vec<TokenStream> = Vec::new(); // `let __tN = __ops[N];`
    let mut pre: Vec<TokenStream> = Vec::new(); // 解析绑定语句（按 token 序）
    let mut ops_done = vec![false; info.operands.len()];
    for (ti, tok) in tokens.iter().enumerate() {
        let tvar = format_ident!("__t{ti}");
        let_binds.push(quote! { let #tvar = __ops[#ti]; });
        match classify_token(tok)? {
            TokenShape::Simple(n) => {
                let slot = &info.operands[n].2;
                let parse = operand_parse_expr(slot, quote! { #tvar })?;
                let fid = info.operands[n].1.clone();
                pre.push(quote! { let #fid = #parse; });
                ops_done[n] = true;
            }
            TokenShape::Mem { off, base } => {
                let base_group = info.operands[base]
                    .2
                    .class
                    .as_deref()
                    .ok_or_else(|| format!("mem base operand {} missing class", base))?;
                let mem_fn = format_ident!("__parse_mem_{}", base_group);
                let bfid = info.operands[base].1.clone();
                let ofid = info.operands[off].1.clone();
                pre.push(quote! {
                    let (__b, __o) = #mem_fn(#tvar)?;
                    let #bfid = __b;
                    let #ofid = __o;
                });
                ops_done[base] = true;
                ops_done[off] = true;
            }
            TokenShape::Mem0 { base } => {
                let base_group = info.operands[base]
                    .2
                    .class
                    .as_deref()
                    .ok_or_else(|| format!("mem base operand {} missing class", base))?;
                let mem_fn = format_ident!("__parse_mem_{}", base_group);
                let bfid = info.operands[base].1.clone();
                pre.push(quote! {
                    let (__b, __o) = #mem_fn(#tvar)?;
                    if __o != 0 {
                        return Err(format!("expected (reg) with zero offset, got '{}'", #tvar));
                    }
                    let #bfid = __b;
                });
                ops_done[base] = true;
            }
        }
    }
    if let Some(i) = ops_done.iter().position(|d| !d) {
        return Err(format!(
            "asm template '{template}' of {}: operand {i} not covered by any token",
            info.inst.name
        ));
    }
    // ctor 字段名按操作数序（值已由 pre 绑定，用字段简写）
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
        if __ops.len() == #expected {
            #(#let_binds)*
            #(#pre)*
            return Ok(#ctor);
        }
    })
}

/// 操作数解析表达式（按槽类别）。
fn operand_parse_expr(slot: &OperandSlot, tok: TokenStream) -> Result<TokenStream, String> {
    match slot.kind {
        OperandKind::Reg => {
            let group = slot
                .class
                .as_deref()
                .ok_or_else(|| "reg slot missing class".to_string())?;
            let pfn = format_ident!("__parse_reg_{}", group);
            Ok(quote! { #pfn(#tok)? })
        }
        OperandKind::Imm | OperandKind::Label => Ok(quote! { __parse_imm(#tok)? }),
        other => Err(format!(
            "assemble: operand kind {other:?} unsupported in iteration 2"
        )),
    }
}

/// 每个 mem 基址组生成 `__parse_mem_{group}`：`imm(reg)` / `(reg)` → (base, offset)。
/// 只生成模板中实际出现 Mem/Mem0 形状的组，避免无用函数警告。
fn gen_mem_helpers(infos: &[InstInfo]) -> Result<TokenStream, String> {
    let mut groups: Vec<String> = Vec::new();
    for info in infos {
        let template = asm_template(info);
        if template.is_empty() {
            continue;
        }
        for tok in template.split(", ") {
            let shape = classify_token(tok)?;
            let base = match shape {
                TokenShape::Mem { base, .. } | TokenShape::Mem0 { base } => base,
                TokenShape::Simple(_) => continue,
            };
            let group = info.operands[base].2.class.clone().ok_or_else(|| {
                format!(
                    "mem base operand {base} of {} missing class",
                    info.inst.name
                )
            })?;
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
    }
    let helpers: Vec<_> = groups
        .iter()
        .map(|g| {
            let fn_name = format_ident!("__parse_mem_{}", g);
            let reg_fn = format_ident!("__parse_reg_{}", g);
            quote! {
                fn #fn_name(s: &str) -> Result<(u32, i64), String> {
                    let t = s.trim();
                    let Some(open) = t.find('(') else {
                        return Err(format!("expected memory operand 'imm(reg)', got '{s}'"));
                    };
                    let Some(close) = t.rfind(')') else {
                        return Err(format!("expected memory operand 'imm(reg)', got '{s}'"));
                    };
                    if !t[close + 1..].is_empty() {
                        return Err(format!("trailing text after ')' in '{s}'"));
                    }
                    let off_s = t[..open].trim();
                    let reg_s = t[open + 1..close].trim();
                    let off = if off_s.is_empty() { 0 } else { __parse_imm(off_s)? };
                    let base = #reg_fn(reg_s)?;
                    Ok((base, off))
                }
            }
        })
        .collect();
    Ok(quote! { #(#helpers)* })
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

// ─────────────────────────────── asm 模板 ───────────────────────────────

/// 操作数部分模板（不含 mnemonic）：缺省 `"{0}, {1}, ..."`。
fn asm_template(info: &InstInfo) -> String {
    if let Some(a) = &info.inst.asm {
        return a.clone();
    }
    (0..info.operands.len())
        .map(|i| format!("{{{i}}}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 分类单个 token 模板。
fn classify_token(tok: &str) -> Result<TokenShape, String> {
    if let Some(rest) = tok.strip_prefix('(') {
        let inner = rest
            .strip_suffix(')')
            .ok_or_else(|| format!("unsupported asm token '{tok}' (expected '({{n}})' form)"))?;
        return Ok(TokenShape::Mem0 {
            base: parse_idx(inner, tok)?,
        });
    }
    if let Some(open) = tok.find("({") {
        // `{I}({J})`：head=`{I}`，tail（'(' 之后）=`{J})`
        let head = &tok[..open];
        let tail = &tok[open + 1..];
        let off = parse_idx(head, tok)?;
        let base_s = tail
            .strip_suffix(')')
            .ok_or_else(|| format!("bad mem token '{tok}'"))?;
        let base = parse_idx(base_s, tok)?;
        return Ok(TokenShape::Mem { off, base });
    }
    if tok.contains('(') || tok.contains(')') {
        return Err(format!("unsupported asm token '{tok}'"));
    }
    let idx = parse_idx(tok, tok)?;
    Ok(TokenShape::Simple(idx))
}

fn parse_idx(s: &str, ctx: &str) -> Result<usize, String> {
    let t = s.trim();
    let inner = t
        .strip_prefix('{')
        .and_then(|r| r.strip_suffix('}'))
        .ok_or_else(|| format!("unsupported asm token '{ctx}'"))?;
    inner
        .parse::<usize>()
        .map_err(|_| format!("unsupported asm token '{ctx}'"))
}
