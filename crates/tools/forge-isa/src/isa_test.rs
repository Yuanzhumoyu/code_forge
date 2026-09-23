//! `forge-isa test <谱> [--json]`：**真的把谱里的向量跑起来**（ISA-DSL v19 V3a）。
//!
//! 为什么这能成立（V1/V2 的直接受益）：生成物是"纯 Rust 文件 + 单一运行时依赖"，
//! 所以这里能给任意谱现搭一个**零宿主** crate（依赖只有 `forge-isa-runtime`，build 依赖
//! 只有 `forge-isa-dsl`），build script 用 `pregenerate` 生成、`src/lib.rs` 只写一句
//! `include!`。**不需要 forge-codegen，也不需要宿主写任何代码**：`cargo test` 直接把
//! 生成物里的 `__spec_tests`（谱内 `[[vectors]]` + 每条指令的闭环用例）跑一遍。
//!
//! 三条实现约束（都是踩过的坑）：
//!
//! - **独立 `CARGO_TARGET_DIR`**（`target/isa-test-target`）：与正在跑的 `cargo test`
//!   分开，否则嵌套调用会等同一把 target 目录锁（自己把自己锁死）；
//! - cargo 的 stdout/stderr **各落一个文件再回读**，不接管道（宿主沙箱下管道可能不可用，
//!   且大输出不会卡住子进程）；
//! - 谱路径走**环境变量** `FORGE_ISA_SPEC` 传给生成的 build script（不插值进源码：
//!   Windows 反斜杠/引号会变成转义坑）。
//!
//! 退出码：`0` 全过、`1` 有向量失败或谱有诊断、`2` 用法/环境错误（与其它子命令一致）。

use std::fs::File;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use forge_isa_dsl::report;

/// 临时 crate 的三件套模板（独立文件，免得在 `format!` 里跟花括号打架）。
const CARGO_TOML: &str = include_str!("isa_test/Cargo.toml.tmpl");
const BUILD_RS: &str = include_str!("isa_test/build.rs.tmpl");
const LIB_RS: &str = include_str!("isa_test/lib.rs.tmpl");

/// 跑一份谱的自测：谱内向量 + 每条指令的闭环用例。
pub fn run(spec: &Path, json: bool) -> ExitCode {
    let spec_abs = match std::fs::canonicalize(spec) {
        Ok(p) => strip_unc(p),
        Err(e) => {
            eprintln!("读不到谱 {}：{e}", spec.display());
            return ExitCode::from(2);
        }
    };
    // 先校验（含 include 合并）：向量写坏了在这里就报诊断，不进编译（省一轮 cargo）。
    let loaded = match report::load_spec(&spec_abs) {
        Ok(l) => l,
        Err(diags) => {
            // 与 `validate` 用同一套渲染（父模块的 `print_diags`）。
            let _ = crate::print_diags(&spec_abs, &diags);
            return ExitCode::from(1);
        }
    };
    let n_vectors = loaded
        .text
        .lines()
        .filter(|l| l.trim() == "[[vectors]]")
        .count();

    let repo = match repo_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let work = repo
        .join("target")
        .join("isa-test")
        .join(dir_name(&spec_abs));
    if let Err(e) = write_crate(&work, &repo) {
        eprintln!("{e}");
        return ExitCode::from(2);
    }

    let target_dir = repo.join("target").join("isa-test-target");
    let out_log = work.join("cargo-test.out");
    let err_log = work.join("cargo-test.err");
    let (Ok(out_file), Ok(err_file)) = (File::create(&out_log), File::create(&err_log)) else {
        eprintln!("无法在 {} 写日志文件", work.display());
        return ExitCode::from(2);
    };
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    // `--offline`：临时 crate 的依赖版本已在本地注册表缓存里（workspace 构建过），
    // 离线解析因此可用，也不会因为 CI/沙箱没网而在"解析依赖"这一步失败。
    let status = Command::new(cargo)
        .arg("test")
        .arg("--quiet")
        .arg("--offline")
        .current_dir(&work)
        .env("FORGE_ISA_SPEC", &spec_abs)
        .env("CARGO_TARGET_DIR", &target_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .status();
    let status = match status {
        Ok(s) => s,
        Err(e) => {
            eprintln!("跑 cargo 失败：{e}");
            return ExitCode::from(2);
        }
    };

    let text = format!("{}{}", read(&out_log), read(&err_log));
    let sum = parse_summary(&text);
    let ok = status.success() && sum.failed == 0;
    print_report(&spec_abs, &work, n_vectors, &sum, ok, json);
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// 去掉 Windows `canonicalize` 加的 `\\?\` 前缀。
///
/// **必须去**：那个前缀在 TOML 路径依赖里是非法 URL（`invalid path url //?/D:\…`），
/// 而且它出现在给用户的输出里也很难看。真正的 UNC 路径（`\\server\share`）没有这个前缀，
/// 不受影响。
fn strip_unc(p: PathBuf) -> PathBuf {
    match p.to_string_lossy().strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}

/// 仓库根：`CARGO_MANIFEST_DIR`（= `<repo>/crates/tools/forge-isa`）上溯三级。
///
/// 临时 crate 要按**路径依赖**引用运行时与生成器，所以这个命令必须在本仓库 checkout 里跑
/// （装了二进制但在别处运行时给出明确原因，而不是让 cargo 报一堆找不到路径的错）。
fn repo_root() -> Result<PathBuf, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let root = root
        .canonicalize()
        .map(strip_unc)
        .map_err(|e| format!("定位仓库根失败（{}）：{e}", root.display()))?;
    let rt = root.join("crates/foundation/forge-isa-runtime");
    if !rt.is_dir() {
        return Err(format!(
            "找不到 {}——`forge-isa test` 要为谱现搭一个依赖 forge-isa-runtime 的临时 crate，请在本仓库 checkout 里运行",
            rt.display()
        ));
    }
    Ok(root)
}

/// 每个谱一个**稳定**目录名（同一份谱重复跑复用同一个 crate 与 target 缓存）。
fn dir_name(spec: &Path) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    spec.hash(&mut h);
    let stem = spec
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "spec".into());
    format!("{stem}_{:016x}", h.finish())
}

/// 写出临时 crate（幂等：每次覆盖三个小文件）。
fn write_crate(work: &Path, repo: &Path) -> Result<(), String> {
    let runtime = repo.join("crates/foundation/forge-isa-runtime");
    let dsl = repo.join("crates/frontend/forge-isa-dsl");
    let manifest = CARGO_TOML
        .replace("{{RUNTIME}}", &slash(&runtime))
        .replace("{{DSL}}", &slash(&dsl));
    for (path, body) in [
        (work.join("Cargo.toml"), manifest),
        (work.join("build.rs"), BUILD_RS.to_string()),
        (work.join("src/lib.rs"), LIB_RS.to_string()),
    ] {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("建目录 {} 失败：{e}", dir.display()))?;
        }
        std::fs::write(&path, body).map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
    }
    Ok(())
}

/// 路径统一成 `/`（TOML 里 Windows 反斜杠要转义；正斜杠两边都能解析）。
fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// `cargo test` 的汇总。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    pub passed: usize,
    pub failed: usize,
    /// 失败项：`test … FAILED` 行 + 向量定位消息（`[向量 N] …`）。
    pub failures: Vec<String>,
}

/// 解析 `cargo test` 的文本输出（libtest 的 `test result:` 行是唯一计数来源）。
pub(crate) fn parse_summary(text: &str) -> Summary {
    let mut sum = Summary::default();
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("test result:") {
            for part in rest.split(';') {
                let p = part.trim();
                for (suffix, slot) in [(" passed", 0usize), (" failed", 1)] {
                    if let Some(head) = p.strip_suffix(suffix)
                        && let Some(n) = head.split_whitespace().last()
                    {
                        let v = n.parse::<usize>().unwrap_or(0);
                        if slot == 0 {
                            sum.passed += v;
                        } else {
                            sum.failed += v;
                        }
                    }
                }
            }
        } else if t.starts_with("test ") && t.ends_with("FAILED") {
            sum.failures.push(t.to_string());
        } else if let Some(i) = t.find("[向量 ") {
            let msg = t[i..].trim().to_string();
            if !sum.failures.iter().any(|f| f.contains(&msg)) {
                sum.failures.push(msg);
            }
        }
    }
    sum
}

/// 打印结果（人读 / `--json`）。
fn print_report(spec: &Path, work: &Path, n_vectors: usize, sum: &Summary, ok: bool, json: bool) {
    if json {
        let fails: Vec<String> = sum.failures.iter().map(|f| json_str(f)).collect::<Vec<_>>();
        println!(
            "{{\"spec\":{},\"vectors\":{},\"passed\":{},\"failed\":{},\"ok\":{},\"failures\":[{}],\"workdir\":{}}}",
            json_str(&spec.display().to_string()),
            n_vectors,
            sum.passed,
            sum.failed,
            ok,
            fails.join(","),
            json_str(&work.display().to_string())
        );
        return;
    }
    println!("谱 {}：谱内向量 {} 条", spec.display(), n_vectors);
    println!(
        "生成期自测（谱内向量 + 每条指令的闭环用例）：通过 {} / 失败 {}",
        sum.passed, sum.failed
    );
    for f in &sum.failures {
        println!("  ✗ {f}");
    }
    println!(
        "临时宿主：{}（可删；`--json` 里也有这个路径）",
        work.display()
    );
}

/// 最小 JSON 字符串转义（本仓库不引 serde_json，见 `docs/archive/forge-dsl-v18-plan.md` §10.4）。
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 计数必须从 libtest 行里取到（`ok. 970 passed` 的 `ok.` 前缀不能把数字吃掉）。
    #[test]
    fn parse_summary_counts_and_collects_failures() {
        let text = "\
running 970 tests
test arch::x::__spec_tests::spec_add ... ok
test arch::x::__spec_tests::spec_vector_62 ... FAILED
thread 'spec_vector_62' panicked at src/lib.rs:1:1:
[向量 62] \"addi X1, X2, 4096\" 的错误消息不含 \"out of range\": no matching instruction

test result: FAILED. 969 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
";
        let s = parse_summary(text);
        assert_eq!(s.passed, 969);
        assert_eq!(s.failed, 1);
        assert!(
            s.failures
                .iter()
                .any(|f| f.contains("spec_vector_62") && f.contains("FAILED")),
            "{:?}",
            s.failures
        );
        assert!(
            s.failures.iter().any(|f| f.starts_with("[向量 62]")),
            "向量定位消息要留下：{:?}",
            s.failures
        );
    }

    /// 全过时不该有失败项；空输出退化为 0/0（调用方据 cargo 退出码判定）。
    #[test]
    fn parse_summary_ok_run() {
        let s = parse_summary("test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured");
        assert_eq!(
            s,
            Summary {
                passed: 12,
                failed: 0,
                failures: vec![]
            }
        );
        assert_eq!(parse_summary(""), Summary::default());
    }

    /// 临时 crate 三件套：Cargo.toml 指到运行时/生成器，lib.rs 只有一句 include。
    #[test]
    fn scaffold_points_at_runtime_and_dsl() {
        let tmp =
            std::env::temp_dir().join(format!("forge_isa_test_scaffold_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let repo = Path::new("D:/repo");
        write_crate(&tmp, repo).expect("写临时 crate");
        let manifest = read(&tmp.join("Cargo.toml"));
        assert!(
            manifest.contains(
                "forge-isa-runtime = { path = \"D:/repo/crates/foundation/forge-isa-runtime\" }"
            ),
            "{manifest}"
        );
        assert!(
            manifest
                .contains("forge-isa-dsl = { path = \"D:/repo/crates/frontend/forge-isa-dsl\" }"),
            "{manifest}"
        );
        assert!(
            manifest.contains("[workspace]"),
            "临时 crate 必须自成 workspace：{manifest}"
        );
        let lib = read(&tmp.join("src/lib.rs"));
        assert!(lib.contains("include!(concat!(env!(\"OUT_DIR\")"), "{lib}");
        assert!(read(&tmp.join("build.rs")).contains("forge_isa_dsl::gen_file::pregenerate"));
        // 目录名稳定：同一份谱两次算同一个名字。
        assert_eq!(
            dir_name(Path::new("a/b.toml")),
            dir_name(Path::new("a/b.toml"))
        );
        assert_ne!(
            dir_name(Path::new("a/b.toml")),
            dir_name(Path::new("a/c.toml"))
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
