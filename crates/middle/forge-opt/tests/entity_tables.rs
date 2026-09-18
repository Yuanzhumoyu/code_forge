//! 句柄键表纪律守卫（v3 S2 余项④：优化 pass 侧与全仓的密集句柄表）。
//!
//! `Value`/`Inst`/`Block` 都是**密集句柄**（`EntityRef`：下标即句柄），用它们做键的表
//! 不该走 `HashMap`（每次访问一次 SipHash）。本项分两批完成：
//!
//! **第一批**（重映射表 / use-count 表）：`scalar/dead_code.rs::build_use_counts`、
//! `scalar/copy_prop.rs::build_copy_map`、`scalar/cse.rs`/`scalar/gvn.rs` 的
//! `replacements`、`ipa/inline.rs::repl`，以及核心 API
//! `Function::apply_replacements` 改收 `&SecondaryMap<Value, Value>`。
//!
//! **第二批**（余下全部）：`scalar/{const_fold,sccp,gvn,gvn_pre}.rs`、
//! `loops/{licm,loop_unroll}.rs`、`advanced/algebraic.rs`、`ipa/{lto,func_specialize,inline}.rs`，
//! 并顺带清掉 `forge-ir`（`analysis.rs` 的 `postorder_rank`/`preds_map`、`loop_info.rs`、
//! `ir_parser/semantics.rs::per_pred`）与 `forge-codegen`（`agg_expand::AggSlots`、
//! `compiler::rewrite`、`liverange`、`lowering::roots`）里的同类表。为此给
//! `SecondaryMap` 补了 `FromIterator<(K, V)>`（与 `HashMap::collect()` 同形）。
//!
//! **`XReg` 是唯一豁免**：`XReg { index, class }` 的 `Eq`/`Hash` **含 class**，同一
//! index 配不同类是两个不同的键；按 index 做下标的密集表会把它们合并（语义变化）。
//! 见 `crates/backend/forge-codegen/tests/entity_tables.rs::xreg_is_index_plus_class_so_not_dense`。
#![cfg(debug_assertions)]

use std::path::{Path, PathBuf};

/// 全仓扫描：`crates/**` 与根 `src/` 的**代码**里不得再有以 `Value`/`Block`/`Inst`
/// 为键的 `HashMap`（注释行不计；`BlockId`/`ValueId` 这类别的句柄类型不误伤）。
#[test]
fn no_dense_handle_hashmaps_in_forge_opt() {
    let root = repo_root();
    let mut hits: Vec<String> = Vec::new();
    walk_src(&root.join("crates/middle/forge-opt/src"), &root, &mut hits);
    assert!(
        hits.is_empty(),
        "forge-opt 里仍以密集句柄为键用 HashMap（请改用 `forge_ir::entity_map::SecondaryMap`）：\n{}",
        hits.join("\n")
    );
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

fn walk_src(dir: &Path, root: &Path, hits: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_src(&p, root, hits);
            continue;
        }
        if p.extension().is_none_or(|x| x != "rs") {
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
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if let Some(bad) = dense_handle_hashmap_in(line) {
                hits.push(format!("{rel}:{}: [{bad}] {}", i + 1, line.trim()));
            }
        }
    }
}

/// 只认**精确**的 `Value`/`Block`/`Inst` 键（不误伤 `BlockId` 等）。
fn dense_handle_hashmap_in(line: &str) -> Option<&'static str> {
    const KEYS: [&str; 3] = ["Value", "Block", "Inst"];
    let mut rest = line;
    while let Some(pos) = rest.find("HashMap<") {
        let after = rest[pos + "HashMap<".len()..].trim_start();
        if let Some(key) = KEYS.iter().find(|k| {
            after.strip_prefix(**k).is_some_and(|r| {
                r.starts_with(',')
                    || r.starts_with('>')
                    || r.starts_with(" ,")
                    || r.starts_with(" >")
            })
        }) {
            return Some(key);
        }
        rest = after;
    }
    None
}

/// 核心 API 守卫：`apply_replacements` 必须收密集表（回退成 `HashMap` 即红）。
#[test]
fn apply_replacements_takes_a_dense_map() {
    let path = repo_root().join("crates/foundation/forge-ir/src/function.rs");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
    assert!(
        text.contains(
            "fn apply_replacements(&mut self, replacements: &SecondaryMap<Value, Value>)"
        ),
        "`Function::apply_replacements` 必须收 `&SecondaryMap<Value, Value>`（重映射表是密集表）"
    );
    assert!(
        !text.contains("fn apply_replacements(&mut self, replacements: &HashMap<Value, Value>)"),
        "`apply_replacements` 回退成 HashMap 了"
    );
}

/// 核心 API 守卫：`clone_inst` 的 `value_remap` 同样必须是密集表。
#[test]
fn clone_inst_takes_a_dense_remap() {
    let path = repo_root().join("crates/foundation/forge-ir/src/dfg.rs");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
    assert!(
        text.contains("value_remap: &mut SecondaryMap<Value, Value>,"),
        "`DataFlowGraph::clone_inst` 的 `value_remap` 必须是 `&mut SecondaryMap<Value, Value>`"
    );
}
