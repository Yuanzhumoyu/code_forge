//! 诊断设施（P2.3 / P2.4）：panic 现场 dump + `FORGE_TRACE_*` 环境变量收拢。
//!
//! # panic 现场 dump
//! `install_panic_hook()`（幂等，backend.rs 入口调用）在 panic 时输出当前
//! 编译上下文（正在 lowering 的实例）+ 排查提示（可用哪些 trace env）。
//! rustc 自己的 panic hook（ICE 报告/abort）会被链式保留。
//!
//! # 诊断 env 收拢
//! 所有 `FORGE_TRACE_*` 读取统一走 [`trace_enabled`]（内部转发到运行时的唯一台账
//! `forge_isa_runtime::diagnostics::KNOBS`）。**名字清单不在这里重复一份**——它住在
//! `docs/reference/backend-pipeline.md` 的「观测开关」表，与台账、源码调用点由守卫
//! `trace_knobs_are_bijective` 三方钉住；这里再抄一遍必然漂移（v26 M1 之前就是这样）。

use std::cell::RefCell;

thread_local! {
    /// 当前正在编译的实例上下文（set_panic_context 维护）。
    static PANIC_CTX: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// 设置/清除 panic 上下文（进入单个实例编译时设置，完成后清除）。
pub(crate) fn set_panic_context(ctx: Option<String>) {
    PANIC_CTX.with(|c| *c.borrow_mut() = ctx);
}

/// 注册 panic hook（幂等）。panic 时先打印 forge-rustc 侧上下文，
/// 再调用 rustc 原有的 hook（ICE 报告）。
pub(crate) fn install_panic_hook() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let ctx = PANIC_CTX.with(|c| c.borrow().clone());
            if let Some(ctx) = ctx {
                eprintln!("[forge] panic during codegen, context: {ctx}");
            }
            eprintln!(
                "[forge] re-run with FORGE_TRACE_FN=1 (and/or FORGE_TRACE_STMT/SLOT/PAIR/ENUM/FIELD/GLOBAL/SYM=1) for a lowering trace"
            );
            prev(info);
        }));
    });
}

/// 查询 `FORGE_TRACE_<name>` 是否开启（诊断输出开关）。
///
/// v26 M1：转发到 `forge_isa_runtime::diagnostics::trace`——**开关台账只有一张**
/// （后端 11 项 + 直读项 + 本 crate 27 项），本 crate 不再自带名字集合。未登记的名字
/// debug 下当场 panic、release 下警告一次，绝不静默（旧的白名单曾让 `FORGE_TRACE_AGG`
/// 静默失效数月）。
pub(crate) fn trace_enabled(name: &str) -> bool {
    code_forge::forge_isa_runtime::diagnostics::trace(name)
}
