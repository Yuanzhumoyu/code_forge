//! **观测开关台账守卫**（v24 M4 立，v26 M1 扩到两侧）：`diagnostics::KNOBS`、
//! 源码里的 `FORGE_TRACE_*` 字面量、`docs/reference/backend-pipeline.md` 的表格
//! **三方双向相等**。
//!
//! ## 为什么是三方 + 双向
//!
//! 病根是一个**静默失效**：旧 `trace_enabled` 的白名单漏了一个名字，那个开关就恒为假、
//! 探针一句不打印而没人发现（`FORGE_TRACE_AGG` 死了几个月，`FORGE_TRACE_M4` 同病；
//! 白名单里的 `FORGE_TRACE_ISEL` 则一处调用点都没有——死条目）。所以这条守卫要同时钉住：
//!
//! 1. **源码**：`knob("FORGE_TRACE_X")` 与 `trace_enabled("X")` 的字面量集合 —— 真实用到的开关
//!    （含**直读** `env::var_os("FORGE_TRACE_X")` 的写法：v26 M1 之前有 4 处绕过台账）；
//! 2. **台账**：`forge_isa_runtime::diagnostics::KNOBS` —— 我们声称拥有的开关与用途；
//! 3. **文档**：契约文档「观测开关」小节的表 —— 使用者看到的清单。
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

/// 台账里的名字——**直接读类型**（不再解析源码文本：台账已下沉，`Knob.name` 是公开字段）。
fn table_knobs() -> BTreeSet<String> {
    forge_isa_runtime::diagnostics::KNOBS
        .iter()
        .map(|k| k.name.to_string())
        .collect()
}

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

/// 源码里的开关名：`FORGE_TRACE_*` 字面量 ∪ `trace_enabled("后缀")`。
///
/// * 跳过台账定义文件（否则"台账里的每个名字都在源码里"恒真，死条目永远抓不到）；
/// * 跳过注释行（文档注释里会举 `FORGE_TRACE_ISEL` 这类历史名）；
/// * 每个文件在第一个 `#[cfg(test)]` 处截断（测试夹具名如
///   `FORGE_TRACE_NOT_REGISTERED_AT_ALL` 不属于生产面——本仓扫描型守卫的既有惯用法）。
fn source_knobs(root: &std::path::Path) -> BTreeSet<String> {
    let mut files = Vec::new();
    rs_files(&root.join("crates"), &mut files);
    let mut out = BTreeSet::new();
    for p in &files {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "diagnostics.rs" || name == "trace_knobs.rs" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        for line in text.lines() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if t.contains("#[cfg(test)]") {
                break; // 之后的都是测试面
            }
            // 全名字面量（knob("FORGE_TRACE_X") / env::var_os("FORGE_TRACE_X")）
            let mut rest = line;
            while let Some(i) = rest.find("FORGE_TRACE_") {
                let tail = &rest[i + "FORGE_TRACE_".len()..];
                let n: String = tail
                    .chars()
                    .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                    .collect();
                if !n.is_empty() {
                    out.insert(format!("FORGE_TRACE_{n}"));
                }
                rest = tail;
            }
            // `trace_enabled("后缀")`（forge-rustc 侧写法）
            let mut rest = line;
            while let Some(i) = rest.find("trace_enabled(\"") {
                let tail = &rest[i + "trace_enabled(\"".len()..];
                if let Some((suffix, _)) = tail.split_once('"') {
                    out.insert(format!("FORGE_TRACE_{suffix}"));
                }
                rest = tail;
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
    let table = table_knobs();
    let source = source_knobs(&root);
    let doc = doc_knobs(&root);

    assert!(
        table.len() > 10,
        "台账只有 {} 条——守卫本身失效了？（v26 M1 后应为 41 条：后端 + forge-rustc）",
        table.len()
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
        "源码字面量 vs `diagnostics::KNOBS` 台账",
        "源码多 ⇒ 漏登记（旧病的形状：恒为假、静默失效）；台账多 ⇒ 死条目（如当年的 FORGE_TRACE_ISEL）",
    );
    diff(
        &doc,
        &table,
        "文档表格 vs `diagnostics::KNOBS` 台账",
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
