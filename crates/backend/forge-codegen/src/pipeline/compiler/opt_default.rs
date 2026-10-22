//! 进程级默认优化级别（v27 M5 接线）。
//!
//! **背景**：`CompileSession::with_opt_level` 此前只有测试在调用，生产路径上 `opt_level`
//! 恒为 `None` ⇒ `forge-opt` 一次都没跑过（每个 MIR 局部都留在栈槽、`-C opt-level` 几乎无效）。
//!
//! **为什么要一个进程级量**：管线对象由 `forge_isa_runtime` 的**注册表工厂**创建
//! （`pipeline_hooks::Hook` → `CompileSession::new`），工厂签名只拿得到机器引用；
//! 而"这一轮编译该用哪档优化"是**进程/编译会话**级别的事实——由后端在 `codegen_crate`
//! 起点设一次即可，并发编译的多 CGU 共享同一档，符合 rustc 语义。
//!
//! **默认为"不优化"**：一旦让 verifier 真跑起来会暴露若干既有非法 IR（见方案账本
//! 「从未启用 IR 优化管线」一节），清零之前不改默认；用 `FORGE_OPT_LEVEL=0..3` 手动开启。

use std::sync::atomic::{AtomicU8, Ordering};

use forge_opt::OptimizationLevel;

/// `0` = 不优化，`1..=4` = `O0..=O3`。
static DEFAULT_OPT: AtomicU8 = AtomicU8::new(0);

/// 设置进程级默认优化级别（`None` = 不跑优化管线）。
pub fn set_default_opt_level(level: Option<OptimizationLevel>) {
    let code = match level {
        None => 0,
        Some(OptimizationLevel::O0) => 1,
        Some(OptimizationLevel::O1) => 2,
        Some(OptimizationLevel::O2) => 3,
        Some(OptimizationLevel::O3) => 4,
    };
    DEFAULT_OPT.store(code, Ordering::Relaxed);
}

/// 读取进程级默认优化级别。
pub fn default_opt_level() -> Option<OptimizationLevel> {
    match DEFAULT_OPT.load(Ordering::Relaxed) {
        0 => None,
        1 => Some(OptimizationLevel::O0),
        2 => Some(OptimizationLevel::O1),
        3 => Some(OptimizationLevel::O2),
        _ => Some(OptimizationLevel::O3),
    }
}
