//! 跳转线程 (Jump Threading) pass。
//!
//! 检测并优化冗余跳转链和空块：
//!
//! 1. **跳转链折叠**: `A: jmp B` → `B: jmp C` 变为 `A: jmp C`
//! 2. **空块消除**: 无指令、无参数、只有无条件跳转的块，
//!    将其所有前驱直接重定向到跳转目标。
//!
//! # 算法
//!
//! 迭代进行直到不动点：
//! 1. 收集所有前驱信息
//! 2. 对每个空块（无指令、无参数、仅 Jump），重定向所有前驱
//! 3. 对每个以 Jump 结尾的块，检查目标是否为仅含 Jump 的空块，重定向

use crate::{OptimizationPass, PassResult};
use forge_ir::CompileError;
use forge_ir::*;
use smallvec::smallvec;

/// 跳转线程 pass。
#[derive(Default)]
pub struct JumpThreadPass;

impl JumpThreadPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for JumpThreadPass {
    fn name(&self) -> &'static str {
        "jump-thread"
    }

    fn description(&self) -> &'static str {
        "Folds chains of unconditional jumps and eliminates empty blocks"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        thread_jumps(func)
    }
}

/// 对函数执行跳转线程优化。
pub fn thread_jumps(func: &mut Function) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    let block_count = func.dfg.blocks.len();
    let entry = func.entry_block.unwrap_or(Block(0));

    loop {
        let mut changed = false;

        // 1. Collect predecessor info (recompute each iteration, clone to release borrow)
        let preds = func.predecessors().clone();

        // 2. Fold jump chains: if target block is empty with only Jump, redirect directly
        for bi in 0..block_count {
            let target_info = {
                let block = &func.dfg.blocks[bi];
                if let Terminator::Jump { target, args } = &block.terminator {
                    if args.is_empty() {
                        // Check if target block is an empty block with only Jump
                        let target_block = &func.dfg.blocks[target.0 as usize];
                        if target_block.inst_order.is_empty()
                            && target_block.params.is_empty()
                            && matches!(target_block.terminator, Terminator::Jump { .. })
                        {
                            if let Terminator::Jump {
                                target: final_target,
                                args: final_args,
                            } = &target_block.terminator
                            {
                                Some((*target, *final_target, final_args.clone()))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some((_, final_target, final_args)) = target_info {
                func.dfg.blocks[bi].terminator = Terminator::Jump {
                    target: final_target,
                    args: final_args,
                };
                changed = true;
                result.blocks_removed += 1;
            }
        }

        // 3. Empty block elimination: no insts, no params, only Jump
        for bi in 0..block_count {
            let block_id = Block(bi as u32);
            if block_id == entry {
                continue; // preserve entry block
            }
            let (is_empty_jump, jump_target, jump_args) = {
                let block = &func.dfg.blocks[bi];
                if block.inst_order.is_empty() && block.params.is_empty() {
                    if let Terminator::Jump { target, args } = &block.terminator {
                        (true, *target, args.clone())
                    } else {
                        (false, Block(0), smallvec![])
                    }
                } else {
                    (false, Block(0), smallvec![])
                }
            };

            if is_empty_jump {
                // Get predecessors and redirect
                let pred_list: Vec<Block> = preds.get(&block_id).cloned().unwrap_or_default();
                for &pred_id in &pred_list {
                    let pred_block = &mut func.dfg.blocks[pred_id.0 as usize];
                    match &mut pred_block.terminator {
                        Terminator::Branch {
                            then_block,
                            else_block,
                            then_args,
                            else_args,
                            ..
                        } => {
                            let is_then = *then_block == block_id;
                            let is_else = *else_block == block_id;
                            if is_then && is_else {
                                pred_block.terminator = Terminator::Jump {
                                    target: jump_target,
                                    args: jump_args.clone(),
                                };
                            } else if is_then {
                                *then_block = jump_target;
                                *then_args = jump_args.clone();
                            } else if is_else {
                                *else_block = jump_target;
                                *else_args = jump_args.clone();
                            }
                        }
                        Terminator::Jump { target, args } if *target == block_id => {
                            *target = jump_target;
                            *args = jump_args.clone();
                        }
                        Terminator::Switch {
                            default_block,
                            cases,
                            ..
                        } => {
                            if *default_block == block_id {
                                *default_block = jump_target;
                            }
                            for (_, case_target, case_args) in cases.iter_mut() {
                                if *case_target == block_id {
                                    *case_target = jump_target;
                                    *case_args = jump_args.clone();
                                }
                            }
                        }
                        _ => {}
                    }
                }
                // Set empty block to Unreachable
                func.dfg.blocks[bi].terminator = Terminator::Unreachable;
                changed = true;
                result.blocks_removed += 1;
            }
        }

        if !changed {
            break;
        }
    }

    result.changed = result.blocks_removed > 0;
    Ok(result)
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use FunctionBuilder;

    #[test]
    fn jump_thread_chain() {
        // B0: jmp B1 → B1: jmp B2 (empty) → B2: return
        // After: B0: jmp B2, B1: unreachable
        let sig = FunctionSignature::new(&[], &[]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let b0 = b.create_block();
        let b1 = b.create_block();
        let b2 = b.create_block();

        b.switch_to_block(b0);
        b.jump(b1, &[]);

        b.switch_to_block(b1);
        b.jump(b2, &[]);

        b.switch_to_block(b2);
        b.ret(&[]);

        let mut func = b.finish();
        let pass = JumpThreadPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        // B0 should now jump directly to B2
        assert!(
            matches!(&func.dfg.blocks[0].terminator, Terminator::Jump { target, .. } if *target == b2)
        );
        // B1 should be unreachable
        assert!(matches!(
            func.dfg.blocks[1].terminator,
            Terminator::Unreachable
        ));
    }

    #[test]
    fn empty_block_elimination() {
        // B0: cond → br B1, B2 → B1/B2 empty jump to B3 → B3: ret
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let b0 = b.create_block();
        let b1 = b.create_block();
        let b2 = b.create_block();
        let b3 = b.create_block();

        b.switch_to_block(b0);
        let v1 = b.iconst_i32(1);
        let v0 = b.iconst_i32(0);
        let cond = b.icmp(IntCC::NotEqual, v1, v0);
        b.branch(cond, b1, &[], b2, &[]);

        b.switch_to_block(b1);
        b.jump(b3, &[]);

        b.switch_to_block(b2);
        b.jump(b3, &[]);

        b.switch_to_block(b3);
        b.ret(&[]);

        let mut func = b.finish();
        let pass = JumpThreadPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        // B1 and B2 should be unreachable
        assert!(matches!(
            func.dfg.blocks[1].terminator,
            Terminator::Unreachable
        ));
        assert!(matches!(
            func.dfg.blocks[2].terminator,
            Terminator::Unreachable
        ));
    }

    #[test]
    fn no_change_for_non_empty_block() {
        // B0: iconst; jmp B1 (with args)
        // B1: ret
        // Should NOT change because B1 has args
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let b0 = b.create_block();
        let b1 = b.create_block();

        b.switch_to_block(b0);
        let v = b.iconst_i32(42);
        b.jump(b1, &[v]);

        b.switch_to_block(b1);
        b.ret(&[]);

        let mut func = b.finish();
        let pass = JumpThreadPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed);
    }
}
