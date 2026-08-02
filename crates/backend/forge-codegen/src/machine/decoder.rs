//! TargetDecoder — 字节 → 指令解码接口。
//!
//! 可选组件：ISA 支持从原始字节解码指令时实现。

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

/// 指令解码器。
pub trait TargetDecoder: Send + Sync + 'static {
    type Inst: super::inst::MachineInst;

    /// 解码：字节序列 → (指令, 消费的字节数)。
    fn decode(&self, bytes: &[u8]) -> Result<(Self::Inst, usize), DecodeError>;
}
