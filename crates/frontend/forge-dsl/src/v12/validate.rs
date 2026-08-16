//! v12 语义校验：引用完整 / 位域越界 / 重复声明 / 类型约束。
//!
//! 错误信息带 TOML 路径（如 `[[instructions.MOV_R_RM]]`），便于定位。

use super::model::*;
use std::collections::BTreeSet;

pub fn validate(m: &V12Model) -> Result<(), String> {
    validate_meta(m)?;
    validate_regs(m)?;
    validate_conventions(m)?;
    validate_operand_slots(m)?;
    validate_forms(m)?;
    validate_instructions(m)?;
    validate_families(m)?;
    validate_lowering(m)?;
    validate_abi(m)?;
    validate_emit(m)?;
    validate_spill(m)?;
    Ok(())
}

// ─────────────────────────── [meta] ───────────────────────────

fn validate_meta(m: &V12Model) -> Result<(), String> {
    let name = &m.meta.name;
    if !is_valid_meta_name(name) {
        return Err(format!(
            "[meta].name '{name}' is not a valid ISA name (letters/digits/_/-, must start with a letter or _)"
        ));
    }
    if m.meta.default_inst_width.is_some() && m.meta.variable_length {
        return Err(
            "[meta]: `default_inst_width` (fixed-width) conflicts with `variable_length = true`"
                .into(),
        );
    }
    if let Some(w) = m.meta.default_inst_width
        && w == 0
    {
        return Err("[meta].default_inst_width must be > 0".into());
    }
    if let Some(l) = m.meta.max_inst_len
        && l == 0
    {
        return Err("[meta].max_inst_len must be > 0".into());
    }
    Ok(())
}

/// 合法 ISA 名：字母/数字/`_`/`-`，首字符为字母或 `_`（派生模块名：小写 + `-`→`_`）。
fn is_valid_meta_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// ─────────────────────────── [reg.*] ───────────────────────────

fn validate_regs(m: &V12Model) -> Result<(), String> {
    if m.reg.is_empty() {
        return Err("missing [reg.*] sections: at least one register group is required".into());
    }
    for (gname, g) in &m.reg {
        if g.width == 0 {
            return Err(format!("[reg.{gname}].width must be > 0"));
        }
        match &g.names {
            Some(names) => {
                if names.is_empty() {
                    return Err(format!("[reg.{gname}].names must not be empty"));
                }
                let mut seen = BTreeSet::new();
                for n in names {
                    if n.trim().is_empty() {
                        return Err(format!("[reg.{gname}].names contains an empty name"));
                    }
                    if !seen.insert(n.clone()) {
                        return Err(format!("[reg.{gname}].names has duplicate '{n}'"));
                    }
                }
                if let Some(c) = g.count
                    && c as usize != names.len()
                {
                    return Err(format!(
                        "[reg.{gname}]: count ({c}) != names.len ({})",
                        names.len()
                    ));
                }
            }
            None => {
                let Some(c) = g.count else {
                    return Err(format!(
                        "[reg.{gname}]: either `names` or `count` is required"
                    ));
                };
                if c == 0 {
                    return Err(format!("[reg.{gname}].count must be > 0"));
                }
            }
        }
    }
    Ok(())
}

// ─────────────────────── [conventions.*] ───────────────────────

fn validate_conventions(m: &V12Model) -> Result<(), String> {
    let conv = &m.conventions;
    for (name, bf) in &conv.bitfields {
        match (&bf.offset, &bf.width, &bf.pieces) {
            (Some(off), Some(w), None) => {
                if *w == 0 {
                    return Err(format!("[conventions.bitfields.{name}].width must be > 0"));
                }
                if off + w > 64 {
                    return Err(format!(
                        "[conventions.bitfields.{name}]: offset {off} + width {w} exceeds 64 bits"
                    ));
                }
            }
            (None, None, Some(pieces)) => {
                if pieces.is_empty() {
                    return Err(format!(
                        "[conventions.bitfields.{name}].pieces must not be empty"
                    ));
                }
                // 字位域 [offset, offset+width) 两两不重叠 + 值位域 [shift, shift+width) 两两不重叠
                let mut word_ranges: Vec<(u32, u32)> = Vec::new();
                let mut value_ranges: Vec<(u32, u32)> = Vec::new();
                for (i, p) in pieces.iter().enumerate() {
                    if p.width == 0 {
                        return Err(format!(
                            "[conventions.bitfields.{name}].pieces[{i}].width must be > 0"
                        ));
                    }
                    if p.offset + p.width > 64 || p.shift + p.width > 64 {
                        return Err(format!(
                            "[conventions.bitfields.{name}].pieces[{i}]: offset/shift + width exceeds 64 bits"
                        ));
                    }
                    if word_ranges
                        .iter()
                        .any(|(s, e)| p.offset < *e && p.offset + p.width > *s)
                    {
                        return Err(format!(
                            "[conventions.bitfields.{name}].pieces: word-bit ranges overlap at piece {i}"
                        ));
                    }
                    if value_ranges
                        .iter()
                        .any(|(s, e)| p.shift < *e && p.shift + p.width > *s)
                    {
                        return Err(format!(
                            "[conventions.bitfields.{name}].pieces: value-bit ranges overlap at piece {i}"
                        ));
                    }
                    word_ranges.push((p.offset, p.offset + p.width));
                    value_ranges.push((p.shift, p.shift + p.width));
                }
            }
            _ => {
                return Err(format!(
                    "[conventions.bitfields.{name}]: declare either {{ offset, width }} or {{ pieces = [...] }}, not both/neither"
                ));
            }
        }
    }
    if let Some(modrm) = &conv.modrm {
        for f in [&modrm.reg_field, &modrm.rm_field] {
            if !conv.bitfields.contains_key(f) {
                return Err(format!(
                    "[conventions.modrm]: bitfield '{f}' is not declared in [conventions.bitfields]"
                ));
            }
        }
        let mut seen = BTreeSet::new();
        for b in &modrm.force_disp_base {
            if *b >= 16 {
                return Err(format!(
                    "[conventions.modrm].force_disp_base: {b} out of range (0..=15, REX.X-extended rm)"
                ));
            }
            if !seen.insert(*b) {
                return Err(format!(
                    "[conventions.modrm].force_disp_base: duplicate {b}"
                ));
            }
        }
    }
    if let Some(p) = &conv.opsize_prefix {
        for (k, v) in p {
            let parsed: u32 = k.parse().map_err(|_| {
                format!("[conventions.opsize_prefix]: key '{k}' is not an integer operand size")
            })?;
            if parsed == 0 {
                return Err("[conventions.opsize_prefix]: operand size must be > 0".into());
            }
            if *v > 0xFF {
                return Err(format!(
                    "[conventions.opsize_prefix.{k}]: prefix byte 0x{v:x} exceeds one byte"
                ));
            }
        }
    }
    Ok(())
}

// ──────────────────── [[operand_slots]] ────────────────────

fn validate_operand_slots(m: &V12Model) -> Result<(), String> {
    if m.operand_slots.is_empty() {
        return Err("missing [[operand_slots]]: at least one operand slot is required".into());
    }
    let mut seen = BTreeSet::new();
    for (i, s) in m.operand_slots.iter().enumerate() {
        let path = format!("[[operand_slots]] #{i} ('{}')", s.name);
        if !is_valid_meta_name(&s.name) {
            return Err(format!("{path}: invalid slot name '{}'", s.name));
        }
        if !seen.insert(s.name.clone()) {
            return Err(format!(
                "[[operand_slots]]: duplicate slot name '{}'",
                s.name
            ));
        }
        match s.kind {
            OperandKind::Reg => {
                let Some(class) = &s.class else {
                    return Err(format!(
                        "{path}: reg slot requires `class` (a [reg.*] group name)"
                    ));
                };
                if !m.reg.contains_key(class) {
                    return Err(format!(
                        "{path}: class '{class}' is not a declared [reg.*] group"
                    ));
                }
                match s.field_width {
                    None => {
                        return Err(format!(
                            "{path}: reg slot requires `field_width` (encoding bits)"
                        ));
                    }
                    Some(0) => {
                        return Err(format!("{path}: field_width must be > 0"));
                    }
                    _ => {}
                }
            }
            OperandKind::Imm => match s.width {
                None => {
                    return Err(format!("{path}: imm slot requires `width` (value bits)"));
                }
                Some(0) => {
                    return Err(format!("{path}: width must be > 0"));
                }
                _ => {}
            },
            _ => {}
        }
        if let Some(roles) = &s.roles {
            if roles.is_empty() {
                return Err(format!("{path}: roles must not be empty"));
            }
            let mut rseen = BTreeSet::new();
            for r in roles {
                if !rseen.insert(*r) {
                    return Err(format!("{path}: duplicate role {r:?}"));
                }
            }
        }
    }
    Ok(())
}

// ──────────────────────── [[forms]] ────────────────────────

fn validate_forms(m: &V12Model) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for f in &m.forms {
        if !is_valid_meta_name(&f.name) {
            return Err(format!("[[forms]]: invalid form name '{}'", f.name));
        }
        if !seen.insert(f.name.clone()) {
            return Err(format!("[[forms]]: duplicate form name '{}'", f.name));
        }
        if let Some(n) = f.opcode_bytes
            && (n == 0 || n > 4)
        {
            return Err(format!(
                "[[forms.{}]].opcode_bytes must be in 1..=4, got {n}",
                f.name
            ));
        }
        if let Some(slots) = &f.operand_slots {
            for s in slots {
                if !slot_exists(m, s) {
                    return Err(format!(
                        "[[forms.{}]]: operand slot '{s}' is not declared in [[operand_slots]]",
                        f.name
                    ));
                }
            }
        }
        if let Some(bf) = &f.opcode_field
            && !m.conventions.bitfields.contains_key(bf)
        {
            return Err(format!(
                "[[forms.{}]].opcode_field '{bf}' is not declared in [conventions.bitfields]",
                f.name
            ));
        }
        if let Some(of) = &f.operand_fields {
            for (i, bf) in of.iter().enumerate() {
                if !m.conventions.bitfields.contains_key(bf) {
                    return Err(format!(
                        "[[forms.{}]].operand_fields[{i}] '{bf}' is not declared in [conventions.bitfields]",
                        f.name
                    ));
                }
            }
        }
        if let Some(esc) = &f.escape {
            if esc.is_empty() {
                return Err(format!("[[forms.{}]].escape must not be empty", f.name));
            }
            if esc.len() > 4 {
                return Err(format!(
                    "[[forms.{}]].escape: at most 4 bytes, got {}",
                    f.name,
                    esc.len()
                ));
            }
        }
        // 变长语义键校验
        if let Some(p) = &f.prefix
            && p != "field"
            && p != "opsize"
            && parse_u64(p).is_none()
        {
            return Err(format!(
                "[[forms.{}]].prefix must be \"field\", \"opsize\" or a byte literal, got '{p}'",
                f.name
            ));
        }
        if let Some(o) = &f.opsize {
            match o {
                super::model::OpsizeSpec::Auto(a) => {
                    if a != "auto" {
                        return Err(format!(
                            "[[forms.{}]].opsize string must be \"auto\", got '{a}'",
                            f.name
                        ));
                    }
                }
                super::model::OpsizeSpec::Fixed(v) => {
                    if !matches!(v, 8 | 16 | 32 | 64) {
                        return Err(format!(
                            "[[forms.{}]].opsize must be 8/16/32/64, got {v}",
                            f.name
                        ));
                    }
                }
            }
        }
        if let Some(w) = &f.rex_w
            && w != "auto"
            && w != "field"
            && w != "always"
        {
            return Err(format!(
                "[[forms.{}]].rex_w must be \"auto\"/\"field\"/\"always\", got '{w}'",
                f.name
            ));
        }
        if let Some(imm) = f.imm
            && imm == 0
        {
            return Err(format!("[[forms.{}]].imm must be > 0", f.name));
        }
    }
    Ok(())
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

fn slot_exists(m: &V12Model, name: &str) -> bool {
    m.operand_slots.iter().any(|s| s.name == name)
}

fn form_exists(m: &V12Model, name: &str) -> bool {
    m.forms.iter().any(|f| f.name == name)
}

// ──────────────────── [[instructions]] ────────────────────

fn validate_instructions(m: &V12Model) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for inst in &m.instructions {
        if !seen.insert(inst.name.clone()) {
            return Err(format!(
                "[[instructions]]: duplicate instruction name '{}'",
                inst.name
            ));
        }
        if !form_exists(m, &inst.form) {
            return Err(format!(
                "[[instructions.{}]]: form '{}' is not declared in [[forms]]",
                inst.name, inst.form
            ));
        }
        // 操作数数量 ≤ form.operand_fields（多余的位域位置由 fields/隐式 0 填充）
        if let Some(form) = m.forms.iter().find(|f| f.name == inst.form)
            && let Some(of) = &form.operand_fields
            && inst.operands.len() > of.len()
        {
            return Err(format!(
                "[[instructions.{}]]: {} operands exceed form '{}' operand_fields count {}",
                inst.name,
                inst.operands.len(),
                inst.form,
                of.len()
            ));
        }
        // 定宽 form（opcode_field 存在）才要求 operand.field 是位域
        let is_fixed = m
            .forms
            .iter()
            .find(|f| f.name == inst.form)
            .map(|f| f.opcode_field.is_some())
            .unwrap_or(false);
        for op in &inst.operands {
            if !slot_exists(m, &op.slot) {
                return Err(format!(
                    "[[instructions.{}]]: operand slot '{}' is not declared in [[operand_slots]]",
                    inst.name, op.slot
                ));
            }
            if is_fixed
                && let Some(f) = &op.field
                && !m.conventions.bitfields.contains_key(f)
            {
                return Err(format!(
                    "[[instructions.{}]]: operand field '{f}' is not declared in [conventions.bitfields]",
                    inst.name
                ));
            }
        }
        if let Some(fields) = &inst.fields
            && is_fixed
        {
            for k in fields.keys() {
                if !m.conventions.bitfields.contains_key(k) {
                    return Err(format!(
                        "[[instructions.{}]]: fixed field '{k}' is not declared in [conventions.bitfields]",
                        inst.name
                    ));
                }
            }
        }
        // asm 完整格式：首词即 mnemonic，与 `mnemonic` 字段交叉校验（单一事实来源）
        if let Some(a) = &inst.asm {
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
        }
    }
    Ok(())
}

// ──────────────────────── [[families]] ────────────────────────

fn validate_families(m: &V12Model) -> Result<(), String> {
    let mut fseen = BTreeSet::new();
    for fam in &m.families {
        if !fseen.insert(fam.name.clone()) {
            return Err(format!(
                "[[families]]: duplicate family name '{}'",
                fam.name
            ));
        }
        if !form_exists(m, &fam.form) {
            return Err(format!(
                "[[families.{}]]: form '{}' is not declared in [[forms]]",
                fam.name, fam.form
            ));
        }
        if fam.variants.is_empty() {
            return Err(format!(
                "[[families.{}]]: at least one variant is required",
                fam.name
            ));
        }
        let mut vseen = BTreeSet::new();
        for v in &fam.variants {
            if !vseen.insert(v.name.clone()) {
                return Err(format!(
                    "[[families.{}.variants]]: duplicate variant name '{}'",
                    fam.name, v.name
                ));
            }
            if v.opcode.is_none() && v.fields.is_none() {
                return Err(format!(
                    "[[families.{}.variants.{}]]: variant needs `opcode` or `fields`",
                    fam.name, v.name
                ));
            }
        }
    }
    Ok(())
}

// ──────────────────────── [[lowering]] ────────────────────────

fn validate_lowering(m: &V12Model) -> Result<(), String> {
    for (i, l) in m.lowering.iter().enumerate() {
        if l.op.trim().is_empty() {
            return Err(format!("[[lowering]] #{i}: op must not be empty"));
        }
        if l.insts.is_empty() {
            return Err(format!("[[lowering.{}]]: insts must not be empty", l.op));
        }
    }
    Ok(())
}

// ───────────────────────── [abi] ─────────────────────────

fn validate_abi(m: &V12Model) -> Result<(), String> {
    let Some(abi) = &m.abi else {
        return Ok(());
    };
    if let Some(align) = abi.stack_align
        && (align == 0 || align % 8 != 0)
    {
        return Err(format!(
            "[abi].stack_align must be a positive multiple of 8, got {align}"
        ));
    }
    let mut seen = BTreeSet::new();
    for ac in &abi.arg_class {
        if ac.class.trim().is_empty() {
            return Err("[abi.arg_class]: class must not be empty".into());
        }
        if !seen.insert(ac.class.clone()) {
            return Err(format!("[abi.arg_class]: duplicate class '{}'", ac.class));
        }
        // regs 为空仅当有传参策略（by-ref 等按引用策略不占用寄存器）。
        if ac.regs.is_empty() && ac.strategy.is_none() {
            return Err(format!(
                "[abi.arg_class.{}]: regs must not be empty (or declare a strategy)",
                ac.class
            ));
        }
    }
    // [abi.frame]：sp 必填且非空；alloc/free 指令名非空。
    if let Some(f) = &abi.frame {
        if f.sp.trim().is_empty() {
            return Err("[abi.frame].sp must not be empty".into());
        }
        if let Some(a) = &f.alloc_inst
            && a.trim().is_empty()
        {
            return Err("[abi.frame].alloc_inst must not be empty".into());
        }
        if let Some(a) = &f.free_inst
            && a.trim().is_empty()
        {
            return Err("[abi.frame].free_inst must not be empty".into());
        }
    }
    Ok(())
}

// ───────────────────────── [emit] ─────────────────────────

fn validate_emit(m: &V12Model) -> Result<(), String> {
    let Some(em) = &m.emit else {
        return Ok(());
    };
    if let Some(p) = &em.prologue
        && p.insts.is_empty()
    {
        return Err("[emit.prologue].insts must not be empty".into());
    }
    if let Some(e) = &em.epilogue
        && e.insts.is_empty()
    {
        return Err("[emit.epilogue].insts must not be empty".into());
    }
    Ok(())
}

// ───────────────────────── [spill.*] ─────────────────────────

fn validate_spill(m: &V12Model) -> Result<(), String> {
    for (name, s) in &m.spill {
        if s.load.trim().is_empty() {
            return Err(format!("[spill.{name}].load must not be empty"));
        }
        if s.store.trim().is_empty() {
            return Err(format!("[spill.{name}].store must not be empty"));
        }
    }
    Ok(())
}
