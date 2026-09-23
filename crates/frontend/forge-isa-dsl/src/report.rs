//! `report` — 面向**工具**（`forge-isa` CLI，v18 S7b）的只读报告层。
//!
//! 设计：内部模型（`V12Model`/`V12Error`/`InstInfo`/`diag::Diag`）保持 crate 私有，
//! 本模块把它们**投影成公开的纯数据结构**（可 JSON 化）。这样 CLI 不需要暴露模型
//! 细节，也不必让 `pub` 泄漏到模型层（`pub enum V12Error` 会连带公开 `diag::Diag`）。
//!
//! 单一事实源：指令的"生效规格"直接取 `codegen::collect_inst_infos`（form 预设 ⊕
//! 指令级覆盖的**同一份**判定），CLI 不另写一遍。

use std::collections::BTreeMap;
use std::path::Path;

use crate::v12::V12Error;
use crate::v12::codegen::collect_inst_infos;
use crate::v12::model::{EncodingKind, RegClass, V12Model};

/// 一条诊断（渲染与 JSON 都用它）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagLine {
    /// 来源文件（多文件谱里诊断可能落在 include 的文件上；单文件 = 根文件）。
    pub file: Option<String>,
    /// 节级错误码（`DSL-INST`/`DSL-TEMPLATE`/…）。
    pub code: String,
    /// ISA TOML 内 1-based 行/列。
    pub line: usize,
    pub col: usize,
    pub msg: String,
    /// 附加说明（同名声明位置等）。
    pub notes: Vec<String>,
}

impl DiagLine {
    /// 无位置的整条消息（生成期错误、文件读不到等）。
    pub fn plain(msg: &str) -> Self {
        Self {
            file: None,
            code: "DSL-OTHER".into(),
            line: 1,
            col: 1,
            msg: msg.to_string(),
            notes: Vec::new(),
        }
    }

    /// `行:列: 码: 消息`（与 `V12Error` 的 Display 同形，无路径前缀）。
    pub fn render(&self) -> String {
        format!("{}:{}: {}: {}", self.line, self.col, self.code, self.msg)
    }
}

impl From<&V12Error> for Vec<DiagLine> {
    fn from(e: &V12Error) -> Self {
        e.diags()
            .into_iter()
            .map(|d| DiagLine {
                file: None,
                code: d.code.to_string(),
                line: d.line,
                col: d.col,
                msg: d.msg,
                notes: d.notes,
            })
            .collect()
    }
}

/// 解析 + 校验，返回全部诊断（空 = 通过）。
pub fn validate(source: &str) -> Vec<DiagLine> {
    match crate::v12::parse_and_validate(source) {
        Ok(_) => Vec::new(),
        Err(e) => Vec::from(&e),
    }
}

/// ISA 概览（`explain` 抬头用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsaSummary {
    pub name: String,
    pub version: Option<String>,
    pub encoding_kind: String,
    /// `fixed`：字长（位）；`mixed`：允许字长集；`prefix_scan`：空。
    pub widths_bits: Vec<u32>,
    pub max_len: Option<u8>,
    pub templates: usize,
    pub instructions: usize,
    pub lowering_rules: usize,
    pub pseudo: usize,
    pub reloc: usize,
    pub derive: usize,
}

/// 展开后的一条指令（生效规格）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstRow {
    pub name: String,
    /// 由哪个模板展开（`from_template`；手写指令 = None）。
    pub from_template: Option<String>,
    /// 模板里的行号（1-based，仅 `from_template` 有值时给）。
    pub template_row: Option<usize>,
    /// 生效字长（位）：指令 `width` > `[encoding].bits`。
    pub width_bits: Option<u32>,
    /// 生效字长（字节，ceil；`prefix_scan` 无固定值）。
    pub len_bytes: Option<u32>,
    pub form: Option<String>,
    pub opcode: Option<u64>,
    pub ops: Vec<String>,
    pub asm: String,
    pub reference: Option<String>,
    pub reloc: Option<String>,
    /// 生效编码键（form 预设 ⊕ 指令覆盖），`key = value` 字典序。
    pub enc: Vec<(String, String)>,
}

impl InstRow {
    /// 差异比较用的一行（字段名 → 值），也是 `insts --json` 的主体。
    pub fn fields(&self) -> Vec<(&'static str, String)> {
        let enc = self
            .enc
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        vec![
            (
                "from_template",
                self.from_template.clone().unwrap_or_default(),
            ),
            (
                "template_row",
                self.template_row.map(|r| r.to_string()).unwrap_or_default(),
            ),
            (
                "width_bits",
                self.width_bits.map(|w| w.to_string()).unwrap_or_default(),
            ),
            (
                "len_bytes",
                self.len_bytes.map(|w| w.to_string()).unwrap_or_default(),
            ),
            ("form", self.form.clone().unwrap_or_default()),
            (
                "opcode",
                self.opcode.map(|o| format!("{o:#x}")).unwrap_or_default(),
            ),
            ("ops", self.ops.join(",")),
            ("asm", self.asm.clone()),
            ("ref", self.reference.clone().unwrap_or_default()),
            ("reloc", self.reloc.clone().unwrap_or_default()),
            ("enc", enc),
        ]
    }
}

/// 模板里的一行（`explain` 用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateRow {
    pub template: String,
    /// 行号（1-based，`rows` 里的位置）。
    pub index: usize,
    /// 模板级 `body`（键值对，`key = value` 原样文本）。
    pub body: Vec<(String, String)>,
    /// 该行的键值对。
    pub row: Vec<(String, String)>,
}

/// `explain` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explain {
    pub isa: IsaSummary,
    pub inst: InstRow,
    /// 来源模板行（手写指令 = None）。
    pub template: Option<TemplateRow>,
}

fn summary(m: &V12Model) -> IsaSummary {
    let widths = match m.encoding.kind {
        EncodingKind::Fixed => m.encoding.bits.into_iter().collect(),
        EncodingKind::Mixed => m.encoding.widths.clone(),
        EncodingKind::PrefixScan => Vec::new(),
    };
    IsaSummary {
        name: m.meta.name.clone(),
        version: m.meta.version.clone(),
        encoding_kind: match m.encoding.kind {
            EncodingKind::Fixed => "fixed".into(),
            EncodingKind::Mixed => "mixed".into(),
            EncodingKind::PrefixScan => "prefix_scan".into(),
        },
        widths_bits: widths,
        max_len: m.encoding.max_len,
        templates: m.templates.len(),
        instructions: m.instructions.len(),
        lowering_rules: m.lowering.len(),
        pseudo: m.pseudo.len(),
        reloc: m.reloc.len(),
        derive: m.derive.len(),
    }
}

/// 编码键 → `(key, value)` 列表（字典序）。用 serde 把 `EncKeys` 折成 TOML 表，
/// **不维护第二份键名清单**（新增编码键自动出现在 `insts`/`explain`/`diff` 里）。
fn enc_pairs(keys: &crate::v12::model::EncKeys) -> Vec<(String, String)> {
    let Ok(toml::Value::Table(t)) = toml::Value::try_from(keys) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = t.into_iter().map(|(k, v)| (k, v.to_string())).collect();
    out.sort();
    out
}

fn row_of(m: &V12Model, info: &crate::v12::codegen::InstInfo<'_>) -> InstRow {
    let inst = &info.inst;
    let template_row = inst.from_template.as_ref().and_then(|tname| {
        find_template(m, tname).and_then(|t| {
            t.rows
                .iter()
                .position(|r| r.inst == inst.name)
                .map(|i| i + 1)
        })
    });
    InstRow {
        name: inst.name.clone(),
        from_template: inst.from_template.as_ref().map(|s| s.to_string()),
        template_row,
        width_bits: inst.width.or(m.encoding.bits),
        len_bytes: m.inst_width_bytes(inst).ok(),
        form: inst.form.clone(),
        opcode: inst.opcode,
        ops: inst.ops.clone().unwrap_or_default(),
        asm: inst.asm.clone(),
        reference: inst.reference.clone(),
        reloc: inst.reloc.clone(),
        enc: enc_pairs(&info.form),
    }
}

/// 展开后的全部指令（按 `[[instructions]]` 声明序 + 模板展开序）。
pub fn insts(source: &str) -> Result<(IsaSummary, Vec<InstRow>), Vec<DiagLine>> {
    let m = crate::v12::parse_and_validate(source).map_err(|e| Vec::from(&e))?;
    let infos = collect_inst_infos(&m).map_err(|e| vec![DiagLine::plain(&e)])?;
    let rows = infos.iter().map(|i| row_of(&m, i)).collect();
    Ok((summary(&m), rows))
}

/// 单条指令的完整解释（含来源模板行）。
pub fn explain(source: &str, name: &str) -> Result<Explain, Vec<DiagLine>> {
    let m = crate::v12::parse_and_validate(source).map_err(|e| Vec::from(&e))?;
    let infos = collect_inst_infos(&m).map_err(|e| vec![DiagLine::plain(&e)])?;
    let info = infos.iter().find(|i| i.inst.name == name).ok_or_else(|| {
        let mut names: Vec<&str> = infos.iter().map(|i| i.inst.name.as_str()).collect();
        names.sort_unstable();
        vec![DiagLine {
            file: None,
            code: "DSL-INST".into(),
            line: 1,
            col: 1,
            msg: format!(
                "没有名为 '{name}' 的指令（展开后共 {} 条：{}）",
                names.len(),
                names.join(", ")
            ),
            notes: Vec::new(),
        }]
    })?;
    let inst = row_of(&m, info);
    let template = inst.from_template.as_ref().and_then(|tname| {
        find_template(&m, tname).map(|t| {
            let idx = inst.template_row.unwrap_or(1);
            TemplateRow {
                template: tname.clone(),
                index: idx,
                body: t.body.as_ref().map(table_pairs).unwrap_or_default(),
                row: t
                    .rows
                    .get(idx.saturating_sub(1))
                    .map(|r| table_pairs(&r.fields))
                    .unwrap_or_default(),
            }
        })
    });
    Ok(Explain {
        isa: summary(&m),
        inst,
        template,
    })
}

/// 按模板名找模板：显式 `name` 优先，缺省时按"首行的 `inst`"（与解析期一致）。
fn find_template<'a>(m: &'a V12Model, name: &str) -> Option<&'a crate::v12::model::Template> {
    m.templates.iter().find(|t| match &t.name {
        Some(n) => n == name,
        None => t.rows.first().is_some_and(|r| r.inst == name),
    })
}

/// `toml::Table` → `(key, value)`（字典序）。
fn table_pairs(t: &toml::Table) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> =
        t.iter().map(|(k, v)| (k.clone(), v.to_string())).collect();
    out.sort();
    out
}

/// 规格差异（`explain` 的"展开后有效规格"逐字段比较）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpecDiff {
    /// 只在 B 里有的指令。
    pub added: Vec<String>,
    /// 只在 A 里有的指令。
    pub removed: Vec<String>,
    /// 两边都有但字段不同：`(指令名, ["字段: A → B", …])`。
    pub changed: Vec<(String, Vec<String>)>,
}

impl SpecDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// 两份谱的规格 diff（A → B）。
pub fn diff(a: &str, b: &str) -> Result<SpecDiff, Vec<DiagLine>> {
    let (_, ra) = insts(a)?;
    let (_, rb) = insts(b)?;
    let ma: BTreeMap<&str, &InstRow> = ra.iter().map(|r| (r.name.as_str(), r)).collect();
    let mb: BTreeMap<&str, &InstRow> = rb.iter().map(|r| (r.name.as_str(), r)).collect();
    let mut out = SpecDiff::default();
    for name in mb.keys() {
        if !ma.contains_key(name) {
            out.added.push((*name).to_string());
        }
    }
    for name in ma.keys() {
        if !mb.contains_key(name) {
            out.removed.push((*name).to_string());
        }
    }
    for (name, x) in &ma {
        let Some(y) = mb.get(name) else { continue };
        let fx = x.fields();
        let fy = y.fields();
        let mut diffs = Vec::new();
        for ((k, va), (_, vb)) in fx.iter().zip(fy.iter()) {
            if va != vb {
                diffs.push(format!("{k}: {va:?} → {vb:?}"));
            }
        }
        if !diffs.is_empty() {
            out.changed.push(((*name).to_string(), diffs));
        }
    }
    Ok(out)
}

/// 寄存器类 → 可读名（`explain` 的槽/类信息用）。
pub fn class_name(c: RegClass) -> String {
    match c {
        RegClass::GPR(w) => format!("gpr{w}"),
        RegClass::FPR(w) => format!("fpr{w}"),
        RegClass::VEC(w) => format!("vec{w}"),
        RegClass::KReg(w) => format!("kreg{w}"),
    }
}

/// 读文件 + 校验（CLI 的 `validate <file>` 用）；返回 (渲染好的诊断行, ISA 名)。
pub fn validate_file(path: &Path) -> (Vec<DiagLine>, Option<String>) {
    validate_file_opts(path, false)
}

/// 读文件 + 带档位校验（v19 V6b：`validate --strict-overlap`）。
pub fn validate_file_opts(path: &Path, strict_overlap: bool) -> (Vec<DiagLine>, Option<String>) {
    // 加载失败（缺 include / 成环 / 同名标量冲突 / `[[override]]` 目标不存在…）：
    // **原样**把加载器的消息交出去。不要改写成"读不到文件：<根路径>"——那会把
    // "片段缺文件"这类真正可诊断的问题伪装成"根文件读不到"（v18 S7d 修）。
    let spec = match crate::loader::LoadedSpec::load(path) {
        Ok(s) => s,
        Err(e) => {
            return (
                e.lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(DiagLine::plain)
                    .collect(),
                None,
            );
        }
    };
    let diags = validate_loaded_opts(&spec, strict_overlap);
    let name = crate::v12::parse_and_validate(&spec.text)
        .ok()
        .map(|m| m.meta.name);
    (diags, name)
}

/// 读文件 + 展开（CLI 的 `insts`/`explain`/`diff` 用）。
pub fn read_source(path: &Path) -> Result<String, Vec<DiagLine>> {
    std::fs::read_to_string(path).map_err(|e| {
        vec![DiagLine::plain(&format!(
            "读不到文件 {}：{e}",
            path.display()
        ))]
    })
}

// ─────────────────── 多文件谱（include）入口（v18 S7d）───────────────────
//
// 注意：这些**必须排在 `mod tests` 之前**——clippy 的 `items_after_test_module`
// 会拒绝"测试模块之后还有条目"（它抓的正是"新代码顺手加在文件末尾"）。

/// 把诊断的合并行号映射回来源文件（[`crate::loader::LoadedSpec::map_line`]）。
fn map_files(spec: &crate::loader::LoadedSpec, diags: &mut [DiagLine]) {
    for d in diags.iter_mut() {
        let (file, line) = spec.map_line(d.line);
        d.file = Some(file.display().to_string());
        d.line = line;
    }
}

/// 校验已加载的谱（支持 `include`；诊断带来源文件）。
pub fn validate_loaded(spec: &crate::loader::LoadedSpec) -> Vec<DiagLine> {
    validate_loaded_opts(spec, false)
}

/// 带档位校验已加载的谱（v19 V6b：`strict_overlap` = `validate --strict-overlap`）。
///
/// 对外只暴露 `bool`（`ValidateOpts` 是 crate 内部档位结构），CLI 与工具不必依赖 `v12`。
pub fn validate_loaded_opts(
    spec: &crate::loader::LoadedSpec,
    strict_overlap: bool,
) -> Vec<DiagLine> {
    let mut d = validate_opts(&spec.text, strict_overlap);
    map_files(spec, &mut d);
    d
}

/// 带档位校验源码（供 CLI / 工具用）。
pub fn validate_opts(source: &str, strict_overlap: bool) -> Vec<DiagLine> {
    let opts = crate::v12::validate::ValidateOpts { strict_overlap };
    match crate::v12::parse_and_validate_opts(source, &opts) {
        Ok(_) => Vec::new(),
        Err(e) => Vec::from(&e),
    }
}

/// 展开已加载的谱（支持 `include`；诊断带来源文件）。
pub fn insts_loaded(
    spec: &crate::loader::LoadedSpec,
) -> Result<(IsaSummary, Vec<InstRow>), Vec<DiagLine>> {
    insts(&spec.text).map_err(|mut d| {
        map_files(spec, &mut d);
        d
    })
}

/// 单条指令解释（支持 `include`）。
pub fn explain_loaded(
    spec: &crate::loader::LoadedSpec,
    name: &str,
) -> Result<Explain, Vec<DiagLine>> {
    explain(&spec.text, name).map_err(|mut d| {
        map_files(spec, &mut d);
        d
    })
}

/// 两份谱的规格 diff（支持 `include`）。
pub fn diff_loaded(
    a: &crate::loader::LoadedSpec,
    b: &crate::loader::LoadedSpec,
) -> Result<SpecDiff, Vec<DiagLine>> {
    diff(&a.text, &b.text).map_err(|mut d| {
        map_files(a, &mut d);
        d
    })
}

/// 读文件 + 展开为 [`crate::loader::LoadedSpec`]（CLI 用；读不到/include 错按诊断返回）。
pub fn load_spec(path: &Path) -> Result<crate::loader::LoadedSpec, Vec<DiagLine>> {
    crate::loader::LoadedSpec::load(path).map_err(|e| vec![DiagLine::plain(&e)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小可用谱：一条手写 + 一条由模板展开（用来验证 provenance 与 diff）。
    const SPEC: &str = r#"
[meta]
name = "report_demo"
[encoding]
kind = "fixed"
bits = 16
[reg.gpr4]
names = ["R0", "R1", "R2", "R3"]
[conventions.bitfields]
op    = { offset = 12, width = 4 }
rd    = { offset = 8, width = 4 }
rs1   = { offset = 4, width = 4 }
[[operand_slots]]
name = "r"
kind = "reg"
class = "gpr4"
roles = ["in", "out"]
[[forms]]
name = "RR"
opcode_field = "op"
operand_fields = ["rd", "rs1"]
[[instructions]]
name = "MOV"
form = "RR"
opcode = 1
ops = ["dst:r:out", "src:r"]
asm = "mov {dst}, {src}"
[[templates]]
name = "ALU"
body = { form = "RR", ops = ["dst:r:out", "src:r"], asm = "alu {dst}, {src}" }
rows = [
  { inst = "ADD", opcode = 2 },
  { inst = "SUB", opcode = 3 },
]
"#;

    #[test]
    fn validate_ok_and_reports_errors() {
        assert!(validate(SPEC).is_empty(), "合法谱不应有诊断");
        let bad = SPEC.replace(r#"form = "RR""#, r#"form = "NOPE""#);
        let diags = validate(&bad);
        assert!(!diags.is_empty(), "未声明的 form 必须报错");
        assert!(
            diags
                .iter()
                .any(|d| d.code == "DSL-INST" || d.code == "DSL-TEMPLATE"),
            "{diags:?}"
        );
        assert!(diags.iter().all(|d| d.line >= 1 && d.col >= 1));
        assert!(diags[0].render().contains("DSL-"));
    }

    #[test]
    fn insts_expands_templates_with_provenance() {
        let (isa, rows) = insts(SPEC).expect("合法谱");
        assert_eq!(isa.name, "report_demo");
        assert_eq!(isa.encoding_kind, "fixed");
        assert_eq!(isa.widths_bits, vec![16]);
        assert_eq!(isa.templates, 1);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["MOV", "ADD", "SUB"], "声明序 + 展开序");
        let add = &rows[1];
        assert_eq!(add.from_template.as_deref(), Some("ALU"));
        assert_eq!(add.template_row, Some(1), "模板里的第 1 行");
        assert_eq!(add.width_bits, Some(16));
        assert_eq!(add.len_bytes, Some(2), "16 位 = 2 字节");
        assert_eq!(add.opcode, Some(2));
        // 生效编码键含 form 预设的键（单一事实源：collect_inst_infos）。
        assert!(
            add.enc
                .iter()
                .any(|(k, v)| k == "opcode_field" && v.contains("op")),
            "{:?}",
            add.enc
        );
        assert!(
            add.enc.iter().any(|(k, _)| k == "operand_fields"),
            "{:?}",
            add.enc
        );
    }

    #[test]
    fn explain_gives_template_row_and_effective_spec() {
        let e = explain(SPEC, "SUB").expect("存在");
        assert_eq!(e.inst.name, "SUB");
        assert_eq!(e.inst.opcode, Some(3));
        let t = e.template.expect("由模板展开");
        assert_eq!(t.template, "ALU");
        assert_eq!(t.index, 2);
        assert!(t.body.iter().any(|(k, v)| k == "form" && v.contains("RR")));
        assert!(t.row.iter().any(|(k, v)| k == "opcode" && v == "3"));
    }

    #[test]
    fn explain_unknown_instruction_lists_candidates() {
        let diags = explain(SPEC, "NOPE").expect_err("不存在");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].msg.contains("没有名为 'NOPE'"), "{}", diags[0].msg);
        assert!(diags[0].msg.contains("ADD"), "应列出候选：{}", diags[0].msg);
    }

    #[test]
    fn diff_reports_added_removed_changed() {
        let same = diff(SPEC, SPEC).expect("合法");
        assert!(same.is_empty(), "{same:?}");

        // 改一个 opcode + 加一条指令。
        let other = SPEC
            .replace("{ inst = \"SUB\", opcode = 3 }", "{ inst = \"SUB\", opcode = 7 }")
            .replace(
                "[[templates]]",
                "[[instructions]]\nname = \"NOP\"\nform = \"RR\"\nopcode = 0\nops = [\"dst:r:out\", \"src:r\"]\nasm = \"nop {dst}, {src}\"\n\n[[templates]]",
            );
        let d = diff(SPEC, &other).expect("合法");
        assert_eq!(d.added, vec!["NOP"]);
        assert!(d.removed.is_empty());
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].0, "SUB");
        assert!(
            d.changed[0].1.iter().any(|f| f.starts_with("opcode:")),
            "{:?}",
            d.changed
        );
    }

    #[test]
    fn diff_propagates_diagnostics() {
        let bad = SPEC.replace(r#"form = "RR""#, r#"form = "NOPE""#);
        assert!(diff(SPEC, &bad).is_err(), "坏谱必须报诊断而不是静默 diff");
    }
}
