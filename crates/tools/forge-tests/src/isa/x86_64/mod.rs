//! x86_64 ISA 测试模块 — 按指令类型拆分。

#[cfg(feature = "test-control")]
pub mod control;
#[cfg(feature = "test-float")]
pub mod float;
#[cfg(feature = "test-int")]
pub mod int;
#[cfg(feature = "test-io")]
pub mod io;

// 临时禁用：字段 Reg 化后的测试改造未完成
// /// 编码 golden 断言 + 编码集成测试。
pub mod encode;
// /// 反汇编测试。
pub mod disasm;

/// 多宽度寄存器类（[reg_classes.*] 暴露 + 分配配置）。
pub mod reg_info;

/// JIT 集成测试（从根 tests/jit_integration.rs 整体迁移）。
pub mod jit;
pub mod text_to_exec;

#[cfg(test)]
crate::coverage!(
    "x86_64",
    code_forge::backend::x86_64::TargetMachine,
    code_forge::backend::x86_64::ensure_registered,
    crate::coverage::build_min,
    crate::coverage::COVERAGE_OPS
);
