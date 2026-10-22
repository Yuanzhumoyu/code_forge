//! 块参数的**结构性替换**契约（`Function::splice_block_param`）。
//!
//! 为什么单独立测试：这是把宽值（i128）拆成肢的唯一合法入口——`dfg.rs` 的契约写明
//! "块参数的结构性增删只能走 `make_block*`/`remove_block`/`Function::*`"，而拆肢要同时改
//! ①参数表 ②每个前驱终结符的实参序列 ③**后续参数的 `ValueDef::Param` 下标**。
//! 三处漏一处，IR 就"看起来能跑"但 verifier 或 isel 会错位。

use forge_ir::ir::dfg::ValueDef;
use forge_ir::ir::terminator::TermKind;
use forge_ir::ir::types::TypeContext;
use forge_ir::{
    Block, Function, FunctionBuilder, FunctionSignature, IntCC, TypeId, Value, Verifier,
};

/// 建一个钻石 CFG：`entry` 按条件进 `then`/`else`，两支都跳 `merge`（带一个 i128 参数）。
///
/// 返回 `(函数, merge, 每条入边的两个 i64 "肢" 候选值)`——肢在前驱里就地建，
/// 正好模拟 `wide_split` 真实场景（肢只能在前驱手上取）。
fn diamond_with_merge_param(types: &TypeContext) -> (Function, Block, Vec<(Block, Vec<Value>)>) {
    let sig = FunctionSignature::new(&[(TypeId::I128, "n")], &[TypeId::I128]);
    let mut b = FunctionBuilder::new("f", types.clone(), sig);
    let entry = b.create_block();
    let then_blk = b.create_block();
    let else_blk = b.create_block();
    let (merge, merge_params) = b.create_block_with_params(&[(TypeId::I128, "r")]);
    b.switch_to_block(entry);
    let one = b.const_int(1i128, TypeId::I128);
    let zero = b.const_int(0i128, TypeId::I128);
    let cond = b.icmp(IntCC::Sgt, one, zero);
    b.branch(cond, then_blk, &[], else_blk, &[]);

    let mut edges: Vec<(Block, Vec<Value>)> = Vec::new();
    b.switch_to_block(then_blk);
    let a = b.const_int(11i128, TypeId::I128);
    let a_lo = b.const_int(11i64, TypeId::I64);
    let a_hi = b.const_int(0i64, TypeId::I64);
    b.jump(merge, &[a]);
    edges.push((then_blk, vec![a_lo, a_hi]));

    b.switch_to_block(else_blk);
    let c = b.const_int(22i128, TypeId::I128);
    let c_lo = b.const_int(22i64, TypeId::I64);
    let c_hi = b.const_int(0i64, TypeId::I64);
    b.jump(merge, &[c]);
    edges.push((else_blk, vec![c_lo, c_hi]));

    b.switch_to_block(merge);
    b.ret(&[merge_params[0]]);
    (b.finish().expect("diamond"), merge, edges)
}

#[test]
fn splice_block_param_rewrites_params_defs_and_pred_args() {
    let types = TypeContext::new();
    let (mut func, merge, edges) = diamond_with_merge_param(&types);
    let old_param = func.dfg.block_param_values(merge)[0];

    let preds: Vec<Block> = func.predecessors().get(merge).cloned().unwrap_or_default();
    assert_eq!(preds.len(), 2, "钻石应有两条入边");

    let mut per_pred: forge_ir::entity::map::SecondaryMap<Block, Vec<Value>> =
        forge_ir::entity::map::SecondaryMap::new();
    for (b, v) in edges {
        per_pred.insert(b, v);
    }
    assert_eq!(per_pred.len(), 2, "两条入边都要给出展开序列");

    let new_params = func.splice_block_param(merge, 0, &[TypeId::I64, TypeId::I64], &per_pred);

    // ① 参数表：两个 i64
    assert_eq!(new_params.len(), 2);
    let params_now = func.dfg.block(merge).params.clone();
    assert_eq!(params_now.len(), 2, "原宽参数应被替换成两个");
    assert_eq!(params_now[0], TypeId::I64);
    assert_eq!(params_now[1], TypeId::I64);

    // ② 值的定义：下标必须是 0/1（删旧参数后后面的参数平移 ⇒ 必须修正）
    for (k, v) in new_params.iter().enumerate() {
        match func.dfg.value_data(*v).def {
            ValueDef::Param(b, idx) => {
                assert_eq!(b, merge);
                assert_eq!(idx as usize, k);
            }
            ref other => panic!("新参数的定义不是 Param: {other:?}"),
        }
    }

    // ③ 前驱边：每个前驱该位置变成两个实参
    for &p in preds.iter() {
        let (t, args) = func.dfg.term_jump(p).expect("前驱应为无条件跳转");
        assert_eq!(t, merge);
        assert_eq!(args.len(), 2, "旧的一个宽实参应展开成两个");
        assert!(!args.contains(&old_param), "旧宽实参必须被移除");
    }

    // ④ 结构改完仍要过 verifier（块参数个数/类型必须与所有前驱一致）
    let mut v = Verifier::with_ctx(types.clone());
    v.verify(&func).expect("splice 之后 IR 必须仍然合法");

    // ⑤ 终结符没被破坏：merge 仍以 `ret` 结束
    assert_eq!(func.dfg.term_kind(merge), Some(TermKind::Return));
    let rv = func.dfg.term_return_values(merge).expect("ret 操作数");
    assert_eq!(rv.len(), 1, "ret 应返回一个值");
}
