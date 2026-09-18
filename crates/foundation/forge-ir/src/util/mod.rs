//! 支撑类型：与 IR 语义无关的基础设施（字符串、任意精度整数）。
//!
//! 目录归类（2026-09-17）时从 `src/` 顶层移入——它们被核心与文本层共用，本身不构成
//! "IR 数据模型"的一部分。各类型的**公开路径**仍由 `lib.rs` 扁平重导出
//! （`forge_ir::ImmStr` / `InternedStr` / `StringPool` / `Big`）。

pub mod big;
pub mod imm_str;
pub mod string_pool;
