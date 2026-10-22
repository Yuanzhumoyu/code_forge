//! **管线规模棘轮 + 分层守卫**（v25 M4）。
//!
//! ## 为什么
//!
//! 立项证据（v25 方案 §1 E7）：`compiler.rs` 1616 行、`regalloc_bt.rs` 1544、`va_expand.rs` 1113、
//! `agg_call.rs` 690——**读改成本高、评审难**。M4 按职责把它们切成目录（模块名不变 ⇒ 调用点零改动），
//! 并把"多大算大"变成**可执行的数字**：
//!
//! 1. `file_size_ratchet`：`src/pipeline/**` 的每个文件都必须 ≤ 签入的上限（缺省 700 行）；
//!    上限**只许按实测下调**（与 asm 计数棘轮同一纪律）——要放行更大的文件必须显式改表并给出理由；
//! 2. `leaf_modules_do_not_depend_on_the_driver`：`legalize/**`、`va_expand/**`、`regalloc_bt/**`
//!    是**叶子**（它们只提供 pass / 展开 / 分配），不得依赖编译会话 `compiler`/`session`。
//!    这条把"调用点抄次序/抄状态"的旧毛病挡在门外（v23 M6 起 legalize 就不再依赖 `compiler.rs`）。

use std::collections::{BTreeMap, BTreeSet};

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
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

/// 超过缺省上限的文件（相对 `crates/backend/forge-codegen/src/`）：实测值即上限。
///
/// 纪律：**只许下调**。要把某条上限调高，必须在这里写清"为什么这个文件必须长"。
const BUDGETS: [(&str, usize); 6] = [
    // 分配器状态机：`BtState` 的 13 个字段 + 17 个方法构成一台状态机，切开会把
    // "谁改了哪个字段"拆散（M4 只切出 spill/tests；再切需先把状态字段收敛成内聚子结构）。
    // 944 = 切分后实测（比初测 +3：跨模块可见性把 13 字段 / 17 方法标成 `pub(super)`）。
    ("pipeline/regalloc_bt/state.rs", 944),
    // 生成物 lowering 的宿主侧辅助（与谱面规则一一对应）。
    ("pipeline/lowering.rs", 835),
    // 物化相：按指令序单遍改写，条目按 IR 形态分段，切开反而难对照。
    ("pipeline/legalize/materialize.rs", 657),
    // va 展开主体：`expand_start` + `expand_arg` 共享局部不变量（游标推进次序）。
    ("pipeline/va_expand/expand.rs", 621),
    // 编译会话入口 + 模块编排（M4 已切出 state 与测试）。
    ("pipeline/compiler/mod.rs", 615),
    // 会话测试体（613 行用例）：测试长不构成阅读负担的同类问题，且与实现同源。
    // 栈探测的正反两面用例（v27 M6-78：大帧必须发 `mov rax,size; call __chkstk`，
    // 小帧不得发）——同一形状的编译级断言放这里最省重复。
    ("pipeline/compiler/alloc_integration_tests.rs", 690),
];

/// 缺省上限：新文件超过它就必须进 [`BUDGETS`] 并写明理由。
const DEFAULT_MAX: usize = 700;

/// **规模棘轮**：每个文件 ≤ 签入上限（只许下调）。
#[test]
fn file_size_ratchet() {
    let root = repo_root();
    let base = root.join("crates/backend/forge-codegen/src");
    let mut files = Vec::new();
    rs_files(&base.join("pipeline"), &mut files);

    let table: BTreeMap<&str, usize> = BUDGETS.into_iter().collect();
    let mut over: Vec<String> = Vec::new();
    let mut hit: BTreeMap<&str, bool> = table.keys().map(|k| (*k, false)).collect();

    for p in &files {
        let rel = p
            .strip_prefix(&base)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/");
        let n = std::fs::read_to_string(p)
            .map(|t| t.lines().count())
            .unwrap_or(0);
        let cap = table.get(rel.as_str()).copied().unwrap_or(DEFAULT_MAX);
        if let Some(flag) = hit.get_mut(rel.as_str()) {
            *flag = true;
        }
        if n > cap {
            over.push(format!("{rel}: {n} 行 > 上限 {cap}"));
        }
    }
    assert!(
        over.is_empty(),
        "管线文件超出规模棘轮（要放行就进 BUDGETS 并写明理由；只许按实测下调）：\n{}",
        over.join("\n")
    );
    // 白名单条目必须被命中（死条目说明文件没了，规则已不适用）。
    let dead: Vec<&str> = hit
        .iter()
        .filter(|(_, seen)| !**seen)
        .map(|(k, _)| *k)
        .collect();
    assert!(
        dead.is_empty(),
        "BUDGETS 里的死条目（文件已不存在）：{dead:?}"
    );
}

/// **叶子模块不得依赖编译会话**：`legalize/**`、`va_expand/**`、`regalloc_bt/**` 只能被
/// 驱动调用，不得反向依赖 `compiler`/`session`。
///
/// 为什么值钱：v23 之前合法化把"次序/状态"抄在调用点旁边，于是 pass 次序变成隐式契约
/// （v23 收尾的四个缺陷同源）。叶子纪律把"谁能决定次序"钉在一处。
#[test]
fn leaf_modules_do_not_depend_on_the_driver() {
    let root = repo_root();
    let base = root.join("crates/backend/forge-codegen/src/pipeline");
    let mut hits: Vec<String> = Vec::new();
    for leaf in ["legalize", "va_expand", "regalloc_bt"] {
        let mut files = Vec::new();
        rs_files(&base.join(leaf), &mut files);
        for p in files {
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            for (i, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if line.contains("crate::pipeline::compiler")
                    || line.contains("crate::pipeline::session")
                    || line.contains("crate::CompileSession")
                {
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
        "叶子模块反向依赖了编译会话（次序/状态应只由驱动持有）：\n{}",
        hits.join("\n")
    );
}

/// 取某小节里"第一列是反引号内容"的表格行（`| `x` | y |`）。
fn doc_rows(doc: &str, heading_contains: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in doc.lines() {
        if line.starts_with("## ") {
            inside = line.contains(heading_contains);
            continue;
        }
        if !inside {
            continue;
        }
        let l = line.trim();
        if !l.starts_with("| `") {
            continue;
        }
        let cells: Vec<String> = l
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim().trim_matches('`').to_string())
            .collect();
        out.push(cells);
    }
    out
}

/// **规模棘轮与契约文档双向相等**（v25 M5）：文档的"规模棘轮"表就是 [`BUDGETS`]，
/// "哪些目录切过"与树上的目录一致——改其一不改另一个就红。
#[test]
fn ratchet_table_matches_doc() {
    let root = repo_root();
    let doc_path = root.join("docs/reference/backend-pipeline.md");
    let doc = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("读不到 {}：{e}", doc_path.display()));

    // 规模表： (文件, 上限)
    let doc_budgets: BTreeMap<String, usize> = doc_rows(&doc, "规模棘轮")
        .into_iter()
        .filter(|r| r.len() >= 2 && r[1].chars().all(|c| c.is_ascii_digit()) && !r[1].is_empty())
        .map(|r| (r[0].clone(), r[1].parse().expect("上限是数字")))
        .collect();
    let code_budgets: BTreeMap<String, usize> = BUDGETS
        .iter()
        .map(|(f, n)| ((*f).to_string(), *n))
        .collect();
    assert_eq!(
        doc_budgets, code_budgets,
        "契约文档的规模棘轮表与 `BUDGETS` 不一致（改其一必须改另一个）"
    );
    assert_eq!(
        code_budgets.len(),
        BUDGETS.len(),
        "BUDGETS 里有重复文件条目"
    );

    // 目录表：切过的四个目录（文档里写成 `pipeline/xxx/`）
    let doc_dirs: BTreeSet<String> = doc_rows(&doc, "模块边界与规模棘轮")
        .into_iter()
        .map(|r| r[0].clone())
        .filter(|p| p.starts_with("pipeline/") && p.ends_with('/'))
        .collect();
    let want_dirs: BTreeSet<String> = [
        "pipeline/compiler/",
        "pipeline/regalloc_bt/",
        "pipeline/va_expand/",
        "pipeline/legalize/agg_call/",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    assert_eq!(doc_dirs, want_dirs, "契约文档的模块目录表与切分现状不符");
    // 这些目录在树上必须真的存在（死条目也红）
    for d in &want_dirs {
        let abs = root.join("crates/backend/forge-codegen/src").join(d);
        assert!(abs.is_dir(), "文档列出的目录不存在：{}", abs.display());
    }
}
