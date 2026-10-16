//! **错误码台账守卫**（v26 M1）。
//!
//! ## 病根
//!
//! `docs/reference/isa-dsl-errors.md` 是"报错看不懂先看它"的目录，但**两侧从无人对账**：
//! 实测（2026-10-11）源码发 29 个码、文档列 31 个——文档教了两个**永不发出**的码
//! （`DSL-ABI`/`DSL-SLOT`），而源码新发的两个 `LINT-*` 一个都没写进文档。
//! 深一层：两个 kind（`operand_slots`/`machine`）在 `section_code` 里**没有映射**，
//! 于是这两节的诊断一直落到 `DSL-OTHER`——而文档写着"出现 `DSL-OTHER` 即说明消息缺前缀、应报 bug"，
//! 判据被反用。v26 M1 补齐映射（`DSL-SLOT`/`DSL-MACHINE`）并加这条守卫。
//!
//! ## 判据（双向相等）
//!
//! * **源码侧**：`crates/frontend/forge-isa-dsl/src/**` 里 `"DSL-…"`/`"LINT-…"` 字符串字面量集合；
//! * **文档侧**：`docs/reference/isa-dsl-errors.md` 的 **§2「错误码 = 出错所在节」表**里的码
//!   （只扫这一节：正文散文里的历史提及与链接文本不该进台账——实测 `dsl-tutorial` 这类
//!   链接文本会被大范围正则误命中）。
//!
//! 任一侧多一个少一个都红；改码就同时改这两处。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
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

/// 取一段文本里所有 `DSL-x`/`LINT-x` 形态的码（大写字母/数字/连字符）。
fn codes_in(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        for prefix in ["DSL-", "LINT-"] {
            if bytes[i..].starts_with(&prefix.chars().collect::<Vec<_>>()[..]) {
                let mut j = i + prefix.len();
                let mut code = String::from(prefix);
                while j < bytes.len()
                    && (bytes[j].is_ascii_uppercase()
                        || bytes[j].is_ascii_digit()
                        || bytes[j] == '-')
                {
                    code.push(bytes[j]);
                    j += 1;
                }
                // 至少一个字符，且不以 `-` 结尾（`LINT-*` 这类族名不算）
                if code.len() > prefix.len() && !code.ends_with('-') {
                    out.insert(code);
                }
                i = j;
                break;
            }
        }
        i += 1;
    }
    out
}

/// 文档里 `## 2. …` 那一节（到下一个 `## ` 为止）。
fn section_two(doc: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in doc.lines() {
        if line.starts_with("## ") {
            inside = line.starts_with("## 2.");
            continue;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[test]
fn error_codes_match_doc() {
    let root = repo_root();

    // 源码侧
    let mut files = Vec::new();
    rs_files(&root.join("crates/frontend/forge-isa-dsl/src"), &mut files);
    let mut source: BTreeSet<String> = BTreeSet::new();
    for p in &files {
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        // 只认**带引号的字面量**（注释里的举例不算发射点）
        for (i, line) in text.lines().enumerate() {
            let _ = i;
            if line.trim_start().starts_with("//") {
                continue;
            }
            for code in codes_in(line) {
                source.insert(code);
            }
        }
    }

    // 文档侧
    let doc_path = root.join("docs/reference/isa-dsl-errors.md");
    let doc = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("读不到 {}：{e}", doc_path.display()));
    let documented = codes_in(&section_two(&doc));

    assert!(
        !source.is_empty(),
        "源码里一个错误码都没扫到（扫描口径坏了？）"
    );
    assert!(
        !documented.is_empty(),
        "文档 §2 里一个错误码都没扫到（小节定位坏了？）"
    );
    let only_source: Vec<&String> = source.difference(&documented).collect();
    let only_doc: Vec<&String> = documented.difference(&source).collect();
    assert!(
        only_source.is_empty() && only_doc.is_empty(),
        "错误码台账与源码不一致（改码就同时改两处）：\n  只在源码 {:?}\n  只在文档 {:?}",
        only_source,
        only_doc
    );
}
