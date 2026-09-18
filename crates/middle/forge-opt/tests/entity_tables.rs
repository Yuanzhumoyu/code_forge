//! 句柄键表预算守卫（v3 S2 余项④：优化 pass 侧的密集句柄表）。
//!
//! `Value`/`Inst`/`Block` 都是**密集句柄**（`EntityRef`：下标即句柄），用它们做键的表
//! 不该走 `HashMap`（每次访问一次 SipHash）。本批已迁移：
//!
//! - `scalar/dead_code.rs`：`build_use_counts` → `SecondaryMap<Value, usize>`；
//! - `scalar/copy_prop.rs`：`build_copy_map` → `SecondaryMap<Value, Value>`；
//! - `scalar/cse.rs` / `scalar/gvn.rs`：`replacements` → `SecondaryMap<Value, Value>`；
//! - `ipa/inline.rs`：`repl` → `SecondaryMap<Value, Value>`；
//! - 核心 API：`Function::apply_replacements` 的参数由 `&HashMap` 改 `&SecondaryMap`
//!   （重映射表天然是密集表；4 处调用点同步）。
//!
//! **剩余**的句柄键 `HashMap` 逐文件登记在下面的预算表里（精确相等 + 每条写原因）：
//! 迁移一批就下调一批，新增一处即红。这样"还有多少没迁、为什么没迁"是**可数的**，
//! 而不是靠记忆。
//!
//! 注：本切片**未做** opt 侧 A/B 计时（未宣称 pass 提速）；宣称的是结构一致性
//! （密集句柄不再哈希）与"余量表可数"。

use std::path::{Path, PathBuf};

/// `(文件名, 剩余处数, 为什么还剩这些)`
const BUDGETS: &[(&str, usize, &str)] = &[
    (
        "gvn_pre.rs",
        9,
        "块级数据流表（gen/kill/avail_out：Block → HashSet<ExprId>）——块数远小于值数，收益低；迁它要连 6 处签名一起改，留作后续",
    ),
    (
        "loop_unroll.rs",
        6,
        "循环展开的 val_remap/block_remap 及其两个传参签名（展开副本的临时重映射）",
    ),
    (
        "gvn.rs",
        5,
        "const_map（Value → (Big, TypeId)）与 dom_children（Block → Vec<Block>）及其传参签名",
    ),
    (
        "const_fold.rs",
        5,
        "collect_uses/def_map/known（per-instruction 常量折叠状态）及其传参签名",
    ),
    (
        "sccp.rs",
        4,
        "lattice（Value → LatticeValue）与 collect_all_uses 及其传参签名",
    ),
    (
        "algebraic.rs",
        2,
        "代数化简的 struct 字段 map/bnot_sources（改字段类型要连构造与全部使用点）",
    ),
    (
        "lto.rs",
        1,
        "LTO 内联的 val_remap（与 inline.rs 同形态，跨函数重映射）",
    ),
    ("licm.rs", 1, "LICM 的 val_remap（hoist 后重映射）"),
    (
        "inline.rs",
        1,
        "内联的 value_map（参数/局部值重映射；repl 已迁）",
    ),
    ("func_specialize.rs", 1, "特化副本的 val_remap"),
];

fn src_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn count_handle_hashmaps(path: &Path) -> usize {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));
    ["HashMap<Value", "HashMap<Block", "HashMap<Inst"]
        .iter()
        .map(|pat| text.matches(pat).count())
        .sum()
}

/// 每个文件的句柄键 `HashMap` 处数必须**恰好等于**预算（迁移一批请下调；
/// 新增一处逐次哈希即红）。
#[test]
fn handle_hashmap_budget_matches_measured() {
    let root = src_root();
    assert!(root.is_dir(), "src 目录不存在：{}", root.display());
    let mut checked = 0usize;
    for (file, budget, why) in BUDGETS {
        // src 下的 scoped 子目录：找到该文件（不递归 `**`，直接按已知前缀找）
        let path = find_file(&root, file).unwrap_or_else(|| panic!("找不到 src/**/{file}"));
        let got = count_handle_hashmaps(&path);
        assert_eq!(
            got, *budget,
            "{file} 的句柄键 HashMap 处数变了（实测 {got}，预算 {budget}；{why}）：\n\
             - 新增了？Value/Inst/Block 是密集句柄，请改用 `SecondaryMap`（`forge_ir::entity_map`）；\n\
             - 迁移了一批？请把预算下调到 {got}（预算表要反映现状）。"
        );
        checked += 1;
    }
    assert_eq!(checked, BUDGETS.len(), "预算表必须逐条被检查到");
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(found) = find_file(&p, name) {
                return Some(found);
            }
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    None
}

/// 核心 API 守卫：`apply_replacements` 必须收密集表（回退成 `HashMap` 即红）。
#[test]
fn apply_replacements_takes_a_dense_map() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("foundation/forge-ir/src/function.rs");
    let text =
        std::fs::read_to_string(&root).unwrap_or_else(|e| panic!("读不到 {}：{e}", root.display()));
    assert!(
        text.contains(
            "fn apply_replacements(&mut self, replacements: &SecondaryMap<Value, Value>)"
        ),
        "`Function::apply_replacements` 必须收 `&SecondaryMap<Value, Value>`（重映射表是密集表；\
         v3 S2 余项④）"
    );
    assert!(
        !text.contains("fn apply_replacements(&mut self, replacements: &HashMap<Value, Value>)"),
        "`apply_replacements` 回退成 HashMap 了"
    );
}
