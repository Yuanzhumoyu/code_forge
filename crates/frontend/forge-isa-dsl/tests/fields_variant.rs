//! `fields_variant` 的声明一致性（出路 2 第③步）——**三条负例必须被硬拒**。
//!
//! 为什么用 `forge_isa_dsl::validate_file` 而不是 CLI：`forge-isa validate` 走的是
//! `report::validate_file`（**诊断**通路，只做语法/结构层），而变体门的一致性校验跑在
//! `lib::validate_file` 的**语义**通路上（宿主构建与 `isa_from_file!` 同款）✓ ——
//! 实测过：负例在 CLI 下不报、在同一条语义通路上才报 ✓。
//!
//! 三条负例：① 参数名未声明；② `by` 的键不在声明域；③ 缺 `default`（由**类型**保证：
//! `VariantFieldValue.default` 不是 `Option` ⇒ 反序列化即报 `missing field`）✓。
use std::io::Write;

/// 最小可用谱：一个 32 位定宽 ISA + 一个变体参数 `xlen` + 一条带 `fields_variant` 的指令。
///
/// 注意原始字符串用 `r##"…"##` ✓：谱里有 `comment_char = "#"`，用 `r#"…"#` 会在那里**提前结束** ✗。
fn spec_with(instruction: &str) -> String {
    format!(
        r##"[meta]
name = "fvar"
comment_char = "#"

[meta.variants]
xlen = [32, 64]

[encoding]
kind = "fixed"
bits = 32

[reg.gpr]
base_index = 0
names = ["T0", "T1"]

[[operand_slots]]
name = "r"
kind = "reg"
class = "gpr"

[[forms]]
name = "F"
fields = [
  "u8[31:24]:opcode",
  "u5[11:7]:rd",
  "u5[12:8]:imm",
]

[[instructions]]
name = "FV"
form = "F"
opcode = 0x10
{instruction}
ops = ["dst:r:out"]
asm = "fv {{dst}}"
"##
    )
}

/// 写临时谱并跑语义校验；返回错误文本（成功则空串）。
fn run(tag: &str, instruction: &str) -> String {
    let dir = std::env::temp_dir().join(format!("forge_fvar_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tmpdir");
    let p = dir.join("spec.toml");
    let mut f = std::fs::File::create(&p).expect("create");
    f.write_all(spec_with(instruction).as_bytes()).expect("write");
    drop(f);
    match forge_isa_dsl::validate_file(p.to_str().expect("utf8 path")) {
        Ok(()) => String::new(),
        Err(e) => e.join("\n"),
    }
}

#[test]
fn fields_variant_accepts_a_well_formed_dispatch() {
    let e = run(
        "ok",
        r#"fields_variant = { imm = { param = "xlen", by = { 32 = 0x698 }, default = 0x6b8 } }"#,
    );
    assert!(e.is_empty(), "合法分派不应报错，实际：{e}");
}

#[test]
fn fields_variant_rejects_an_undeclared_param() {
    let e = run(
        "badparam",
        r#"fields_variant = { imm = { param = "nosuch", by = { 32 = 1 }, default = 0 } }"#,
    );
    assert!(
        e.contains("未声明") && e.contains("nosuch"),
        "未声明的参数必须硬报，实际：{e}"
    );
}

#[test]
fn fields_variant_rejects_a_key_outside_the_domain() {
    let e = run(
        "baddomain",
        r#"fields_variant = { imm = { param = "xlen", by = { 99 = 1 }, default = 0 } }"#,
    );
    assert!(
        e.contains("不在") && e.contains("99"),
        "域外的取值必须硬报，实际：{e}"
    );
}

#[test]
fn fields_variant_requires_default() {
    let e = run(
        "nodefault",
        r#"fields_variant = { imm = { param = "xlen", by = { 32 = 1 } } }"#,
    );
    assert!(
        e.contains("default"),
        "缺 `default` 必须报错（类型保证，提示里应出现字段名），实际：{e}"
    );
}
