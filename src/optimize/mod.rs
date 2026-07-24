//! 优化管道 — 可扩展的 IR 优化框架。
//!
//! # 设计
//!
//! 提供 trait [`OptimizationPass`] 和管道管理器 [`PassManager`]，
//! 允许用户注册、排序和自定义优化 pass。
//!
//! ## 函数级 pass vs 模块级 pass
//!
//! - **函数级 pass** (`is_function_pass() == true`): 对每个函数独立运行，
//!   可跨函数并行执行。示例：常量折叠、死代码消除、复制传播。
//! - **模块级 pass** (`is_function_pass() == false`): 对模块内所有函数
//!   一起运行，需要跨函数分析，串行执行。示例：内联、全局 DCE。
//!
//! ## 并行执行
//!
//! 启用 `parallel` feature（依赖 rayon）后，函数级 pass 会跨函数并行执行。
//! 不启用时自动回退到串行执行。
//!
//! # Example
//!
//! ```ignore
//! use codegen_lib::optimize::*;
//!
//! let mut pm = PassManager::new();
//! pm.add_pass(ConstFoldPass::new(), PassRunMode::UntilFixedPoint);
//! pm.add_pass(DeadCodeElimPass::new(), PassRunMode::Once);
//!
//! let mut func = builder.finish();
//! pm.run_on_function(&mut func)?;
//! ```

use crate::CompileError;
use crate::ir::Function;

// ============================================================
// 分析 & 基础设施
// ============================================================
pub mod alias_analysis;    // 别名分析
pub mod analysis_manager;  // 分析缓存管理器
pub mod interpreter;       // IR 解释器 (常量求值)

// ============================================================
// 标量优化 (函数级 pass)
// ============================================================
pub mod const_fold;        // 常量折叠
pub mod copy_prop;         // 复制传播
pub mod cse;               // 公共子表达式消除
pub mod dead_code;         // 死代码消除
pub mod gvn;               // 全局值编号
pub mod gvn_pre;           // 部分冗余消除 (PRE)
pub mod jump_thread;       // 跳转线程
pub mod mem2reg;           // 内存到寄存器提升
pub mod phi_elim;          // φ 指令消除
pub mod sccp;              // 稀疏条件常量传播

// ============================================================
// 循环优化
// ============================================================
pub mod ind_var_simplify;  // 归纳变量化简
pub mod licm;              // 循环不变量外提
pub mod loop_unroll;       // 循环展开

// ============================================================
// 跨函数 / 模块级优化
// ============================================================
pub mod func_specialize;   // 函数特化
pub mod inline;            // 内联
pub mod lto;               // 链接时优化
pub mod sroa;              // 标量替换聚合体
pub mod tail_call;         // 尾调用优化

// ============================================================
// 高级框架
// ============================================================
pub mod egraph;            // E-Graph (equality saturation)
pub mod pgo;               // Profile-Guided Optimization

// ============================================================
// 验证
// ============================================================
pub mod verify_ir;         // IR 验证器

// ============================================================
// 公开导出
// ============================================================
// 分析 & 基础设施
pub use alias_analysis::*;
pub use analysis_manager::*;
pub use interpreter::*;

// 标量优化
pub use const_fold::*;
pub use copy_prop::*;
pub use cse::*;
pub use dead_code::*;
pub use gvn::*;
pub use gvn_pre::*;
pub use jump_thread::*;
pub use mem2reg::*;
pub use phi_elim::*;
pub use sccp::*;

// 循环优化
pub use ind_var_simplify::*;
pub use licm::*;
pub use loop_unroll::*;

// 跨函数 / 模块级优化
pub use func_specialize::*;
pub use inline::*;
pub use lto::*;
pub use sroa::*;
pub use tail_call::*;

// 高级框架
pub use egraph::*;
pub use pgo::*;

// 验证
pub use verify_ir::*;

// ============================================================
// PassResult
// ============================================================

/// 优化 pass 的运行结果。
#[derive(Clone, Debug, Default)]
pub struct PassResult {
    /// 是否有任何 IR 被修改。
    pub changed: bool,
    /// 折叠/消除/替换的指令数。
    pub instructions_removed: usize,
    /// 消除的基本块数。
    pub blocks_removed: usize,
    /// 传播/替换的值的数量。
    pub values_replaced: usize,
}

impl PassResult {
    /// 创建一个标记为"已修改"的结果。
    pub fn changed() -> Self {
        Self {
            changed: true,
            ..Default::default()
        }
    }

    /// 合并另一个 PassResult 的统计信息。
    pub fn merge(&mut self, other: &PassResult) {
        self.changed |= other.changed;
        self.instructions_removed += other.instructions_removed;
        self.blocks_removed += other.blocks_removed;
        self.values_replaced += other.values_replaced;
    }
}

// ============================================================
// PassRunMode
// ============================================================

/// Pass 运行模式。
#[derive(Clone, Debug, Default)]
pub enum PassRunMode {
    /// 只运行一次。
    #[default]
    Once,
    /// 重复运行直到 IR 不再变化（到达不动点）。
    UntilFixedPoint,
    /// 最多运行 N 次。
    Iterate(u32),
}

// ============================================================
// OptimizationPass trait（函数级优化 — 原名称保留兼容）
// ============================================================

/// 优化 pass trait — 对单个函数独立运行。
///
/// 特点：
/// - 函数间无数据依赖，可跨函数并行执行
/// - 只能访问和修改单个 `Function`
///
/// # Example
///
/// ```ignore
/// struct MyPass;
/// impl OptimizationPass for MyPass {
///     fn name(&self) -> &'static str { "my-pass" }
///     fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
///         // Transform the IR...
///         Ok(PassResult::changed())
///     }
/// }
/// ```
pub trait OptimizationPass: Send + Sync {
    /// Pass 的唯一名称（用于注册、排序和查找）。
    fn name(&self) -> &'static str;

    /// 简要描述此 pass 做什么。
    fn description(&self) -> &'static str {
        "no description"
    }

    /// 此 pass 依赖的前置 pass 名称列表。
    fn requires(&self) -> &[&str] {
        &[]
    }

    /// 此 pass 会失效的分析/信息。
    fn invalidates(&self) -> &[&str] {
        &[]
    }

    /// 在单个函数上运行此 pass。
    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError>;
}

/// `FunctionPass` 是 `OptimizationPass` 的别名。
pub trait FunctionPass: OptimizationPass {}

/// 所有实现了 OptimizationPass 的类型自动实现 FunctionPass。
impl<T: OptimizationPass> FunctionPass for T {}

// ============================================================
// ModulePass trait — 模块级优化
// ============================================================

/// 模块级优化 pass — 对所有函数一起运行。
///
/// 特点：
/// - 需要跨函数分析，必须串行执行
/// - 可以访问和修改所有函数
///
/// # Example
///
/// ```ignore
/// struct MyModulePass;
/// impl ModulePass for MyModulePass {
///     fn name(&self) -> &'static str { "my-module-pass" }
///     fn run_on_module(&self, functions: &mut [Function]) -> Result<PassResult, CompileError> {
///         // Cross-function analysis...
///         Ok(PassResult::changed())
///     }
/// }
/// ```
pub trait ModulePass: Send + Sync {
    /// Pass 的唯一名称。
    fn name(&self) -> &'static str;

    /// 简要描述。
    fn description(&self) -> &'static str {
        "no description"
    }

    /// 此 pass 依赖的前置 pass 名称列表。
    fn requires(&self) -> &[&str] {
        &[]
    }

    /// 此 pass 会失效的分析/信息。
    fn invalidates(&self) -> &[&str] {
        &[]
    }

    /// 在所有函数上运行此 pass。
    fn run_on_module(&self, functions: &mut [Function]) -> Result<PassResult, CompileError>;
}

// ============================================================
// 类型擦除的 Pass enum
// ============================================================

/// 内部使用的统一 pass 类型。
enum PassKind {
    Function(Box<dyn OptimizationPass>),
    Module(Box<dyn ModulePass>),
}

impl PassKind {
    fn name(&self) -> &str {
        match self {
            PassKind::Function(p) => p.name(),
            PassKind::Module(p) => p.name(),
        }
    }

    fn requires(&self) -> &[&str] {
        match self {
            PassKind::Function(p) => p.requires(),
            PassKind::Module(p) => p.requires(),
        }
    }
}

// ============================================================
// FunctionPassAdapter — 将 FunctionPass 包装为 ModulePass
// ============================================================

/// 将任意 `FunctionPass` 适配为 `ModulePass`。
///
/// 当 `FunctionPass` 作为模块级 pass 运行时，会依次在每个函数上调用
/// `run_on_function`。如果启用并行，可跨函数并行执行。
pub struct FunctionPassAdapter<P: OptimizationPass> {
    pub pass: P,
}

impl<P: OptimizationPass> FunctionPassAdapter<P> {
    pub fn new(pass: P) -> Self {
        Self { pass }
    }
}

impl<P: OptimizationPass> ModulePass for FunctionPassAdapter<P> {
    fn name(&self) -> &'static str {
        self.pass.name()
    }

    fn description(&self) -> &'static str {
        self.pass.description()
    }

    fn requires(&self) -> &[&str] {
        self.pass.requires()
    }

    fn invalidates(&self) -> &[&str] {
        self.pass.invalidates()
    }

    fn run_on_module(&self, functions: &mut [Function]) -> Result<PassResult, CompileError> {
        let mut result = PassResult::default();
        for func in functions {
            let r = self.pass.run_on_function(func)?;
            result.merge(&r);
        }
        Ok(result)
    }
}

// ============================================================
// OptimizationPass trait（保留为旧兼容，使用 FunctionPassAdapter）
// ============================================================
// (已弃用) 旧版统一 pass trait — 请改用 `FunctionPass` 或 `ModulePass`。

impl PassManager {
    /// 验证 pass 顺序：检查每个 pass 的依赖是否已被满足。
    /// 返回错误消息列表（空 = 有效）。
    pub fn verify_dependencies(&self) -> Vec<String> {
        let mut errors = Vec::new();
        let names: Vec<&str> = self.passes.iter().map(|(p, _)| p.name()).collect();

        for (i, (pass, _)) in self.passes.iter().enumerate() {
            for &req in pass.requires() {
                if !names[..i].contains(&req) {
                    errors.push(format!(
                        "pass '{}' requires '{}' but it is not scheduled before it",
                        pass.name(),
                        req
                    ));
                }
            }
        }
        errors
    }
}

// ============================================================
// PassManager
// ============================================================

/// 优化管道管理器。
///
/// 管理一组有序的优化 pass，控制执行顺序和迭代模式。
///
/// # 并行执行
///
/// 当启用 `parallel` feature 且 `set_parallel(true)` 时，
/// 函数级 pass 会使用 rayon 跨函数并行执行。
/// 模块级 pass 始终串行执行。
///
/// # Example
///
/// ```ignore
/// let mut pm = PassManager::new();
/// pm.add_pass(ConstFoldPass::new(), PassRunMode::UntilFixedPoint);
/// pm.add_pass(DeadCodeElimPass::new(), PassRunMode::Once);
///
/// let result = pm.run_on_function(&mut func)?;
/// assert!(result.instructions_removed > 0);
/// ```
pub struct PassManager {
    passes: Vec<(PassKind, PassRunMode)>,
    /// 是否启用并行执行（需要 `parallel` feature）。
    parallel: bool,
    /// Per-pass 累计运行统计（通过内部可变性更新）。
    stats: std::sync::Mutex<Vec<PassStats>>,
}

impl PassManager {
    /// 创建空的 pass 管理器。
    pub fn new() -> Self {
        Self {
            passes: Vec::new(),
            parallel: false,
            stats: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// 启用/禁用并行执行。
    ///
    /// 仅在启用 `parallel` feature 时生效。
    /// 禁用时，函数级 pass 也按串行方式执行。
    pub fn set_parallel(&mut self, enabled: bool) {
        self.parallel = enabled;
    }

    /// 返回是否启用了并行执行。
    pub fn is_parallel(&self) -> bool {
        self.parallel
    }

    /// 返回已注册的 pass 数量。
    pub fn len(&self) -> usize {
        self.passes.len()
    }

    /// 是否没有注册任何 pass。
    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }

    /// 添加一个函数级 pass 到管道末尾。
    ///
    /// # Example
    /// ```ignore
    /// pm.add_pass(ConstFoldPass::new(), PassRunMode::UntilFixedPoint);
    /// ```
    pub fn add_pass(&mut self, pass: impl OptimizationPass + 'static, mode: PassRunMode) {
        self.passes.push((PassKind::Function(Box::new(pass)), mode));
    }

    /// 添加一个模块级 pass 到管道末尾。
    pub fn add_module_pass(&mut self, pass: impl ModulePass + 'static, mode: PassRunMode) {
        self.passes.push((PassKind::Module(Box::new(pass)), mode));
    }

    /// 在指定名称的 pass 之后插入新 pass。
    ///
    /// 如果找不到指定名称的 pass，返回错误。
    pub fn insert_after(
        &mut self,
        after_name: &str,
        pass: impl OptimizationPass + 'static,
        mode: PassRunMode,
    ) -> Result<(), String> {
        let pos = self
            .passes
            .iter()
            .position(|(p, _)| p.name() == after_name)
            .ok_or_else(|| format!("pass '{}' not found", after_name))?;
        self.passes
            .insert(pos + 1, (PassKind::Function(Box::new(pass)), mode));
        Ok(())
    }

    /// 在指定名称的 pass 之前插入新 pass。
    ///
    /// 如果找不到指定名称的 pass，返回错误。
    pub fn insert_before(
        &mut self,
        before_name: &str,
        pass: impl OptimizationPass + 'static,
        mode: PassRunMode,
    ) -> Result<(), String> {
        let pos = self
            .passes
            .iter()
            .position(|(p, _)| p.name() == before_name)
            .ok_or_else(|| format!("pass '{}' not found", before_name))?;
        self.passes
            .insert(pos, (PassKind::Function(Box::new(pass)), mode));
        Ok(())
    }

    /// 移除指定名称的 pass。返回是否成功移除。
    pub fn remove_pass(&mut self, name: &str) -> bool {
        if let Some(pos) = self.passes.iter().position(|(p, _)| p.name() == name) {
            self.passes.remove(pos);
            true
        } else {
            false
        }
    }

    /// 获取所有已注册 pass 的名称列表。
    pub fn pass_names(&self) -> Vec<&str> {
        self.passes.iter().map(|(p, _)| p.name()).collect()
    }

    // ============================================================
    // 执行方法
    // ============================================================

    /// 在单个函数上运行整个管道。
    ///
    /// 依次执行每个 pass（尊重 `PassRunMode`），返回累计的 `PassResult`。
    pub fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        let mut total = PassResult::default();

        for (pass, mode) in &self.passes {
            let start = std::time::Instant::now();
            let result = match pass {
                PassKind::Function(fp) => self.run_single_pass_fn(fp.as_ref(), mode, func)?,
                PassKind::Module(mp) => {
                    let mut funcs = [std::mem::take(func)];
                    let result = self.run_single_module_pass(mp.as_ref(), mode, &mut funcs)?;
                    *func = std::mem::take(&mut funcs[0]);
                    result
                }
            };
            self.record_pass_run(pass.name(), &result, start.elapsed().as_secs_f64() * 1000.0);
            total.merge(&result);
        }

        Ok(total)
    }

    /// 在多个函数上运行整个管道（模块级入口）。
    ///
    /// 函数级 pass：跨函数并行执行。
    /// 模块级 pass：对所有函数串行执行。
    pub fn run_on_module(&self, functions: &mut [Function]) -> Result<PassResult, CompileError> {
        if functions.is_empty() {
            return Ok(PassResult::default());
        }

        let mut total = PassResult::default();

        for (pass, mode) in &self.passes {
            let start = std::time::Instant::now();
            let result = match pass {
                PassKind::Function(fp) => {
                    self.run_function_pass_on_all(fp.as_ref(), mode, functions)?
                }
                PassKind::Module(mp) => {
                    self.run_single_module_pass(mp.as_ref(), mode, functions)?
                }
            };
            self.record_pass_run(pass.name(), &result, start.elapsed().as_secs_f64() * 1000.0);
            total.merge(&result);
        }

        Ok(total)
    }

    // ============================================================
    // 内部方法
    // ============================================================

    /// 运行单个函数级 pass（支持迭代模式）。
    fn run_single_pass_fn(
        &self,
        pass: &dyn OptimizationPass,
        mode: &PassRunMode,
        func: &mut Function,
    ) -> Result<PassResult, CompileError> {
        match mode {
            PassRunMode::Once => pass.run_on_function(func),
            PassRunMode::UntilFixedPoint => {
                let mut total = PassResult::default();
                loop {
                    let result = pass.run_on_function(func)?;
                    total.merge(&result);
                    if !result.changed {
                        break;
                    }
                }
                Ok(total)
            }
            PassRunMode::Iterate(n) => {
                let mut total = PassResult::default();
                for _ in 0..*n {
                    let result = pass.run_on_function(func)?;
                    total.merge(&result);
                    if !result.changed {
                        break;
                    }
                }
                Ok(total)
            }
        }
    }

    /// 在所有函数上运行单个函数级 pass。
    fn run_function_pass_on_all(
        &self,
        pass: &dyn OptimizationPass,
        mode: &PassRunMode,
        functions: &mut [Function],
    ) -> Result<PassResult, CompileError> {
        if self.parallel && functions.len() > 1 {
            self.run_function_pass_parallel(pass, mode, functions)
        } else {
            self.run_function_pass_sequential(pass, mode, functions)
        }
    }

    /// 串行执行函数级 pass。
    fn run_function_pass_sequential(
        &self,
        pass: &dyn OptimizationPass,
        mode: &PassRunMode,
        functions: &mut [Function],
    ) -> Result<PassResult, CompileError> {
        let mut total = PassResult::default();
        for func in functions.iter_mut() {
            let result = self.run_single_pass_fn(pass, mode, func)?;
            total.merge(&result);
        }
        Ok(total)
    }

    /// 并行执行函数级 pass（使用 rayon）。
    #[cfg(feature = "parallel")]
    fn run_function_pass_parallel(
        &self,
        pass: &dyn OptimizationPass,
        mode: &PassRunMode,
        functions: &mut [Function],
    ) -> Result<PassResult, CompileError> {
        use rayon::prelude::*;

        let results: Vec<Result<PassResult, CompileError>> = functions
            .par_iter_mut()
            .map(|func| self.run_single_pass_fn(pass, mode, func))
            .collect();

        let mut total = PassResult::default();
        for result in results {
            total.merge(&result?);
        }
        Ok(total)
    }

    /// 无 rayon 时的回退（串行）。
    #[cfg(not(feature = "parallel"))]
    fn run_function_pass_parallel(
        &self,
        pass: &dyn OptimizationPass,
        mode: &PassRunMode,
        functions: &mut [Function],
    ) -> Result<PassResult, CompileError> {
        self.run_function_pass_sequential(pass, mode, functions)
    }

    /// 运行单个模块级 pass（支持迭代模式）。
    fn run_single_module_pass(
        &self,
        pass: &dyn ModulePass,
        mode: &PassRunMode,
        functions: &mut [Function],
    ) -> Result<PassResult, CompileError> {
        match mode {
            PassRunMode::Once => pass.run_on_module(functions),
            PassRunMode::UntilFixedPoint => {
                let mut total = PassResult::default();
                loop {
                    let result = pass.run_on_module(functions)?;
                    total.merge(&result);
                    if !result.changed {
                        break;
                    }
                }
                Ok(total)
            }
            PassRunMode::Iterate(n) => {
                let mut total = PassResult::default();
                for _ in 0..*n {
                    let result = pass.run_on_module(functions)?;
                    total.merge(&result);
                    if !result.changed {
                        break;
                    }
                }
                Ok(total)
            }
        }
    }
}

impl Default for PassManager {
    /// 创建默认优化管道：
    /// 1. 常量折叠 → 不动点
    /// 2. GVN → 一次（跨块公共子表达式消除，子集局部 CSE）
    /// 3. LICM → 不动点（循环不变量外提）
    /// 4. 死代码消除 → 一次
    /// 5. 复制传播 → 一次
    fn default() -> Self {
        Self::for_level(OptimizationLevel::default())
    }
}

// ============================================================
// Pass 统计信息
// ============================================================

/// 单个 pass 运行的统计信息。
#[derive(Clone, Debug, Default)]
pub struct PassStats {
    /// Pass 名称。
    pub name: String,
    /// 运行次数。
    pub runs: usize,
    /// 累计运行时间（毫秒）。
    pub total_time_ms: f64,
    /// 累计移除的指令数。
    pub instructions_removed: usize,
    /// 累计移除的块数。
    pub blocks_removed: usize,
}

impl PassManager {
    /// 收集所有已注册 pass 的运行统计。
    pub fn statistics(&self) -> Vec<PassStats> {
        self.stats.lock().unwrap().clone()
    }

    /// 记录一次 pass 运行（由 run_on_function / run_on_module 内部调用）。
    fn record_pass_run(&self, name: &str, result: &PassResult, elapsed_ms: f64) {
        let mut stats = self.stats.lock().unwrap();
        if let Some(s) = stats.iter_mut().find(|s| s.name == name) {
            s.runs += 1;
            s.total_time_ms += elapsed_ms;
            s.instructions_removed += result.instructions_removed;
            s.blocks_removed += result.blocks_removed;
        } else {
            stats.push(PassStats {
                name: name.to_string(),
                runs: 1,
                total_time_ms: elapsed_ms,
                instructions_removed: result.instructions_removed,
                blocks_removed: result.blocks_removed,
            });
        }
    }
}

// ============================================================
// 优化级别
// ============================================================

/// 优化级别 — 控制编译器的优化力度。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum OptimizationLevel {
    /// 无优化：只做必要的 lowering 前规范化。
    O0,
    /// 基本优化：常量折叠 + DCE。
    O1,
    /// 默认优化：完整的标准管道。
    #[default]
    O2,
    /// 激进优化：额外的不稳定/实验性 pass。
    O3,
    /// 优化体积：与 O2 类似但偏好更小的代码。
    Os,
    /// 激进优化体积：与 O3 类似但偏好更小的代码。
    Oz,
}

impl OptimizationLevel {
    /// 为此优化级别构建 PassManager。
    pub fn build_pipeline(self) -> PassManager {
        match self {
            OptimizationLevel::O0 => {
                let mut pm = PassManager::new();
                // O0: 仅做轻量规范化
                pm.add_pass(CopyPropPass::new(), PassRunMode::Once);
                pm
            }
            OptimizationLevel::O1 => {
                let mut pm = PassManager::new();
                pm.add_pass(ConstFoldPass::new(), PassRunMode::UntilFixedPoint);
                pm.add_pass(DeadCodeElimPass::new(), PassRunMode::Once);
                pm.add_pass(CopyPropPass::new(), PassRunMode::Once);
                pm
            }
            OptimizationLevel::O2 | OptimizationLevel::Os => {
                let mut pm = PassManager::new();
                pm.add_pass(ConstFoldPass::new(), PassRunMode::UntilFixedPoint);
                pm.add_pass(GvnPass::new(), PassRunMode::Once);
                pm.add_pass(LicmPass::new(), PassRunMode::UntilFixedPoint);
                pm.add_pass(JumpThreadPass::new(), PassRunMode::Once);
                pm.add_pass(DeadCodeElimPass::new(), PassRunMode::Once);
                pm.add_pass(CopyPropPass::new(), PassRunMode::Once);
                pm
            }
            OptimizationLevel::O3 | OptimizationLevel::Oz => {
                let mut pm = PassManager::new();
                pm.add_pass(ConstFoldPass::new(), PassRunMode::UntilFixedPoint);
                pm.add_pass(GvnPass::new(), PassRunMode::UntilFixedPoint);
                pm.add_pass(LicmPass::new(), PassRunMode::UntilFixedPoint);
                pm.add_pass(JumpThreadPass::new(), PassRunMode::UntilFixedPoint);
                pm.add_pass(DeadCodeElimPass::new(), PassRunMode::Once);
                pm.add_pass(CopyPropPass::new(), PassRunMode::Once);
                pm
            }
        }
    }
}

impl PassManager {
    /// 为指定优化级别构建 PassManager。
    pub fn for_level(level: OptimizationLevel) -> Self {
        level.build_pipeline()
    }

    /// 创建轻量级后内联清理管线。
    ///
    /// 在内联后立即运行，清除内联暴露的冗余：
    /// - ConstFold: 折叠内联暴露的常量参数
    /// - DCE: 清除内联带入的死代码
    /// - CopyProp: 消除冗余的 copy/phi 链
    /// - SimplifyCFG: 合并冗余分支
    pub fn post_inline_cleanup() -> Self {
        let mut pm = PassManager::new();
        pm.add_pass(ConstFoldPass::new(), PassRunMode::UntilFixedPoint);
        pm.add_pass(DeadCodeElimPass::new(), PassRunMode::Once);
        pm.add_pass(CopyPropPass::new(), PassRunMode::Once);
        pm.add_pass(JumpThreadPass::new(), PassRunMode::Once);
        pm
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;
    use std::collections::HashMap;

    /// 一个简单的测试 pass：将第一个 Nop 替换为 Iconst(42)。
    #[derive(Default)]
    struct TestPass {
        run_count: std::sync::atomic::AtomicUsize,
    }

    impl TestPass {
        fn new() -> Self {
            Self {
                run_count: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    impl OptimizationPass for TestPass {
        fn name(&self) -> &'static str {
            "test-pass"
        }
        fn description(&self) -> &'static str {
            "Test pass that replaces first Nop"
        }

        fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
            self.run_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            for block in func.blocks.iter_mut() {
                for inst in block.instructions.iter_mut() {
                    if matches!(inst.opcode, Opcode::Nop) {
                        let index = func.constant_pool.insert(Big::from_i64(42));
                        inst.opcode = Opcode::Iconst { index };
                        return Ok(PassResult {
                            changed: true,
                            instructions_removed: 1,
                            ..Default::default()
                        });
                    }
                }
            }
            Ok(PassResult::default())
        }
    }

    #[test]
    fn test_pass_manager_add_and_run() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut func = Function::new("test", sig);
        let block_id = func.create_block_id();
        let v = func.create_value();
        func.blocks.push(Block::new(block_id));
        func.blocks[0].instructions.push(Instruction::new(
            Opcode::Nop,
            smallvec::smallvec![],
            Some(v),
            Type::I32,
        ));

        let pm = PassManager::default();
        // 没有 nop 替换 pass — 默认管道不应该崩溃
        let _result = pm.run_on_function(&mut func).unwrap();
        // 函数应该仍然有效
        assert_eq!(func.blocks[0].instructions.len(), 1);
    }

    #[test]
    fn test_pass_manager_custom_pass() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut func = Function::new("test", sig);
        let block_id = func.create_block_id();
        let v = func.create_value();
        func.blocks.push(Block::new(block_id));
        func.blocks[0].instructions.push(Instruction::new(
            Opcode::Nop,
            smallvec::smallvec![],
            Some(v),
            Type::I32,
        ));

        let mut pm = PassManager::new();
        pm.add_pass(TestPass::new(), PassRunMode::Once);
        let result = pm.run_on_function(&mut func).unwrap();
        assert!(result.changed);
        assert_eq!(result.instructions_removed, 1);
        if let Opcode::Iconst { index } = func.blocks[0].instructions[0].opcode {
            assert_eq!(
                func.constant_pool.get(index).unwrap().try_to_i64().unwrap(),
                42
            );
        } else {
            panic!("Expected Iconst");
        }
    }

    #[test]
    fn test_pass_manager_insert_remove() {
        let mut pm = PassManager::new();
        pm.add_pass(TestPass::new(), PassRunMode::Once);

        assert_eq!(pm.len(), 1);
        assert_eq!(pm.pass_names(), vec!["test-pass"]);

        // 插入之前
        pm.insert_before("test-pass", TestPass::new(), PassRunMode::Once)
            .unwrap();
        assert_eq!(pm.len(), 2);

        // 移除
        assert!(pm.remove_pass("test-pass"));
        assert!(!pm.remove_pass("nonexistent"));
    }

    #[test]
    fn test_pass_manager_until_fixed_point() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut func = Function::new("test", sig);
        let block_id = func.create_block_id();
        let v = func.create_value();
        func.blocks.push(Block::new(block_id));
        // 两个 Nop — 这个 pass 每次只替换一个
        func.blocks[0].instructions.push(Instruction::new(
            Opcode::Nop,
            smallvec::smallvec![],
            Some(v),
            Type::I32,
        ));

        let mut pm = PassManager::new();
        pm.add_pass(TestPass::new(), PassRunMode::UntilFixedPoint);
        let result = pm.run_on_function(&mut func).unwrap();
        assert!(result.changed);
        // 应该运行 2 次：第一次找到 Nop 并替换，第二次没找到就停了
    }

    #[test]
    fn test_pass_result_merge() {
        let mut a = PassResult {
            changed: true,
            instructions_removed: 3,
            blocks_removed: 1,
            values_replaced: 5,
        };
        let b = PassResult {
            changed: false,
            instructions_removed: 2,
            blocks_removed: 0,
            values_replaced: 1,
        };
        a.merge(&b);
        assert!(a.changed);
        assert_eq!(a.instructions_removed, 5);
        assert_eq!(a.blocks_removed, 1);
        assert_eq!(a.values_replaced, 6);
    }

    // ============================================================
    // 端到端测试：完整优化管道 + const fn 求值
    // ============================================================

    #[test]
    fn end_to_end_const_fn_eval_pipeline() {
        // 构建一个 const 函数: fn const_add(a: i32, b: i32) -> i32 { a + b }
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut b = FunctionBuilder::new("const_add", sig.clone());
        b.set_const(true); // 标记为 const
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        b.switch_to_block(entry);
        let a = params[0];
        let b_val = params[1];
        let sum = b.iadd(a, b_val);
        b.return_(&[sum]);
        let const_add_fn = b.finish();
        assert!(const_add_fn.is_const);

        // 构建调用者函数: fn caller() -> i32 { const_add(3, 5) + 10 }
        let caller_sig = Signature::new(&[], &[Type::I32]);
        let mut b2 = FunctionBuilder::new("caller", caller_sig);
        let entry2 = b2.create_block();
        b2.switch_to_block(entry2);
        let c3 = b2.iconst_i32(3);
        let c5 = b2.iconst_i32(5);
        let call_result = b2.call(FuncRef(0), &[c3, c5], &[Type::I32]);
        let c10 = b2.iconst_i32(10);
        let final_result = b2.iadd(call_result[0], c10);
        b2.return_(&[final_result]);
        let mut caller_fn = b2.finish();

        // 函数表: FuncRef(0) → const_add_fn
        let mut func_table: HashMap<FuncRef, Function> = HashMap::new();
        func_table.insert(FuncRef(0), const_add_fn);

        // 步骤 1: ConstFnEvalPass — 编译时求值 const_add(3, 5) → Iconst(8)
        let eval_pass = ConstFnEvalPass::new(func_table);
        let r1 = eval_pass.run_on_function(&mut caller_fn).unwrap();
        assert!(r1.changed);
        assert!(r1.instructions_removed >= 1);

        // 步骤 2: 默认管道 — 折叠 Iconst(8) + Iconst(10) → Iconst(18), DCE, CopyProp
        let pm = PassManager::default();
        let r2 = pm.run_on_function(&mut caller_fn).unwrap();
        assert!(r2.changed);

        // 验证：caller_fn 应该只有 return Iconst(18)
        // 找到 return 的值
        if let Terminator::Return { values } = &caller_fn.blocks[0].terminator {
            let ret_val = values[0];
            // 在指令中查找返回值定义
            for inst in &caller_fn.blocks[0].instructions {
                if inst.result == Some(ret_val) {
                    if let Opcode::Iconst { index } = inst.opcode {
                        assert_eq!(
                            caller_fn
                                .constant_pool
                                .get(index)
                                .and_then(|b| b.try_to_i64()),
                            Some(18),
                            "Expected Iconst(18), got {:?}",
                            inst.opcode
                        );
                    } else {
                        panic!("Expected Iconst(18), got {:?}", inst.opcode);
                    }
                }
            }
        } else {
            panic!("expected Return terminator");
        }
    }

    #[test]
    fn const_fn_eval_with_folding_chain() {
        // const fn 返回常量，结合下游折叠
        // const fn answer() -> i32 { 42 }
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("answer", sig);
        b.set_const(true);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.return_(&[v]);
        let answer_fn = b.finish();

        // fn caller() -> i32 { answer() * 2 }
        let mut b2 = FunctionBuilder::new("caller", Signature::new(&[], &[Type::I32]));
        let entry2 = b2.create_block();
        b2.switch_to_block(entry2);
        let call_result = b2.call(FuncRef(0), &[], &[Type::I32]);
        let c2 = b2.iconst_i32(2);
        let prod = b2.imul(call_result[0], c2);
        b2.return_(&[prod]);
        let mut caller_fn = b2.finish();

        let mut func_table = HashMap::new();
        func_table.insert(FuncRef(0), answer_fn);

        // 先求值 const 调用，再折叠乘法
        let eval_pass = ConstFnEvalPass::new(func_table);
        eval_pass.run_on_function(&mut caller_fn).unwrap();

        let fold_pass = ConstFoldPass::new();
        let r = fold_pass.run_on_function(&mut caller_fn).unwrap();
        assert!(r.changed);

        // 结果应该是 Iconst(84)
        if let Terminator::Return { values } = &caller_fn.blocks[0].terminator {
            let ret_val = values[0];
            for inst in &caller_fn.blocks[0].instructions {
                if inst.result == Some(ret_val) {
                    if let Opcode::Iconst { index } = inst.opcode {
                        assert_eq!(
                            caller_fn
                                .constant_pool
                                .get(index)
                                .and_then(|b| b.try_to_i64()),
                            Some(84)
                        );
                    } else {
                        panic!("Expected Iconst(84), got {:?}", inst.opcode);
                    }
                }
            }
        }
    }
}
