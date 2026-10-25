//! TargetFrameLowering — 函数栈帧发射接口。
//!
//! 负责 prologue/epilogue 生成和栈帧布局。
//! 与 TargetABI 分离：ABI 描述调用约定，FrameLowering 实现栈帧操作。

use crate::{AllocResult, CodeSink, IrError};
use forge_ir::RegClass; // AllocResult = AllocResult (type alias)

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
    ) -> Result<(), IrError>;

    /// 发射函数尾声：恢复 callee-saved 寄存器、释放栈帧、返回。
    fn emit_epilogue(
        &self,
        frame_size: u32,
        reg_map: &AllocResult,
        sink: &mut CodeSink,
    ) -> Result<(), IrError>;

    /// Emit a jump to the epilogue label (used at end of return blocks).
    fn emit_epilogue_jump(
        &self,
        _encoder: &std::sync::Arc<dyn super::encoder::TargetEncoder<Inst = Self::Inst>>,
        _reg_map: &AllocResult,
        _epilogue_block: crate::emit::LabelRef,
        _sink: &mut CodeSink,
    ) -> Result<(), IrError> {
        // 默认实现：未覆盖的 ISA 报错而非 panic（错误优雅传播）
        // 这份谱没覆盖这个帧操作 ⇒ **能力缺口**（进台账，可搜索、可豁免），不是"未实现"。
        Err(IrError::capability(
            "FRAME_EMIT_EPILOGUE_JUMP",
            "TargetFrameLowering::emit_epilogue_jump must be overridden per ISA",
        ))
    }

    /// 溢出槽偏移以**哪个寄存器**为基（v27 M6-106）。
    ///
    /// - `false`（缺省）= 帧指针口径：发射侧传入的偏移就是「相对 fp」的有符号字节数
    ///   （历史口径，imm9/有符号 imm12 形态能直接表达）。
    /// - `true` = 栈指针口径：发射侧改传「相对 sp」的偏移（fp-inside 布局下 =
    ///   `frame + fp_offset`，恒非负、按槽宽对齐）——模板必须用**无符号 scaled
    ///   imm12** 形态（如 A64 `LDRX {0}, SP, {1}`），这是大帧（> ±256B）能继续
    ///   溢出的唯一办法。
    ///
    /// 生成物按 `[machine.spill.*].offset_base = "sp"` 覆盖（按寄存器类分派，因为
    /// 不同类的模板可以选不同基准）。
    fn spill_offset_from_sp(&self, _class: RegClass) -> bool {
        false
    }

    /// Emit a spill load: load a spilled value from stack into `dst_reg`.
    ///
    /// ## `class` 与 `width` 回答的是**两个不同的问题**（v23 M5 明确，别合并）
    ///
    /// - `class` = 该值所在的**寄存器类**：决定用哪一族搬运指令 —— FPR/VEC ⇒ 浮点/向量
    ///   模板、GPR ⇒ 通用模板、KReg ⇒ 本仓没有掩码溢出模板，**显式 fail-closed**
    ///   （不静默退回 GPR）。原来这里是 `is_fp: bool`，FPR/VEC 与 KReg 在同一个 `false`
    ///   上撞车，掩码值会静默走通用寄存器搬运。
    /// - `width` = 该值在栈槽里占的**字节数**：决定同一族内取哪一档模板
    ///   （生成物按 `width <= 浮点值池` / `width == 16/32/64` 分派，见 WA-46）。
    ///
    /// 二者**不能互相替代**：`class.bytes()` 是**寄存器类**的宽度（池宽），`width` 是
    /// **值**的宽度——同一个 `FPR(8)` 池的寄存器可装 f32(4B) 或 f64(8B)，同一个
    /// `GPR(8)` 池的寄存器可装 i32(4B) 或 i64(8B)。所以调用点两样都要给，且必须同源：
    /// **类取值的类**（`xreg.class()`，不是分配到的池类 `preg.class`）、
    /// **宽度取值的字节数**（`xreg.bytes()`）。
    fn emit_spill_load(
        &self,
        _dst_reg: u32,
        _offset: i32,
        _width: u16,
        _class: RegClass,
        _sink: &mut CodeSink,
    ) -> Result<(), IrError> {
        // 这份谱没覆盖这个帧操作 ⇒ **能力缺口**（进台账，可搜索、可豁免），不是"未实现"。
        Err(IrError::capability(
            "FRAME_SPILL_LOAD",
            "TargetFrameLowering::emit_spill_load must be overridden per ISA",
        ))
    }

    /// Emit a spill store: store `src_reg` to spilled stack slot.
    /// `width` / `class` 语义同 [`Self::emit_spill_load`]。
    fn emit_spill_store(
        &self,
        _src_reg: u32,
        _offset: i32,
        _width: u16,
        _class: RegClass,
        _sink: &mut CodeSink,
    ) -> Result<(), IrError> {
        // 这份谱没覆盖这个帧操作 ⇒ **能力缺口**（进台账，可搜索、可豁免），不是"未实现"。
        Err(IrError::capability(
            "FRAME_SPILL_STORE",
            "TargetFrameLowering::emit_spill_store must be overridden per ISA",
        ))
    }
}
