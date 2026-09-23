//! ISA-DSL 静态体检（v19 V4a）：**不执行、不编译**，只回答"这份谱里有没有写了却没用上的东西"。
//!
//! 与 `validate` 的分工：`validate` 判"对不对"（错就编译不过），lint 判"干不干净 / 有没有
//! 笔误"（合法但可疑）。所以 lint 有自己的子命令与退出码（`forge-isa lint`），且**零误报**
//! 是硬要求——凡是有"作者可能故意这么写"空间的规则都不进 V4a。
//!
//! V4a/V4b 三条规则（都只看模型 / 合并后的文本）：
//!
//! 1. `LINT-UNUSED-SLOT`：`[[operand_slots]]` 没有被任何 `ops = ["名:槽[:角色]", …]` 引用；
//! 2. `LINT-UNUSED-FORM`：`[[forms]]` 没有被任何指令 / 模板 `body.form` 引用；
//! 3. `LINT-UNUSED-BITFIELD`（V4b）：`[conventions.bitfields]` 的位域名在整份谱里**只出现在
//!    声明处**。判据故意用**文本标识符计数**而不是"遍历模型找引用点"——引用形态太多
//!    （form 的 `operand_fields`、指令/模板 `fields`、编码键 `imm = "imm12"`、asm 占位符…），
//!    枚举必然漏、漏了就成误报；文本计数的代价只是"注释里提到也算引用"（漏报，可接受）。
//!
//! **一行锚定是近似的**：位域的锚定走 `DeclIndex::anchor`，它会按名字找行——名字若不是
//! 独立标识符（如 `p6` 是 `op6` 的子串），可能落到同一节里邻近的行上。消息里始终带着
//! 位域名，因此不影响可操作性与"零误报"判据。
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
    Ok(checks(&model, source, &idx))
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

/// 三条规则的实现。
fn checks(m: &V12Model, source: &str, idx: &DeclIndex) -> Vec<DiagLine> {
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
}
