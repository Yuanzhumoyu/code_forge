//! 句柄字段私有化的守卫（v3 方案 S2：句柄字段私有化）。
//!
//! 裸 u32 句柄（`Value`/`Inst`/`Block`/`TypeId`/`FuncRef`/`ConstId`/`AggId`/
//! `GlobalId`/`SigRef`/`VReg`）此前字段是 `pub u32`：crate 外可以 `Value(999)`
//! 凭空造句柄（把"坏句柄"从构造点泄漏到整个下游），也可以 `v.0` 直读索引
//! （让句柄的**表示**成为公开契约，表示一改就全仓编译失败）。
//!
//! 现在字段是 `pub(crate)`，唯一外部出入口是：
//!
//! | 句柄 | 构造 | 读索引 |
//! | --- | --- | --- |
//! | `Value`/`Inst`/`Block`/`TypeId`/`FuncRef`/`GlobalId`/`SigRef`/`AggId`/`VReg` | `::new(u32)` | `.index()` |
//! | `ConstId` | `::from_raw(u32)` / `::pack(tag, index)` | `.raw()` / `.index()`（低 30 位池内索引）/ `.tag()` |
//!
//! **注意 `ConstId` 的语义**：`index()` 早于本步就存在，含义是"低 30 位池内索引"，
//! 与 `raw()`（含 tag 位的打包值）不同——所以它的构造口叫 `from_raw` 而不是 `new`。
//!
//! 迁移面（编译器逐条点名，非文本猜测）：本仓 178 + 45 处编译错误，含
//! ① 构造点 `X(n)` → `X::new(n)`；② 索引读 `x.0` → `x.index()`；
//! ③ **DSL 生成器模板**（`forge-dsl/src/v12/codegen/{lowering,placeholder,machine,integration}.rs`
//!    里的 `quote!` 文本，32 处）；④ `forge-rustc`（本机不可编译，改为文本审计 +
//!    定点修补：`FuncRef`/`GlobalId`/`Block` 构造与 `block_id.0`）。

use forge_ir::{AggId, Block, ConstId, FuncRef, GlobalId, Inst, SigRef, TypeId, VReg, Value};

/// 9 个"裸 u32"句柄：`new`/`index` 往返、`Default` 为零、Display/Debug 不变。
#[test]
fn plain_handles_roundtrip_new_and_index() {
    macro_rules! check {
        ($($t:ty),* $(,)?) => {$({
            let h = <$t>::new(7);
            assert_eq!(h.index(), 7, "{} new/index 往返", stringify!($t));
            // Copy/Eq/Hash 语义不变（句柄可作为密集表键）
            let h2 = h;
            assert_eq!(h, h2);
            assert_eq!(format!("{:?}", h).is_empty(), false, "Debug 可用");
        })*};
    }
    check!(
        Value, Inst, Block, TypeId, FuncRef, GlobalId, SigRef, AggId, VReg
    );

    // `Default` = 0（`AggId` 无此 derive——它只用于聚合常量池句柄）
    macro_rules! check_default {
        ($($t:ty),* $(,)?) => {$({
            assert_eq!(<$t>::default().index(), 0, "{} Default = 0", stringify!($t));
        })*};
    }
    check_default!(Value, Inst, Block, TypeId, FuncRef, GlobalId, SigRef, VReg);
}

/// `ConstId`：打包值（`raw`）与池内索引（`index`）语义不同，构造口是 `from_raw`。
#[test]
fn const_id_raw_vs_pool_index() {
    let packed = ConstId::pack(ConstId::TAG_BIG, 7);
    assert_eq!(packed.raw(), packed_from_parts(ConstId::TAG_BIG, 7));
    assert_eq!(packed.index(), 7, "index() = 低 30 位池内索引");
    assert_eq!(packed.tag(), ConstId::TAG_BIG);

    // from_raw 与 raw 往返（解析器/常量池恢复句柄的路径）
    assert_eq!(ConstId::from_raw(0xDEAD_BEEF).raw(), 0xDEAD_BEEF);
    // 大索引不被截断（30 位上限内的最大索引）
    let big = ConstId::pack(ConstId::TAG_VEC, (1 << 30) - 1);
    assert_eq!(big.index(), (1 << 30) - 1);
    assert_eq!(big.tag(), ConstId::TAG_VEC);
}

fn packed_from_parts(tag: u32, index: u32) -> u32 {
    ((tag & 0x3) << 30) | (index & ((1 << 30) - 1))
}

/// 句柄仍可经 `Display` 打印（表示私有化不改文本契约）。
#[test]
fn handle_display_is_unchanged() {
    assert_eq!(format!("{}", Value::new(3)), "v3");
    assert_eq!(format!("{}", Block::new(2)), "b2");
    assert_eq!(format!("{}", VReg::new(5)), "%5");
}

/// 源码断言：`entity.rs` 里不得再有 `pub u32` 句柄字段（字段必须是 `pub(crate)`）。
///
/// 编译器已经挡住 crate 外的构造/读索引，这条守卫挡的是**回潮**：把字段改回
/// `pub u32` 能让全仓重新出现"凭空造句柄 + 表示泄漏"，而且不会有任何测试失败。
#[test]
fn no_public_handle_field_in_entity_rs() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("entity.rs");
    let text = std::fs::read_to_string(&path).expect("读 entity.rs");
    const HANDLES: &[&str] = &[
        "Value", "Inst", "Block", "TypeId", "FuncRef", "ConstId", "AggId", "GlobalId", "SigRef",
        "VReg",
    ];
    let mut violations = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.starts_with("//") {
            continue;
        }
        // 只认这 10 个句柄的**元组结构体**声明（`PReg.num` 等其它类型不在其列）
        if let Some(rest) = t.strip_prefix("pub struct ")
            && let Some(name) = rest.split('(').next()
            && HANDLES.contains(&name.trim())
            && !t.contains("pub(crate) u32")
        {
            violations.push(format!("entity.rs:{}: {t}", i + 1));
        }
    }
    assert!(
        violations.is_empty(),
        "句柄字段必须私有（`pub(crate) u32`）；crate 外经 `::new`/`.index()` \
         （ConstId 为 `::from_raw`/`.raw()`）：\n{}",
        violations.join("\n")
    );
}
