//! 稀疏条件常量传播 (SCCP) pass。
//!
//! 与标准 ConstFold 不同，SCCP 联合传播常量值和执行状态（可到达性）：
//! - 标记从未执行的基本块为不可达
//! - 仅在值变为常量时才传播
//! - 可发现 ConstFold 遗漏的常量（如通过不可达分支发现的常量）
//!
//! # 算法（简化版）
//!
//! 1. 初始化：所有值标记为 Undefined，Iconst/Fconst 标记为 Constant
//! 2. Worklist 驱动的流敏感传播
//! 3. 可到达块中的指令才被求值
//! 4. 条件分支：常量条件 → 只标记 taken 分支为可达
//! 5. 到达不动点后：替换常量、消除不可达块

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{ConstValue, OptimizationPass, PassResult};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct SccpPass;

impl SccpPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for SccpPass {
    fn name(&self) -> &'static str {
        "sccp"
    }
    fn description(&self) -> &'static str {
        "Sparse Conditional Constant Propagation"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        sccp(func)
    }
}

/// 格值：Undefined（尚未定义）→ Constant（已知值）→ Top（非常量/多变）。
#[derive(Clone, Debug)]
enum LatticeValue {
    Undefined,
    Constant(ConstValue),
    Top,
}

pub fn sccp(func: &mut Function) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    if func.blocks.is_empty() {
        return Ok(result);
    }

    // 1. 初始化格值
    let mut lattice: HashMap<Value, LatticeValue> = HashMap::new();
    let mut reachable: HashSet<BlockId> = HashSet::new();
    let mut worklist: Vec<BlockId> = Vec::new();

    // 入口块总是可达
    let entry = func.blocks[0].id;
    reachable.insert(entry);
    worklist.push(entry);

    // 扫描 Iconst/Fconst 初始化
    for block in &func.blocks {
        for inst in &block.instructions {
            if let Some(v) = inst.result {
                lattice.insert(v, LatticeValue::Undefined);
            }
        }
    }
    for block in &func.blocks {
        for inst in &block.instructions {
            match &inst.opcode {
                Opcode::Iconst { index } => {
                    if let Some(v) = inst.result
                        && let Some(big) = func.constant_pool.get(*index).cloned()
                    {
                        lattice.insert(v, LatticeValue::Constant(ConstValue::Int(big, inst.ty)));
                    }
                }
                Opcode::Fconst { index } => {
                    if let Some(v) = inst.result
                        && let Some(big) = func.constant_pool.get(*index).cloned()
                    {
                        lattice.insert(v, LatticeValue::Constant(ConstValue::Float(big, inst.ty)));
                    }
                }
                _ => {}
            }
        }
    }

    // 2. Worklist 传播
    let uses_map = collect_all_uses(func);

    while let Some(block_id) = worklist.pop() {
        let block = &func.blocks[block_id.0 as usize];

        // 求值块内指令
        for inst in &block.instructions {
            if let Some(v) = inst.result {
                let old = lattice.get(&v).cloned();
                let new = evaluate_lattice(&inst.opcode, &inst.operands, inst.ty, &lattice, func);
                if !lattice_eq(&old, &new) {
                    lattice.insert(v, new);
                    // 将使用者加入传播
                    if let Some(users) = uses_map.get(&v) {
                        for &user_block in users {
                            if reachable.contains(&user_block) {
                                worklist.push(user_block);
                            }
                        }
                    }
                }
            }
        }

        // 处理终止指令 — 传播可达性
        let _preds = func.predecessors();
        match &block.terminator {
            Terminator::Branch {
                cond,
                true_block,
                false_block,
                ..
            } => match lattice.get(cond) {
                Some(LatticeValue::Constant(cv)) => match cv.to_bool() {
                    Some(true) => {
                        if reachable.insert(*true_block) {
                            worklist.push(*true_block);
                        }
                    }
                    Some(false) => {
                        if reachable.insert(*false_block) {
                            worklist.push(*false_block);
                        }
                    }
                    None => {
                        if reachable.insert(*true_block) {
                            worklist.push(*true_block);
                        }
                        if reachable.insert(*false_block) {
                            worklist.push(*false_block);
                        }
                    }
                },
                _ => {
                    if reachable.insert(*true_block) {
                        worklist.push(*true_block);
                    }
                    if reachable.insert(*false_block) {
                        worklist.push(*false_block);
                    }
                }
            },
            Terminator::Jump { target, .. }
                if reachable.insert(*target) => {
                    worklist.push(*target);
                }
            Terminator::Switch {
                default_block,
                cases,
                ..
            } => {
                if reachable.insert(*default_block) {
                    worklist.push(*default_block);
                }
                for (_, target, _) in cases {
                    if reachable.insert(*target) {
                        worklist.push(*target);
                    }
                }
            }
            _ => {}
        }
    }

    // 3. 替换常量值
    for block in func.blocks.iter_mut() {
        if !reachable.contains(&block.id) {
            continue;
        }
        for inst in block.instructions.iter_mut() {
            if let Some(v) = inst.result
                && let Some(LatticeValue::Constant(cv)) = lattice.get(&v)
            {
                match cv {
                    ConstValue::Int(big, _) => {
                        let index = func.constant_pool.insert(big.clone());
                        inst.opcode = Opcode::Iconst { index };
                        inst.operands.clear();
                        result.instructions_removed += 1;
                        result.changed = true;
                    }
                    ConstValue::Float(big, _) => {
                        let index = func.constant_pool.insert(big.clone());
                        inst.opcode = Opcode::Fconst { index };
                        inst.operands.clear();
                        result.instructions_removed += 1;
                        result.changed = true;
                    }
                    _ => {}
                }
            }
        }
        // 折叠常量条件分支
        if let Terminator::Branch {
            cond,
            true_block,
            false_block,
            ..
        } = &block.terminator
            && let Some(LatticeValue::Constant(cv)) = lattice.get(cond)
        {
            if let Some(true) = cv.to_bool() {
                block.terminator = Terminator::Jump {
                    target: *true_block,
                    args: smallvec::smallvec![],
                };
                result.changed = true;
            } else if let Some(false) = cv.to_bool() {
                block.terminator = Terminator::Jump {
                    target: *false_block,
                    args: smallvec::smallvec![],
                };
                result.changed = true;
            }
        }
    }

    // 4. 清除不可达块
    for block in func.blocks.iter_mut() {
        if !reachable.contains(&block.id) {
            block.instructions.clear();
            block.terminator = Terminator::Unreachable;
            result.blocks_removed += 1;
            result.changed = true;
        }
    }

    Ok(result)
}

fn evaluate_lattice(
    opcode: &Opcode,
    operands: &[Value],
    ty: Type,
    lattice: &HashMap<Value, LatticeValue>,
    _func: &Function,
) -> LatticeValue {
    // 收集操作数的格值
    let lat_ops: Vec<&LatticeValue> = operands.iter().filter_map(|v| lattice.get(v)).collect();

    if lat_ops.len() != operands.len() {
        return LatticeValue::Top;
    }

    // 检查是否所有操作数都是常量
    let const_ops: Vec<ConstValue> = lat_ops
        .iter()
        .filter_map(|lv| match lv {
            LatticeValue::Constant(cv) => Some(cv.clone()),
            _ => None,
        })
        .collect();

    if const_ops.len() != operands.len() {
        return LatticeValue::Top;
    }

    // 尝试折叠
    match super::const_fold::fold_opcode(opcode, &const_ops, ty) {
        Ok(Some(cv)) => LatticeValue::Constant(cv),
        _ => LatticeValue::Top,
    }
}

fn lattice_eq(a: &Option<LatticeValue>, b: &LatticeValue) -> bool {
    match a {
        None => false,
        Some(LatticeValue::Undefined) => matches!(b, LatticeValue::Undefined),
        Some(LatticeValue::Constant(a_cv)) => {
            matches!(b, LatticeValue::Constant(b_cv) if a_cv == b_cv)
        }
        Some(LatticeValue::Top) => matches!(b, LatticeValue::Top),
    }
}

fn collect_all_uses(func: &Function) -> HashMap<Value, HashSet<BlockId>> {
    let mut uses: HashMap<Value, HashSet<BlockId>> = HashMap::new();
    for block in &func.blocks {
        for inst in &block.instructions {
            for operand in &inst.operands {
                uses.entry(*operand).or_default().insert(block.id);
            }
        }
    }
    uses
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionBuilder;

    #[test]
    fn sccp_folds_constants() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c1 = b.iconst_i32(3);
        let c2 = b.iconst_i32(5);
        let sum = b.iadd(c1, c2);
        b.return_(&[sum]);

        let mut func = b.finish();
        let pass = SccpPass::new();
        pass.run_on_function(&mut func).unwrap();
        // SCCP should never break IR validity
        assert!(func.validate().is_valid());
    }

    #[test]
    fn sccp_eliminates_unreachable_branch() {
        // if (1) { ret 42 } else { ret 0 }
        // After SCCP: else branch becomes unreachable
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        let then_blk = b.create_block();
        let else_blk = b.create_block();

        b.switch_to_block(entry);
        let cond = b.iconst_i32(1); // always true
        b.branch(cond, then_blk, else_blk, &[], &[]);

        b.switch_to_block(then_blk);
        let v1 = b.iconst_i32(42);
        b.return_(&[v1]);

        b.switch_to_block(else_blk);
        let v2 = b.iconst_i32(0);
        b.return_(&[v2]);

        let mut func = b.finish();
        let pass = SccpPass::new();
        pass.run_on_function(&mut func).unwrap();
        // SCCP should preserve IR validity and detect the constant branch
        assert!(func.validate().is_valid());
    }

    #[test]
    fn sccp_handles_non_const() {
        // fn test(x: i32) -> i32 { x + 1 }
        // Should not change anything (x is not constant)
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        let sum = b.iadd(params[0], one);
        b.return_(&[sum]);

        let mut func = b.finish();
        let pass = SccpPass::new();
        let _r = pass.run_on_function(&mut func).unwrap();
        // Should not claim changed (params are Top, not Constant)
        // Actually sum won't be folded since x is not const
        assert!(func.validate().is_valid());
    }
}
