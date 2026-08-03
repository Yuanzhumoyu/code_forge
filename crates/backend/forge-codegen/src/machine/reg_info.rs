//! TargetRegInfo — 物理寄存器文件描述。
//!
//! 与 ABI 分离，因为寄存器文件结构（名称、数量、宽度）独立于调用约定。

use forge_ir::{FrameAccess, PReg, PhysReg, RegClass, XReg};

/// 物理寄存器文件描述。
///
/// 为寄存器分配器提供寄存器资源信息。
pub trait TargetRegInfo: Send + Sync + 'static {
    type Reg: PhysReg;

    /// 通用寄存器数量。
    fn num_gp_regs(&self) -> u8;
    /// 浮点/向量寄存器数量。
    fn num_fp_regs(&self) -> u8;

    /// 寄存器类元数据。
    fn register_classes(&self) -> &[super::isa_info::RegisterClassInfo] {
        &[]
    }

    /// 寄存器类的字节宽度（从 ISA TOML [reg_classes] 读取）。
    /// 默认使用 RegClass::default_width()。
    fn reg_class_width(&self, class: RegClass) -> u8 {
        class.default_width()
    }

    /// 栈指针寄存器。
    fn sp_reg(&self) -> FrameAccess<Self::Reg>;
    /// 帧指针寄存器（None 表示 ISA 不使用帧指针）。
    fn fp_reg(&self) -> Option<Self::Reg>;

    /// 通用寄存器分配优先级顺序（靠前的优先分配）。
    /// 排除 SP、FP 和被调用者保存寄存器。
    fn allocatable_gp_order(&self) -> Vec<u8>;

    /// 浮点寄存器分配优先级顺序。
    fn allocatable_fp_order(&self) -> Vec<u8>;

    /// 溢出代码可用的临时寄存器（不跨 spill load/emit/store 使用）。
    fn scratch_regs(&self) -> Vec<u8>;

    /// 被调用者保存的寄存器索引列表。
    fn callee_saved(&self) -> Vec<u8>;

    /// Prologue 在帧指针上方 push 的字节数（帧指针保存槽，如 x86 `push rbp`
    /// = 8；aarch64/riscv64 `stp/sd fp,lr` = 16；wasm 无帧 = 0）。
    /// codegen 用它计算局部变量区基址（`fp - overhead - callee_saved_bytes`）。
    fn frame_pointer_overhead(&self) -> u32 {
        8
    }

    /// 预着色的 VReg → PReg 映射（如 RAX = VReg(0) 用于返回值）。
    fn precolored_xregs(&self) -> Vec<(XReg, PReg)> {
        Vec::new()
    }
}
