//! RISC-V64 v12 pilot — 迭代 2：v12 唯一语法生成的定宽 encode/decode/asm
//! 自包含模块（仅依赖 std，未接入 TargetMachine——ABI/lowering 在迭代 5）。
//!
//! 与 v11 `riscv64` 后端并存：本模块用于 golden 字节对比与全量往返验证
//! （见 `tests/riscv64_v12_tests.rs`）。模块名 = 文件 stem（v12 约定，
//! 非 meta.name；避免与 v11 模块双嵌套冲突）。

forge_dsl::isa_v12_from_file!("isa/riscv64_v12.toml");
pub use self::riscv64_v12::*;
