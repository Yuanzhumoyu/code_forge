//! demo_v12 — 类型约束自动分发演示 ISA（定宽 32 位）。
//!
//! 证明 v12 新能力：同一助记符（`add`/`mov`）的 16/32/64 位寄存器版本由
//! 汇编器按**操作数实际类型**（寄存器类）自动分发到不同编码，无需手写
//! `mov16`/`mov32`/`mov64` 等拆分助记符。验证见
//! `tests/demo_v12_tests.rs`。

forge_dsl::isa_from_file!("isa/demo_v12.toml");
pub use self::demo_v12::*;
// touch-b3
// touch-b3b
// touch-d2
