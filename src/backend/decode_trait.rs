//! Decoder trait — 独立的指令解码接口。
//!
//! 与 `InstructionSet` 中的 `decode()` 不同，此 trait 专为解码器设计，
//! 支持更丰富的解码上下文（前缀状态、模式切换等）。

use crate::backend::instruction_set::DecodeError;
use crate::backend::isa_info::IsaInfo;
use crate::backend::machine_inst::MachineInst;

/// 解码器 trait — 独立于 InstructionSet 的可选解码组件。
///
/// 当 ISA 支持解码时实现此 trait。解码器可以被单独使用，
/// 不需要完整的指令选择/发射管线。
pub trait Decoder: IsaInfo {
    /// 机器指令类型（必须与 InstructionSet 一致）。
    type Inst: MachineInst;

    /// 前缀状态类型（定长 ISA 可使用 `()`）。
    type PrefixState: Default + Clone + std::fmt::Debug;

    /// 解码：裸字节序列 → 指令。
    ///
    /// 返回 (指令, 消费的字节数)。
    fn decode(bytes: &[u8]) -> Result<(Self::Inst, usize), DecodeError>;

    /// 带前缀上下文的解码。
    ///
    /// `prefix` 是之前解码的前缀状态（如 x86 的 Legacy/REX/VEX/EVEX 前缀）。
    fn decode_with_prefix(
        bytes: &[u8],
        prefix: &Self::PrefixState,
    ) -> Result<(Self::Inst, usize), DecodeError>;

    /// 仅解码前缀部分，返回剩余字节和前上下文。
    ///
    /// 用于流式解码场景（如 x86 的分层前缀解码）。
    fn decode_prefix_only(bytes: &[u8]) -> Result<(Self::PrefixState, usize), DecodeError> {
        let _ = bytes;
        Ok((Self::PrefixState::default(), 0))
    }
}
