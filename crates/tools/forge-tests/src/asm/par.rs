//! 语料扫描的**并行**工具。
//!
//! 语料有三百多份文件、主要开销是逐行喂汇编器，串行跑太慢（2026-10-07 用户反馈）。
//! 这里只用 `std::thread::scope` + 原子取号，**不引入 rayon**；结果按**入参序**返回，
//! 因此摘要/棘轮/记分板的输出与串行时逐字节一致（确定性不受并行影响）。

/// 并行映射：`f` 在多个工作线程上跑，返回与 `items` **同序**的结果。
///
/// 线程数 = `available_parallelism()`（夹在 `1..=items.len()`）；`items` 为空或只有
/// 一个元素时直接串行，避免起线程的开销。
pub fn map_parallel<T, R>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R>
where
    T: Sync,
    R: Send,
{
    let n = items.len();
    if n == 0 {
        return Vec::new();
    }
    let workers = std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(4)
        .clamp(1, n);
    if workers <= 1 {
        return items.iter().map(&f).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut got: Vec<(usize, R)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let next = &next;
                let f = &f;
                scope.spawn(move || {
                    let mut local: Vec<(usize, R)> = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        local.push((i, f(&items[i])));
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    got.sort_by_key(|(i, _)| *i);
    got.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_input_order_and_values() {
        let items: Vec<u32> = (0..97).collect();
        let out = map_parallel(&items, |x| x * x + 1);
        assert_eq!(out.len(), items.len());
        for (i, v) in out.iter().enumerate() {
            assert_eq!(*v, (i as u32) * (i as u32) + 1, "第 {i} 项顺序/值不符");
        }
    }

    #[test]
    fn empty_and_single_are_fine() {
        let empty: Vec<u8> = Vec::new();
        assert!(map_parallel(&empty, |x| *x).is_empty());
        assert_eq!(map_parallel(&[7u8], |x| *x + 1), vec![8]);
    }
}