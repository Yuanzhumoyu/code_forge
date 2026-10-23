//! TargetAssembler — 汇编文本 → 指令接口。
//!
//! ISA-DSL 生成代码的每 ISA `Assembler` 直接实现 `parse_insts`（表驱动 asm
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

    /// 解析 ASM 源码，并允许**文本里出现、定义在文本之外**的标签（v27 M6-94）。
    ///
    /// `external` = `(名字, 值)`：rustc inline asm 的 `label` 操作数把**目标基本块**
    /// 当作标签用；值与"文本内标签回填后的值"同一空间（本仓 = IR 块下标），因此
    /// 汇编器只需在找不到文本标签时查这张表。默认实现：
    ///
    /// - 没有外部标签 ⇒ 退化为 [`Self::parse_insts`]（零行为变化）；
    /// - 有外部标签 ⇒ **明确拒绝**（不认识这个能力的 ISA 不该静默按别的语义解析）。
    fn parse_insts_with_labels(
        &self,
        source: &str,
        external: &[(&str, i64)],
    ) -> Result<Vec<Self::Inst>, AsmError> {
        if external.is_empty() {
            self.parse_insts(source)
        } else {
            Err(AsmError::Other(
                "本 ISA 的汇编器不支持外部标签（inline asm 的 label 操作数）".to_string(),
            ))
        }
    }
}
