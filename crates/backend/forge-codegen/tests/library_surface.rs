//! 库表面守卫：**demo/示例 ISA 不进发行库**。
//!
//! ISA-DSL 的示例谱（`demo_v12` / `demo8_v12`）只服务于测试：定宽 32 位、
//! 指令集极小，便于穷尽断言汇编器/解码器/编码器/lowering。它们必须留在
//! `tests/`（`tests/isa/*.toml` + `tests/common/mod.rs` 用
//! `isa_from_file!(…)` 生成），库本体只有真实后端
//! （x86_64 / aarch64 / riscv64）。
//!
//! 回归方式：任何人把 demo 谱搬回 `src/`、或重新在 `src/arch/mod.rs` /
//! `src/lib.rs` 里导出，本测试立刻失败。

use std::path::{Path, PathBuf};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// 递归收集 `dir` 下的 `.rs` 文件。
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// 库源码（`src/**`）不得**引用** demo 谱（模块声明、re-export、名字表、
/// 指令名等）。仅允许在文档注释里提到"示例谱在 tests/"。
#[test]
fn library_sources_do_not_reference_demo_isas() {
    let src = manifest_dir().join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(!files.is_empty(), "src/** 为空？路径错误");

    let mut violations: Vec<String> = Vec::new();
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let rel = f
            .strip_prefix(&src)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/");
        for (i, line) in text.lines().enumerate() {
            let t = line.trim();
            if t.starts_with("//") {
                continue; // 注释里可以说明"夹具在 tests/"（不是引用）
            }
            if t.contains("demo_v12") || t.contains("demo8_v12") || t.contains("arch::demo") {
                violations.push(format!("{rel}:{}: {t}", i + 1));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "库源码引用 demo 谱（必须留在 tests/isa/ + tests/common/mod.rs）：\n{}",
        violations.join("\n")
    );
}

/// `src/arch/mod.rs` 只登记真实后端；`src/lib.rs` 不 re-export demo。
#[test]
fn arch_module_lists_only_real_backends() {
    let arch_mod = manifest_dir().join("src/arch/mod.rs");
    let text = std::fs::read_to_string(&arch_mod).expect("src/arch/mod.rs 必须存在");
    let mods: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("pub mod "))
        .collect();
    assert_eq!(
        mods,
        vec![
            "pub mod arm64_v12;",
            "pub mod riscv64_v12;",
            "pub mod x86_v12;",
        ],
        "arch 模块清单必须是真实后端（demo 谱在 tests/）"
    );

    let lib = std::fs::read_to_string(manifest_dir().join("src/lib.rs")).expect("src/lib.rs");
    for forbidden in ["pub use arch::demo_v12", "pub use arch::demo8_v12"] {
        assert!(
            !lib.contains(forbidden),
            "src/lib.rs 不得 re-export demo 谱：{forbidden}"
        );
    }
}

/// 仓库根 `isa/` 只放**发行后端**的谱；demo 谱必须在 crate 的 tests/ 下。
#[test]
fn repo_isa_dir_has_no_demo_specs() {
    // CARGO_MANIFEST_DIR = crates/backend/forge-codegen → 仓库根 = 上 3 级
    let root_isa = manifest_dir().join("../../..").join("isa");
    let Ok(entries) = std::fs::read_dir(&root_isa) else {
        panic!("找不到仓库根 isa/：{root_isa:?}");
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "arm64_v12.toml".to_string(),
            "riscv64_v12.toml".to_string(),
            "x86_v12.toml".to_string(),
        ],
        "仓库根 isa/ 应只有真实后端谱"
    );

    // 测试夹具谱必须存在（否则测试共享模块编译不过，但守卫先点名缺哪个）
    for f in [
        "demo_v12.toml",
        "demo8_v12.toml",
        "demo_inst8_v12.toml",
        "demo_inst12_v12.toml",
        "demo_inst100_v12.toml",
    ] {
        let p = manifest_dir().join("tests/isa").join(f);
        assert!(p.is_file(), "测试夹具谱缺失：{p:?}");
    }
}
