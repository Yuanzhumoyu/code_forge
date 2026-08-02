//! x86_64 ISA 测试模块 — 按指令类型拆分。

#[cfg(feature = "test-control")]
pub mod control;
#[cfg(feature = "test-float")]
pub mod float;
#[cfg(feature = "test-int")]
pub mod int;
#[cfg(feature = "test-io")]
pub mod io;

/// 编码 golden 断言（encode_golden! 宏）+ 编码集成测试（迁移自根 tests/encoder_tests.rs）。
pub mod encode;

/// 反汇编测试（disasm! 宏 + 迁移自根 tests/disasm_tests.rs 的用例）。
pub mod disasm;

/// JIT 集成测试（从根 tests/jit_integration.rs 整体迁移）。
pub mod jit;

#[cfg(test)]
crate::coverage!(
    "x86_64",
    code_forge::backend::x86_64::TargetMachine,
    code_forge::backend::x86_64::ensure_registered,
    crate::coverage::build_min,
    crate::coverage::COVERAGE_OPS
);
