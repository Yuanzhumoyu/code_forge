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
use forge_ir::IrError;
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

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        thread_jumps(func)
    }
}

/// 对函数执行跳转线程优化。
pub fn thread_jumps(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();
    let block_count = func.dfg.blocks.len();
    let entry = func.entry();

    loop {
        let mut changed = false;

        // 1. Collect predecessor info (recompute each iteration, clone to release borrow)
        let preds = func.predecessors().clone();

        // 2. Fold jump chains: if target block is empty with only Jump, redirect directly
        for bi in 0..block_count {
            let target_info = {
                let block_id = Block(bi as u32);
                if let Some((target, args)) = func.dfg.term_jump(block_id) {
                    if args.is_empty() {
                        // Check if target block is an empty block with only Jump
                        let target_block = &func.dfg.blocks[target.0 as usize];
                        if target_block.inst_order.is_empty()
                            && target_block.params.is_empty()
                            && func.dfg.term_kind(target) == Some(TermKind::Jump)
                        {
                            func.dfg
                                .term_jump(target)
                                .map(|(final_target, final_args)| {
                                    (target, final_target, final_args.to_vec())
                                })
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
                func.jump(Block(bi as u32), final_target, &final_args);
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
            let (is_empty_jump, jump_target, jump_args): (
                bool,
                Block,
                smallvec::SmallVec<[Value; 2]>,
            ) = {
                let block = &func.dfg.blocks[bi];
                if block.inst_order.is_empty() && block.params.is_empty() {
                    if let Some((target, args)) = func.dfg.term_jump(block_id) {
                        (true, target, args.iter().copied().collect())
                    } else {
                        (false, Block(0), smallvec![])
                    }
                } else {
                    (false, Block(0), smallvec![])
                }
            };

            if is_empty_jump {
                // Get predecessors and redirect（用结构化 API：retarget + 整体替换 args）
                let pred_list: Vec<Block> = preds.get(block_id).cloned().unwrap_or_default();
                for &pred_id in &pred_list {
                    let target = jump_target;
                    let new_args = jump_args.clone();
                    // 原实现：then 与 else 都指向 block_id 时合并为 Jump（丢弃 cond）。
                    // 保持该语义——否则 Branch 的两个分支都指向同一空 jump 块。
                    let both_sides = matches!(
                        func.dfg.term_branch(pred_id),
                        Some((_, t, _, e, _)) if t == block_id && e == block_id
                    );
                    if both_sides {
                        func.jump(pred_id, target, &new_args);
                    } else {
                        // 目标改指 + 实参整体替换（两个写入口都自动重登记 use 项）
                        func.retarget_terminator(pred_id, block_id, target);
                        func.replace_terminator_args(pred_id, target, &new_args);
                    }
                }
                // Set empty block to Unreachable
                func.unreachable(Block(bi as u32));
                changed = true;
                result.blocks_removed += 1;
            }
        }

        if !changed {
            break;
        }
    }

    result.changed = result.blocks_removed > 0;
    if result.changed {
        // 终止符/块已修改：失效分析缓存（前驱/后继/支配树），
        // 避免后续 pass 复用 stale 的 CFG 分析。
        func.analysis_mut().invalidate();
    }
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

        let mut func = b.finish().expect("build");
        let pass = JumpThreadPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        // B0 should now jump directly to B2
        assert!(
            func.dfg
                .term_jump(Block(0))
                .is_some_and(|(target, _)| target == b2)
        );
        // B1 should be unreachable
        assert!(func.dfg.term_is_unreachable(Block(1)));
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

        let mut func = b.finish().expect("build");
        let pass = JumpThreadPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed);
        // B1 and B2 should be unreachable
        assert!(func.dfg.term_is_unreachable(Block(1)));
        assert!(func.dfg.term_is_unreachable(Block(2)));
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

        let mut func = b.finish().expect("build");
        let pass = JumpThreadPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed);
    }
}
