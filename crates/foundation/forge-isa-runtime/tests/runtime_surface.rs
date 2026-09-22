//! 运行面边界守卫（v19 V1）：`forge-isa-runtime` 不认识、也不得依赖编译管线。
//!
//! 这条不变量是 G1（"任意普通 crate 只依赖 runtime 就能承载一份 ISA 谱"）的地基：
//! 一旦 runtime 反向引用 `forge_codegen`/`pipeline`，拆出去就白拆了。守卫做两件事：
//!
//! 1. **依赖面**：`Cargo.toml` 的 `[dependencies]` 只允许 `forge-ir` 与两个纯工具库
//!    （`smallvec`/`thiserror`）；出现 `forge-codegen` 直接失败。
//! 2. **源码面**：`src/**` 里不得出现 `forge_codegen` / `crate::pipeline` 之类指向管线的路径
//!    （文档注释里提到"管线"这个词是允许的——只查路径形态）。
//!
//! 反例一出现就红，因此"顺手把 pipeline 的东西塞回 runtime"不会悄悄发生。

use std::path::{Path, PathBuf};

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

#[test]
fn runtime_dependencies_are_pipeline_free() {
    let manifest = std::fs::read_to_string(crate_dir().join("Cargo.toml")).expect("Cargo.toml");
    let deps = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("有 [dependencies] 段")
        .split("\n[")
        .next()
        .expect("段落存在");
    let allowed = ["forge-ir", "smallvec", "thiserror"];
    let mut seen = 0;
    for line in deps.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let name = line.split(['=', ' ']).next().unwrap_or("");
        assert!(
            allowed.contains(&name),
            "forge-isa-runtime 只允许依赖 {allowed:?}，多了 `{name}`——运行面不得反向依赖编译管线"
        );
        seen += 1;
    }
    assert!(seen > 0, "没解析到任何依赖？守卫失效");
}

#[test]
fn runtime_sources_never_reference_the_pipeline() {
    let mut checked = 0usize;
    for entry in walk(&crate_dir().join("src")) {
        let text = std::fs::read_to_string(&entry).expect("读源文件");
        // 只查**代码行**：文档注释里提到"管线/宿主"是允许的，路径形态才是硬约束。
        for (no, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for needle in ["forge_codegen", "crate::pipeline"] {
                assert!(
                    !line.contains(needle),
                    "{}:{} 出现了 `{needle}`：运行面不得引用宿主编译管线",
                    entry.display(),
                    no + 1
                );
            }
        }
        checked += 1;
    }
    assert!(checked >= 10, "只扫到 {checked} 个文件？路径不对");
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out
}
