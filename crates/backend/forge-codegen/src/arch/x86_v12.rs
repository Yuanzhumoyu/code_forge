//! x86-64 v12 试点 — 迭代 3：变长语义键（ModRM/REX/opsize/SSE）。
//!
//! v12 唯一语法生成的变长 encode/decode/asm 自包含模块（仅依赖 std，
//! 未接入 TargetMachine——ABI/lowering 在迭代 5）。与 v11 `x86_64` 后端
//! 并存用于 golden 字节对比（见 `tests/x86_v12_tests.rs`）。
//!
//! 模块名 = 文件 stem（v12 约定）；不 glob 导出以免与 v11 x86_64 冲突。

forge_dsl::isa_v12_from_file!("isa/x86_v12.toml");
pub use self::x86_v12::*;
