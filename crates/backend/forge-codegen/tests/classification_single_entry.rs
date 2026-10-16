//! **分类单一入口守卫**（v25 M1）。
//!
//! ## 病根
//!
//! v24 的两相化只做完了**改写**那一半：计划相产出的 `AbiEdges` 无人消费，而落点决策
//! 散在 5 个调用点，每处各自跑一遍引擎规划（`plan_for_shapes` + 注册表读锁），
//! 并用本仓手抄的落点枚举表达（它是 `forge-abi-model::Placement` 的三值投影）。
//!
//! ## 判据
//!
//! 1. `plan_for_shapes`（**唯一**问 ABI 引擎的函数）在 `crates/backend/forge-codegen/src`
//!    里只允许出现在两处：`legalize/classify.rs`（合法化分类）与 `pipeline/abi_setup.rs`
//!    （函数级调用布局）。清单**双向相等**：出现未列出的文件 ⇒ 分类又分叉了；
//!    列出的文件不再出现 ⇒ 白名单是死的（仓库纪律：白名单条目**必须被命中**）。
//! 2. **删除清单**：手抄的落点枚举与两个单类型探针在全仓**零出现**
//!    （守卫文件自身必须写出这些名字 ⇒ 自跳过，理由同 M3/M4 的守卫）。

use std::collections::BTreeSet;

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

/// 允许问 ABI 引擎的三个文件（相对 `crates/backend/forge-codegen/`）：
///
/// * `legalize/classify.rs` —— 合法化的**唯一**分类入口（本守卫的主角）；
/// * `abi_setup.rs` —— 编译入口的函数级调用布局（`ctx.conv.layout`，v20 A6 起 fail-closed）；
/// * `abi_target.rs` —— 管线**引擎入口本身**（`impl AbiTarget` 的工厂闭包里调
///   `plan_for_shapes`）。它不是消费者，而是"问引擎"这件事的实现点。
const ALLOWED: [&str; 3] = [
    "src/pipeline/legalize/classify.rs",
    "src/pipeline/abi_setup.rs",
    "src/pipeline/abi_target.rs",
];

fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// **分类只有一个入口**：问引擎的调用点集合与白名单**双向相等**。
#[test]
fn classification_has_one_entry() {
    let root = repo_root();
    let cg = root.join("crates/backend/forge-codegen");
    let mut files = Vec::new();
    rs_files(&cg.join("src"), &mut files);

    let mut hits: BTreeSet<String> = BTreeSet::new();
    for p in &files {
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        if !text.contains("plan_for_shapes(") {
            continue;
        }
        let rel = p
            .strip_prefix(&cg)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/");
        hits.insert(rel);
    }
    let want: BTreeSet<String> = ALLOWED.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(
        hits, want,
        "问 ABI 引擎（`plan_for_shapes`）的位置与白名单不一致：\n  实际 {hits:?}\n  期望 {want:?}\n  \
         未列出的文件 = 分类分叉；列出的文件没出现 = 白名单死条目（两向都红）"
    );
}

/// **计划驱动改写**（v25 M1）：生产者的落点必须**来自计划**，而不是自己再建一个分类器。
///
/// 两条互补判据（都是源码级、双向相等）：
///
/// 1. `Classifier::new` 只允许出现在计划相 `legalize/plan.rs`——生产者**没有**机器与约定名
///    可问（签名已改），这条钉住"别再把它们传回去"；
/// 2. 生产者 `legalize/agg_param.rs` / `legalize/agg_call.rs` **必须**引用计划类型
///    （`AbiEdges`）——否则它们又从别处拿落点（计划就再次沦为装饰）。
#[test]
fn edges_drive_the_rewrite() {
    let root = repo_root();
    let dir = root.join("crates/backend/forge-codegen/src/pipeline/legalize");
    let mut files = Vec::new();
    rs_files(&dir, &mut files);

    let mut classifiers: BTreeSet<String> = BTreeSet::new();
    let mut consumers: BTreeSet<String> = BTreeSet::new();
    for p in &files {
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        let rel = p
            .strip_prefix(&dir)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/");
        if text.contains("Classifier::new(") {
            classifiers.insert(rel.clone());
        }
        if text.contains("AbiEdges") {
            consumers.insert(rel);
        }
    }
    // `classify.rs` 是**定义处**（它自己带单测，单测里当然要建分类器）⇒ 与计划相一起白名单。
    let want_clf: BTreeSet<String> = ["plan.rs", "classify.rs"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(
        classifiers, want_clf,
        "只有计划相（与定义处）可以建分类器：\n  实际 {classifiers:?}\n  期望 {want_clf:?}"
    );
    let want_consumers: BTreeSet<String> = [
        "plan.rs",
        "classify.rs",
        "agg_param.rs",
        "mod.rs",
        // v25 M4 把 `agg_call.rs` 切成目录 ⇒ 三段都引用计划（同一个 pass 的三部分）。
        "agg_call/mod.rs",
        "agg_call/args.rs",
        "agg_call/results.rs",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    assert_eq!(
        consumers, want_consumers,
        "生产者必须引用计划（`AbiEdges`），否则计划只是装饰：\n  实际 {consumers:?}\n  期望 {want_consumers:?}"
    );
}

/// **手抄落点类型与单类型探针已退场**：全仓零出现。
#[test]
fn hand_copied_placement_is_gone() {
    let root = repo_root();
    let mut files = Vec::new();
    rs_files(&root.join("crates"), &mut files);
    let mut hits: Vec<String> = Vec::new();
    for p in files {
        // 本守卫文件必须写出这些名字（否则无从检查）⇒ 自跳过。
        if p.file_name()
            .is_some_and(|n| n == "classification_single_entry.rs")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            for name in ["AggArgLoc", "agg_arg_place", "agg_ret_is_sret"] {
                if line.contains(name) {
                    hits.push(format!(
                        "{}:{}: {}",
                        p.strip_prefix(&root).unwrap_or(&p).display(),
                        i + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "手抄的落点枚举 / 单类型探针回潮了（v25 M1 已统一到 `legalize/classify.rs`）：\n{}",
        hits.join("\n")
    );
}
