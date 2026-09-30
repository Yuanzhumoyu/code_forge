//! **文档守卫**：参考文档里的"守卫索引"不许指向不存在的测试。
//!
//! `docs/reference/calling-conventions.md` 的「现状总览与守卫索引」按能力列出状态与**钉住它
//! 的那条守卫**（测试名）。这类索引最容易的腐坏方式就是"测试改名/删掉了，文档还指着它"——
//! 于是用一条守卫把两边对齐：表里的每个反引号标识符都必须在 `crates/**/*.rs` 里找得到
//! `fn <名字>`（同 `schema_guard` 对键表、`varargs-plan` 对 `va_list` 形状表的做法）。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// 递归收集 `root` 下的 `.rs` 文件（跳过 `target/`）。
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect_rs(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// 取出索引表里"守卫"那一列的反引号标识符（跳过状态列与文件名/路径形态的片段）。
fn guard_names(doc: &str) -> Vec<String> {
    let Some(start) = doc.find("## 现状总览与守卫索引") else {
        panic!("docs/reference/calling-conventions.md 缺少「现状总览与守卫索引」一节");
    };
    let rest = &doc[start..];
    // 到下一个 `## ` 标题为止（该节只有这一张表）。
    let end = rest[3..].find("\n## ").map(|i| i + 3).unwrap_or(rest.len());
    let section = &rest[..end];

    let mut out: Vec<String> = Vec::new();
    for line in section.lines().filter(|l| l.starts_with('|')) {
        let cells: Vec<&str> = line.split('|').collect();
        if cells.len() < 4 {
            continue;
        }
        // 第三格 = 守卫列；成对反引号里是标识符（`a::b` 也算，取最后一段）。
        for tok in cells[3].split('`').skip(1).step_by(2) {
            let name = tok.rsplit("::").next().unwrap_or(tok).trim().to_string();
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                out.push(name);
            }
        }
    }
    out
}

#[test]
fn guard_index_names_exist() {
    let doc = std::fs::read_to_string(repo_root().join("docs/reference/calling-conventions.md"))
        .expect("读 docs/reference/calling-conventions.md");
    let names = guard_names(&doc);
    assert!(
        names.len() >= 15,
        "守卫索引解析到的名字太少（{} 个），检查表格式：{names:?}",
        names.len()
    );

    let mut sources: Vec<PathBuf> = Vec::new();
    collect_rs(&repo_root().join("crates"), &mut sources);
    assert!(sources.len() > 50, "源码文件收集异常：{sources:?}");

    let mut missing: Vec<String> = Vec::new();
    for name in &names {
        let needle = format!("fn {name}(");
        let found = sources.iter().any(|p| {
            std::fs::read_to_string(p)
                .map(|t| t.contains(&needle))
                .unwrap_or(false)
        });
        if !found {
            missing.push(name.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "守卫索引指向了不存在的测试（改名/删除了？）：{missing:?}\n\
         修法：改 docs/reference/calling-conventions.md 的「现状总览与守卫索引」，或补回守卫。"
    );
}
