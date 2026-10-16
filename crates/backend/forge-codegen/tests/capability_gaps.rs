//! **能力缺口台账守卫**（v24 M3）：源码里的缺口集合与
//! `docs/reference/backend-gaps.md` 的台账**逐一相等**。
//!
//! ## 为什么是"双向相等"而不是"包含"
//!
//! 只查一个方向都能被绕过：只查"台账 ⊆ 源码"⇒ 台账可以留一堆早已不存在的行（读者按它
//! 排查会白费功夫）；只查"源码 ⊆ 台账"⇒ 新加的缺口可以不出现在台账里（那正是 M3 要治的病：
//! 缺口散在代码里、没人知道全貌）。两个方向都查，台账才**可信**。
//!
//! ## 判据
//!
//! 1. 源码（`crates/backend/forge-codegen/src/**`，**不含** `gaps.rs` 自己）里
//!    `capability("ID", …)` 的 ID 集合 == 台账表的 ID 列；
//! 2. 每个 ID 在源码里**恰好一处**（同一缺口不该有多个触发点——抽不出一个失败点说明
//!    缺口的边界还没想清）；
//! 3. 台账每行的"决策"列必须是封闭词表之一（`已实现`/`数据层`/`不做`/`待做`）——
//!    否则"决策"会退化成随手写的散文。

use std::collections::{BTreeMap, BTreeSet};

/// 决策列的封闭词表（与台账 §2 的口径说明一致）。
const DECISIONS: [&str; 4] = ["已实现", "数据层", "不做", "待做"];

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

/// 源码里所有 `capability("ID", …)` 的 (ID → 出现处)。
fn source_gaps(root: &std::path::Path) -> BTreeMap<String, Vec<String>> {
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
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
            if p.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            // `gaps.rs` 是**助手本体**（它自己的单测里有一个 DEMO_GAP）——不是缺口站点的集合。
            if p.file_name().is_some_and(|n| n == "gaps.rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            // **跨行**扫描：ID 允许写在 `capability(` 的下一行（多行调用），
            // 所以从每次出现处往后跳过空白，再要求一个引号包起来的 ID。
            let mut from = 0usize;
            while let Some(rel_pos) = text[from..].find("capability(") {
                let at = from + rel_pos;
                let line = text[..at].matches('\n').count() + 1;
                let after = text[at + "capability(".len()..].trim_start();
                if let Some(id) = after.strip_prefix('"').and_then(|r| r.split('"').next())
                    && !id.is_empty()
                {
                    found
                        .entry(id.to_string())
                        .or_default()
                        .push(format!("{rel}:{line}"));
                }
                from = at + "capability(".len();
            }
        }
    }
    found
}

/// 台账表的 (ID → 决策)（只认形如 `| \`ID\` | … | 决策 | … |` 的行）。
fn ledger_gaps(root: &std::path::Path) -> BTreeMap<String, String> {
    let path = root.join("docs/reference/backend-gaps.md");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
    let mut rows: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("| `") {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 4 {
            continue;
        }
        let id = cells[0].trim_matches('`').to_string();
        if !id.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
            continue;
        }
        // 决策列去掉 Markdown 强调（`**已实现**` 的呈现层次不属于词表）。
        let decision = cells[2].trim_matches(['*', ' ', '`']).to_string();
        rows.insert(id, decision);
    }
    rows
}

/// **双向逐一相等**：源码缺口集合 == 台账 ID 列；每个 ID 在源码里恰好一处；
/// 决策列 ∈ 封闭词表。
#[test]
fn capability_gaps_guard() {
    let root = repo_root();
    let src = source_gaps(&root);
    let ledger = ledger_gaps(&root);

    let src_ids: BTreeSet<&String> = src.keys().collect();
    let ledger_ids: BTreeSet<&String> = ledger.keys().collect();
    let only_src: Vec<&&String> = src_ids.difference(&ledger_ids).collect();
    let only_ledger: Vec<&&String> = ledger_ids.difference(&src_ids).collect();
    assert!(
        only_src.is_empty(),
        "这些缺口的失败点已在源码里，但台账没有行（台账漏了 = 读者看不到全貌）：\n{}",
        only_src
            .iter()
            .map(|id| format!("  {id} @ {:?}", src[**id]))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        only_ledger.is_empty(),
        "这些台账行已没有对应的失败点（行是死的 = 读者会白费功夫）：{only_ledger:?}"
    );

    for (id, sites) in &src {
        assert_eq!(
            sites.len(),
            1,
            "缺口 `{id}` 在源码里有 {} 个触发点（{sites:?}）——同一缺口应抽成一个失败点",
            sites.len()
        );
    }

    for (id, decision) in &ledger {
        assert!(
            DECISIONS.contains(&decision.as_str()),
            "台账行 `{id}` 的决策列是 {decision:?}，不在封闭词表 {DECISIONS:?} 里"
        );
    }

    assert!(!ledger.is_empty(), "台账为空——守卫本身失效了（读表失败？）");
}
