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
    let block_count = func.dfg.block_count();

    for bi in 0..block_count {
        // Check if block ends with Return（投影读取，不依赖 Terminator 表示）
        let Some(values) = func.dfg.term_return_values(Block(bi as u32)) else {
            continue;
        };
        let return_values: smallvec::SmallVec<[Value; 2]> = values.iter().copied().collect();
        if return_values.is_empty() {
            continue;
        }

        // Find last non-Nop Call instruction
        let call_info = {
            let block = &func.dfg.block(Block(bi as u32));
            let mut found = None;
            for &inst_id in block.inst_order.iter().rev() {
                let inst = &func.dfg.inst_data(inst_id);
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

        // **只处理自递归尾调用**（`call self(args); ret r` → `jump entry(args)`，
        // 递归转循环）。跨函数的尾调用**不能**这样改写：`callee.entry_block` 是
        // 另一个函数的块编号空间，写进本函数的终结符等于"跳到本函数的第 N 块"
        // （2026-09-14 严格校验实测：`BlockParamCountMismatch { block: 0, expected: 0,
        // found: 1 }` 正是这条路径产生的非法 IR）。跨函数尾调用需要把被调方整块
        // 拼接进来——那是 inline 的职责，本 pass 不做。
        if callee.name != func.name {
            continue;
        }
        let entry = match func.entry_block {
            Some(b) => b,
            None => continue,
        };
        // 先决条件：实参数必须与入口块参数**数量一致**（fail-closed：不一致就跳过，
        // 不产出非法 IR）。
        let entry_param_count = func.dfg.block(entry).params.len();
        if call_operands.len() != entry_param_count {
            continue;
        }

        // 把 Call 原子删除（`kill_inst` 同步 use-lists + 结果值 VOID 化 + 墓碑）；
        // 历史实现手工清 opcode/operands/immediates 而**不清理 use-lists** →
        // 留下指向旧操作数的陈旧 use 项（`UseListInconsistency`）。
        func.kill_inst(call_inst_id);

        // Replace Return with Jump（目标 = 本函数入口块：递归转循环）
        let args: smallvec::SmallVec<[Value; 2]> = call_operands.iter().copied().collect();
        func.jump(Block(bi as u32), entry, &args);
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

    /// **自递归**尾调用：`call self(x); ret(r)` → `jump entry(x)`（递归转循环）。
    #[test]
    fn tail_call_optimizes_self_recursion() {
        // f(x) = if x == 0 { 0 } else { f(x-1) }  —— 尾递归
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("rec", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        let recurse = b.create_block();
        let done = b.create_block();
        b.switch_to_block(entry);
        let zero = b.iconst_i32(0);
        let cond = b.icmp(IntCC::Equal, params[0], zero);
        b.branch(cond, done, &[], recurse, &[]);
        b.switch_to_block(recurse);
        let one = b.iconst_i32(1);
        let dec = b.isub(params[0], one);
        // 自递归调用：FuncRef(0) 指向函数自身（函数表里放同名函数的克隆）
        let call_ret = b.call(FuncRef(0), &[dec], &[TypeId::I32]);
        b.ret(&[call_ret[0]]);
        b.switch_to_block(done);
        b.ret(&[zero]);
        let mut rec_func = b.finish().expect("build");
        let entry_block = rec_func.entry_block.expect("entry");

        let mut func_table = HashMap::new();
        func_table.insert(FuncRef(0), rec_func.clone());

        let pass = TailCallPass::new(func_table);
        let r = pass.run_on_function(&mut rec_func).unwrap();
        assert!(r.changed, "自递归尾调用应转成 jump entry");
        // 目标必须是**本函数**入口块，且实参数与入口块参数一致
        let recurse_block = rec_func
            .dfg
            .block_data_iter()
            .enumerate()
            .position(|(i, _)| {
                rec_func
                    .dfg
                    .term_jump(Block(i as u32))
                    .is_some_and(|(target, _)| target == entry_block)
            })
            .expect("应有一个块被改写成 jump entry");
        let (_, args) = rec_func
            .dfg
            .term_jump(Block(recurse_block as u32))
            .expect("预期 Jump");
        assert_eq!(
            args.len(),
            rec_func.dfg.block(entry_block).params.len(),
            "实参数必须与入口块参数数量一致"
        );
        // 严格校验：改写后 IR 必须仍然合法
        let mut v = forge_ir::verify::Verifier::with_ctx(rec_func.types.clone());
        assert!(
            v.verify(&rec_func).is_ok(),
            "自递归尾调用改写后必须通过校验"
        );
    }

    /// **跨函数**尾调用**不**改写：`callee.entry_block` 属于另一个函数的块编号
    /// 空间，写进本函数就是非法目标（历史实现这么做，产出
    /// `BlockParamCountMismatch` 的非法 IR）。跨函数尾调用需要块拼接 = inline 的职责。
    #[test]
    fn cross_function_tail_call_is_not_rewritten() {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("callee", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        let result = b.iadd(params[0], one);
        b.ret(&[result]);
        let callee_func = b.finish().expect("build");

        let mut func_table = HashMap::new();
        func_table.insert(FuncRef(0), callee_func);

        let sig2 = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b2 = FunctionBuilder::new("caller", TypeContext::new(), sig2);
        let (entry2, params2) = b2.create_block_with_params(&[(TypeId::I32, "x")]);
        b2.switch_to_block(entry2);
        let call_results = b2.call(FuncRef(0), &[params2[0]], &[TypeId::I32]);
        b2.ret(&[call_results[0]]);
        let mut caller_func = b2.finish().expect("build");

        let pass = TailCallPass::new(func_table);
        let r = pass.run_on_function(&mut caller_func).unwrap();
        assert!(
            !r.changed,
            "跨函数尾调用不得改写（否则跳进另一个函数的块编号空间）"
        );
        let mut v = forge_ir::verify::Verifier::with_ctx(caller_func.types.clone());
        assert!(v.verify(&caller_func).is_ok(), "原样 IR 必须合法");
    }
}
