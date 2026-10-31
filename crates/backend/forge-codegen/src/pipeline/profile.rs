//! 单函数编译的命名计时（v30 M2）。
//!
//! 取代 `compile_stages` 里 `_t1.._t7` 的一串裸 `Instant`：标记名就是
//! `COMPILE-DONE` 事件的字段名，事件拼装与 `FORGE_TRACE_TIMING` 都从这里取。
//! 标记缺失 ⇒ 0（不会悄悄改变事件字段集合——字段表由守卫钉住）。

use std::time::{Duration, Instant};

/// 一次单函数编译的计时器。
pub(crate) struct Profile {
    start: Instant,
    last: Instant,
    marks: Vec<(&'static str, Duration)>,
}

impl Profile {
    /// 开始计时（在 `COMPILE-START` 之前调用）。
    pub(crate) fn start() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            last: now,
            marks: Vec::new(),
        }
    }

    /// 记录"自上次 `mark` 以来"的耗时。
    pub(crate) fn mark(&mut self, name: &'static str) {
        let now = Instant::now();
        self.marks.push((name, now.duration_since(self.last)));
        self.last = now;
    }

    /// 从 `start` 到现在的总耗时。
    pub(crate) fn total(&self) -> Duration {
        self.start.elapsed()
    }

    /// 取某个标记的耗时；没有该标记 ⇒ 0。
    pub(crate) fn get(&self, name: &str) -> Duration {
        self.marks
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, d)| *d)
            .unwrap_or_default()
    }
}
