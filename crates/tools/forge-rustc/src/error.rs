//! forge-rustc 本地错误类型（P2.2）。
//!
//! 不直接暴露 forge-ir 的 `IrError` 作为公共错误面：统一包装为
//! `ForgeError`，携带 rustc 侧上下文（ISA 名称 / 说明），供 `tcx.dcx()`
//! 报告与 e2e 断言。`From<IrError>` 使内部 `?` 自动转换。

use crate::prelude::*;

/// forge-rustc 降级/编译期的统一错误。
#[derive(Debug)]
pub(crate) enum ForgeError {
    /// 后端编译失败（forge-ir / forge-codegen 内部错误）。
    Backend(IrError),
    /// ISA 后端未注册（`register_backend!` 未调用或名称不匹配）。
    BackendNotFound(String),
    /// 其它降级期错误（携带上下文说明）。
    Message(String),
}

impl From<IrError> for ForgeError {
    fn from(e: IrError) -> Self {
        ForgeError::Backend(e)
    }
}

impl std::fmt::Display for ForgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForgeError::Backend(e) => write!(f, "backend: {e}"),
            ForgeError::BackendNotFound(name) => {
                write!(f, "ISA backend '{name}' not registered")
            }
            ForgeError::Message(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ForgeError {}
