//! TargetPeephole — 机器指令窥孔优化接口。
//!
//! 在寄存器分配前对 VCode 基本块内的机器指令序列进行局部优化。

/// 窥孔优化器。
pub trait TargetPeephole: Send + Sync + 'static {
    type Inst: super::inst::MachineInst;

    /// 优化一个基本块的指令序列。
    /// 返回消除的指令数量。
    fn optimize(&self, insts: &mut Vec<Self::Inst>) -> usize;
}
