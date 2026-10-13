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

    /// callee-saved 寄存器：**(类, 该类内的号)** 一张表（v23 M5 的 C4 收口）。
    ///
    /// 以前是两份平行列表（主 GPR 类一份、主 FPR 类一份），消费端两处都**只比号不比类**：
    /// `pop_free` 的"优先用 callee-saved"因此会让 GPR 号与 FPR 号互相误判（arm64 的 V8 与
    /// X8 同号）；保存集那一侧又各写一遍扫描逻辑。合成一张表之后，① 偏好过滤是 `(类, 号)`
    /// 精确匹配；② 保存集按类一次扫完。
    ///
    /// 这张表只表达"哪些是 callee-saved"，**不**表达"存不存"——两类在保存阶段的语义仍不同
    ///（GPR 整表推、FPR 只存实际用到的：后者是 arm64 QEMU 通道的历史闸门，见
    /// `regalloc_bt.rs` 的注释）。
    pub callee_saved: Vec<(RegClass, u32)>,

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
            scratch_regs: Vec::new(),
            param_xregs: Vec::new(),
            param_reg_count: 0,
            main_gpr_class: RegClass::GPR64,
            main_fpr_class: RegClass::FPR64,
        }
    }
}

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
}
