//! v12 语义校验：引用完整 / 位域越界 / 重复声明 / 类型约束。
//!
//! 错误信息带 TOML 路径（如 `[[instructions.MOV_R_RM]]`），便于定位。

use super::model::*;
use super::shared::parse_u64;
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
        // 立即数约束一致性：min ≤ max。
        if let (Some(lo), Some(hi)) = (s.min, s.max)
            && lo > hi
        {
            return Err(format!("{path}: min ({lo}) > max ({hi})"));
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
        // rex_w 值域由 `RexW` 枚举在反序列化期强制（未知值 → serde 报错 + 候选列表）
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
        // global_reloc 值域由 `GlobalReloc` 枚举在反序列化期强制
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

/// 全部已声明的汇编助记符（`asm` 首词）——含 families 展开（`{name}` → 变体名
/// 小写）。lowering/emit 模板的行首必须命中其一。
fn declared_mnemonics(m: &V12Model) -> BTreeSet<String> {
    let head = |asm: &str| {
        asm.trim()
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string()
    };
    let mut out: BTreeSet<String> = m.instructions.iter().map(|i| head(&i.asm)).collect();
    for f in &m.families {
        for v in &f.variants {
            let asm = v.asm.as_deref().unwrap_or(&f.asm);
            out.insert(head(&asm.replace("{name}", &v.name.to_lowercase())));
        }
    }
    out.remove("");
    out
}

/// `[[lowering]]` 校验。
///
/// 历史状态：本函数只查 `op`/`insts` 非空（14 行），于是三类写错**静默通过**：
/// 1. 助记符打错 → 直到 codegen 才报，且消息无位置；
/// 2. 占位符打错（`{iconst_lo}` 少 `12`）→ 落 fallback 装成字面量 0，
///    生成能编译但语义错的代码；
/// 3. `when` 属性名打错 → `pred::eval` 对未知属性返回 false，规则**永不命中**，
///    既不报错也不生效（最难查的一类）。
/// 现在三类都在编译期拒绝，另加同 op 完全重复规则检测。
fn validate_lowering(m: &V12Model) -> Result<(), String> {
    let mnemonics = declared_mnemonics(m);
    // (op, 规范化 when, insts) → 首次出现的下标；用于重复检测
    let mut seen: std::collections::HashMap<(String, String, Vec<String>), usize> =
        std::collections::HashMap::new();
    for (i, l) in m.lowering.iter().enumerate() {
        if l.op.trim().is_empty() {
            return Err(format!("[[lowering]] #{i}: op must not be empty"));
        }
        let path = format!("[[lowering.{}]]", l.op);
        if l.insts.is_empty() {
            return Err(format!("{path}: insts must not be empty"));
        }
        for t in &l.insts {
            let trimmed = t.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            // `{out} = INST …` 的 lhs 仅文档性，取 `=` 右侧
            let rhs = trimmed.split_once('=').map_or(trimmed, |(_, r)| r.trim());
            let head = rhs.split_whitespace().next().unwrap_or("");
            if !head.starts_with('@') && !mnemonics.contains(head) {
                return Err(format!(
                    "{path}: insts 引用了未声明的助记符 '{head}'（行: {trimmed}）"
                ));
            }
            for tok in placeholder_tokens(trimmed) {
                if !crate::v12::codegen::placeholder::is_known(&tok) {
                    return Err(format!(
                        "{path}: 未知占位符 '{tok}'（行: {trimmed}）"
                    ));
                }
            }
        }
        let when_key = match &l.when {
            None => String::new(),
            Some(v) => {
                let p = super::pred::parse(v).map_err(|e| format!("{path}.when: {e}"))?;
                let mut attrs = Vec::new();
                super::pred::attrs_of(&p, &mut attrs);
                for a in &attrs {
                    if !super::pred::PRED_ATTRS.contains(&a.as_str()) {
                        return Err(format!(
                            "{path}.when: 未知属性 '{a}'（可用：{}）——未知属性恒为假，规则永不命中",
                            super::pred::PRED_ATTRS.join("/")
                        ));
                    }
                }
                format!("{p:?}")
            }
        };
        let key = (l.op.clone(), when_key, l.insts.clone());
        if let Some(first) = seen.insert(key, i) {
            return Err(format!(
                "{path}: 与 #{first} 完全重复（同 op、同 when、同 insts）——删掉一条"
            ));
        }
    }
    validate_lowering_order(m)
}

/// 死规则检测：按**裁决序**（`V12Model::lowering_by_op`）逐对判断前序规则是否
/// 已覆盖后序规则的全部取值域。
///
/// 意义：排序改为按特异性裁决后，"被前面更宽的规则完全吃掉"就成了纯粹的作者
/// 错误（写了永不生效的规则）。判定域是每属性的闭区间集合（`eq/ne/lt/le/gt/ge/in`
/// 都能归一化），含 `or`/`not` 的规则记为 Opaque 并跳过——保守，宁可漏报也不误报。
fn validate_lowering_order(m: &V12Model) -> Result<(), String> {
    for (op, rules) in m.lowering_by_op() {
        let mut domains: Vec<super::pred::RuleDomain> = Vec::with_capacity(rules.len());
        for r in &rules {
            let pred = match &r.when {
                None => None,
                Some(v) => Some(super::pred::parse(v).map_err(|e| format!("[[lowering.{op}]]: {e}"))?),
            };
            domains.push(super::pred::domain_of(pred.as_ref()));
        }
        for (j, dj) in domains.iter().enumerate() {
            for di in domains[..j].iter() {
                if super::pred::subsumes(di, dj) {
                    return Err(format!(
                        "[[lowering.{op}]]: 第 {} 条规则是死规则——裁决序里排在它前面的规则\
                         已覆盖它的全部取值域（priority 降 / 谓词叶子数降 / 声明序升）。\
                         删掉它，或给它更高的 priority",
                        j + 1
                    ));
                }
            }
        }
    }
    Ok(())
}

/// 抽出模板里的 `{…}` token（含花括号）。
fn placeholder_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = line[i..].find('}') {
                out.push(line[i..i + end + 1].to_string());
                i += end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

// ───────────────────────── [abi] ─────────────────────────

fn validate_abi(m: &V12Model) -> Result<(), String> {
    let Some(abi) = &m.abi else {
        return Ok(());
    };
    // arg_slot / arg_class.strategy 的值域由枚举在反序列化期强制
    if let Some(align) = abi.stack_align
        && (align == 0 || align % 8 != 0)
    {
        return Err(format!(
            "[abi].stack_align must be a positive multiple of 8, got {align}"
        ));
    }
    // stack_arg_shadow：>0 且 8 的倍数（栈参数槽 8 字节对齐）。
    if let Some(shadow) = abi.stack_arg_shadow
        && (shadow == 0 || shadow % 8 != 0)
    {
        return Err(format!(
            "[abi].stack_arg_shadow must be a positive multiple of 8, got {shadow}"
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for ac in &abi.arg_class {
        if !seen.insert(ac.class) {
            return Err(format!(
                "[abi.arg_class]: duplicate class '{}'",
                ac.class.name()
            ));
        }
        // regs 为空仅当有传参策略（by-ref 等按引用策略不占用寄存器）。
        if ac.regs.is_empty() && ac.strategy.is_none() {
            return Err(format!(
                "[abi.arg_class.{}]: regs must not be empty (or declare a strategy)",
                ac.class.name()
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
