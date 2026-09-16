//! 链接时优化 (LTO) 支持。
//!
//! v2: 使用 forge-ir 的 `Module` 类型进行跨模块 IR 管理。
//! LtoPass 使用跨模块函数表在模块间执行函数内联。
//!
//! # 算法
//!
//! 1. LtoContext 管理多个 Module
//! 2. build_callee_map() 构建 FuncRef → (module_idx, &Function) 映射
//! 3. LtoPass::run_on_module() 遍历目标模块中的 Call 指令
//! 4. 对于调用外部模块函数的 Call，克隆 callee body 并内联

use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;
use std::collections::HashMap;

/// LTO 上下文 — 管理跨模块 IR。
pub struct LtoContext {
    modules: Vec<Module>,
}

impl LtoContext {
    pub fn new() -> Self {
        Self {
            modules: Vec::new(),
        }
    }

    pub fn add_module(&mut self, module: Module) {
        self.modules.push(module);
    }

    /// 从所有模块构建全局函数引用表。
    pub fn build_ref_table(&self) -> HashMap<ImmStr, FuncRef> {
        let mut table = HashMap::new();
        for module in &self.modules {
            for (fr, func) in module.iter_func_refs() {
                table.insert(func.name.clone(), fr);
            }
        }
        table
    }

    /// 获取所有模块中的函数引用。
    pub fn all_func_refs(&self) -> Vec<(FuncRef, &str)> {
        self.modules
            .iter()
            .flat_map(|m| m.iter_func_refs())
            .map(|(fr, f)| (fr, f.name.as_str()))
            .collect()
    }

    /// 构建跨模块 callee 查找映射: FuncRef → &Function
    pub fn build_callee_map(&self) -> HashMap<FuncRef, (&Function, usize)> {
        let mut map = HashMap::new();
        for (mi, module) in self.modules.iter().enumerate() {
            for (fr, func) in module.iter_func_refs() {
                map.entry(fr).or_insert((func, mi));
            }
        }
        map
    }
}

impl Default for LtoContext {
    fn default() -> Self {
        Self::new()
    }
}

/// LTO pass — 跨模块内联优化。
#[doc(hidden)]
pub struct LtoPass {
    context: LtoContext,
    threshold: usize,
}

impl LtoPass {
    pub fn new(context: LtoContext) -> Self {
        Self {
            context,
            threshold: 20,
        }
    }
}

impl OptimizationPass for LtoPass {
    fn name(&self) -> &'static str {
        "lto"
    }
    fn description(&self) -> &'static str {
        "Link-time optimization: cross-module function inlining"
    }
    fn is_function_pass(&self) -> bool {
        false
    }

    fn run_on_module(&self, module: &mut Module) -> Result<PassResult, IrError> {
        let mut result = PassResult::default();

        // Build lookup: FuncRef → callee function (only from OTHER modules)
        let callee_map = self.context.build_callee_map();

        // Collect all cross-module call sites
        struct CallSite {
            func_ref: FuncRef,
            call_inst: Inst,
            _block: Block,
        }

        let mut call_sites: Vec<CallSite> = Vec::new();
        for (_fr, func) in module.iter_func_refs() {
            for bi in 0..func.dfg.block_count() {
                let block_id = Block::new(bi as u32);
                let block_data = &func.dfg.block(Block::new(bi as u32));
                for &inst_id in &block_data.inst_order {
                    let inst = &func.dfg.inst_data(inst_id);
                    if inst.opcode != Opcode::Call {
                        continue;
                    }
                    let callee_ref = inst.immediates.iter().find_map(|i| i.as_func());
                    if let Some(cr) = callee_ref {
                        // Only inline if callee is from another module
                        // (same-module inlining is handled by InlinePass)
                        if callee_map.contains_key(&cr) {
                            call_sites.push(CallSite {
                                func_ref: _fr,
                                call_inst: inst_id,
                                _block: block_id,
                            });
                        }
                    }
                }
            }
        }

        // Process each call site
        for site in call_sites {
            let Some((callee_func, _callee_module_idx)) = callee_map.get(&site.func_ref) else {
                continue;
            };

            // Skip large functions
            let callee_size: usize = callee_func
                .dfg
                .block_data_iter()
                .map(|b| b.inst_order.len())
                .sum();
            if callee_size > self.threshold {
                continue;
            }

            // Get caller function
            let caller = module.get_function_mut(site.func_ref);

            let call_inst_data = &caller.dfg.inst_data(site.call_inst);
            let call_block = call_inst_data.block;
            let call_results: Vec<Value> = call_inst_data.results.iter().copied().collect();
            let call_operands: Vec<Value> = call_inst_data.operands.iter().copied().collect();

            // Clone callee body into caller
            let ret_vals = lto_inline_callee(caller, callee_func, call_block, &call_operands)?;

            // Replace call results with return values（DFG + use-lists 双更新）
            for (i, &call_result) in call_results.iter().enumerate() {
                if let Some(&ret_val) = ret_vals.get(i) {
                    caller.replace_all_uses(call_result, ret_val);
                }
            }

            // Remove Call instruction（原子：use-lists + 墓碑化）
            caller.kill_inst(site.call_inst);
            result.instructions_removed += 1;
            result.values_replaced += call_results.len();
            result.changed = true;
        }

        Ok(result)
    }
}

/// Clone a callee function's body into a caller function at a specific block.
/// Returns the values corresponding to the callee's Return.
fn lto_inline_callee(
    caller: &mut Function,
    callee: &Function,
    target_block: Block,
    call_args: &[Value],
) -> Result<Vec<Value>, IrError> {
    let mut val_remap: HashMap<Value, Value> = HashMap::new();

    // Map callee params to call arguments
    for callee_block in callee.dfg.block_data_iter() {
        for (pv, arg) in callee_block.param_values.iter().zip(call_args.iter()) {
            val_remap.insert(*pv, *arg);
        }
    }

    // Clone instructions from callee into caller
    let mut ret_vals: Vec<Value> = Vec::new();
    for bi in 0..callee.dfg.block_count() {
        let callee_block = &callee.dfg.block(Block::new(bi as u32));

        for &inst_id in &callee_block.inst_order {
            let inst = &callee.dfg.inst_data(inst_id);

            // Remap operands
            let new_operands: smallvec::SmallVec<[Value; 4]> = inst
                .operands
                .iter()
                .map(|v| val_remap.get(v).copied().unwrap_or(*v))
                .collect();

            // Remap immediates: copy constants from callee pool to caller pool
            //（remap_from 按 tag 全池重建，替代手写三级 fallback）
            let new_immediates: smallvec::SmallVec<[Immediate; 4]> = inst
                .immediates
                .iter()
                .map(|im| match im {
                    Immediate::Const(cid) => {
                        Immediate::Const(caller.constants.remap_from(&callee.constants, *cid))
                    }
                    _ => *im,
                })
                .collect();

            let result_tys: Vec<TypeId> = inst
                .results
                .iter()
                .map(|&v| callee.dfg.value_data(v).ty)
                .collect();

            // 保留全字段（flags/mem_flags/metadata/loc/isel_strategy）
            let new_inst = caller.make_inst_with_meta_and_loc(
                inst.opcode,
                target_block,
                new_operands,
                new_immediates,
                &result_tys,
                inst.flags,
                inst.mem_flags,
                inst.metadata().iter().cloned().collect(),
                inst.loc.clone(),
            );
            if let Some(strategy) = inst.isel_strategy() {
                caller
                    .dfg
                    .inst_mut(new_inst)
                    .set_isel_strategy(strategy.clone());
            }

            // Map old results to new results
            let new_results = &caller.dfg.inst_data(new_inst).results;
            for (i, &old_r) in inst.results.iter().enumerate() {
                if i < new_results.len() {
                    val_remap.insert(old_r, new_results[i]);
                }
            }
        }

        // Handle Return
        if let Some(values) = callee.dfg.term_return_values(Block::new(bi as u32)) {
            for &v in values {
                ret_vals.push(val_remap.get(&v).copied().unwrap_or(v));
            }
        }
    }

    Ok(ret_vals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lto_context_empty() {
        let ctx = LtoContext::new();
        assert!(ctx.all_func_refs().is_empty());
    }

    #[test]
    fn lto_context_with_module() {
        let mut ctx = LtoContext::new();
        let sig = FunctionSignature::new(&[], &[]);
        let mut module = Module::new();
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        b.ret(&[]);
        module.add_function(b.finish().expect("build"));
        ctx.add_module(module);

        assert_eq!(ctx.all_func_refs().len(), 1);
    }

    #[test]
    fn lto_pass_no_crash() {
        let ctx = LtoContext::new();
        let sig = FunctionSignature::new(&[], &[]);
        let mut module = Module::new();
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        b.ret(&[]);
        module.add_function(b.finish().expect("build"));

        let pass = LtoPass::new(ctx);
        let r = pass.run_on_module(&mut module).unwrap();
        assert!(!r.changed); // no other modules to inline from
    }
}
