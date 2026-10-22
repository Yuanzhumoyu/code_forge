//! 常量池（v28 新设计）回归：比特值合并、无预置槽、端序进键、跨池导入、聚合句柄统一。

use forge_ir::ir::constant::{BitValue, ConstEntry};
use forge_ir::{ConstId, ConstantPool, Endianness, TypeId};

/// 整数与浮点合并成**同一个比特值通道**：同 `(bits,width)` 即同一条目。
#[test]
fn bits_channel_merges_int_and_float() {
    let mut pool = ConstantPool::new();
    let f32_bits = 0x3FC0_0000u128; // f32 1.5 的位模式
    // 同一个位模式，一条按 int 入池、一条按 float 入池
    let as_int = pool.intern_bits(BitValue::new(f32_bits, 32));
    let as_float = pool.intern_float(f32_bits, 32);
    assert_eq!(
        as_int, as_float,
        "合并通道：(bits,width) 相同 ⇒ 同一 ConstId（解释由使用点类型决定）"
    );
    // 位宽参与键：同一位模式不同宽度是不同常量
    let as_f64 = pool.intern_float(f32_bits, 64);
    assert_ne!(as_float, as_f64, "同一位模式不同值宽必须是两个 ConstId");
    assert_eq!(pool.bits(as_float), Some(BitValue::new(f32_bits, 32)));
    assert_eq!(pool.bits(as_f64), Some(BitValue::new(f32_bits, 64)));
    assert_eq!(pool.len(), 2, "两个不同 (bits,width) ⇒ 两条");
}

/// 整数入池是**符号扩展规范形**：`intern_int(-1,64)` 与 `intern_bits(u64::MAX, 64)` 同一条目。
#[test]
fn int_canonical_form_is_sign_extended() {
    let mut pool = ConstantPool::new();
    let a = pool.intern_int(-1, 64);
    // 规范形是**符号扩展**后的位图（低 64 位全 1、高位也全 1）
    let b = pool.intern_bits(BitValue::new(u128::MAX, 64));
    assert_eq!(a, b, "-1 在 64 位里的规范形是全 1 位图");
    assert_eq!(pool.bits(a), Some(BitValue::new(u128::MAX, 64)));
    // `i1` 是 bool：不做符号扩展
    let t = pool.intern_int(1, 1);
    let f = pool.intern_int(0, 1);
    assert_ne!(t, f);
    assert_eq!(pool.bits(t), Some(BitValue::new(1, 1)));
    assert_eq!(pool.bits(f), Some(BitValue::new(0, 1)));
}

/// 空池：**没有**预置槽；bool 只是普通常量（去重后自然两条）。
#[test]
fn pool_has_no_presets() {
    let mut pool = ConstantPool::new();
    assert!(pool.is_empty());
    assert_eq!(pool.len(), 0);
    let f = pool.intern_int(0, 1);
    let t = pool.intern_int(1, 1);
    assert_ne!(f, t);
    // 反复入池不膨胀
    for _ in 0..10 {
        pool.intern_int(1, 1);
    }
    assert_eq!(pool.len(), 2);
}

/// 向量：**端序进去重键**（同字节不同端序是不同常量）。
#[test]
fn vector_endian_is_part_of_key() {
    let mut pool = ConstantPool::new();
    let le = pool.intern_vector(&[1, 2, 3, 4], Endianness::Little);
    let be = pool.intern_vector(&[1, 2, 3, 4], Endianness::Big);
    assert_ne!(le, be, "同字节不同端序不能去重");
    assert_eq!(pool.vector(le), Some(&[1u8, 2, 3, 4][..]));
    assert_eq!(pool.vector_endian(le), Some(Endianness::Little));
    assert_eq!(pool.vector_endian(be), Some(Endianness::Big));
    // 同字节同端序去重
    assert_eq!(pool.intern_vector(&[1, 2, 3, 4], Endianness::Big), be);
}

/// 聚合：子节点就是 `ConstId`（标量或嵌套聚合），不再有独立 `AggId`。
#[test]
fn aggregate_children_are_const_ids() {
    let mut pool = ConstantPool::new();
    let one = pool.intern_int(1, 32);
    let inner = pool.intern_agg(TypeId::I32, vec![one]);
    let outer = pool.intern_agg(TypeId::I16, vec![one, inner]);
    let agg = pool.agg(outer).expect("聚合可解析");
    assert_eq!(agg.children, vec![one, inner]);
    // 子节点类型按 tag 自辨
    assert!(matches!(pool.get(one), Some(ConstEntry::Bits(_))));
    assert!(matches!(pool.get(inner), Some(ConstEntry::Agg(_))));
    assert_ne!(one.tag(), outer.tag());
}

/// 跨池导入：四通道全覆盖、聚合递归、端序保留、不依赖索引。
#[test]
fn import_rebuilds_all_channels() {
    let mut src = ConstantPool::new();
    let mut dst = ConstantPool::new();

    let i = src.intern_int(42, 64);
    let f = src.intern_float(1.5f64.to_bits() as u128, 64);
    let b = src.intern_big(forge_ir::util::big::Big::from_i128(1i128 << 100));
    let v = src.intern_vector(&[9, 8, 7, 6], Endianness::Big);
    let agg = src.intern_agg(TypeId::I32, vec![i, b]);

    // 先让 dst 索引错开，验证导入不依赖索引
    dst.intern_int(7, 8);

    let ri = dst.import(&src, i);
    let rf = dst.import(&src, f);
    let rb = dst.import(&src, b);
    let rv = dst.import(&src, v);
    let ragg = dst.import(&src, agg);

    assert_eq!(dst.bits(ri), src.bits(i));
    assert_eq!(dst.bits(rf), src.bits(f));
    assert_eq!(dst.big(rb), src.big(b));
    assert_eq!(dst.vector(rv), src.vector(v));
    assert_eq!(dst.vector_endian(rv), src.vector_endian(v));
    let children = dst.agg(ragg).expect("聚合已导入").children.clone();
    assert_eq!(children.len(), 2);
    assert_eq!(dst.big(children[1]), src.big(b), "聚合子节点也被重建");
}

/// 超 128 位的整数只能走 `Big` 通道：`bits()` 对其返回 None。
#[test]
fn big_channel_is_not_bits() {
    let mut pool = ConstantPool::new();
    let big = pool.intern_big(forge_ir::util::big::Big::from_i128(-5));
    assert_eq!(pool.bits(big), None);
    assert!(pool.big(big).is_some());
    // 视图强转仍可用
    assert_eq!(pool.as_i64(big), Some(-5));
}

/// 句柄编码：四元 tag 全域定义，任何 `u32` 都能往返。
#[test]
fn const_id_encoding_is_total() {
    for raw in [0u32, 1, 0x4000_0001, 0x8000_0002, 0xC000_0003, u32::MAX] {
        let id = ConstId::from_raw(raw);
        assert_eq!(id.raw(), raw, "raw/from_raw 对已编码值必须逐位往返");
    }
    let ids = [
        ConstId::Bits(3),
        ConstId::Big(4),
        ConstId::Vector(5),
        ConstId::Agg(6),
    ];
    for id in ids {
        assert_eq!(ConstId::from_raw(id.raw()), id, "raw/from_raw 往返");
        assert_eq!(
            ConstId::pack(id.tag(), id.index()),
            id,
            "pack/tag/index 往返"
        );
        assert!(
            id.index() < (1 << 30),
            "索引只占低 30 位（tag 在最高 2 位）"
        );
    }
    assert_eq!(ConstId::default(), ConstId::Bits(0));
}

/// `AsConst`：Rust 标量 → IR 常量的类型与值。
#[test]
fn as_const_covers_scalars() {
    use forge_ir::ir::builder::FunctionBuilder;
    use forge_ir::ir::constant::AsConst;
    use forge_ir::ir::types::{FunctionSignature, TypeContext};

    let tc = TypeContext::new();
    let mut fb = FunctionBuilder::new("c", tc, FunctionSignature::new(&[], &[TypeId::I64]));
    fb.create_block_here();
    // 每个标量都物化成一条常量指令；类型由 Rust 类型决定
    let (i32v, i8v, u64v, f32v, f64v, bv, i128v, u128v, sugar, explicit) = (
        42i32.into_const(&mut fb),
        7i8.into_const(&mut fb),
        9u64.into_const(&mut fb),
        1.5f32.into_const(&mut fb),
        2.5f64.into_const(&mut fb),
        true.into_const(&mut fb),
        1i128.into_const(&mut fb),
        u128::MAX.into_const(&mut fb),
        fb.constant(3i32),
        fb.const_int(3, TypeId::I32),
    );
    fb.ret(&[i32v]);
    let func = fb.finish().expect("build");

    let ty = |v: forge_ir::Value| func.dfg.value_type(v);
    assert_eq!(ty(i32v), Some(TypeId::I32));
    assert_eq!(ty(i8v), Some(TypeId::I8));
    assert_eq!(ty(u64v), Some(TypeId::I64), "u64 与 i64 同位宽");
    assert_eq!(ty(f32v), Some(TypeId::F32));
    assert_eq!(ty(f64v), Some(TypeId::F64));
    assert_eq!(ty(bv), Some(TypeId::BOOL));
    assert_eq!(ty(i128v), Some(TypeId::I128));
    assert_eq!(ty(u128v), Some(TypeId::I128), "u128 以位图形式落在 i128");

    // 常量值：便捷入口与显式构造落到同一个 ConstId
    let cid_of = |v: forge_ir::Value| -> ConstId {
        let forge_ir::ValueDef::Inst(inst, _) = func.dfg.value_def(v).expect("值是 Define")
        else {
            panic!("常量应由指令定义");
        };
        match func.dfg.inst_data(*inst).immediates.first() {
            Some(forge_ir::Immediate::Const(c)) => *c,
            other => panic!("常量指令应带 Const 立即数，实测 {other:?}"),
        }
    };
    assert_eq!(cid_of(sugar), cid_of(explicit));
    let v = func.constants.bits(cid_of(u128v)).expect("u128 位值");
    assert_eq!(v.bits, u128::MAX);
    assert_eq!(v.width, 128);
}
