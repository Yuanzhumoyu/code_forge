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

use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};

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

    loop {
        let mut changed = false;

        // 1. 收集前驱信息（每次迭代重新计算）
        let preds = func.predecessors();

        // 2. 折叠跳转链: 如果目标块只有无条件跳转且无参数，直接跳转到最终目标
        for block_idx in 0..func.blocks.len() {
            let target_info = {
                let block = &func.blocks[block_idx];
                if let Terminator::Jump { target, args } = &block.terminator {
                    if args.is_empty() {
                        // 检查目标块是否为仅含 Jump 的空块
                        let target_block = &func.blocks[target.0 as usize];
                        if target_block.instructions.is_empty()
                            && target_block.params.is_empty()
                            && matches!(target_block.terminator, Terminator::Jump { .. })
                        {
                            if let Terminator::Jump {
                                target: final_target,
                                args: final_args,
                            } = &target_block.terminator
                            {
                                let _ = target; // target is the same as final_target's predecessor
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

            if let Some((_target, final_target, final_args)) = target_info {
                func.blocks[block_idx].terminator = Terminator::Jump {
                    target: final_target,
                    args: final_args,
                };
                changed = true;
                result.blocks_removed += 1;
            }
        }

        // 3. 空块消除: 无指令、无参数、仅 Jump 且无前驱重定向冲突
        #[allow(clippy::needless_range_loop)]
        for block_idx in 0..func.blocks.len() {
            if block_idx == 0 {
                continue; // 保留入口块
            }
            let (is_empty_jump, jump_target, jump_args) = {
                let block = &func.blocks[block_idx];
                if block.instructions.is_empty() && block.params.is_empty() {
                    if let Terminator::Jump { target, args } = &block.terminator {
                        (true, *target, args.clone())
                    } else {
                        (false, BlockId(0), smallvec::smallvec![])
                    }
                } else {
                    (false, BlockId(0), smallvec::smallvec![])
                }
            };

            if is_empty_jump {
                let block_id = BlockId(block_idx as u32);
                // 重定向所有前驱
                for &pred_id in &preds[block_idx] {
                    let pred_block = &mut func.blocks[pred_id.0 as usize];
                    match &mut pred_block.terminator {
                        Terminator::Branch {
                            true_block,
                            false_block,
                            true_args,
                            false_args,
                            ..
                        } => {
                            let is_true = *true_block == block_id;
                            let is_false = *false_block == block_id;
                            if is_true && is_false {
                                // Both branches go to the same block — convert to Jump
                                pred_block.terminator = Terminator::Jump {
                                    target: jump_target,
                                    args: jump_args.clone(),
                                };
                            } else if is_true {
                                *true_block = jump_target;
                                *true_args = jump_args.clone();
                            } else if is_false {
                                *false_block = jump_target;
                                *false_args = jump_args.clone();
                            }
                        }
                        Terminator::Jump { target, args }
                            if *target == block_id => {
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
                // 将空块设为 Unreachable
                func.blocks[block_idx].terminator = Terminator::Unreachable;
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
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionBuilder;

    #[test]
    fn jump_thread_chain() {
        // B0: jmp B1
        // B1: jmp B2 (empty)
        // B2: return
        // After: B0: jmp B2, B1: unreachable
        let mut b = FunctionBuilder::new("test", Signature::void());
        let b0 = b.create_block();
        let b1 = b.create_block();
        let b2 = b.create_block();

        b.switch_to_block(b0);
        b.jump(b1, &[]);

        b.switch_to_block(b1);
        b.jump(b2, &[]);

        b.switch_to_block(b2);
        b.return_(&[]);

        let mut func = b.finish();
        let pass = JumpThreadPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        // B0 should now jump directly to B2
        assert!(
            matches!(func.blocks[0].terminator, Terminator::Jump { target, .. } if target == b2)
        );
        // B1 should be unreachable
        assert!(matches!(func.blocks[1].terminator, Terminator::Unreachable));
    }

    #[test]
    fn empty_block_elimination() {
        // B0: cond → br B1, B2
        // B1: jmp B3 (empty, no params)
        // B2: jmp B3 (empty, no params)
        // B3: return
        // After: B0: cond → br B3, B3 → converted to jmp B3 + B1/B2 unreachable
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let b0 = b.create_block();
        let b1 = b.create_block();
        let b2 = b.create_block();
        let b3 = b.create_block();

        b.switch_to_block(b0);
        let cond = b.iconst_i32(1);
        b.branch(cond, b1, b2, &[], &[]);

        b.switch_to_block(b1);
        b.jump(b3, &[]);

        b.switch_to_block(b2);
        b.jump(b3, &[]);

        b.switch_to_block(b3);
        b.return_(&[]);

        let mut func = b.finish();
        let pass = JumpThreadPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        // B1 and B2 should be unreachable
        assert!(matches!(func.blocks[1].terminator, Terminator::Unreachable));
        assert!(matches!(func.blocks[2].terminator, Terminator::Unreachable));
    }

    #[test]
    fn no_change_for_non_empty_block() {
        // B0: iconst; jmp B1
        // B1: iconst; ret
        // Should NOT change
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let b0 = b.create_block();
        let b1 = b.create_block();

        b.switch_to_block(b0);
        let v = b.iconst_i32(42);
        b.jump(b1, &[v]);

        b.switch_to_block(b1);
        b.return_(&[]);

        let mut func = b.finish();
        let pass = JumpThreadPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // Should not change because B1 has params or we pass args
        assert!(!r.changed);
    }
}
