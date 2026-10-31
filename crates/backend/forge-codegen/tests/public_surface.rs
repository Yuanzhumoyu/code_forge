//! v30 M5：公开面快照 —— 新 API 在、旧 API 不在。
//!
//! 编译器已经保证"旧方法被删后没人能调用"，但文本守卫还能挡住**回潮**
//! （有人为了省事把 `get_fn` / `STAGES` / `opt_default` 加回来）。
//! 只扫生产源码，且跳过注释行（仓库既有守卫的同一口径）。

use std::path::{Path, PathBuf};

fn src_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            src_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn src_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// 根 `lib.rs` 必须导出 v30 的宿主面。
#[test]
fn root_exports_the_v30_surface() {
    let lib = std::fs::read_to_string(src_root().join("lib.rs")).expect("读 src/lib.rs");
    // 收集**逻辑** `pub use` 语句（rustfmt 会把长列表折成多行）。
    let mut block = String::new();
    let mut inside = false;
    for line in lib.lines() {
        let t = line.trim();
        if t.starts_with("pub use ") {
            inside = true;
        }
        if inside {
            block.push_str(t);
            block.push('\n');
        }
        if inside && t.ends_with(';') {
            inside = false;
        }
    }
    for name in [
        "CompileSession",
        "CompiledModule",
        "CompiledUnit",
        "CompileOptions",
        "CompileError",
        "Stage",
        "JitSignatureInfo",
        "Parallelism",
    ] {
        assert!(
            block.contains(name),
            "`lib.rs` 根导出缺少 {name}（v30 公开面）"
        );
    }
}

/// 旧符号不得在生产源码里回潮（注释行跳过）。
#[test]
fn removed_apis_do_not_reappear() {
    let mut files = Vec::new();
    src_files(&src_root(), &mut files);
    let mut hits = Vec::new();
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") || t.starts_with('*') {
                continue;
            }
            for (pat, why) in [
                ("pub const STAGES", "阶段清单唯一来源是 Stage::ALL"),
                ("pub mod opt_default", "选项已收进 pipeline/options.rs"),
                (
                    "pub fn get_fn<",
                    "JitLoader 的类型化入口是 get/get_unchecked",
                ),
            ] {
                if line.contains(pat) {
                    hits.push(format!(
                        "{}:{}: {why}（命中 `{pat}`）",
                        f.strip_prefix(src_root()).unwrap_or(f).display(),
                        i + 1
                    ));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "已删除的旧 API 又出现了：\n{}",
        hits.join("\n")
    );
}

/// 阶段枚举仍是**六个**管线阶段（`Module` 不属于管线）。
#[test]
fn pipeline_stage_count_is_six() {
    let stage = std::fs::read_to_string(src_root().join("pipeline/stage.rs")).expect("stage.rs");
    assert!(
        stage.contains("pub const ALL: [Stage; 6]"),
        "`Stage::ALL` 应为 6 个管线阶段（Module 不进 ALL）"
    );
}
