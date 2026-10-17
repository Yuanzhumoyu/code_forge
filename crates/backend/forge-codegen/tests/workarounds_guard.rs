//! **WORKAROUNDS 登记表守卫**（v27 M5）：`[WA-NN]` 编号唯一 + 代码引用都能在表里找到。
//!
//! ## 为什么需要它
//!
//! `crates/tools/forge-rustc/WORKAROUNDS.md` 是**机读清单**（代码注释用 `[WA-NN]` 回指），
//! 但此前没有任何东西检查它：
//!
//! - 编号可以重复（两个不同的绕法共用一个号 ⇒ 注释回指到哪个？）；
//! - 代码可以引用一个**表里不存在**的编号（读者照编号去查会扑空）；
//! - 立项时更直接的教训（v27 §8）：**"根治方向"列是写那一行时的判断，不是现状**——
//!   方案照着它做计划，会把早已关闭的项当缺口（E5/E6 就是这么来的）。
//!
//! 本守卫钉前两条（可机检）；第三条在表头已用文字写明（"状态以代码与测试为准"）。
//!
//! ## 口径
//!
//! 1. `WORKAROUNDS.md` 里每个 `WA-NN` 编号**唯一**（表行 + 表头示例除外）；
//! 2. `crates/**/*.rs` 里出现的 `[WA-NN]` 引用，**都能**在表里找到对应行。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

/// 从文本里抽 `WA-NN`（只认大写 `WA-` + 数字）。
fn wa_ids(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 3 < bytes.len() {
        if &bytes[i..i + 3] == b"WA-" {
            let mut j = i + 3;
            let start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > start {
                out.push(text[i..j].to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// 递归收集 `*.rs`。
fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn workarounds_guard() {
    let root = repo_root();
    let wa = root.join("crates/tools/forge-rustc/WORKAROUNDS.md");
    let text =
        std::fs::read_to_string(&wa).unwrap_or_else(|e| panic!("读不到 {}：{e}", wa.display()));

    // ① 表行编号唯一。口径：**基础编号**（`WA-NN`）唯一；
    //    该表允许"增补"行（`WA-33 增补（M2）`/`（M10）`——同一条绕法的后续补记），
    //    它们**共用**基础编号，不算重复；但"裸编号行"（首格恰好等于 `WA-NN`）只能有一行。
    let mut table: BTreeMap<String, usize> = BTreeMap::new(); // 基础编号 → 行数
    let mut bare: BTreeMap<String, usize> = BTreeMap::new(); // 裸编号行 → 行数
    for line in text.lines() {
        if !line.trim_start().starts_with("| WA-") {
            continue;
        }
        let cell = line
            .trim_start()
            .trim_start_matches('|')
            .trim()
            .split('|')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        let base = wa_ids(&cell).into_iter().next().unwrap_or_default();
        if base.is_empty() {
            continue;
        }
        *table.entry(base.clone()).or_default() += 1;
        if cell == base {
            *bare.entry(base).or_default() += 1;
        }
    }
    let dup: Vec<(&String, &usize)> = bare.iter().filter(|(_, n)| **n > 1).collect();
    assert!(
        dup.is_empty(),
        "WORKAROUNDS.md 里有重复的**裸编号行**（每个绕法一个号，注释才有唯一回指）：{dup:?}"
    );
    assert!(
        !table.is_empty(),
        "WORKAROUNDS.md 里一行表项都没解析到——守卫口径失效"
    );

    // ② 代码里的引用都能找到表行。
    let mut files = Vec::new();
    rs_files(&root.join("crates"), &mut files);
    let mut dangling: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        for id in wa_ids(&src) {
            if !table.contains_key(&id) {
                let rel = f
                    .strip_prefix(&root)
                    .unwrap_or(f)
                    .to_string_lossy()
                    .replace('\\', "/");
                dangling.entry(id).or_default().push(rel);
            }
        }
    }
    assert!(
        dangling.is_empty(),
        "代码里引用了 WORKAROUNDS.md 里不存在的编号（读者照号去查会扑空）：{dangling:?}"
    );

    // 反向**只报告不失败**：先收集"被引用的编号"集合。
    let referenced: BTreeSet<String> = {
        let mut s = BTreeSet::new();
        for f in &files {
            if let Ok(src) = std::fs::read_to_string(f) {
                s.extend(wa_ids(&src));
            }
        }
        s
    };
    // 有些行是约定/工具链条目，本来就没有代码站点可回指——把它们判成红会逼着作者写假注释。：有些行是"约定/工具链要求"（WA-12/WA-13、`__CxxFrameHandler3` 之类），
    // 本来就没有代码站点可回指——把它们判成红会逼着作者写假注释（本仓明确反对）。
    let unref: Vec<&String> = table
        .keys()
        .filter(|id| !referenced.contains(*id))
        .collect();
    if !unref.is_empty() {
        println!("note: 以下编号没有代码回指（可能是约定/工具链条目，不是缺陷）：{unref:?}");
    }

    println!(
        "OK  workarounds_guard：表内 {} 个编号，唯一且全部代码引用可解析（未回指 {} 个）",
        table.len(),
        unref.len()
    );
}
