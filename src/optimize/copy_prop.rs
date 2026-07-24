//! 复制传播 pass。
//!
//! 将 `v2 = Copy(v1)` 的所有使用替换为 `v1`，然后移除 Copy 指令。
//! 这减少了不必要的寄存器移动。

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};
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
        for block in func.blocks.iter_mut() {
            for inst in block.instructions.iter_mut() {
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
            match &mut block.terminator {
                Terminator::Branch {
                    cond,
                    true_args,
                    false_args,
                    ..
                } => {
                    if let Some(target) = copy_map.get(cond) {
                        *cond = *target;
                        changed = true;
                        result.values_replaced += 1;
                    }
                    for v in true_args.iter_mut().chain(false_args.iter_mut()) {
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
        for block in func.blocks.iter_mut() {
            for inst in block.instructions.iter_mut() {
                if matches!(inst.opcode, Opcode::Copy) {
                    inst.opcode = Opcode::Nop;
                    inst.operands.clear();
                    inst.ty = Type::Void;
                    inst.result = None;
                    result.instructions_removed += 1;
                }
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
    for block in func.blocks.iter() {
        for inst in &block.instructions {
            if matches!(inst.opcode, Opcode::Copy)
                && let (Some(result), Some(source)) = (inst.result, inst.operands.first())
            {
                map.insert(result, *source);
            }
        }
    }

    // 解析链式 Copy：每个键追踪到最终的源
    for value in map.clone().keys() {
        let mut current = *value;
        let mut visited = Vec::new();

        while let Some(target) = map.get(&current) {
            if visited.contains(&current) {
                // 循环引用 — 停止追踪
                break;
            }
            visited.push(current);
            current = *target;
        }

        if current != *value {
            map.insert(*value, current);
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
    use crate::optimize::OptimizationPass;

    #[test]
    fn propagate_single_copy() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let src = b.iconst_i32(42);
        let copied = b.copy(src);
        b.return_(&[copied]);

        let mut func = b.finish();
        let pass = CopyPropPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        assert!(r.instructions_removed >= 1);
        // copied should be Nop; return should use src directly
        if let Terminator::Return { values } = &func.blocks[0].terminator {
            assert_eq!(values[0], src);
        } else {
            panic!("expected Return terminator");
        }
    }

    #[test]
    fn propagate_chain_copy() {
        // v2 = Copy(v1), v3 = Copy(v2) → v3 should become v1
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v1 = b.iconst_i32(10);
        let v2 = b.copy(v1);
        let v3 = b.copy(v2);
        b.return_(&[v3]);

        let mut func = b.finish();
        let pass = CopyPropPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        // v3 应该被替换为 v1（链式传播）
        if let Terminator::Return { values } = &func.blocks[0].terminator {
            assert_eq!(values[0], v1);
        } else {
            panic!("expected Return terminator");
        }
    }

    #[test]
    fn propagate_in_branch_terminator() {
        // Copy 的使用在 branch args 中也应被替换
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        let then_block = b.create_block();
        let else_block = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let val = b.iconst_i32(1);
        let copied = b.copy(val);
        let cond = b.icmp(IntCC::Equal, val, val);
        b.branch(cond, then_block, else_block, &[copied], &[val]);

        b.switch_to_block(then_block);
        b.jump(merge, &[]);

        b.switch_to_block(else_block);
        b.jump(merge, &[]);

        b.switch_to_block(merge);
        b.return_(&[]);

        let mut func = b.finish();
        let pass = CopyPropPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        // entry block 的 branch true_args 应该直接用 val（不是 copied）
        if let Terminator::Branch { true_args, .. } = &func.blocks[0].terminator {
            assert_eq!(true_args[0], val);
        } else {
            panic!("expected Branch terminator");
        }
    }

    #[test]
    fn no_copy_no_change() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(3);
        let b_val = b.iconst_i32(5);
        let sum = b.iadd(a, b_val);
        b.return_(&[sum]);

        let mut func = b.finish();
        let pass = CopyPropPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(!r.changed);
    }

    #[test]
    fn propagate_to_return_values() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(7);
        let c = b.copy(v);
        b.return_(&[c]);

        let mut func = b.finish();
        let pass = CopyPropPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        if let Terminator::Return { values } = &func.blocks[0].terminator {
            assert_eq!(values[0], v);
        } else {
            panic!("expected Return terminator");
        }
    }
}
