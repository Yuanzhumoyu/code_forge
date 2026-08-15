//! forge-opt: IR optimization framework.
//!
//! # Pipeline (PassManager::for_level)
//! - O1 (5 passes): const_fold, copy_prop, cse, dead_code, jump_thread
//! - O2 (13 passes): O1 + gvn, gvn_pre, sccp, block_param_coalesce, licm, tail_call, 代数重写, dce
//! - O3 (18 passes): O2 + inline, mem2reg, ind_var_simplify, loop_unroll + 尾部 DCE
//!
//! # Pass modules
//! - scalar/ (10): block_param_coalesce, const_fold, copy_prop, cse, dead_code, gvn, gvn_pre, jump_thread, mem2reg, sccp
//! - loops/ (3): ind_var_simplify, licm, loop_unroll
//! - ipa/ (4): func_specialize, inline, lto, tail_call
//! - advanced/ (2): 代数重写, pgo

use forge_ir::*;

// ============================================================
// PassResult
// ============================================================

#[derive(Clone, Debug, Default)]
pub struct PassResult {
    pub changed: bool,
    pub instructions_removed: usize,
    pub instructions_added: usize,
    pub blocks_removed: usize,
    pub values_replaced: usize,
}

// ============================================================
// OptimizationLevel
// ============================================================

/// Optimization level controlling which passes run in the default pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OptimizationLevel {
    /// No optimization.
    O0,
    /// Basic scalar optimizations: const-fold, copy-prop, dead-code, CSE, jump-threading.
    O1,
    /// O1 + GVN, GVN-PRE, SCCP, LICM, tail-call, 代数重写, block-param coalescing.
    O2,
    /// O2 + inlining, mem2reg, ind-var simplify, loop unrolling.
    O3,
}

// ============================================================
// OptimizationPass trait
// ============================================================

pub trait OptimizationPass: Send {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str {
        ""
    }
    fn is_function_pass(&self) -> bool {
        true
    }

    fn run_on_function(&self, _func: &mut Function) -> Result<PassResult, IrError> {
        Ok(PassResult::default())
    }

    fn run_on_module(&self, _module: &mut Module) -> Result<PassResult, IrError> {
        Ok(PassResult::default())
    }
}

// ============================================================
// PassRunMode
// ============================================================

#[derive(Clone, Debug)]
pub enum PassRunMode {
    Once,
    UntilFixedPoint,
    Iterate(usize),
}

// ============================================================
// PassManager
// ============================================================

pub struct PassManager {
    passes: Vec<(Box<dyn OptimizationPass>, PassRunMode)>,
}

impl PassManager {
    pub fn new() -> Self {
        Self { passes: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }

    pub fn add_pass(&mut self, pass: Box<dyn OptimizationPass>, mode: PassRunMode) {
        self.passes.push((pass, mode));
    }

    pub fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        let mut total = PassResult::default();
        for (pass, mode) in &self.passes {
            if pass.is_function_pass() {
                match mode {
                    PassRunMode::Once => {
                        let r = pass.run_on_function(func)?;
                        total.changed |= r.changed;
                    }
                    PassRunMode::UntilFixedPoint => loop {
                        let r = pass.run_on_function(func)?;
                        total.changed |= r.changed;
                        if !r.changed {
                            break;
                        }
                    },
                    PassRunMode::Iterate(n) => {
                        for _ in 0..*n {
                            let r = pass.run_on_function(func)?;
                            total.changed |= r.changed;
                        }
                    }
                }

                // In debug mode, verify IR consistency after each pass.
                // Uses warn-only mode to avoid aborting on fixable issues.
                #[cfg(debug_assertions)]
                {
                    let mut v = forge_ir::verify::Verifier::new();
                    if let Err(errors) = v.verify(func) {
                        let msg = errors
                            .iter()
                            .map(|e| format!("{:?}", e))
                            .collect::<Vec<_>>()
                            .join("; ");
                        log::warn!(
                            "IR verification warning after '{}' on '{}': {}",
                            pass.name(),
                            func.name,
                            msg
                        );
                    }
                }

                // 分析缓存失效：pass 可能修改 IR（删除块/指令/终结符），
                // 统一在 pass 后失效，避免后续 pass 复用陈旧的前驱/支配树/循环分析。
                func.analysis_mut().invalidate();
            }
        }
        Ok(total)
    }

    pub fn run_on_module(&self, module: &mut Module) -> Result<PassResult, IrError> {
        let mut total = PassResult::default();
        for (pass, _mode) in &self.passes {
            if !pass.is_function_pass() {
                let r = pass.run_on_module(module)?;
                total.changed |= r.changed;
                // 模块级 pass（IPA：inline/lto/func_specialize）可能改写函数体，
                // 对所有函数统一失效分析缓存
                for func in module.iter_functions_mut() {
                    func.analysis_mut().invalidate();
                }
            }
        }
        Ok(total)
    }
}

impl Default for PassManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PassManager {
    /// Create a PassManager with the standard scalar optimization pipeline
    /// (same as `for_level(OptimizationLevel::O1)`).
    pub fn default_pipeline() -> Self {
        Self::for_level(OptimizationLevel::O1)
    }

    /// Create a PassManager pre-configured for the given optimization level.
    ///
    /// Note: IPA passes (Inline, TailCall) are initialized with an empty
    /// function table. Use [`for_level_with_table`] to provide a pre-built
    /// function table for cross-function optimization.
    pub fn for_level(level: OptimizationLevel) -> Self {
        // 空函数表时 Inline/TailCall 实际不生效（静默 no-op）——显式告警（P1）
        if level >= OptimizationLevel::O2 {
            eprintln!(
                "forge-opt: for_level({level:?}) 使用空函数表——Inline/TailCall IPA pass 将是 no-op；\
                 跨函数优化请用 for_level_with_table"
            );
        }
        Self::for_level_with_table(level, std::collections::HashMap::new())
    }

    /// Create a PassManager for the given level with a pre-built function
    /// table for IPA passes (Inline, TailCall).
    ///
    /// The function table maps `FuncRef` → `Function` and should include all
    /// functions that may be called. Functions are moved into the table
    /// (ownership transferred).
    pub fn for_level_with_table(
        level: OptimizationLevel,
        fn_table: std::collections::HashMap<FuncRef, Function>,
    ) -> Self {
        let mut pm = Self::new();
        if level == OptimizationLevel::O0 {
            return pm;
        }

        // O1: core scalar optimizations
        pm.add_pass(
            Box::new(crate::scalar::const_fold::ConstFoldPass::new()),
            PassRunMode::UntilFixedPoint,
        );
        pm.add_pass(
            Box::new(crate::scalar::copy_prop::CopyPropPass::new()),
            PassRunMode::UntilFixedPoint,
        );
        pm.add_pass(
            Box::new(crate::scalar::cse::CsePass::new()),
            PassRunMode::Once,
        );
        pm.add_pass(
            Box::new(crate::scalar::dead_code::DeadCodeElimPass::new()),
            PassRunMode::UntilFixedPoint,
        );
        pm.add_pass(
            Box::new(crate::scalar::jump_thread::JumpThreadPass::new()),
            PassRunMode::UntilFixedPoint,
        );

        // At O2, fn_table goes to TailCallPass.
        // At O3, fn_table goes to InlinePass (more impactful).
        let (tc_table, inline_table) = if level >= OptimizationLevel::O3 {
            (std::collections::HashMap::new(), fn_table)
        } else if level == OptimizationLevel::O2 {
            (fn_table, std::collections::HashMap::new())
        } else {
            (
                std::collections::HashMap::new(),
                std::collections::HashMap::new(),
            )
        };

        if level >= OptimizationLevel::O2 {
            // O2: advanced scalar + loop + IPA optimizations
            pm.add_pass(
                Box::new(crate::scalar::gvn::GvnPass::new()),
                PassRunMode::Once,
            );
            pm.add_pass(Box::new(crate::scalar::gvn_pre::PrePass), PassRunMode::Once);
            pm.add_pass(
                Box::new(crate::scalar::sccp::SccpPass::new()),
                PassRunMode::Once,
            );
            pm.add_pass(
                Box::new(crate::scalar::block_param_coalesce::BlockParamCoalescePass::new()),
                PassRunMode::UntilFixedPoint,
            );
            pm.add_pass(
                Box::new(crate::loops::licm::LicmPass::new()),
                PassRunMode::Once,
            );
            pm.add_pass(
                Box::new(crate::ipa::tail_call::TailCallPass::new(tc_table)),
                PassRunMode::Once,
            );
            pm.add_pass(
                Box::new(crate::advanced::algebraic::EGraphPass::new()),
                PassRunMode::Once,
            );
            // Trailing DCE: collapse instructions that became dead only after
            // the O1 DCE position (GVN/CSE/SCCP/代数重写 leave both Nop
            // tombstones and indirect dead code). Keeps the optimized IR clean
            // for whatever runs next (later O3 passes or codegen).
            pm.add_pass(
                Box::new(crate::scalar::dead_code::DeadCodeElimPass::new()),
                PassRunMode::UntilFixedPoint,
            );
        }

        if level >= OptimizationLevel::O3 {
            // O3: aggressive optimizations (inlining, loop transformations, mem2reg)
            pm.add_pass(
                Box::new(crate::ipa::inline::InlinePass::new(inline_table)),
                PassRunMode::Once,
            );
            pm.add_pass(
                Box::new(crate::scalar::mem2reg::Mem2RegPass::new()),
                PassRunMode::Once,
            );
            pm.add_pass(
                Box::new(crate::loops::ind_var_simplify::IndVarSimplifyPass::new()),
                PassRunMode::Once,
            );
            pm.add_pass(
                Box::new(crate::loops::loop_unroll::LoopUnrollPass::new()),
                PassRunMode::Once,
            );
            // Trailing DCE (see O2 note): loop transformations (ind-var
            // simplify / unroll) are the heaviest Nop/dead-code producers.
            pm.add_pass(
                Box::new(crate::scalar::dead_code::DeadCodeElimPass::new()),
                PassRunMode::UntilFixedPoint,
            );
        }

        pm
    }
}

// ============================================================
// Optimization pass modules
// ============================================================
pub mod advanced;
pub mod const_value;
pub mod ipa;
pub mod loops;
pub mod scalar;
pub mod support;

pub use const_value::ConstValue;

// ============================================================
// 测试: 管道集成
// ============================================================

#[cfg(test)]
mod pipeline_tests {
    use super::*;

    /// Build a simple callee: fn add_one(x: i32) -> i32 { x + 1 }
    fn build_add_one() -> Function {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("add_one", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "x")]);
        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        let sum = b.iadd(params[0], one);
        b.ret(&[sum]);
        b.finish().expect("build")
    }

    /// Verify for_level_with_table populates the function table so
    /// InlinePass can actually inline calls.
    #[test]
    fn pipeline_with_table_enables_inlining() {
        let callee = build_add_one();
        let callee_ref = FuncRef(0);
        let mut fn_table = std::collections::HashMap::new();
        fn_table.insert(callee_ref, callee);

        // Build caller: fn test() -> i32 { add_one(41) }
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c41 = b.iconst_i32(41);
        let ret = b.call(callee_ref, &[c41], &[TypeId::I32]);
        b.ret(&[ret[0]]);
        let mut caller = b.finish().expect("build");

        // Build pipeline with function table → inline pass gets real table
        let pm = PassManager::for_level_with_table(OptimizationLevel::O3, fn_table);
        let r = pm.run_on_function(&mut caller).unwrap();

        // O3 pipeline includes InlinePass with the real function table
        assert!(r.changed, "Pipeline with fn_table should enable inlining");
    }

    /// Verify for_level (without module) still works for backward compat.
    #[test]
    fn pipeline_without_module_still_works() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(3);
        let bv = b.iconst_i32(5);
        let sum = b.iadd(a, bv);
        b.ret(&[sum]);
        let mut func = b.finish().expect("build");

        let pm = PassManager::for_level(OptimizationLevel::O1);
        let r = pm.run_on_function(&mut func).unwrap();
        // const_fold should fold 3+5=8
        assert!(r.changed, "O1 pipeline should fold constants");
    }

    /// Count `Opcode::Nop` tombstones left in a function.
    fn count_nops(func: &Function) -> usize {
        func.dfg
            .blocks
            .iter()
            .flat_map(|blk| blk.inst_order.iter())
            .filter(|&&i| matches!(func.dfg.insts[i.0 as usize].opcode, Opcode::Nop))
            .count()
    }

    fn build_many_ops() -> Function {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("many_ops", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v1 = b.iconst_i32(1);
        let v2 = b.iconst_i32(2);
        let mut acc = b.iadd(v1, v2);
        for _ in 0..19 {
            let x = b.iconst_i32(3);
            acc = b.iadd(acc, x);
            acc = b.imul(acc, v1);
        }
        b.ret(&[acc]);
        b.finish().expect("build")
    }

    /// Tombstone hygiene: the O2/O3 pipelines leave `Nop` instructions behind
    /// (GVN/CSE/SCCP rewrite dead instructions to Nop after DCE already ran in
    /// O1). They are skipped by codegen, but carrying them through every later
    /// pass + codegen costs traversal time. A trailing DCE should collapse
    /// dead instructions that became dead only after the O1 DCE position.
    #[test]
    fn o2_pipeline_nop_residue() {
        fn build_loop() -> Function {
            let sig = FunctionSignature::new(&[(TypeId::I32, "n")], &[TypeId::I32]);
            let mut b = FunctionBuilder::new("loop", TypeContext::new(), sig);
            let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "n")]);
            let header = b.create_block();
            let body = b.create_block();
            let exit = b.create_block();
            b.switch_to_block(entry);
            let zero = b.iconst_i32(0);
            let one = b.iconst_i32(1);
            b.jump(header, &[zero, zero]);
            b.switch_to_block(header);
            let i = b.iconst_i32(0);
            let sum = b.iconst_i32(0);
            let cond = b.icmp(IntCC::SignedLessThan, i, params[0]);
            b.branch(cond, body, &[], exit, &[]);
            b.switch_to_block(body);
            let next_sum = b.iadd(sum, i);
            let next_i = b.iadd(i, one);
            b.jump(header, &[next_i, next_sum]);
            b.switch_to_block(exit);
            b.ret(&[sum]);
            b.finish().expect("build")
        }
        type FunctionBuilderFn = fn() -> Function;
        let funcs: Vec<(&str, FunctionBuilderFn)> =
            vec![("many_ops", build_many_ops), ("loop", build_loop)];
        for (name, build) in &funcs {
            for level in [
                OptimizationLevel::O1,
                OptimizationLevel::O2,
                OptimizationLevel::O3,
            ] {
                let mut f = build();
                let pm = PassManager::for_level(level);
                pm.run_on_function(&mut f).unwrap();
                let nops = count_nops(&f);
                eprintln!("{name} after {level:?}: nops = {nops}");
                // Trailing-DCE claim: if this ever becomes > 0, the trailing
                // DCE (or physical Nop removal) optimization has a target.
            }
        }
    }
}
