//! TargetAssembler — 汇编文本 → 指令接口。
//!
//! v12 生成代码的每 ISA `Assembler` 直接实现 `parse_insts`（表驱动 asm
//! 模板，无 v11 的 lalrpop parser / AsmLine 中间表示层）。

/// 汇编错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum AsmError {
    #[error("parse error: {0}")]
    ParseError(String),
    #[error("undefined label: {0}")]
    UndefinedLabel(String),
    #[error("ambiguous instruction: {0}")]
    Ambiguous(String),
    #[error("type mismatch: {mnemonic} operand {index} expected {expected}, got {got}")]
    TypeMismatch {
        mnemonic: String,
        index: usize,
        expected: String,
        got: String,
    },
    #[error("{0}")]
    Other(String),
}

/// 汇编器。
pub trait TargetAssembler: Send + Sync + 'static {
    type Inst: super::inst::MachineInst;

    /// 解析 ASM 源码 → 指令列表（含标签解析）。
    fn parse_insts(&self, source: &str) -> Result<Vec<Self::Inst>, AsmError>;
}
