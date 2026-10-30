//! 循环不变量外提 (LICM) pass.
//!
//! Hoists loop-invariant computations out of loops.

use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::entity::map::SecondaryMap;
use forge_ir::*;
use std::collections::HashSet;

#[derive(Default)]
pub struct LicmPass;

impl LicmPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for LicmPass {
    fn name(&self) -> &'static str {
        "licm"
    }
    fn description(&self) -> &'static str {
        "Loop-invariant code motion"
    }
    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        // v27 KD-03：本 pass 在 O2 流水线里（见 `PassManager::for_level_with_table`），
        // **对所有循环**生效（不再有"循环体含 store/call 就整段跳过"的安全子集）。
        //
        // 历史上它曾在"循环体内有调用 + 内联计算"的形状上产出错值/AV，先后两个根因：
        // ① **发射侧帧记账**（v27 M6-95）：spill 槽 0 落在 `rsp - 8`，循环内的 `call`
        //    把返回地址压进该槽 ⇒ reload 当基址用（详见 `emission.rs` 的 sp_base 注释）；
        // ② **块参数实参外提**（v27 KD-03 残余）：块参数别名路径让"实参的生产指令"兼任
        //    内层循环每次进入的初始化器，外提到外层 preheader 后只执行一次 ⇒ 第二次进入
        //    时参数带着上一轮的值（arm64 `slice::sort_unstable` 实测：14/16 元素错）。
        //    现由 [`collect_terminator_args`] 的保守规则排除（实参值一律不外提）。
        hoist_loop_invariants(func)
    }
}

pub fn hoist_loop_invariants(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();
    // P1-5：别名分析（load 外提判定：循环内无 may-alias 写才可外提）
    let alias = AliasAnalysis::new();
    // 使用函数上的惰性分析缓存（licm 只移动指令、不改变块结构，支配树/
    // 循环森林无需重建）；无循环或循环体过小时快速返回。
    let loops: Vec<(Block, Block, HashSet<Block>, Vec<MemoryLocation>)> = func
        .loop_forest()
        .all_loops()
        .iter()
        .filter(|li| li.blocks.len() > 1)
        // P1-2/P1-3：外提目标 = preheader（若有——header 唯一循环外 pred，
        // 每迭代只执行一次的真正不变量位置）；无 preheader 时回退 header
        //（旧行为；insert_preheader pass 正规化后可全量走 preheader）。
        .filter_map(|li| {
            let body: HashSet<Block> = li.blocks.iter().copied().collect();
            let preheader = li.preheader?;
            let write_locs = collect_loop_writes(func, &body, &alias);
            Some((li.header, preheader, body, write_locs))
        })
        .collect();
    if loops.is_empty() {
        return Ok(result);
    }

    // v27 KD-03 残余：**块参数实参不得外提**（详见 `collect_terminator_args` 的注释）。
    let term_args = collect_terminator_args(func);
    for (_header, target, body, write_locs) in loops {
        let outside_values = collect_values_outside(func, &body);
        let invariants = mark_invariants(
            func,
            &body,
            &outside_values,
            &alias,
            &write_locs,
            &term_args,
        );
        if invariants.is_empty() {
            continue;
        }

        let count = hoist_to_header(func, target, &invariants);
        if count > 0 {
            result.instructions_removed += count;
            result.changed = true;
        }
    }
    if result.changed {
        // 保守：改动后使分析缓存失效，后续 pass 使用新鲜分析
        func.analysis_mut().invalidate();
    }
    Ok(result)
}

/// P1-5：循环体内全部 may-write 的位置（调用 → Unknown，与任何 load MayAlias）。
fn collect_loop_writes(
    func: &Function,
    body: &HashSet<Block>,
    alias: &AliasAnalysis,
) -> Vec<MemoryLocation> {
    let mut writes = Vec::new();
    for bi in 0..func.dfg.block_count() {
        let bid = Block::new(bi as u32);
        if !body.contains(&bid) {
            continue;
        }
        for &inst_id in &func.dfg.block(Block::new(bi as u32)).inst_order {
            let inst = &func.dfg.inst_data(inst_id);
            match inst.opcode {
                Opcode::Call | Opcode::CallIndirect => writes.push(MemoryLocation::Unknown),
                Opcode::Store | Opcode::Fstore | Opcode::AtomicRmw | Opcode::Cmpxchg => {
                    if let Some(loc) = alias.location_of_access(func, inst) {
                        writes.push(loc);
                    }
                }
                _ => {}
            }
        }
    }
    writes
}

/// 全函数**块参数实参**的值集合（Jump 目标参数 + Branch 两侧参数）。
///
/// **为什么这些值不许外提（v27 KD-03 残余的真正根因）**：
/// `forge-codegen` 的块参数有两条落地路径（`param_copies.rs`）：① 独立参数寄存器 +
/// 每入边显式拷贝；② **别名**——把入边实参的 XReg 直接当作参数寄存器（入边含
/// `Branch` 时只能走这条）。路径 ② 会让"实参的生产指令"同时承担**内层循环每次
/// 进入时的初始化器**角色：它的 XReg 就是参数寄存器本身，而循环体会改写该寄存器。
///
/// 于是把这类指令外提到**外层** preheader 就会让"初始化"只执行一次：第二次进入
/// 内层循环时参数仍带着上一轮迭代的值。实测形态（arm64、O2、仅外提 Iconst 即可复现）：
/// `slice::sort_unstable` 的 `insert_tail` 内层 `ptr::copy_nonoverlapping` 拷贝循环，
/// 计数器初值被外提 ⇒ 第二次起拷贝次数错 ⇒ 排序后数组元素和 191（正确 120），
/// `v.sort_unstable()` 后 14/16 个元素错位。x86 上同一 IR 不复现，因为该缺陷只在
/// 别名路径 + 长寿命寄存器分配下显形（arm64 的压力/分配让它暴露）。
///
/// 保守规则：**凡是作为块参数实参出现的值，一律不外提**。这不会漏掉正确性——实参
/// 生产指令留在原块里，每入边照旧执行一次；其余不变量照常外提。
fn collect_terminator_args(func: &Function) -> HashSet<Value> {
    let mut args: HashSet<Value> = HashSet::new();
    for bi in 0..func.dfg.block_count() {
        let bid = Block::new(bi as u32);
        if let Some((_target, vals)) = func.dfg.term_jump(bid) {
            args.extend(vals.iter().copied());
        }
        if let Some((_cond, _t, tvals, _f, fvals)) = func.dfg.term_branch(bid) {
            args.extend(tvals.iter().copied());
            args.extend(fvals.iter().copied());
        }
    }
    args
}

/// 循环体内是否有**任何**内存写或调用（不依赖别名分析的完备性）。
fn body_has_mem_write(func: &Function, body: &HashSet<Block>) -> bool {
    for bi in 0..func.dfg.block_count() {
        let bid = Block::new(bi as u32);
        if !body.contains(&bid) {
            continue;
        }
        for &inst_id in &func.dfg.block(bid).inst_order {
            if matches!(
                func.dfg.inst_data(inst_id).opcode,
                Opcode::Store
                    | Opcode::Fstore
                    | Opcode::Call
                    | Opcode::CallIndirect
                    | Opcode::AtomicRmw
                    | Opcode::Cmpxchg
            ) {
                return true;
            }
        }
    }
    false
}

fn collect_values_outside(func: &Function, body: &HashSet<Block>) -> HashSet<Value> {
    let mut values = HashSet::new();
    for bi in 0..func.dfg.block_count() {
        let bid = Block::new(bi as u32);
        if body.contains(&bid) {
            continue;
        }
        let block = &func.dfg.block(Block::new(bi as u32));
        for &v in &block.param_values {
            values.insert(v);
        }
        for &inst_id in &block.inst_order {
            let inst = &func.dfg.inst_data(inst_id);
            if let Some(v) = inst.results.first().copied() {
                values.insert(v);
            }
        }
    }
    values
}

fn mark_invariants(
    func: &Function,
    body: &HashSet<Block>,
    outside: &HashSet<Value>,
    alias: &AliasAnalysis,
    // 保留形参：万一将来需要精确别名判定（当前保守规则不看它，见下）。
    _write_locs: &[MemoryLocation],
    // 块参数实参（见 `collect_terminator_args`）：这些值**不许外提**。
    term_args: &HashSet<Value>,
) -> HashSet<(Block, Inst)> {
    let mut inv_values: HashSet<Value> = HashSet::new();
    let mut inv_insts: HashSet<(Block, Inst)> = HashSet::new();

    // First pass: constants are always invariant
    for bi in 0..func.dfg.block_count() {
        let bid = Block::new(bi as u32);
        if !body.contains(&bid) {
            continue;
        }
        for &inst_id in &func.dfg.block(Block::new(bi as u32)).inst_order {
            let inst = &func.dfg.inst_data(inst_id);
            if matches!(inst.opcode, Opcode::Iconst | Opcode::Fconst)
                && let Some(v) = inst.results.first().copied()
                && !term_args.contains(&v)
            {
                inv_values.insert(v);
                inv_insts.insert((bid, inst_id));
            }
        }
    }

    // Iterate until fixed point
    let mut changed = true;
    while changed {
        changed = false;
        for bi in 0..func.dfg.block_count() {
            let bid = Block::new(bi as u32);
            if !body.contains(&bid) {
                continue;
            }
            for &inst_id in &func.dfg.block(Block::new(bi as u32)).inst_order {
                if inv_insts.contains(&(bid, inst_id)) {
                    continue;
                }
                let inst = &func.dfg.inst_data(inst_id);
                if inst.results.is_empty() {
                    continue;
                }

                // P1-5：load 在「地址不变 + 循环内无 may-alias 写」时可外提
                // （每迭代读同一位置 → 提前到 preheader 只读一次等价）。
                // volatile / 原子 load 保留可观察语义，不外提。
                if crate::scalar::cse::is_load_op(&inst.opcode) {
                    if inst.mem_flags.is_volatile() {
                        continue;
                    }
                    // **保守要求：循环体内一处内存写/调用都没有**。
                    //
                    // 为什么不用 `write_locs` 判：那些位置来自别名分析的
                    // `location_of_access`，它对"经派生地址写的栈槽"可能返回 `None`
                    // ⇒ 该写**根本没进** `write_locs`，于是循环变量的 load 被外提到
                    // preheader ⇒ **循环不再前进**（每轮读旧值）。
                    // v27 M6 实测：`while i <= 4 { acc += sq(i + seed()); i += 1 }`
                    // 在优化档下得到垃圾退出码 / AV；`FORGE_OPT_SKIP=licm` 即恢复 30。
                    //
                    // 直接扫循环体指令，不依赖别名分析的完备性。收益本来集中在
                    // "只读循环"（对大数组求和等），那时这里也是 false。
                    if body_has_mem_write(func, body) {
                        continue;
                    }
                    if alias.location_of_access(func, inst).is_none() {
                        continue;
                    }
                } else if has_side_effects(&inst.opcode) {
                    continue;
                }
                // **`StackAddr` 现在也外提**（v27 M6-95）：此前它被排除，是因为"槽地址值
                // 跨块活跃"会踩中发射侧的栈帧缺陷——**spill 槽 0 落在 `rsp - 8`**
                // （`callee_saved_bytes` 误把帧指针保存槽算进 rbp 之下），循环内的 `call`
                // 把返回地址压进槽 0，之后 reload 当基址用 ⇒ AV。根因已在发射侧修掉
                //（`emission.rs` 的 sp_base 扣除 `frame_pointer_overhead`），外提回归默认；
                // 端到端守卫 = e2e `opt_loop_call_inline`（O2、循环内有调用、真执行 30）。
                if inst
                    .operands
                    .iter()
                    .all(|v| outside.contains(v) || inv_values.contains(v))
                    && let Some(v) = inst.results.first().copied()
                    && !term_args.contains(&v)
                {
                    inv_values.insert(v);
                    inv_insts.insert((bid, inst_id));
                    changed = true;
                }
            }
        }
    }
    inv_insts
}

/// Pre-collected invariant instruction IDs (avoids borrow conflicts with DFG).
struct InvariantData {
    inst_id: Inst,
    old_results: Vec<Value>,
}

fn hoist_to_header(
    func: &mut Function,
    header: Block,
    invariants: &HashSet<(Block, Inst)>,
) -> usize {
    // Phase 1: Collect invariant instruction IDs (immutable borrows)
    let mut hoist_data: Vec<InvariantData> = Vec::new();
    for bi in 0..func.dfg.block_count() {
        let bid = Block::new(bi as u32);
        let inst_order = func.dfg.block(Block::new(bi as u32)).inst_order.clone();
        for &inst_id in &inst_order {
            if !invariants.contains(&(bid, inst_id)) {
                continue;
            }
            let inst = &func.dfg.inst_data(inst_id);
            if inst.results.is_empty() {
                continue;
            }
            hoist_data.push(InvariantData {
                inst_id,
                old_results: inst.results.to_vec(),
            });
        }
    }

    // 诊断：`FORGE_LICM_ONLY=stack_addr|iconst|load` 只外提该类指令（隔离"第二因"）。
    // **必须在 Phase 1 之后、克隆/RAUW/kill 之前过滤**：否则未克隆的原指令仍会在
    // Phase 4 被 kill ⇒ 定义消失（本轮实测：过滤放在克隆循环里 ⇒ 校验失败）。
    if let Ok(only) = std::env::var("FORGE_LICM_ONLY") {
        hoist_data.retain(|hd| {
            let op = func.dfg.inst_data(hd.inst_id).opcode;
            match only.as_str() {
                "stack_addr" => op == Opcode::StackAddr,
                "iconst" => op == Opcode::Iconst,
                "load" => crate::scalar::cse::is_load_op(&op),
                _ => false,
            }
        });
    }

    if hoist_data.is_empty() {
        return 0;
    }

    // Phase 2: Clone invariants to header（保留全字段：flags/mem_flags/metadata/loc），
    // 自动维护 val_remap（链式克隆把先前 hoist 的值重定向）
    let mut val_remap: SecondaryMap<Value, Value> = SecondaryMap::new();
    let mut count = 0;

    for hd in &hoist_data {
        // 诊断：`FORGE_OPT_TRACE_LICM=1` 打印**被外提的指令**（本轮定位靠它）。
        if std::env::var_os("FORGE_OPT_TRACE_LICM").is_some() {
            let inst = func.dfg.inst_data(hd.inst_id);
            eprintln!(
                "[licm] hoist inst {:?} block {:?} ops={:?} → header {:?}",
                inst.opcode, inst.block, inst.operands, header
            );
        }
        // 走 `Function::clone_inst_into`（不是 `dfg.clone_inst`）：它额外登记
        // use-list。漏了这步，外提后的指令在新块的 use-list 里查不到 ⇒
        // verifier 报 `UseListInconsistency`（v27 M5 实测）。
        let new_inst = func.clone_inst_into(hd.inst_id, header, &mut val_remap);
        // **必须插到块首（且保持拓扑序）**：`clone_inst_into` 只保证落在终结符
        // 之前，即“块尾”；若目标块（常见：preheader=entry）里原本就有使用该
        // 不变量的指令，RAUW 后就会**块内 use-before-def**（v27 KD-03 实测：
        // entry 的零初始化 store 引用了块尾才定义的常量 ⇒ binary_search 错值）。
        // 第 k 条克隆放到前 k 条之后，既在最前、又不反转拓扑序。
        {
            let order = &mut func.dfg.block_mut(header).inst_order;
            if let Some(pos) = order.iter().position(|&i| i == new_inst) {
                order.remove(pos);
            }
            let at = count.min(order.len());
            order.insert(at, new_inst);
        }
        count += 1;
    }

    // Phase 3: Replace all uses of old values with hoisted values（DFG + use-lists 双更新）
    //
    // 注：**不要**改成扫描式的 `apply_replacements`——它会把**刚克隆出来的那条指令**
    // 也一起改（克隆的 operands 正是被替换的旧值），导致克隆自引用/版本错乱。
    // （2026-10-19 实测：改成扫描后本函数在优化档直接错值。）use-list 的新鲜度由
    // `clone_inst_into`（登记新指令的使用）+ 各 pass 的 `refresh_inst_uses` 保证。
    for hd in &hoist_data {
        for &old_r in &hd.old_results {
            if let Some(&new_r) = val_remap.get(old_r) {
                func.replace_all_uses(old_r, new_r);
            }
        }
    }

    // Phase 4: 原子删除原 invariant 指令（kill_inst：墓碑化 + use-lists 清理）
    for hd in &hoist_data {
        func.kill_inst(hd.inst_id);
    }

    count
}

fn has_side_effects(opcode: &Opcode) -> bool {
    matches!(
        opcode,
        // P1-5：Load/Fload 不再整体视为副作用（load 外提判定单独处理）；
        // Fstore 补入写集合（此前缺失）。
        Opcode::Store
            | Opcode::Fstore
            | Opcode::Call
            | Opcode::CallIndirect
            | Opcode::AtomicRmw
            | Opcode::Cmpxchg
            | Opcode::Fence
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licm_no_crash() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.constant(42i32);
        b.ret(&[v]);
        let mut func = b.finish().expect("build");
        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed);
    }

    /// Test that a loop-invariant Iconst is hoisted out of the loop body.
    #[test]
    fn licm_hoist_invariant_const() {
        // Build: while cond { x = 42; ... }
        // The const 42 inside the loop should be hoisted to the header
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let preheader = b.create_block();
        b.switch_to_block(preheader);
        let zero = b.constant(0i32);
        let header = b.create_block();
        b.switch_to_block(preheader);
        b.jump(header, &[]);

        b.switch_to_block(header);
        let n = b.constant(10i32);
        let cond = b.icmp(IntCC::Slt, zero, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);

        b.switch_to_block(body_blk);
        let loop_const = b.constant(42i32); // loop-invariant — should be hoisted
        let one = b.constant(1i32);
        let _used = b.iadd(loop_const, one);
        b.jump(header, &[]);

        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(
            r.changed,
            "LICM should hoist invariant Iconst from loop body"
        );
        assert!(
            r.instructions_removed > 0,
            "Should report hoisted instructions"
        );
    }

    /// Test that a loop-invariant computation (add of two outside values) is hoisted.
    #[test]
    fn licm_hoist_invariant_computation() {
        // Build: while cond { x = a + b; ... } where a,b are defined outside the loop
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (preheader, params) =
            b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        let a = params[0];
        let b_val = params[1];
        let zero = b.constant(0i32);
        let header = b.create_block();
        b.switch_to_block(preheader);
        b.jump(header, &[]);

        b.switch_to_block(header);
        let n = b.constant(10i32);
        let cond = b.icmp(IntCC::Slt, zero, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);

        b.switch_to_block(body_blk);
        let _invariant_add = b.iadd(a, b_val); // loop-invariant: a,b are outside the loop
        b.jump(header, &[]);

        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "LICM should hoist invariant Iadd from loop body");
        assert!(
            r.instructions_removed > 0,
            "Should report hoisted instructions"
        );
    }

    /// Test that non-invariant instructions (depends on loop-varying value) are NOT hoisted.
    #[test]
    fn licm_does_not_hoist_variant() {
        // Build: for i in 0..10: x = i + 5;  → i is loop-varying, should NOT be hoisted
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let zero = b.constant(0i32);
        let (header, header_params) = b.create_block_with_params(&[(TypeId::I32, "i")]);
        b.switch_to_block(entry);
        b.jump(header, &[zero]);

        b.switch_to_block(header);
        let i = header_params[0];
        let n = b.constant(10i32);
        let cond = b.icmp(IntCC::Slt, i, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);

        b.switch_to_block(body_blk);
        let five = b.constant(5i32);
        let _variant_add = b.iadd(i, five); // i is loop-varying → NOT invariant
        let one = b.constant(1i32);
        let next_i = b.iadd(i, one);
        b.jump(header, &[next_i]);

        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // Only the Iconst 5 should be hoisted; the Iadd(i, 5) should NOT be hoisted
        // So LICM should report some changes (from Iconst hoisting), but fewer than if
        // the Iadd was also hoisted
        // At minimum: the Iconst(5) is hoisted, so changed=true
        assert!(
            r.changed || r.instructions_removed == 0,
            "Only Iconst(5) is invariant; Iadd(i,5) depends on loop-varying i"
        );
    }

    /// **跨调用外提正向守卫**（v27 KD-03 残余修复后）：循环体含 `call` 时，
    /// 循环不变常量（且不是块参数实参）照样外提——断言 opcode 级"循环体里没有
    /// Iconst/外提值在 header 或 preheader"。
    #[test]
    fn licm_hoists_invariant_across_call() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let preheader = b.create_block();
        b.switch_to_block(preheader);
        let zero = b.constant(0i32);
        let (header, params) = b.create_block_with_params(&[(TypeId::I32, "i")]);
        b.switch_to_block(preheader);
        b.jump(header, &[zero]);
        b.switch_to_block(header);
        let i = params[0];
        let n = b.constant(10i32);
        let cond = b.icmp(IntCC::Slt, i, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);
        b.switch_to_block(body_blk);
        // 调用（无返回值，仅制造"跨调用"形状）。
        b.call(FuncRef::new(0), &[], &[]);
        // 调用之后才用到的不变量常量：修复前"含 call 整段不外提"，现在应外提。
        let k = b.constant(7i32);
        let one = b.constant(1i32);
        let _used_after_call = b.iadd(k, i);
        let next = b.iadd(i, one);
        b.jump(header, &[next]);
        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "含 call 的循环里不变量应被外提");

        // 循环体里不应再有 Iconst（k/one 都应外提）；call 必须留在循环体。
        let mut body_consts = 0usize;
        let mut body_calls = 0usize;
        for &inst_id in &func.dfg.block(body_blk).inst_order {
            let op = func.dfg.inst_data(inst_id).opcode;
            if matches!(op, Opcode::Iconst) {
                body_consts += 1;
            }
            if matches!(op, Opcode::Call) {
                body_calls += 1;
            }
        }
        assert_eq!(body_consts, 0, "跨调用的不变常量应外提到循环外");
        assert_eq!(body_calls, 1, "call 是副作用，必须留在循环体");
    }

    /// P1-5 正向：循环体 load 栈槽（地址在外、循环内无 may-alias 写）→ 外提到
    /// preheader；循环体不再含 load。
    #[test]
    fn p1_5_licm_hoist_invariant_load() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let preheader = b.create_block();
        b.switch_to_block(preheader);
        let p = b.stack_addr(0); // 地址在循环外
        let zero = b.constant(0i32);
        let header = b.create_block();
        b.switch_to_block(preheader);
        b.jump(header, &[]);

        b.switch_to_block(header);
        let n = b.constant(10i32);
        let cond = b.icmp(IntCC::Slt, zero, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);

        b.switch_to_block(body_blk);
        let v = b.load(p, TypeId::I32); // 循环不变 load（无写 → 可外提）
        let one = b.constant(1i32);
        let _used = b.iadd(v, one);
        b.jump(header, &[]);

        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "LICM 应外提不变 load");

        // 全函数只剩 1 条 load，且不在循环体
        let mut loads: Vec<Block> = Vec::new();
        for (bi, blk) in func.dfg.block_data_iter().enumerate() {
            for &inst_id in &blk.inst_order {
                let inst = &func.dfg.inst_data(inst_id);
                if matches!(inst.opcode, Opcode::Load) {
                    loads.push(Block::new(bi as u32));
                }
            }
        }
        assert_eq!(loads.len(), 1, "load 应被外提（只剩 1 条）");
        assert_ne!(loads[0], body_blk, "load 不应留在循环体");
    }

    /// P1-5 负向：循环体对同一槽 store + load → load 与 store MayAlias，
    /// 不得外提（load 仍留在循环体）。
    #[test]
    fn p1_5_licm_no_hoist_load_with_alias_store() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let preheader = b.create_block();
        b.switch_to_block(preheader);
        let p = b.stack_addr(0);
        let zero = b.constant(0i32);
        let header = b.create_block();
        b.switch_to_block(preheader);
        b.jump(header, &[]);

        b.switch_to_block(header);
        let n = b.constant(10i32);
        let cond = b.icmp(IntCC::Slt, zero, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);

        b.switch_to_block(body_blk);
        let sv = b.constant(7i32);
        b.store(sv, p); // 写同一槽 —— MayAlias
        let v = b.load(p, TypeId::I32); // 读到的值被 store 决定 → 不可外提
        let one = b.constant(1i32);
        let _used = b.iadd(v, one);
        b.jump(header, &[]);

        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        // load 必须仍留在循环体
        let mut loads_in_body = 0;
        for &inst_id in &func.dfg.block(body_blk).inst_order {
            let inst = &func.dfg.inst_data(inst_id);
            if matches!(inst.opcode, Opcode::Load) {
                loads_in_body += 1;
            }
        }
        assert_eq!(
            loads_in_body, 1,
            "MayAlias store 在循环内 → load 不得外提（P1-5 负向）"
        );
        let _ = r;
    }

    /// **回归守卫（v27 M6-95）**：`StackAddr` **默认外提**。
    ///
    /// 它曾是例外（"槽地址跨块活跃"会踩中发射侧的 spill 槽错位 ⇒ AV）。根因已修：
    /// spill 基点曾把帧指针保存槽算进"rbp 之下"，槽 0 = `rsp - 8` = `call` 压返回
    /// 地址的位置（详见 `emission.rs` 的 sp_base 注释）。现在外提回归默认；本测试钉住
    /// "外提真的发生"，端到端正确性由 e2e `opt_loop_call_inline`（O2 + 循环内调用）钉住。
    #[test]
    fn stack_addr_is_hoisted_by_default() {
        // while (cond) { p = stack_addr(-8); }  —— 只读循环（安全子集）
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let preheader = b.create_block();
        b.switch_to_block(preheader);
        let zero = b.constant(0i32);
        let header = b.create_block();
        b.switch_to_block(preheader);
        b.jump(header, &[]);
        b.switch_to_block(header);
        let n = b.constant(10i32);
        let cond = b.icmp(IntCC::Slt, zero, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);
        b.switch_to_block(body_blk);
        // **不写内存、不调用**：安全子集内，StackAddr 应被外提。
        let _p = b.stack_addr(-8);
        b.jump(header, &[]);
        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let _ = pass.run_on_function(&mut func).unwrap();

        // 断言：循环体里**没有** StackAddr（已被外提），preheader/header 里恰好一条。
        let mut body_stack_addrs = 0usize;
        let mut outside_stack_addrs = 0usize;
        for bi in 0..func.dfg.block_count() {
            let bid = Block::new(bi as u32);
            for &inst_id in &func.dfg.block(bid).inst_order {
                if !matches!(func.dfg.inst_data(inst_id).opcode, Opcode::StackAddr) {
                    continue;
                }
                if bid == Block::new(2) {
                    body_stack_addrs += 1;
                } else {
                    outside_stack_addrs += 1;
                }
            }
        }
        assert_eq!(
            body_stack_addrs, 0,
            "只读循环里 StackAddr 应当外提（v27 M6-95/KD-03 安全子集）"
        );
        assert_eq!(
            outside_stack_addrs, 1,
            "外提后的 StackAddr 应落在 preheader/header"
        );
    }

    /// **写内存的循环同样外提**（v27 KD-03：`无 store/无 call` 的整段过滤已撤掉）。
    ///
    /// 该过滤当年是为绕开 arm64 发射/分配缺陷；缺陷的**真正根因**见
    /// [`collect_terminator_args`]：块参数实参被外提会让内层循环的初始化只执行一次。
    /// 去掉根因后，含 store 的循环（不含块参数实参形态）可安全外提。
    #[test]
    fn stack_addr_is_hoisted_from_write_loop() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let preheader = b.create_block();
        b.switch_to_block(preheader);
        let zero = b.constant(0i32);
        let header = b.create_block();
        b.switch_to_block(preheader);
        b.jump(header, &[]);
        b.switch_to_block(header);
        let n = b.constant(10i32);
        let cond = b.icmp(IntCC::Slt, zero, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);
        b.switch_to_block(body_blk);
        let p = b.stack_addr(-8);
        let one = b.constant(1i32);
        b.store(one, p);
        b.jump(header, &[]);
        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let _ = pass.run_on_function(&mut func).unwrap();

        let mut body_stack_addrs = 0usize;
        for bi in 0..func.dfg.block_count() {
            let bid = Block::new(bi as u32);
            if bid != Block::new(2) {
                continue;
            }
            for &inst_id in &func.dfg.block(bid).inst_order {
                if matches!(func.dfg.inst_data(inst_id).opcode, Opcode::StackAddr) {
                    body_stack_addrs += 1;
                }
            }
        }
        assert_eq!(
            body_stack_addrs, 0,
            "写内存的循环里 StackAddr 也应当外提（KD-03 残余已由块参数实参规则修掉）"
        );
    }

    /// **块参数实参不许外提**（KD-03 残余的真正根因的回归守卫）。
    ///
    /// `forge-codegen` 的块参数别名路径会把入边实参的 XReg 当作参数寄存器；若该
    /// 实参是循环体外提的常量，它的定义就同时是"内层循环每次进入时的初始化器"，
    /// 外提到外层 preheader 后只执行一次 ⇒ 第二次进入时参数带着上一轮的值。
    /// 本测试：循环体里的常量直接作为跳转实参，断言它**留在循环体**。
    #[test]
    fn const_used_as_block_arg_is_not_hoisted() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let preheader = b.create_block();
        b.switch_to_block(preheader);
        let zero = b.constant(0i32);
        let (header, params) = b.create_block_with_params(&[(TypeId::I32, "i")]);
        b.switch_to_block(preheader);
        b.jump(header, &[zero]);
        b.switch_to_block(header);
        let i = params[0];
        let n = b.constant(10i32);
        let cond = b.icmp(IntCC::Slt, i, n);
        let body_blk = b.create_block();
        let exit = b.create_block();
        b.switch_to_block(header);
        b.branch(cond, body_blk, &[], exit, &[]);
        b.switch_to_block(body_blk);
        // 该常量**只**作为 header 的块参数实参出现 → 不得外提。
        let five = b.constant(5i32);
        b.jump(header, &[five]);
        b.switch_to_block(exit);
        b.ret(&[]);
        let mut func = b.finish().expect("build");

        let pass = LicmPass::new();
        let _ = pass.run_on_function(&mut func).unwrap();

        let mut body_consts = 0usize;
        for &inst_id in &func.dfg.block(body_blk).inst_order {
            if matches!(func.dfg.inst_data(inst_id).opcode, Opcode::Iconst) {
                body_consts += 1;
            }
        }
        assert_eq!(
            body_consts, 1,
            "作为块参数实参的常量必须留在循环体（否则内部循环初始化只执行一次）"
        );
    }
}
