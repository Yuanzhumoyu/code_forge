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
