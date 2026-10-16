//! **观测开关台账守卫**（v24 M4）：`pipeline::trace::KNOBS`、源码调用点与
//! `docs/reference/backend-pipeline.md` 的表格**三方双向相等**。
//!
//! ## 为什么是三方 + 双向
//!
//! 病根是一个**静默失效**：`trace_enabled` 的白名单漏了一个名字，那个开关就恒为假、
//! 探针一句不打印而没人发现（`FORGE_TRACE_AGG` 死了几个月，`FORGE_TRACE_M4` 同病）。
//! 所以这条守卫要同时钉住三个面：
//!
//! 1. **源码**：`trace::knob("…")` 的字面量集合 —— 真实用到的开关；
//! 2. **台账**：`KNOBS` 表 —— 我们声称拥有的开关与它们的用途；
//! 3. **文档**：`reference/backend-pipeline.md` 的表格 —— 使用者看到的清单。
//!
//! 任一面多一条或少一条都红：源码多 ⇒ 漏登记（正是旧病）；台账多 ⇒ 死条目
//! （`FORGE_TRACE_ISEL` 就是这样）；文档多/少 ⇒ 承诺与实现分叉。
//!
//! 另外钉住 M4 的验收判据：全仓 `TRACE_KEYS` == 0（旧白名单不许回潮）。

use std::collections::BTreeSet;

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

/// `KNOBS` 表里的名字（`Knob::new("NAME", …)`）。
///
/// **跨行**解析：`rustfmt` 会把长行折成 `Knob::new(\n    "NAME",\n    "说明",\n)`——按行匹配
/// 会在格式化之后**静默漏掉条目**（M3 的能力缺口守卫踩过同一个坑）。
fn table_knobs(root: &std::path::Path) -> BTreeSet<String> {
    let text =
        std::fs::read_to_string(root.join("crates/backend/forge-codegen/src/pipeline/trace.rs"))
            .expect("读 trace.rs");
    let mut out = BTreeSet::new();
    let mut from = 0usize;
    while let Some(rel) = text[from..].find("Knob::new(") {
        let at = from + rel + "Knob::new(".len();
        let rest = text[at..].trim_start();
        if let Some(after_quote) = rest.strip_prefix('"')
            && let Some((name, _)) = after_quote.split_once('"')
        {
            out.insert(name.to_string());
        }
        from = at;
    }
    out
}

/// 源码里 `trace::knob("NAME")` 的字面量（生产代码，不含 trace.rs 自己）。
fn source_knobs(root: &std::path::Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let src = root.join("crates/backend/forge-codegen/src");
    let mut stack = vec![src];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().is_none_or(|x| x != "rs")
                || p.file_name().is_some_and(|n| n == "trace.rs")
            {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let mut from = 0usize;
            while let Some(rel) = text[from..].find("trace::knob(\"") {
                let at = from + rel + "trace::knob(\"".len();
                let rest = &text[at..];
                if let Some((name, _)) = rest.split_once('"') {
                    out.insert(name.to_string());
                }
                from = at;
            }
        }
    }
    out
}

/// 文档表格里的开关名（`| \`FORGE_TRACE_X\` | … |`）。
fn doc_knobs(root: &std::path::Path) -> BTreeSet<String> {
    let path = root.join("docs/reference/backend-pipeline.md");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("| `FORGE_TRACE_") {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        out.insert(cells[0].trim_matches('`').to_string());
    }
    out
}

#[test]
fn trace_knobs_are_bijective() {
    let root = repo_root();
    let table = table_knobs(&root);
    let source = source_knobs(&root);
    let doc = doc_knobs(&root);

    assert!(
        !table.is_empty(),
        "KNOBS 表为空——守卫本身失效了（解析失败？）"
    );

    let diff = |a: &BTreeSet<String>, b: &BTreeSet<String>, what: &str, hint: &str| {
        let extra: Vec<_> = a.difference(b).collect();
        let missing: Vec<_> = b.difference(a).collect();
        assert!(
            extra.is_empty() && missing.is_empty(),
            "{what} 不一致：\n  多出 {extra:?}\n  缺少 {missing:?}\n  {hint}"
        );
    };
    diff(
        &source,
        &table,
        "源码调用点 vs `KNOBS` 台账",
        "源码多 ⇒ 漏登记（旧病的形状：恒为假、静默失效）；台账多 ⇒ 死条目",
    );
    diff(
        &doc,
        &table,
        "文档表格 vs `KNOBS` 台账",
        "文档是使用者看到的承诺；分叉就是当年\"文档承诺可见、实际看不见\"",
    );
}

/// M4 验收判据：**全仓 `TRACE_KEYS` == 0**（旧白名单不许回潮）。
#[test]
fn old_whitelist_is_gone() {
    let root = repo_root();
    let mut hits: Vec<String> = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            // **本守卫文件自身**必须写出旧名（否则无从检查）⇒ 跳过它自己。
            if p.file_name().is_some_and(|n| n == "trace_knobs.rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            for (i, line) in text.lines().enumerate() {
                if line.contains("TRACE_KEYS") {
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
        "旧的 `TRACE_KEYS` 白名单回潮了（v24 M4 已改为开放查询 + `KNOBS` 台账）：\n{}",
        hits.join("\n")
    );
}
