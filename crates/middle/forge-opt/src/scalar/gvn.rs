//! 全局值编号 (GVN) pass。
//!
//! 使用支配树进行跨基本块的公共子表达式消除。
//! 如果一条指令与支配树中某个祖先块的指令完全等价（相同操作码、相同操作数、相同类型），
//! 则可以用祖先的结果替换当前结果。
//!
//! # 特性
//!
//! - **全局 CSE**: 跨基本块消除冗余计算（支配树作用域栈）
//! - **常量折叠**: GVN 过程中自动简化纯常量操作
//! - **Memory Load GVN**: 相同地址的 load 在无中间 store/kill 时被消除
//! - **交换律**: 自动规范化交换律操作的参数顺序
//!
//! # 示例
//!
//! ```ignore
//! // Before GVN:
//! entry:  v2 = iadd v0, v1     // a+b
//!         branch cond, then, else
//! then:   v3 = iadd v0, v1     // duplicate!
//! else:   v5 = iadd v0, v1     // duplicate!
//!
//! // After GVN:
//! entry:  v2 = iadd v0, v1
//!         branch cond, then, else
//! then:   v4 = imul v2, v2     // v3 → v2
//! else:   v6 = imul v2, v2     // v5 → v2
//! ```

use super::cse::ExprKey;
use crate::{ConstValue, OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;
use std::collections::HashMap;

/// 全局值编号 pass。
#[derive(Default)]
pub struct GvnPass;

impl GvnPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for GvnPass {
    fn name(&self) -> &'static str {
        "gvn"
    }

    fn description(&self) -> &'static str {
        "Global value numbering: eliminates duplicate computations across blocks using dominator tree, with constant folding and memory load GVN"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        global_value_numbering(func)
    }
}

/// 对单个函数执行全局值编号（含常量折叠和内存 load GVN）。
pub fn global_value_numbering(func: &mut Function) -> Result<PassResult, IrError> {
    let n = func.dfg.blocks.len();
    if n == 0 {
        return Ok(PassResult::default());
    }

    // Build dominator children map (release dominator tree borrow before DFS)
    let dom_children: HashMap<Block, Vec<Block>> = {
        let dt = func.dominator_tree();
        let mut map = HashMap::new();
        for bi in 0..n {
            let block = Block(bi as u32);
            map.insert(block, dt.children(block).to_vec());
        }
        map
    };

    let mut result = PassResult::default();
    let mut replacements: HashMap<Value, Value> = HashMap::new();
    // 待删除的冗余表达式指令（循环后统一 kill）
    let mut to_kill: Vec<Inst> = Vec::new();
    // Scope stack: top of stack is current block's scope
    let mut scopes: Vec<HashMap<ExprKey, Value>> = vec![];
    // Constant map: Value → (Big value, TypeId)
    let mut const_map: HashMap<Value, (Big, TypeId)> = HashMap::new();

    // DFS from entry block
    let entry_id = func.entry_block.unwrap_or(Block(0));
    // P1-5：别名分析（load 消重的 kill 精度）
    let alias = AliasAnalysis::new();
    gvn_dfs(
        func,
        entry_id,
        &dom_children,
        &mut scopes,
        &mut replacements,
        &mut const_map,
        &mut to_kill,
        &mut result,
        &alias,
    );

    // Apply replacements across all instructions（同步 use-lists）
    if !replacements.is_empty() {
        result.values_replaced += replacements.len();
        func.apply_replacements(&replacements);
    }
    // 原子删除被 GVN 消除的冗余指令
    for inst in to_kill {
        func.kill_inst(inst);
    }

    Ok(result)
}

/// Attempt constant folding for an instruction. If all operands are known constants, evaluate.
/// Returns `Some((big_value, ty))` if fold succeeded.
fn try_const_fold(
    opcode: &Opcode,
    mapped_operands: &[Value],
    const_map: &HashMap<Value, (Big, TypeId)>,
) -> Option<(Big, TypeId)> {
    // 第三十五轮:统一走 const_fold::fold_opcode 权威实现(sccp.rs:277 先例;
    // 原 9 个 opcode 手写求值与 fold_opcode 逐行等价,truncate_to_type/
    // bit_mask 双份 helper 消除)。除零:fold_opcode 返回 Err——统一为 None
    // 保守跳过(与 gvn 原行为一致)。
    let const_ops: smallvec::SmallVec<[ConstValue; 4]> = mapped_operands
        .iter()
        .map(|v| {
            let (big, ty) = const_map.get(v)?;
            Some(ConstValue::Int(big.clone(), *ty))
        })
        .collect::<Option<smallvec::SmallVec<[ConstValue; 4]>>>()?;
    let ty = const_ops.first().map(|c| match c {
        ConstValue::Int(_, t) => *t,
        ConstValue::Float(_, t) => *t,
        ConstValue::Bool(_) => TypeId::I16,
    })?;
    let folded = super::const_fold::fold_opcode(opcode, &const_ops, ty).ok()??;
    match folded {
        ConstValue::Int(v, t) => Some((v, t)),
        ConstValue::Float(v, t) => Some((v, t)),
        ConstValue::Bool(_) => None,
    }
}

/// Look up an expression key across all scopes (top-down).
fn gvn_lookup(scopes: &[HashMap<ExprKey, Value>], key: &ExprKey) -> Option<Value> {
    for scope in scopes.iter().rev() {
        if let Some(&v) = scope.get(key) {
            return Some(v);
        }
    }
    None
}

/// DFS traverse the dominator tree, performing GVN within each block.
#[allow(clippy::too_many_arguments)] // 遍历上下文参数组(dom 栈/替换表/常量表)——内部私有
fn gvn_dfs(
    func: &mut Function,
    block_id: Block,
    dom_children: &HashMap<Block, Vec<Block>>,
    scopes: &mut Vec<HashMap<ExprKey, Value>>,
    replacements: &mut HashMap<Value, Value>,
    const_map: &mut HashMap<Value, (Big, TypeId)>,
    to_kill: &mut Vec<Inst>,
    result: &mut PassResult,
    alias: &AliasAnalysis,
) {
    // 1. Enter block: push new scope
    scopes.push(HashMap::new());

    // 2. Process all instructions in this block
    // inst_ids 借用 blocks（insts 的 &mut 借用与其不冲突——dfg 字段级拆分）
    let inst_ids = &func.dfg.blocks[block_id.0 as usize].inst_order;

    for inst_id in inst_ids {
        // P1-5：只读快照（kill 分支做别名查询时不得持有 dfg.insts 的 &mut）
        let (snap_opcode, snap_result) = {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            (inst.opcode, inst.results.first().copied())
        };
        let inst_result = match snap_result {
            Some(v) => v,
            None => {
                // 无结果指令：内存写 kill load 表达式。
                // P0-4：kill 集合覆盖全部 may-write（Fstore/Call/CallIndirect/
                // AtomicRmw/Cmpxchg），不只 Store。
                // P1-5：store/原子按别名精化（只 kill may-alias load）；
                // 调用可能写任意内存 → kill 全部 load。
                if matches!(snap_opcode, Opcode::Call | Opcode::CallIndirect) {
                    for scope in scopes.iter_mut() {
                        scope.retain(|key, _| !super::cse::is_load_op(&key.opcode));
                    }
                } else if matches!(
                    snap_opcode,
                    Opcode::Store | Opcode::Fstore | Opcode::AtomicRmw | Opcode::Cmpxchg
                ) {
                    let wloc = alias.location_of_access(func, &func.dfg.insts[inst_id.0 as usize]);
                    for scope in scopes.iter_mut() {
                        scope.retain(|key, _| !super::cse::killed_by_write(key, wloc, alias, func));
                    }
                }
                continue;
            }
        };

        let inst = &mut func.dfg.insts[inst_id.0 as usize];
        let ty = func.dfg.values[inst_result.0 as usize].ty;

        // Handle Iconst / Fconst: record in constant map
        match &inst.opcode {
            Opcode::Iconst => {
                if let Some(cid) = inst.immediates.first().and_then(|i| i.as_const())
                    && let Some(big) = func.constants.resolve_big(cid)
                {
                    const_map.insert(inst_result, (big, ty));
                }
                continue;
            }
            Opcode::Fconst => {
                if let Some(cid) = inst.immediates.first().and_then(|i| i.as_const())
                    && let Some(big) = func.constants.resolve_big(cid)
                {
                    const_map.insert(inst_result, (big, ty));
                }
                continue;
            }
            _ => {}
        }

        // P0-6：多结果指令（overflow 系）不进消重表——只映射 results[0]，
        // results[1]（flag）会悬空指向被 kill 的指令。
        if inst.results.len() > 1 {
            continue;
        }

        // P0-3：volatile load 不参与 GVN（可观察语义）。
        if super::cse::is_load_op(&inst.opcode)
            && inst
                .mem_flags
                .contains(forge_ir::mem_flags::MemFlags::VOLATILE)
        {
            continue;
        }

        // Skip non-GVN-able instructions（P1-5：load 在 kill 精化下可消重）
        if !super::cse::is_cse_candidate(&inst.opcode) && !super::cse::is_load_op(&inst.opcode) {
            continue;
        }

        // Compute expression key (operands already mapped to replacement values)
        let mapped_operands: smallvec::SmallVec<[Value; 4]> = inst
            .operands
            .iter()
            .map(|v| replacements.get(v).copied().unwrap_or(*v))
            .collect();

        // === Constant folding ===
        if !super::cse::is_load_op(&inst.opcode)
            && let Some((folded_val, folded_ty)) =
                try_const_fold(&inst.opcode, &mapped_operands, const_map)
        {
            // Replace current instruction with a constant
            let cid = func.constants.insert_big(folded_val.clone());
            inst.opcode = Opcode::Iconst;
            inst.operands.clear();
            inst.immediates.clear();
            inst.immediates.push(Immediate::Const(cid));
            func.dfg.values[inst_result.0 as usize].ty = folded_ty;
            const_map.insert(inst_result, (folded_val, folded_ty));
            result.instructions_removed += 1;
            result.changed = true;
            continue;
        }

        let key = super::cse::expr_key(&inst.opcode, &mapped_operands, ty);

        // === Memory Load GVN ===（P1-5：Load/Fload——kill 精化后真正生效；
        // 跨块复用前提：地址为同一 SSA 值且支配，且无中间 may-alias 写）
        if super::cse::is_load_op(&inst.opcode) {
            // Load instructions: only GVN if no intervening store killed the expression
            if let Some(existing) = gvn_lookup(scopes, &key) {
                replacements.insert(inst_result, existing);
                to_kill.push(*inst_id);
                result.instructions_removed += 1;
                result.changed = true;
                continue;
            }
            scopes.last_mut().unwrap().insert(key, inst_result);
            continue;
        }

        // === General GVN lookup ===
        if let Some(existing) = gvn_lookup(scopes, &key) {
            replacements.insert(inst_result, existing);
            to_kill.push(*inst_id);
            result.instructions_removed += 1;
            result.changed = true;
        } else {
            // First occurrence — record in current scope
            scopes.last_mut().unwrap().insert(key, inst_result);
        }
    }

    // 3. DFS children
    if let Some(children) = dom_children.get(&block_id) {
        for &child in children {
            gvn_dfs(
                func,
                child,
                dom_children,
                scopes,
                replacements,
                const_map,
                to_kill,
                result,
                alias,
            );
        }
    }

    // 4. Leave block: pop scope
    scopes.pop();
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OptimizationPass;

    #[test]
    fn gvn_cross_block_duplicate() {
        // entry: v2 = a+b; branch ..., then, else
        // then: v3 = a+b (duplicate of v2) → should be replaced
        // else: v5 = a+b (duplicate of v2) → should be replaced
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        let then_block = b.create_block();
        let else_block = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let a = params[0];
        let b_val = params[1];
        let sum = b.iadd(a, b_val);
        let c = b.iconst_i32(1);
        let zero_val = b.iconst_i32(0);
        let cond = b.icmp(IntCC::NotEqual, c, zero_val);
        b.branch(cond, then_block, &[], else_block, &[]);

        b.switch_to_block(then_block);
        let sum_then = b.iadd(a, b_val); // duplicate of sum
        b.jump(merge, &[sum_then]);

        b.switch_to_block(else_block);
        let sum_else = b.iadd(a, b_val); // duplicate of sum
        b.jump(merge, &[sum_else]);

        b.switch_to_block(merge);
        b.ret(&[sum]); // placeholder

        let mut func = b.finish().expect("build");
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
    }

    #[test]
    fn gvn_diamond_reuse() {
        // Diamond CFG where a+b in entry should be reused in both branches
        let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::I32, "b")]);
        let left = b.create_block();
        let right = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let sum = b.iadd(params[0], params[1]);
        let c = b.iconst_i32(1);
        let zero = b.iconst_i32(0);
        let cond = b.icmp(IntCC::NotEqual, c, zero);
        b.branch(cond, left, &[sum], right, &[sum]);

        b.switch_to_block(left);
        b.jump(merge, &[sum]);

        b.switch_to_block(right);
        let sum2 = b.iadd(params[0], params[1]); // duplicate
        b.jump(merge, &[sum2]);

        b.switch_to_block(merge);
        b.ret(&[sum]);

        let mut func = b.finish().expect("build");
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
    }

    #[test]
    fn gvn_preserves_side_effects() {
        // Store instructions should not be moved or eliminated
        let sig = FunctionSignature::new(&[(TypeId::PTR, "p"), (TypeId::I32, "v")], &[]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::PTR, "p"), (TypeId::I32, "v")]);
        b.switch_to_block(entry);
        let v = params[1];
        let p = params[0];
        b.store(v, p);
        b.store(v, p); // NOT a duplicate — second store has distinct side effect
        b.ret(&[]);

        let mut func = b.finish().expect("build");
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // Should not eliminate the second store
        // Just verify it doesn't crash
        assert!(r.instructions_removed == 0 || !r.changed);
    }

    #[test]
    fn gvn_const_fold_bnot() {
        // GVN should fold Bnot of a constant: ~42 → -43 (for i32)
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        let not_v = b.bnot(v);
        b.ret(&[not_v]);

        let mut func = b.finish().expect("build");
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Bnot of constant should be folded");
        assert!(
            r.instructions_removed > 0,
            "Bnot instruction should be replaced"
        );
    }

    #[test]
    fn gvn_const_fold_add_truncation() {
        // Verify Iadd with constants is folded and result is truncated to type width.
        // For i32: 0xFFFFFFFF + 1 = 0 (with truncation, overflow wraps)
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let max_u32 = b.iconst_i32(-1); // 0xFFFFFFFF as i32
        let one = b.iconst_i32(1);
        let sum = b.iadd(max_u32, one);
        b.ret(&[sum]);

        let mut func = b.finish().expect("build");
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "0xFFFFFFFF + 1 should be constant-folded to 0");
        assert!(
            r.instructions_removed > 0,
            "Iadd should be replaced by Iconst"
        );
    }

    #[test]
    fn gvn_no_crash_on_loop() {
        // Loop CFG — should not crash
        let sig = FunctionSignature::new(&[(TypeId::I32, "n")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "n")]);
        let header = b.create_block();
        let body = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        let n = params[0];
        let zero = b.iconst_i32(0);
        b.jump(header, &[zero]);

        b.switch_to_block(header);
        let i = b.iconst_i32(0);
        let cond = b.icmp(IntCC::SignedLessThan, i, n);
        b.branch(cond, body, &[], exit, &[]);

        b.switch_to_block(body);
        let one = b.iconst_i32(1);
        let next_i = b.iadd(i, one);
        b.jump(header, &[next_i]);

        b.switch_to_block(exit);
        b.ret(&[i]);

        let mut func = b.finish().expect("build");
        let pass = GvnPass::new();
        let _r = pass.run_on_function(&mut func).unwrap();
        // Just verify no crash
    }

    /// P1-5 正向：跨块 load 复用——entry 定义 p + `l1 = load p`，分支块的
    /// `l2 = load p`（同一地址 SSA 值、被 entry 支配、无中间写）→ l2 消除复用 l1。
    /// （旧行为：load 不在 GVN 候选 → 永不消重。）
    #[test]
    fn p1_5_gvn_load_reuse_across_blocks() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        let then_blk = b.create_block();
        let else_blk = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let p = b.stack_addr(0);
        let l1 = b.load(p, TypeId::I32);
        let one = b.iconst_i32(1);
        let zero = b.iconst_i32(0);
        let cond = b.icmp(IntCC::NotEqual, one, zero);
        b.branch(cond, then_blk, &[], else_blk, &[]);

        b.switch_to_block(then_blk);
        let l2 = b.load(p, TypeId::I32); // 复用 l1（同地址、支配、无中间写）
        b.jump(merge, &[l2]);

        b.switch_to_block(else_blk);
        b.jump(merge, &[l1]);

        b.switch_to_block(merge);
        b.ret(&[l1]);
        let mut func = b.finish().expect("build");

        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "GVN 应跨块复用 load（P1-5）");
        assert_eq!(
            func.dfg.value_type(l2),
            Some(TypeId::VOID),
            "l2 应被消除并复用 l1"
        );
    }
}
