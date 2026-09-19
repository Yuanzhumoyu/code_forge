//! S0 基线：**诊断行为的现状取证**（`docs/plans/forge-dsl-v18-plan.md` §2.5/§2.7 的证据）。
//!
//! 本文件只做一件事：把"坏 spec 会得到什么"实测记录下来，供 S1（多错误 + 精确 span +
//! 错误码）与 S3（`[emit]`/`[spill]` 校验）对照。因此这里的断言**故意写成现状**
//! （例如"只报 1 条"），S1/S3 落地时必须改写本文件——这是有意为之的"基线钉"。
//!
//! 运行：
//! `cargo test -p forge-dsl --lib s0_baseline -- --nocapture`

use super::parse_and_validate;

/// 最小合法骨架（riscv 风格定宽 ISA），坏用例都在它上面改一处或加一处。
const BASE: &str = r#"
[meta]
name = "s0_base"
default_inst_width = 32

[reg.gpr8]
names = ["X0", "X1", "X2", "X3"]

[conventions.bitfields]
rd     = { offset = 7,  width = 5 }
rs1    = { offset = 15, width = 5 }
rs2    = { offset = 20, width = 5 }
opcode = { offset = 0,  width = 7 }
funct3 = { offset = 12, width = 3 }

[[operand_slots]]
name = "gpr"
kind = "reg"
class = "gpr8"
roles = ["in", "out"]

[[forms]]
name = "R"
opcode_field = "opcode"
operand_fields = ["rd", "rs1", "rs2"]

[[instructions]]
name = "ADD"
form = "R"
opcode = 0x33
fields = { funct3 = 0 }
ops = ["dst:gpr:out", "src:gpr", "src2:gpr"]
asm = "add {dst}, {src}, {src2}"

[[lowering]]
op = "Iadd"
insts = ["ADD {out}, {0}, {1}"]
"#;

/// 追加一段合法 TOML（`BASE` 之后）。
fn plus(extra: &str) -> String {
    format!("{BASE}\n{extra}\n")
}

/// 一屏一批坏 spec，打印"文档 → 诊断"，便于人工比对 S0/S1 的差异。
fn report(name: &str, doc: &str) -> String {
    let msg = diag(doc);
    // 诊断的 `路径` 前缀由宏拼，这里只看 `行:列: 阶段: 消息`。
    let line = msg.lines().next().unwrap_or("").to_string();
    eprintln!("[s0] {name:<28} → {line}");
    msg
}

#[test]
fn baseline_single_error_only() {
    // 三处**互相独立**的错误（TOML 本身合法）：
    // ① 指令引用未声明的 form；② 指令名重复；③ lowering 的 when 用了未知属性。
    let doc = BASE.replace("form = \"R\"", "form = \"NOPE\"")
        + r#"
[[instructions]]
name = "ADD"
form = "R"
opcode = 0x33
fields = { funct3 = 0 }
ops = ["dst:gpr:out", "src:gpr", "src2:gpr"]
asm = "add {dst}, {src}, {src2}"

[[lowering]]
op = "Isub"
insts = ["ADD {out}, {0}, {1}"]
when = { eq = ["rd_width", 32] }
"#;
    let msg = report("三处独立错误", &doc);
    // S0 现状：fail-fast ⇒ 只报第一条。S1 起此断言改为"≥3 条"。
    assert_eq!(
        msg.lines().filter(|l| !l.trim().is_empty()).count(),
        1,
        "S0 基线：validate 是 fail-fast（一次只报第一条）——S1 落地后请改写本断言\n{msg}"
    );
}

/// 报错文本（`Err` 的 Display；成功返回 `<ok>`）。
fn diag(doc: &str) -> String {
    match parse_and_validate(doc) {
        Ok(_) => "<ok>".to_string(),
        Err(e) => e.to_string(),
    }
}

#[test]
fn baseline_unknown_form_is_reported_with_position() {
    let doc = BASE.replace("form = \"R\"", "form = \"NOPE\"");
    let msg = report("未知 form", &doc);
    assert!(msg.contains("NOPE"), "应点名未知的 form：{msg}");
    // 现状：位置由 `diag::locate` 的 `source.find` 启发式给出（可能指错行）。
    assert!(msg.contains(":"), "应带位置前缀：{msg}");
}

#[test]
fn baseline_unknown_when_attribute_is_reported() {
    let doc = plus(
        r#"
[[lowering]]
op = "Isub"
insts = ["ADD {out}, {0}, {1}"]
when = { eq = ["rd_width", 32] }
"#,
    );
    let msg = report("未知 when 属性", &doc);
    assert!(msg.contains("rd_width"), "应点名未知属性：{msg}");
}

#[test]
fn baseline_edit_and_spill_references_are_unchecked() {
    // §2.5 的证据：`[emit]`/`[spill]` 引用不存在的指令、未知伪指令、未知占位符，
    // **当前都不报错**（只查非空）。
    let doc = plus(
        r#"
[emit.prologue]
insts = ["NO_SUCH_INST RSP, RBP", "@nope", "{bogus}"]

[spill.GPR]
load = "NO_SUCH_LOAD {0}, {1}"
store = "NO_SUCH_STORE {0}, {1}"
"#,
    );
    let msg = report("emit/spill 引用不存在", &doc);
    assert_eq!(
        msg, "<ok>",
        "S0 基线：`[emit]`/`[spill]` 的名字与占位符**未校验**\
         （S1 落地后此断言应改为报错并点名行号）\n{msg}"
    );
}

#[test]
fn baseline_anchor_degrades_to_first_line() {
    // 无声明名的节（`[stack]`）出错 ⇒ 现状退化到 1:1（丢掉跳转能力）。
    let doc = BASE.replace("[meta]", "[stack]\nalign = 0\n\n[meta]");
    let msg = report("无名节的错误定位", &doc);
    assert!(
        msg.starts_with("1:1:"),
        "S0 基线：抽不出声明名的错误退化为 1:1（S1 起应给精确 span）\n{msg}"
    );
}

#[test]
fn baseline_valid_doc_passes() {
    assert_eq!(report("合法骨架", BASE), "<ok>", "骨架必须合法");
}
