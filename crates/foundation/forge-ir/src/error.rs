//! forge-ir 错误类型——统一错误面。
//!
//! 文本层(parse/semantic)、验证、常量求值与后端边界错误共用该枚举;
//! Display 输出人类可读信息(前缀大写——部分测试断言依赖),Error 实现
//! 支持 `?` 传播。

/// forge-ir 统一错误。
#[derive(Debug, Clone)]
pub enum IrError {
    /// 文本层语法错误(lexer/parser 失败,含位置信息文本)。
    Parse(String),
    /// 语义错误(类型/值域/引用校验失败)。
    Semantic(String),
    /// 验证器错误(IR 结构不合法)。
    Verify(String),
    /// 常量求值/字节打包等未支持操作。
    Unsupported(String),
    /// 尚未实现的特性(编译期拒绝——按路线图推进)。
    Unimplemented(String),
    /// 发射/对象写入错误。
    Emit(String),
    /// 后端(ISA)未注册/未找到。
    BackendNotFound(String),
    /// 寄存器分配失败。
    RegAlloc(String),
    /// 符号重复定义。
    DuplicateSymbol(String),
    /// 未识别操作码。
    UnknownOpcode(String),
    /// 未知整数调用约定。
    UnknownIntCc(String),
    /// 未知浮点调用约定。
    UnknownFloatCc(String),
    /// 除零(常量求值)。
    DivisionByZero,
    /// 内部不变式违例(代码缺陷)。
    Internal(String),
}

impl std::fmt::Display for IrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IrError::Parse(m) => write!(f, "Parse error: {m}"),
            IrError::Semantic(m) => write!(f, "Semantic error: {m}"),
            IrError::Verify(m) => write!(f, "Verify error: {m}"),
            IrError::Unsupported(m) => write!(f, "Unsupported: {m}"),
            IrError::Unimplemented(m) => write!(f, "Unimplemented: {m}"),
            IrError::Emit(m) => write!(f, "Emit error: {m}"),
            IrError::BackendNotFound(m) => write!(f, "Backend not found: {m}"),
            IrError::RegAlloc(m) => write!(f, "Register allocation failed: {m}"),
            IrError::DuplicateSymbol(m) => write!(f, "Duplicate symbol: {m}"),
            IrError::UnknownOpcode(m) => write!(f, "Unknown opcode: {m}"),
            IrError::UnknownIntCc(m) => write!(f, "Unknown integer calling convention: {m}"),
            IrError::UnknownFloatCc(m) => write!(f, "Unknown float calling convention: {m}"),
            IrError::DivisionByZero => write!(f, "Division by zero"),
            IrError::Internal(m) => write!(f, "Internal error: {m}"),
        }
    }
}

impl std::error::Error for IrError {}

impl From<String> for IrError {
    fn from(s: String) -> Self {
        IrError::Semantic(s)
    }
}

impl From<&str> for IrError {
    fn from(s: &str) -> Self {
        IrError::Semantic(s.to_string())
    }
}
