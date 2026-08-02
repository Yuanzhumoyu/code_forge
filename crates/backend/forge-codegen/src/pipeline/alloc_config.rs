//! RegAllocConfig — 寄存器分配器配置。
//!
//! 从 TargetRegInfo 构建，ISA 无关。

use forge_ir::*;
use std::collections::HashMap;

use crate::pipeline::alloc_result::AllocResult;

/// 寄存器分配器配置 — ISA 特定的分配参数。
///
/// 由 `CompilerState::build_regalloc_config()` 从 `TargetRegInfo` 构建。
#[derive(Clone, Debug)]
pub struct RegAllocConfig {
    /// 所有寄存器类及其分配参数
    pub classes: HashMap<RegClass, ClassConfig>,

    /// 栈指针物理寄存器索引
    pub sp_reg: u8,
    /// 帧指针物理寄存器索引（可选）
    pub fp_reg: Option<u8>,

    /// callee-saved 物理寄存器索引列表
    pub callee_saved: Vec<u8>,

    /// 预着色 XReg → PReg（ABI 强制映射：返回值、参数寄存器等）
    pub precolored: HashMap<XReg, PReg>,

    /// scratch 寄存器池（spill reload 时借用）
    pub scratch_regs: Vec<PReg>,

    /// 函数参数 XReg（live range 需从程序点 0 开始）
    pub param_xregs: Vec<XReg>,
}

/// 单个寄存器类的配置。
#[derive(Clone, Debug)]
pub struct ClassConfig {
    /// 此类可分配的物理寄存器编号列表
    pub allocatable: Vec<u8>,
    /// 寄存器的字节宽度（用于计算栈槽大小）
    pub reg_width: u8,
}

impl RegAllocConfig {
    /// 从原始参数构造最小配置（用于测试）。
    pub fn new(num_gp_regs: u8, num_fp_regs: u8, sp_reg: u8, fp_reg: Option<u8>) -> Self {
        let mut classes = HashMap::new();
        classes.insert(
            RegClass::GPR,
            ClassConfig {
                allocatable: (0..num_gp_regs).collect(),
                reg_width: 8,
            },
        );
        classes.insert(
            RegClass::FPR,
            ClassConfig {
                allocatable: (0..num_fp_regs).collect(),
                reg_width: 8,
            },
        );
        Self {
            classes,
            sp_reg,
            fp_reg,
            callee_saved: Vec::new(),
            precolored: HashMap::new(),
            scratch_regs: Vec::new(),
            param_xregs: Vec::new(),
        }
    }
}

/// 从 LowerCtx 提取的、分配器需要的上下文子集。
///
/// 避免寄存器分配器依赖整个 LowerCtx。
/// 类型（RegClass）与位宽（width）均由 XReg 值内嵌，无需额外表。
#[derive(Clone, Debug, Default)]
pub struct AllocContext {}

impl AllocContext {
    /// 从 AllocResult 提取上下文（用于编码阶段的 size estimation）。
    pub fn from_alloc_result(_result: &AllocResult) -> Self {
        Self {}
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_new_defaults() {
        let cfg = RegAllocConfig::new(16, 16, 13, Some(14));
        assert_eq!(cfg.sp_reg, 13);
        assert_eq!(cfg.fp_reg, Some(14));
        assert_eq!(cfg.classes.len(), 2);
        assert!(cfg.callee_saved.is_empty());
        assert!(cfg.precolored.is_empty());
        assert!(cfg.param_xregs.is_empty());
    }

    #[test]
    fn test_config_gpr_class() {
        let cfg = RegAllocConfig::new(8, 0, 7, None);
        let gpr = cfg.classes.get(&RegClass::GPR).unwrap();
        assert_eq!(gpr.allocatable.len(), 8);
        assert_eq!(gpr.reg_width, 8);
    }

    #[test]
    fn test_config_fpr_class() {
        let cfg = RegAllocConfig::new(0, 16, 7, None);
        let fpr = cfg.classes.get(&RegClass::FPR).unwrap();
        assert_eq!(fpr.allocatable.len(), 16);
        assert_eq!(fpr.reg_width, 8);
    }

    #[test]
    fn test_alloc_context_default() {
        // 类型与位宽由 XReg 值内嵌，AllocContext 为空结构。
        let ctx = AllocContext::default();
        let _ = ctx;
    }
}
