//! 文本层：LLVM 文本 IR 的**解析器**与**打印机**（v3 S7 起由 `features = ["text"]` 门控）。
//!
//! 目录归类：原 `src/ir_parser/` 与 `src/display.rs` 迁到本目录，成为 crate 里**唯一**
//! 的门控点（`lib.rs` 只有 `#[cfg(feature = "text")] pub mod text;` 一行）。
//!
//! - [`parser`]：logos 词法 + lalrpop 语法（build.rs 生成）+ 语义构建；
//! - [`display`]：`impl Display for Module` 等打印路径；
//! - 常用入口在本模块重导出：[`parse_module`]、[`parse_function`]、[`function_to_string`]。
//!
//! 关闭 `text` 后核心 IR（类型/函数/DFG/verifier/pass/**二进制序列化**）照常编译，
//! 只少掉"文本 ⇄ IR"的两个端口；边界由 `tests/text_feature_gate.rs` 钉住。

pub mod display;
pub mod parser;

pub use display::function_to_string;
pub use parser::{parse_function, parse_module};
