//! S2 验收：**`[[templates]]`（唯一指令分组机制）**的展开语义。
//!
//! v18 的最终形态：**一个**机制覆盖原先三套（`[[families]]` / `[[aliases]]` /
//! 平行参数表版模板）——行（`[[templates.rows]]`）是事实载体，`body` 是共享默认值，
//! `ref` 是指令的引用名（多条指令共用即多态）。
//!
//! 覆盖：逐行展开、`body`+行深合并、`{inst.lower}` 派生、`ref` 共用/逐行、
//! 整串占位符保留类型、行级嵌套值，以及全部失败路径。
//!
//! 运行：`cargo test -p forge-dsl --lib template -- --nocapture`

use super::parse_and_validate;

/// 最小可展开骨架：32 位定宽 ISA，两个寄存器槽（窄/宽各一）。
const BASE: &str = r#"
[meta]
name = "s2_base"
[encoding]
kind = "fixed"
bits = 32

[reg.gpr4]
names = ["W0", "W1", "W2", "W3"]

[reg.gpr8]
names = ["X0", "X1", "X2", "X3"]

[conventions.bitfields]
rd     = { offset = 7,  width = 5 }
rs1    = { offset = 15, width = 5 }
rs2    = { offset = 20, width = 5 }
opcode = { offset = 0,  width = 7 }
funct3 = { offset = 12, width = 3 }
funct7 = { offset = 25, width = 7 }

[[operand_slots]]
name = "r32"
kind = "reg"
class = "gpr4"
roles = ["in", "out"]

[[operand_slots]]
name = "r64"
kind = "reg"
class = "gpr8"
roles = ["in", "out"]

[[operand_slots]]
name = "imm12"
kind = "imm"
signed = true
width = 12

[[forms]]
name = "R"
opcode_field = "opcode"
operand_fields = ["rd", "rs1", "rs2"]
"#;

fn plus(extra: &str) -> String {
    format!("{BASE}\n{extra}\n")
}

fn inst_names(doc: &str) -> Vec<String> {
    let m = parse_and_validate(doc).expect("夹具必须可解析");
    m.instructions
        .iter()
        .filter(|i| i.from_template.is_some())
        .map(|i| i.name.clone())
        .collect()
}

fn err_msg(doc: &str) -> String {
    match parse_and_validate(doc) {
        Ok(_) => panic!("期望展开失败"),
        Err(e) => e.to_string(),
    }
}

// ─────────────────────── 正常展开 ───────────────────────

#[test]
fn one_row_one_instruction() {
    let doc = plus(
        r#"
[[templates]]
[[templates.rows]]
inst = "ADDW"
form = "R"
opcode = 0x33
ops = ["rd:r32:out", "rs1:r32", "rs2:r32"]
asm = "add {rd}, {rs1}, {rs2}"
[[templates.rows]]
inst = "ADDX"
form = "R"
opcode = 0x3B
ops = ["rd:r64:out", "rs1:r64", "rs2:r64"]
asm = "add {rd}, {rs1}, {rs2}"
"#,
    );
    let m = parse_and_validate(&doc).expect("展开");
    assert_eq!(inst_names(&doc), vec!["ADDW", "ADDX"]);
    assert_eq!(m.instructions[0].opcode, Some(0x33));
    assert_eq!(m.instructions[1].opcode, Some(0x3B));
    assert_eq!(m.instructions[1].ops.as_ref().unwrap()[0], "rd:r64:out");
    assert_eq!(m.instructions[0].from_template.as_deref(), Some("ADDW"));
}

#[test]
fn body_is_shared_defaults_and_rows_deep_merge() {
    let doc = plus(
        r#"
[[templates]]
name = "ADD"
body = { form = "R", opcode = 0x33, ops = ["rd:{slot}:out", "rs1:{slot}", "rs2:{slot}"], asm = "add {rd}, {rs1}, {rs2}", fields = { funct3 = 0, funct7 = 0 } }
[[templates.rows]]
inst = "ADDW"
slot = "r32"
[[templates.rows]]
inst = "ADDX"
slot = "r64"
fields = { funct7 = 1 }
"#,
    );
    let m = parse_and_validate(&doc).expect("展开");
    let w = &m.instructions[0];
    let x = &m.instructions[1];
    assert_eq!(w.ops.as_ref().unwrap()[0], "rd:r32:out");
    assert_eq!(x.ops.as_ref().unwrap()[0], "rd:r64:out");
    // fields 递归合并：行只覆盖 funct7，funct3 仍是 body 的
    let xf = x.fields.as_ref().unwrap();
    assert_eq!(xf.get("funct3"), Some(&0));
    assert_eq!(xf.get("funct7"), Some(&1));
    assert_eq!(w.fields.as_ref().unwrap().get("funct7"), Some(&0));
}

#[test]
fn inst_lower_derives_the_mnemonic() {
    // 原先 `[[families]]` 的"变体名小写即助记符"由此**显式**表达。
    let doc = plus(
        r#"
[[templates]]
name = "RR"
body = { form = "R", opcode = 0x33, ops = ["rd:r64:out", "rs1:r64", "rs2:r64"], asm = "{inst.lower} {rd}, {rs1}, {rs2}" }
[[templates.rows]]
inst = "ADD"
fields = { funct3 = 0, funct7 = 0 }
[[templates.rows]]
inst = "SUB"
fields = { funct3 = 0, funct7 = 0x20 }
"#,
    );
    let m = parse_and_validate(&doc).expect("展开");
    assert_eq!(inst_names(&doc), vec!["ADD", "SUB"]);
    assert_eq!(m.instructions[0].asm, "add {rd}, {rs1}, {rs2}");
    assert_eq!(m.instructions[1].asm, "sub {rd}, {rs1}, {rs2}");
}

#[test]
fn whole_string_placeholder_keeps_the_type() {
    let doc = plus(
        r#"
[[templates]]
body = { form = "R", opcode = "{op}", ops = ["rd:{slot}:out"], asm = "op {rd}", fields = { funct3 = "{f3}" } }
[[templates.rows]]
inst = "OPW"
op = 0x33
f3 = 0
slot = "r32"
[[templates.rows]]
inst = "OPX"
op = 0x3B
f3 = 1
slot = "r64"
"#,
    );
    let m = parse_and_validate(&doc).expect("展开");
    assert_eq!(m.instructions[0].opcode, Some(0x33), "opcode 必须是整数");
    assert_eq!(
        m.instructions[1].fields.as_ref().unwrap().get("funct3"),
        Some(&1),
        "fields 值也保留整数类型"
    );
}

#[test]
fn rows_can_carry_nested_values() {
    // 行键空间就是 TOML：可以让**整条 `ops` 数组**逐行不同（旧平行参数表做不到）。
    let doc = plus(
        r#"
[[templates]]
body = { form = "R", opcode = 0x33, asm = "op {rd}" }
[[templates.rows]]
inst = "OPA"
ops = ["rd:r64:out", "rs1:r64", "rs2:r64"]
[[templates.rows]]
inst = "OPB"
ops = ["rd:r64:out", "rs1:r64"]
"#,
    );
    let m = parse_and_validate(&doc).expect("展开");
    assert_eq!(m.instructions[0].ops.as_ref().unwrap().len(), 3);
    assert_eq!(m.instructions[1].ops.as_ref().unwrap().len(), 2);
}

// ─────────────────────── ref（唯一的引用名机制）───────────────────────

#[test]
fn rows_sharing_a_ref_form_one_polymorphic_reference() {
    let doc = plus(
        r#"
[[templates]]
body = { form = "R", asm = "add {rd}, {rs1}, {rs2}", ref = "add" }
[[templates.rows]]
inst = "ADDW"
opcode = 0x33
ops = ["rd:r32:out", "rs1:r32", "rs2:r32"]
[[templates.rows]]
inst = "ADDX"
opcode = 0x3B
ops = ["rd:r64:out", "rs1:r64", "rs2:r64"]

[[lowering]]
op = "Iadd"
insts = ["add {out}, {0}, {1}"]
"#,
    );
    let m = parse_and_validate(&doc).expect("共用 ref 的多态引用必须合法");
    assert_eq!(
        m.instructions[0].reference.as_deref(),
        Some("add"),
        "body 里的 ref 落到每一行"
    );
    assert_eq!(m.lowering.len(), 1);
}

#[test]
fn ref_can_be_overridden_per_row() {
    let doc = plus(
        r#"
[[templates]]
body = { form = "R", ops = ["rd:r64:out", "rs1:r64"], ref = "ld" }
[[templates.rows]]
inst = "LDW"
opcode = 0x03
asm = "ldw {rd}, {rs1}"
[[templates.rows]]
inst = "LDX"
opcode = 0x13
asm = "ldx {rd}, {rs1}"
ref = "ldx64"
"#,
    );
    let m = parse_and_validate(&doc).expect("行级 ref 覆盖");
    assert_eq!(m.instructions[0].reference.as_deref(), Some("ld"));
    assert_eq!(m.instructions[1].reference.as_deref(), Some("ldx64"));
}

#[test]
fn ref_on_plain_instruction_feeds_the_same_namespace() {
    let doc = plus(
        r#"
[[instructions]]
name = "MOVW"
ref = "mov"
form = "R"
opcode = 0x33
ops = ["rd:r32:out", "rs1:r32"]
asm = "mov {rd}, {rs1}"

[[templates]]
[[templates.rows]]
inst = "MOVX"
ref = "mov"
form = "R"
opcode = 0x3B
ops = ["rd:r64:out", "rs1:r64"]
asm = "mov {rd}, {rs1}"

[[lowering]]
op = "Copy"
insts = ["mov {out}, {0}"]
"#,
    );
    let m = parse_and_validate(&doc).expect("手写指令与模板行可共用同一个 ref");
    let refs: Vec<&str> = m
        .instructions
        .iter()
        .filter_map(|i| i.reference.as_deref())
        .collect();
    assert_eq!(refs, vec!["mov", "mov"]);
}

// ─────────────────────── 失败路径 ───────────────────────

#[test]
fn rejects_empty_rows_and_empty_inst() {
    let empty = plus("[[templates]]\nrows = []\n");
    assert!(
        err_msg(&empty).contains("rows 不能为空"),
        "{}",
        err_msg(&empty)
    );

    let bad_inst = plus(
        r#"
[[templates]]
[[templates.rows]]
inst = "  "
form = "R"
opcode = 0x33
ops = ["rd:r32:out"]
asm = "op {rd}"
"#,
    );
    assert!(
        err_msg(&bad_inst).contains("`inst` 不能为空"),
        "{}",
        err_msg(&bad_inst)
    );
}

#[test]
fn param_typo_is_caught_by_operand_validation() {
    // 行键名打错（`slott`）⇒ 插值原样保留 `{slot}`，展开出的指令引用未声明的槽，
    // 由既有的槽校验拒绝——诊断锚回**模板声明行**。
    let doc = plus(
        r#"
[[templates]]
body = { form = "R", opcode = 0x33, ops = ["rd:{slot}:out"], asm = "op {rd}" }
[[templates.rows]]
inst = "OPW"
slott = "r32"
"#,
    );
    let msg = err_msg(&doc);
    assert!(
        msg.contains("slott") && msg.contains("[[templates."),
        "应点名拼错的键并锚回模板：{msg}"
    );
}

#[test]
fn inner_dsl_placeholders_survive_untouched() {
    // `asm` 里的 `{rd}`/`{imm}` 是**操作数**占位符，不参与模板插值。
    let doc = plus(
        r#"
[[templates]]
body = { form = "R", opcode = 0x33, ops = ["rd:{slot}:out", "rs1:{slot}", "imm:imm12"], asm = "op {rd}, {rs1}, #{imm}" }
[[templates.rows]]
inst = "OPW"
slot = "r32"
"#,
    );
    let m = parse_and_validate(&doc).expect("展开");
    assert_eq!(m.instructions[0].asm, "op {rd}, {rs1}, #{imm}");
    assert_eq!(m.instructions[0].ops.as_ref().unwrap()[0], "rd:r32:out");
}

#[test]
fn rejects_scalar_body_and_bad_row_field() {
    let scalar = plus(
        r#"
[[templates]]
body = "not a table"
[[templates.rows]]
inst = "OPW"
"#,
    );
    let msg = err_msg(&scalar);
    assert!(msg.contains("body") || msg.contains("TOML"), "{msg}");

    // 行里拼错的指令字段由 Instruction 的反序列化拒绝（点名实例）
    let bad_field = plus(
        r#"
[[templates]]
[[templates.rows]]
inst = "OPW"
form = "R"
opcode = 0x33
opss = ["rd:r32:out"]
asm = "op {rd}"
"#,
    );
    let msg = err_msg(&bad_field);
    assert!(msg.contains("OPW"), "应点名实例：{msg}");
}

#[test]
fn duplicate_instance_names_are_rejected() {
    let doc = plus(
        r#"
[[templates]]
body = { form = "R", opcode = 0x33, ops = ["rd:r32:out"], asm = "op {rd}" }
[[templates.rows]]
inst = "OPW"
[[templates.rows]]
inst = "OPW"
"#,
    );
    let msg = err_msg(&doc);
    assert!(msg.contains("duplicate"), "{msg}");
    assert!(msg.contains("DSL-INST"), "应带错误码：{msg}");
}

#[test]
fn ref_colliding_with_an_instruction_name_is_rejected() {
    let doc = plus(
        r#"
[[instructions]]
name = "OPW"
form = "R"
opcode = 0x33
ops = ["rd:r32:out"]
asm = "op {rd}"

[[templates]]
[[templates.rows]]
inst = "OPX"
ref = "OPW"
form = "R"
opcode = 0x3B
ops = ["rd:r64:out"]
asm = "op {rd}"
"#,
    );
    let msg = err_msg(&doc);
    assert!(msg.contains("与指令名冲突"), "{msg}");
}

#[test]
fn base_fixture_is_valid() {
    parse_and_validate(BASE).expect("骨架必须合法");
}
