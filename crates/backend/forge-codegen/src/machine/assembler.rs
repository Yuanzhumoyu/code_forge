//! TargetAssembler — 汇编文本 → 指令接口。
//!
//! DSL 生成的每 ISA `Assembler` 实现三个方法：
//! - `parse_lines`：lalrpop parser 把汇编文本解析为类型化中间表示（AsmLine）
//! - `bind`：按字段类型签名把中间表示绑定为 `Inst`（含语义层消歧、两遍标签解析）
//! - `parse_insts`（trait 默认实现）：parse_lines + bind

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
    fn parse_insts(&self, source: &str) -> Result<Vec<Self::Inst>, AsmError> {
        let lines = self.parse_lines(source)?;
        self.bind(lines)
    }

    /// 解析 ASM 源码 → 中间表示（保留操作数值，供调用方消费）。
    fn parse_lines(&self, _source: &str) -> Result<Vec<crate::assembler::AsmLine>, AsmError> {
        Err(AsmError::Other("parse_lines not implemented".into()))
    }

    /// 中间表示 → 指令（按字段类型签名绑定 + 标签回填）。
    fn bind(&self, _lines: Vec<crate::assembler::AsmLine>) -> Result<Vec<Self::Inst>, AsmError> {
        Err(AsmError::Other("bind not implemented".into()))
    }
}
