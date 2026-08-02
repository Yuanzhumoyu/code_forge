//! aarch64 ISA 测试模块 — 按指令类型拆分。

#[cfg(feature = "test-control")]
pub mod control;
#[cfg(feature = "test-float")]
pub mod float;
#[cfg(feature = "test-int")]
pub mod int;
#[cfg(feature = "test-io")]
pub mod io;

/// 编码 golden 断言 + 编码集成测试（encode_golden! 宏 + 迁移自根 tests/aarch64_encoder_tests.rs）。
pub mod encode;

/// 反汇编测试（disasm! 宏 + 迁移自根 tests/aarch64_disasm_tests.rs 的用例）。
pub mod disasm;

#[cfg(test)]
crate::coverage!(
    "aarch64",
    code_forge::backend::aarch64::TargetMachine,
    code_forge::backend::aarch64::ensure_registered,
    crate::coverage::build_min,
    crate::coverage::COVERAGE_OPS
);
