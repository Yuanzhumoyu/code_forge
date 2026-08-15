//! 词法基础设施（ISA 无关）——泛型 token 流。
//!
//! Token 枚举本身**不在这里**：因为每 ISA 的语法需要把 mnemonic
//! （"mov"/"sete"/"rol"…）作为独立 token 变体（lalrpop 自定义 token 模式
//! 下字符串 literal 无法直接用于 mnemonic 匹配），所以完整 Token 枚举由
//! forge-dsl 生成器（asm_grammar.rs）按 ISA 输出，`#[derive(::logos::Logos)]`
//! 嵌入 DSL 生成的 ISA 模块。
//!
//! 本模块只提供与 Token 无关的共享件：
//! - `LexError`：词法错误（lalrpop extern 块的 Error 类型）
//! - `TokenStream<'a, T>`：包装 `logos::Lexer`，产出 lalrpop 可消费的
//!   `Result<(usize, T, usize), LexError>` 迭代器（参照 forge-ir 的 TokenStream）

/// 词法错误（lalrpop extern 块的 Error 类型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError;

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unexpected character")
    }
}

impl std::error::Error for LexError {}

/// 带位置的 token 流（lalrpop 消费：`Result<(usize, T, usize), LexError>`）。
pub struct TokenStream<'a, T: logos::Logos<'a>> {
    lexer: logos::Lexer<'a, T>,
}

impl<'a, T> TokenStream<'a, T>
where
    T: logos::Logos<'a, Source = str>,
    T::Extras: Default,
{
    pub fn new(src: &'a str) -> Self {
        Self {
            lexer: T::lexer(src),
        }
    }
}

impl<'a, T> Iterator for TokenStream<'a, T>
where
    T: logos::Logos<'a, Source = str>,
    T::Extras: Default,
{
    type Item = Result<(usize, T, usize), LexError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.lexer.next() {
            Some(Ok(tok)) => {
                let span = self.lexer.span();
                Some(Ok((span.start, tok, span.end)))
            }
            Some(Err(_)) => Some(Err(LexError)),
            None => None,
        }
    }
}
