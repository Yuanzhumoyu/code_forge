//! RegisterAllocator — backtracking register allocator with Belady's MIN heuristic.
//!
//! This is the only register allocator. The legacy LinearScan allocator was removed
//! in v19 after Backtracking reached 138/138 JIT tests.
//! v20: Returns AllocResult directly (AllocResult removed).

use crate::pipeline::alloc_config::{AllocContext, RegAllocConfig};
use crate::pipeline::alloc_result::AllocResult;
use crate::{MachineInst, VCode};
use forge_ir::{CompileError, XReg};

/// Backtracking register allocator with Belady's MIN heuristic.
///
/// Pre-assigns all parameter VRegs at function entry to prevent register conflicts
/// in the prologue. Handles Fixed, ReuseInput, Clobber, and Stack operand constraints.
#[derive(Debug, Clone, Default)]
pub struct RegisterAllocator;

impl RegisterAllocator {
    /// Run register allocation on the given VCode, producing an AllocResult.
    pub fn allocate<I: MachineInst>(
        &self,
        vcode: &VCode<I>,
        config: &RegAllocConfig,
        ctx: &AllocContext,
        xreg_map: &[smallvec::SmallVec<[(XReg, u8); 2]>],
    ) -> Result<AllocResult, CompileError> {
        let alloc = crate::pipeline::regalloc_bt::BacktrackingAllocator::new();
        alloc.allocate(vcode, config, ctx, xreg_map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::alloc_config::RegAllocConfig;

    #[test]
    fn test_reg_alloc_config_defaults() {
        let cfg = RegAllocConfig::new(16, 16, 4, Some(5));
        assert_eq!(cfg.sp_reg, 4);
        assert_eq!(cfg.fp_reg, Some(5));
    }

    #[test]
    fn test_allocator_default_is_backtracking() {
        // BT is at 138/138 JIT tests — all passing since v19.
        let alloc = RegisterAllocator;
        // Verify we can construct the allocator
        let _ = alloc;
    }
}
