//! Assembler trait — 独立的汇编接口。
//!
//! 将 ASM 文本解析为指令列表，或直接汇编为可执行的 JIT 函数。

use crate::backend::instruction_set::InstructionSet;
use crate::backend::isa_info::IsaInfo;
use crate::jit::JitCompiler;

/// 汇编错误类型。
#[derive(Debug, thiserror::Error)]
pub enum AsmError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("unknown mnemonic: {0}")]
    UnknownMnemonic(String),
    #[error("unknown register: {0}")]
    UnknownRegister(String),
    #[error("invalid operand: {0}")]
    InvalidOperand(String),
    #[error("encode error: {0}")]
    Encode(#[from] crate::EncodeError),
    #[error("unresolved label: {0}")]
    UnresolvedLabel(String),
}

/// 汇编器 trait — ASM 文本 → 指令 / 可执行 JIT 函数。
///
/// 当 ISA 支持从汇编文本生成机器码和 JIT 函数时实现此 trait。
/// `Self::Inst` 由 super-trait `InstructionSet` 提供。
///
/// # Example
///
/// ```ignore
/// use codegen_lib::backend::Assembler;
/// use codegen_lib::backend::x86_64::X86Isa;
///
/// // 只解析指令
/// let insts = X86Isa::parse_insts("mov rax, 42\nret")?;
///
/// // 汇编 + 执行
/// let jit = X86Isa::assemble("answer", "mov rax, 42\nret")?;
/// let f: extern "C" fn() -> i64 = jit.get_fn("answer")?;
/// assert_eq!(f(), 42);
/// ```
pub trait Assembler: InstructionSet {
    /// 解析 ASM 源码 → 已构造的指令列表（含标签解析）。
    ///
    /// 此方法完成语法解析、助记符消歧、操作数绑定、标签解析，
    /// 返回可直接编码的 `Inst` 枚举值列表。
    fn parse_insts(
        source: &str,
    ) -> Result<Vec<<Self as InstructionSet>::Inst>, AsmError>;

    /// 汇编 ASM 源码 → JitCompiler，其中包含名为 `name` 的已编译函数。
    ///
    /// 内部调用 `parse_insts` → `Encoder::encode` → `JitCompiler::add_compiled`。
    /// 返回的 `JitCompiler` 可直接通过 `get_fn::<F>(name)` 获取类型安全的函数指针。
    fn assemble(
        name: &str,
        source: &str,
    ) -> Result<JitCompiler<Self>, AsmError>;
}
