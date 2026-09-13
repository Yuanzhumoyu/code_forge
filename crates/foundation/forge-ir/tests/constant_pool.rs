//! 常量池与浮点宽度守卫（S0 止血回归）。
//!
//! 背景（2026-09-14 审计）：
//! 1. `float_consts: Vec<u128>` 不记位宽 → f32 `1.5`（0x3FC0_0000）与位模式相同的
//!    f64 去重成**同一个 `ConstId`**，而 phi 打印路径恒按 `f64::from_bits` 还原
//!    → f32 常量作为 phi 入边被打印成错误的十进制值；
//! 2. `#[derive(Default)]` 绕过 `new()` 的 bool 预置槽，而 `bool_const` 直接构造
//!    `ConstId(index 0|1)` → `ConstantPool::default()` 造出的池里这两个 id 悬空；
//! 3. `total_len()/is_empty()` 漏了聚合池。

use forge_ir::{AggChild, ConstantPool, FunctionBuilder, FunctionSignature, Module, TypeId};

/// 位宽必须进去重键：同一位模式、不同精度 = 两个常量（不再合并）。
#[test]
fn float_width_is_part_of_dedup_key() {
    let mut pool = ConstantPool::new();
    let f32_bits = 0x3FC0_0000u128; // f32 1.5 的位模式
    let as_f32 = pool.insert_float_typed(f32_bits, 32);
    let as_f64 = pool.insert_float_typed(f32_bits, 64);
    assert_ne!(
        as_f32, as_f64,
        "同一位模式不同值宽必须是两个 ConstId（历史实现合并成一个）"
    );
    assert_eq!(pool.get_float_width(as_f32), Some(32));
    assert_eq!(pool.get_float_width(as_f64), Some(64));
    // 位模式本身不变（宽度是元数据，不做掩码/截断）
    assert_eq!(pool.get_float128(as_f32), Some(f32_bits));
    assert_eq!(pool.get_float128(as_f64), Some(f32_bits));

    // 同 (bits, width) 才去重
    let again = pool.insert_float_typed(f32_bits, 32);
    assert_eq!(again, as_f32, "同 (bits,width) 必须去重");
    assert_eq!(
        pool.total_len(),
        4,
        "bool 预置 2 项 + f32/f64 各 1 项（同键不再新增）"
    );
}

/// 旧入口语义不变：`insert_float` = 64 位、`insert_float128` = 128 位。
#[test]
fn legacy_inserters_record_their_width() {
    let mut pool = ConstantPool::new();
    let a = pool.insert_float(0x3FF8_0000_0000_0000);
    let b = pool.insert_float128(0x3FF8_0000_0000_0000);
    assert_eq!(pool.get_float_width(a), Some(64));
    assert_eq!(pool.get_float_width(b), Some(128));
    assert_eq!(pool.get_float(a), Some(0x3FF8_0000_0000_0000));
    assert_eq!(pool.get_float128(b), Some(0x3FF8_0000_0000_0000));
}

/// `Default` == `new()`：bool 预置槽存在，`bool_const` 不再是悬空 id。
#[test]
fn default_pool_has_bool_slots() {
    let pool = ConstantPool::default();
    assert_eq!(pool.total_len(), ConstantPool::new().total_len());
    assert_eq!(
        pool.get_int(pool.bool_const(false)),
        Some((0, 1)),
        "Default 池必须预置 false 槽（历史实现绕过 new()，该 id 悬空）"
    );
    assert_eq!(pool.get_int(pool.bool_const(true)), Some((1, 1)));
    assert_eq!(pool.get_float(pool.bool_const(false)), None);
}

/// 聚合池计入 total_len / is_empty。
#[test]
fn totals_include_aggregates() {
    let mut pool = ConstantPool::new();
    let before = pool.total_len();
    let scalar = pool.insert_int(7, 32);
    let agg = pool.insert_aggregate(TypeId::I32, vec![AggChild::Scalar(scalar)]);
    assert!(pool.get_aggregate(agg).is_some());
    assert_eq!(
        pool.total_len(),
        before + 2,
        "聚合必须计入 total_len（历史实现漏算）"
    );
    assert!(!pool.is_empty());
}

/// 跨池重定位保留位宽（内联/克隆路径）。
#[test]
fn remap_carries_float_width() {
    let mut src = ConstantPool::new();
    let cid = src.insert_float_typed(0x3FC0_0000, 32);
    let mut dst = ConstantPool::new();
    let moved = dst.remap_from(&src, cid);
    assert_eq!(dst.get_float_width(moved), Some(32));
    assert_eq!(dst.get_float128(moved), Some(0x3FC0_0000));
}

/// f32 常量作为 **phi 入边** 必须按 f32 还原（历史实现恒按 f64 → 错值）。
///
/// 注意：函数的类型上下文必须与 `Module` 的一致——`Module` 展示时用**模块的**
/// `TypeContext` 解析类型 id，用独立 ctx 造函数会让 id 越界 panic（同一陷阱见
/// `Module::set_data_layout` 的文档）。
#[test]
fn f32_constant_in_phi_prints_as_f32() {
    let mut m = Module::new();
    let ctx = m.types.clone();
    let f32_ty = ctx.f32_ty();
    let sig = FunctionSignature::new(&[], &[f32_ty]);
    let mut b = FunctionBuilder::new("f32_phi", ctx, sig);
    let (entry, _) = b.create_entry_block();
    // `create_block_with_params` 会把当前块切到新块 → 建完目标块再切回 entry 填体
    let (tgt, params) = b.create_block_with_params(&[(f32_ty, "p")]);
    b.switch_to_block(entry);
    let c = b.fconst_f32(1.5);
    b.jump(tgt, &[c]);
    b.switch_to_block(tgt);
    b.ret(&[params[0]]);
    let func = b.finish().expect("finish");
    m.add_function(func);

    let text = format!("{m}");
    assert!(
        text.contains("phi f32 [ 1.5,"),
        "f32 常量应打印成 f32 十进制 1.5；实际：\n{text}"
    );
    assert!(
        !text.contains("e-314"),
        "不得按 f64 还原 f32 位模式（0x3FC0_0000 会是 ~1.9e-314）：\n{text}"
    );
}

/// 同一 f32 位模式的 f64 常量与之**并存**且各自还原。
#[test]
fn f32_and_f64_same_bit_pattern_coexist() {
    let ctx = forge_ir::TypeContext::new();
    let f32_ty = ctx.f32_ty();
    let f64_ty = ctx.f64_ty();
    let sig = FunctionSignature::new(&[], &[f32_ty, f64_ty]);
    let mut b = FunctionBuilder::new("both", ctx, sig);
    let (_entry, _) = b.create_entry_block();
    let a = b.fconst_f32(1.5);
    // 同一个位模式当 f64 常量插入（公开入口 `fconst(bits, ty)`）
    let c = b.fconst(0x3FC0_0000, f64_ty);
    b.ret(&[a, c]);
    let func = b.finish().expect("finish");

    // 两个常量占两个槽位（不再合并）
    assert_eq!(
        func.constants.total_len(),
        4,
        "bool 预置 2 项 + f32/f64 各 1 项（历史实现合并成 1 项 → 3）"
    );
}
