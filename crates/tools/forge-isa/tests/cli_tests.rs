//! `forge-isa` CLI 集成测试（v18 S7b）：直接跑二进制，断言退出码与输出。
//!
//! 覆盖：validate（好/坏谱）、insts（文本 + JSON）、explain（模板来源 + 未知指令）、
//! diff（相同/不同）、用法错误退出码、`--help`/`--version`。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 仓库根：`crates/tools/forge-isa` 上溯三级。
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn isa(name: &str) -> String {
    root().join("isa").join(name).to_string_lossy().to_string()
}

fn fixture(name: &str) -> String {
    root()
        .join("crates/backend/forge-codegen/tests/isa")
        .join(name)
        .to_string_lossy()
        .to_string()
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str]) -> Out {
    let out = Command::new(env!("CARGO_BIN_EXE_forge-isa"))
        .args(args)
        .output()
        .expect("跑 forge-isa");
    Out {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// 写一个临时谱文件（测试结束删除）。
fn temp_spec(name: &str, body: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("forge_isa_cli_{}_{name}", std::process::id()));
    std::fs::write(&p, body).expect("写临时谱");
    p
}

#[test]
fn validate_shipped_isas_ok() {
    let out = run(&[
        "validate",
        &isa("x86_v12.toml"),
        &isa("riscv64_v12.toml"),
        &isa("arm64_v12.toml"),
    ]);
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    assert!(out.stdout.contains("OK"), "{}", out.stdout);
    assert!(out.stdout.contains("ISA = x86_64_v12"), "{}", out.stdout);
}

#[test]
fn validate_reports_diagnostics_with_position() {
    // 未声明的 `form` → DSL-INST 诊断，带 行:列。
    let bad = temp_spec(
        "bad.toml",
        r#"
[meta]
name = "bad"
[encoding]
kind = "fixed"
bits = 32
[reg.gpr4]
count = 8
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr4"
[[instructions]]
name = "I"
form = "NOPE"
opcode = 1
asm = "i"
"#,
    );
    let out = run(&["validate", bad.to_str().unwrap()]);
    let _ = std::fs::remove_file(&bad);
    assert_eq!(out.code, 1, "应报错退出 1：{}", out.stdout);
    assert!(
        out.stdout.contains("DSL-INST") && out.stdout.contains("NOPE"),
        "{}",
        out.stdout
    );
    // `路径:行:列: 码: 消息` 形态（可点击）。
    let first = out.stdout.lines().next().unwrap_or("");
    assert!(
        first.contains(":1:") || first.contains(":2:"),
        "应带行号：{first}"
    );
}

#[test]
fn insts_lists_expanded_instructions() {
    let out = run(&["insts", &isa("riscv64_v12.toml")]);
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    assert!(out.stdout.contains("116 条指令"), "{}", out.stdout);
    // 模板展开出的实例也在列表里（SLLW 来自 [[templates]] 行）。
    assert!(out.stdout.contains("SLLW"), "{}", out.stdout);
    assert!(out.stdout.contains("template="), "{}", out.stdout);
}

#[test]
fn insts_json_is_machine_readable() {
    let out = run(&["insts", &fixture("demo_mixed16_32_v12.toml"), "--json"]);
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    let t = out.stdout.trim();
    assert!(t.starts_with('{') && t.ends_with('}'), "{t}");
    assert!(t.contains("\"width_bits\":16"), "{t}");
    assert!(t.contains("\"width_bits\":32"), "{t}");
    assert!(t.contains("\"name\":\"LADD32\""), "{t}");
}

#[test]
fn explain_shows_template_provenance() {
    let out = run(&["explain", &isa("arm64_v12.toml"), "ADDREGW"]);
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    assert!(
        out.stdout.contains("[[templates.ADDREG]] 第 2 行"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("生效规格"), "{}", out.stdout);
    assert!(out.stdout.contains("opcode = 0xb"), "{}", out.stdout);
}

#[test]
fn explain_unknown_instruction_fails() {
    let out = run(&["explain", &isa("arm64_v12.toml"), "NO_SUCH_INST"]);
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("没有名为"), "{}", out.stdout);
}

#[test]
fn diff_same_file_is_identical() {
    let out = run(&["diff", &isa("arm64_v12.toml"), &isa("arm64_v12.toml")]);
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    assert!(out.stdout.contains("规格完全相同"), "{}", out.stdout);
}

#[test]
fn diff_detects_added_instructions() {
    let out = run(&["diff", &isa("x86_v12.toml"), &fixture("demo_v12.toml")]);
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    assert!(out.stdout.contains("  + "), "{}", out.stdout);
    assert!(out.stdout.contains("共 "), "{}", out.stdout);
}

#[test]
fn usage_errors_exit_2() {
    let no_args = run(&[]);
    assert_eq!(no_args.code, 2, "缺子命令应退出 2");
    assert!(no_args.stderr.contains("用法"), "{}", no_args.stderr);
    let unknown = run(&["nope"]);
    assert_eq!(unknown.code, 2);
    assert!(unknown.stderr.contains("未知子命令"), "{}", unknown.stderr);
    let too_few = run(&["diff", &isa("x86_v12.toml")]);
    assert_eq!(too_few.code, 2);
}

#[test]
fn help_and_version() {
    let help = run(&["--help"]);
    assert_eq!(help.code, 0);
    assert!(help.stdout.contains("validate"), "{}", help.stdout);
    assert!(help.stdout.contains("schema"), "{}", help.stdout);
    let ver = run(&["--version"]);
    assert_eq!(ver.code, 0);
    assert!(ver.stdout.contains("forge-isa "), "{}", ver.stdout);
}

#[test]
fn schema_prints_json_and_writes_file() {
    let out = run(&["schema"]);
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    let t = out.stdout.trim();
    assert!(t.starts_with('{') && t.ends_with('}'), "{t}");
    assert!(t.contains("\"$schema\""), "{t}");
    assert!(t.contains("\"encoding\""), "{t}");
    assert!(t.contains("\"kind\""), "{t}");

    // `--out` 写出的内容与 stdout 相同（仓库根那份 `isa-dsl.schema.json` 就是这样生成的）。
    let file =
        std::env::temp_dir().join(format!("forge_isa_cli_schema_{}.json", std::process::id()));
    let w = run(&["schema", "--out", file.to_str().unwrap()]);
    assert_eq!(w.code, 0, "stderr={}", w.stderr);
    let written = std::fs::read_to_string(&file).expect("读到写出的 schema");
    let _ = std::fs::remove_file(&file);
    assert_eq!(written.trim_end(), t, "--out 内容应与 stdout 一致");
}

#[test]
fn validate_accepts_schema_comment() {
    // `#:schema` 只是 TOML 注释：不得影响解析（编辑器补全用）。
    let spec = temp_spec(
        "with_schema_comment.toml",
        "#:schema ../../../isa-dsl.schema.json\n[meta]\nname = \"cmt\"\n[encoding]\nkind = \"fixed\"\nbits = 16\n[reg.gpr4]\ncount = 8\n[[operand_slots]]\nname = \"g\"\nkind = \"reg\"\nclass = \"gpr4\"\n[[instructions]]\nname = \"N\"\nform = \"R\"\nopcode = 1\nops = [\"d:g:out\"]\nasm = \"n {d}\"\n[[forms]]\nname = \"R\"\nopcode_field = \"op\"\noperand_fields = [\"rd\"]\n[conventions.bitfields]\nop = { offset = 0, width = 8 }\nrd = { offset = 8, width = 3 }\n",
    );
    let out = run(&["validate", spec.to_str().unwrap()]);
    let _ = std::fs::remove_file(&spec);
    assert_eq!(out.code, 0, "stdout={}", out.stdout);
}
