//! InstructionSet trait — ISA 核心 lowering + emit 接口。
//!
//! 关联 IsaInfo 作为 super trait。Decoder/Encoder/Disassembler/Simulator/Assembler
//! 等已提取为独立 trait，各自按需实现。

use crate::backend::isa_info::IsaInfo;
use crate::backend::machine_inst::MachineInst;
use crate::backend::*;
use crate::CompileError;
use std::collections::HashMap;

/// 指令集 trait — v9 重构版本。
///
/// 核心变更：
/// - 关联 `IsaInfo` 作为 super trait（ISA 必须能自描述）
/// - 新增 `decode`/`encode`/`disassemble`/`simulate` 默认空实现
/// - 保留核心 `lower` + `emit` 方法
pub trait InstructionSet: IsaInfo + Send + Sync + 'static {
    /// 本 ISA 的机器指令类型。
    type Inst: MachineInst;

    /// 本 ISA 的物理寄存器类型。
    type Reg: PhysReg;

    // ============================================================
    // 核心方法（必须实现）
    // ============================================================

    /// 指令选择：将 IR 操作码降低为机器指令序列。
    fn lower(
        op: &Opcode,
        args: &[VReg],
        result: Option<VReg>,
        ctx: &mut LowerCtx,
    ) -> Result<Vec<Self::Inst>, CompileError>;

    /// lowering 终止指令（条件分支、跳转、返回等）。
    fn lower_terminator(
        term: &Terminator,
        value_to_vreg: &HashMap<Value, VReg>,
        block_to_vblock: &HashMap<BlockId, VCodeBlockId>,
        ctx: &mut LowerCtx,
    ) -> Result<Vec<Self::Inst>, CompileError>;

    /// 代码发射：将一条机器指令转换为原始字节。
    fn emit(
        inst: &Self::Inst,
        reg_map: &RegMap,
        sink: &mut CodeSink,
    ) -> Result<(), CompileError>;

    // ============================================================
    // ABI / 编译器配置（有默认值，可按需覆盖）
    // ============================================================

    /// 通用寄存器数量。
    fn num_gp_regs() -> u8 {
        16
    }

    /// 浮点寄存器数量。
    fn num_fp_regs() -> u8 {
        16
    }

    /// 预着色的虚拟寄存器 — 必须映射到特定物理寄存器。
    fn precolored_vregs() -> std::collections::HashMap<crate::ir::VReg, crate::ir::PReg> {
        std::collections::HashMap::new()
    }

    /// 参数传递寄存器列表。
    fn arg_regs() -> Vec<Self::Reg> {
        (0..4)
            .map(|i| Self::Reg::from_index(i, RegClass::Int))
            .collect()
    }

    /// 返回值寄存器列表。
    fn ret_regs() -> Vec<Self::Reg> {
        vec![Self::Reg::from_index(0, RegClass::Int)]
    }

    /// 被调用者保存寄存器列表。
    fn callee_save_regs() -> Vec<Self::Reg> {
        (4..8)
            .map(|i| Self::Reg::from_index(i, RegClass::Int))
            .collect()
    }

    /// 栈指针位置（寄存器或内存）。
    fn sp_reg() -> FrameAccess<Self::Reg> {
        FrameAccess::Register(Self::Reg::from_index(14, RegClass::Int))
    }

    /// 帧指针位置。
    fn fp_reg() -> FrameAccess<Self::Reg> {
        FrameAccess::Register(Self::Reg::from_index(15, RegClass::Int))
    }

    /// 栈对齐要求（字节）。
    fn stack_align() -> u32 {
        16
    }

    /// 红区大小（字节）。返回 `Some(n)` 表示栈指针以下 `n` 字节不会被
    /// 信号处理程序破坏。x86-64 System V ABI 为 `Some(128)`，
    /// Windows x64 为 `None`。
    fn red_zone() -> Option<u32> {
        None
    }

    // ============================================================
    // 可选覆盖的方法
    // ============================================================

    /// 计算指令大小（用于分支偏移计算）。
    fn inst_size(_inst: &Self::Inst) -> u32 {
        4
    }

    /// 生成函数序言。
    fn emit_prologue(frame_size: u32, reg_map: &RegMap, sink: &mut CodeSink) {
        let _ = (frame_size, reg_map, sink);
    }

    /// 生成函数尾声。
    fn emit_epilogue(frame_size: u32, reg_map: &RegMap, sink: &mut CodeSink) {
        let _ = (frame_size, reg_map, sink);
    }

    /// 编码 ISA 特定的重定位。
    fn encode_isa_reloc(
        _kind: &crate::RelocKind,
        _bytes: &mut [u8],
        _offset: usize,
        _target: usize,
    ) -> Result<(), CompileError> {
        Err(CompileError::Emit(
            "ISA-specific relocation not supported".into(),
        ))
    }

    /// 函数在对象文件中的对齐要求（字节）。
    fn function_alignment() -> u32 {
        1
    }

    /// 窥孔优化。
    fn peephole_optimize(insts: &mut Vec<Self::Inst>) -> usize {
        let mut eliminated = 0;
        let mut i = 0;
        while i < insts.len() {
            // Remove no-op moves: mov x, x
            if let Some((dst, src)) = insts[i].is_move() && dst == src {
                insts.remove(i); eliminated += 1; continue;
            }
            // Remove redundant defs: if current def is overwritten by next instruction
            // AND the current instruction's uses are all covered by its own defs
            // (i.e., it's a pure computation, not a value-forwarding move)
            if i + 1 < insts.len() {
                let defs_current = insts[i].defs();
                let defs_next = insts[i + 1].defs();
                let uses_current = insts[i].uses();
                if !defs_current.is_empty() && defs_current == defs_next
                    && insts[i].is_foldable() && !insts[i + 1].has_side_effects()
                    // Only remove if current doesn't forward external values
                    && uses_current.iter().all(|u| defs_current.contains(u))
                {
                    insts.remove(i); eliminated += 1; continue;
                }
            }
            i += 1;
        }
        eliminated
    }

    /// 基于模式的指令选择 — 当 pattern_isel 在 IR 中检测到已知模式时调用。
    ///
    /// `pattern_name` 是匹配的模式（例如 `"lea-merge-iadd-imul"`）。
    /// `matched_insts` 是匹配的 IR 指令切片。
    /// `args` / `result` 解析自 SSA 值。
    ///
    /// 默认返回空 vec（无操作——模式匹配被记录但无重写发生）。
    /// 覆盖此方法以为特定模式提供优化的机器码序列。
    fn lower_pattern(
        _pattern_name: &str,
        _matched_insts: &[crate::ir::Instruction],
        _args: &[VReg],
        _result: Option<VReg>,
        _ctx: &mut LowerCtx,
    ) -> Result<Vec<Self::Inst>, CompileError> {
        Ok(Vec::new())
    }

}


/// 解码错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum DecodeError {
    #[error("decode not supported")]
    Unsupported,
    #[error("invalid instruction bytes at offset {0}")]
    InvalidBytes(usize),
    #[error("{0}")]
    Other(String),
}

/// 编码错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum EncodeError {
    #[error("encode not supported")]
    Unsupported,
    #[error("{0}")]
    Other(String),
}

/// 模拟器状态 trait。
pub trait SimulationState: std::fmt::Debug + Clone + Default {
    /// 读取寄存器（按编号）。
    fn read_reg(&self, reg: u8) -> u64;
    /// 写入寄存器。
    fn write_reg(&mut self, reg: u8, val: u64);
    /// 读取内存。
    #[allow(clippy::result_unit_err)]
    fn read_mem(&self, addr: u64, size: u8) -> Result<u64, ()>;
    /// 写入内存。
    #[allow(clippy::result_unit_err)]
    fn write_mem(&mut self, addr: u64, size: u8, val: u64) -> Result<(), ()>;
}

/// 模拟错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum SimError {
    #[error("simulation not supported")]
    Unsupported,
    #[error("unimplemented instruction")]
    Unimplemented,
    #[error("{0}")]
    Other(String),
}
