//! 执行引擎模块。
//!
//! - `harness`：统一测试 harness（build → compile → exec 一条龙）
//! - `executor`：统一的 `Executor` trait（本机 x86_64 / QEMU riscv64）
//! - `qemu`：QEMU riscv64 system-mode 执行（ELF 打包 + semihosting）

pub mod executor;
pub mod harness;
pub mod qemu;

/// 性质/模糊测试（确定性、算术恒等式、边界值；本机 x86_64 JIT）。
pub mod fuzz;
