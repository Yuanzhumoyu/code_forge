//! 常量池 — 存储编译时常量值，通过 `ConstId` 索引引用。
//!
//! 支持三类常量:
//! - 整数常量 (i128 + 位宽，自动去重)
//! - 浮点常量 (IEEE 754 bits，自动去重)
//! - 任意精度常量 (Big 值，基于哈希去重)

use super::entity::ConstId;
use crate::big::Big;
use std::collections::HashMap;

// ============================================================
// ConstantPool
// ============================================================

#[derive(Clone, Debug, Default)]
pub struct ConstantPool {
    /// 整数常量: 值 + 位宽
    int_consts: Vec<(i128, u16)>,
    int_dedup: HashMap<(i128, u16), ConstId>,

    /// 浮点常量: IEEE 754 bits
    float_consts: Vec<u64>,
    float_dedup: HashMap<u64, ConstId>,

    /// 任意精度常量 (超大整数、非标准浮点等)
    big_consts: Vec<Big>,
    big_dedup: HashMap<BigHashKey, ConstId>,
}

impl ConstantPool {
    pub fn new() -> Self {
        Self::default()
    }

    // ============================================================
    // 插入
    // ============================================================

    /// 插入整数常量 (自动去重)。
    pub fn insert_int(&mut self, value: i128, bits: u16) -> ConstId {
        let key = (value, bits);
        if let Some(&id) = self.int_dedup.get(&key) {
            return id;
        }
        let id = ConstId::pack(ConstId::TAG_INT, self.int_consts.len() as u32);
        self.int_consts.push(key);
        self.int_dedup.insert(key, id);
        id
    }

    /// 插入浮点常量 (IEEE 754 bits，自动去重)。
    pub fn insert_float(&mut self, bits: u64) -> ConstId {
        if let Some(&id) = self.float_dedup.get(&bits) {
            return id;
        }
        let id = ConstId::pack(ConstId::TAG_FLOAT, self.float_consts.len() as u32);
        self.float_consts.push(bits);
        self.float_dedup.insert(bits, id);
        id
    }

    /// 插入任意精度常量 (自动去重)。
    pub fn insert_big(&mut self, value: Big) -> ConstId {
        let key = BigHashKey(value.clone());
        if let Some(&id) = self.big_dedup.get(&key) {
            return id;
        }
        let id = ConstId::pack(ConstId::TAG_BIG, self.big_consts.len() as u32);
        self.big_consts.push(value);
        self.big_dedup.insert(key, id);
        id
    }

    // ============================================================
    // 查询
    // ============================================================

    /// 获取整数常量: (value, bits)。若 ConstId 不是整数类型则返回 None。
    pub fn get_int(&self, id: ConstId) -> Option<(i128, u16)> {
        if id.tag() != ConstId::TAG_INT {
            return None;
        }
        let idx = id.index() as usize;
        self.int_consts.get(idx).copied()
    }

    /// 获取浮点常量: IEEE 754 bits。若 ConstId 不是浮点类型则返回 None。
    pub fn get_float(&self, id: ConstId) -> Option<u64> {
        if id.tag() != ConstId::TAG_FLOAT {
            return None;
        }
        let idx = id.index() as usize;
        self.float_consts.get(idx).copied()
    }

    /// 获取任意精度常量。若 ConstId 不是 Big 类型则返回 None。
    pub fn get_big(&self, id: ConstId) -> Option<&Big> {
        if id.tag() != ConstId::TAG_BIG {
            return None;
        }
        let idx = id.index() as usize;
        self.big_consts.get(idx)
    }

    /// 常量总数。
    pub fn total_len(&self) -> usize {
        self.int_consts.len() + self.float_consts.len() + self.big_consts.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.int_consts.is_empty() && self.float_consts.is_empty() && self.big_consts.is_empty()
    }

    /// 返回所有 Big 常量的切片 (兼容旧 API)。
    pub fn constants(&self) -> &[Big] {
        &self.big_consts
    }

    /// 常量总数。
    pub fn len(&self) -> usize {
        self.total_len()
    }

    /// 解析 ConstId 为 i64 值 (自动根据 tag 分发到 int/big 池).
    pub fn resolve_int(&self, id: ConstId) -> Option<i64> {
        match id.tag() {
            ConstId::TAG_INT => self.get_int(id).map(|(v, _)| v as i64),
            ConstId::TAG_BIG => self.get_big(id).and_then(|b| b.try_to_i64()),
            _ => None,
        }
    }

    /// 解析 ConstId 为 Big 值 (自动根据 tag 分发到 int/float/big 池).
    pub fn resolve_big(&self, id: ConstId) -> Option<Big> {
        match id.tag() {
            ConstId::TAG_INT => self.get_int(id).map(|(v, _)| Big::from_i64(v as i64)),
            ConstId::TAG_FLOAT => self
                .get_float(id)
                .and_then(|bits| Big::from_f64(f64::from_bits(bits))),
            ConstId::TAG_BIG => self.get_big(id).cloned(),
            _ => None,
        }
    }

    /// 解析 ConstId 为 f64 位模式 (自动根据 tag 分发到 float/big 池).
    pub fn resolve_float(&self, id: ConstId) -> Option<u64> {
        match id.tag() {
            ConstId::TAG_FLOAT => self.get_float(id),
            ConstId::TAG_BIG => self.get_big(id).map(|b| b.to_f64().to_bits()),
            _ => None,
        }
    }

    /// 迭代所有整数常量。
    pub fn iter_ints(&self) -> impl Iterator<Item = (ConstId, i128, u16)> + '_ {
        self.int_consts
            .iter()
            .enumerate()
            .map(|(i, &(v, bits))| (ConstId::pack(ConstId::TAG_INT, i as u32), v, bits))
    }

    /// 迭代所有浮点常量。
    pub fn iter_floats(&self) -> impl Iterator<Item = (ConstId, u64)> + '_ {
        self.float_consts
            .iter()
            .enumerate()
            .map(|(i, &bits)| (ConstId::pack(ConstId::TAG_FLOAT, i as u32), bits))
    }
}

// ============================================================
// BigHashKey — Big 的哈希包装
// ============================================================

/// Big 的哈希键 — dashu 的 Real 不实现 Hash，
/// 因此使用规范字符串表示计算哈希。
#[derive(Clone, Debug, PartialEq, Eq)]
struct BigHashKey(Big);

impl std::hash::Hash for BigHashKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match &self.0 {
            Big::Signed(i) => {
                state.write_u8(0);
                i.hash(state);
            }
            Big::Unsigned(u) => {
                state.write_u8(1);
                u.hash(state);
            }
            Big::Float(r) => {
                state.write_u8(2);
                // dashu Real 不实现 Hash，使用 Display 字符串作为哈希输入
                // 这是保守但正确的方式
                state.write(r.to_string().as_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_int_dedup() {
        let mut pool = ConstantPool::new();
        let a = pool.insert_int(42, 32);
        let b = pool.insert_int(42, 32);
        assert_eq!(a, b);
        assert_eq!(pool.get_int(a), Some((42, 32)));
    }

    #[test]
    fn test_int_different_bits() {
        let mut pool = ConstantPool::new();
        let a = pool.insert_int(42, 32);
        let b = pool.insert_int(42, 64);
        assert_ne!(a, b); // same value, different bit width
    }

    #[test]
    fn test_float_dedup() {
        let mut pool = ConstantPool::new();
        let bits = 1.5f64.to_bits();
        let a = pool.insert_float(bits);
        let b = pool.insert_float(bits);
        assert_eq!(a, b);
        assert_eq!(pool.get_float(a), Some(bits));
    }

    #[test]
    fn test_big_dedup() {
        let mut pool = ConstantPool::new();
        let a = pool.insert_big(Big::from_i64(42));
        let b = pool.insert_big(Big::from_i64(42));
        assert_eq!(a, b);
    }

    #[test]
    fn test_empty_pool() {
        let pool = ConstantPool::new();
        assert!(pool.is_empty());
        assert_eq!(pool.total_len(), 0);
    }
}
