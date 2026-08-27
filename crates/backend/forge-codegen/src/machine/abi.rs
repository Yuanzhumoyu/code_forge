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

    /// 按引用传参的向量大小阈值（字节）：**超过**此字节的向量参数/返回走
    /// by-ref（调用方栈上分配副本、传指针 GPR；被调方入口从 [ptr] 加载）。
    /// 例如 x86 声明 `vector by-ref limit=128`（位）→ 16 字节：>16 字节向量
    /// （V256/V512）按引用传参，≤16 字节（V64/V128）仍寄存器传值。
    /// None = 不启用 by-ref（缺省；宽向量参数/返回将被拒绝）。
    fn vector_by_ref_limit(&self) -> Option<u32> {
        None
    }

    /// 帧布局的额外栈填充（字节）：x86 = 8（align/2，SysV/Windows x64
    /// red-zone 约束）；其他 ABI 缺省 0。架构事实由 TOML 的 `[abi].frame_padding` 声明。
    fn frame_padding(&self) -> i32 {
        0
    }

    /// 帧最小字节数（[abi.frame].min_frame_bytes）：riscv 的 ra/fp 保存槽
    /// 需要帧 ≥ 固定值（emit 模板的 `{frame_size_mN}` 偏移才非负）。缺省 0。
    fn min_frame_bytes(&self) -> u32 {
        0
    }

    /// callee-saved 区字节数覆盖（[abi.frame].callee_saved_bytes_override）。
    /// None = 按 frame_pointer_overhead + callee_saved×宽 计算（x86 语义：
    /// push 在帧外/fp 上方）。riscv 覆盖 0（保存槽在帧内顶部）→ spill 槽
    /// sp_base = -(frame) 留在帧内（否则落帧外与递归帧重叠）。
    fn callee_saved_bytes_override(&self) -> Option<u32> {
        None
    }

    /// 栈槽（StackAddr/Alloca）的帧顶平移字节数（[abi.frame].
    /// stack_slot_shift）。None = 回退 callee_saved_bytes（x86 语义）。
    /// riscv = fp_push_bytes（16）——栈槽基准 fp - 16（避开 ra/fp 保存槽），
    /// 递归时各帧栈槽独立（相对 sp 的 -4 会跨帧重叠——fib_slot 结果错）。
    fn stack_slot_shift(&self) -> Option<i32> {
        None
    }

    /// 红区大小。`Some(n)` 表示栈指针以下 n 字节不被信号处理程序破坏。
    fn red_zone(&self) -> Option<u32> {
        None
    }
}
