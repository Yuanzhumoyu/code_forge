//! 句柄键表纪律守卫（v3 S2 余项④：代码生成侧的密集句柄表）。
//!
//! 背景：`Value`/`Inst`/`Block`/`VReg` 都是**密集句柄**（`EntityRef`：下标即句柄），
//! 用它们做键的表不该走 `HashMap`（每次访问一次 SipHash）。本切片把 lowering 侧
//! 的六张表换成 `SecondaryMap`：
//!
//! - `LowerCtx::{vreg_classes, vreg_types, vreg_widths}`（VReg 键）；
//! - `CompileState::{value_to_xreg, block_map, alloca_offsets}`（Value/Block/Inst 键）。
//!
//! **`XReg` 不在此列**（实测结论，见 [`xreg_is_index_plus_class_so_not_dense`]）：
//! `XReg { index, class }` 的 `Eq`/`Hash` **含 class**，同一个 index 配不同类是两个
//! 不同的键——按 index 做下标会把它们合并，属语义变化。所以 `xreg_types` 等
//! XReg 键表保持 `HashMap`，直到先决定"class 是否属于键"。
//!
//! A/B（demo8 夹具，`compile_raw`，release，500 次 × 7 轮取中位数；临时探针，跑完即删）：
//! 64 条指令 116.7µs → **103.3µs**（−11.5%，样本区间不重叠）；256 条指令
//! 412.7µs → 400.5µs（−3.0%，尾部与噪声重叠）。

use forge_codegen::prelude::{LowerCtx, RegClass, SecondaryMap, XReg};
use std::collections::HashMap;

/// 编译期守卫：`LowerCtx` 的三张 VReg 表必须是密集表（改了这里就编不过）。
#[test]
fn lowerctx_vreg_tables_are_dense() {
    fn dense<K: forge_ir::entity_map::EntityRef, V>(_: &SecondaryMap<K, V>) {}
    let ctx = LowerCtx::new();
    dense(&ctx.vreg_classes);
    dense(&ctx.vreg_types);
    dense(&ctx.vreg_widths);
}

/// 源码级守卫：`CompileState` 的三张表也必须是密集表（`pub(crate)` 字段，
/// 测试里拿不到值，只能扫源码）。
#[test]
fn compile_state_tables_are_dense() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pipeline/compiler.rs");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
    for field in ["value_to_xreg", "block_map", "alloca_offsets"] {
        assert!(
            text.contains(&format!("{field}: SecondaryMap<")),
            "`{field}` 必须是 `SecondaryMap`（密集句柄表；v3 S2 余项④）"
        );
    }
    for bad in [
        "value_to_xreg: HashMap<",
        "block_map: HashMap<",
        "alloca_offsets: HashMap<",
    ] {
        assert!(
            !text.contains(bad),
            "`{bad}` 回潮了——Value/Block/Inst 是密集句柄，不该走 HashMap"
        );
    }
}

/// `XReg` 的键含 `(index, class)`：同一 index 不同类**不同键**，所以按 index 做
/// 下标的密集表会改变语义——这是 `xreg_types` 等表保持 `HashMap` 的理由。
#[test]
fn xreg_is_index_plus_class_so_not_dense() {
    let a = XReg::new(7, RegClass::GPR(8));
    let b = XReg::new(7, RegClass::GPR(4));
    assert_eq!(a.index(), b.index(), "同一分配下标");
    assert_ne!(a, b, "不同类 ⇒ 不同的 XReg（键含 class）");

    let mut m: HashMap<XReg, i32> = HashMap::new();
    m.insert(a, 1);
    assert_eq!(m.get(&a), Some(&1));
    assert_eq!(
        m.get(&b),
        None,
        "按 (index, class) 区分：换成「下标=index」的密集表就会把两者合并"
    );
}

/// **全仓守卫**：`crates/**` 与根 `src/` 的**代码**里不得再出现以
/// `Value`/`Block`/`Inst` 为键的 `HashMap`（都是密集句柄，必须用 `SecondaryMap`）。
///
/// `XReg` 是唯一豁免：它的键含 `class`（见上一个用例），按 index 密化会改变语义。
/// 注释行（`//`）不计——`use_list.rs` 的文档里提到旧实现用了 `HashMap<Value, _>`。
#[test]
fn no_dense_handle_hashmaps_repo_wide() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..");
    let mut hits: Vec<String> = Vec::new();
    for start in ["crates", "src"] {
        walk_src(&root.join(start), &root, &mut hits);
    }
    assert!(
        hits.is_empty(),
        "以下位置仍以密集句柄为键用 HashMap（应改用 `forge_ir::entity_map::SecondaryMap`；\
         若确需 `XReg` 这类「键含 class」的句柄请在本守卫里写明理由）：\n{}",
        hits.join("\n")
    );
}

fn walk_src(dir: &std::path::Path, root: &std::path::Path, hits: &mut Vec<String>) {
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
        // 只看生产代码（`tests/` 下的夹具不算）
        if p.components().any(|c| c.as_os_str() == "tests") {
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
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if let Some(bad) = dense_handle_hashmap_in(line) {
                hits.push(format!("{rel}:{}: [{bad}] {}", i + 1, line.trim()));
            }
        }
    }
}

/// 行内是否出现"以密集句柄为键的 `HashMap`"（只认**精确**的 `Value`/`Block`/`Inst`，
/// 不误伤 `BlockId`/`ValueId` 这类别的句柄类型）。
fn dense_handle_hashmap_in(line: &str) -> Option<&'static str> {
    const KEYS: [&str; 3] = ["Value", "Block", "Inst"];
    let mut rest = line;
    while let Some(pos) = rest.find("HashMap<") {
        let after = &rest[pos + "HashMap<".len()..];
        let after = after.trim_start();
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
