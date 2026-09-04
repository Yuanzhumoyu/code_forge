//! TargetABI — 调用约定接口。
//!
//! 描述参数传递、返回值、栈对齐等 ABI 规则。
//! 与 TargetFrameLowering 分离：ABI 描述"什么"，FrameLowering 实现"怎么做"。

use forge_ir::{CallConv, PhysReg};

/// 帧布局模式（[abi.frame].layout）：callee-saved 保存槽相对帧的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameLayoutKind {
    /// fp-outside（x86/demo）：callee-saved 用硬件 push 在帧指针上方（帧外）。
    Outside,
    /// fp-inside（riscv）：ra/fp/callee-saved 保存槽在帧内顶部。
    Inside,
}

/// 声明式帧布局（[abi.frame] 的两个正交事实）。其余帧数值
/// （min_frame / callee_saved_bytes / stack_slot_shift）由
/// `pipeline::frame_layout::frame_layout_info` 从本结构 + reg_info 推导。
#[derive(Debug, Clone, Copy)]
pub struct FrameLayout {
    /// 帧布局模式（缺省 fp-outside）。
    pub kind: FrameLayoutKind,
    /// prologue 在帧指针上方保存的帧指针槽字节数（x86 = 8、riscv 16）。
    pub fp_push_bytes: u32,
}

impl Default for FrameLayout {
    fn default() -> Self {
        FrameLayout {
            kind: FrameLayoutKind::Outside,
            fp_push_bytes: 8,
        }
    }
}

/// 调用约定描述。
pub trait TargetABI: Send + Sync + 'static {
    type Reg: PhysReg;

    /// 当前调用约定。
    fn call_conv(&self) -> CallConv {
        CallConv::Default
    }

    /// 参数传递寄存器（按顺序）。
    fn arg_regs(&self) -> Vec<Self::Reg>;

    /// 寄存器参数位置上限（by-position：int/float 共享位置计数，位置
    /// < 此值走寄存器、≥ 此值走栈——Windows x64 = 4 个 int 槽）。缺省
    /// = arg_regs().len()（全部参数寄存器传参，无栈参数）。
    fn int_arg_slot_count(&self) -> usize {
        self.arg_regs().len()
    }

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

    /// 声明式帧布局：`[abi.frame].layout`（fp-inside/fp-outside）+
    /// `fp_push_bytes`。min_frame / callee_saved_bytes / stack_slot_shift
    /// 三数值由 [`crate::pipeline::frame_layout::frame_layout_info`] 从本结构
    /// + reg_info 推导，不再有 `min_frame_bytes` 等魔法数方法。
    fn frame_layout(&self) -> FrameLayout {
        FrameLayout::default()
    }

    /// 红区大小。`Some(n)` 表示栈指针以下 n 字节不被信号处理程序破坏。
    fn red_zone(&self) -> Option<u32> {
        None
    }
}
