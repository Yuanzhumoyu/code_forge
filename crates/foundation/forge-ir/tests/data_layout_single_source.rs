//! `DataLayout` 单一数据源守卫（v3 S3：`set_data_layout` 不再重建类型存储）。
//!
//! 旧实现：`Module::set_data_layout` 直接 `self.types = TypeContext::with_data_layout(..)`
//! ——**换掉整个类型存储**，于是
//!
//! - 已 intern 的签名/结构类型**全部丢失**（文档里那句"必须在 `add_function`
//!   之前调用（重建会清空已注册签名）"就是这条约束的自我描述）；
//! - 在改布局之前构建的 `Function`/builder 持有的是**旧存储**的 `Arc` 克隆 ⇒
//!   同一模块内"模块看到的布局"与"函数看到的布局"分叉（指针宽度/大小/对齐
//!   各算各的，静默错值）。
//!
//! 新实现：布局是共享存储里的一个字段，`set_data_layout` **原地更新**
//! （`TypeStore::set_data_layout`）⇒ 所有 `TypeContext` 克隆（含既有函数）立即
//! 看到新布局，已 intern 的类型与签名原样保留，`Module::data_layout()` 读的就是
//! 存储里的那一份（无第二个副本可漂移）。

use forge_ir::Module;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::data_layout::DataLayout;
use forge_ir::ir::types::FunctionSignature;

const DL32: &str = "e-m:e-p:32:32-i64:64-n8:16:32";
const DL64: &str = "e-m:e-p:64:64-i64:64-n8:16:32:64";

fn dl32() -> DataLayout {
    DataLayout::parse(DL32).expect("parse p:32")
}

fn dl64() -> DataLayout {
    DataLayout::parse(DL64).expect("parse p:64")
}

/// `DataLayout` 未实现 `PartialEq`：按 Debug 文本比较（同一份字符串解析出的布局）。
fn dl_eq(a: &DataLayout, b: &DataLayout) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

/// ① 改布局不得丢已 intern 的类型与签名（旧实现整表重建 ⇒ 全丢）。
#[test]
fn set_data_layout_preserves_interned_types_and_signatures() {
    let mut m = Module::new();
    let i32_ty = m.types.i32_ty();
    let sig = FunctionSignature::new(&[(i32_ty, "x")], &[i32_ty]);
    let sr = m.types.register_signature(sig.clone());
    let vec_ty = m.types.borrow_mut().vector_ty(i32_ty, 4);
    assert_eq!(
        m.types.borrow().size_bytes(m.types.ptr_ty()),
        8,
        "默认 p:64"
    );

    m.set_data_layout(dl32());

    let store = m.types.borrow();
    assert_eq!(
        format!("{:?}", store.get_signature(sr)),
        format!("{sig:?}"),
        "改布局不该清空已注册签名（旧实现重建存储 ⇒ 这里拿不到/panic）"
    );
    assert_eq!(
        store.size_bytes(vec_ty),
        16,
        "改布局前 intern 的向量类型必须仍然有效"
    );
    assert_eq!(
        store.size_bytes(m.types.ptr_ty()),
        4,
        "布局原地更新后指针宽度必须立即生效"
    );
}

/// ② 共享同一存储的所有 `TypeContext` 克隆（含改布局之前构建的函数）都看到新布局。
#[test]
fn existing_function_sees_layout_update() {
    let mut m = Module::new();
    let ctx = m.types.clone();
    let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
    let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
    let (_entry, _) = fb.create_entry_block();
    let v = fb.iconst_i32(7);
    fb.ret(&[v]);
    let func = fb.finish().expect("build");

    // 构建之后才设布局（旧实现下函数仍绑旧存储 ⇒ 仍是 p:64 = 8）
    m.set_data_layout(dl32());

    assert_eq!(
        func.types.borrow().size_bytes(func.types.ptr_ty()),
        4,
        "既有函数必须看到模块的布局更新（旧实现：模块/函数两套存储 ⇒ 8）"
    );
    assert_eq!(
        ctx.borrow().size_bytes(ctx.ptr_ty()),
        4,
        "所有克隆都指向同一存储"
    );
    assert_eq!(m.types.borrow().size_bytes(m.types.ptr_ty()), 4);
}

/// ③ 类型 id 只由结构决定：改布局不会让同一个向量类型分叉成两个 id。
#[test]
fn layout_change_does_not_fork_type_identity() {
    let mut m = Module::new();
    let i32_ty = m.types.i32_ty();
    let v1 = m.types.borrow_mut().vector_ty(i32_ty, 4);
    m.set_data_layout(dl32());
    let v2 = m.types.borrow_mut().vector_ty(i32_ty, 4);
    assert_eq!(v1, v2, "去重键不含 DataLayout（宽度是查询期算的）");
    assert_eq!(m.types.borrow().size_bytes(v1), 16);
}

/// ④ `Module` 的布局与存储里的布局是同一份（单一数据源，无副本可漂移）。
#[test]
fn module_data_layout_is_the_store_layout() {
    let mut m = Module::new();
    m.set_data_layout(dl32());
    assert!(
        dl_eq(&m.data_layout(), &dl32()),
        "Module 暴露的布局必须等于刚设的那份"
    );
    assert!(
        dl_eq(&m.data_layout(), &m.types.borrow().data_layout),
        "Module 的布局字段与存储内嵌布局必须一致（单一数据源）"
    );

    // 再来一次：换回 p:64，两侧同步
    m.set_data_layout(dl64());
    assert!(dl_eq(&m.data_layout(), &dl64()));
    assert_eq!(m.types.borrow().size_bytes(m.types.ptr_ty()), 8);
}
