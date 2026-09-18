//! 读路径预算守卫（v3 S3）：逐文件钉住 `X.borrow()` 的处数。
//!
//! 「读路径纪律」的目标形态是**入口取一次锁、把 `&TypeStore` 显式传下去**：
//! 每多一处 `borrow()` 就多一次 `RwLock` 读锁获取，且在循环/每条指令上的取锁
//! 会让代价随输入规模增长（display/verify 两处已用运行时计数守卫钉死，
//! 见 `type_store_read_path.rs`）。
//!
//! 本文件是**剩下的模块**的静态预算：预算 = 当前实测处数，**必须精确相等**
//! （`==` 而非 `<=`）——这样"又加了一处逐次取锁"会立刻红，而"迁移完一批"
//! 也迫使作者把预算下调（防止预算变成摆设）。
//!
//! 每条的"为什么还剩这些"都必须写清，否则下一个人无法判断能不能继续降：
//!
//! - `display.rs`：2 处是入口（`Module::fmt` / `function_to_string`），其余 12 处在
//!   `#[cfg(test)] mod tests` 的测试辅助函数里（每个测试各取一次，不影响生产路径）。
//! - `verify.rs`：只剩 `verify()` 入口 1 处。
//! - `semantics.rs`：24 处都在**同时 intern 的函数**里（`to_type`/`build_inst`/
//!   `operand_to_value` 等有 `borrow_mut`）——读写交错，不能持有长读锁（`RwLock`
//!   不可重入 ⇒ 自锁死），只能保留逐段取锁。
//! - `types.rs`：13 处是 `TypeContext` 的**一次性查询封装**（`ctx.size_bytes(ty)`
//!   这类），每次调用恰好 1 次读锁；多查询调用方应改用 `&TypeStore`（见
//!   `type_store_read_path.rs` 的 `one_shot_type_context_accessors_take_one_lock`）。
//! - `pipeline/compiler.rs`：20 处在生产路径——各 `expand_*`/`memoryize_*` 入口一次取锁，
//!   其余在 `rewrite_agg_value_uses`/`expand_large_aggs`/`expand_large_agg_params`/
//!   `compile_with_alloc` 里（**显式 `drop` 守卫**的读写交错段，或会调到唯一
//!   `borrow_mut` 的编排点）；另有 1 处在 `#[cfg(test)]` 测试里（给 `LowerCtx` 取快照）。
//!   v3 S3 余项①顺带收掉了宽向量门的"每指令取锁"（3 处 → 1 次入口快照）。
//! - `forge-dsl/.../lowering.rs`：**0**（v3 S3 余项①已迁移）。这 11 处原本是
//!   `quote! { ... }` 模板文本里的 `tc.borrow()`（生成的 lowering 在 codegen 期逐指令
//!   取锁）；现在 `LowerCtx` 带**快照**（`type_store: Option<TypeStoreRef>`，lowering
//!   入口取一次），模板改读快照。预算留 0 作为**回潮守卫**：模板里再出现逐次取锁即红。
#![cfg(debug_assertions)]

use std::path::PathBuf;

/// `(仓库相对路径, 预算处数, 为什么还剩这些)`
const BUDGETS: &[(&str, usize, &str)] = &[
    (
        "crates/foundation/forge-ir/src/display.rs",
        14,
        "2 处入口 + 12 处测试辅助",
    ),
    (
        "crates/foundation/forge-ir/src/verify.rs",
        1,
        "只剩 verify() 入口",
    ),
    (
        "crates/foundation/forge-ir/src/ir_parser/semantics.rs",
        15,
        "4 个纯读助手（pack_*_init / int_init_bytes / float_init_bytes）各一次 + \
         `to_type*` 的命名查询 + `build_inst` 的 3 处单点读 + 3 个只读助手；\
         读数段已按 v3 S3 余项② 合并（operand_to_value 9 → 1、extractvalue 索引链 1、\
         vconst/agg_const 各 1）",
    ),
    (
        "crates/foundation/forge-ir/src/types.rs",
        13,
        "TypeContext 一次性查询封装（每次调用恰好 1 次）",
    ),
    (
        "crates/backend/forge-codegen/src/pipeline/compiler.rs",
        21,
        "20 处生产路径（各 expand_*/memoryize_* 入口 + 读写交错短读段）+ 1 处测试辅助",
    ),
    (
        "crates/frontend/forge-dsl/src/v12/codegen/lowering.rs",
        0,
        "已迁移到快照（LowerCtx::type_store）；0 = 回潮守卫",
    ),
];

fn repo_root() -> PathBuf {
    // <repo>/crates/foundation/forge-ir → <repo>
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

fn count_borrows(path: &std::path::Path) -> usize {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));
    text.matches(".borrow()").count()
}

/// 每个文件的 `borrow()` 处数必须**恰好等于**预算（多一处逐次取锁即红；
/// 迁移完请下调预算——预算是纪律，不是上限装饰）。
#[test]
fn borrow_site_budget_matches_measured() {
    let root = repo_root();
    assert!(
        root.join("crates").is_dir(),
        "仓库根解析错误：{}",
        root.display()
    );
    let mut checked = 0usize;
    for (rel, budget, why) in BUDGETS {
        let path = root.join(rel);
        let got = count_borrows(&path);
        assert_eq!(
            got, *budget,
            "{rel} 的 `borrow()` 处数变了（实测 {got}，预算 {budget}；{why}）：\n\
             - 新增了逐次取锁？请改成入口取一次、把 `&TypeStore` 传下去；\n\
             - 迁移完成了一批？请把预算下调到 {got}。"
        );
        checked += 1;
    }
    assert_eq!(checked, BUDGETS.len(), "预算表必须逐条被检查到");
}

/// 预算表不得含已不存在的文件（防"表还在、文件走了"的腐烂）。
#[test]
fn budget_entries_point_at_existing_files() {
    let root = repo_root();
    for (rel, _, _) in BUDGETS {
        assert!(
            root.join(rel).is_file(),
            "预算表里的 {rel} 不存在（文件改名/移动后请更新本表）"
        );
    }
}
