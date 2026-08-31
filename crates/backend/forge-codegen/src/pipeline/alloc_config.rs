//! RegAllocConfig — 寄存器分配器配置。
//!
//! 从 TargetRegInfo 构建，ISA 无关。

use forge_ir::*;
use std::collections::HashMap;

/// 寄存器分配器配置 — ISA 特定的分配参数。
///
/// 由 `CompilerState::build_regalloc_config()` 从 `TargetRegInfo` 构建。
#[derive(Clone, Debug)]
pub struct RegAllocConfig {
    /// 所有寄存器类及其分配参数
    pub classes: HashMap<RegClass, ClassConfig>,

    /// 栈指针物理寄存器索引
    pub sp_reg: u32,
    /// 帧指针物理寄存器索引（可选）
    pub fp_reg: Option<u32>,

    /// callee-saved 物理寄存器索引列表
    pub callee_saved: Vec<u32>,

    /// 预着色 XReg → PReg（ABI 强制映射：返回值、参数寄存器等）
    pub precolored: HashMap<XReg, PReg>,

    /// scratch 寄存器池（spill reload 时借用）
    pub scratch_regs: Vec<PReg>,

    /// 主整数类（lowering 默认 alloc_xreg 目标；由 TargetRegInfo::default_gpr_class 提供）
    pub main_gpr_class: RegClass,
    /// 主浮点类（同上，default_fpr_class）
    pub main_fpr_class: RegClass,

    /// 函数参数 XReg（live range 需从程序点 0 开始）
    pub param_xregs: Vec<XReg>,
    /// 前 `param_reg_count` 个参数在函数入口分配物理寄存器（ABI 寄存器
    /// 传参）；其余参数（栈参数，Windows x64 第 5+）不预分配寄存器——
    /// 强制 spill 到栈槽，避免参数占满寄存器导致函数体无寄存器可驱逐
    ///（grow_impl_runtime 7 参数 regalloc 失败）。0 = 全部按普通值处理。
    pub param_reg_count: usize,
}

/// 单个寄存器类的配置。
#[derive(Clone, Debug)]
pub struct ClassConfig {
    /// 此类可分配的物理寄存器编号列表
    pub allocatable: Vec<u32>,
    /// 寄存器的字节宽度（用于计算栈槽大小）
    pub reg_width: u8,
}

impl RegAllocConfig {
    /// 从原始参数构造最小配置（用于测试）。
    pub fn new(num_gp_regs: u32, num_fp_regs: u32, sp_reg: u32, fp_reg: Option<u32>) -> Self {
        let mut classes = HashMap::new();
        classes.insert(
            RegClass::GPR64,
            ClassConfig {
                allocatable: (0..num_gp_regs).collect(),
                reg_width: 8,
            },
        );
        classes.insert(
            RegClass::FPR64,
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
            param_reg_count: 0,
            main_gpr_class: RegClass::GPR64,
            main_fpr_class: RegClass::FPR64,
        }
    }
}

/// 从 LowerCtx 提取的、分配器需要的上下文子集。
///
/// 避免寄存器分配器依赖整个 LowerCtx。
/// 类型（RegClass）与位宽（width）均由 XReg 值内嵌，无需额外表。
#[derive(Clone, Debug, Default)]
pub struct AllocContext {}

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
        let gpr = cfg.classes.get(&RegClass::GPR64).unwrap();
        assert_eq!(gpr.allocatable.len(), 8);
        assert_eq!(gpr.reg_width, 8);
    }

    #[test]
    fn test_config_fpr_class() {
        let cfg = RegAllocConfig::new(0, 16, 7, None);
        let fpr = cfg.classes.get(&RegClass::FPR64).unwrap();
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
