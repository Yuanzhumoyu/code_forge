//! 执行引擎模块。
//!
//! - `harness`：统一测试 harness（build → compile → exec 一条龙）
//! - `executor`：统一的 `Executor` trait（本机 x86_64 + unicorn 跨架构）
//! - `unicorn`（exec-unicorn feature）：unicorn-engine 模拟执行 shim

pub mod executor;
pub mod harness;

/// 性质/模糊测试（确定性、算术恒等式、边界值；本机 x86_64 JIT）。
pub mod fuzz;

#[cfg(feature = "exec-unicorn")]
pub mod unicorn;
#[cfg(feature = "exec-unicorn")]
pub mod unicorn_ffi;
