//! 常量池 — 存储任意精度的编译时常量。
//!
//! `ConstantPool` 提供整数和浮点常量的去重存储，通过索引引用。
//! 这允许 `Iconst`/`Fconst` 支持任意宽度的 `Big` 值。

use super::big::Big;
use std::collections::HashMap;

/// 常量池 — 存储 `Big` 值并通过索引引用。
///
/// 内部使用 `HashMap<String, u32>`（基于 Display 字符串）进行 O(1) 去重，
/// 同时维护 `Vec<Big>` 以保持索引稳定。Big 本身不实现 Hash，因此使用 Display
/// 字符串作为键（字符串比较提供与 Big 相同的等价语义）。
///
/// # Example
///
/// ```ignore
/// let mut pool = ConstantPool::new();
/// let idx = pool.insert(Big::from_i64(42));
/// assert_eq!(pool.get(idx), Some(&Big::from_i64(42)));
/// ```
#[derive(Clone, Debug, Default)]
pub struct ConstantPool {
    /// 存储的常量值（按索引顺序）。
    constants: Vec<Big>,
    /// Display 字符串 → 索引映射（O(1) 去重查找，Big 不实现 Hash）。
    index_map: HashMap<String, u32>,
}

impl ConstantPool {
    /// 创建一个新的空常量池。
    pub fn new() -> Self {
        Self {
            constants: Vec::new(),
            index_map: HashMap::new(),
        }
    }

    /// 插入一个常量值（O(1) 去重），返回其索引。
    ///
    /// 如果值已存在，返回已有索引；否则插入并返回新索引。
    pub fn insert(&mut self, value: Big) -> u32 {
        let key = format!("{}", value);
        if let Some(&idx) = self.index_map.get(&key) {
            return idx;
        }
        let index = self.constants.len() as u32;
        self.index_map.insert(key, index);
        self.constants.push(value);
        index
    }

    /// 按索引获取常量值。
    pub fn get(&self, index: u32) -> Option<&Big> {
        self.constants.get(index as usize)
    }

    /// 返回常量池中的常量数量。
    pub fn len(&self) -> usize {
        self.constants.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.constants.is_empty()
    }

    /// 迭代所有常量及其索引。
    pub fn iter(&self) -> impl Iterator<Item = (u32, &Big)> {
        self.constants
            .iter()
            .enumerate()
            .map(|(i, k)| (i as u32, k))
    }

    /// 获取所有常量的切片。
    pub fn constants(&self) -> &[Big] {
        &self.constants
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_dedup() {
        let mut pool = ConstantPool::new();
        let i1 = pool.insert(Big::from_i64(42));
        let i2 = pool.insert(Big::from_i64(42));
        assert_eq!(i1, i2, "same integer should be deduplicated");
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn test_insert_different() {
        let mut pool = ConstantPool::new();
        let i1 = pool.insert(Big::from_i64(1));
        let i2 = pool.insert(Big::from_i64(2));
        assert_ne!(i1, i2);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn test_float_dedup() {
        let mut pool = ConstantPool::new();
        let f1 = Big::from_f64(std::f64::consts::PI).expect("valid float");
        let f2 = Big::from_f64(std::f64::consts::PI).expect("valid float");
        let i1 = pool.insert(f1);
        let i2 = pool.insert(f2);
        assert_eq!(i1, i2, "same float should be deduplicated");
        assert_eq!(pool.len(), 1);
        // Verify retrieval
        let val = pool.get(i1).expect("should exist");
        assert!((val.to_f64() - std::f64::consts::PI).abs() < 0.001);
    }

    #[test]
    fn test_float_distinct() {
        let mut pool = ConstantPool::new();
        let i1 = pool.insert(Big::from_f64(1.0).unwrap());
        let i2 = pool.insert(Big::from_f64(2.0).unwrap());
        assert_ne!(i1, i2, "different floats should get different indices");
        assert_eq!(pool.len(), 2);
    }
}
