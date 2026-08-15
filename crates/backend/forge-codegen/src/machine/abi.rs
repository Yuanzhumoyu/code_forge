//! TargetABI — 调用约定接口。
//!
//! 描述参数传递、返回值、栈对齐等 ABI 规则。
//! 与 TargetFrameLowering 分离：ABI 描述"什么"，FrameLowering 实现"怎么做"。

use forge_ir::{CallConv, PhysReg};

/// 调用约定描述。
pub trait TargetABI: Send + Sync + 'static {
    type Reg: PhysReg;

    /// 当前调用约定。
    fn call_conv(&self) -> CallConv {
        CallConv::Default
    }

    /// 参数传递寄存器（按顺序）。
    fn arg_regs(&self) -> Vec<Self::Reg>;

    /// 返回值寄存器（按顺序）。
    fn ret_regs(&self) -> Vec<Self::Reg>;

    /// 栈对齐（字节）。
    fn stack_align(&self) -> u32 {
        16
    }

    /// 帧布局的额外栈填充（字节）：x86 = 8（align/2，SysV/Windows x64
    /// red-zone 约束）；其他 ABI 缺省 0。架构事实由 TOML 的 `[abi].frame_padding` 声明。
    fn frame_padding(&self) -> i32 {
        0
    }

    /// 红区大小。`Some(n)` 表示栈指针以下 n 字节不被信号处理程序破坏。
    fn red_zone(&self) -> Option<u32> {
        None
    }
}
