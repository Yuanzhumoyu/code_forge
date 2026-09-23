//! 运行面边界守卫（v19 V1）：`forge-isa-runtime` 不认识、也不得依赖编译管线。
//!
//! 这条不变量是 G1（"任意普通 crate 只依赖 runtime 就能承载一份 ISA 谱"）的地基：
//! 一旦 runtime 反向引用 `forge_codegen`/`pipeline`，拆出去就白拆了。守卫做两件事：
//!
//! 1. **依赖面**：`Cargo.toml` 的 `[dependencies]` 只允许 `forge-ir` 与两个纯工具库
//!    （`smallvec`/`thiserror`）；出现 `forge-codegen` 直接失败。
//! 2. **源码面**：`src/**` 里不得出现 `forge_codegen` / `crate::pipeline` 之类指向**宿主**
//!    管线的路径（文档注释里提到"管线"这个词是允许的——只查路径形态）。**例外**：
//!    `$crate::pipeline` 是 `#[macro_export]` 宏指"定义宏的 crate 自己"（= runtime）的
//!    正确写法（`src/erased.rs`），不是反向依赖——v19 V2 修掉这条假阳性，判据见
//!    `pipeline_hits`，并由 `pipeline_matcher_ignores_dollar_crate_only` 钉住。
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
            if let Some(needle) = pipeline_hits(line).first() {
                panic!(
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

/// 源码行里指向**宿主管线**的路径。
///
/// 判据比"包含 `crate::pipeline`"细一格：`$crate::pipeline` 是
/// `#[macro_export]` 宏里指"定义宏的 crate 自己"（= runtime）的**唯一正确写法**
/// ——它出现在 `src/erased.rs` 的宏体里（`impl_erased_target_machine!` 的 `compile`），
/// 不是反向依赖宿主。v19 V2 修：此前这条守卫把它当命中，而它是在 V1b 把宏搬进
/// runtime 之后才出现的**假阳性**（本守卫只查源码形态，不含宏展开）。
fn pipeline_hits(line: &str) -> Vec<&'static str> {
    let mut hits = Vec::new();
    if line.contains("forge_codegen") {
        hits.push("forge_codegen");
    }
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(rel) = line[from..].find("crate::pipeline") {
        let at = from + rel;
        if at == 0 || bytes[at - 1] != b'$' {
            hits.push("crate::pipeline");
            break;
        }
        from = at + "crate::pipeline".len();
    }
    hits
}

/// 守卫自身的判据（防止"修假阳性"修成"永远不报"）。
#[test]
fn pipeline_matcher_ignores_dollar_crate_only() {
    assert!(
        pipeline_hits(
            "        $crate::pipeline::compile_via_pipeline(self.erased_name(), self, func)"
        )
        .is_empty(),
        "`$crate::pipeline` 是宏里的正确写法，不得判红"
    );
    assert_eq!(pipeline_hits("use crate::pipeline::emit;").len(), 1);
    // `$` 前缀命中的**后续**出现仍要判红（循环必须继续往后扫，而不是一遇到 `$` 就收手）。
    assert_eq!(
        pipeline_hits("let a = $crate::pipeline::x; let b = crate::pipeline::y;").len(),
        1
    );
    // 源文件里路径形态由 rustfmt 归一（`crate::pipeline`），带空格的写法不匹配——
    // 匹配器只管真实源码形态，不为不可能的写法加复杂度。
    assert!(pipeline_hits("let p = crate :: pipeline;").is_empty());
    assert_eq!(
        pipeline_hits("fn f() { forge_codegen::pipeline_hooks::ensure_registered(); }").len(),
        1
    );
    assert_eq!(
        pipeline_hits("//! 文档里可以提 crate::pipeline 与 forge_codegen").len(),
        2,
        "匹配器本身不管注释——注释由调用方按 `//` 前缀跳过"
    );
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
