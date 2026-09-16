//! 终结符**投影访问器 + 按形式写入口**的契约守卫（S4-d）。
//!
//! 背景：S4 主体要把终结符并入指令流并删除 `Terminator`。为了让那次原子切换
//! 只需要重写"访问器实现 + 写入口实现"，读取方一律经 [`DataFlowGraph`] 的投影
//! 访问器、写入方一律经 `Function` 的按形式写入口。本文件把这两面钉住：
//!
//! 1. 每种形式的写入都能按同形式的投影**原样读回**（种类、目标、实参、用值）；
//! 2. 写入口与就地改写都会让 use-def 保持新鲜；
//! 3. `TermKind` 判别与 `successors`/`args_to` 语义一致。

use forge_ir::{
    Block, CallConv, FuncRef, Function, FunctionSignature, Opcode, TermKind, TypeContext, TypeId,
    Value,
};

/// 构造一个未终止的空壳函数：入口块 + 两个带 i32 参数的块。
fn fixture() -> (Function, Block, Block, Block, Value) {
    let ctx = TypeContext::new();
    let sig_ref = ctx.register_signature(FunctionSignature::new(&[], &[ctx.i32_ty()]));
    let mut func = Function::new("t", ctx, sig_ref, CallConv::Default);
    let b0 = func.dfg.make_block();
    func.entry_block = Some(b0);
    let (b1, p1) = func.dfg.make_block_with_params(&[TypeId::I32]);
    let (b2, _p2) = func.dfg.make_block_with_params(&[TypeId::I32]);
    (func, b0, b1, b2, p1[0])
}

#[test]
fn jump_write_reads_back_through_projection() {
    let (mut func, b0, b1, _b2, v) = fixture();
    func.jump(b0, b1, [v]);

    assert_eq!(func.dfg.term_kind(b0), Some(TermKind::Jump));
    let (target, args) = func.dfg.term_jump(b0).expect("jump 投影");
    assert_eq!(target, b1);
    assert_eq!(args, &[v]);
    assert_eq!(func.dfg.block_successors(b0), vec![b1]);
    assert_eq!(func.dfg.term_args_to(b0, b1), &[v]);
    assert_eq!(func.dfg.term_used_values(b0), vec![v]);
    // 其它形式的投影必须为 None（判别准确）
    assert!(func.dfg.term_branch(b0).is_none());
    assert!(func.dfg.term_return_values(b0).is_none());
    assert!(func.dfg.term_switch(b0).is_none());
    assert!(func.dfg.term_invoke(b0).is_none());
    assert!(func.dfg.term_resume_value(b0).is_none());
    assert!(!func.dfg.term_is_unreachable(b0));
}

#[test]
fn branch_write_reads_back_through_projection() {
    let (mut func, b0, b1, b2, v) = fixture();
    func.branch(b0, v, b1, [v], b2, []);

    assert_eq!(func.dfg.term_kind(b0), Some(TermKind::Branch));
    let (cond, then_block, then_args, else_block, else_args) =
        func.dfg.term_branch(b0).expect("branch 投影");
    assert_eq!((cond, then_block, else_block), (v, b1, b2));
    assert_eq!(then_args, &[v]);
    assert!(else_args.is_empty());
    assert_eq!(func.dfg.block_successors(b0), vec![b1, b2]);
    assert_eq!(func.dfg.term_args_to(b0, b1), &[v]);
    assert!(func.dfg.term_args_to(b0, b2).is_empty());
    // 规范序遍历序：cond, then_args…
    let mut flat = Vec::new();
    func.dfg
        .for_each_term_value(b0, |idx, value| flat.push((idx, value)));
    assert_eq!(flat, vec![(0, v), (1, v)]);
}

#[test]
fn ret_switch_unreachable_write_read_back() {
    let (mut func, b0, b1, b2, v) = fixture();

    func.ret(b0, [v]);
    assert_eq!(func.dfg.term_kind(b0), Some(TermKind::Return));
    assert_eq!(func.dfg.term_return_values(b0), Some(&[v][..]));
    assert!(func.dfg.block_successors(b0).is_empty());

    // switch：discriminant + default 实参 + case 实参
    let case_args = [v];
    let cases: [(i64, Block, &[Value]); 1] = [(7, b2, &case_args)];
    func.switch(b0, v, b1, [v], &cases);
    assert_eq!(func.dfg.term_kind(b0), Some(TermKind::Switch));
    let view = func.dfg.term_switch(b0).expect("switch 投影");
    assert_eq!((view.discriminant, view.default_block), (v, b1));
    assert_eq!(view.default_args, &[v]);
    assert_eq!(view.cases.len(), 1);
    assert_eq!(view.cases[0].value, 7);
    assert_eq!(view.cases[0].target, b2);
    assert_eq!(view.cases[0].args, &[v]);
    assert!(func.dfg.block_successors(b0).contains(&b1));
    assert!(func.dfg.block_successors(b0).contains(&b2));

    func.unreachable(b0);
    assert_eq!(func.dfg.term_kind(b0), Some(TermKind::Unreachable));
    assert!(func.dfg.term_is_unreachable(b0));
    assert!(func.dfg.block_successors(b0).is_empty());
}

#[test]
fn invoke_and_resume_write_read_back() {
    let (mut func, b0, b1, b2, v) = fixture();

    func.invoke(b0, FuncRef::new(0), [v], TypeId::VOID, b1, [v], b2, []);
    assert_eq!(func.dfg.term_kind(b0), Some(TermKind::Invoke));
    let (callee, args, ret_ty, normal, normal_args, unwind, unwind_args) =
        func.dfg.term_invoke(b0).expect("invoke 投影");
    assert_eq!(callee, FuncRef::new(0));
    assert_eq!(args, &[v]);
    assert_eq!(ret_ty, TypeId::VOID);
    assert_eq!((normal, unwind), (b1, b2));
    assert_eq!(normal_args, &[v]);
    assert!(unwind_args.is_empty());
    assert_eq!(func.dfg.block_successors(b0), vec![b1, b2]);

    func.resume(b0, v);
    assert_eq!(func.dfg.term_kind(b0), Some(TermKind::Resume));
    assert_eq!(func.dfg.term_resume_value(b0), Some(v));
    assert!(func.dfg.block_successors(b0).is_empty());
}

/// 写入口必须让 use-def 保持新鲜；就地改写入口（retarget / replace_args）同样。
#[test]
fn terminator_writes_keep_use_def_fresh() {
    let (mut func, b0, b1, b2, v) = fixture();

    func.branch(b0, v, b1, [v], b2, []);
    // cond(v) + then_args(v) = 2
    assert_eq!(func.use_lists.use_count(v), 2);
    assert!(func.use_lists.verify(&func.dfg).is_ok());

    // 整体替换 then 的实参为空 ⇒ v 的使用降到 1（只剩 cond 槽）
    func.replace_terminator_args(b0, b1, []);
    assert_eq!(func.use_lists.use_count(v), 1);
    assert!(func.use_lists.verify(&func.dfg).is_ok());

    // 目标重指 b1 → b2：只动块编号，用值不变
    func.retarget_terminator(b0, b1, b2);
    let (_, then_block, then_args, else_block, _) = func.dfg.term_branch(b0).expect("branch 投影");
    assert_eq!((then_block, else_block), (b2, b2));
    assert!(then_args.is_empty());
    assert_eq!(func.use_lists.use_count(v), 1);
    assert!(func.use_lists.verify(&func.dfg).is_ok());

    // 重写整个终结符：旧记录被摘除、新记录建立
    func.ret(b0, []);
    assert_eq!(func.use_lists.use_count(v), 0);
    assert_eq!(func.dfg.term_kind(b0), Some(TermKind::Return));
    assert!(func.use_lists.verify(&func.dfg).is_ok());
}

/// **S4 主体的核心不变量**：终结符是 `dfg.insts` 里的一条指令（因此 use-def
/// 天然完整），但**不在 `inst_order` 里**（块内指令列表语义不变）。
#[test]
fn terminator_is_an_inst_outside_inst_order() {
    let (mut func, b0, b1, _b2, v) = fixture();
    func.jump(b0, b1, [v]);

    let term = func.dfg.block_terminator(b0).expect("终结符指令");
    assert_eq!(func.dfg.inst_data(term).opcode, Opcode::Jmp);
    assert!(
        !func.dfg.block(b0).inst_order.contains(&term),
        "终结符不进 inst_order（块内指令列表只含非终结符指令）"
    );
    // 终结符用值就是它的操作数：use-def 里可见、可按普通指令 RAUW
    assert_eq!(func.use_lists.use_count(v), 1);
    assert_eq!(func.use_lists.user_insts(v), vec![term]);
    assert_eq!(func.dfg.inst_data(term).block, b0);

    // 重写终结符：旧指令被墓碑化、新指令接替（同一块只留一条终结符）
    func.ret(b0, [v]);
    let term2 = func.dfg.block_terminator(b0).expect("新终结符指令");
    assert_ne!(term, term2);
    assert_eq!(func.dfg.inst_data(term).opcode, Opcode::Nop);
    assert_eq!(func.dfg.inst_data(term2).opcode, Opcode::Ret);
    assert_eq!(func.use_lists.use_count(v), 1, "旧指令的 use 项已摘除");
    assert!(func.use_lists.verify(&func.dfg).is_ok());
    // （不跑完整 Verifier：本夹具的另两个块未终止，会用块参数规则报多入口）
}

/// 种类判别与 opcode 一致（判别表本体在 `terminator.rs` 单测里与 ops.toml 对账）。
#[test]
fn term_kind_matches_variant() {
    let (mut func, b0, _b1, _b2, _v) = fixture();
    func.unreachable(b0);
    assert_eq!(func.dfg.term_kind(b0), Some(TermKind::Unreachable));
    assert!(TermKind::Branch.has_multiple_successors());
    assert!(TermKind::Switch.has_multiple_successors());
    assert!(TermKind::Invoke.has_multiple_successors());
    assert!(!TermKind::Jump.has_multiple_successors());
    assert!(!TermKind::Return.has_multiple_successors());
    assert!(!TermKind::Resume.has_multiple_successors());
    assert!(!TermKind::Unreachable.has_multiple_successors());
}
