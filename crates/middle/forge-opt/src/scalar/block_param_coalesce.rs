//! 块参数合并 (Block Parameter Coalescing) pass。
//!
//! v2 IR 用 Block Parameters 替代传统的 Phi 指令。Block param coalescing
//! 检测从所有前驱块接收相同值的 block param，并将其替换为该值 —
//! 消除冗余参数，简化控制流。
//!
//! # 算法
//!
//! 1. 对于每个带参数的块:
//! 2. 对于每个 block param，收集所有前驱 Jump/Branch 中对应位置的参数值
//! 3. 若所有传入值相同（单一 SSA value 或常量），则该 block param 是冗余的
//! 4. 用该值替换 param 的所有使用，从 block 移除该参数，从所有前驱 terminator 移除对应参数
//!
//! # 与 phi_elim 的区别
//!
//! v1 的 `phi_elim` 将 Phi 指令转换为 Copy。v2 中没有 Phi，这个 pass
//! 处理 Block Parameters 的自然冗余。

use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;

#[derive(Default)]
pub struct BlockParamCoalescePass;

impl BlockParamCoalescePass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for BlockParamCoalescePass {
    fn name(&self) -> &'static str {
        "block-param-coalesce"
    }
    fn description(&self) -> &'static str {
        "Eliminates redundant block parameters that receive the same value from all predecessors"
    }
    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        coalesce_block_params(func)
    }
}

/// 对函数中的所有块执行块参数合并。
pub fn coalesce_block_params(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();
    let preds = func.predecessors().clone();

    let block_count = func.dfg.blocks.len();
    for bi in 0..block_count {
        let block = Block(bi as u32);
        let param_count = func.dfg.blocks[bi].params.len();
        if param_count == 0 {
            continue;
        }

        let pred_list = match preds.get(&block) {
            Some(p) if !p.is_empty() => p,
            _ => continue, // entry block — params are function args, don't coalesce
        };

        // For each param, check if all predecessors pass the same value
        let mut params_to_coalesce: Vec<(usize, Value)> = Vec::new();

        for param_idx in 0..param_count {
            let mut common_value: Option<Value> = None;
            let mut all_same = true;

            for &pred_block in pred_list {
                let pred_term = &func.dfg.blocks[pred_block.0 as usize].terminator;
                let arg = match pred_term {
                    Terminator::Jump { args, .. } => args.get(param_idx).copied(),
                    Terminator::Branch {
                        then_block,
                        then_args,
                        else_block,
                        else_args,
                        ..
                    } => {
                        if *then_block == block {
                            then_args.get(param_idx).copied()
                        } else if *else_block == block {
                            else_args.get(param_idx).copied()
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                match (common_value, arg) {
                    (None, Some(v)) => common_value = Some(v),
                    (Some(cv), Some(v)) if cv == v => {} // same, continue
                    (Some(_), Some(_)) => {
                        all_same = false;
                        break;
                    }
                    _ => {
                        all_same = false;
                        break;
                    }
                }
            }

            if all_same && let Some(v) = common_value {
                params_to_coalesce.push((param_idx, v));
            }
        }

        if params_to_coalesce.is_empty() {
            continue;
        }

        result.changed = true;

        // Process in reverse order so indices stay valid
        for &(param_idx, replacement) in params_to_coalesce.iter().rev() {
            let param_val = func.dfg.blocks[bi].param_values[param_idx];

            // RAUW（DFG + use-lists 双更新）
            func.replace_all_uses(param_val, replacement);

            // 原子删除参数（含所有前驱终结符对应 args 清理）
            func.remove_block_param(block, param_idx);

            result.values_replaced += 1;
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesce_no_crash_no_params() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.ret(&[v]);
        let mut func = b.finish().expect("build");

        let pass = BlockParamCoalescePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed);
    }

    #[test]
    fn coalesce_entry_block_params_untouched() {
        // Entry block params are function args, should not be coalesced
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("id", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        b.ret(&[params[0]]);
        let mut func = b.finish().expect("build");

        let pass = BlockParamCoalescePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed); // Entry block params should not be coalesced
    }

    #[test]
    fn coalesce_redundant_block_param() {
        // Create a diamond CFG where merge block's param always gets the same value
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("redundant", TypeContext::new(), sig);
        let entry = b.create_block();
        let then_block = b.create_block();
        let else_block = b.create_block();
        let merge_block = b.create_block_with_tys(&[TypeId::I32]);

        b.switch_to_block(entry);
        let c42 = b.iconst_i32(42);
        let c0 = b.iconst_i32(0);
        let cond = b.icmp(IntCC::NotEqual, c42, c0);
        b.branch(cond, then_block, &[], else_block, &[]);

        b.switch_to_block(then_block);
        b.jump(merge_block.0, &[c42]);

        b.switch_to_block(else_block);
        b.jump(merge_block.0, &[c42]); // Both branches pass c42!

        b.switch_to_block(merge_block.0);
        let _merge_val = merge_block.1[0];
        b.ret(&[c42]);

        let mut func = b.finish().expect("build");
        let param_count_before = func.dfg.blocks[3].params.len();
        assert_eq!(param_count_before, 1);

        let pass = BlockParamCoalescePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Should coalesce redundant block param");
        assert_eq!(
            func.dfg.blocks[3].params.len(),
            0,
            "Param should be removed"
        );
    }

    #[test]
    fn coalesce_no_change_for_different_values() {
        // Diamond where branches pass DIFFERENT values — should NOT coalesce
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("different", TypeContext::new(), sig);
        let entry = b.create_block();
        let then_block = b.create_block();
        let else_block = b.create_block();
        let merge_block = b.create_block_with_tys(&[TypeId::I32]);

        b.switch_to_block(entry);
        let c42 = b.iconst_i32(42);
        let c99 = b.iconst_i32(99);
        let cond = b.icmp(IntCC::NotEqual, c42, c99);
        b.branch(cond, then_block, &[], else_block, &[]);

        b.switch_to_block(then_block);
        b.jump(merge_block.0, &[c42]);

        b.switch_to_block(else_block);
        b.jump(merge_block.0, &[c99]); // DIFFERENT value!

        b.switch_to_block(merge_block.0);
        let merge_val = merge_block.1[0];
        b.ret(&[merge_val]);

        let mut func = b.finish().expect("build");
        let param_count_before = func.dfg.blocks[3].params.len();
        assert_eq!(param_count_before, 1);

        let pass = BlockParamCoalescePass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed, "Should NOT coalesce when values differ");
        assert_eq!(func.dfg.blocks[3].params.len(), 1, "Param should remain");
    }
}
