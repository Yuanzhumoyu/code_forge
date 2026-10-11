//! 声明注释（`doc`）：继承 / 插值 / 落进生成物 / 未知占位符报错（v21，2026-10-08）。
//!
//! 口径（与 `docs/reference/isa-dsl.md` 的「声明注释」一节同源）：
//! `[inst].doc` > family `body.doc` > `[form].doc`；占位符是**固定小集 + 操作数派生**，
//! 写别的键**报错**（注释没有第二个消费者）；三处都不写 ⇒ 生成物一字不加。
//!
//! 断言两面：① `report::insts` 的 `doc` 列（已插值的解析结果，也是 `insts --json` 的主体）；
//! ② **生成物**里真的出现 `#[doc = …]`（`expand_file` 的扁平 token 串）。

use std::path::Path;

use forge_isa_dsl::{ExpandOptions, Parts, expand_file, report};

/// 一份覆盖全部形态的谱：form 共享注释、指令自带、family 按行插值、以及一条完全没有注释的。
fn spec() -> String {
    r##"[meta]
name = "docspec"
comment_char = "#"

[encoding]
kind = "fixed"
bits = 32

[reg.gpr]
base_index = 0
names = ["T0", "T1"]

[operand.r]
kind = "reg"
class = "gpr"

[form.F]
doc = "form共享:{ops}|槽={dst.slot}"
fields = ["u8[31:24]:opcode", "u5[11:7]:rd", "u5[12:8]:imm"]

[form.G]
fields = ["u8[31:24]:opcode", "u5[11:7]:rd", "u5[12:8]:imm"]

[inst.INHERIT]
form = "F"
opcode = 0x10
asm = "inherit {dst:r:out}"
width = 32

[inst.OWN]
form = "F"
opcode = 0x11
doc = "自己:{name}|form={form}|w={width}|ops={ops}|cls={dst.class}"
asm = "own {dst:r:out}"
width = 32

[inst.NODOC]
form = "G"
opcode = 0x12
asm = "nodoc {dst:r:out}"

[family.ALU]
body = { form = "F", doc = "族:{family}/{name}/{cc}", asm = "alu {cc} {dst:r:out}" }
rows = [
  { inst = "ADDX", opcode = 0x20, cc = "add" },
  { inst = "SUBX", opcode = 0x21, cc = "sub" },
]
"##
    .to_string()
}

fn opts() -> ExpandOptions {
    ExpandOptions {
        spec_tests: false,
        name: None,
        parts: Parts::all(),
        params: Default::default(),
    }
}

/// 展开成扁平 token 串（去掉空白——`#[doc = "…"]` 会渲染成 `#[doc="…"]`）。
fn flatten(spec: &str, tag: &str) -> String {
    let tmp = std::env::temp_dir().join(format!("forge_doc_{tag}_{}.toml", std::process::id()));
    std::fs::write(&tmp, spec).expect("写临时谱");
    let ts = expand_file(tmp.to_str().unwrap(), &opts()).expect("展开谱");
    let _ = std::fs::remove_file(&tmp);
    ts.to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// 取一条指令的 `doc` 列（已继承 + 已插值）。
fn doc_of(spec: &str, name: &str) -> Option<String> {
    let (_s, rows) = report::insts(spec).expect("insts 报告");
    rows.into_iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("报告里应有指令 {name}"))
        .doc
}

#[test]
fn form_doc_is_inherited_and_placeholders_resolve() {
    let s = spec();
    assert_eq!(
        doc_of(&s, "INHERIT").as_deref(),
        Some("form共享:dst|槽=r"),
        "没写 doc 的指令应继承 form 的 doc，且操作数派生占位符按该指令解析"
    );
    assert_eq!(
        doc_of(&s, "OWN").as_deref(),
        Some("自己:OWN|form=F|w=32|ops=dst|cls=gpr32"),
        "指令自己的 doc 覆盖 form，且 name/form/width/操作数派生都解析"
    );
    assert_eq!(doc_of(&s, "NODOC"), None, "三处都没写 ⇒ 没有 doc");
}

#[test]
fn family_body_doc_is_shared_per_row_with_row_keys() {
    let s = spec();
    assert_eq!(
        doc_of(&s, "ADDX").as_deref(),
        Some("族:ALU/ADDX/add"),
        "family 的 body.doc 每行继承：族名/指令名由解析器补、行键由展开期补"
    );
    assert_eq!(doc_of(&s, "SUBX").as_deref(), Some("族:ALU/SUBX/sub"));
}

#[test]
fn resolved_doc_lands_in_generated_inst_enum() {
    let flat = flatten(&spec(), "gen");
    // 生成物里真的带 `#[doc = …]`（扁平后 `#[doc="…"]`）
    for want in [
        r#"#[doc="form共享:dst|槽=r"]"#,
        r#"#[doc="自己:OWN|form=F|w=32|ops=dst|cls=gpr32"]"#,
        r#"#[doc="族:ALU/ADDX/add"]"#,
        r#"#[doc="族:ALU/SUBX/sub"]"#,
    ] {
        assert!(flat.contains(want), "生成物里应有 {want}");
    }
    assert!(
        !flat.contains(r#"#[doc="族:ALU/NODOC"#),
        "没写 doc 的指令不该凭空长出一条 doc"
    );
}

#[test]
fn unknown_or_unavailable_placeholder_fails_closed() {
    let bad = spec().replace(r#"doc = "自己:{name}"#, r#"doc = "自己:{nope}"#);
    let tmp = std::env::temp_dir().join(format!("forge_doc_bad_{}.toml", std::process::id()));
    std::fs::write(&tmp, &bad).expect("写临时谱");
    let err = expand_file(tmp.to_str().unwrap(), &opts()).expect_err("未知占位符应报错");
    let _ = std::fs::remove_file(&tmp);
    let msg = err.to_string();
    assert!(
        msg.contains("nope") && msg.contains("[inst.OWN]"),
        "错误应点名占位符与节：{msg}"
    );
}

/// `{{` / `}}` 是字面花括号（注释里写 `{}` 的唯一合法方式）。
#[test]
fn literal_braces_are_via_doubling() {
    let s = spec().replace(
        r#"自己:{name}|form={form}"#,
        r#"字面{{大括号}}与{name}|form={form}"#,
    );
    assert_eq!(
        doc_of(&s, "OWN").as_deref(),
        Some("字面{大括号}与OWN|form=F|w=32|ops=dst|cls=gpr32"),
        "双写应还原成单个花括号"
    );
}

/// 仓库里的发行谱暂时没写 `doc`（可选特性）——它们仍必须能展开。
#[test]
fn shipped_specs_still_expand_without_docs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for isa in ["x86.toml", "riscv64.toml", "arm64.toml"] {
        let p = root.join("isa").join(isa);
        let ts = expand_file(p.to_str().unwrap(), &opts())
            .unwrap_or_else(|e| panic!("展开 {isa} 失败：{e}"));
        let flat: String = ts
            .to_string()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(flat.contains("pubenumInst"), "{isa} 应生成 Inst 枚举");
    }
}
