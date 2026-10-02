//! AArch64 (arm64) — A64 定宽 encode/decode/asm 自包含模块。
//!
//! 编码定义见 `isa/arm64.toml`（严格按 ARM A-profile A64 官方位段，
//! golden 由本机 LLVM clang+objdump 校验）。验证见
//! `tests/arm64_tests.rs`。模块名 = 文件 stem（约定）。

forge_dsl::isa_from_file!("isa/arm64.toml");
pub use self::arm64::*;
