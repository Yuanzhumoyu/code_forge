//! ISA-DSL 静态体检（v19 V4a）：**不执行、不编译**，只回答"这份谱里有没有写了却没用上的东西"。
//!
//! 与 `validate` 的分工：`validate` 判"对不对"（错就编译不过），lint 判"干不干净 / 有没有
//! 笔误"（合法但可疑）。所以 lint 有自己的子命令与退出码（`forge-isa lint`），且**零误报**
//! 是硬要求——凡是有"作者可能故意这么写"空间的规则都不进 V4a。
//!
//! V4a 两条规则（都只看模型）：
//!
//! 1. `LINT-UNUSED-SLOT`：`[[operand_slots]]` 没有被任何 `ops = ["名:槽[:角色]", …]` 引用；
//! 2. `LINT-UNUSED-FORM`：`[[forms]]` 没有被任何指令 / 模板 `body.form` 引用。
//!
//! **不重复报解析期已经拦住的东西**：模板行里的键笔误（`typo_key = 1`）在 `TemplateRow` 的
//! `deny_unknown_fields` 下就是硬错误，lint 再报一次只会变成噪声——这条规则写完才发现，
//! 直接删掉（留这条注释给后来者）。
//!
//! 位域 / `ref` / 能力缺口留给 V4b（需要展开后的指令视图与宿主管线清单，误报面更大；
//! V0 已把"终结指令 / 宿主管线 / 真缺口"三类分开的口径定好）。

use std::collections::BTreeSet;

use crate::report::DiagLine;
use crate::v12::diag::DeclIndex;
use crate::v12::model::V12Model;

/// 对一个**已加载**的谱文件做体检（解析失败时返回诊断，交给调用方按 `validate` 的样式渲染）。
pub fn lint_source(source: &str) -> Result<Vec<DiagLine>, Vec<DiagLine>> {
    let model = match crate::v12::parse_and_validate(source) {
        Ok(m) => m,
        Err(e) => return Err(Vec::from(&e)),
    };
    let idx = DeclIndex::build(source);
    Ok(checks(&model, &idx))
}

/// 三条规则的实现。
fn checks(m: &V12Model, idx: &DeclIndex) -> Vec<DiagLine> {
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

    // ── 3. 模板行键笔误：**不报**（解析期 `deny_unknown_fields` 已是硬错误，见模块头）──

    out
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
fn anchor(idx: &DeclIndex, path: &str, code: &str, msg: String) -> DiagLine {
    let text = format!("{path}: {msg}");
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

    /// 两条规则各报一条：`unused_slot` / `UNUSED_FORM`；在用的 `g`/`RR` 不报。
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
}
