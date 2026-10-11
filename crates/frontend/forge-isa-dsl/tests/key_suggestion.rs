//! **未知键建议**（v22 ③）：TOML 里写错键名时，诊断必须点明**所属节**并给出**编辑距离 ≤2**
//! 的候选（实现见 `forge_isa_dsl::schema::unknown_key_hint`）——serde 原文只有
//! ``unknown field `xx` ``，作者得自己回查是哪一节、更像哪个键。

use forge_isa_dsl::report::{self, DiagLine};

/// 诊断文本（`DiagLine` 的 `Debug` 含 `msg` 字段，与 `inline_ops.rs` 同一口径）。
fn rendered(diags: Vec<DiagLine>) -> String {
    format!("{diags:?}")
}

/// 最小可解析谱（定宽 16 位、一个 imm 槽），只替换 `[inst.A]` 里的一行。
fn spec_with(line: &str) -> String {
    format!(
        r#"[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 16
[reg.g]
count = 8
[operand.imm4]
kind = "imm"
bits = 4
[form.F]
fields = ["u4[15:12]:opcode", "u4[3:0]:imm4"]
[inst.A]
{line}
opcode = 1
asm = "a {{k:imm4}}"
"#
    )
}

#[test]
fn unknown_key_gets_section_and_near_candidate() {
    let err = rendered(report::insts(&spec_with("forn = \"F\"")).unwrap_err());
    assert!(
        err.contains("unknown field `forn`"),
        "要保留 serde 原文：{err}"
    );
    assert!(err.contains("`inst.A`"), "要点明所属节：{err}");
    assert!(
        err.contains("你是不是想写 `form`"),
        "编辑距离 1 的 `form` 必须被建议出来：{err}"
    );
    assert!(
        err.contains("本节合法键"),
        "没有近候选时也要列出本节的键：{err}"
    );
    // 建议里不该出现已撤出对外表面的遗留键（v22：`ops`/`bind` 不在 schema 里）。
    assert!(
        !err.contains("`ops`") && !err.contains("`bind`"),
        "遗留键不该出现在建议里：{err}"
    );
}

#[test]
fn unknown_key_in_form_section_points_at_that_section() {
    let src = r#"[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 16
[reg.g]
count = 8
[operand.imm4]
kind = "imm"
bits = 4
[form.F]
fieldz = ["u4[15:12]:opcode"]
[inst.A]
form = "F"
opcode = 1
asm = "a {k:imm4}"
"#;
    let err = rendered(report::insts(src).unwrap_err());
    assert!(err.contains("unknown field `fieldz`"), "{err}");
    assert!(err.contains("`form.F`"), "要点明是 form 节：{err}");
    assert!(
        err.contains("你是不是想写 `fields`"),
        "编辑距离 1 的 `fields` 必须被建议出来：{err}"
    );
}
