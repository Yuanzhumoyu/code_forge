//! 分析缓存一致性守卫（v3 S6：`AnalysisManager` 修订号自校验 + 快照语义）。
//!
//! 旧契约（`OnceLock`）：缓存一旦初始化就**只能靠调用方记得失效**——忘记
//! `analysis_mut().invalidate()` 就静默返回改图之前的结果，本文件当时靠
//! "校验器重算比对"事后抓漏网。
//!
//! 新契约（`AnalysisManager`，2026-09-16）：
//!
//! - 缓存槽记着"算于哪个修订号"（[`AnalysisRevision`] = DFG 结构修订号 + 显式
//!   失效次数），**读取时自校验、过期即重算** ⇒ 陈旧结果不可能被读到，
//!   "忘了失效"不再是缺陷（`invalidate_analysis()` 只剩省一次重算的作用）；
//! - 结构修订号在三条改 CFG 的路上自动前进：块增删、终结符写入、**终结符被
//!   墓碑化**（最后这条是实测出来的漏洞：`tombstone_inst` 此前不失效缓存，
//!   而块内顺序表不含终结符，所以它是一条绕过 `set_terminator` 的改图路）；
//! - 读者拿 `Arc` 快照：快照在本次调用内恒定，不被后续改写影响。

use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::{Block, InstFlags, Opcode, TypeId};
use std::sync::Arc;

/// 两个块 + 一个条件分支：`entry: br c, then, else`（三块都 return）。
fn diamond() -> (forge_ir::Function, Block, Block, Block) {
    let ctx = forge_ir::ir::types::TypeContext::new();
    let sig = forge_ir::ir::types::FunctionSignature::new(&[], &[ctx.i32_ty()]);
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

fn succs_of(func: &forge_ir::Function, block: Block) -> Vec<Block> {
    func.successors().get(block).cloned().unwrap_or_default()
}

/// 缓存读在前、改 CFG 在后：`successors` 必须反映新边（写入口自动失效）。
#[test]
fn cfg_write_entries_invalidate_successors_cache() {
    let (mut func, entry, then_b, else_b) = diamond();
    // 先初始化缓存
    assert_eq!(succs_of(&func, entry), vec![then_b, else_b]);
    assert_eq!(
        func.predecessors().get(then_b).cloned().unwrap_or_default(),
        vec![entry]
    );
    let _ = func.dominator_tree(); // 初始化支配树缓存

    // 无条件跳到 then_b（else_b 变成不可达）
    func.jump(entry, then_b, []);

    assert_eq!(succs_of(&func, entry), vec![then_b], "改 CFG 后必须是新图");
    assert!(
        func.predecessors()
            .get(else_b)
            .map(|v| v.is_empty())
            .unwrap_or(true),
        "else 块不应再有前驱"
    );
    assert_eq!(func.dominator_tree().idom(else_b), None, "不可达块无 idom");
}

/// 快照语义：先读到的分析在改写控制流后**不变**，新读才是新图。
///
/// （旧 `OnceLock`+`&T` 语义下第二次读会拿到被就地改写过的内容，"同一份分析"
/// 在一次使用期内可能前后不一致。）
#[test]
fn analysis_snapshot_is_stable_across_cfg_rewrite() {
    let (mut func, entry, then_b, else_b) = diamond();
    let before = func.successors();
    assert_eq!(
        before.get(entry).cloned().unwrap_or_default(),
        vec![then_b, else_b]
    );
    let dt_before = func.dominator_tree();
    assert!(dt_before.idom(else_b).is_some());

    func.jump(entry, then_b, []);

    assert_eq!(
        before.get(entry).cloned().unwrap_or_default(),
        vec![then_b, else_b],
        "已取出的快照必须保持读它时的内容"
    );
    assert_eq!(succs_of(&func, entry), vec![then_b], "新读必须看到新图");
    assert_eq!(dt_before.idom(else_b), Some(entry), "旧支配树快照同样不变");
}

/// **实测漏洞**：原地墓碑化终结符也改 CFG（块内顺序表不含终结符，所以这是绕过
/// `set_terminator` 的一条改图路），此前不失效缓存 ⇒ 读者拿到旧后继。
#[test]
fn tombstoning_terminator_refreshes_cfg() {
    let (mut func, entry, then_b, else_b) = diamond();
    assert_eq!(succs_of(&func, entry), vec![then_b, else_b]);

    let term = func.dfg.block_terminator(entry).expect("entry 有终结符");
    func.tombstone_inst(term);

    assert!(
        succs_of(&func, entry).is_empty(),
        "终结符被墓碑化后 entry 不应再有后继（块内顺序表不含终结符）"
    );
}

/// 绕过 `Function` 直接改 `dfg` 结构（新建块）同样不许留下旧图。
#[test]
fn raw_dfg_block_creation_refreshes_analysis() {
    let (mut func, entry, then_b, else_b) = diamond();
    let before = func.successors();

    // 缓存已初始化，然后**绕过 Function** 直接建块（不调 invalidate_analysis）
    let fresh_block = func.dfg.make_block();

    let after = func.successors();
    assert!(
        after.get(fresh_block).is_some(),
        "新建块必须出现在新鲜的后继表里（结构修订号由 dfg 自己前进）"
    );
    assert!(
        !Arc::ptr_eq(&before, &after),
        "结构变了，缓存必须重算（同一份快照被复用即为漏检）"
    );
    assert_eq!(
        after.get(entry).cloned().unwrap_or_default(),
        vec![then_b, else_b]
    );
}

/// `kill_inst` 掉终结符 → 该块没有后继（新图）。
///
/// 槽位语义：`block_terminator` 返回的是"块上挂着的终结符指令"（删除后仍挂着那条
/// 墓碑指令），而 CFG 解码看 `term_kind`（墓碑 ⇒ `Nop` ⇒ 无终结符）——所以这里断
/// `term_kind` 与后继表，不断言槽位为 `None`。
#[test]
fn killing_terminator_refreshes_cfg() {
    let (mut func, entry, then_b, _else_b) = diamond();
    assert_eq!(succs_of(&func, entry).len(), 2);
    let term = func.dfg.block_terminator(entry).expect("entry 有终结符");
    func.kill_inst(term);
    assert!(succs_of(&func, entry).is_empty());
    assert!(
        func.dfg.term_kind(entry).is_none(),
        "墓碑化的终结符不再是终结符（CFG 解码口径）"
    );
    assert_ne!(then_b, entry);
}

/// `retarget_terminator` 换目标 → 后继跟着换（去重保序）。
#[test]
fn retarget_terminator_refreshes_cfg() {
    let (mut func, entry, then_b, else_b) = diamond();
    assert_eq!(succs_of(&func, entry), vec![then_b, else_b]);
    func.retarget_terminator(entry, else_b, then_b);
    assert_eq!(
        succs_of(&func, entry),
        vec![then_b],
        "两个分支都指向 then_b（去重）"
    );
}

/// 反面：**不改控制流**的写不该作废 CFG 缓存（否则每次写指令都要重算分析）。
#[test]
fn non_cfg_write_reuses_cache() {
    let (mut func, _entry, _then_b, _else_b) = diamond();
    let before = func.successors();
    let dt_before = func.dominator_tree();

    // 直写一条常量指令（不走 Function 的指令创建口，纯指令层写入）
    let b = func.entry();
    let ty = TypeId::I32;
    let _ = func.dfg.make_inst(
        Opcode::Iconst,
        b,
        smallvec::SmallVec::new(),
        smallvec::smallvec![forge_ir::Immediate::Const(func.constants.insert_int(3, 32))],
        &[ty],
        InstFlags::NONE,
    );

    assert!(
        Arc::ptr_eq(&before, &func.successors()),
        "指令层写入不改变结构修订号 ⇒ 后继缓存应复用"
    );
    assert!(
        Arc::ptr_eq(&dt_before, &func.dominator_tree()),
        "支配树缓存同样应复用"
    );
}

/// `invalidate_analysis()` 仍然可用（显式提前释放），且不影响正确性。
#[test]
fn explicit_invalidate_keeps_analysis_correct() {
    let (mut func, entry, then_b, else_b) = diamond();
    let before = func.successors();
    func.invalidate_analysis();
    let after = func.successors();
    assert!(!Arc::ptr_eq(&before, &after), "显式失效必须让下一次读重算");
    assert_eq!(succs_of(&func, entry), vec![then_b, else_b]);
}
