//! TargetFrameLowering — 函数栈帧发射接口。
//!
//! 负责 prologue/epilogue 生成和栈帧布局。
//! 与 TargetABI 分离：ABI 描述调用约定，FrameLowering 实现栈帧操作。

use crate::{AllocResult, CodeSink, CompileError}; // AllocResult = AllocResult (type alias)

/// 函数级栈帧管理。
pub trait TargetFrameLowering: Send + Sync + 'static {
    type Inst: super::inst::MachineInst;

    /// 是否需要独立的尾声标签（传统架构为 true，WASM 等栈机为 false）。
    fn needs_epilogue_label(&self) -> bool {
        true
    }

    /// 函数在对象文件中的对齐要求（字节）。
    fn function_alignment(&self) -> u32 {
        1
    }

    /// 发射函数序言：保存帧指针、callee-saved 寄存器、分配栈帧。
    fn emit_prologue(
        &self,
        frame_size: u32,
        reg_map: &AllocResult,
        sink: &mut CodeSink,
    ) -> Result<(), CompileError>;

    /// 发射函数尾声：恢复 callee-saved 寄存器、释放栈帧、返回。
    fn emit_epilogue(
        &self,
        frame_size: u32,
        reg_map: &AllocResult,
        sink: &mut CodeSink,
    ) -> Result<(), CompileError>;

    /// Emit a jump to the epilogue label (used at end of return blocks).
    fn emit_epilogue_jump(
        &self,
        _encoder: &std::sync::Arc<dyn super::encoder::TargetEncoder<Inst = Self::Inst>>,
        _reg_map: &AllocResult,
        _epilogue_block: forge_ir::Block,
        _sink: &mut CodeSink,
    ) -> Result<(), CompileError> {
        unimplemented!("TargetFrameLowering::emit_epilogue_jump must be overridden per ISA")
    }

    /// Emit a spill load: load a spilled value from stack into `dst_reg`.
    /// `width` is the size in bytes of the spilled value.
    fn emit_spill_load(
        &self,
        _dst_reg: u8,
        _offset: i32,
        _width: u8,
        _sink: &mut CodeSink,
    ) -> Result<(), CompileError> {
        unimplemented!("TargetFrameLowering::emit_spill_load must be overridden per ISA")
    }

    /// Emit a spill store: store `src_reg` to spilled stack slot.
    /// `width` is the size in bytes of the spilled value.
    fn emit_spill_store(
        &self,
        _src_reg: u8,
        _offset: i32,
        _width: u8,
        _sink: &mut CodeSink,
    ) -> Result<(), CompileError> {
        unimplemented!("TargetFrameLowering::emit_spill_store must be overridden per ISA")
    }
}
