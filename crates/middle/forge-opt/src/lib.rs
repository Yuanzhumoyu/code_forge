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

/// 不动点迭代轮数上限（防线：pass 的 `changed` 若恒真会让流水线挂死）。
pub const MAX_FIXED_POINT_ROUNDS: usize = 256;

/// pass 之后的 IR 校验策略（仅 debug 构建生效）。
///
/// 历史行为等价于 `Warn`：校验失败只 `log::warn`，坏 IR 继续流向下一个 pass。
/// **现在默认 `Error`**：pass 破坏不变量当场返回 `IrError::Internal`
/// （S6 门禁形态，2026-09-14 起生效）。
///
/// S6 修复记录（打开严格校验后逐条暴露并修掉，见
/// `docs/plans/forge-ir-v3-plan.md` §6）：
/// - 多个 pass 直接 `dfg.make_inst` 建指令而不登记 use-lists
///   （`gvn_pre`/`pgo`/`inline`/`lto`/`func_specialize` 与 forge-codegen 的聚合展开）
///   → 统一改用 `Function::{make_inst, make_inst_with_meta_and_loc}`；
/// - `gvn_pre` 往"操作数尚未定义"的前驱块插入表达式（`DominanceViolation`）
///   → 插入前检查操作数定义块是否支配该前驱；
/// - `tail_call` 把**跨函数**尾调用改写成"跳到被调方入口块编号"
///   （`BlockParamCountMismatch`：块编号空间不同）→ 只处理自递归尾调用，并用
///   `kill_inst` 做原子删除；
/// - `inline` 只用 `replace_all_uses` 替换 call 结果（漏终结符用值），随后
///   `kill_inst` 把该值 VOID 化 → 调用方 `ret` 返回 VOID（真实错码）
///   → 改用 `apply_replacements`（指令操作数 + 终结符全覆盖）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PassVerify {
    /// 不校验（release 构建下的等价行为）。
    Off,
    /// 校验失败只记录 warn（排查用；历史上曾是默认值）。
    Warn,
    /// 校验失败即返回 `IrError::Internal`（**当前默认**，S6 门禁）。
    #[default]
    Error,
}

/// 累加单轮 `PassResult` 到总计。
///
/// 历史实现只做 `total.changed |= r.changed`，`instructions_removed` 等计数
/// **全部丢失**（调用方拿到的统计恒为 0）。
fn accumulate(total: &mut PassResult, r: &PassResult) {
    total.changed |= r.changed;
    total.instructions_removed += r.instructions_removed;
    total.instructions_added += r.instructions_added;
    total.blocks_removed += r.blocks_removed;
    total.values_replaced += r.values_replaced;
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
    verify_after_pass: PassVerify,
}

impl PassManager {
    pub fn new() -> Self {
        Self {
            passes: Vec::new(),
            verify_after_pass: PassVerify::default(),
        }
    }

    /// 设置 pass 之后的 IR 校验策略（debug 构建生效；见 [`PassVerify`]）。
    pub fn set_verify_after_pass(&mut self, policy: PassVerify) -> &mut Self {
        self.verify_after_pass = policy;
        self
    }

    /// 当前策略。
    pub fn verify_after_pass(&self) -> PassVerify {
        self.verify_after_pass
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
                        accumulate(&mut total, &r);
                    }
                    PassRunMode::UntilFixedPoint => {
                        // 不动点迭代必须有上限：历史实现是 `loop { ... }`，
                        // 任何"每轮都报 changed"的 pass 会让整条流水线挂死。
                        let mut rounds = 0usize;
                        loop {
                            let r = pass.run_on_function(func)?;
                            let changed = r.changed;
                            accumulate(&mut total, &r);
                            rounds += 1;
                            if !changed {
                                break;
                            }
                            if rounds >= MAX_FIXED_POINT_ROUNDS {
                                return Err(IrError::Internal(format!(
                                    "pass '{}' 在 '{}' 上 {rounds} 轮未达不动点（上限 {MAX_FIXED_POINT_ROUNDS}）\
                                     ——pass 的 changed 判定可能有误（每轮都报变化）",
                                    pass.name(),
                                    func.name
                                )));
                            }
                        }
                    }
                    PassRunMode::Iterate(n) => {
                        for _ in 0..*n {
                            let r = pass.run_on_function(func)?;
                            accumulate(&mut total, &r);
                        }
                    }
                }

                // Debug 构建下逐 pass 校验 IR 一致性。策略见 `PassVerify`。
                #[cfg(debug_assertions)]
                {
                    if self.verify_after_pass != PassVerify::Off {
                        let mut v = forge_ir::verify::Verifier::with_ctx(func.types.clone());
                        if let Err(errors) = v.verify(func) {
                            let msg = errors
                                .iter()
                                .map(|e| format!("{e:?}"))
                                .collect::<Vec<_>>()
                                .join("; ");
                            match self.verify_after_pass {
                                PassVerify::Error => {
                                    return Err(IrError::Internal(format!(
                                        "pass '{}' 之后 '{}' 未通过 IR 校验：{msg}",
                                        pass.name(),
                                        func.name
                                    )));
                                }
                                _ => log::warn!(
                                    "IR 校验欠账 after '{}' on '{}': {}",
                                    pass.name(),
                                    func.name,
                                    msg
                                ),
                            }
                        }
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
                accumulate(&mut total, &r);
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
        // 别名驱动的冗余 store 消除（P1-5 别名分析的消费方）：
        // 同位置连续 store 且中间无读 → 删除前者
        pm.add_pass(
            Box::new(crate::scalar::dead_store::DeadStoreElimPass::new()),
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
            // P1 剩余项：规范 preheader——多循环外 pred 的循环插入独立
            // preheader 块，LICM 外提目标不再回退 header。
            pm.add_pass(
                Box::new(crate::loops::insert_preheader::InsertPreheaderPass::new()),
                PassRunMode::Once,
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
            .filter(|&&i| matches!(func.dfg.inst_data(i).opcode, Opcode::Nop))
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
            // header 是循环头：携带 (i, sum) 两个块参数——jump/branch 的实参数
            // 必须与块参数一一对应（此前的夹具建了 0 参数块却传 2 个实参，
            // 属于**非法 IR**，被当时"只 warn 不失败"的 pass 后校验掩盖）。
            let (header, hparams) =
                b.create_block_with_params(&[(TypeId::I32, "i"), (TypeId::I32, "sum")]);
            let body = b.create_block();
            let exit = b.create_block();
            b.switch_to_block(entry);
            let zero = b.iconst_i32(0);
            let one = b.iconst_i32(1);
            b.jump(header, &[zero, zero]);
            b.switch_to_block(header);
            let i = hparams[0];
            let sum = hparams[1];
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

    /// **欠账钉住**：`PassVerify::Error`（S6 的目标门禁）在当前 pass 集上**必然失败**。
    ///
    /// 2026-09-14 打开严格校验后实测：`inline` / `gvn_pre` / `mem2reg` 等 pass 会留下
    /// use-list 不一致或返回类型不匹配的 IR（详情见 `PassVerify` 文档）。本测试把
    /// 这个事实固定下来，避免它再次被"只 warn"掩盖：
    ///
    /// **S6 修好这些 pass 之后，本测试会开始失败** —— 届时把断言改为
    /// `assert!(result.is_ok())` 并把 pass 后校验默认值切到 `PassVerify::Error`。
    /// **S6 门禁：严格校验下所有流水线必须保持 IR 不变量**。
    ///
    /// 2026-09-14 打开"pass 后严格校验"时首次暴露两笔欠账：
    /// ① 多个 pass 直接 `dfg.make_inst` 建指令而**不登记 use-lists**
    ///    （`gvn_pre`/`pgo`/`inline`/`lto`/`func_specialize`，以及 forge-codegen 的
    ///    聚合展开路径）；② `gvn_pre` 会往"操作数尚未定义"的前驱块插入表达式
    ///    （`DominanceViolation`）。
    /// 两处都已修（改用 `Function::make_inst` 系列 + PRE 的先决条件检查），
    /// 本测试把"严格模式全绿"固定下来：任何 pass 再制造不变量破坏都会在此变成
    /// 失败用例，而不是继续被 `Warn` 静默吞掉。
    #[test]
    fn strict_verification_passes_for_all_pipelines() {
        fn build_loop() -> Function {
            let sig = FunctionSignature::new(&[(TypeId::I32, "n")], &[TypeId::I32]);
            let mut b = FunctionBuilder::new("debt", TypeContext::new(), sig);
            let (entry, params) = b.create_block_with_params(&[(TypeId::I32, "n")]);
            let (header, hparams) =
                b.create_block_with_params(&[(TypeId::I32, "i"), (TypeId::I32, "sum")]);
            let body = b.create_block();
            let exit = b.create_block();
            b.switch_to_block(entry);
            let zero = b.iconst_i32(0);
            let one = b.iconst_i32(1);
            b.jump(header, &[zero, zero]);
            b.switch_to_block(header);
            let i = hparams[0];
            let sum = hparams[1];
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

        fn build_call_with_table() -> (Function, std::collections::HashMap<FuncRef, Function>) {
            let callee = build_add_one();
            let callee_ref = FuncRef(0);
            let mut table = std::collections::HashMap::new();
            table.insert(callee_ref, callee);

            let sig = FunctionSignature::new(&[], &[TypeId::I32]);
            let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
            let entry = b.create_block();
            b.switch_to_block(entry);
            let c41 = b.iconst_i32(41);
            let call_ret = b.call(callee_ref, &[c41], &[TypeId::I32]);
            b.ret(&[call_ret[0]]);
            (b.finish().expect("build"), table)
        }

        // 三种形状 × 三个优化级别，全部在严格校验下跑通
        for level in [
            OptimizationLevel::O1,
            OptimizationLevel::O2,
            OptimizationLevel::O3,
        ] {
            for build in [
                build_many_ops as fn() -> Function,
                build_loop as fn() -> Function,
            ] {
                let mut f = build();
                let mut pm = PassManager::for_level(level);
                pm.set_verify_after_pass(PassVerify::Error);
                pm.run_on_function(&mut f)
                    .unwrap_or_else(|e| panic!("{level:?} 严格校验失败：{e:?}"));
            }

            // 带函数表（启用 inline）的路径
            let (mut caller, table) = build_call_with_table();
            let mut pm = PassManager::for_level_with_table(level, table);
            pm.set_verify_after_pass(PassVerify::Error);
            pm.run_on_function(&mut caller)
                .unwrap_or_else(|e| panic!("{level:?} + inline 严格校验失败：{e:?}"));
        }
    }
}
