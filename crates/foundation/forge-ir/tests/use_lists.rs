//! use-list 契约守卫（S6）。
//!
//! 背景（2026-09-14）：`Function::refresh_inst_uses` 的初版实现先调
//! `UseLists::remove_inst`——那个方法是按指令**当前**操作数逐条删的，而
//! `refresh_inst_uses` 是在**改写之后**调用，于是旧操作数的记录根本删不掉，
//! 留下陈旧 use 项（`use-list for v1: inst i2 operand 1 mismatch`，
//! 由 codegen 聚合展开用例 `gep_array_index`/`gep_dynamic_index` 抓到）。
//! 现在改为 `UseLists::forget_inst`（按 `user == inst` 清扫，与操作数内容无关）。
//! 本文件把语义钉住：就地改写操作数后 `refresh_inst_uses` 必须让 use-lists
//! 与 DFG 完全一致。

use forge_ir::{
    Function, FunctionBuilder, FunctionSignature, Inst, InstFlags, Opcode, TypeContext, TypeId,
    Value, ValueDef, Verifier,
};

/// 构造 `%s = iadd %a, %b`（外加两个 iconst），返回函数、操作数、结果与 iadd 指令 id。
fn build_add() -> (Function, Value, Value, Inst) {
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("f", TypeContext::new(), sig);
    let (_entry, _) = b.create_entry_block();
    let a = b.iconst_i32(1);
    let bv = b.iconst_i32(2);
    let s = b.iadd(a, bv);
    b.ret(&[s]);
    let func = b.finish().expect("build");

    let s_inst = match func.dfg.value_def(s) {
        Some(ValueDef::Inst(i, 0)) => *i,
        other => panic!("预期 %s 由指令定义，实际 {other:?}"),
    };
    (func, a, bv, s_inst)
}

/// 就地改写操作数后 `refresh_inst_uses`：旧操作数记录必须消失、新记录必须建立。
#[test]
fn refresh_after_operand_rewrite_is_consistent() {
    let (mut func, a, bv, s_inst) = build_add();
    // 初始：%a 与 %b 各被 %s 使用一次
    assert_eq!(func.use_lists.use_count(a), 1);
    assert_eq!(func.use_lists.use_count(bv), 1);

    // 模拟 pass 的就地改写：第二个操作数 %b → %a（保持二元 Iadd 合法，
    // 于是本用例可以顺带跑完整 Verifier）。旧实现会留下 %b 的陈旧记录。
    func.dfg.insts[s_inst.0 as usize].operands = smallvec::smallvec![a, a];
    func.refresh_inst_uses(s_inst);

    assert_eq!(
        func.use_lists.use_count(bv),
        0,
        "旧操作数 %b 的使用记录必须被清掉（按 user 清扫，而不是按当前操作数）"
    );
    assert_eq!(
        func.use_lists.use_count(a),
        2,
        "改写后的两个操作数位置都记到 %a 名下"
    );
    let mut v = Verifier::with_ctx(func.types.clone());
    let res = v.verify(&func);
    assert!(
        res.is_ok(),
        "改写 + refresh 后 use-lists 必须与 DFG 一致：{:?}",
        res.err()
    );
}

/// `forget_inst` 删除该指令的**全部**记录（不只是当前操作数对应的位置）。
#[test]
fn forget_inst_removes_all_entries_of_that_user() {
    let (mut func, a, bv, s_inst) = build_add();
    // 先就地改写，制造"按当前操作数删不掉旧记录"的局面
    func.dfg.insts[s_inst.0 as usize].operands = smallvec::smallvec![a, a];

    let removed = func.use_lists.forget_inst(s_inst);
    assert_eq!(removed, 2, "按 user 清扫：两条记录全删（含已陈旧的 %b）");
    assert_eq!(func.use_lists.use_count(a), 0);
    assert_eq!(func.use_lists.use_count(bv), 0);
    // DFG 未动 → 此刻 use-lists 与 DFG 不一致（这正是"忘了重登记"的状态）
    assert!(
        func.use_lists.verify(&func.dfg).is_err(),
        "清空记录但 DFG 仍有操作数时必须报不一致"
    );
    // 重登记后恢复一致
    func.refresh_inst_uses(s_inst);
    assert!(func.use_lists.verify(&func.dfg).is_ok());
    assert_eq!(func.use_lists.use_count(a), 2);
}

/// `Function::make_inst` 建指令时自动登记（pass 不应再直接 `dfg.make_inst`）。
#[test]
fn function_make_inst_registers_uses() {
    let (mut func, a, bv, s_inst) = build_add();
    let entry = func.entry_block.expect("entry block");
    // 再建一条 `%t = iadd %a, %s`
    let s = func.dfg.inst_results(s_inst)[0];
    func.make_inst(
        Opcode::Iadd,
        entry,
        smallvec::smallvec![a, s],
        smallvec::smallvec![],
        &[TypeId::I32],
        InstFlags::NONE,
    );
    assert_eq!(func.use_lists.use_count(a), 2, "%a 被 %s 与 %t 使用");
    assert_eq!(func.use_lists.use_count(bv), 1);
    assert_eq!(func.use_lists.use_count(s), 1, "%s 被 %t 使用");
    assert!(func.use_lists.verify(&func.dfg).is_ok());
}
