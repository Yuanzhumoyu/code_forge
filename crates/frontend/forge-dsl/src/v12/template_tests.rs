//! S2 验收：**参数化模板**（`[[templates]]`）展开语义。
//!
//! 覆盖：逐行替换（字符串/整数）、`ref` 派生别名（共用 vs 逐实例）、逐行补丁
//! （表递归合并 + 列表覆盖）、以及全部失败路径（参数域不等长 / 名字缺失 / body 非表 /
//! 行号越界 / 实例重名），最后验证展开出的指令**能被 lowering 直接引用**。
//!
//! 运行：`cargo test -p forge-dsl --lib template -- --nocapture`

use super::parse_and_validate;

/// 最小可展开骨架：一个 32 位定宽 ISA，两个寄存器槽（窄/宽各一）。
const BASE: &str = r#"
[meta]
name = "s2_base"
default_inst_width = 32

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
extra  = { offset = 25, width = 5 }

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

/// 展开后的指令名（按声明序）。
fn inst_names(doc: &str) -> Vec<String> {
    let m = parse_and_validate(doc).expect("夹具必须可解析");
    m.instructions.iter().map(|i| i.name.clone()).collect()
}

/// 展开失败消息（成功即 panic）。
fn err_msg(doc: &str) -> String {
    match parse_and_validate(doc) {
        Ok(_) => panic!("期望展开失败"),
        Err(e) => e.to_string(),
    }
}

// ─────────────────────── 正常展开 ───────────────────────

#[test]
fn expands_one_row_per_parameter_row() {
    let doc = plus(
        r#"
[[templates]]
name = "ADD{p}"
ref = "add"
params = { p = ["W", "X"], slot = ["r32", "r64"], opcode = [0x33, 0x3B] }
body = { form = "R", opcode = "{opcode}", ops = ["rd:{slot}:out", "rs1:{slot}", "rs2:{slot}"], asm = "add {rd}, {rs1}, {rs2}" }
"#,
    );
    let m = parse_and_validate(&doc).expect("展开");
    assert_eq!(inst_names(&doc), vec!["ADDW", "ADDX"]);
    let w = &m.instructions[0];
    assert_eq!(w.opcode, Some(0x33));
    assert_eq!(w.ops.as_ref().unwrap()[0], "rd:r32:out");
    assert_eq!(m.instructions[1].opcode, Some(0x3B));
    assert_eq!(m.instructions[1].ops.as_ref().unwrap()[0], "rd:r64:out");
    // 整数参数**保留类型**（`opcode = "{opcode}"` 不是文本替换成字符串）
    assert!(w.opcode.is_some(), "opcode 必须是整数");
    // ref 共用 ⇒ 一条别名，成员按行序
    let alias = m
        .aliases
        .iter()
        .find(|a| a.name == "add")
        .expect("ref 应派生别名");
    assert_eq!(alias.insts, vec!["ADDW", "ADDX"]);
    // 展开来源（诊断用）
    assert_eq!(w.from_template.as_deref(), Some("ADD{p}"));
}

#[test]
fn ref_may_be_interpolated_per_instance() {
    let doc = plus(
        r#"
[[templates]]
names = ["LDW", "LDX"]
ref = "ld{m}"
params = { m = ["w", "x"], slot = ["r32", "r64"], opcode = [0x03, 0x13] }
body = { form = "R", opcode = "{opcode}", ops = ["rd:{slot}:out", "rs1:{slot}"], asm = "ld{m} {rd}, {rs1}" }
"#,
    );
    let m = parse_and_validate(&doc).expect("展开");
    assert_eq!(inst_names(&doc), vec!["LDW", "LDX"]);
    let names: Vec<&str> = m.aliases.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["ldw", "ldx"], "逐实例引用名");
    assert_eq!(m.aliases[0].insts, vec!["LDW"]);
    // asm 也逐行替换
    assert_eq!(m.instructions[1].asm, "ldx {rd}, {rs1}");
}

#[test]
fn overrides_patch_rows_and_deep_merge_tables() {
    let doc = plus(
        r#"
[[templates]]
name = "M{m}"
ref = "mv{m}"
params = { m = ["W", "X"], slot = ["r32", "r64"], opcode = [0x0B, 0x1B] }
body = { form = "R", opcode = "{opcode}", ops = ["rd:{slot}:out", "rs1:{slot}"], asm = "mv{m} {rd}, {rs1}", fields = { funct3 = 1 } }
[[templates.overrides]]
row = 1
body = { fields = { funct3 = 2, extra = 7 }, roles = ["gpr_mov"] }
"#,
    );
    let m = parse_and_validate(&doc).expect("展开");
    let w = &m.instructions[0];
    assert_eq!(w.fields.as_ref().unwrap().get("funct3"), Some(&1));
    assert!(w.fields.as_ref().unwrap().get("extra").is_none());
    assert!(w.roles.is_empty());
    let x = &m.instructions[1];
    let f = x.fields.as_ref().unwrap();
    assert_eq!(f.get("funct3"), Some(&2), "补丁覆盖同键");
    assert_eq!(f.get("extra"), Some(&7), "补丁新增键");
    assert_eq!(x.roles.len(), 1, "补丁给列表键");
}

#[test]
fn expanded_instructions_are_referenceable_from_lowering() {
    let doc = plus(
        r#"
[[templates]]
name = "ADD{p}"
ref = "add"
params = { p = ["W", "X"], slot = ["r32", "r64"], opcode = [0x33, 0x3B] }
body = { form = "R", opcode = "{opcode}", ops = ["rd:{slot}:out", "rs1:{slot}", "rs2:{slot}"], asm = "add {rd}, {rs1}, {rs2}" }

[[lowering]]
op = "Iadd"
insts = ["add {out}, {0}, {1}"]
"#,
    );
    let m = parse_and_validate(&doc).expect("展开 + lowering 引用别名必须合法");
    assert_eq!(m.lowering.len(), 1);
    // 也可以按**实例名**直接引用
    let by_name = doc.replace(
        "insts = [\"add {out}, {0}, {1}\"]",
        "insts = [\"ADDX {out}, {0}, {1}\"]",
    );
    parse_and_validate(&by_name).expect("按实例名引用也必须合法");
}

// ─────────────────────── 失败路径 ───────────────────────

#[test]
fn rejects_unequal_parameter_domains() {
    let doc = plus(
        r#"
[[templates]]
name = "T{p}"
ref = "t"
params = { p = ["A", "B"], slot = ["r32"] }
body = { form = "R", opcode = 0x33, ops = ["rd:{slot}:out"], asm = "t {rd}" }
"#,
    );
    let msg = err_msg(&doc);
    assert!(msg.contains("等长"), "{msg}");
    assert!(msg.contains("[[templates."), "应带模板前缀：{msg}");
}

#[test]
fn rejects_missing_and_mismatched_names() {
    let no_name = plus(
        r#"
[[templates]]
ref = "t"
params = { p = ["A"] }
body = { form = "R", opcode = 0x33, ops = ["rd:r32:out"], asm = "t {rd}" }
"#,
    );
    assert!(err_msg(&no_name).contains("name"), "{}", err_msg(&no_name));

    let bad_len = plus(
        r#"
[[templates]]
names = ["A", "B", "C"]
ref = "t"
params = { p = ["A", "B"] }
body = { form = "R", opcode = 0x33, ops = ["rd:r32:out"], asm = "t {rd}" }
"#,
    );
    assert!(
        err_msg(&bad_len).contains("names 长"),
        "{}",
        err_msg(&bad_len)
    );
}

#[test]
fn rejects_empty_params_and_non_table_body() {
    let empty = plus(
        r#"
[[templates]]
name = "T"
ref = "t"
params = {}
body = { form = "R", opcode = 0x33, ops = ["rd:r32:out"], asm = "t {rd}" }
"#,
    );
    assert!(
        err_msg(&empty).contains("params 不能为空"),
        "{}",
        err_msg(&empty)
    );

    let scalar_body = plus(
        r#"
[[templates]]
name = "T{p}"
ref = "t"
params = { p = ["A"] }
body = "not a table"
"#,
    );
    assert!(
        err_msg(&scalar_body).contains("body 必须是内联表"),
        "{}",
        err_msg(&scalar_body)
    );
}

#[test]
fn rejects_bad_instance_body() {
    // 展开后的 body 缺必填 `asm` ⇒ 反序列化错误，消息点名实例。
    let doc = plus(
        r#"
[[templates]]
name = "T{p}"
ref = "t"
params = { p = ["A"] }
body = { form = "R", opcode = 0x33, ops = ["rd:r32:out"] }
"#,
    );
    let msg = err_msg(&doc);
    assert!(
        msg.contains("body 非法") || msg.contains("missing field"),
        "{msg}"
    );
}

#[test]
fn expanded_duplicate_instance_names_are_rejected_by_validation() {
    // 模板与手写指令同名 ⇒ 由既有的重名校验拒绝（诊断带模板前缀与附注）。
    let doc = plus(
        r#"
[[instructions]]
name = "ADDW"
form = "R"
opcode = 0x33
ops = ["rd:r32:out", "rs1:r32", "rs2:r32"]
asm = "add {rd}, {rs1}, {rs2}"

[[templates]]
name = "ADD{p}"
ref = "add"
params = { p = ["W"] }
body = { form = "R", opcode = 0x33, ops = ["rd:r32:out", "rs1:r32", "rs2:r32"], asm = "add {rd}, {rs1}, {rs2}" }
"#,
    );
    let e = parse_and_validate(&doc).expect_err("重名必须被拒");
    let text = e.to_string();
    assert!(text.contains("duplicate"), "{text}");
    assert!(text.contains("DSL-INST"), "应带错误码：{text}");
}
