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
    Block, Function, FunctionBuilder, FunctionSignature, Inst, InstFlags, Opcode, TypeContext,
    TypeId, Value, ValueDef, Verifier,
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
    func.dfg.inst_mut(s_inst).operands = smallvec::smallvec![a, a];
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
    func.dfg.inst_mut(s_inst).operands = smallvec::smallvec![a, a];

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
    assert_eq!(
        func.use_lists.use_count(s),
        2,
        "%s 被 %t 使用，且被 ret 终结符使用（S4-a：终结符用值也进 use-def）"
    );
    assert!(func.use_lists.verify(&func.dfg).is_ok());
}

// ============================================================
// S4-a：终结符用值进入 use-def
// ============================================================

/// 构造一个带 `br` 的函数：
/// `entry: br %cond, %then(%x), %else()` / `then: ret %x` / `else: ret %x`。
///
/// `%x` 的终结符使用 = then_args + 两个 `ret` = 3 处；`%cond` = 1 处。
/// `else` 不带宽参数（无前驱带参块会被 `Verifier` 判为多入口）。
fn build_branch() -> (Function, Value, Value) {
    let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("brancher", TypeContext::new(), sig);
    let (then_blk, _) = b.create_block_with_tys(&[TypeId::I32]);
    let else_blk = b.create_block();
    // create_block_with_tys 会把 cur_block 切到新块 ⇒ 最后回到入口块发指令
    let (_entry, params) = b.create_entry_block();
    let x = params[0];
    let cond = b.iconst_i32(1);
    b.branch(cond, then_blk, &[x], else_blk, &[]);
    b.switch_to_block(then_blk);
    b.ret(&[x]);
    b.switch_to_block(else_blk);
    b.ret(&[x]);
    (b.finish().expect("build"), x, cond)
}

/// 基线：`br` 的条件与实参、`ret` 的返回值都登记为（终结符）指令 use。
#[test]
fn terminator_values_are_registered() {
    let (func, x, cond) = build_branch();
    assert_eq!(func.use_lists.use_count(x), 3, "%x 的终结符使用");
    assert_eq!(
        func.use_lists.user_insts(x).len(),
        3,
        "%x 被 3 条终结符指令使用（br + 两个 ret）"
    );
    assert_eq!(
        func.use_lists.user_blocks(&func.dfg, x).len(),
        3,
        "%x 被 entry(br)/then(ret)/else(ret) 使用"
    );
    assert_eq!(func.use_lists.use_count(cond), 1, "分支条件 %cond");
    assert!(
        func.use_lists.verify(&func.dfg).is_ok(),
        "构造完成后 use-lists 必须自洽：{:?}",
        func.use_lists.verify(&func.dfg).err()
    );
    let mut v = Verifier::with_ctx(func.types.clone());
    assert!(v.verify(&func).is_ok(), "{:?}", v.verify(&func).err());
}

/// RAUW 覆盖终结符指令的操作数（终结符就是指令 ⇒ 天然覆盖）。
#[test]
fn rauw_covers_terminator_args() {
    let (mut func, x, cond) = build_branch();

    let replaced = func.replace_all_uses(x, cond);
    assert_eq!(
        replaced, 3,
        "3 处终结符使用（br 实参 + 两个 ret）都应被替换"
    );

    // 值真的被写进了终结符指令（不是只改 use-list）
    for i in 0..func.dfg.block_count() {
        for v in func.dfg.term_used_values(Block::new(i as u32)) {
            assert_ne!(v, x, "块 {i} 的终结符仍引用 %x");
        }
    }
    assert_eq!(func.use_lists.use_count(x), 0, "%x 不应再有使用者");
    assert_eq!(func.use_lists.use_count(cond), 4, "原条件 1 + 被替换的 3");

    let mut v = Verifier::with_ctx(func.types.clone());
    assert!(v.verify(&func).is_ok(), "{:?}", v.verify(&func).err());
}

/// 按形式写入口（`Function::branch`）：旧的终结符指令被墓碑化、新指令登记 use。
#[test]
fn set_terminator_keeps_use_def_fresh() {
    let (mut func, x, cond) = build_branch();
    let entry = func.entry();
    let succs = func.dfg.block_successors(entry);
    let (then_blk, else_blk) = (succs[0], succs[1]);
    // 新 br：条件从 %cond 换成 %x，then_args 也改成 %x
    //（两个目标块都仍可达 ⇒ 可以顺带跑完整 Verifier）
    func.branch(entry, x, then_blk, [x], else_blk, []);

    assert_eq!(
        func.use_lists.use_count(cond),
        0,
        "旧 br 的条件记录应被摘除"
    );
    assert_eq!(
        func.use_lists.use_count(x),
        4,
        "cond 槽 + then_args + 两个 ret"
    );
    assert!(func.use_lists.verify(&func.dfg).is_ok());
    let mut v = Verifier::with_ctx(func.types.clone());
    assert!(v.verify(&func).is_ok(), "{:?}", v.verify(&func).err());
}

/// 就地改写实参（`Function::replace_terminator_args`）：use-def 自动跟上。
///
/// S4-b：codegen 的三处"`ret` 值就地改写"（大聚合返回展开、段值替换、ret 内
/// RAUW）此前直接改 `BlockData.terminator`，在终结符进入 use-def 之后会留下
/// 陈旧 use 项；S4-d 起它们走按形式写入口（`set_return_values` /
/// `replace_all_uses`），本条把"就地改写后 use-def 自洽"的契约钉在本地
/// （该路径只由 forge-rustc e2e 覆盖，本机不可跑）。
#[test]
fn in_place_arg_rewrite_keeps_use_def_fresh() {
    let (mut func, x, cond) = build_branch();
    let entry = func.entry();
    let then_blk = func.dfg.block_successors(entry)[0];
    // 把 br 传给 then 的实参从 %x 换成 %cond
    func.replace_terminator_args(entry, then_blk, [cond]);

    assert_eq!(func.use_lists.use_count(x), 2, "%x 只剩两个 ret");
    assert_eq!(func.use_lists.use_count(cond), 2, "br 的 cond + then_args");
    assert!(
        func.use_lists.verify(&func.dfg).is_ok(),
        "就地改写后 use-lists 必须自洽：{:?}",
        func.use_lists.verify(&func.dfg).err()
    );
    let mut v = Verifier::with_ctx(func.types.clone());
    assert!(v.verify(&func).is_ok(), "{:?}", v.verify(&func).err());
}

/// 陈旧 use 项必须被校验器抓到（`replace_operand` 手工制造不一致状态）。
#[test]
fn verifier_detects_stale_terminator_use() {
    let (mut func, x, cond) = build_branch();
    let entry = func.entry();
    let succs = func.dfg.block_successors(entry);
    let (then_blk, else_blk) = (succs[0], succs[1]);
    // 正常写入新终结符指令（then_args 从 %x 换成 %cond）……
    func.branch(entry, cond, then_blk, [cond], else_blk, []);
    let term_inst = func.dfg.block_terminator(entry).expect("终结符指令");
    // ……再把 idx1 的登记从 %cond 改记到 %x：DFG 槽位仍是 %cond
    // ⇒ 制造出"陈旧 use 项"（等价于改了终结符却没刷新 use-def 的后果）
    func.use_lists.replace_operand(cond, x, term_inst, 1);

    assert!(
        func.use_lists.verify(&func.dfg).is_err(),
        "陈旧终结符 use 项必须被 use-list 校验抓到"
    );
    let mut v = Verifier::with_ctx(func.types.clone());
    let res = v.verify(&func);
    assert!(res.is_err(), "Verifier 应报错");
    assert!(
        res.err()
            .unwrap()
            .iter()
            .any(|e| format!("{e:?}").contains("UseListInconsistency")),
        "期望 UseListInconsistency"
    );

    // 重登记后恢复一致：陈旧记录被清扫，%x 只剩两个 ret
    func.refresh_terminator_uses(entry);
    assert!(func.use_lists.verify(&func.dfg).is_ok());
    assert_eq!(func.use_lists.use_count(x), 2, "entry 的 %x 陈旧记录被摘除");
    assert_eq!(func.use_lists.use_count(cond), 2, "cond 槽 + then_args");
}
