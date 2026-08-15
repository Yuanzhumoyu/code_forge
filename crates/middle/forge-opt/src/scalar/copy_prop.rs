//! 复制传播 pass。
//!
//! 将 `v2 = Copy(v1)` 的所有使用替换为 `v1`，然后移除 Copy 指令。
//! 这减少了不必要的寄存器移动。

use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
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

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        propagate_copies(func)
    }
}

/// 对单个函数执行复制传播。
pub fn propagate_copies(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();

    // 单轮收敛：build_copy_map 已解析链式 Copy（含循环引用检测），
    // 一轮替换 + 移除后不再有任何 Copy 指令，无需外层 while 的二次空扫。
    let copy_map = build_copy_map(func);

    if copy_map.is_empty() {
        result.changed = false;
        return Ok(result);
    }

    // 1. 在所有指令与终结符中替换被 copy 的值（同步 use-lists）
    result.values_replaced = func.apply_replacements(&copy_map);

    // 2. 原子删除所有 Copy 指令（结果值已全部被替换，无活跃使用）
    let mut to_kill: Vec<Inst> = Vec::new();
    for block in func.dfg.blocks.iter() {
        for &inst_id in &block.inst_order {
            if matches!(func.dfg.insts[inst_id.0 as usize].opcode, Opcode::Copy) {
                to_kill.push(inst_id);
            }
        }
    }
    for inst in to_kill {
        func.kill_inst(inst);
        result.instructions_removed += 1;
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

        let mut func = b.finish().expect("build");
        let pass = CopyPropPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        assert!(r.instructions_removed >= 1);
        // Return should use src directly (not copied)
        let term = &func.dfg.blocks[0].terminator;
        if let Terminator::Return { values, .. } = term {
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

        let mut func = b.finish().expect("build");
        let pass = CopyPropPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(!r.changed);
    }
}
