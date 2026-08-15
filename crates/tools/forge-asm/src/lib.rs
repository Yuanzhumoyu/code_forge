//! Forge Assembler — 运行时汇编引擎共享类型。
//!
//! 提供 DSL 生成的每 ISA 汇编器（`TargetAssembler` 实现）所需的共享件：
//! - 词法基础设施：泛型 `TokenStream` / `LexError`（完整 Token 枚举由 forge-dsl
//!   按 ISA 生成并嵌入 ISA 模块，见 `lexer` 模块文档）
//! - 类型化操作数中间表示：`OperandValue` / `OperandTy` / `AsmLine` / `RawInst`
//! - re-export `logos` / `lalrpop_util`，使生成代码可通过 `crate::assembler`
//!   解析 derive 宏与 parser 运行库路径

/// Re-exported so lalrpop-generated parser code can resolve `lalrpop_util`
/// paths inside the DSL-generated ISA module (via `crate::assembler`).
pub use lalrpop_util;
/// Re-exported so DSL-generated parser code can resolve `logos` paths without
/// the consuming crate declaring a direct dependency (via `crate::assembler`).
pub use logos;

/// 词法基础设施：`LexError` + 泛型 `TokenStream`。
pub mod lexer;
pub use lexer::{LexError, TokenStream};

/// 类型化操作数中间表示。
pub mod value;
pub use value::{AsmLine, OperandTy, OperandValue, RawInst, imm_val, mem_base};
