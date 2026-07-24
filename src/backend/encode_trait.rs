//! Encoder trait — 独立的指令编码接口。
//!
//! 与 `InstructionSet::emit()` 不同，此 trait 专为编码器设计，
//! 不需要寄存器映射（用于汇编器/静态编码场景）。

use crate::backend::emit::CodeSink;
use crate::backend::instruction_set::EncodeError;
use crate::backend::isa_info::IsaInfo;
use crate::backend::machine_inst::MachineInst;
use crate::backend::RegMap;

/// 编码器 trait — 独立于 InstructionSet 的可选编码组件。
///
/// 当 ISA 支持独立编码（从指令到字节，不需要 RegMap）时实现此 trait。
/// 主要用于汇编器、反汇编器等工具场景。
pub trait Encoder: IsaInfo {
    type Inst: MachineInst;
    type EncodeState: Default + Clone + std::fmt::Debug;

    fn encode(inst: &Self::Inst) -> Result<Vec<u8>, EncodeError> {
        Self::encode_with_state(inst, &Self::EncodeState::default())
    }

    fn encode_with_state(
        inst: &Self::Inst,
        state: &Self::EncodeState,
    ) -> Result<Vec<u8>, EncodeError>;

    fn encode_into(inst: &Self::Inst, buf: &mut [u8]) -> Result<usize, EncodeError> {
        let bytes = Self::encode(inst)?;
        let len = bytes.len();
        if buf.len() < len {
            return Err(EncodeError::Other("buffer too small".into()));
        }
        buf[..len].copy_from_slice(&bytes);
        Ok(len)
    }

    fn encoded_size(inst: &Self::Inst) -> Result<usize, EncodeError> {
        Self::encode(inst).map(|v| v.len())
    }
}

