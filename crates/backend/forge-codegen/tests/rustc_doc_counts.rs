//! **forge-rustc 文档计数守卫**（v27 M5）：README 的用例计数与 `tests/e2e.rs` **双向相等**。
//!
//! ## 为什么需要它（立项证据 E4）
//!
//! README 的「支持矩阵」长期写着「**103** 用例：98 硬断言 + 5 个 `FLAKY` 容忍项」，
//! 而 `tests/e2e.rs` 里的真相是 **110 个 `Case`、`known_failure: true` = 0、`FLAKY` 为空**
//! （2026-09-12 五例已全部转正，`WORKAROUNDS.md` 的 WA-11 也这么说）。
//! 两边谁都没错——只是**没有任何东西钉住它们**：代码加了用例、转了正，README 照旧。
//! 后果是读者（和人机）会照着一份错的"当前能力面"做判断。
//!
//! ## 口径
//!
//! README 里有一段**机读标记**（`cases=… hard=… flaky=… known=… expect_trap=…`），
//! 本守卫把它与 `tests/e2e.rs` 的实测计数逐项比对：
//!
//! - `cases` = `CASES` 数组里的 `Case` 个数；
//! - `hard` = `cases - known`（硬断言数）；
//! - `flaky` = `FLAKY` 名单长度；
//! - `known` = 代码里 `known_failure: true` 的个数（注释里的历史说明不算）；
//! - `expect_trap` = `expect_trap: true` 的个数（trap 语义的覆盖数）。
//!
//! **双向**：README 缺标记 ⇒ 红（否则守卫会"看不见"文档）；数字不一致 ⇒ 红。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

/// 只保留"代码行"（去掉 `//` 行注释），避免把文档注释里的历史说明算进计数。
fn code_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect()
}

/// `tests/e2e.rs` 的实测计数。
fn e2e_counts(root: &Path) -> BTreeMap<String, usize> {
    let path = root.join("crates/tools/forge-rustc/tests/e2e.rs");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));

    // 只在 `CASES` 数组范围内数 `Case`（数组外的诊断探针不算）。
    let start = text
        .find("const CASES: &[Case] = &[")
        .expect("找不到 CASES 数组");
    let end = text[start..].find("\n];").expect("CASES 数组没有收尾") + start;
    let cases_src = &text[start..end];
    let cases = cases_src.matches("Case {").count();

    // `known_failure` / `expect_trap`：只看代码行（注释里的历史说明不算）。
    let cases_code = code_lines(cases_src).join("\n");
    let known = cases_code
        .lines()
        .filter(|l| l.contains("known_failure: true"))
        .count();
    let expect_trap = cases_code
        .lines()
        .filter(|l| l.contains("expect_trap: true"))
        .count();

    // `FLAKY` 名单长度（空名单 = 0）。
    let flaky_at = text.find("const FLAKY").expect("找不到 FLAKY 常量");
    let flaky_end = text[flaky_at..].find("];").expect("FLAKY 常量没有收尾") + flaky_at;
    let flaky_src = code_lines(&text[flaky_at..flaky_end]).join("\n");
    let flaky = flaky_src.matches('"').count() / 2;

    let mut m = BTreeMap::new();
    m.insert("cases".to_string(), cases);
    m.insert("hard".to_string(), cases.saturating_sub(known));
    m.insert("flaky".to_string(), flaky);
    m.insert("known".to_string(), known);
    m.insert("expect_trap".to_string(), expect_trap);
    m
}

/// README 机读标记里的计数。
fn readme_counts(root: &Path) -> BTreeMap<String, usize> {
    let path = root.join("crates/tools/forge-rustc/README.md");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
    let at = text.find("计数（机读").unwrap_or_else(|| {
        panic!(
            "README 里找不到机读计数标记（应为 `<!-- 计数（机读，守卫 rustc_doc_counts_guard 钉住）：\
             cases=… hard=… flaky=… known=… expect_trap=… -->`）——\
             改用例数/容忍名单时必须同步它，这正是本守卫的目的"
        )
    });
    let line_end = text[at..].find("-->").map(|i| at + i).unwrap_or(text.len());
    let line = &text[at..line_end];

    let mut m = BTreeMap::new();
    for (i, part) in line.split_whitespace().enumerate() {
        let _ = i;
        if let Some((k, v)) = part.split_once('=')
            // 标记里的人话部分会带全角标点（如 `）：cases=110`）⇒ 键只取 ASCII 标识符部分。
            && let Ok(n) = v.trim_end_matches(|c: char| !c.is_ascii_digit()).parse::<usize>()
        {
            let key: String = k
                .chars()
                .skip_while(|c| !c.is_ascii_alphabetic())
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !key.is_empty() {
                m.insert(key, n);
            }
        }
    }
    m
}

#[test]
fn rustc_doc_counts_guard() {
    let root = repo_root();
    let code = e2e_counts(&root);
    let doc = readme_counts(&root);

    assert!(
        !code.is_empty() && !doc.is_empty(),
        "计数解析失败：code={code:?} doc={doc:?}"
    );

    for (k, v) in &code {
        let dv = doc
            .get(k)
            .unwrap_or_else(|| panic!("README 机读标记缺字段 `{k}`（代码里的实测值 = {v}）"));
        assert_eq!(
            dv, v,
            "README 的 `{k}` = {dv}，而 `tests/e2e.rs` 实测 = {v} —— 文档与代码分叉。\
             改了用例数/容忍名单就同步 README 的机读标记（本守卫就是为这件事存在的）"
        );
    }
    println!("OK  rustc_doc_counts_guard：{code:?}");
}
