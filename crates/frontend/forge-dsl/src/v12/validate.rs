//! v12 语义校验：引用完整 / 位域越界 / 重复声明 / 类型约束。
//!
//! 错误信息带 TOML 路径（如 `[[instructions.MOV_R_RM]]`），由 [`super::diag::DeclIndex`]
//! 换算成精确 `行:列`。**S1 起收集全部错误**：每个"按节"的校验器各报一条，
//! 逐条校验器（指令 / lowering / 模板）对每条声明各报一条，最后由 `Diags` 一次渲染。

use super::diag::{DeclIndex, Diags};
use super::model::*;
use super::shared::parse_u64;
use std::collections::{BTreeMap, BTreeSet};

/// 逐节收集：每节最多一条（节内逐条收集的见 `*_all`）。
fn collect(d: &mut Diags, idx: &DeclIndex, r: Result<(), String>) {
    if let Err(msg) = r {
        d.push_anchored(idx, &msg);
    }
}

/// 全部校验（收集式，一次报全）。
pub fn validate_all(m: &V12Model, idx: &DeclIndex, d: &mut Diags) {
    collect(d, idx, validate_meta(m));
    collect(d, idx, validate_regs(m));
    collect(d, idx, validate_widths(m));
    collect(d, idx, validate_conventions(m));
    collect(d, idx, validate_cond(m));
    collect(d, idx, validate_derives(m));
    collect(d, idx, validate_relocs(m));
    collect(d, idx, validate_operand_slots(m));
    collect(d, idx, validate_forms(m));
    validate_instructions_all(m, idx, d);
    collect(d, idx, validate_references(m));
    validate_lowering_all(m, idx, d);
    collect(d, idx, validate_patterns(m));
    collect(d, idx, validate_abi(m));
    validate_emit_all(m, idx, d);
    validate_spill_all(m, idx, d);
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
    if m.meta.default_inst_width == Some(0) {
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

// ─────────────────────── 类/宽度元数据（去写死）───────────────────────

/// 宽度元数据校验（2026-09-12 去「宽度写死」）：
/// 显式宽度键必须指向已声明组、为合法字节宽度；`default_opsize` 若声明则必须
/// 是 8 的倍数并与某个已声明 GPR 组的位宽一致。缺组一律 Err（fail-closed），
/// 不静默回退到 `GPR(8)`/`GPR(4)` 这类 x86 缺省。
fn validate_widths(m: &V12Model) -> Result<(), String> {
    // 显式宽度键 → 必须存在对应组（派生方法内部即校验）。
    let _ = m.main_gpr_class()?;
    let _ = m.main_fpr_class()?;
    let _ = m.addr_class()?;
    let _ = m.value_gpr_class()?;
    let _ = m.value_fpr_class()?;
    // 向量 by-ref 阈值：`strategy`/`limit` 成对性 + 唯一性 + 8 的倍数
    // （此前只在 `gen_abi` 生成期检查，读 TOML 的人拿不到早期反馈）。
    let _ = m.vector_by_ref_limit_bytes()?;
    if m.slot_bytes()? == 0 {
        return Err("[stack].slot must be > 0".into());
    }
    if m.fp_overhead_bytes()? == 0 && m.stack.as_ref().and_then(|s| s.fp_save).is_some() {
        return Err("[stack].fp_save must be > 0".into());
    }
    if m.stack.as_ref().and_then(|s| s.slot).is_some() && m.slot_bytes()? == 0 {
        return Err("[stack].slot must be > 0".into());
    }
    if m.stack.as_ref().and_then(|s| s.align).is_some() && m.stack_align()? == 0 {
        return Err("[stack].align must be > 0".into());
    }
    // `[abi.stack_args]`：基址只能是 fp|sp；槽数与步长必须 > 0。
    if let Some(sa) = m.abi.as_ref().and_then(|a| a.stack_args.as_ref()) {
        for (key, base) in [
            ("callee_base", sa.callee_base.as_ref()),
            ("caller_base", sa.caller_base.as_ref()),
        ] {
            if let Some(base) = base
                && base != "fp"
                && base != "sp"
            {
                return Err(format!(
                    "[abi.stack_args].{key} = \"{base}\" 非法（只能是 \"fp\" 或 \"sp\"）"
                ));
            }
        }
        if sa.first_offset_slots == Some(0) {
            return Err("[abi.stack_args].first_offset_slots must be > 0".into());
        }
        if sa.stride_slots == Some(0) {
            return Err("[abi.stack_args].stride_slots must be > 0".into());
        }
    }
    for (key, w) in [
        ("default_gpr_width", m.meta.default_gpr_width),
        ("default_fpr_width", m.meta.default_fpr_width),
        ("addr_width", m.meta.addr_width),
        ("value_gpr_width", m.meta.value_gpr_width),
        ("value_fpr_width", m.meta.value_fpr_width),
    ] {
        if let Some(w) = w
            && w == 0
        {
            return Err(format!("[meta].{key} must be > 0"));
        }
    }
    // 向量档位：升序、非 0、去重（生成代码按"最小的 ≥ 请求字节数的档位"选择）。
    if let Some(tiers) = &m.meta.vector_tiers {
        if tiers.is_empty() {
            return Err("[meta].vector_tiers must not be empty".into());
        }
        let mut prev = 0u16;
        for t in tiers {
            if *t == 0 {
                return Err("[meta].vector_tiers must be > 0".into());
            }
            if *t <= prev {
                return Err(format!(
                    "[meta].vector_tiers must be strictly ascending (got {tiers:?})"
                ));
            }
            prev = *t;
        }
    }
    if let Some(bits) = m.meta.default_opsize {
        if bits == 0 || bits % 8 != 0 {
            return Err(format!(
                "[meta].default_opsize = {bits} 必须是 8 的倍数（单位：位）"
            ));
        }
        let want = (bits / 8) as u16;
        if !m
            .reg
            .keys()
            .any(|rc| matches!(rc, RegClass::GPR(w) if *w == want))
        {
            return Err(format!(
                "[meta].default_opsize = {bits}（{want} 字节）没有对应的 [reg.gpr{want}] 组"
            ));
        }
    }
    // 结构化寄存器名字段必须能解析到**已声明组**内的名字（fail-closed）：
    // 历史实现用 `filter_map`/`unwrap_or_default` 静默丢弃未知名字 ⇒ sp/fp 落回
    // 索引 0、scratch/callee_saved 缺失（regalloc 会分配被占用寄存器）。
    // 自由模板（lowering/emit 的 `insts`）不在这里猜——它含助记符与内存语法。
    validate_reg_names(m)?;
    validate_spill_coverage(m)?;
    validate_types(m)?;
    Ok(())
}

/// `[types]` 显式映射校验（B2）：类型名合法、目标组已声明（在
/// `explicit_type_map` 里查）、映射的类宽 ≥ 该类型的字节宽（`ptr` 按 ISA
/// 地址宽；`void` 只能 unsupported）——防止把 `i64` 映射到 1 字节组这类
/// "看起来能用、实际截断"的配置。**显式条目优先于通用值池规则**。
fn validate_types(m: &V12Model) -> Result<(), String> {
    let addr_w = m.addr_class()?.width();
    for (ty, target) in m.explicit_type_map()? {
        let Some(rc) = target else {
            // 显式 unsupported：合法（把"宿主会拒绝"变成"ISA 明确声明不支持"）。
            continue;
        };
        if ty == "void" {
            return Err("[types].void: void 不承载寄存器，只能写 \"unsupported\"".into());
        }
        // 向量类型：类宽必须 ≥ 元素字节数（tier 语义），其余按标量字节宽。
        let need = match ty.as_str() {
            "ptr" => Some(addr_w),
            other => crate::v12::model::type_name_bytes(other),
        };
        if let Some(need) = need
            && rc.width() < need
        {
            return Err(format!(
                "[types].{ty} = \"{rc}\": 类宽 {} 字节 < 该类型 {} 字节（会静默截断）",
                rc.width(),
                need
            ));
        }
    }
    Ok(())
}

/// 溢出模板覆盖校验（R5）：**每个比宿主浮点值池更宽的浮点/向量类**都必须有
/// 对应的 `[spill.FPR<bytes>]` 模板——生成器的 FPR 溢出按宽度档分派，缺档会在
/// **编译期**（IR 编译时）才报 `Unsupported`，读 TOML 的人很难定位。
///
/// 这里把它提前到 DSL 校验期：点名"哪个类需要哪个键"。（没有浮点组、或只有
/// 标量宽度的 ISA 不需要任何档位。）
fn validate_spill_coverage(m: &V12Model) -> Result<(), String> {
    let scalar = m.value_fpr_class()?.map(|c| c.width()).unwrap_or(0);
    // 有 `[spill.FPR]` 才谈档位（否则 FPR 溢出整条路径本来就是 no-op）。
    let Some(_base) = m.spill.get("FPR") else {
        return Ok(());
    };
    let declared_tiers: BTreeSet<u16> = m
        .spill
        .keys()
        .filter_map(|k| k.strip_prefix("FPR"))
        .filter_map(|n| n.parse::<u16>().ok())
        .collect();
    for rc in m.reg.keys() {
        let w = match rc {
            RegClass::FPR(w) | RegClass::VEC(w) => *w,
            _ => continue,
        };
        if w <= scalar || declared_tiers.contains(&w) {
            continue;
        }
        return Err(format!(
            "[reg.{rc}] 宽 {w} 字节 > 浮点值池 {scalar} 字节，但缺少 [spill.FPR{w}] 溢出模板\
             ——生成器按宽度档分派，缺档会在 IR 编译期才报 Unsupported（此处提前点名）"
        ));
    }
    Ok(())
}

/// 结构化寄存器名引用校验：`[abi]` 的 sp/fp/scratch/reserved/callee_saved/
/// ret_regs/call_ret_reg/call_clobbers/implicit_regs/arg_class.regs 与
/// `[spill.*].base` 必须在某个已声明寄存器组内。
fn validate_reg_names(m: &V12Model) -> Result<(), String> {
    let mut declared: BTreeSet<String> = BTreeSet::new();
    for (rc, g) in &m.reg {
        for n in super::shared::group_names(g).map_err(|e| format!("[reg.{rc}]: {e}"))? {
            declared.insert(n);
        }
    }
    let check = |names: &[String], key: &str| -> Result<(), String> {
        for n in names {
            if !declared.contains(n.as_str()) {
                return Err(format!(
                    "{key}: 物理寄存器名 \"{n}\" 不在任何已声明 [reg.*] 组内\
                     （生成期 fail-closed；历史实现静默丢弃 ⇒ 该寄存器不被排除/不被保存）"
                ));
            }
        }
        Ok(())
    };
    if let Some(abi) = &m.abi {
        check(&abi.scratch, "[abi].scratch")?;
        check(&abi.reserved, "[abi].reserved")?;
        check(&abi.ret_regs, "[abi].ret_regs")?;
        check(
            &abi.call_clobbers.clone().unwrap_or_default(),
            "[abi].call_clobbers",
        )?;
        if let Some(r) = &abi.call_ret_reg {
            check(std::slice::from_ref(r), "[abi].call_ret_reg")?;
        }
        if let Some(cs) = &abi.callee_saved {
            check(&cs.gpr, "[abi.callee_saved].gpr")?;
        }
        for ac in &abi.arg_class {
            check(&ac.regs, "[abi.arg_class].regs")?;
        }
        if let Some(f) = &abi.frame {
            check(std::slice::from_ref(&f.sp), "[abi.frame].sp")?;
            if let Some(fp) = &f.fp {
                check(std::slice::from_ref(fp), "[abi.frame].fp")?;
            }
        }
    }
    for (name, t) in &m.spill {
        if let Some(b) = &t.base {
            check(std::slice::from_ref(b), &format!("[spill.{name}].base"))?;
        }
    }
    for inst in &m.instructions {
        if let Some(ir) = &inst.implicit_regs {
            check(
                ir,
                &format!("[[instructions.{name}]].implicit_regs", name = inst.name),
            )?;
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
                // 字侧（offset）无上限——字长是 ISA 数据、由字节数组承载；
                // 只有**单个位域的值宽**受 u64/i64 值表示限制。
                if *w > 64 {
                    return Err(format!(
                        "[conventions.bitfields.{name}].width = {w} 超过值表示上限 64 位\
                         （位域值承载在 u64 常量键与 i64 操作数上；字长本身无上限，\
                         可拆成多个 ≤64 位的域）"
                    ));
                }
                let _ = off;
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
                    // 字侧 piece.offset 无上限；值侧 shift+width ≤ 64（值表示上限）。
                    if p.width > 64 || p.shift + p.width > 64 {
                        return Err(format!(
                            "[conventions.bitfields.{name}].pieces[{i}]: width/shift+width 超过值表示上限 64 位\
                             （字长无上限，可拆更多 piece）"
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
    // 定宽 ISA：每个位域必须落在指令字内（字宽 = ISA 数据；历史实现把
    // "定宽 = 32 位"写死在 codegen，超宽位域会被静默移位出字/截断）。
    if !m.meta.variable_length
        && let Some(bits) = m.meta.default_inst_width
    {
        for (name, bf) in &conv.bitfields {
            let hi = match &bf.pieces {
                Some(ps) => ps.iter().map(|p| p.offset + p.width).max(),
                None => bf.offset.zip(bf.width).map(|(o, w)| o + w),
            };
            if let Some(hi) = hi
                && hi > bits
            {
                return Err(format!(
                    "[conventions.bitfields.{name}]: 位域最高位 {hi} 超出指令字宽 {bits} 位\
                     （[meta].default_inst_width；请缩短位域或加宽指令字）"
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
    if let Some(mt) = &conv.mem {
        let items = super::codegen::mem::parse_mem_template(&mt.template)
            .map_err(|e| format!("[conventions.mem]: {e}"))?;
        super::codegen::mem::validate_mem_template(&items)
            .map_err(|e| format!("[conventions.mem]: {e}"))?;
    }
    Ok(())
}

/// `[[derive]]` 校验（v18 S3f 派生谓词属性）。
///
/// 解析期已查过：名称非空/唯一/不与核心属性重名、`expr` 语法合法。这里查语义：
/// **派生只能引用核心属性**（不支持派生引用派生——见 `expand_derives` 的理由），
/// 且 `expr` 里出现的每个属性名都必须是核心属性。
fn validate_derives(m: &V12Model) -> Result<(), String> {
    for d in &m.derive {
        let Some(pred) = m.derived_preds.get(&d.name) else {
            continue;
        };
        let mut attrs = Vec::new();
        super::pred::attrs_of(pred, &mut attrs);
        for a in &attrs {
            if !super::pred::PRED_ATTRS.contains(&a.as_str()) {
                let hint = if m.derive.iter().any(|o| &o.name == a) {
                    "（派生不能引用派生——把这条条件直接写进 expr）"
                } else {
                    ""
                };
                return Err(format!(
                    "[[derive.{}]] expr: 未知属性 '{a}'{hint}（可用：{}）",
                    d.name,
                    super::pred::PRED_ATTRS.join(" / ")
                ));
            }
        }
    }
    Ok(())
}

/// `[[reloc]]` 校验（v18 S3d 重定位数据化）。
///
/// - 表项名非空、唯一；
/// - `slot` 必须已声明且是 `kind = "imm"` 的槽（absolute 的补丁宽度 = 槽宽；
///   pc_relative 的 fixup 落在指令起始，槽只是"这条指令把重定位值承载在哪"）；
/// - 指令的 `reloc = "<名>"` 必须指向已声明表项，且该指令确实有那个槽的操作数。
fn validate_relocs(m: &V12Model) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for r in &m.reloc {
        if r.name.trim().is_empty() {
            return Err("[[reloc]]: name must not be empty".into());
        }
        if !seen.insert(r.name.clone()) {
            return Err(format!("[[reloc.{}]]: 重定位名重复", r.name));
        }
        let Some(slot) = m.operand_slots.iter().find(|s| s.name == r.slot) else {
            return Err(format!(
                "[[reloc.{}]]: 绑定的操作数槽 '{}' 未在 [[operand_slots]] 声明",
                r.name, r.slot
            ));
        };
        if slot.kind != OperandKind::Imm {
            return Err(format!(
                "[[reloc.{}]]: 绑定的槽 '{}' 必须是 imm 槽（重定位值要写进一个数值槽），实际是 {:?}",
                r.name, r.slot, slot.kind
            ));
        }
    }
    for inst in &m.instructions {
        let Some(name) = inst.reloc.as_deref() else {
            continue;
        };
        let Some(def) = m.reloc.iter().find(|r| r.name == name) else {
            return Err(format!(
                "[[instructions.{}]]: reloc '{name}' 未在 [[reloc]] 声明（可用：{}）",
                inst.name,
                if m.reloc.is_empty() {
                    "<空>".to_string()
                } else {
                    m.reloc
                        .iter()
                        .map(|r| r.name.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ")
                }
            ));
        };
        let ops = super::codegen::parse_asm_decl(&inst.asm, inst.ops.as_deref(), &inst.name)?.0;
        if !ops.iter().any(|o| o.slot == def.slot) {
            return Err(format!(
                "[[instructions.{}]]: reloc '{name}' 绑定的槽 '{}' 不在该指令的操作数里（asm '{}'）",
                inst.name, def.slot, inst.asm
            ));
        }
    }
    Ok(())
}

/// `[conventions.cond]` 校验（v18 S3b 条件码数据化）。
///
/// 这张表同时服务三处（汇编解析名、反汇编渲染名、lowering 的 `{cc}`），因此校验也
/// 三面都盖：
///
/// 1. 键非空、`code` 落在 4 位条件字段内（0..=15）；
/// 2. `ir` 必须是 [`IR_INT_COND_NAMES`] 之一，且**每个 IR 条件只能被映射一次**
///    （否则 `{cc}` 该取哪个编码就没有答案）；
/// 3. **用到 `cond` 槽却没有表** ⇒ 报错（v18 S3b 起不再回退 x86 的 16 项表）；
///    **用到 `{cc}` 却没把 10 个 IR 条件映射全** ⇒ 报错（缺的那个会在运行期
///    静默退化成 0 = 溢出条件，是最难查的一类错）。
fn validate_cond(m: &V12Model) -> Result<(), String> {
    let table = m.conventions.cond.as_ref();
    if let Some(t) = table {
        if t.is_empty() {
            return Err(
                "[conventions.cond]: 条件码表不能为空（要么整节不写，要么至少一条）".into(),
            );
        }
        let mut owner: BTreeMap<&str, &str> = BTreeMap::new();
        for (name, e) in t {
            if name.trim().is_empty() {
                return Err("[conventions.cond]: 条件名不能为空".into());
            }
            if e.code > 15 {
                return Err(format!(
                    "[conventions.cond.{name}]: code {} 超出条件字段宽度（4 位，0..=15）",
                    e.code
                ));
            }
            if let Some(ir) = e.ir_condition(name.as_str()) {
                if !IR_INT_COND_NAMES.contains(&ir) {
                    return Err(format!(
                        "[conventions.cond.{name}].ir '{ir}' 不是 IR 整数条件名（可用：{}）",
                        IR_INT_COND_NAMES.join(" / ")
                    ));
                }
                if let Some(prev) = owner.insert(ir, name) {
                    return Err(format!(
                        "[conventions.cond]: IR 条件 '{ir}' 被 '{prev}' 与 '{name}' 重复映射\
                         （每个 IR 条件只能映射到一个编码）"
                    ));
                }
            }
        }
    }
    // `cond` 槽（汇编侧）需要表
    if table.is_none()
        && let Some(slot) = m.operand_slots.iter().find(|s| s.kind == OperandKind::Cond)
    {
        return Err(format!(
            "[conventions.cond]: 操作数槽 '{}' 的 kind = \"cond\" 需要条件码表——\
             声明 [conventions.cond]（键 = 本 ISA 汇编可见的条件名）",
            slot.name
        ));
    }
    // `{cc}`（lowering 侧）需要**全部** IR 整数条件
    if m.lowering
        .iter()
        .any(|r| r.insts.iter().any(|s| s.contains("{cc}")))
    {
        let missing: Vec<&str> = IR_INT_COND_NAMES
            .iter()
            .copied()
            .filter(|n| !m.cond_ir_codes().iter().any(|(k, _)| k == n))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "[[lowering]]: 用了 `{{cc}}`（当前 IR 条件 → 本 ISA 编码）但 [conventions.cond] \
                 没把所有 IR 整数条件映射全——缺 {}（每条用 ir = \"<条件名>\" 指出它实现哪个条件）",
                missing.join(" / ")
            ));
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
        if let Some(bf) = &f.keys.opcode_field
            && !m.conventions.bitfields.contains_key(bf)
        {
            return Err(format!(
                "[[forms.{}]].opcode_field '{bf}' is not declared in [conventions.bitfields]",
                f.name
            ));
        }
        if let Some(of) = &f.keys.operand_fields {
            for (i, bf) in of.iter().enumerate() {
                if !m.conventions.bitfields.contains_key(bf) {
                    return Err(format!(
                        "[[forms.{}]].operand_fields[{i}] '{bf}' is not declared in [conventions.bitfields]",
                        f.name
                    ));
                }
            }
        }
        if let Some(esc) = &f.keys.escape {
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
        if let Some(p) = &f.keys.prefix
            && p != "field"
            && p != "opsize"
            && parse_u64(p).is_none()
        {
            return Err(format!(
                "[[forms.{}]].prefix must be \"field\", \"opsize\" or a byte literal, got '{p}'",
                f.name
            ));
        }
        if let Some(o) = &f.keys.opsize {
            // v12.1：opsize = 操作数序号（宽度由该操作数寄存器推导）；
            // 范围/类型校验在 codegen（vlen_ctx 需指令操作数解析后）。
            let _ = o;
        }
        // rex_w 值域由 `RexW` 枚举在反序列化期强制（未知值 → serde 报错 + 候选列表）
        if f.keys.vex.is_some() && f.keys.evex.is_some() {
            return Err(format!(
                "[[forms.{}]]: `vex` and `evex` are mutually exclusive",
                f.name
            ));
        }
        if let Some(imm) = f.keys.imm
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

/// 逐条收集：角色唯一性 + 重名（整表层）各报一次，然后**每条指令**各报一条。
///
/// 重名诊断由 `DeclIndex` 自动附注"同名声明也出现在 行:列"（S1 新增能力）。
fn validate_instructions_all(m: &V12Model, idx: &DeclIndex, d: &mut Diags) {
    // 角色全 ISA 唯一：同一个语义位置有两条候选时生成器（collect_inst_infos
    // 的 .find）只会取第一条，静默丢掉另一条——直接拒绝。[[instructions]] 与
    // `[[templates]]` 展开在解析期完成，此处看到的就是完整指令表。
    let mut role_owner: std::collections::BTreeMap<Role, &str> = Default::default();
    for inst in &m.instructions {
        for r in &inst.roles {
            if let Some(prev) = role_owner.insert(*r, inst.name.as_str()) {
                d.push_anchored(
                    idx,
                    &format!(
                        "[[instructions.{}]]: 角色 \"{r}\" 已由 {prev} 声明——每个角色全 ISA 唯一",
                        inst.name
                    ),
                );
            }
        }
    }

    let mut seen = BTreeSet::new();
    for inst in &m.instructions {
        if !seen.insert(inst.name.clone()) {
            // 重复声明：诊断落在**后出现**的那一处，并附注首次声明（索引里的同名位置）。
            d.push_duplicate(
                idx,
                &format!(
                    "[[instructions.{}]]: duplicate instruction name '{}'",
                    inst.name, inst.name
                ),
            );
        }
    }
    for inst in &m.instructions {
        if let Err(msg) = check_instruction(m, inst) {
            // 模板展开出的实例：把错误锚回**模板声明**（`[[templates.X]]`），
            // 而不是一个在源里根本不存在的 `[[instructions.实例名]]`。
            let msg = match &inst.from_template {
                Some(t) => msg
                    .replacen(
                        &format!("[[instructions.{}]]", inst.name),
                        &format!("[[templates.{t}]]"),
                        1,
                    )
                    .replacen("[[instructions]]", &format!("[[templates.{t}]]"), 1),
                None => msg,
            };
            d.push_anchored(idx, &msg);
        }
    }
}

/// 单条指令的全部校验（原 `validate_instructions` 的循环体）。
fn check_instruction(m: &V12Model, inst: &Instruction) -> Result<(), String> {
    if let Some(f) = &inst.form
        && !form_exists(m, f)
    {
        return Err(format!(
            "[[instructions.{}]]: form '{f}' is not declared in [[forms]]",
            inst.name
        ));
    }
    // `reloc` 的取值域由 `validate_relocs` 校验（名字须在 [[reloc]] 表里）
    let asm = inst.asm.clone();
    if asm.trim().is_empty() {
        return Err(format!(
            "[[instructions.{}]]: asm must not be empty",
            inst.name
        ));
    }
    // 操作数声明（asm 占位符内联）：解析 + 槽存在/角色合法/序号连续校验
    let (uses, _) = super::codegen::parse_asm_decl(&asm, inst.ops.as_deref(), &inst.name)?;
    for op in &uses {
        if !slot_exists(m, &op.slot) {
            return Err(format!(
                "[[instructions.{}]]: operand slot '{}' is not declared in [[operand_slots]] (asm '{}')",
                inst.name, op.slot, asm
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
    // 定宽（opcode_field 存在）要求操作数数量 ≤ operand_fields。
    // 键取 form 预设 ⊕ 指令级覆盖（与 codegen 的 `EncKeys::over` 同语义）。
    let preset = inst
        .form
        .as_ref()
        .and_then(|n| m.forms.iter().find(|f| &f.name == n))
        .map(|f| f.keys.clone())
        .unwrap_or_default();
    let enc = inst.enc.over(&preset);
    let is_fixed = enc.opcode_field.is_some();
    if is_fixed {
        let of_len = enc.operand_fields.as_ref().map(|f| f.len()).unwrap_or(0);
        if uses.len() > of_len {
            return Err(format!(
                "[[instructions.{}]]: {} operands exceed operand_fields count {}",
                inst.name,
                uses.len(),
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
    // 编码信息下限：至少要有一个**编码来源**（`opcode`/`fields`，或任一变长编码键）。
    // 原先只对 `[[families]]` 变体检查；S2c 统一机制后族/模板/指令同一套，故对所有
    // 指令生效——什么都不编码的指令会生成"空编码臂"，是静默错。
    // 注意 `opcode_field`/`operand_fields`/`prefix`/`opsize`/`rex_w` 只是**修饰**：
    // 只有它们（form 预设给的）而没有主编码，仍属缺编码信息。
    let has_enc = inst.opcode.is_some()
        || inst.fields.is_some()
        || enc.opcode_reg.is_some()
        || enc.imm.is_some()
        || enc.escape.is_some()
        || enc.modrm.is_some()
        || enc.modrm_fixed.is_some()
        || enc.vex.is_some()
        || enc.evex.is_some();
    if !has_enc {
        return Err(format!(
            "[[instructions.{}]]: 指令缺少编码信息——至少要给 `opcode`/`fields`，\
             或 `opcode_reg`/`modrm`/`vex`/`evex`/`imm` 之一",
            inst.name
        ));
    }
    Ok(())
}

// ──────────────────────── [[lowering]] ────────────────────────

/// 全部指令名（含 `[[templates]]` 展开出的实例——展开发生在解析期，此处看到的
/// 就是完整指令表）。lowering/pattern/emit 模板行首可引用其任一。
fn declared_names(m: &V12Model) -> BTreeSet<String> {
    m.instructions.iter().map(|i| i.name.clone()).collect()
}

/// 引用名全集 = 指令名 ∪ 各指令声明的 `ref`。lowering/pattern/emit 行首必须命中其一。
fn declared_refs(m: &V12Model) -> BTreeSet<String> {
    let mut out = declared_names(m);
    for i in &m.instructions {
        if let Some(r) = &i.reference {
            out.insert(r.clone());
        }
    }
    out
}

/// `ref` 校验：非空、不与指令名冲突（`ref` 与指令名同池——都是 lowering 行首的引用名）。
fn validate_references(m: &V12Model) -> Result<(), String> {
    let names = declared_names(m);
    for i in &m.instructions {
        let Some(r) = &i.reference else { continue };
        if r.trim().is_empty() {
            return Err(format!("[[instructions.{}]]: ref 不能为空", i.name));
        }
        if names.contains(r) {
            return Err(format!(
                "[[instructions.{}]]: ref '{r}' 与指令名冲突（引用名与指令名同池）",
                i.name
            ));
        }
    }
    Ok(())
}

/// `[[lowering]]` 校验（逐条收集 + 死规则检测）。
///
/// 历史状态：本函数只查 `op`/`insts` 非空（14 行），于是三类写错**静默通过**：
/// 1. 助记符打错 → 直到 codegen 才报，且消息无位置；
/// 2. 占位符打错（`{iconst_lo}` 少 `12`）→ 落 fallback 装成字面量 0，
///    生成能编译但语义错的代码；
/// 3. `when` 属性名打错 → `pred::eval` 对未知属性返回 false，规则**永不命中**，
///    既不报错也不生效（最难查的一类）。
///
/// 现在三类都在编译期拒绝，另加同 op 完全重复规则检测；S1 起**逐条收集**。
fn validate_lowering_all(m: &V12Model, idx: &DeclIndex, d: &mut Diags) {
    let refs = declared_refs(m);
    // `when` 可用属性 = 核心属性 + `[[derive]]` 名（v18 S3f）
    let pred_attrs = m.pred_attr_names();
    // (op, 规范化 when, insts) → 首次出现的下标；用于重复检测
    let mut seen: std::collections::HashMap<(String, String, Vec<String>), usize> =
        std::collections::HashMap::new();
    for (i, l) in m.lowering.iter().enumerate() {
        if l.op.trim().is_empty() {
            d.push_anchored(idx, &format!("[[lowering]] #{i}: op must not be empty"));
            continue;
        }
        let path = format!("[[lowering.{}]]", l.op);
        if l.insts.is_empty() {
            d.push_anchored(idx, &format!("{path}: insts must not be empty"));
            continue;
        }
        if let Err(msg) = validate_inst_lines(&path, &l.insts, &refs, &[]) {
            d.push_anchored(idx, &msg);
        }
        let when_key = match &l.when {
            None => String::new(),
            Some(v) => match super::pred::parse(v) {
                Err(e) => {
                    d.push_anchored(idx, &format!("{path}.when: {e}"));
                    continue;
                }
                Ok(p) => {
                    let mut attrs = Vec::new();
                    super::pred::attrs_of(&p, &mut attrs);
                    let mut ok = true;
                    for a in &attrs {
                        if !pred_attrs.iter().any(|x| x == a) {
                            d.push_anchored(
                                idx,
                                &format!(
                                    "{path}.when: 未知属性 '{a}'（可用：{}）——未知属性恒为假，规则永不命中",
                                    pred_attrs.join("/")
                                ),
                            );
                            ok = false;
                        }
                    }
                    if !ok {
                        continue;
                    }
                    format!("{p:?}")
                }
            },
        };
        let key = (l.op.clone(), when_key, l.insts.clone());
        if let Some(first) = seen.insert(key, i) {
            d.push_anchored(
                idx,
                &format!("{path}: 与 #{first} 完全重复（同 op、同 when、同 insts）——删掉一条"),
            );
        }
    }
    collect(d, idx, validate_lowering_order(m));
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
                Some(v) => {
                    Some(super::pred::parse(v).map_err(|e| format!("[[lowering.{op}]]: {e}"))?)
                }
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

/// 校验一段发射模板行：助记符已声明 + 占位符已知。`extra_known` 是额外允许
/// 的 `{...}` token（pattern 的叶变量，codegen 会改写成 `{N}` 编号操作数）。
fn validate_inst_lines(
    path: &str,
    insts: &[String],
    refs: &BTreeSet<String>,
    extra_known: &[String],
) -> Result<(), String> {
    for t in insts {
        let trimmed = t.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // `{out} = INST …` 的 lhs 仅文档性，取 `=` 右侧
        let rhs = trimmed.split_once('=').map_or(trimmed, |(_, r)| r.trim());
        let head = rhs.split_whitespace().next().unwrap_or("");
        if !head.starts_with('@') && !refs.contains(head) {
            return Err(format!(
                "{path}: insts 引用了未声明的指令/别名 '{head}'（行: {trimmed}）"
            ));
        }
        for tok in placeholder_tokens(trimmed) {
            let known = crate::v12::codegen::placeholder::is_known(&tok)
                || extra_known.iter().any(|k| k == &tok);
            if !known {
                return Err(format!("{path}: 未知占位符 '{tok}'（行: {trimmed}）"));
            }
        }
    }
    Ok(())
}

// ──────────────────────── [[pattern]] ────────────────────────

/// `[[pattern]]` 校验：匹配树解析 / Opcode 合法性（禁 payload 与特判 op）/
/// 叶变量唯一 / insts 模板 / when 属性名。
fn validate_patterns(m: &V12Model) -> Result<(), String> {
    let refs = declared_refs(m);
    for (i, p) in m.pattern.iter().enumerate() {
        let path = format!("[[pattern]] #{i}");
        let tree =
            super::match_tree::parse(&p.r#match).map_err(|e| format!("{path}.match: {e}"))?;

        // Opcode 合法性：仅单元变体可作匹配节点——Fcmp/Icmp 带 payload（运行期
        // 按完整 Opcode 值比较，无法结构匹配）；Copy/Nop 在 forward 循环 pattern
        // 分发之前就被 continue 特判。未知名交给 codegen `Opcode::#name` 报错。
        let mut ops = Vec::new();
        super::match_tree::op_names(&tree, &mut ops);
        for op in &ops {
            if matches!(op.as_str(), "Fcmp" | "Icmp" | "Copy" | "Nop") {
                return Err(format!(
                    "{path}.match: Op '{op}' 不能作模式节点（Fcmp/Icmp 带 payload，\
                     Copy/Nop 被 forward 循环特判）"
                ));
            }
        }

        // 叶变量：非空 + 唯一（重复变量会让 {名字}→{N} 改写歧义）。
        let mut vars = Vec::new();
        super::match_tree::leaf_vars(&tree, &mut vars);
        if vars.is_empty() {
            return Err(format!("{path}.match: 匹配树没有叶变量（纯常量树无意义）"));
        }
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for v in &vars {
            if !seen.insert(v) {
                return Err(format!("{path}.match: 叶变量 '{v}' 重复出现"));
            }
        }

        // insts 非空 + 助记符 declared + 占位符已知（叶变量视为 {N} 改写后的编号操作数）。
        if p.insts.is_empty() {
            return Err(format!("{path}: insts must not be empty"));
        }
        let extra: Vec<String> = vars.iter().map(|v| format!("{{{v}}}")).collect();
        validate_inst_lines(&path, &p.insts, &refs, &extra)?;

        // when 属性名 ∈ 核心属性 ∪ [[derive]]（与 lowering 一致——未知属性恒假，模式永不命中）。
        if let Some(w) = &p.when {
            let pred = super::pred::parse(w).map_err(|e| format!("{path}.when: {e}"))?;
            let mut attrs = Vec::new();
            super::pred::attrs_of(&pred, &mut attrs);
            let known = m.pred_attr_names();
            for a in &attrs {
                if !known.iter().any(|x| x == a) {
                    return Err(format!(
                        "{path}.when: 未知属性 '{a}'（可用：{}）",
                        known.join("/")
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
        if bytes[i] == b'{'
            && let Some(end) = line[i..].find('}')
        {
            out.push(line[i..i + end + 1].to_string());
            i += end + 1;
            continue;
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

    // [abi.stack_args].shadow_bytes：>0 且 **栈槽单位** 的倍数（元数据派生：x86 = 8 字节槽；
    // 1 字节寄存器 ISA 的槽是 1 字节——历史实现写死"8 的倍数"）。
    if let Some(shadow) = abi.stack_args.as_ref().and_then(|s| s.shadow_bytes) {
        if shadow == 0 {
            return Err(format!(
                "[abi.stack_args].shadow_bytes must be > 0, got {shadow}"
            ));
        }
        let unit = m.slot_bytes()? as u32;
        if unit > 1 && shadow % unit != 0 {
            return Err(format!(
                "[abi.stack_args].shadow_bytes must be a positive multiple of the stack slot unit \
                 ({unit} bytes), got {shadow}"
            ));
        }
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
    if let Some(f) = &abi.frame
        && f.sp.trim().is_empty()
    {
        return Err("[abi.frame].sp must not be empty".into());
    }
    Ok(())
}

// ───────────────────────── [emit] ─────────────────────────

/// `[emit]` 模板合法的 `@` 伪指令（DSL 侧 `codegen/frame.rs::gen_emit_pseudo` 的实现集）。
///
/// v15 文档只列了 `@push_callee`/`@frame_alloc`/`@frame_dealloc`/`@pop_callee`——
/// `@frame_dealloc` 这个名字**在代码里不存在**（实际是 `@frame_free`），
/// 且 `@move_args` 没被文档提到。S1 把它变成唯一事实源（文档与验证器同表）。
const EMIT_PSEUDOS: &[&str] = &[
    "push_callee",
    "pop_callee",
    "frame_alloc",
    "frame_free",
    "move_args",
];

/// `[emit]` 模板合法的占位符（`frame.rs` 的实现集）。
const EMIT_PLACEHOLDERS: &[&str] = &["frame_size", "frame_size_neg", "callee_saved_bytes"];

/// `[emit]` 校验（S1：从"只查非空"升级为引用名 + 伪指令 + 占位符全覆盖）。
///
/// 为什么重要：`[emit.prologue].insts` 里写错一个指令名或占位符，旧实现要等到
/// codegen（甚至生成代码编译）才炸，且没有任何位置信息。
fn validate_emit_all(m: &V12Model, idx: &DeclIndex, d: &mut Diags) {
    let Some(em) = &m.emit else {
        return;
    };
    let refs = declared_refs(m);
    for (which, block) in [("prologue", &em.prologue), ("epilogue", &em.epilogue)] {
        let Some(b) = block else { continue };
        let path = format!("[emit.{which}]");
        if b.insts.is_empty() {
            d.push_anchored(idx, &format!("{path}.insts must not be empty"));
            continue;
        }
        for (i, line) in b.insts.iter().enumerate() {
            validate_emit_line(&refs, &format!("{path}.insts[{i}]"), line, idx, d);
        }
    }
}

/// 一行 emit 模板：`@伪指令` 或 `指令引用 op0, op1, …`。
fn validate_emit_line(
    refs: &BTreeSet<String>,
    at: &str,
    line: &str,
    idx: &DeclIndex,
    d: &mut Diags,
) {
    let t = line.trim();
    if let Some(pseudo) = t.strip_prefix('@') {
        if !EMIT_PSEUDOS.contains(&pseudo.trim()) {
            d.push_anchored(
                idx,
                &format!(
                    "{at}: 未知伪指令 '@{pseudo}'（可用：{}）",
                    EMIT_PSEUDOS
                        .iter()
                        .map(|p| format!("@{p}"))
                        .collect::<Vec<_>>()
                        .join(" / ")
                ),
            );
        }
        return;
    }
    let first = t.split_whitespace().next().unwrap_or("");
    if first.is_empty() {
        d.push_anchored(idx, &format!("{at}: 空模板行"));
        return;
    }
    if !refs.contains(first) {
        d.push_anchored(
            idx,
            &format!("{at}: 未知指令引用 '{first}'（须是已声明指令名或某条指令的 ref）"),
        );
    }
    for ph in placeholder_tokens(line) {
        let inner = ph.trim_matches(|c| c == '{' || c == '}');
        let ok = EMIT_PLACEHOLDERS.contains(&inner)
            || inner
                .strip_prefix("frame_size_m")
                .is_some_and(|n| n.parse::<i64>().is_ok());
        if !ok {
            d.push_anchored(
                idx,
                &format!(
                    "{at}: 未知占位符 '{ph}'（可用：{{frame_size}} / {{frame_size_neg}} / \
                     {{frame_size_mN}} / {{callee_saved_bytes}}）"
                ),
            );
        }
    }
}

// ───────────────────────── [spill.*] ─────────────────────────

/// `[spill.*]` 校验（S1：引用名 + `{N}` 占位符 + 基址寄存器名）。
fn validate_spill_all(m: &V12Model, idx: &DeclIndex, d: &mut Diags) {
    let refs = declared_refs(m);
    for (name, s) in &m.spill {
        for (kind, tpl) in [("load", &s.load), ("store", &s.store)] {
            let path = format!("[spill.{name}].{kind}");
            if tpl.trim().is_empty() {
                d.push_anchored(idx, &format!("{path} must not be empty"));
                continue;
            }
            let first = tpl.split_whitespace().next().unwrap_or("");
            if !refs.contains(first) {
                d.push_anchored(
                    idx,
                    &format!("{path}: 未知指令引用 '{first}'（须是已声明指令名或某条指令的 ref）"),
                );
            }
            for ph in placeholder_tokens(tpl) {
                let inner = ph.trim_matches(|c| c == '{' || c == '}');
                if inner.parse::<usize>().is_err() {
                    d.push_anchored(
                        idx,
                        &format!(
                            "{path}: 未知占位符 '{ph}'——spill 模板只接受编号操作数 {{N}}\
                             （{{0}} = 寄存器、{{1}} = 帧偏移）"
                        ),
                    );
                }
            }
        }
        if let Some(base) = &s.base {
            let mut declared: BTreeSet<String> = BTreeSet::new();
            for (rc, g) in &m.reg {
                if let Ok(names) = super::shared::group_names(g) {
                    declared.extend(names);
                }
                let _ = rc;
            }
            if !declared.contains(base.as_str()) {
                d.push_anchored(
                    idx,
                    &format!(
                        "[spill.{name}].base: 物理寄存器名 \"{base}\" 不在任何已声明 [reg.*] 组内"
                    ),
                );
            }
        }
    }
}
