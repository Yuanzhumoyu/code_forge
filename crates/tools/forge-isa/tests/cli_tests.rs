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

// ───────────────── 多文件组合：include / [[override]] / fmt（v18 S7d）─────────────────

/// 建一个临时目录：`frag.toml`（片段）+ `root.toml`（include 它）。返回 (目录, 根路径)。
fn temp_multi_file(name: &str, frag: &str, root: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("forge_isa_cli_inc_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    std::fs::write(dir.join("frag.toml"), frag).expect("写片段");
    let root_path = dir.join("root.toml");
    std::fs::write(&root_path, root).expect("写根");
    (dir, root_path)
}

/// 多文件谱（夹具：片段 + 根 + `[[override]]`）能通过校验，且两条指令都在。
#[test]
fn validate_and_insts_accept_multi_file_spec() {
    let root = fixture("include_root_v12.toml");
    let v = run(&["validate", &root]);
    assert_eq!(v.code, 0, "stdout={}", v.stdout);
    assert!(v.stdout.contains("ISA = demo_include_v12"), "{}", v.stdout);

    let i = run(&["insts", &root]);
    assert_eq!(i.code, 0, "stderr={}", i.stderr);
    assert!(i.stdout.contains("2 条指令"), "{}", i.stdout);
    assert!(
        i.stdout.contains("IADD") && i.stdout.contains("ISUB"),
        "片段里的 IADD 与根里的 ISUB 都应在：{}",
        i.stdout
    );
    // 根文件的 `[[override]] key = "meta.version"` 生效（片段的旧值被替换）。
    assert!(i.stdout.contains("18.0-include"), "{}", i.stdout);

    let j = run(&["insts", &root, "--json"]);
    assert_eq!(j.code, 0, "stderr={}", j.stderr);
    assert!(
        j.stdout.contains("\"version\":\"18.0-include\""),
        "{}",
        j.stdout
    );
}

/// `fmt`：把多文件谱折叠成单文件——写出的文本**不含** `include`/`[[override]]`，
/// 仍能独立校验，且与 stdout 一致；再 fmt 一次结果不变（幂等）。
#[test]
fn fmt_folds_multi_file_into_single_spec() {
    let root = fixture("include_root_v12.toml");
    let out_file =
        std::env::temp_dir().join(format!("forge_isa_cli_fmt_{}.toml", std::process::id()));
    let w = run(&["fmt", &root, "--out", out_file.to_str().unwrap()]);
    assert_eq!(w.code, 0, "stderr={}", w.stderr);
    assert!(w.stdout.contains("来源 2 个文件"), "{}", w.stdout);

    let folded = std::fs::read_to_string(&out_file).expect("读折叠结果");
    assert!(
        !folded.lines().any(|l| l.starts_with("include =")),
        "折叠后不应含组合键 `include`：\n{folded}"
    );
    assert!(
        !folded
            .lines()
            .any(|l| l.trim_start().starts_with("[[override]]")),
        "折叠后不应含组合键 `[[override]]`：\n{folded}"
    );
    assert!(
        folded.contains("IADD") && folded.contains("ISUB"),
        "折叠后两条指令都在"
    );

    // stdout（不带 --out）= 同一份文本。
    let s = run(&["fmt", &root]);
    assert_eq!(s.code, 0, "stderr={}", s.stderr);
    assert_eq!(
        s.stdout.trim_end(),
        folded.trim_end(),
        "stdout 与 --out 应一致"
    );

    // 折叠结果是一份独立可用的谱。
    let v = run(&["validate", out_file.to_str().unwrap()]);
    assert_eq!(v.code, 0, "stdout={}", v.stdout);

    // 幂等：对折叠结果再 fmt，逐字不变。
    let again = run(&["fmt", out_file.to_str().unwrap()]);
    assert_eq!(again.code, 0, "stderr={}", again.stderr);
    assert_eq!(again.stdout.trim_end(), folded.trim_end(), "fmt 应幂等");
    let _ = std::fs::remove_file(&out_file);
}

/// 多文件诊断指向**真正写那一行的文件**（不是合并文本的行号）。
#[test]
fn diagnostics_point_at_the_included_file() {
    // 片段里故意写一个未声明的寄存器类：错误必须落在 `frag.toml` 上。
    let (dir, root) = temp_multi_file(
        "diag",
        "[reg.gpr4]\ncount = 4\n\n[[operand_slots]]\nname = \"r\"\nkind = \"reg\"\nclass = \"nope\"\n",
        "include = [\"frag.toml\"]\n\n[meta]\nname = \"multi\"\n\n[encoding]\nkind = \"fixed\"\nbits = 16\n\n[[instructions]]\nname = \"BAD\"\nopcode = 1\nops = [\"d:r:out\"]\nasm = \"bad {d}\"\n",
    );
    let out = run(&["validate", root.to_str().unwrap()]);
    assert_eq!(out.code, 1, "应报错：{}", out.stdout);
    assert!(
        out.stdout.contains("frag.toml:"),
        "诊断应指向片段文件：{}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("读不到文件"),
        "不应退化成「读不到文件」：{}",
        out.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 缺文件 / 覆盖目标不存在 —— 两种都必须是**点名具体文件/键**的明确错误。
#[test]
fn include_errors_are_explicit() {
    let (dir, root) = temp_multi_file(
        "missing",
        "[reg.gpr4]\ncount = 4\n",
        "include = [\"nope.toml\"]\n[meta]\nname = \"x\"\n[encoding]\nkind = \"fixed\"\nbits = 16\n",
    );
    let out = run(&["validate", root.to_str().unwrap()]);
    assert_eq!(out.code, 1);
    assert!(
        out.stdout.contains("nope.toml"),
        "错误应点名缺的 include 文件：{}",
        out.stdout
    );

    // `[[override]]` 指向不存在的键 → 明确报错（拼错不静默）。
    let (dir2, root2) = temp_multi_file(
        "override",
        "[reg.gpr4]\ncount = 4\n",
        "include = [\"frag.toml\"]\n[[override]]\nkey = \"meta.version\"\nvalue = \"9\"\n[meta]\nname = \"x\"\n[encoding]\nkind = \"fixed\"\nbits = 16\n",
    );
    let out2 = run(&["validate", root2.to_str().unwrap()]);
    assert_eq!(out2.code, 1);
    assert!(
        out2.stdout.contains("meta.version"),
        "错误应点名覆盖键：{}",
        out2.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

// ─────────────────────── test（v19 V3a）───────────────────────

/// `test` 的用法/环境错误：谱不存在 ⇒ 退出 2（**不进编译**，因此这个用例很快）。
#[test]
fn test_subcommand_usage_errors_exit_2() {
    let out = run(&["test", "no/such/spec.toml"]);
    assert_eq!(out.code, 2, "stdout={} stderr={}", out.stdout, out.stderr);
    assert!(out.stderr.contains("读不到谱"), "{}", out.stderr);

    let out = run(&["test"]);
    assert_eq!(out.code, 2, "缺参数应是用例错误");
    assert!(
        out.stderr.contains("test 需要恰好一个谱文件"),
        "{}",
        out.stderr
    );
}

/// `abi` 子命令（v20 A1）：约定/绑定数据 + 谱的能力视图，全部**不需要后端**。
///
/// 三份发行谱的实测结论（2026-09-24 本机）：x86 的 `win64`/`sysv64` 各 15 条代表签名
/// 全部可规划、riscv64 的 `lp64d` 同样全绿；arm64 的 `aapcs64` 有 **6 条缺口**
/// （谱里没有 FPR 寄存器组 → `float`/`ret_float` 池缺 → 浮点/HFA 无寄存器可落）。
/// 这些缺口是**如实上报**的 fail-closed 边界，不是失败——所以默认退出码是 0，
/// `--strict` 才让它们决定退出码。
#[test]
fn abi_check_reports_gaps_without_failing() {
    let out = run(&[
        "abi",
        "check",
        &isa("x86_v12.toml"),
        &isa("riscv64_v12.toml"),
        &isa("arm64_v12.toml"),
    ]);
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    assert!(out.stdout.contains("== x86_64_v12"), "{}", out.stdout);
    assert!(out.stdout.contains("✓ win64"), "{}", out.stdout);
    assert!(out.stdout.contains("✓ sysv64"), "{}", out.stdout);
    assert!(out.stdout.contains("✓ lp64d"), "{}", out.stdout);
    assert!(out.stdout.contains("⚠ aapcs64"), "{}", out.stdout);
    assert!(out.stdout.contains("硬错 0"), "{}", out.stdout);
    // 缺口必须点名根因（v20 A5 起 arm64 只剩 HFA4 返回那条"≥3 槽见 A6"的限制），
    // 而不是含糊地说"失败"。
    assert!(
        out.stdout.contains("hfa4") && out.stdout.contains("连续寄存器槽"),
        "应给出 arm64 的根因提示：{}",
        out.stdout
    );
    // 固定用途/链接寄存器也读得对（谱里的事实）。
    assert!(out.stdout.contains("链接寄存器 X30"), "{}", out.stdout);
}

#[test]
fn abi_check_strict_fails_on_gaps() {
    let out = run(&["abi", "check", &isa("arm64_v12.toml"), "--strict"]);
    assert_eq!(out.code, 1, "stdout={} stderr={}", out.stdout, out.stderr);
    let out = run(&["abi", "check", &isa("x86_v12.toml"), "--strict"]);
    assert_eq!(out.code, 0, "stdout={}", out.stdout);
}

#[test]
fn abi_list_shows_builtin_data() {
    let out = run(&["abi", "list"]);
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    for want in [
        "win64",
        "sysv64",
        "aapcs64",
        "lp64d",
        "x86_64_v12",
        "riscv64_v12",
    ] {
        assert!(out.stdout.contains(want), "缺 {want}：{}", out.stdout);
    }
    // win64 的关键事实：32 字节 shadow、按位置计数、变参走栈。
    assert!(out.stdout.contains("shadow=32"), "{}", out.stdout);
    assert!(out.stdout.contains("位置=ByPosition"), "{}", out.stdout);

    let json = run(&["abi", "list", "--json"]);
    assert_eq!(json.code, 0);
    assert!(
        json.stdout.contains("\"position\": \"ByPosition\""),
        "{}",
        json.stdout
    );
    assert!(
        json.stdout.contains("\"conv\": \"win64\""),
        "{}",
        json.stdout
    );
}

#[test]
fn abi_plan_prints_a_deterministic_plan() {
    let out = run(&[
        "abi",
        "plan",
        &isa("x86_v12.toml"),
        "--conv",
        "win64",
        "--sig",
        "i64, f64 -> i64",
    ]);
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    assert!(out.stdout.contains("conv win64"), "{}", out.stdout);
    // win64 是**按位置**计数：第 2 个参数（浮点）应落在 XMM1。
    assert!(
        out.stdout.contains("arg a0 size=8 -> reg RCX"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("arg a1 size=8 -> reg XMM1"),
        "{}",
        out.stdout
    );
    // 标量返回在 RAX（返回池），不是参数池的 RCX。
    assert!(out.stdout.contains("ret reg RAX"), "{}", out.stdout);
    // 同一输入两次必须逐字节一致。
    let again = run(&[
        "abi",
        "plan",
        &isa("x86_v12.toml"),
        "--conv",
        "win64",
        "--sig",
        "i64, f64 -> i64",
    ]);
    assert_eq!(out.stdout, again.stdout);
}

#[test]
fn abi_plan_fails_closed_on_gaps() {
    // arm64 的 HFA4 **返回**（≥3 槽）仍是缺口：规划必须明确报缺，而不是静默换寄存器。
    //（v20 A5 起 arm64 的 f64 参数已有 v0-v7，不再是缺口——这正是本测试改用 HFA4 返回的原因。）
    let out = run(&[
        "abi",
        "plan",
        &isa("arm64_v12.toml"),
        "--conv",
        "aapcs64",
        "--sig",
        "hfa4 -> hfa4",
    ]);
    assert_eq!(out.code, 1, "stdout={}", out.stdout);
    assert!(
        out.stdout.contains("寄存器槽") || out.stdout.contains("hfa4"),
        "{}",
        out.stdout
    );

    // 用法错误：缺 --conv / 看不懂的类型。
    let out = run(&["abi", "plan", &isa("x86_v12.toml"), "--sig", "i64"]);
    assert_eq!(out.code, 2, "{}", out.stderr);
    let out = run(&[
        "abi",
        "plan",
        &isa("x86_v12.toml"),
        "--conv",
        "win64",
        "--sig",
        "banana",
    ]);
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("看不懂的类型"), "{}", out.stderr);
}

/// `test` 的**端到端**：现搭零宿主 crate → 生成物 → `cargo test` 跑谱里的向量。
///
/// **默认跳过**：它会嵌套起一次 cargo（首次要编译 forge-isa-runtime/forge-isa-dsl 到独立
/// target 目录，几十秒），不适合塞进每次 `cargo test`。打开方式（与仓库其它重用例同风格）：
///
/// ```text
/// FORGE_ISA_TEST_E2E=1 cargo test -p forge-isa --test cli_tests -- --nocapture
/// ```
///
/// 本机 2026-09-23 实测：`test isa/riscv64_v12.toml --json` ⇒ `{"vectors":67,…,"passed":295,
/// "failed":0,"ok":true}`（生成的用例数随部件变化——这里只有 encode/decode/asm）。
#[test]
fn test_subcommand_runs_spec_vectors_e2e() {
    if std::env::var("FORGE_ISA_TEST_E2E").as_deref() != Ok("1") {
        eprintln!("SKIP test_subcommand_runs_spec_vectors_e2e（设 FORGE_ISA_TEST_E2E=1 才跑）");
        return;
    }
    let out = run(&["test", &isa("riscv64_v12.toml"), "--json"]);
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    assert!(out.stdout.contains("\"ok\":true"), "{}", out.stdout);
    assert!(out.stdout.contains("\"vectors\":67"), "{}", out.stdout);
    assert!(out.stdout.contains("\"failed\":0"), "{}", out.stdout);
    assert!(
        out.stdout.contains("isa-test"),
        "JSON 里应给出临时 crate 路径：{}",
        out.stdout
    );
}
