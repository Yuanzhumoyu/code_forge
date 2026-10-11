//! **v22 内联操作数声明**：`{名字:槽[:角色]}` 声明、`{@字段:槽[:角色]}` 绑定字段（2026-10）。
//!
//! 口径（与 `docs/reference/isa-dsl.md` 的「操作数声明」一节同源）：
//!
//! - 声明写进 `asm` 模板本身，**出现序 = 编码序**（取代旧 `ops` 键的数组序）；
//! - `{@字段:槽}` **作用于紧随其后的那个操作数声明**，自身不打印；槽/角色必须一致；
//! - 抽取结果必须与旧写法**逐字段等价**（本文件第一条用例把它钉成"生成物面等价"）。
//!
//! 迁移期两种写法并存；同一条指令同时用 `ops` 与内联声明 ⇒ 报错。

use forge_isa_dsl::report;

/// 旧写法基线（`ops` 键 + 纯引用模板）。
fn spec_legacy() -> String {
    r##"[meta]
name = "inline"
endian = "little"

[encoding]
kind = "fixed"
bits = 16

[reg.gpr8]
names = ["R0", "R1", "R2", "R3"]

[operand.g]
kind = "reg"
class = "gpr8"

[operand.imm5]
kind = "imm"
bits = 5
signed = true

[form.RR]
fields = [
  "u4[15:12]:opcode",
  "u3[11:8]:rd",
  "u3[7:5]:rs1",
]

[form.RI]
fields = [
  "u4[15:12]:opcode",
  "u3[11:8]:rd",
  "u5[4:0]:imm",
]

[inst.ADD]
form = "RR"
opcode = 1
ops = ["dst:g:out", "src:g"]
asm = "add {dst}, {src}"
effect = ["Pure"]

[inst.MOVI]
form = "RI"
opcode = 3
ops = ["dst:g:out", "imm:imm5"]
asm = "movi {dst}, {imm}"
effect = ["Pure"]

[inst.BRZ]
form = "RI"
opcode = 4
ops = ["src:g", "target:imm5"]
asm = "brz {src}, {target}"
effect = ["Branch"]

[inst.NOP]
opcode = 0
asm = "nop"
effect = ["Pure"]
"##
    .to_string()
}

/// 等价的内联写法：声明 + 显式字段绑定（`ADD` 的 `src` 在 RR 里就是 `rs1`；
/// `BRZ` 的 `src` 在 RI 里落在 `rd`——旧写法靠位置回退，这里写出来）。
fn spec_inline() -> String {
    r##"[meta]
name = "inline"
endian = "little"

[encoding]
kind = "fixed"
bits = 16

[reg.gpr8]
names = ["R0", "R1", "R2", "R3"]

[operand.g]
kind = "reg"
class = "gpr8"

[operand.imm5]
kind = "imm"
bits = 5
signed = true

[form.RR]
fields = [
  "u4[15:12]:opcode",
  "u3[11:8]:rd",
  "u3[7:5]:rs1",
]

[form.RI]
fields = [
  "u4[15:12]:opcode",
  "u3[11:8]:rd",
  "u5[4:0]:imm",
]

[inst.ADD]
form = "RR"
opcode = 1
asm = "add {@rd:g:out}{dst:g:out}, {@rs1:g}{src:g}"
effect = ["Pure"]

[inst.MOVI]
form = "RI"
opcode = 3
asm = "movi {@rd:g:out}{dst:g:out}, {@imm:imm5}{imm:imm5}"
effect = ["Pure"]

[inst.BRZ]
form = "RI"
opcode = 4
asm = "brz {@rd:g}{src:g}, {@imm:imm5}{target:imm5}"
effect = ["Branch"]

[inst.NOP]
opcode = 0
asm = "nop"
effect = ["Pure"]
"##
    .to_string()
}

/// 取谱的报错文本（`report::insts` 的错误是诊断行，用 `Debug` 渲染）。
fn errs(spec: &str) -> String {
    match report::insts(spec) {
        Ok(_) => panic!("这份谱本应报错"),
        Err(diags) => format!("{diags:?}"),
    }
}

/// 内联写法与旧写法在 `insts` 报告面**逐字段相同**（操作数表、渲染文本、生效编码键）。
#[test]
fn inline_form_matches_legacy_report() {
    let (_l, legacy) = report::insts(&spec_legacy()).expect("旧写法谱");
    let (_i, inline) = report::insts(&spec_inline()).expect("内联写法谱");
    assert_eq!(legacy.len(), inline.len(), "指令条数应一致");
    for (a, b) in legacy.iter().zip(inline.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.ops, b.ops, "[{0}] 操作数表（名字:槽:角色）应一致", a.name);
        assert_eq!(
            a.asm, b.asm,
            "[{0}] 渲染文本应一致（绑定占位符不打印）",
            a.name
        );
        assert_eq!(a.form, b.form);
        assert_eq!(a.opcode, b.opcode, "[{0}] opcode", a.name);
    }
}

/// 绑定占位符的槽必须与它绑定的操作数一致。
#[test]
fn binding_slot_mismatch_fails_closed() {
    let bad = spec_inline().replace("{@rs1:g}{src:g}", "{@rs1:imm5}{src:g}");
    let msg = errs(&bad);
    assert!(
        msg.contains("rs1") && msg.contains("槽必须一致"),
        "错误应点名字段与规则：{msg}"
    );
}

/// 绑定占位符后面必须紧跟操作数声明。
#[test]
fn binding_without_following_operand_fails_closed() {
    let bad = spec_inline().replace("{@rs1:g}{src:g}", "{src:g}{@rs1:g}");
    let msg = errs(&bad);
    assert!(
        msg.contains("后面没有操作数声明"),
        "错误应说明绑定必须紧挨操作数：{msg}"
    );
}

/// 不存在的字段名（form 里没有）也要报错——绑定是显式的，不许静默回退。
#[test]
fn binding_to_undeclared_field_fails_closed() {
    let bad = spec_inline().replace("{@rs1:g}{src:g}", "{@nope:g}{src:g}");
    let msg = errs(&bad);
    assert!(msg.contains("nope"), "错误应点名该字段：{msg}");
}

/// `ops` 键与内联声明不许同现。
#[test]
fn mixing_ops_key_and_inline_decl_fails_closed() {
    let bad = spec_inline().replace(
        r#"asm = "add {@rd:g:out}{dst:g:out}, {@rs1:g}{src:g}""#,
        "ops = [\"dst:g:out\", \"src:g\"]\nasm = \"add {@rd:g:out}{dst:g:out}, {@rs1:g}{src:g}\"",
    );
    let msg = errs(&bad);
    assert!(msg.contains("只写一处"), "错误应说明两者选一处：{msg}");
}

/// 多模板（`asm = [...]` 的别名写法）必须声明同一组操作数。
#[test]
fn templates_must_declare_the_same_operands_fails_closed() {
    let bad = spec_inline().replace(
        r#"asm = "add {@rd:g:out}{dst:g:out}, {@rs1:g}{src:g}""#,
        "asm = [\"add {@rd:g:out}{dst:g:out}, {@rs1:g}{src:g}\", \"add {@rs1:g}{src:g}, {@rd:g:out}{dst:g:out}\"]",
    );
    let msg = errs(&bad);
    assert!(
        msg.contains("相对顺序不一致") || msg.contains("声明不一致"),
        "错误应说明各模板的声明序必须一致（顺序 = 编码序）：{msg}"
    );
}

/// 发行谱（仍在旧写法上，迁移期）不得因本特性回归。
#[test]
fn shipped_specs_still_parse() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for isa in ["x86.toml", "riscv64.toml", "arm64.toml"] {
        let src = std::fs::read_to_string(root.join("isa").join(isa)).expect("读谱");
        report::insts(&src).unwrap_or_else(|e| panic!("{isa} 应仍能解析：{e:?}"));
    }
}
