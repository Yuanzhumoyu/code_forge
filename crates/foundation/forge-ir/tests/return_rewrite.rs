//! `set_return_values` 必须同步清掉旧返回值的 use、登记新返回值。
//!
//! 宽返回打包（i128→V128）依赖这个不变量；没有它，泄漏守卫会误报旧值仍活。
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::{Opcode, TypeId};

#[test]
fn set_return_values_clears_old_use() {
    let i128 = TypeId::I128;
    let i64 = TypeId::I64;
    let sig = FunctionSignature::new(&[(i64, "a")], &[i128]);
    let mut b = FunctionBuilder::new("widen", TypeContext::new(), sig);
    let (e, p) = b.create_block_with_params(&[(i64, "a")]);
    b.switch_to_block(e);
    let wide = b.uextend(p[0], i128);
    b.ret(&[wide]);
    let mut f = b.finish().expect("build");
    // 造一个**不依赖旧值**的新 i128（Iconst 0），改成返回它。
    let new = f.make_inst(
        Opcode::Iconst,
        e,
        smallvec::smallvec![],
        smallvec::smallvec![forge_ir::Immediate::Int(0)],
        &[i128],
        forge_ir::InstFlags::NONE,
    );
    let new_v = f.dfg.inst_data(new).results[0];
    f.dfg.block_mut(e).inst_order.insert(0, new);
    f.set_return_values(e, [new_v]);
    assert!(!f.use_lists.has_uses(wide), "旧返回值仍被使用");
    assert!(f.use_lists.has_uses(new_v), "新返回值没有使用");
}
