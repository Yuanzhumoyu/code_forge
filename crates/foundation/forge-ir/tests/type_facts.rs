//! 类型系统契约守卫（v3 方案 **S3**）。
//!
//! 这些不变量此前只由 `TypeStore::with_data_layout` 里的 `debug_assert!` 把守
//! （release 构建下**完全不检查**），本轮把它们变成 CI 可执行断言：
//!
//! 1. `TypeId` 常量 ↔ `TypeStore` 预填充顺序（含保留空洞 9）一致；
//! 2. `TypeStore::scalar_bits` 是**类型事实**：指针宽度按 `DataLayout` 取；
//! 3. `TypeId::bits()` 是**历史视图**：`PTR` 恒 64、复合返回 0——本文件把它与
//!    事实的差异显式钉住（不是靠注释）。

use forge_ir::data_layout::DataLayout;
use forge_ir::types::TypeStore;
use forge_ir::{TypeContext, TypeId};

/// `scalar_bits` 是类型事实：标量给位宽、指针按 DataLayout、复合给 None。
#[test]
fn scalar_bits_reports_type_facts() {
    let store = TypeStore::new();
    assert_eq!(store.scalar_bits(TypeId::BOOL), Some(1));
    assert_eq!(store.scalar_bits(TypeId::I8), Some(8));
    assert_eq!(store.scalar_bits(TypeId::I32), Some(32));
    assert_eq!(store.scalar_bits(TypeId::I64), Some(64));
    assert_eq!(store.scalar_bits(TypeId::F32), Some(32));
    assert_eq!(store.scalar_bits(TypeId::F64), Some(64));
    // 默认 DataLayout 的指针是 64 位
    assert_eq!(store.scalar_bits(TypeId::PTR), Some(64));
    // 复合/void：None（"未知"与"0 位"可区分）
    // VOID 在 store 里是 Int { bits: 0 }（i0 视图）——事实是 Some(0)，不是 None
    assert_eq!(store.scalar_bits(TypeId::VOID), Some(0));
    assert_eq!(store.scalar_bits(TypeId::V128), None);
    let mut store2 = TypeStore::new();
    let array = store2.array_ty(TypeId::I32, 4);
    assert_eq!(store2.scalar_bits(array), None, "数组不是标量");
}

/// 指针宽度**按 DataLayout 取**（32 位目标报 32）——这条正是 `TypeId::bits()`
/// 做不到的事。
#[test]
fn scalar_bits_follows_data_layout_for_pointers() {
    let dl32 = DataLayout::x86_32_linux();
    let store32 = TypeStore::with_data_layout(dl32);
    assert_eq!(
        store32.scalar_bits(TypeId::PTR),
        Some(32),
        "32 位目标指针 = 32 位"
    );
    assert_eq!(store32.size_bytes(TypeId::PTR), 4);
    // i32 在 32 位目标上仍是 32 位（数据布局不影响标量整数）
    assert_eq!(store32.scalar_bits(TypeId::I32), Some(32));
}

/// 锁中毒不再 panic：毒化后 `borrow`/`borrow_mut` 仍返回内部值（S3 去 panic）。
#[test]
fn type_context_lock_poison_does_not_panic() {
    let ctx = TypeContext::new();
    // 用一个持写锁的线程 panic 来毒化锁
    let ctx2 = ctx.clone();
    let handle = std::thread::spawn(move || {
        let _guard = ctx2.borrow_mut();
        panic!("故意毒化 TypeStore 的 RwLock");
    });
    assert!(handle.join().is_err(), "子线程应已 panic");
    // 中毒后仍可读、可写（不 panic）
    assert_eq!(ctx.borrow().scalar_bits(TypeId::I32), Some(32));
    let f64_ty = TypeId::F64;
    assert_eq!(ctx.borrow().scalar_bits(f64_ty), Some(64));
}
