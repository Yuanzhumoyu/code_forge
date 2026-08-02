//! wasm32 ISA 测试模块 — compile-only（无本机执行）+ 编码 golden。

/// 编码 golden 断言 + 编码集成测试（从根 tests/wasm32_encoder_tests.rs 迁移）。
pub mod encode;

/// 反汇编测试（disasm! 宏 + 从根 tests/wasm32_disasm_tests.rs 迁移的用例）。
pub mod disasm;

#[cfg(test)]
crate::coverage!(
    "wasm32",
    code_forge::backend::wasm32::TargetMachine,
    code_forge::backend::wasm32::ensure_registered,
    crate::coverage::build_min,
    crate::coverage::COVERAGE_OPS
);
