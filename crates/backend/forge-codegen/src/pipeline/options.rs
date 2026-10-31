//! 编译选项（v30 M1）。
//!
//! 取代散落的 `with_opt_level` / 进程级 static / 环境变量三套：宿主构造一份
//! [`CompileOptions`]，交给 `CompileSession::with_options` 即可。
//!
//! **默认值仍从进程级量读**（[`default_opt_level`]）：管线对象由
//! `forge_isa_runtime` 的注册表工厂创建，工厂签名只拿得到机器引用；"这一轮
//! 编译该用哪档优化"是进程/编译会话级别的事实，由后端在 `codegen_crate`
//! 起点设一次。显式选项（[`CompileOptions::off`] / [`CompileOptions::speed`]）
//! 优先于进程级默认——测试与金标必须显式给，避免互相污染。

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

/// 整模块编译的并行策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parallelism {
    /// 函数数 ≥ 8 时走 scoped 并行（历史行为，默认）。
    Auto,
    /// 永不并行。
    Serial,
    /// 函数数 ≥ `n` 时并行（`n <= 1` 等价于 [`Parallelism::Serial`]）。
    Threads(usize),
}

impl Parallelism {
    /// worker 数（`1` = 串行）。
    ///
    /// `Auto` 保持历史行为：函数数 ≥ 8 时每函数一个 worker（= 函数数）；
    /// `Threads(n)` 起 `min(n, 函数数)` 个 worker 从共享下标取活。
    pub(crate) const fn worker_count(self, functions: usize) -> usize {
        match self {
            Parallelism::Auto => {
                if functions >= 8 {
                    functions
                } else {
                    1
                }
            }
            Parallelism::Serial => 1,
            Parallelism::Threads(n) => {
                if n <= 1 {
                    1
                } else if n < functions {
                    n
                } else {
                    functions
                }
            }
        }
    }
}

/// 一份编译选项（构造后不可变）。
///
/// `#[non_exhaustive]`：只允许经构造器/`with_*` 生成，后续加字段不破坏调用点。
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct CompileOptions {
    opt_level: Option<OptimizationLevel>,
    parallel: Parallelism,
}

impl CompileOptions {
    /// 进程级默认（[`default_opt_level`]）+ 自动并行。
    pub fn new() -> Self {
        Self {
            opt_level: default_opt_level(),
            parallel: Parallelism::Auto,
        }
    }

    /// **明确不跑 IR 优化管线**（金标/单测用：不受进程级默认影响）。
    pub fn off() -> Self {
        Self {
            opt_level: None,
            parallel: Parallelism::Auto,
        }
    }

    /// 指定优化级别（跑对应级别的 `forge-opt` 管线）。
    pub fn speed(level: OptimizationLevel) -> Self {
        Self {
            opt_level: Some(level),
            parallel: Parallelism::Auto,
        }
    }

    /// 覆盖优化级别。
    pub fn with_opt_level(mut self, level: OptimizationLevel) -> Self {
        self.opt_level = Some(level);
        self
    }

    /// 覆盖并行策略。
    pub fn with_parallel(mut self, parallel: Parallelism) -> Self {
        self.parallel = parallel;
        self
    }

    /// 当前优化级别（`None` = 不跑优化管线）。
    pub fn opt_level(&self) -> Option<OptimizationLevel> {
        self.opt_level
    }

    /// 当前并行策略。
    pub fn parallel(&self) -> Parallelism {
        self.parallel
    }
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self::new()
    }
}
