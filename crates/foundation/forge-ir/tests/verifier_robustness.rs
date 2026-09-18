//! 校验器**健壮性**守卫（v3 S6）：坏 IR 上只许返回错误，**绝不许 panic**。
//!
//! `Verifier` 是"最后一道防线"——pass 后严格校验、以及用户手写/解析进来的 IR
//! 都要先过它。因此它的契约是：**能读的就读，读不了就报错**；`dfg.inst_data`/
//! `block`/`value_data` 这类 fail-closed 读取口只允许在"投影已命中"的前提下调用
//! （命中即保证句柄有效），一旦这个前提不成立就会变成 panic——而 panic 会把
//! "IR 坏了"变成"编译器崩了"。
//!
//! 本文件用手工破坏的 IR 逐个钉住这条契约：**返回 `Err` 才行，panic 就是 bug**。

use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::verify::{Verifier, VerifyError};
use forge_ir::{Block, Inst, Value};

/// 造一个 `entry: br cond ? then : else`（三块，均 return）的合法函数。
fn diamond() -> (forge_ir::Function, Block, Block, Block) {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
    let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
    let (entry, _) = fb.create_entry_block();
    let then_b = fb.create_block();
    let else_b = fb.create_block();
    let c = fb.iconst_bool(true);
    fb.branch(c, then_b, &[], else_b, &[]);
    fb.switch_to_block(then_b);
    let v1 = fb.iconst_i32(1);
    fb.ret(&[v1]);
    fb.switch_to_block(else_b);
    let v2 = fb.iconst_i32(2);
    fb.ret(&[v2]);
    let func = fb.finish().expect("build");
    (func, entry, then_b, else_b)
}

fn verify(func: &forge_ir::Function) -> Result<(), Vec<VerifyError>> {
    let mut v = Verifier::with_ctx(func.types.clone());
    v.verify(func)
}

/// 坏 IR ①：跳转目标句柄越界（arena 里根本没有这个块）→ 报错而非 panic。
#[test]
fn out_of_range_branch_target_reports_instead_of_panicking() {
    let (mut func, entry, _then_b, _else_b) = diamond();
    func.jump(entry, Block::new(9999), []);

    let errs = verify(&func).expect_err("越界目标必须上报");
    assert!(
        errs.iter()
            .any(|e| matches!(e, VerifyError::InvalidTerminatorTarget { .. })),
        "应报 InvalidTerminatorTarget：{errs:?}"
    );
}

/// 坏 IR ①b：目标块被 `remove_block` 清空（槽位仍在、但已无终结符）→ 报错而非 panic。
///
/// 注意 arena **不回收槽位**（墓碑语义）：所以移除的块仍能被"找到"，只是空了——
/// 这正是"空块"与"越界句柄"必须分开报的原因。
#[test]
fn removed_block_target_reports_instead_of_panicking() {
    let (mut func, _entry, then_b, _else_b) = diamond();
    func.dfg.remove_block(then_b);

    let errs = verify(&func).expect_err("被移除的目标块必须上报");
    assert!(
        errs.iter()
            .any(|e| matches!(e, VerifyError::MissingTerminator { .. })),
        "应报 MissingTerminator（槽位存在但未终止）：{errs:?}"
    );
}

/// 坏 IR ②：操作数句柄越界（值表里没有这个值）→ 报错而非 panic。
#[test]
fn out_of_range_operand_reports_instead_of_panicking() {
    let (mut func, _entry, _then_b, _else_b) = diamond();
    let add = func
        .dfg
        .insts()
        .find(|(_, i)| i.opcode == forge_ir::Opcode::Iconst)
        .map(|(id, _)| id)
        .expect("iconst");
    // 把一条指令的操作数换成越界句柄（再塞一条 iadd 以便有操作数可改）
    let c1 = func.dfg.insts().map(|(id, _)| id).next().expect("指令");
    let ty = forge_ir::TypeId::I32;
    let inst: Inst = func.dfg.make_inst(
        forge_ir::Opcode::Iadd,
        func.entry(),
        smallvec::smallvec![func.dfg.inst_results(c1)[0]; 2],
        smallvec::SmallVec::new(),
        &[ty],
        forge_ir::InstFlags::NONE,
    );
    func.dfg.inst_mut(inst).operands[0] = Value::new(9999);
    let _ = add;

    let errs = verify(&func).expect_err("越界操作数必须上报");
    assert!(
        errs.iter().any(
            |e| matches!(e, VerifyError::UndefinedValue { value, .. } if value.index() == 9999)
        ),
        "应报 UndefinedValue：{errs:?}"
    );
}

/// 坏 IR ③：被跳转到的块**没有终结符**（未终止）→ 报错而非 panic。
#[test]
fn unterminated_target_block_reports_instead_of_panicking() {
    let (mut func, entry, _then_b, _else_b) = diamond();
    let fresh = func.dfg.make_block(); // 新建块 = 未终止（terminator: None）
    func.jump(entry, fresh, []);

    let errs = verify(&func).expect_err("未终止的被跳转块必须上报");
    assert!(
        errs.iter()
            .any(|e| matches!(e, VerifyError::MissingTerminator { .. })),
        "应报 MissingTerminator：{errs:?}"
    );
}

/// 坏 IR ④：块参数与传入实参数量不符（switch case 表被截断）→ 报错而非 panic。
#[test]
fn truncated_switch_case_table_reports_instead_of_panicking() {
    let ctx = TypeContext::new();
    let sig_ref = ctx.register_signature(FunctionSignature::new(&[], &[ctx.i32_ty()]));
    let mut func = forge_ir::Function::new("s", ctx.clone(), sig_ref, forge_ir::CallConv::Default);
    let (b0, _) = func.dfg.make_block_with_params(&[]);
    let (b1, _) = func.dfg.make_block_with_params(&[]);
    let (b2, _) = func.dfg.make_block_with_params(&[]);
    func.entry_block = Some(b0);
    let disc = func.dfg.make_inst(
        forge_ir::Opcode::Iconst,
        b0,
        smallvec::SmallVec::new(),
        smallvec::smallvec![forge_ir::Immediate::Const(func.constants.insert_int(0, 32))],
        &[forge_ir::TypeId::I32],
        forge_ir::InstFlags::NONE,
    );
    let dv = func.dfg.inst_results(disc)[0];
    func.switch(b0, dv, b1, [], &[(0, b2, &[])]);
    func.ret(b1, []);
    func.ret(b2, []);

    // 截断 immediates：把 case_count 改成一个不可能的大数
    let term = func.dfg.block_terminator(b0).expect("switch");
    let imms = &mut func.dfg.inst_mut(term).immediates;
    if let Some(forge_ir::Immediate::Uint(n)) = imms.get_mut(2) {
        *n = 99;
    }

    let errs = verify(&func).expect_err("坏 case 表必须上报");
    let _ = errs;
}

/// 合法 IR 不该被这些健壮性路径误伤（对照组）。
#[test]
fn well_formed_ir_still_verifies_ok() {
    let (func, _entry, _then_b, _else_b) = diamond();
    assert!(verify(&func).is_ok(), "合法 IR 必须通过");
}

/// 严重级分级：不可达死块是**合法 IR 上的可疑现象** ⇒ 建议级，不是违规级。
#[test]
fn unreachable_block_is_a_warning_not_an_error() {
    let (mut func, entry, then_b, else_b) = diamond();
    // 造一个"不可达且不止一条指令"的死块（解掉 check_reachability 的死 merge 豁免）
    func.jump(entry, then_b, []); // else_b 变成不可达
    let three = {
        let mut v = None;
        for k in 0..3 {
            let i = func.dfg.make_inst(
                forge_ir::Opcode::Iconst,
                else_b,
                smallvec::SmallVec::new(),
                smallvec::smallvec![forge_ir::Immediate::Const(func.constants.insert_int(k, 32))],
                &[forge_ir::TypeId::I32],
                forge_ir::InstFlags::NONE,
            );
            v = Some(i);
        }
        v.expect("指令")
    };
    func.ret(else_b, []);
    let _ = three;

    let errs = verify(&func).expect_err("可疑死块仍会被报告");
    let dead: Vec<_> = errs
        .iter()
        .filter(|e| matches!(e, VerifyError::UnreachableBlock { .. }))
        .collect();
    assert!(!dead.is_empty(), "应报 UnreachableBlock：{errs:?}");
    assert!(
        dead.iter().all(|e| !e.is_error()),
        "不可达块必须是建议级（合法 IR）"
    );
    assert_eq!(
        dead[0].severity(),
        forge_ir::verify::VerifySeverity::Warning
    );
}

/// 结构违规必须是违规级（默认倾向 Error，不因"没测试"而漂移）。
#[test]
fn structural_violations_are_errors() {
    let (mut func, entry, _then_b, _else_b) = diamond();
    func.jump(entry, Block::new(9999), []);
    let errs = verify(&func).expect_err("越界目标必须上报");
    assert!(
        errs.iter()
            .any(|e| matches!(e, VerifyError::InvalidTerminatorTarget { .. }) && e.is_error()),
        "越界目标必须是违规级：{errs:?}"
    );
}
