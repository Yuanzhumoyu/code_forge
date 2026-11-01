//! win64 i128 返回打包（从 `wide_split` 拆出，控制在文件规模棘轮内）。
//!
//! C ABI 把 16 字节返回值放 XMM0；IR 侧把两个肢 store 进 16B 栈缓冲，再
//! load `<2 x i64>`。不用 `Vinsert`：x86 的 `rd≤128` 规则恒写低 lane，忽略
//! lane 立即数，插高肢会覆盖低肢。

use super::*;
use forge_ir::entity::map::SecondaryMap;

pub(super) fn pack_win64_i128_return(
    e: &mut LimbEmitter<'_>,
    parts: &SecondaryMap<Value, Limbs>,
    ret_xmm: bool,
) -> Result<(), IrError> {
    if !ret_xmm {
        return Ok(());
    }
    let mut jobs: Vec<(Block, Value)> = Vec::new();
    for (b, _bd) in e.func.dfg.blocks() {
        if let Some(vals) = e.func.dfg.term_return_values(b) {
            for &v in vals {
                if e.func.dfg.value_type(v) == Some(TypeId::I128) {
                    jobs.push((b, v));
                }
            }
        }
    }
    if jobs.is_empty() {
        return Ok(());
    }

    let vec_ty = e.func.types.vector_ty(TypeId::I64, 2);
    let entry = e.func.entry();
    let alloc = e.func.make_inst(
        Opcode::Alloca,
        entry,
        smallvec::smallvec![],
        smallvec::smallvec![Immediate::Type(TypeId::I64), Immediate::Uint(2)],
        &[TypeId::PTR],
        InstFlags::NONE,
    );
    let buf = e.func.dfg.inst_data(alloc).results[0];
    // make_inst 已把 alloca 追加到块序；移到入口开头保证支配所有使用。
    {
        let order = &mut e.func.dfg.block_mut(entry).inst_order;
        let ni = order.pop().expect("刚 push 的 alloca");
        order.insert(0, ni);
    }
    e.created += 1;
    for (b, old) in jobs {
        let limbs = parts.get(old).cloned().unwrap_or_default();
        if limbs.len() != 2 {
            return Err(wide_shape_unsupported(
                "win64 i128 返回：宽值未拆成两个肢，无法打包向量",
            ));
        }
        let one = e.func.make_inst(
            Opcode::Iconst,
            b,
            smallvec::smallvec![],
            smallvec::smallvec![Immediate::Int(1)],
            &[TypeId::I64],
            InstFlags::NONE,
        );
        let one_v = e.func.dfg.inst_data(one).results[0];
        let hi = e.func.make_inst(
            Opcode::GetElementPtr,
            b,
            smallvec::smallvec![buf, one_v],
            smallvec::smallvec![Immediate::Type(TypeId::I64)],
            &[TypeId::PTR],
            InstFlags::NONE,
        );
        let hi_v = e.func.dfg.inst_data(hi).results[0];
        let _st_lo = e.func.make_inst(
            Opcode::Store,
            b,
            smallvec::smallvec![limbs[0], buf],
            smallvec::smallvec![],
            &[],
            InstFlags::SIDE_EFFECT,
        );
        let _st_hi = e.func.make_inst(
            Opcode::Store,
            b,
            smallvec::smallvec![limbs[1], hi_v],
            smallvec::smallvec![],
            &[],
            InstFlags::SIDE_EFFECT,
        );
        let load = e.func.make_inst(
            Opcode::Load,
            b,
            smallvec::smallvec![buf],
            smallvec::smallvec![],
            &[vec_ty],
            InstFlags::NONE,
        );
        let vec = e.func.dfg.inst_data(load).results[0];
        e.created += 5;
        e.func.set_return_values(b, [vec]);
    }
    let old_sig = e
        .func
        .types
        .borrow()
        .get_signature(e.func.signature)
        .clone();
    let new_sig = FunctionSignature {
        params: old_sig.params.clone(),
        returns: vec![vec_ty],
        calling_convention: old_sig.calling_convention,
        variadic: old_sig.variadic,
    };
    e.func.signature = e.func.types.borrow_mut().register_signature(new_sig);
    Ok(())
}
