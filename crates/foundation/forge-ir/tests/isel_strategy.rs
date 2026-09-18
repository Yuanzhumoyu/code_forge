//! 指令选择标签的守卫（v3 方案 S5 第 1 项：`isel_strategy` 类型化 + 字段私有化）。
//!
//! 这里只经**公开入口**读写标签：`Instruction::{isel_strategy,
//! set_isel_strategy, clear_isel_strategy}`。字段私有化本身没有运行期断言
//! （直接写字段会编译失败），所以本文件同时是"外部 crate 只能这么用"的样例。

use forge_ir::{
    Block, CallConv, Function, FunctionSignature, Inst, InstFlags, IselStrategy, Opcode,
    TypeContext, TypeId, Value,
};

/// 一个未终止的单块函数：入参 `(i32, i32)` + 块内一条 `Iadd`。
fn fixture(name: &str) -> (Function, Block, Inst, Value) {
    let ctx = TypeContext::new();
    let sig_ref = ctx.register_signature(FunctionSignature::new(&[], &[ctx.i32_ty()]));
    let mut func = Function::new(name, ctx, sig_ref, CallConv::Default);
    let (b0, params) = func.dfg.make_block_with_params(&[TypeId::I32, TypeId::I32]);
    func.entry_block = Some(b0);
    let iadd = func.dfg.make_inst(
        Opcode::Iadd,
        b0,
        smallvec::smallvec![params[0], params[1]],
        smallvec::SmallVec::new(),
        &[TypeId::I32],
        InstFlags::NONE,
    );
    let result = func.dfg.inst_results(iadd)[0];
    (func, b0, iadd, result)
}

/// 运行期（非 `'static`）标签名可直接挂上：不再需要 `Box::leak` 造静态串。
#[test]
fn runtime_label_needs_no_static_leak() {
    let (mut func, _b0, iadd, _) = fixture("f");
    // 名字来自运行期数据（不是字面量）——旧类型 `Option<&'static str>` 只能
    // 靠 `Box::leak` 才能接受它
    let dynamic_name = format!("lea_sib:{}", 2 + 2);
    let tag = IselStrategy::new(dynamic_name.clone());
    assert_eq!(tag.name(), dynamic_name, "名字原样保留（含目标侧参数）");

    func.dfg.inst_mut(iadd).set_isel_strategy(tag);
    assert_eq!(
        func.dfg
            .inst_data(iadd)
            .isel_strategy()
            .map(IselStrategy::name),
        Some(dynamic_name.as_str()),
        "标签应可读回"
    );
}

/// 标签随 `clone_inst` 保留，且**按内容**相等（不是池内 id 的指针相等）。
#[test]
fn label_survives_clone_inst() {
    let (mut func, _b0, iadd, _) = fixture("f");
    func.dfg
        .inst_mut(iadd)
        .set_isel_strategy(IselStrategy::from_static("lea_sib:4"));

    let target = func.dfg.make_block();
    let mut remap = forge_ir::entity::map::SecondaryMap::new();
    let cloned = func.dfg.clone_inst(iadd, target, &mut remap);

    let orig_tag = func
        .dfg
        .inst_data(iadd)
        .isel_strategy()
        .expect("原指令有标签")
        .clone();
    let cloned_tag = func
        .dfg
        .inst_data(cloned)
        .isel_strategy()
        .expect("克隆指令应保留标签");
    assert_eq!(cloned_tag, &orig_tag, "克隆后标签内容相同");
    // 值语义：另造一个同名标签也相等
    assert_eq!(
        IselStrategy::new(String::from("lea_sib:4")),
        orig_tag,
        "标签相等按内容"
    );

    // 摘除：返回被摘下的标签，之后读回 None（`None` 是"无标签"的唯一编码）
    let taken = func.dfg.inst_mut(cloned).clear_isel_strategy();
    assert_eq!(taken.as_ref(), Some(&orig_tag));
    assert!(func.dfg.inst_data(cloned).isel_strategy().is_none());
}

/// 标签是**值**：可从被调方 DFG 读到、挂到调用方 DFG 的指令上
/// （inline / lto / func_specialize 的搬运形态，池内 id 在这里会失真）。
#[test]
fn label_crosses_dfgs() {
    let (mut callee, _cb, callee_inst, _) = fixture("callee");
    callee
        .dfg
        .inst_mut(callee_inst)
        .set_isel_strategy(IselStrategy::from_static("lea-merge-iadd-imul-4"));

    let (mut caller, _pb, caller_inst, _) = fixture("caller");
    assert!(caller.dfg.inst_data(caller_inst).isel_strategy().is_none());

    let moved = callee
        .dfg
        .inst_data(callee_inst)
        .isel_strategy()
        .expect("被调方有标签")
        .clone();
    caller.dfg.inst_mut(caller_inst).set_isel_strategy(moved);

    assert_eq!(
        caller
            .dfg
            .inst_data(caller_inst)
            .isel_strategy()
            .map(IselStrategy::name),
        Some("lea-merge-iadd-imul-4"),
        "跨 DFG 搬运后名字完好（值语义）"
    );
}

/// 覆盖写标签：后写者胜，不叠加（标签是单值附件，不是元数据列表）。
#[test]
fn label_is_single_valued() {
    let (mut func, _b0, iadd, _) = fixture("f");
    let inst = func.dfg.inst_mut(iadd);
    assert!(inst.isel_strategy().is_none(), "默认无标签");
    inst.set_isel_strategy(IselStrategy::from_static("first"));
    inst.set_isel_strategy(IselStrategy::from_static("second"));
    assert_eq!(inst.isel_strategy().map(IselStrategy::name), Some("second"));
}

/// 空标签名 fail-closed：`None` 才是"没有标签"，空串不是一种标签。
#[test]
#[should_panic(expected = "指令选择标签名不能为空")]
fn empty_label_is_rejected() {
    let _ = IselStrategy::new(String::new());
}
