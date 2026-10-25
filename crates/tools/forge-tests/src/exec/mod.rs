//! 执行引擎模块。
//!
//! - `harness`：统一测试 harness（build → compile → exec 一条龙）
//! - `executor`：统一的 `Executor` trait（本机 x86_64 / QEMU riscv64 / aarch64）
//! - `qemu`：QEMU riscv64 system-mode 执行（ELF 打包 + sifive_test）
//! - `qemu_aarch64`：QEMU aarch64 system-mode 执行（ELF 打包 + semihosting）

pub mod executor;
pub mod harness;
pub mod qemu;
pub mod qemu_aarch64;

/// 性质/模糊测试（确定性、算术恒等式、边界值；本机 x86_64 JIT）。
pub mod fuzz;
/// **QEMU 串行化锁**（v27 M6-107）：矩阵/单测在多线程下会同时起多个 QEMU 进程，
/// 实测偶发 `got 0x1`（QEMU 自己失败退出）导致假红。QEMU 是**外部共享资源**，
/// 测试内部串行化比压并发更稳（用例本身仍是真执行，只是不并行跑虚拟机）。
pub(crate) fn qemu_serialize() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
