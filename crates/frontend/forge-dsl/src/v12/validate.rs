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
    if m.meta.comment_char.chars().count() != 1 {
        return Err("[meta].comment_char must be exactly one char".into());
    }
    if m.meta.label_suffix.is_empty() {
        return Err("[meta].label_suffix must not be empty".into());
    }
    if let Some(p) = &m.meta.imm_prefix
        && p.chars().count() != 1
    {
        return Err("[meta].imm_prefix must be exactly one char or absent".into());
    }
    if m.meta.directive_prefix.is_empty() {
        return Err("[meta].directive_prefix must not be empty".into());
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
        if gname.width() == 0 {
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
        for f in [&modrm.reg_field, &modrm.rm_field].into_iter().flatten() {
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
    if let Some(ps) = &conv.prefix_scan {
        const KNOWN: [&str; 6] = ["opsize16", "lock", "repe", "repne", "addr16", "rex"];
        for (i, e) in ps.iter().enumerate() {
            let path = format!("[conventions.prefix_scan][{i}]");
            if e.byte.is_none() && e.range.is_none() {
                return Err(format!("{path}: entry needs `byte` or `range`"));
            }
            if e.byte.is_some() && e.range.is_some() {
                return Err(format!("{path}: `byte` and `range` are mutually exclusive"));
            }
            if let Some(b) = e.byte
                && b > 0xFF
            {
                return Err(format!("{path}: byte 0x{b:x} exceeds one byte"));
            }
            for fx in &e.effects {
                if !KNOWN.contains(&fx.as_str()) {
                    return Err(format!(
                        "{path}: unknown effect '{fx}' (opsize16/lock/repe/repne/addr16/rex)"
                    ));
                }
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
                // class/classes 可省略 → 任意寄存器类（不推荐，会吞掉更具体的
                // 重载形式）。指定时必须是已声明的 [reg.*] 组（悬空引用报错）。
                let mut all = Vec::new();
                if let Some(c) = &s.class {
                    all.push(*c);
                }
                if let Some(cs) = &s.classes {
                    all.extend(cs.iter().cloned());
                }
                for c in &all {
                    if !m.reg.contains_key(c) {
                        return Err(format!(
                            "{path}: class '{c}' is not a declared [reg.*] group"
                        ));
                    }
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
        // 立即数约束一致性：min ≤ max；枚举值在 [min, max] 内（若都声明）。
        if let (Some(lo), Some(hi)) = (s.min, s.max) {
            if lo > hi {
                return Err(format!("{path}: min ({lo}) > max ({hi})"));
            }
            if let Some(vs) = &s.values {
                for v in vs {
                    if *v < lo || *v > hi {
                        return Err(format!(
                            "{path}: enum value {v} outside [min, max] = [{lo}, {hi}]"
                        ));
                    }
                }
            }
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
            // v12.1：opsize = 操作数序号（宽度由该操作数寄存器推导）；
            // 范围/类型校验在 codegen（vlen_ctx 需指令操作数解析后）。
            let _ = o;
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
        if f.vex.is_some() && f.evex.is_some() {
            return Err(format!(
                "[[forms.{}]]: `vex` and `evex` are mutually exclusive",
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
        if inst.asm.trim().is_empty() {
            return Err(format!(
                "[[instructions.{}]]: asm must not be empty",
                inst.name
            ));
        }
        // 操作数声明（asm 占位符内联）：解析 + 槽存在/角色合法/序号连续校验
        let (_, uses) = super::codegen::parse_asm_decl(&inst.asm, &inst.name)?;
        for op in &uses {
            if !slot_exists(m, &op.slot) {
                return Err(format!(
                    "[[instructions.{}]]: operand slot '{}' is not declared in [[operand_slots]] (asm '{}')",
                    inst.name, op.slot, inst.asm
                ));
            }
            if let Some(slot) = m.operand_slots.iter().find(|s| s.name == op.slot)
                && let (Some(r), Some(roles)) = (op.role, &slot.roles)
            {
                // 兼容性：InOut 槽支持 in/out/inout（读改写能力的子集）；
                // In 槽仅 in、Out 槽仅 out。
                let ok = roles.iter().any(|x| match x {
                    OperandRole::InOut => true,
                    OperandRole::In => r == OperandRole::In,
                    OperandRole::Out => r == OperandRole::Out,
                });
                if !ok {
                    return Err(format!(
                        "[[instructions.{}]]: operand role {:?} not allowed by slot '{}' roles {:?}",
                        inst.name, r, op.slot, roles
                    ));
                }
            }
        }
        // 定宽 form（opcode_field 存在）要求操作数数量 ≤ operand_fields
        let is_fixed = m
            .forms
            .iter()
            .find(|f| f.name == inst.form)
            .map(|f| f.opcode_field.is_some())
            .unwrap_or(false);
        if is_fixed {
            let of_len = m
                .forms
                .iter()
                .find(|f| f.name == inst.form)
                .and_then(|f| f.operand_fields.as_ref())
                .map(|f| f.len())
                .unwrap_or(0);
            if uses.len() > of_len {
                return Err(format!(
                    "[[instructions.{}]]: {} operands exceed form '{}' operand_fields count {}",
                    inst.name,
                    uses.len(),
                    inst.form,
                    of_len
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
