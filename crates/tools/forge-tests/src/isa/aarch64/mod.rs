//! aarch64 ISA 测试模块 — 按指令类型拆分。

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
// pub mod encode;
// /// 反汇编测试。
// pub mod disasm;

#[cfg(test)]
crate::coverage!(
    "aarch64",
    code_forge::backend::aarch64::TargetMachine,
    code_forge::backend::aarch64::ensure_registered,
    crate::coverage::build_min,
    crate::coverage::COVERAGE_OPS
);
