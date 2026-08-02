//! TargetAssembler — 汇编文本 → 指令接口。
//!
//! 可选组件：ISA 支持从汇编文本解析为机器指令时实现。

/// 汇编错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum AsmError {
    #[error("parse error: {0}")]
    ParseError(String),
    #[error("undefined label: {0}")]
    UndefinedLabel(String),
    #[error("{0}")]
    Other(String),
}

/// 汇编器。
pub trait TargetAssembler: Send + Sync + 'static {
    type Inst: super::inst::MachineInst;

    /// 解析 ASM 源码 → 指令列表（含标签解析）。
    fn parse_insts(&self, source: &str) -> Result<Vec<Self::Inst>, AsmError>;
}
