//! 诊断设施（P2.3 / P2.4）：panic 现场 dump + `FORGE_TRACE_*` 环境变量收拢。
//!
//! # panic 现场 dump
//! `install_panic_hook()`（幂等，backend.rs 入口调用）在 panic 时输出当前
//! 编译上下文（正在 lowering 的实例）+ 排查提示（可用哪些 trace env）。
//! rustc 自己的 panic hook（ICE 报告/abort）会被链式保留。
//!
//! # 诊断 env 收拢
//! 所有 `FORGE_TRACE_*` 读取统一走 [`trace_enabled`]，取值集中在本文件：
//!
//! | env | 输出内容 |
//! |---|---|
//! | `FORGE_TRACE_FN` | 每个函数的 lowering 开始 |
//! | `FORGE_TRACE_STMT` | 每条 MIR statement |
//! | `FORGE_TRACE_SLOT` | 栈槽分配 |
//! | `FORGE_TRACE_PAIR` | ScalarPair 判定与偏移 |
//! | `FORGE_TRACE_ENUM` | enum 布局细节 |
//! | `FORGE_TRACE_FIELD` | 字段偏移计算 |
//! | `FORGE_TRACE_GLOBAL` | 全局/重定位符号解析 |
//! | `FORGE_TRACE_SYM` | 符号名计算 |
//! | `FORGE_TRACE_CALL` | 直接调用（callee 签名 / 实参打包明细） |
//! | `FORGE_TRACE_SUBSTS` | 泛型替换解析（FnDef → instance） |
//! | `FORGE_TRACE_ABI` | rustc FnAbi 参数/返回形态（P4.1 一致性校验用） |
//! | `FORGE_TRACE_LIVE` | 活区间 dump（forge-codegen liverange，调试 regalloc 用） |
//! | `FORGE_TRACE_LOWER/ALLOC/SPILL/EXPIRE`（主库 forge-codegen） | 主库 lowering 逐指令 / regalloc 分配 / spill-reload / expire 诊断（`pipeline::trace_enabled`） |

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
pub(crate) fn trace_enabled(name: &str) -> bool {
    std::env::var_os(format!("FORGE_TRACE_{name}")).is_some()
}
