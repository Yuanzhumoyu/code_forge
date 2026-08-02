//! 复制传播 pass。
//!
//! 将 `v2 = Copy(v1)` 的所有使用替换为 `v1`，然后移除 Copy 指令。
//! 这减少了不必要的寄存器移动。

use crate::{OptimizationPass, PassResult};
use forge_ir::CompileError;
use forge_ir::*;
use std::collections::HashMap;

/// 复制传播 pass。
#[derive(Default)]
pub struct CopyPropPass;

impl CopyPropPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for CopyPropPass {
    fn name(&self) -> &'static str {
        "copy-prop"
    }

    fn description(&self) -> &'static str {
        "Copy propagation: replaces uses of Copy instructions with their source"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        propagate_copies(func)
    }
}

/// 对单个函数执行复制传播。
pub fn propagate_copies(func: &mut Function) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    let mut changed = true;

    while changed {
        changed = false;

        // 1. 找到所有 Copy 指令，建立替换映射
        let copy_map = build_copy_map(func);

        if copy_map.is_empty() {
            break;
        }

        // 2. 在所有指令中替换被 copy 的值
        let inst_count = func.dfg.insts.len();
        for inst_idx in 0..inst_count {
            let inst = &mut func.dfg.insts[inst_idx];
            // 跳过 Copy 指令自身（将被移除）
            if matches!(inst.opcode, Opcode::Copy) {
                continue;
            }
            for operand in inst.operands.iter_mut() {
                if let Some(target) = copy_map.get(operand) {
                    *operand = *target;
                    changed = true;
                    result.values_replaced += 1;
                }
            }
        }

        // 替换终止指令中的值
        let block_count = func.dfg.blocks.len();
        for bi in 0..block_count {
            let block = &mut func.dfg.blocks[bi];
            match &mut block.terminator {
                Terminator::Branch {
                    cond,
                    then_args,
                    else_args,
                    ..
                } => {
                    if let Some(target) = copy_map.get(cond) {
                        *cond = *target;
                        changed = true;
                        result.values_replaced += 1;
                    }
                    for v in then_args.iter_mut().chain(else_args.iter_mut()) {
                        if let Some(target) = copy_map.get(v) {
                            *v = *target;
                            changed = true;
                            result.values_replaced += 1;
                        }
                    }
                }
                Terminator::Jump { args, .. } => {
                    for v in args.iter_mut() {
                        if let Some(target) = copy_map.get(v) {
                            *v = *target;
                            changed = true;
                            result.values_replaced += 1;
                        }
                    }
                }
                Terminator::Return { values } => {
                    for v in values.iter_mut() {
                        if let Some(target) = copy_map.get(v) {
                            *v = *target;
                            changed = true;
                            result.values_replaced += 1;
                        }
                    }
                }
                Terminator::Unreachable => {}
                Terminator::Switch {
                    discriminant,
                    cases,
                    ..
                } => {
                    if let Some(target) = copy_map.get(discriminant) {
                        *discriminant = *target;
                        changed = true;
                        result.values_replaced += 1;
                    }
                    for (_, _, args) in cases.iter_mut() {
                        for v in args.iter_mut() {
                            if let Some(target) = copy_map.get(v) {
                                *v = *target;
                                changed = true;
                                result.values_replaced += 1;
                            }
                        }
                    }
                }
            }
        }

        // 3. 移除 Copy 指令（替换为 Nop）
        for inst_idx in 0..func.dfg.insts.len() {
            let inst = &mut func.dfg.insts[inst_idx];
            if matches!(inst.opcode, Opcode::Copy) {
                inst.opcode = Opcode::Nop;
                inst.operands.clear();
                if let Some(result_val) = inst.results.first().copied() {
                    func.dfg.values[result_val.0 as usize].ty = TypeId::VOID;
                }
                inst.results.clear();
                result.instructions_removed += 1;
            }
        }

        // 如果还有 Copy 指令（链式 Copy 传播），继续迭代
        if changed {
            // 检查是否还有可传播的 Copy
        }
    }

    result.changed = result.instructions_removed > 0 || result.values_replaced > 0;
    Ok(result)
}

/// 构建 Copy 映射：result → source。
///
/// 解析链式 Copy：如果 `v3 = Copy(v2)` 且 `v2 = Copy(v1)`，
/// 则 `v3 → v1`。
fn build_copy_map(func: &Function) -> HashMap<Value, Value> {
    let mut map: HashMap<Value, Value> = HashMap::new();

    // 收集直接 Copy 映射
    for block in func.dfg.blocks.iter() {
        for &inst_id in &block.inst_order {
            let inst = &func.dfg.insts[inst_id.0 as usize];
            if matches!(inst.opcode, Opcode::Copy)
                && let (Some(result), Some(source)) =
                    (inst.results.first().copied(), inst.operands.first())
            {
                map.insert(result, *source);
            }
        }
    }

    // 解析链式 Copy：每个键追踪到最终的源
    let keys: Vec<Value> = map.keys().copied().collect();
    for start_value in keys {
        let mut current = start_value;
        let mut visited = Vec::new();

        while let Some(&target) = map.get(&current) {
            if visited.contains(&current) {
                // 循环引用 — 停止追踪
                break;
            }
            visited.push(current);
            current = target;
        }

        if current != start_value {
            map.insert(start_value, current);
        }
    }

    map
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OptimizationPass;

    #[test]
    fn propagate_single_copy() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let src = b.iconst_i32(42);
        let copied = b.copy(src);
        b.ret(&[copied]);

        let mut func = b.finish();
        let pass = CopyPropPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        assert!(r.instructions_removed >= 1);
        // Return should use src directly (not copied)
        let term = &func.dfg.blocks[0].terminator;
        if let Terminator::Return { values } = term {
            assert_eq!(values[0], src);
        } else {
            panic!("expected Return terminator");
        }
    }

    #[test]
    fn no_copy_no_change() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(3);
        let b_val = b.iconst_i32(5);
        let sum = b.iadd(a, b_val);
        b.ret(&[sum]);

        let mut func = b.finish();
        let pass = CopyPropPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(!r.changed);
    }
}
