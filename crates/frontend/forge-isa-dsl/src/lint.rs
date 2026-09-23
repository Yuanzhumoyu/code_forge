//! ISA-DSL 静态体检（v19 V4）：**不执行、不编译**，只回答"这份谱里有没有写了却用不上 /
//! 自相矛盾 / 覆盖不到的东西"。
//!
//! 与 `validate` 的分工：`validate` 判"对不对"（错就编译不过），lint 判"干不干净 / 有没有
//! 笔误 / 覆盖率如何"（合法但可疑）。所以 lint 有自己的子命令与退出码（`forge-isa lint`），
//! 且**零误报**是硬要求——凡是有"作者可能故意这么写"空间的规则都不进默认档。
//!
//! 规则清单（V4a/V4b/V4c；全部只看模型 / 合并后的文本，锚到 TOML 行列）：
//!
//! 1. `LINT-UNUSED-SLOT`：`[[operand_slots]]` 没有被任何 `ops = ["名:槽[:角色]", …]` 引用；
//! 2. `LINT-UNUSED-FORM`：`[[forms]]` 没有被任何指令 / 模板 `body.form` 引用；
//! 3. `LINT-UNUSED-BITFIELD`（V4b）：`[conventions.bitfields]` 的位域名在整份谱里**只出现在
//!    声明处**。判据故意用**文本标识符计数**而不是"遍历模型找引用点"——引用形态太多
//!    （form 的 `operand_fields`、指令/模板 `fields`、编码键、asm 占位符…），枚举必然漏、
//!    漏了就成误报；文本计数的代价只是"注释里提到也算引用"（漏报，可接受）。
//! 4. `LINT-BITFIELD-OVERLAP`（V4c）：**同一条指令的生效字段视图**里两个位域抢同一批位。
//!    必须按"form 预设 ⊕ 指令覆盖"后的**逐指令视图**判定——按整张 `[conventions.bitfields]`
//!    表判会把 riscv 的 `shamt5`/`shamt6`、`funct5`/`funct6`/`funct7`、arm64 的
//!    `op6`+`imm26`、两边的 `word`（全字常量）这类"同一批位的多种解释"全判成错。
//! 5. `LINT-REF-UNUSED`（V4c）：指令声明了 `ref`，但**没有任何** lowering / pattern / emit /
//!    pseudo / spill 模板行首引用它——多态分派用不上它（拼错名字时尤其隐蔽）。
//! 6. `LINT-OP-GAP`（V4c，**需要 `--ops <宿主 op 表>`**）：宿主 op 表里有、本谱既没有
//!    `[[lowering]]` 也没有 `[[pattern]]` 覆盖的 op。按[方案 §10.3]的口径**三类分开**：
//!    终结指令（生成的 `lower_terminator` 按 `TermKind` 分派，谱里根本不该有）与宿主管线
//!    直查的 op 都**不算**缺口，只有"真缺口"才报（另有 `LintReport::ops_coverage` 给口径）。
//!
//! **一行锚定是近似的**：位域的锚定走 `DeclIndex::anchor`，它会按名字找行——名字若不是
//! 独立标识符（如 `p6` 是 `op6` 的子串），可能落到同一节里邻近的行上。消息里始终带着
//! 位域名，因此不影响可操作性与"零误报"判据。
//!
//! **不重复报解析期已经拦住的东西**：模板行里的键笔误（`typo_key = 1`）在 `TemplateRow` 的
//! `deny_unknown_fields` 下就是硬错误，lint 再报一次只会变成噪声——这条规则写完才发现，
//! 直接删掉（留这条注释给后来者）。
//!
//! [方案 §10.3]: ../../../../docs/plans/forge-isa-dsl-v19-plan.md

use std::collections::{BTreeMap, BTreeSet};

use crate::report::DiagLine;
use crate::v12::diag::DeclIndex;
use crate::v12::model::{Bitfield, V12Model};

/// 宿主 op 表里**由生成的 `lower_terminator` 处理**的 op（按 `TermKind` 分派）。
///
/// 这些 op 在谱里**不该**有 `[[lowering]]`（V0 实测：三份发行谱一个都没有），因此
/// "宿主有而谱里没有"不算缺口。名单与 `forge-codegen` 的 `GenLowerTerminator` 实现集
/// 一一对应；漏一个就会变成误报，多一个会漏报——守卫
/// `tests/lint_shipped.rs::op_gap_matches_section_10_3` 把三谱的缺口数钉成 3/42/95。
pub const TERMINATOR_OPS: [&str; 6] = ["Ret", "Jmp", "Br", "Switch", "Unreachable", "Invoke"];

/// 宿主**直查**（不走 `[[lowering]]`）的 op：宿主里有 `Opcode::X` 的专门路径。
pub const HOST_PIPELINE_OPS: [&str; 7] = [
    "Bitcast",
    "Call",
    "CallIndirect",
    "ExtractValue",
    "InsertValue",
    "GetElementPtr",
    "LandingPad",
];

/// lint 档位（v19 V4c）：两项可选输入。
#[derive(Debug, Clone, Default)]
pub struct LintOpts {
    /// 宿主 op 名清单（用 [`host_ops_from_toml`] 从 `ops.toml` 读出）。
    /// 空 = 不做能力缺口检查（`LINT-OP-GAP` 与覆盖率口径都不出现）。
    pub host_ops: Vec<String>,
    /// 报"声明了却没被任何模板行首引用的 `ref`"（`LINT-REF-UNUSED`）。
    ///
    /// **默认关**：`ref` 有"给未来 lowering 预留引用名"的合法用法——实测三份发行谱有
    /// 28 条（arm64 27 条是给该谱"还没写的整数 lowering"留的多态名，x86 1 条
    /// `vmovups` 疑似残留）。这类"预留 vs 残留"只有作者能判，所以不进默认档
    /// （同"零误报是硬要求"的纪律）；写新谱时打开它抓拼错名字最有用。
    pub check_unused_refs: bool,
}

/// 能力缺口口径（[方案 §10.3] 的四类计数）。只在给了 `--ops` 时出现。
///
/// [方案 §10.3]: ../../../../docs/plans/forge-isa-dsl-v19-plan.md
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpsCoverage {
    /// 本谱 `[[lowering]].op` ∪ `[[pattern]]` 根 op 覆盖了几条宿主 op。
    pub covered: usize,
    /// 宿主 op 里的终结指令（不进 lowering，不算缺口）。
    pub terminators: usize,
    /// 宿主 op 里由宿主管线直查的（不算缺口）。
    pub host_pipeline: usize,
    /// **真缺口**：宿主有、本谱没有覆盖，也不是上面两类。
    pub gaps: Vec<String>,
}

/// 体检结果：结论 + （可选）能力缺口口径。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LintReport {
    pub findings: Vec<DiagLine>,
    pub ops_coverage: Option<OpsCoverage>,
}

/// 对一个**已加载**的谱文件做体检（解析失败时返回诊断，交给调用方按 `validate` 的样式渲染）。
pub fn lint_source(source: &str) -> Result<Vec<DiagLine>, Vec<DiagLine>> {
    lint_source_opts(source, &LintOpts::default()).map(|r| r.findings)
}

/// 带档位的体检（v19 V4c：`--ops` 走能力缺口检查）。
pub fn lint_source_opts(source: &str, opts: &LintOpts) -> Result<LintReport, Vec<DiagLine>> {
    let model = match crate::v12::parse_and_validate(source) {
        Ok(m) => m,
        Err(e) => return Err(Vec::from(&e)),
    };
    let idx = DeclIndex::build(source);
    Ok(checks(&model, source, &idx, opts))
}

/// 从宿主 op 表（`crates/foundation/forge-ir/ops.toml`）里取 op 名清单。
///
/// 只认 `[[ops]]` 风格的条目里的 `name = "…"`（其余字段是宿主自己的元数据，DSL 不解释）。
/// 这样"宿主能力"始终是**宿主的数据**，DSL 侧不留第二份会漂移的名单。
pub fn host_ops_from_toml(text: &str) -> Result<Vec<String>, String> {
    // 走 serde 的**文档**解析器（与 `v12::parse` 同一入口）；`Value` 自己的 `FromStr`
    // 在本仓的 `toml` 版本里是"解析单个值"，喂整份文档会报 `unexpected content`。
    let value: toml::Value =
        toml::from_str(text).map_err(|e| format!("宿主 op 表不是合法 TOML：{e}"))?;
    let mut out = Vec::new();
    let collect = |out: &mut Vec<String>, table: &toml::value::Table| {
        if let Some(name) = table.get("name").and_then(|v| v.as_str()) {
            out.push(name.to_string());
        }
    };
    match &value {
        toml::Value::Array(items) => {
            for it in items {
                if let toml::Value::Table(t) = it {
                    collect(&mut out, t);
                }
            }
        }
        toml::Value::Table(t) => {
            // `crates/foundation/forge-ir/ops.toml` 的形态：顶层 `[[op]]`（116 条）。
            // 兼容 `[[ops]]` 复数写法（别的宿主可能这么起名）。
            let arr = ["op", "ops"].iter().find_map(|k| match t.get(*k) {
                Some(toml::Value::Array(items)) => Some(items),
                _ => None,
            });
            match arr {
                Some(items) => {
                    for it in items {
                        if let toml::Value::Table(inner) = it {
                            collect(&mut out, inner);
                        }
                    }
                }
                None => {
                    return Err(
                        "宿主 op 表里没读到 `[[op]]`/`[[ops]]` 数组——期望每个 op 一条 + `name = \"…\"`"
                            .into(),
                    );
                }
            }
        }
        _ => return Err("宿主 op 表顶层应是 `[[ops]]` 数组".into()),
    }
    if out.is_empty() {
        return Err("宿主 op 表里没读到任何 op 名（期望 `[[ops]]` + `name = \"…\"`）".into());
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// 位域名在源码里出现的次数（按标识符边界算，避免 `imm1` 命中 `imm12`）。
fn count_ident(source: &str, name: &str) -> usize {
    let bytes = source.as_bytes();
    let mut n = 0;
    let mut from = 0;
    while let Some(rel) = source[from..].find(name) {
        let at = from + rel;
        let before_ok = at == 0 || !is_ident_byte(bytes[at - 1]);
        let end = at + name.len();
        let after_ok = end >= bytes.len() || !is_ident_byte(bytes[end]);
        if before_ok && after_ok {
            n += 1;
        }
        from = end;
    }
    n
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// 全部规则的实现。
fn checks(m: &V12Model, source: &str, idx: &DeclIndex, opts: &LintOpts) -> LintReport {
    let mut out = Vec::new();

    // ── 1. 未被引用的操作数槽 ──
    let mut used_slots: BTreeSet<String> = BTreeSet::new();
    let mut all_ops: Vec<String> = m
        .instructions
        .iter()
        .flat_map(|i| i.ops.iter().flatten())
        .cloned()
        .collect();
    all_ops.extend(body_ops(m));
    for op in &all_ops {
        if let Some(slot) = op.split(':').nth(1) {
            used_slots.insert(slot.trim().to_string());
        }
    }
    for (i, s) in m.operand_slots.iter().enumerate() {
        if !used_slots.contains(&s.name) {
            out.push(anchor(
                idx,
                &format!("[[operand_slots]] #{i} ('{}')", s.name),
                "LINT-UNUSED-SLOT",
                format!(
                    "槽 '{}' 没有被任何 `ops` 引用（写了却用不上）——删掉它，或把它写进某条指令的 `ops`",
                    s.name
                ),
            ));
        }
    }

    // ── 2. 未被引用的编码形式 ──
    let mut used_forms: BTreeSet<String> = BTreeSet::new();
    for f in m.instructions.iter().filter_map(|i| i.form.as_ref()) {
        used_forms.insert(f.clone());
    }
    for t in &m.templates {
        if let Some(f) = t
            .body
            .as_ref()
            .and_then(|b| b.get("form"))
            .and_then(|v| v.as_str())
        {
            used_forms.insert(f.to_string());
        }
    }
    for (i, f) in m.forms.iter().enumerate() {
        if !used_forms.contains(&f.name) {
            out.push(anchor(
                idx,
                &format!("[[forms]] #{i} ('{}')", f.name),
                "LINT-UNUSED-FORM",
                format!(
                    "form '{}' 没有被任何指令/模板引用（写了却用不上）——删掉它，或写 `form = \"{}\"`",
                    f.name, f.name
                ),
            ));
        }
    }

    // ── 3. 未被任何地方引用的位域 ──
    //
    // 判据是**文本出现次数**：位域名在整份（合并后的）谱里只出现在声明那一处 ⇒ 定义上就是
    // 死声明。比"遍历模型找引用点"更稳——引用点有 form 的 `operand_fields`、指令/模板行的
    // `fields`、编码键（`imm = "imm12"`、`opcode_field = "op"`）、asm 模板占位符等多种形态，
    // 逐个枚举必然漏，漏了就变成误报。代价是**注释里提到**也算引用（漏报，可接受）。
    for (i, name) in m.conventions.bitfields.keys().enumerate() {
        if count_ident(source, name) <= 1 {
            out.push(anchor(
                idx,
                &format!("[conventions.bitfields] #{i} ('{name}')"),
                "LINT-UNUSED-BITFIELD",
                format!(
                    "位域 '{name}' 只出现在声明处——没有任何 form/指令/模板/编码键引用它，删掉它"
                ),
            ));
        }
    }

    // ── 4. 同一条指令的字段视图里位域重叠（V4c）──
    out.extend(bitfield_overlaps(m, idx));

    // ── 5. 声明了却没人引用的 `ref`（V4c，`--refs` 才开）──
    if opts.check_unused_refs {
        out.extend(unused_refs(m, idx));
    }

    // ── 6. 能力缺口（V4c，需 `--ops`）──
    let ops_coverage = op_gap(m, opts, idx, &mut out);

    LintReport {
        findings: out,
        ops_coverage,
    }
}

// ─────────────────────── 规则 4：位域重叠 ───────────────────────

/// 位域的字位区间（`pieces` 展开成多段；`word` 这类全字常量是单段 `[0, 32)`）。
fn bit_ranges(bf: &Bitfield) -> Vec<(u32, u32)> {
    match &bf.pieces {
        Some(ps) => ps.iter().map(|p| (p.offset, p.offset + p.width)).collect(),
        None => match (bf.offset, bf.width) {
            (Some(o), Some(w)) => vec![(o, o + w)],
            _ => Vec::new(),
        },
    }
}

/// 逐指令判"两个字段抢同一批位"。消息里给出双方区间，便于直接改 TOML。
fn bitfield_overlaps(m: &V12Model, idx: &DeclIndex) -> Vec<DiagLine> {
    let mut out = Vec::new();
    for inst in &m.instructions {
        let preset = inst
            .form
            .as_ref()
            .and_then(|name| m.forms.iter().find(|f| &f.name == name));
        let enc = inst
            .enc
            .over(&preset.map(|f| f.keys.clone()).unwrap_or_default());
        // `fields` 在模型里是 `Option<BTreeMap<String, u64>>`——名字集合就是键集合。
        let empty = std::collections::BTreeMap::new();
        let fields: &std::collections::BTreeMap<String, u64> =
            inst.fields.as_ref().unwrap_or(&empty);
        let mut names: Vec<String> = fields.keys().cloned().collect();
        if let Some(f) = &enc.opcode_field {
            names.push(f.clone());
        }
        names.extend(enc.operand_fields.iter().flatten().cloned());
        if (enc.modrm.is_some() || enc.modrm_fixed.is_some())
            && let Some(md) = &m.conventions.modrm
        {
            names.extend(
                [md.reg_field.clone(), md.rm_field.clone()]
                    .into_iter()
                    .flatten(),
            );
        }
        names.sort();
        names.dedup();

        let mut ranges: BTreeMap<&str, Vec<(u32, u32)>> = BTreeMap::new();
        for n in &names {
            if let Some(bf) = m.conventions.bitfields.get(n) {
                ranges.insert(n.as_str(), bit_ranges(bf));
            }
        }
        let names: Vec<&str> = ranges.keys().copied().collect();
        for (i, a) in names.iter().enumerate() {
            for b in names.iter().skip(i + 1) {
                let Some((ra, rb)) = overlapping(&ranges[a], &ranges[b]) else {
                    continue;
                };
                out.push(anchor(
                    idx,
                    &format!("[[instructions.{}]]", inst.name),
                    "LINT-BITFIELD-OVERLAP",
                    format!(
                        "位域 '{a}' [{}, {}) 与 '{b}' [{}, {}) 在同一条指令的字段视图里重叠——\
                         两者抢同一批位（编码互相覆盖）；确认哪一个才是这条指令的字段，\
                         或把指令拆成两条（不同字段视图）",
                        ra.0, ra.1, rb.0, rb.1
                    ),
                ));
            }
        }
    }
    out
}

/// 两组字位区间里第一对相交的（无交集返回 `None`）。
fn overlapping(a: &[(u32, u32)], b: &[(u32, u32)]) -> Option<((u32, u32), (u32, u32))> {
    for x in a {
        for y in b {
            if x.0 < y.1 && y.0 < x.1 {
                return Some((*x, *y));
            }
        }
    }
    None
}

// ─────────────────────── 规则 5：未引用的 ref ───────────────────────

/// 模板行首引用到的名字全集（lowering / pattern / emit / pseudo / spill）。
fn referenced_heads(m: &V12Model) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut add = |lines: &[String]| {
        for l in lines {
            if let Some(h) = crate::v12::validate::inst_head_ref(l) {
                out.insert(h);
            }
        }
    };
    for l in &m.lowering {
        add(&l.insts);
    }
    for p in &m.pattern {
        add(&p.insts);
    }
    if let Some(em) = &m.emit {
        for b in [em.prologue.as_ref(), em.epilogue.as_ref()]
            .into_iter()
            .flatten()
        {
            add(&b.insts);
        }
    }
    for s in m.spill.values() {
        add(&[s.load.clone(), s.store.clone()]);
    }
    for p in &m.pseudo {
        add(&p.emit);
    }
    out
}

/// 声明了 `ref` 却没有**任何**模板行首引用它：多态分派用不上（拼错名字时尤其隐蔽）。
///
/// 同一个 `ref` 被多条指令共用时只报一次（按名字去重），锚到第一条声明它的指令。
fn unused_refs(m: &V12Model, idx: &DeclIndex) -> Vec<DiagLine> {
    let used = referenced_heads(m);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for inst in &m.instructions {
        let Some(r) = &inst.reference else { continue };
        if used.contains(r) || !seen.insert(r.clone()) {
            continue;
        }
        out.push(anchor(
            idx,
            &format!("[[instructions.{}]]", inst.name),
            "LINT-REF-UNUSED",
            format!(
                "ref '{r}' 没有被任何 lowering/pattern/emit/pseudo/spill 模板行首引用——\
                 多态分派用不上它（模板里写的是别的名字？还是可以删掉这条 ref？）"
            ),
        ));
    }
    out
}

// ─────────────────────── 规则 6：能力缺口 ───────────────────────

/// 宿主 op 三类分开报（V4c）：终结指令 / 宿主管线直查 / 真缺口。
fn op_gap(
    m: &V12Model,
    opts: &LintOpts,
    idx: &DeclIndex,
    out: &mut Vec<DiagLine>,
) -> Option<OpsCoverage> {
    if opts.host_ops.is_empty() {
        return None;
    }
    // 本谱覆盖：`[[lowering]].op` ∪ `[[pattern]]` 匹配树的根 op。
    let mut covered: BTreeSet<String> =
        m.lowering.iter().map(|l| l.op.name().to_string()).collect();
    for p in &m.pattern {
        if let Ok(tree) = crate::v12::match_tree::parse(&p.r#match) {
            let mut ops = Vec::new();
            crate::v12::match_tree::op_names(&tree, &mut ops);
            covered.extend(ops);
        }
    }
    let terminators: BTreeSet<&str> = TERMINATOR_OPS.into_iter().collect();
    let pipeline: BTreeSet<&str> = HOST_PIPELINE_OPS.into_iter().collect();
    let mut gaps = Vec::new();
    let mut n_term = 0;
    let mut n_pipe = 0;
    for op in &opts.host_ops {
        if covered.contains(op) {
            continue;
        }
        if terminators.contains(op.as_str()) {
            n_term += 1;
        } else if pipeline.contains(op.as_str()) {
            n_pipe += 1;
        } else {
            gaps.push(op.clone());
        }
    }
    gaps.sort();
    for op in &gaps {
        out.push(anchor(
            idx,
            "",
            "LINT-OP-GAP",
            format!(
                "宿主 op '{op}' 在本谱里既没有 `[[lowering]]` 也没有 `[[pattern]]` 覆盖\
                 （真缺口：宿主会把这条 op 交给本 ISA）——补 lowering，或在宿主侧明确拒绝"
            ),
        ));
    }
    let known = opts.host_ops.len();
    Some(OpsCoverage {
        covered: known - n_term - n_pipe - gaps.len(),
        terminators: n_term,
        host_pipeline: n_pipe,
        gaps,
    })
}

/// 模板 `body.ops` 里的槽名（指令通常已展开进 `m.instructions`，这里兜底）。
fn body_ops(m: &V12Model) -> Vec<String> {
    let mut out = Vec::new();
    for t in &m.templates {
        if let Some(arr) = t
            .body
            .as_ref()
            .and_then(|b| b.get("ops"))
            .and_then(|v| v.as_array())
        {
            out.extend(arr.iter().filter_map(|v| v.as_str()).map(str::to_string));
        }
    }
    out
}

/// 把结论锚回 TOML：消息以路径开头，`DeclIndex::anchor` 据此算出行列。
///
/// `code == "LINT-OP-GAP"` 时路径为空（缺口不属于任何声明），锚到 `1:1`。
fn anchor(idx: &DeclIndex, path: &str, code: &str, msg: String) -> DiagLine {
    let text = if path.is_empty() {
        msg
    } else {
        format!("{path}: {msg}")
    };
    let a = idx.anchor(&text);
    DiagLine {
        file: None,
        code: code.to_string(),
        line: a.line,
        col: a.col,
        msg: text,
        notes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r#"
[meta]
name = "linttoy"
[encoding]
kind = "fixed"
bits = 16

[reg.gpr1]
names = ["R0", "R1"]

[conventions.bitfields]
op = { offset = 12, width = 4 }
rd = { offset = 8, width = 3 }
funct3 = { offset = 0, width = 3 }

[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr1"

[[operand_slots]]
name = "unused_slot"
kind = "imm"
width = 5

[[forms]]
name = "RR"
opcode_field = "op"
operand_fields = ["rd"]

[[forms]]
name = "UNUSED_FORM"
opcode_field = "op"
operand_fields = ["rd"]

[[instructions]]
name = "ADD"
form = "RR"
opcode = 1
ops = ["dst:g:out"]
asm = "add {dst}"

[[templates]]
name = "T"
body = { form = "RR", opcode = 2, ops = ["dst:g:out"], asm = "t {dst}" }
rows = [
  { inst = "T1", fields = { funct3 = 0 } },
]
"#;

    /// 前两条规则各报一条：`unused_slot` / `UNUSED_FORM`；在用的 `g`/`RR` 不报。
    #[test]
    fn reports_unused_slot_and_form() {
        let found = lint_source(SPEC).expect("谱本身合法");
        let codes: Vec<&str> = found.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(
            codes,
            vec!["LINT-UNUSED-SLOT", "LINT-UNUSED-FORM"],
            "{found:?}"
        );
        assert!(found[0].msg.contains("unused_slot"), "{}", found[0].msg);
        assert!(found[1].msg.contains("UNUSED_FORM"), "{}", found[1].msg);
        assert!(found[0].line > 1, "锚到了 TOML 行列：{:?}", found[0]);
    }

    #[test]
    fn clean_spec_has_no_findings() {
        let clean = SPEC
            .replace(
                "[[operand_slots]]\nname = \"unused_slot\"\nkind = \"imm\"\nwidth = 5\n",
                "",
            )
            .replace(
                "[[forms]]\nname = \"UNUSED_FORM\"\nopcode_field = \"op\"\noperand_fields = [\"rd\"]\n",
                "",
            );
        assert!(!clean.contains("UNUSED_FORM"));
        let found = lint_source(&clean).expect("谱本身合法");
        assert!(found.is_empty(), "干净谱不该有结论：{found:?}");
    }

    #[test]
    fn invalid_spec_returns_diagnostics_instead() {
        let bad = SPEC.replace("form = \"RR\"", "form = \"NO_SUCH_FORM\"");
        assert!(lint_source(&bad).is_err(), "校验不过时 lint 直接返回诊断");
    }

    /// V4b：只出现在声明处的位域要报；在用的一次都不报（`imm1` 不会命中 `imm12`）。
    #[test]
    fn reports_only_truly_unused_bitfields() {
        let with_dead = SPEC.replace(
            "[conventions.bitfields]",
            "[conventions.bitfields]\ndead_bits = { offset = 3, width = 2 }",
        );
        let found = lint_source(&with_dead).expect("谱合法");
        let dead: Vec<&DiagLine> = found
            .iter()
            .filter(|d| d.code == "LINT-UNUSED-BITFIELD")
            .collect();
        assert_eq!(dead.len(), 1, "只该报 dead_bits：{found:?}");
        assert!(dead[0].msg.contains("dead_bits"), "{}", dead[0].msg);

        // 与死声明相邻的 `imm1`/`imm12`：名字是前缀关系，不能互相命中。
        assert_eq!(count_ident("imm12 = 1\nimm1 = 2\n", "imm1"), 1);
        assert_eq!(count_ident("imm12 = 1\nimm1 = 2\n", "imm12"), 1);
    }

    /// 解析期已经拦住的东西**不重复报**：模板行键笔误是硬错误（`deny_unknown_fields`）。
    #[test]
    fn row_key_typos_are_validate_errors_not_lint_findings() {
        let bad = SPEC.replace(
            "  { inst = \"T1\", fields = { funct3 = 0 } },",
            "  { inst = \"T1\", typo_key = 1 },",
        );
        let err = lint_source(&bad).expect_err("行键笔误必须由 validate 报");
        assert!(
            err.iter().any(|d| d.msg.contains("typo_key")),
            "诊断应点名 typo_key：{err:?}"
        );
    }

    /// V4c-1：**同一条指令的视图**里两个字段抢同一批位才算重叠。
    ///
    /// 关键反面：`op`/`rd` 这类"同一批位的多种解释"只要不在**同一条指令**里共存就不报
    /// （riscv 的 `shamt5`/`shamt6`、arm64 的 `op6`+`imm26` 都是合法的）。
    #[test]
    fn bitfield_overlap_is_per_instruction_view() {
        // 变体 a：`T1` 行里 `funct3`（[0,3)）与新增的 `over`（[2,4)）重叠 → 报。
        let bad = SPEC
            .replace(
                "[conventions.bitfields]",
                "[conventions.bitfields]\nover = { offset = 2, width = 2 }",
            )
            .replace(
                "  { inst = \"T1\", fields = { funct3 = 0 } },",
                "  { inst = \"T1\", fields = { funct3 = 0, over = 0 } },",
            );
        let found = lint_source(&bad).expect("谱合法");
        let hits: Vec<&DiagLine> = found
            .iter()
            .filter(|d| d.code == "LINT-BITFIELD-OVERLAP")
            .collect();
        assert_eq!(hits.len(), 1, "只该报一条重叠：{found:?}");
        let msg = &hits[0].msg;
        assert!(
            msg.contains("'funct3'")
                && msg.contains("'over'")
                && msg.contains("[[instructions.T1]]"),
            "消息要点名两条字段与指令：{msg}"
        );
        assert!(hits[0].line > 1, "锚到了行列：{hit:?}", hit = hits[0]);

        // 变体 b：同一个 `over` 声明**没人用** → 只有"没用上"结论，没有重叠结论。
        let unused = SPEC.replace(
            "[conventions.bitfields]",
            "[conventions.bitfields]\nover = { offset = 2, width = 2 }",
        );
        let found = lint_source(&unused).expect("谱合法");
        assert!(
            found.iter().all(|d| d.code != "LINT-BITFIELD-OVERLAP"),
            "没进任何指令视图的位域不该报重叠：{found:?}"
        );
    }

    /// V4c-2：`ref` 未被引用只在 `--refs`（`check_unused_refs`）下报；
    /// 被模板行首（含 `@名字`）引用过的不报。
    #[test]
    fn reports_unreferenced_ref_only_when_opted_in() {
        let doc = |lowering: &str, reference: &str| {
            format!(
                "{SPEC}\n[[instructions]]\nname = \"MOV\"\nform = \"RR\"\nopcode = 3\n\
                 ops = [\"dst:g:out\"]\nasm = \"mov {{dst}}\"\nref = \"{reference}\"\n{lowering}"
            )
        };
        let on = LintOpts {
            check_unused_refs: true,
            ..Default::default()
        };
        // 默认档：一条 `LINT-REF-UNUSED` 都没有（"给未来 lowering 预留"是合法写法）。
        let bad = doc(
            "[[lowering]]\nop = \"Copy\"\ninsts = [\"ADD {out}\"]\n",
            "mv",
        );
        let plain = lint_source(&bad).expect("谱合法");
        assert!(
            plain.iter().all(|d| d.code != "LINT-REF-UNUSED"),
            "默认档不该报未引用 ref：{plain:?}"
        );
        // 打开 `--refs`：`ref = "mv"` 而 lowering 行首写的是 `ADD` → 报。
        let report = lint_source_opts(&bad, &on).expect("谱合法");
        let hits: Vec<&DiagLine> = report
            .findings
            .iter()
            .filter(|d| d.code == "LINT-REF-UNUSED")
            .collect();
        assert_eq!(hits.len(), 1, "该报一条未引用 ref：{:?}", report.findings);
        assert!(hits[0].msg.contains("'mv'"), "{}", hits[0].msg);

        // lowering 行首改成 `@mv`（引用名形态）→ 不报。
        let good = doc(
            "[[lowering]]\nop = \"Copy\"\ninsts = [\"@mv {out}\"]\n",
            "mv",
        );
        let report = lint_source_opts(&good, &on).expect("谱合法");
        assert!(
            report.findings.iter().all(|d| d.code != "LINT-REF-UNUSED"),
            "被 `@名字` 引用过的 ref 不该报：{:?}",
            report.findings
        );
    }

    /// V4c-3：能力缺口三类分开——只有"真缺口"报结论，终结指令/宿主管线只计口径。
    #[test]
    fn op_gap_reports_true_gaps_only() {
        // 宿主 op 表：一条谱里覆盖的（ADD 不是 op 名，用 Copy）、终结指令、宿主管线、两条真缺口。
        let ops = host_ops_from_toml(
            r#"
[[ops]]
name = "Copy"
[[ops]]
name = "Ret"
[[ops]]
name = "Call"
[[ops]]
name = "VaArg"
[[ops]]
name = "Resume"
"#,
        )
        .expect("宿主 op 表");
        assert_eq!(ops, vec!["Call", "Copy", "Resume", "Ret", "VaArg"]);

        let src = SPEC.replace(
            "[[templates]]",
            "[[lowering]]\nop = \"Copy\"\ninsts = [\"ADD {out}\"]\n\n[[templates]]",
        );
        let report = lint_source_opts(
            &src,
            &LintOpts {
                host_ops: ops.clone(),
                ..Default::default()
            },
        )
        .expect("谱合法");
        let gaps: Vec<&DiagLine> = report
            .findings
            .iter()
            .filter(|d| d.code == "LINT-OP-GAP")
            .collect();
        let names: Vec<String> = gaps
            .iter()
            .map(|d| d.msg.split('\'').nth(1).unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names,
            vec!["Resume", "VaArg"],
            "只报真缺口：{:?}",
            report.findings
        );
        let cov = report.ops_coverage.expect("给了 --ops 就有口径");
        assert_eq!(
            (
                cov.covered,
                cov.terminators,
                cov.host_pipeline,
                cov.gaps.len()
            ),
            (1, 1, 1, 2)
        );

        // 不给 `--ops`：能力缺口口径与结论都不出现（它是可选输入）。
        let plain = lint_source_opts(&src, &LintOpts::default()).expect("谱合法");
        assert!(plain.ops_coverage.is_none());
        assert!(
            plain.findings.iter().all(|d| d.code != "LINT-OP-GAP"),
            "{:?}",
            plain.findings
        );
    }

    /// `host_ops_from_toml` 的错也要可操作（不是合法 TOML / 没有 op 名）。
    #[test]
    fn host_ops_parse_errors_are_actionable() {
        let e = host_ops_from_toml("not toml = = =").unwrap_err();
        assert!(e.contains("不是合法 TOML"), "{e}");
        let e = host_ops_from_toml("[meta]\nname = \"x\"\n").unwrap_err();
        assert!(e.contains("没读到"), "{e}");
    }
}
