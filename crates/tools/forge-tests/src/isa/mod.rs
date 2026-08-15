//! ISA 测试模块 — 按 ISA 开关（feature）门控。
//!
//! 每个 ISA 下按指令类型拆分模块（整数 / 浮点 / IO / 控制流），
//! 分别由 `test-int` / `test-float` / `test-io` / `test-control` 门控。

#[cfg(feature = "isa-aarch64")]
pub mod aarch64;
#[cfg(feature = "isa-riscv64")]
pub mod riscv64;
#[cfg(feature = "isa-x86_64")]
pub mod x86_64;


/// 跨架构执行测试（exec-unicorn：aarch64/riscv64 经 unicorn 模拟执行）。
#[cfg(feature = "exec-unicorn")]
pub mod cross_arch_exec;
