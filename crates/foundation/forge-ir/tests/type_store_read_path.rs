//! 类型读路径纪律守卫（v3 S3："`TypeStore` 显式传参 / 读一次锁"）。
//!
//! 背景：`TypeContext::borrow()` 每次都是一次 `RwLock` 读锁获取，而 `RwLock`
//! **不可重入**——读锁存活期间再调 `borrow_mut()`（intern 新类型）会自锁死。
//! 所以多查询路径（display/verify/lowering）的正确形态是"**入口取一次锁，把
//! `&TypeStore` 显式传下去**"：既避免重复取锁，也让"这个作用域只读"落在类型上
//! （拿不到 `&TypeStore` 就写不出 intern 调用）。
//!
//! 改前实测：`display.rs` 有 51 处 `borrow()`（`value_as_literal` 每个值一次、
//! `NameResolver::new` 每个块/值各一次）——打印一个模块的取锁次数与函数体大小
//! 成正比。改后非测试路径只剩 2 处入口取锁。
//!
//! 本文件用 `TypeContext::debug_read_count`（仅 `debug_assertions`）把这条纪律
//! 钉成可测不变量：**打印一个模块 / 一个函数各只取 1 次读锁**。
#![cfg(debug_assertions)]

use forge_ir::builder::FunctionBuilder;
use forge_ir::display::function_to_string;
use forge_ir::types::{FunctionSignature, TypeContext};
use forge_ir::{Module, TypeId};

/// 一个内容较"重"的模块：参数化入口块 + 第二个块 + 整型/浮点/向量常量——
/// 覆盖 display 里会查类型的那些路径（值字面量、块参数、签名、函数头）。
fn rich_module() -> (Module, TypeContext) {
    let mut m = Module::new();
    let ctx = m.types.clone();
    let i32_ty = ctx.i32_ty();
    let v4f32 = ctx.borrow_mut().vector_ty(ctx.f32_ty(), 4);

    let sig = FunctionSignature::new(&[(i32_ty, "x")], &[i32_ty]);
    let mut fb = FunctionBuilder::new("rich", ctx.clone(), sig);
    let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "p")]);
    fb.switch_to_block(entry);
    let a = fb.iadd(params[0], params[0]);
    let f = fb.fconst_f32(1.5);
    let v = fb.vconst_bytes(vec![0u8; 16], v4f32);
    let cond = fb.iconst_bool(true);
    let body = fb.create_block();
    fb.branch(cond, body, &[], entry, &[]);
    fb.switch_to_block(body);
    let r = fb.iadd(a, a);
    fb.ret(&[r]);
    let func = fb.finish().expect("build");
    m.add_function(func);
    // 让浮点/向量常量都被引用上（display 会走它们的字面量路径）
    let _ = (f, v);
    (m, ctx)
}

/// 打印整个模块只取 1 次读锁（改前与函数体大小成正比）。
#[test]
fn module_display_takes_one_read_lock() {
    let (m, ctx) = rich_module();
    ctx.debug_reset_read_count();
    let text = format!("{m}");
    let reads = ctx.debug_read_count();
    assert!(
        text.contains("rich"),
        "模块文本应包含函数（实际输出：{text}）"
    );
    assert_eq!(
        reads,
        1,
        "打印模块必须只取 1 次类型读锁（实际 {reads} 次；文本 {} 字节）",
        text.len()
    );
}

/// 单函数 dump（`function_to_string`）同样只取 1 次。
#[test]
fn function_display_takes_one_read_lock() {
    let (m, ctx) = rich_module();
    let func = m.iter_functions().next().expect("一个函数");
    ctx.debug_reset_read_count();
    let text = function_to_string(func);
    let reads = ctx.debug_read_count();
    assert!(text.contains("define"), "单函数文本应含 define");
    assert_eq!(reads, 1, "function_to_string 必须只取 1 次类型读锁");
}

/// 重复打印：每次打印各取 1 次（不共享、不泄漏、不累积）。
#[test]
fn repeated_display_keeps_the_budget() {
    let (m, ctx) = rich_module();
    ctx.debug_reset_read_count();
    for _ in 0..3 {
        let _ = format!("{m}");
    }
    assert_eq!(
        ctx.debug_read_count(),
        3,
        "3 次打印 = 3 次取锁（每次打印独立取一次）"
    );
}
