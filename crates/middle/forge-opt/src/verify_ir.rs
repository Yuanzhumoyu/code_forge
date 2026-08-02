//! IR verification pass — insert IR correctness checks in optimization pipeline.
//!
//! In debug mode, automatically validates IR after each pass to catch optimization bugs.
//!
//! # Usage
//!
//! ```ignore
//! pm.add_pass(VerifyIrPass::new(), PassRunMode::Once);
//! ```

use crate::{OptimizationPass, PassResult};
use forge_ir::CompileError;
use forge_ir::Function;

/// IR verification pass — calls `VerifyContext::verify()` and reports errors.
pub struct VerifyIrPass {
    /// Whether to abort compilation on verification failure.
    pub fail_on_error: bool,
}

impl VerifyIrPass {
    pub fn new() -> Self {
        Self {
            fail_on_error: true,
        }
    }

    /// Create a warn-only (non-fatal) verification pass.
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
        // Use forge_ir::Verifier for IR validation
        let mut v = forge_ir::verify::Verifier::new();
        if let Err(errors) = v.verify(func) {
            let msg = errors
                .iter()
                .map(|e| format!("{:?}", e))
                .collect::<Vec<_>>()
                .join("; ");
            log::error!("IR verification failed in '{}': {}", func.name, msg);
            if self.fail_on_error {
                return Err(CompileError::Internal(format!(
                    "IR verification failed for '{}': {}",
                    func.name, msg
                )));
            }
        }
        Ok(PassResult::default())
    }
}
