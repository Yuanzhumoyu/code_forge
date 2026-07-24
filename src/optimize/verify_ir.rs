//! IR 验证 pass — 在优化管道中插入 IR 正确性检查。
//!
//! Debug 模式下在每个 pass 之后自动验证 IR，捕获优化 bug。
//!
//! # 使用
//!
//! ```ignore
//! pm.add_pass(VerifyIrPass::new(), PassRunMode::Once);
//! ```

use crate::CompileError;
use crate::ir::Function;
use crate::optimize::{OptimizationPass, PassResult};

/// IR 验证 pass — 调用 `Function::validate()` 并报告错误。
pub struct VerifyIrPass {
    /// 是否在验证失败时终止编译。
    pub fail_on_error: bool,
}

impl VerifyIrPass {
    pub fn new() -> Self {
        Self {
            fail_on_error: true,
        }
    }

    /// 创建仅警告（不终止编译）的验证 pass。
    pub fn warn_only() -> Self {
        Self {
            fail_on_error: false,
        }
    }
}

impl Default for VerifyIrPass {
    fn default() -> Self {
        Self::new()
    }
}

impl OptimizationPass for VerifyIrPass {
    fn name(&self) -> &'static str {
        "verify-ir"
    }
    fn description(&self) -> &'static str {
        "Validates IR correctness after each optimization pass"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        let validation = func.validate();
        if !validation.is_valid() {
            let msg = validation.errors.join("; ");
            log::error!("IR verification failed in '{}': {}", func.name, msg);
            if self.fail_on_error {
                return Err(CompileError::Internal(format!(
                    "IR verification failed for '{}': {}",
                    func.name, msg
                )));
            }
        }
        for warning in &validation.warnings {
            log::warn!("IR warning in '{}': {}", func.name, warning);
        }
        Ok(PassResult::default())
    }
}
