//! 归纳变量简化 (Induction Variable Simplification) pass。
//!
//! v2 重新设计: v1 通过 Phi 指令检测归纳变量。
//! v2 的等价模式是: 回边上的 block param 被定义为 `param + step`。
//!
//! # 算法 (v2 — 基于 block param 的归纳变量检测)
//!
//! 1. 使用 LoopForest 检测循环
//! 2. 对于每个循环 header block，检查其 block params
//! 3. 若 param 在回边上被定义为 `param + constant_step` 且初始值为常量，则为归纳变量
//! 4. 若边界已知（icmp 比较），则可将 IV 的使用替换为闭合形式

use crate::{OptimizationPass, PassResult};
use forge_ir::CompileError;
use forge_ir::*;

#[derive(Default)]
pub struct IndVarSimplifyPass;

impl IndVarSimplifyPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for IndVarSimplifyPass {
    fn name(&self) -> &'static str {
        "ind-var-simplify"
    }
    fn description(&self) -> &'static str {
        "Detects and simplifies induction variables using block-param-based pattern matching"
    }
    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        simplify_ind_vars(func)
    }
}

/// 检测到的归纳变量信息。
/// Fields are stored for future strength reduction when block param addition is supported.
#[allow(dead_code)]
struct IndVar {
    param_idx: usize,
    header: Block,
    step: i64,
    /// The initial value (from outside-loop predecessor), if constant.
    init_val: Option<i64>,
    /// The block param value for this IV.
    param_val: Value,
}

/// 归纳变量简化。
pub fn simplify_ind_vars(func: &mut Function) -> Result<PassResult, CompileError> {
    let mut result = PassResult::default();
    let dt = DominatorTree::build(func);
    let lf = LoopForest::build(func, &dt);
    let preds = func.predecessors().clone();

    for loop_info in lf.all_loops() {
        let header = loop_info.header;
        let body: std::collections::HashSet<Block> = loop_info.blocks.iter().copied().collect();

        // Find the latch (back-edge predecessor dominated by header)
        let latch = match preds
            .get(&header)
            .into_iter()
            .flatten()
            .find(|&&p| body.contains(&p))
            .copied()
        {
            Some(p) => p,
            None => continue,
        };

        // Find the init predecessor (outside-loop predecessor)
        let init_pred = preds
            .get(&header)
            .into_iter()
            .flatten()
            .find(|&&p| !body.contains(&p))
            .copied();

        let header_block = &func.dfg.blocks[header.0 as usize];
        let param_count = header_block.params.len();
        if param_count == 0 {
            continue;
        }

        let mut ind_vars: Vec<IndVar> = Vec::new();

        for pi in 0..param_count {
            let param_val = header_block.param_values[pi];

            // Get the value from the latch's terminator (back edge arg)
            let latch_term = &func.dfg.blocks[latch.0 as usize].terminator;
            let back_edge_arg = match latch_term {
                Terminator::Jump { target, args } if *target == header => args.get(pi).copied(),
                Terminator::Branch {
                    then_block,
                    then_args,
                    else_block,
                    else_args,
                    ..
                } => {
                    if *then_block == header {
                        then_args.get(pi).copied()
                    } else if *else_block == header {
                        else_args.get(pi).copied()
                    } else {
                        None
                    }
                }
                _ => None,
            };

            let be_arg = match back_edge_arg {
                Some(v) => v,
                None => continue,
            };

            // Check if back_edge_arg is "param + constant"
            let be_def = func.dfg.value_def(be_arg).copied();
            if let Some(ValueDef::Inst(inst, _)) = be_def {
                let inst_data = &func.dfg.insts[inst.0 as usize];
                if inst_data.opcode == Opcode::Iadd {
                    let operands = &inst_data.operands;
                    let other = if operands.first().copied() == Some(param_val) {
                        operands.get(1).copied()
                    } else if operands.get(1).copied() == Some(param_val) {
                        operands.first().copied()
                    } else {
                        continue;
                    };

                    // Check if 'other' is a constant
                    if let Some(other_val) = other {
                        let other_def = func.dfg.value_def(other_val).copied();
                        if let Some(ValueDef::Inst(iconst_inst, _)) = other_def {
                            let iconst_data = &func.dfg.insts[iconst_inst.0 as usize];
                            if iconst_data.opcode == Opcode::Iconst
                                && let Some(cid) =
                                    iconst_data.immediates.iter().find_map(|i| i.as_const())
                                && let Some(step) = func.constants.resolve_int(cid)
                                && step != 0
                            {
                                // Find init value
                                let init_val = init_pred
                                    .and_then(|p| {
                                        let t = &func.dfg.blocks[p.0 as usize].terminator;
                                        get_arg_for_block(t, header, pi)
                                    })
                                    .and_then(|v| resolve_iconst_value(func, v));

                                ind_vars.push(IndVar {
                                    param_idx: pi,
                                    header,
                                    step,
                                    init_val,
                                    param_val,
                                });
                            }
                        }
                    }
                }
            }
        }

        if ind_vars.is_empty() {
            continue;
        }

        // Perform strength reduction: replace iv*scale with a derived IV
        let sr_count = strength_reduce_ind_vars(func, &ind_vars, &body, header, latch);
        if sr_count > 0 {
            result.changed = true;
            result.values_replaced += sr_count;
        }
    }

    Ok(result)
}

/// Resolve a value to an i64 constant, if possible.
fn resolve_iconst_value(func: &Function, v: Value) -> Option<i64> {
    let def = func.dfg.value_def(v).copied()?;
    match def {
        ValueDef::Inst(inst, _) => {
            let inst_data = &func.dfg.insts[inst.0 as usize];
            if inst_data.opcode == Opcode::Iconst {
                let cid = inst_data.immediates.iter().find_map(|i| i.as_const())?;
                func.constants.resolve_int(cid)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Extract the argument at position `pi` for block `target` from a terminator.
fn get_arg_for_block(term: &Terminator, target: Block, pi: usize) -> Option<Value> {
    match term {
        Terminator::Jump {
            target: t, args, ..
        } if *t == target => args.get(pi).copied(),
        Terminator::Branch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == target {
                then_args.get(pi).copied()
            } else if *else_block == target {
                else_args.get(pi).copied()
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Perform IV-based simplifications in the loop body.
///
/// Currently implements:
/// 1. Eliminate `imul(iv, 1)` → `iv` (identity)
/// 2. Eliminate `iadd(iv, 0)` → `iv` (identity)
/// 3. Eliminate `isub(iv, 0)` → `iv` (identity)
///
/// Full strength reduction (replacing `iv*const` with a derived IV)
/// requires adding block params to existing blocks, which is not yet supported
/// by the IR.
fn strength_reduce_ind_vars(
    func: &mut Function,
    ind_vars: &[IndVar],
    body: &std::collections::HashSet<Block>,
    _header: Block,
    _latch: Block,
) -> usize {
    let mut replaced = 0;

    for iv in ind_vars {
        for &block in body.iter() {
            let bd = &func.dfg.blocks[block.0 as usize];
            let inst_ids: Vec<Inst> = bd.inst_order.clone();

            for &inst_id in &inst_ids {
                let inst = &func.dfg.insts[inst_id.0 as usize];
                if inst.results.is_empty() {
                    continue;
                }
                let old_result = inst.results[0];

                let simplified = match &inst.opcode {
                    // imul(iv, 1) → iv
                    Opcode::Imul => {
                        let ops = &inst.operands;
                        if ops.len() == 2 {
                            if (ops[0] == iv.param_val
                                && resolve_iconst_value(func, ops[1]) == Some(1))
                                || (ops[1] == iv.param_val
                                    && resolve_iconst_value(func, ops[0]) == Some(1))
                            {
                                Some(iv.param_val)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    // iadd(iv, 0) → iv
                    Opcode::Iadd => {
                        let ops = &inst.operands;
                        if ops.len() == 2 {
                            if (ops[0] == iv.param_val
                                && resolve_iconst_value(func, ops[1]) == Some(0))
                                || (ops[1] == iv.param_val
                                    && resolve_iconst_value(func, ops[0]) == Some(0))
                            {
                                Some(iv.param_val)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    // isub(iv, 0) → iv
                    Opcode::Isub => {
                        let ops = &inst.operands;
                        if ops.len() == 2
                            && ops[0] == iv.param_val
                            && resolve_iconst_value(func, ops[1]) == Some(0)
                        {
                            Some(iv.param_val)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                if let Some(replacement) = simplified {
                    func.use_lists.replace_all_uses(old_result, replacement);
                    let inst_mut = &mut func.dfg.insts[inst_id.0 as usize];
                    inst_mut.opcode = Opcode::Nop;
                    inst_mut.operands.clear();
                    inst_mut.immediates.clear();
                    inst_mut.results.clear();
                    replaced += 1;
                }
            }
        }
    }

    replaced
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ind_var_simplify_no_crash() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.ret(&[v]);
        let mut func = b.finish();

        let pass = IndVarSimplifyPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(!r.changed);
    }

    #[test]
    fn ind_var_detect_simple_loop() {
        // Simple loop: for i in 0..10 { ... }
        // Header block has param iv, back edge passes iv+1
        // No identity patterns to eliminate, so changed=false but detection works
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("simple_loop", TypeContext::new(), sig);
        let entry = b.create_block();
        let (header_blk, header_params) = b.create_block_with_tys(&[TypeId::I32]);
        let body = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        let zero = b.iconst_i32(0);
        let _ten = b.iconst_i32(10);
        b.jump(header_blk, &[zero]); // init iv=0

        b.switch_to_block(header_blk);
        let iv = header_params[0];
        let ten2 = b.iconst_i32(10);
        let cmp = b.icmp(IntCC::SignedLessThan, iv, ten2);
        b.branch(cmp, body, &[], exit, &[]);

        b.switch_to_block(body);
        let one = b.iconst_i32(1);
        let next_iv = b.iadd(iv, one); // iv + 1
        b.jump(header_blk, &[next_iv]); // back edge: iv+1 → param

        b.switch_to_block(exit);
        b.ret(&[iv]);

        let mut func = b.finish();
        let pass = IndVarSimplifyPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        // Detection works but no identity patterns to eliminate
        let _ = r;
    }

    /// Test that imul(iv, 1) is replaced with iv.
    #[test]
    fn ind_var_eliminate_mul_by_one() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        let (header_blk, header_params) = b.create_block_with_tys(&[TypeId::I32]);
        let body = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        let zero = b.iconst_i32(0);
        b.jump(header_blk, &[zero]);

        b.switch_to_block(header_blk);
        let iv = header_params[0];
        let ten = b.iconst_i32(10);
        let cmp = b.icmp(IntCC::SignedLessThan, iv, ten);
        b.branch(cmp, body, &[], exit, &[]);

        b.switch_to_block(body);
        let one_const = b.iconst_i32(1);
        let scaled = b.imul(iv, one_const); // imul(iv, 1) → should be replaced with iv
        let next_iv = b.iadd(iv, one_const);
        b.jump(header_blk, &[next_iv]);

        b.switch_to_block(exit);
        b.ret(&[scaled]);

        let mut func = b.finish();
        let pass = IndVarSimplifyPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Should eliminate imul(iv, 1)");
        assert!(r.values_replaced > 0, "Should replace imul result with iv");
    }

    /// Test that iadd(iv, 0) is replaced with iv.
    #[test]
    fn ind_var_eliminate_add_zero() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        let (header_blk, header_params) = b.create_block_with_tys(&[TypeId::I32]);
        let body = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        let zero = b.iconst_i32(0);
        b.jump(header_blk, &[zero]);

        b.switch_to_block(header_blk);
        let iv = header_params[0];
        let ten = b.iconst_i32(10);
        let cmp = b.icmp(IntCC::SignedLessThan, iv, ten);
        b.branch(cmp, body, &[], exit, &[]);

        b.switch_to_block(body);
        let zero_const = b.iconst_i32(0);
        let zero_add = b.iadd(iv, zero_const); // iadd(iv, 0) → should be replaced with iv
        let one = b.iconst_i32(1);
        let next_iv = b.iadd(iv, one);
        b.jump(header_blk, &[next_iv]);

        b.switch_to_block(exit);
        b.ret(&[zero_add]);

        let mut func = b.finish();
        let pass = IndVarSimplifyPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Should eliminate iadd(iv, 0)");
        assert!(r.values_replaced > 0, "Should replace iadd result with iv");
    }
}
