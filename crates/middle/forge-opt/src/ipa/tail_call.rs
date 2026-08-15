//! 尾调用优化 (TCO) pass.
//!
//! Detects `v = call f(args); return v` patterns and converts to Jump to callee entry.

use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;
use std::collections::HashMap;

pub struct TailCallPass {
    function_table: HashMap<FuncRef, Function>,
}

impl TailCallPass {
    pub fn new(function_table: HashMap<FuncRef, Function>) -> Self {
        Self { function_table }
    }
}

impl OptimizationPass for TailCallPass {
    fn name(&self) -> &'static str {
        "tail-call"
    }
    fn description(&self) -> &'static str {
        "Converts tail calls into jumps"
    }
    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        optimize_tail_calls(func, &self.function_table)
    }
}

pub fn optimize_tail_calls(
    func: &mut Function,
    function_table: &HashMap<FuncRef, Function>,
) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();
    let block_count = func.dfg.blocks.len();

    for bi in 0..block_count {
        // Check if block ends with Return
        let return_values = match &func.dfg.blocks[bi].terminator {
            Terminator::Return { values, .. } => values.clone(),
            _ => continue,
        };
        if return_values.is_empty() {
            continue;
        }

        // Find last non-Nop Call instruction
        let call_info = {
            let block = &func.dfg.blocks[bi];
            let mut found = None;
            for &inst_id in block.inst_order.iter().rev() {
                let inst = &func.dfg.insts[inst_id.0 as usize];
                if matches!(inst.opcode, Opcode::Nop) {
                    continue;
                }
                if matches!(inst.opcode, Opcode::Call)
                    && let Some(callee) = inst.immediates.iter().find_map(|i| i.as_func())
                {
                    found = Some((
                        inst_id,
                        callee,
                        inst.operands.clone(),
                        inst.results.first().copied(),
                    ));
                }
                break;
            }
            found
        };

        let (call_inst_id, callee_ref, call_operands, call_result) = match call_info {
            Some(v) => v,
            None => continue,
        };
        let call_result_val = match call_result {
            Some(v) => v,
            None => continue,
        };
        if return_values[0] != call_result_val {
            continue;
        }

        let callee = match function_table.get(&callee_ref) {
            Some(f) => f,
            None => continue,
        };
        let callee_entry = match callee.entry_block {
            Some(b) => b,
            None => continue,
        };

        // Replace Call with Nop
        {
            let inst = &mut func.dfg.insts[call_inst_id.0 as usize];
            inst.opcode = Opcode::Nop;
            inst.operands.clear();
            inst.immediates.clear();
            if let Some(v) = inst.results.first().copied() {
                func.dfg.values[v.0 as usize].ty = TypeId::VOID;
            }
            inst.results.clear();
        }

        // Replace Return with Jump
        let args: smallvec::SmallVec<[Value; 2]> = call_operands.iter().copied().collect();
        func.dfg.blocks[bi].terminator = Terminator::Jump {
            target: callee_entry,
            args,
            metadata: smallvec::smallvec![],
        };
        result.instructions_removed += 1;
        result.changed = true;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_call_no_crash() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        b.ret(&[params[0]]);
        let mut func = b.finish().expect("build");
        let pass = TailCallPass::new(HashMap::new());
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed);
    }

    /// Test that a simple `call f(x); ret(result)` tail call is optimized to `jump f_entry(x)`.
    #[test]
    fn tail_call_optimizes_call_ret_pattern() {
        // Build callee: f(x) = x + 1
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("callee", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        let result = b.iadd(params[0], one);
        b.ret(&[result]);
        let callee_func = b.finish().expect("build");
        let callee_ref = FuncRef(0);

        let mut func_table = HashMap::new();
        func_table.insert(callee_ref, callee_func);

        // Build caller: call callee(x); ret(result)
        let sig2 = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b2 = FunctionBuilder::new("caller", TypeContext::new(), sig2);
        let (entry2, params2) = b2.create_block_with_params(&[(TypeId::I32, "x")]);
        b2.switch_to_block(entry2);
        let call_results = b2.call(callee_ref, &[params2[0]], &[TypeId::I32]);
        b2.ret(&[call_results[0]]);
        let mut caller_func = b2.finish().expect("build");

        let pass = TailCallPass::new(func_table);
        let r = pass.run_on_function(&mut caller_func).unwrap();
        assert!(r.changed, "Tail call should be optimized to jump");
    }
}
