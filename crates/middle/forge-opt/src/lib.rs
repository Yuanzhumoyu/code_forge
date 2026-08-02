//! forge-opt: IR optimization framework.
//!
//! # Pipeline (PassManager::for_level)
//! - O1 (5 passes): const_fold, copy_prop, cse, dead_code, jump_thread
//! - O2 (12 passes): O1 + gvn, gvn_pre, sccp, block_param_coalesce, licm, tail_call, egraph
//! - O3 (16 passes): O2 + inline, mem2reg, ind_var_simplify, loop_unroll
//!
//! # Pass modules
//! - scalar/ (10): block_param_coalesce, const_fold, copy_prop, cse, dead_code, gvn, gvn_pre, jump_thread, mem2reg, sccp
//! - loops/ (3): ind_var_simplify, licm, loop_unroll
//! - ipa/ (4): func_specialize, inline, lto, tail_call
//! - advanced/ (2): egraph, pgo

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
    /// O1 + GVN, GVN-PRE, SCCP, LICM, tail-call, egraph, block-param coalescing.
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

    fn run_on_function(&self, _func: &mut Function) -> Result<PassResult, CompileError> {
        Ok(PassResult::default())
    }

    fn run_on_module(&self, _module: &mut Module) -> Result<PassResult, CompileError> {
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

    pub fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
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
            }
        }
        Ok(total)
    }

    pub fn run_on_module(&self, module: &mut Module) -> Result<PassResult, CompileError> {
        let mut total = PassResult::default();
        for (pass, _mode) in &self.passes {
            if !pass.is_function_pass() {
                let r = pass.run_on_module(module)?;
                total.changed |= r.changed;
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
                Box::new(crate::advanced::egraph::EGraphPass::new()),
                PassRunMode::Once,
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
        }

        pm
    }
}

// ============================================================
// ModulePass trait — cross-function optimization passes
// ============================================================

/// A pass that operates on a collection of functions (module-level).
///
/// Unlike [`OptimizationPass`], which processes one function at a time,
/// `ModulePass` receives all functions at once, enabling inter-procedural
/// optimizations like inlining and LTO.
pub trait ModulePass: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str {
        ""
    }
    fn run_on_module(&self, functions: &mut [Function]) -> Result<PassResult, CompileError>;
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
pub mod verify_ir;

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
        b.finish()
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
        let mut caller = b.finish();

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
        let mut func = b.finish();

        let pm = PassManager::for_level(OptimizationLevel::O1);
        let r = pm.run_on_function(&mut func).unwrap();
        // const_fold should fold 3+5=8
        assert!(r.changed, "O1 pipeline should fold constants");
    }
}
