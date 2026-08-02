//! RegAlloc trait — 可插拔寄存器分配器接口。
//!
//! 分配器是纯函数：VCode + 配置 → AllocResult。
//! VCode 在分配后不变；所有分配信息在 AllocResult 中。

use crate::pipeline::alloc_config::{AllocContext, RegAllocConfig};
use forge_ir::XReg;
use crate::pipeline::alloc_result::AllocResult;
use crate::{CompileError, MachineInst, VCode};

/// 寄存器分配器接口。
///
/// # 设计原则
///
/// 1. **纯函数**: 输入只读，输出独立。VCode 在分配后不变。
/// 2. **ISA 无关**: 核心算法不依赖 ISA；ISA 特定约束通过 [`RegAllocConfig`] 注入。
/// 3. **可替换**: 编译时或运行时切换分配器实现。
pub trait RegAlloc: Send + Sync {
    /// 执行寄存器分配。
    ///
    /// # 参数
    /// - `vcode`: 机器指令序列（只读）
    /// - `config`: ISA 特定的寄存器类、precolor、scratch 池等配置
    /// - `ctx`: lowering 上下文（VReg→RegClass、VReg→宽度映射）
    ///
    /// # 返回
    /// [`AllocResult`] — assignments + spill slots + frame info
    fn allocate<I: MachineInst>(
        &self,
        vcode: &VCode<I>,
        config: &RegAllocConfig,
        ctx: &AllocContext,
        xreg_map: &[smallvec::SmallVec<[XReg; 2]>],
    ) -> Result<AllocResult, CompileError>;

    /// 分配器名称（用于调试/日志）。
    fn name(&self) -> &'static str;
}
