pub mod alloc_config;
pub mod agg_const;
pub mod agg_expand;
pub mod alloc_result;
pub mod compiler;
pub mod emission;
pub mod emit;
pub mod frame_layout;
pub mod liverange;
pub mod lowering;
pub mod regalloc_bt;
pub mod vcode;

use std::sync::OnceLock;

/// FORGE_TRACE_* 环境变量的静态缓存。
///
/// `std::env::var_os` 每次调用都会线性扫描整个环境块，而 codegen 管线在
/// 每指令热路径上会多次查询（regalloc/lowering/emission），基准显示 500
/// 条指令的函数编译中该开销可占 7-18%。用 `OnceLock` 缓存全部 trace 开关：
/// 语义不变（调试期环境变量不会在运行时改变），首次查询后零开销。
pub(crate) fn trace_enabled(key: &'static str) -> bool {
    const TRACE_KEYS: &[&str] = &[
        "FORGE_TRACE_VCODE",
        "FORGE_TRACE_EMIT",
        "FORGE_TRACE_ISEL",
        "FORGE_TRACE_STACK",
        "FORGE_TRACE_LOWER",
        "FORGE_TRACE_REGALLOC",
        "FORGE_TRACE_ALLOC",
        "FORGE_TRACE_EXPIRE",
        "FORGE_TRACE_SPILL",
    ];
    static CACHE: OnceLock<std::collections::HashMap<&'static str, bool>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            TRACE_KEYS
                .iter()
                .map(|k| (*k, std::env::var_os(k).is_some()))
                .collect()
        })
        .get(key)
        .copied()
        .unwrap_or(false)
}
