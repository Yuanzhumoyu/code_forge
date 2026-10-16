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
    /// 二进制解码失败(偏移 + 说明;魔数/版本/段表/截断/未知 tag 一律走这里,
    /// **绝不 panic**)。
    BinaryDecode {
        /// 出错处相对字节流起点的偏移。
        offset: usize,
        /// 人类可读说明。
        msg: String,
    },
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
            IrError::Emit(m) => write!(f, "Emit error: {m}"),
            IrError::BackendNotFound(m) => write!(f, "Backend not found: {m}"),
            IrError::RegAlloc(m) => write!(f, "Register allocation failed: {m}"),
            IrError::DuplicateSymbol(m) => write!(f, "Duplicate symbol: {m}"),
            IrError::UnknownOpcode(m) => write!(f, "Unknown opcode: {m}"),
            IrError::UnknownIntCc(m) => write!(f, "Unknown integer calling convention: {m}"),
            IrError::UnknownFloatCc(m) => write!(f, "Unknown float calling convention: {m}"),
            IrError::DivisionByZero => write!(f, "Division by zero"),
            IrError::BinaryDecode { offset, msg } => {
                write!(f, "Binary decode error at offset {offset}: {msg}")
            }
            IrError::Internal(m) => write!(f, "Internal error: {m}"),
        }
    }
}

impl std::error::Error for IrError {}

impl IrError {
    /// 构造一条**能力缺口**错误（v25 M2 起住在 `forge-ir`：`forge-isa-runtime` 的 trait 默认
    /// 实现也要能构造缺口错误）。
    ///
    /// 消息以 **`[cap:ID]`** 开头 ⇒ 用户/CI 能按前缀搜索归类；`id` 与
    /// `docs/reference/backend-gaps.md` 台账的 ID 列一致，且**每个 ID 在源码里只出现一次**
    /// （守卫 `capability_gaps_guard` 钉住）。
    ///
    /// **什么不算缺口**（混进来会让台账失去意义）：畸形 IR / 内部不变量（用 `IrError::Internal`）、
    /// 使用错误（约定未注册、装载 ISA 不匹配 ⇒ `IrError::Semantic`）、
    /// 约定/形状数据自相矛盾（`IrError::Semantic`，报错已点名"数据有问题"）。
    pub fn capability(id: &str, msg: impl std::fmt::Display) -> Self {
        Self::Unsupported(format!("[cap:{id}] {msg}"))
    }

    /// 从一条错误里取出**缺口 ID**（`[cap:ID]`）；不是缺口错误 ⇒ `None`。
    ///
    /// 为什么要访问器而不是让调用方自己解析字符串：`IrError::Unsupported` 的 `Display`
    /// 会在前面加 `Unsupported: `，位置会变；把"怎么从错误里认出缺口"收在一处，
    /// 上层（测试、CI、诊断工具）就不用各自写正则。
    pub fn gap_id(&self) -> Option<String> {
        let s = format!("{self}");
        let start = s.find("[cap:")? + "[cap:".len();
        let rest = &s[start..];
        let end = rest.find(']')?;
        Some(rest[..end].to_string())
    }
}

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

#[cfg(test)]
mod capability_tests {
    use super::*;

    /// 缺口错误的消息里必须带 `[cap:ID]`（台账守卫靠这个前缀把集合抓出来），
    /// 且 `gap_id` 能原样取回 ID（不依赖 `Display` 的前缀位置）。
    ///
    /// v25 M2 从后端的旧缺口构造层迁来（构造器随之下沉到 `forge-ir`，
    /// 因为 `forge-isa-runtime` 的 trait 默认实现也要能构造缺口错误）。
    #[test]
    fn capability_errors_are_marked() {
        let e = IrError::capability("DEMO_GAP", "这台机器没有该能力");
        let s = format!("{e}");
        assert!(s.contains("[cap:DEMO_GAP]"), "实际：{s}");
        assert!(s.contains("这台机器没有该能力"));
        assert_eq!(e.gap_id().as_deref(), Some("DEMO_GAP"));
        // 非缺口错误 ⇒ `None`（不能把普通 Unsupported 也认成缺口）。
        assert_eq!(IrError::Unsupported("普通失败".into()).gap_id(), None);
        // 分类学：畸形 IR / 数据错误各有自己的变体，不该伪装成缺口。
        assert_eq!(IrError::Internal("字段越界".into()).gap_id(), None);
        assert_eq!(IrError::Semantic("约定没注册".into()).gap_id(), None);
    }
}
