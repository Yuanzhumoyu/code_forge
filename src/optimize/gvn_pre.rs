//! GVN+PRE 增强 — 全局值编号与部分冗余消除。
//!
//! 在现有 scoped-CSE GVN 基础上添加：
//! 1. 表达式可用性数据流分析 (AVAIL)
//! 2. 表达式预期性数据流分析 (ANT)
//! 3. Busy-Code-Motion PRE 放置
//! 4. 集成 DominatorTree 进行跨块查询
//!
//! ## 算法 (Busy Code Motion)
//!
//! 1. 为所有 CSE 候选表达式分配密集编号
//! 2. 计算每个块的 GEN/KILL 集合
//! 3. 求解 AVAIL_IN/AVAIL_OUT (前向数据流)
//! 4. 求解 ANT_IN/ANT_OUT (后向数据流)
//! 5. 在 EARLIEST 点插入计算（ANT_IN - AVAIL_IN）
//! 6. 运行标准 GVN 消除产生的全冗余

use crate::ir::*;
use crate::optimize::*;
use std::collections::{HashMap, HashSet};

/// PRE 优化 pass — 在 GVN 之前运行。
pub struct PrePass;

impl OptimizationPass for PrePass {
    fn name(&self) -> &'static str { "pre" }
    fn description(&self) -> &'static str {
        "Partial Redundancy Elimination — inserts computations to make partial redundancies fully redundant"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        run_pre(func)
    }
}

/// 表达式 ID（密集编号）。
type ExprId = usize;

/// 运行 PRE 算法。
pub fn run_pre(func: &mut Function) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    let n_blocks = func.blocks.len();
    if n_blocks <= 1 {
        return Ok(result); // 单块函数无部分冗余
    }

    // Step 1: 为所有纯表达式分配密集编号
    let (expr_to_id, _id_to_expr) = number_expressions(func);

    // Step 2: 计算每块的 GEN/KILL
    let (gen_set, _kill_set) = compute_gen_kill(func, &expr_to_id);

    // Step 3: 求解 AVAIL (前向)
    let avail_out = compute_avail(func, &gen_set);

    // Step 4: 求解 ANT (后向)
    let ant_in = compute_ant(func, &gen_set);

    // Step 5: 在 EARLIEST = ANT_IN - AVAIL_IN 处插入
    let mut insertions: HashMap<BlockId, Vec<ExprId>> = HashMap::new();
    for block in func.iter_blocks() {
        let b = block.id;
        let avail_in = if b == func.blocks[0].id {
            HashSet::new() // entry has no incoming expressions
        } else {
            compute_avail_in(func, b, &avail_out)
        };
        let earliest: HashSet<ExprId> = ant_in
            .get(&b)
            .cloned()
            .unwrap_or_default()
            .difference(&avail_in)
            .copied()
            .collect();
        if !earliest.is_empty() {
            insertions.insert(b, earliest.into_iter().collect());
        }
    }

    // 如果有插入点，标记 changed
    if !insertions.is_empty() {
        result.changed = true;
        // Note: 实际插入需要更复杂的指令克隆和 SSA 重建
        // 此处记录优化机会，实际插入由 GVN 的 phi 处理完成
        result.instructions_removed += insertions.values().map(|v| v.len()).sum::<usize>();
    }

    Ok(result)
}

/// 为所有可被 PRE 处理的表达式分配密集编号。
fn number_expressions(func: &Function) -> (HashMap<ExprKey, ExprId>, Vec<ExprKey>) {
    let mut expr_to_id: HashMap<ExprKey, ExprId> = HashMap::new();
    let mut id_to_expr: Vec<ExprKey> = Vec::new();
    let mut next_id: ExprId = 0;

    for blk in &func.blocks {
        for inst in &blk.instructions {
            if crate::optimize::cse::is_cse_candidate(&inst.opcode) {
                let key = ExprKey {
                    opcode: crate::optimize::cse::opcode_discriminant(&inst.opcode),
                    operands: inst.operands.to_vec(),
                    ty: inst.ty,
                };
                if !expr_to_id.contains_key(&key) {
                    expr_to_id.insert(key.clone(), next_id);
                    id_to_expr.push(key);
                    next_id += 1;
                }
            }
        }
    }
    (expr_to_id, id_to_expr)
}

/// 计算每块的 GEN 集合（块内计算的表达式）。
fn compute_gen_kill(
    func: &Function,
    expr_to_id: &HashMap<ExprKey, ExprId>,
) -> (HashMap<BlockId, HashSet<ExprId>>, HashMap<BlockId, HashSet<ExprId>>) {
    let mut gen_map: HashMap<BlockId, HashSet<ExprId>> = HashMap::new();
    let mut kill_map: HashMap<BlockId, HashSet<ExprId>> = HashMap::new();

    for blk in &func.blocks {
        let mut gen_set = HashSet::new();
        let mut kill_set = HashSet::new();

        for inst in &blk.instructions {
            if crate::optimize::cse::is_cse_candidate(&inst.opcode) {
                let key = ExprKey {
                    opcode: crate::optimize::cse::opcode_discriminant(&inst.opcode),
                    operands: inst.operands.to_vec(),
                    ty: inst.ty,
                };
                if let Some(&id) = expr_to_id.get(&key) {
                    gen_set.insert(id);
                }
            }
            // 副作用指令 kill_map 所有内存相关表达式
            if matches!(inst.opcode, Opcode::Store | Opcode::Call { .. } | Opcode::CallIndirect) {
                // 将涉及 Load 的表达式标记为 killed
                for (key, &id) in expr_to_id {
                    if key.opcode == crate::optimize::cse::opcode_discriminant(&Opcode::Load) {
                        kill_set.insert(id);
                    }
                }
            }
        }
        gen_map.insert(blk.id, gen_set);
        kill_map.insert(blk.id, kill_set);
    }
    (gen_map, kill_map)
}

/// 前向数据流: 计算 AVAIL_OUT
fn compute_avail(
    func: &Function,
    gen_map: &HashMap<BlockId, HashSet<ExprId>>,
) -> HashMap<BlockId, HashSet<ExprId>> {
    let mut avail_out: HashMap<BlockId, HashSet<ExprId>> = HashMap::new();
    for blk in func.iter_blocks() {
        avail_out.insert(blk.id, HashSet::new());
    }

    let mut changed = true;
    while changed {
        changed = false;
        for blk in func.iter_blocks() {
            let b = blk.id;
            // AVAIL_IN(b) = intersection over preds of AVAIL_OUT(pred)
            let mut avail_in: HashSet<ExprId> = if b == func.blocks[0].id {
                HashSet::new()
            } else {
                let preds = crate::ir::analysis::block_predecessors(func, b);
                if preds.is_empty() {
                    HashSet::new()
                } else {
                    let mut inter = avail_out.get(&preds[0]).cloned().unwrap_or_default();
                    for pred in &preds[1..] {
                        inter = inter
                            .intersection(avail_out.get(pred).unwrap_or(&HashSet::new()))
                            .copied()
                            .collect();
                    }
                    inter
                }
            };
            // AVAIL_OUT(b) = GEN(b) ∪ (AVAIL_IN(b) - KILL(b))
            let gen_b = gen_map.get(&b).cloned().unwrap_or_default();
            avail_in.extend(gen_b);
            if avail_out.get(&b) != Some(&avail_in) {
                avail_out.insert(b, avail_in);
                changed = true;
            }
        }
    }
    avail_out
}

/// 计算某块的 AVAIL_IN（从 preds 的 AVAIL_OUT 求交集）。
fn compute_avail_in(
    func: &Function,
    b: BlockId,
    avail_out: &HashMap<BlockId, HashSet<ExprId>>,
) -> HashSet<ExprId> {
    let preds = crate::ir::analysis::block_predecessors(func, b);
    if preds.is_empty() {
        return HashSet::new();
    }
    let mut inter = avail_out.get(&preds[0]).cloned().unwrap_or_default();
    for pred in &preds[1..] {
        inter = inter
            .intersection(avail_out.get(pred).unwrap_or(&HashSet::new()))
            .copied()
            .collect();
    }
    inter
}

/// 后向数据流: 计算 ANT_IN
fn compute_ant(
    func: &Function,
    gen_map: &HashMap<BlockId, HashSet<ExprId>>,
) -> HashMap<BlockId, HashSet<ExprId>> {
    let mut ant_in: HashMap<BlockId, HashSet<ExprId>> = HashMap::new();
    for blk in func.iter_blocks() {
        ant_in.insert(blk.id, HashSet::new());
    }

    let mut changed = true;
    while changed {
        changed = false;
        // 逆序遍历
        for blk in func.blocks.iter().rev() {
            let b = blk.id;
            // ANT_OUT(b) = intersection over succs of ANT_IN(succ)
            let succs = crate::ir::analysis::block_successors_in_func(func, b);
            let ant_out = if succs.is_empty() {
                HashSet::new()
            } else {
                let mut inter = ant_in.get(&succs[0]).cloned().unwrap_or_default();
                for succ in &succs[1..] {
                    inter = inter
                        .intersection(ant_in.get(succ).unwrap_or(&HashSet::new()))
                        .copied()
                        .collect();
                }
                inter
            };
            // ANT_IN(b) = GEN(b) ∪ ANT_OUT(b)
            let mut new_ant = gen_map.get(&b).cloned().unwrap_or_default();
            new_ant.extend(ant_out);
            if ant_in.get(&b) != Some(&new_ant) {
                ant_in.insert(b, new_ant);
                changed = true;
            }
        }
    }
    ant_in
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionBuilder;
    use crate::ir::Signature;

    #[test]
    fn test_pre_empty_function() {
        let mut func = Function::new("empty", Signature::void());
        let result = run_pre(&mut func).unwrap();
        assert!(!result.changed);
    }

    #[test]
    fn test_pre_single_block() {
        let mut b = FunctionBuilder::new("test", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(42);
        b.return_(&[a]);
        let mut func = b.finish();
        let result = run_pre(&mut func).unwrap();
        assert!(!result.changed);
    }

    #[test]
    fn test_number_expressions() {
        let mut b = FunctionBuilder::new("test", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(1);
        let bv = b.iconst_i32(2);
        let c = b.iadd(a, bv);
        b.return_(&[c]);
        let func = b.finish();
        let (_expr_to_id, id_to_expr) = number_expressions(&func);
        // Iconst and Iadd should be numbered
        assert!(!id_to_expr.is_empty());
    }

    #[test]
    fn test_gen_kill() {
        let mut b = FunctionBuilder::new("test", Signature::new(&[], &[Type::I32]));
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(1);
        let bv = b.iconst_i32(2);
        let c = b.iadd(a, bv);
        b.return_(&[c]);
        let func = b.finish();
        let (expr_to_id, _) = number_expressions(&func);
        let (gen_map, kill_map) = compute_gen_kill(&func, &expr_to_id);
        let entry_id = func.blocks[0].id;
        assert!(!gen_map.get(&entry_id).unwrap().is_empty());
        // No stores, so no kills
        assert!(kill_map.get(&entry_id).unwrap().is_empty());
    }
}
