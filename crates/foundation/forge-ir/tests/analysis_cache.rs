//! 分析缓存一致性守卫（v3 S6：重算 CFG/支配树并比对）。
//!
//! `Function::{predecessors, successors, dominator_tree}` 是 `OnceLock` 惰性缓存：
//! 只要有人改了控制流却忘了失效缓存，后续读者就会拿旧 CFG/旧支配树算出"看起来
//! 合理但错误"的结果，而且**没有任何测试会失败**。
//!
//! 两面一起钉：
//!
//! - **写完自动失效**：`Function` 的改控制流写入口（`jump`/`branch`/`ret`/`switch`/
//!   `unreachable`/`invoke`/`resume`/`retarget_terminator`/`kill_inst`）内部调
//!   `invalidate_analysis()`，所以"先读缓存、再改 CFG、再读"必须看到新 CFG
//!   （此前只有 `forge-opt` 的 8 处 pass 自己记得失效）。
//! - **校验器兜底**：`Verifier` 的 `AnalysisCacheStale` 现场重算 CFG 与支配树，
//!   与已初始化的缓存逐块比对——抓到绕过 `Function` 直接改 `dfg` 的漏网。

use forge_ir::builder::FunctionBuilder;
use forge_ir::verify::{Verifier, VerifyError};
use forge_ir::{Block, InstFlags, Opcode, TypeId, Value};

/// 两个块 + 一个条件分支：`entry: br c, then, else`（三块都 return）。
fn diamond() -> (forge_ir::Function, Block, Block, Block) {
    let ctx = forge_ir::types::TypeContext::new();
    let sig = forge_ir::types::FunctionSignature::new(&[], &[ctx.i32_ty()]);
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

/// 缓存读在前、改 CFG 在后：`successors` 必须反映新边（写入口自动失效）。
#[test]
fn cfg_write_entries_invalidate_successors_cache() {
    let (mut func, entry, then_b, else_b) = diamond();
    // 先初始化缓存
    assert_eq!(
        func.successors().get(entry).cloned().unwrap_or_default(),
        vec![then_b, else_b]
    );
    assert_eq!(
        func.predecessors().get(then_b).cloned().unwrap_or_default(),
        vec![entry]
    );
    let _ = func.dominator_tree(); // 初始化支配树缓存

    // 无条件跳到 then_b（else_b 变成不可达）
    func.jump(entry, then_b, []);

    assert_eq!(
        func.successors().get(entry).cloned().unwrap_or_default(),
        vec![then_b],
        "改 CFG 后缓存必须已失效"
    );
    assert!(
        func.predecessors()
            .get(else_b)
            .map(|v| v.is_empty())
            .unwrap_or(true),
        "else 块不应再有前驱"
    );
    assert_eq!(func.dominator_tree().idom(else_b), None, "不可达块无 idom");
}

/// 校验器兜底：手工塞进陈旧缓存必须被 `AnalysisCacheStale` 抓到。
#[test]
fn verifier_reports_stale_successors_cache() {
    let (mut func, entry, _then_b, _else_b) = diamond();
    // 真缓存（含两条边）之后，手工用"空后继表"覆盖缓存（模拟漏失效）
    let stale = func.successors().clone();
    func.invalidate_analysis();
    let mut wrong: forge_ir::entity_map::SecondaryMap<Block, Vec<Block>> =
        forge_ir::entity_map::SecondaryMap::new();
    wrong.insert(entry, Vec::new());
    func.analysis_mut()
        .successors
        .set(wrong)
        .expect("缓存未初始化");

    let mut verifier = Verifier::with_ctx(func.types.clone());
    let errs = verifier.verify(&func).expect_err("陈旧缓存必须上报");
    assert!(
        errs.iter().any(|e| matches!(
            e,
            VerifyError::AnalysisCacheStale { what, .. } if what == "successors"
        )),
        "应报 successors 陈旧：{errs:?}\n(真缓存={:?})",
        stale.get(entry)
    );
}

/// 校验器兜底：陈旧支配树同样要报（改 CFG 后把旧树塞回缓存）。
#[test]
fn verifier_reports_stale_dominator_tree() {
    let (mut func, entry, then_b, _else_b) = diamond();
    let stale_tree = forge_ir::analysis::DominatorTree::build(&func).clone();
    // 改 CFG（自动失效）→ 再把改之前的树塞回去
    func.jump(entry, then_b, []);
    func.analysis_mut()
        .dominator_tree
        .set(stale_tree)
        .expect("缓存已失效");

    let mut verifier = Verifier::with_ctx(func.types.clone());
    let errs = verifier.verify(&func).expect_err("陈旧支配树必须上报");
    assert!(
        errs.iter().any(|e| matches!(
            e,
            VerifyError::AnalysisCacheStale { what, .. } if what == "dominator_tree"
        )),
        "应报 dominator_tree 陈旧：{errs:?}"
    );
}

/// 没有改 CFG 时不该误报（正常函数 + 已初始化缓存 → 校验通过）。
#[test]
fn fresh_cache_is_not_reported() {
    let (mut func, _entry, _then_b, _else_b) = diamond();
    // 初始化全部缓存后加点无害指令（不改 CFG）
    let _ = func.successors();
    let _ = func.predecessors();
    let _ = func.dominator_tree();
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
    let _: Vec<Value> = Vec::new();

    let mut verifier = Verifier::with_ctx(func.types.clone());
    let outcome = verifier.verify(&func);
    assert!(outcome.is_ok(), "新鲜缓存不应报陈旧：{outcome:?}");
}
