//! 常量池 — 存储编译时常量值，通过 `ConstId` 索引引用。
//!
//! 支持五类常量:
//! - 整数常量 (i128 + 位宽，自动去重)
//! - 浮点常量 (IEEE 754 bits，自动去重)
//! - 任意精度常量 (Big 值，基于哈希去重)
//! - 向量常量 (扁平字节池 + 段端序，insert_vector / insert_vector_with_endian)
//! - 聚合常量（树形：标量/嵌套聚合，AggConst）

use crate::entity::TypeId;
use crate::entity::{AggId, ConstId, Endianness};
use crate::util::big::Big;
use std::collections::HashMap;

/// 聚合常量子节点：标量（ConstId）或嵌套聚合（AggId）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AggChild {
    Scalar(ConstId),
    Agg(AggId),
}

/// 聚合常量（树形）：类型 + 子节点（标量/嵌套聚合）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AggConst {
    pub ty: TypeId,
    pub children: Vec<AggChild>,
}

// ============================================================
// ConstantPool
// ============================================================

#[derive(Clone, Debug)]
pub struct ConstantPool {
    /// 整数常量: 值 + 位宽
    int_consts: Vec<(i128, u32)>,
    int_dedup: HashMap<(i128, u32), ConstId>,

    /// 浮点常量: (IEEE 754 bits, **值宽 bits** = 16/32/64/128)。
    /// 位宽必须进池：历史的 `Vec<u128>` 只有位模式，f32 `1.5`（0x3FC0_0000）
    /// 会与位模式相同的 f64（约 1.9e-314，反规格化）**去重成同一个 ConstId**，
    /// 打印期只能靠结果类型猜宽度——phi 入边那条路径猜错，输出错误的十进制值
    /// （2026-09-14 审计发现）。
    float_consts: Vec<(u128, u16)>,
    float_dedup: HashMap<(u128, u16), ConstId>,

    /// 任意精度常量 (超大整数、非标准浮点等)
    big_consts: Vec<Big>,
    big_dedup: HashMap<BigHashKey, ConstId>,

    /// 向量常量池：扁平字节存储（各常量段连续排列，缓存友好）。
    /// 段 i 范围 = vec_offsets[i] .. vec_offsets[i+1]（末段到 vec_data.len()）；
    /// 段长可由向量类型（vector_len × elem_size）验证，不冗余存储。
    /// 每段端序记录于 vec_endian[i]（Little/Big），DSL 按端序还原字节。
    vec_data: Vec<u8>,
    vec_offsets: Vec<u32>,
    vec_endian: Vec<Endianness>,
    vec_dedup: HashMap<Vec<u8>, ConstId>,

    /// 聚合常量池（树形：标量/嵌套聚合；3.1 聚合常量 Value）。
    aggregates: Vec<AggConst>,
    agg_dedup: HashMap<(TypeId, Vec<AggChild>), AggId>,
}

impl ConstantPool {
    /// 新建常量池（预置 bool 槽：index 0 = false、index 1 = true）。
    pub fn new() -> Self {
        let mut pool = Self {
            int_consts: Vec::new(),
            int_dedup: HashMap::new(),
            float_consts: Vec::new(),
            float_dedup: HashMap::new(),
            big_consts: Vec::new(),
            big_dedup: HashMap::new(),
            vec_data: Vec::new(),
            vec_offsets: Vec::new(),
            vec_endian: Vec::new(),
            vec_dedup: HashMap::new(),
            aggregates: Vec::new(),
            agg_dedup: HashMap::new(),
        };
        // 预置 bool 常量槽：index 0 = false (0,1)，index 1 = true (1,1)。
        // bool 只有 0/1 两个值，固定占位避免 iconst_bool 反复插入膨胀常量池；
        // 后续 insert_int(0,1)/insert_int(1,1) 经去重命中这两个槽位。
        pool.insert_int(0, 1);
        pool.insert_int(1, 1);
        pool
    }

    /// 获取预置的 bool 常量槽位（false → index 0，true → index 1）。
    ///
    /// `ConstantPool::new()` 已预置 (0,1) 与 (1,1)，此处直接构造 `ConstId`，
    /// 不产生新的池条目。后端按 `ConstId(index)` 解析常量值（forge-dsl 的
    /// `iconst` 虚拟操作数），因此布尔常量始终复用固定槽位。
    pub fn bool_const(&self, value: bool) -> ConstId {
        ConstId::pack(ConstId::TAG_INT, if value { 1 } else { 0 })
    }

    // ============================================================
    // 插入
    // ============================================================

    /// 插入整数常量 (自动去重)。
    pub fn insert_int(&mut self, value: i128, bits: u32) -> ConstId {
        let key = (value, bits);
        if let Some(&id) = self.int_dedup.get(&key) {
            return id;
        }
        let id = ConstId::pack(ConstId::TAG_INT, self.int_consts.len() as u32);
        self.int_consts.push(key);
        self.int_dedup.insert(key, id);
        id
    }

    /// 插入浮点常量 (IEEE 754 bits，自动去重)。f64/f32 位模式经 u64 传入。
    pub fn insert_float(&mut self, bits: u64) -> ConstId {
        self.insert_float_typed(bits as u128, 64)
    }

    /// 插入浮点常量并**显式记录值宽**（bits：16/32/64/128）。
    ///
    /// 位宽进去重键 → 同一位模式的不同精度不再合并（f32 0x3FC0_0000 ≠ f64 同模式）。
    /// `bits` 必须能装进 `width`（越界在 debug 构建下断言失败，release 保留完整
    /// 位模式——不静默截断；宽度是元数据，不是掩码）。
    pub fn insert_float_typed(&mut self, bits: u128, width: u16) -> ConstId {
        debug_assert!(
            (1..=128).contains(&width),
            "浮点值宽必须是 1..=128 位，得到 {width}"
        );
        debug_assert!(
            width == 128 || bits >> width == 0,
            "浮点位模式 {bits:#x} 装不进 {width} 位（调用方传错位宽）"
        );
        let key = (bits, width);
        if let Some(&id) = self.float_dedup.get(&key) {
            return id;
        }
        let id = ConstId::pack(ConstId::TAG_FLOAT, self.float_consts.len() as u32);
        self.float_consts.push(key);
        self.float_dedup.insert(key, id);
        id
    }

    /// 插入聚合常量（树形，自动去重）；返回 AggId。
    pub fn insert_aggregate(&mut self, ty: TypeId, children: Vec<AggChild>) -> AggId {
        let key = (ty, children.clone());
        if let Some(&id) = self.agg_dedup.get(&key) {
            return id;
        }
        let id = AggId(self.aggregates.len() as u32);
        self.aggregates.push(AggConst { ty, children });
        self.agg_dedup.insert(key, id);
        id
    }

    /// 查询聚合常量。
    pub fn get_aggregate(&self, id: AggId) -> Option<&AggConst> {
        self.aggregates.get(id.0 as usize)
    }

    /// 插入 f128 浮点常量（128 位 IEEE 754 bits，自动去重）。
    pub fn insert_float128(&mut self, bits: u128) -> ConstId {
        self.insert_float_typed(bits, 128)
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

    /// 插入向量常量（原始字节：元素按 Little 端序拼接，自动去重）。
    ///
    /// 元素位宽不限（u8..u128/f32/f64）——数据完整性与"还原"解耦：
    /// 还原由指令集/后端决定（如 DSL 按 64 位块 from_le_bytes）。
    pub fn insert_vector(&mut self, data: &[u8]) -> ConstId {
        self.insert_vector_with_endian(data, Endianness::Little)
    }

    /// 插入向量常量并记录端序（Little/Big）。去重 key 为字节本身——
    /// 同一字节序列不同端序在物理上不同，自然区分。
    pub fn insert_vector_with_endian(&mut self, data: &[u8], endian: Endianness) -> ConstId {
        if let Some(&id) = self.vec_dedup.get(data) {
            return id;
        }
        let id = ConstId::pack(ConstId::TAG_VEC, self.vec_offsets.len() as u32);
        self.vec_dedup.insert(data.to_vec(), id);
        self.vec_offsets.push(self.vec_data.len() as u32);
        self.vec_endian.push(endian);
        self.vec_data.extend_from_slice(data);
        id
    }

    // ============================================================
    // 查询
    // ============================================================

    /// 获取整数常量: (value, bits)。若 ConstId 不是整数类型则返回 None。
    pub fn get_int(&self, id: ConstId) -> Option<(i128, u32)> {
        if id.tag() != ConstId::TAG_INT {
            return None;
        }
        let idx = id.index() as usize;
        self.int_consts.get(idx).copied()
    }

    /// 获取浮点常量: IEEE 754 bits（低 64 位；f128 常量请用 [`get_float128`]）。
    /// 若 ConstId 不是浮点类型则返回 None。
    pub fn get_float(&self, id: ConstId) -> Option<u64> {
        self.get_float128(id).map(|b| b as u64)
    }

    /// 获取浮点常量: 128 位 IEEE 754 bits（f64 常量为零扩展的低 64 位）。
    /// 若 ConstId 不是浮点类型则返回 None。
    pub fn get_float128(&self, id: ConstId) -> Option<u128> {
        self.get_float_with_width(id).map(|(bits, _)| bits)
    }

    /// 获取浮点常量: (位模式, 值宽 bits)。值宽是入库时记录的（见 `insert_float_typed`）。
    pub fn get_float_with_width(&self, id: ConstId) -> Option<(u128, u16)> {
        if id.tag() != ConstId::TAG_FLOAT {
            return None;
        }
        let idx = id.index() as usize;
        self.float_consts.get(idx).copied()
    }

    /// 获取浮点常量的值宽（bits：16/32/64/128）。
    pub fn get_float_width(&self, id: ConstId) -> Option<u16> {
        self.get_float_with_width(id).map(|(_, w)| w)
    }

    /// 获取任意精度常量。若 ConstId 不是 Big 类型则返回 None。
    pub fn get_big(&self, id: ConstId) -> Option<&Big> {
        if id.tag() != ConstId::TAG_BIG {
            return None;
        }
        let idx = id.index() as usize;
        self.big_consts.get(idx)
    }

    /// 把源池中的常量重定位到本池（跨函数克隆/内联用）。按 ConstId tag
    /// 分发：int → 解析 (value, bits) 重建；float → 128 位全宽重建；
    /// big → 解析重建；vector → 字节 + 端序重建。解析失败时原样返回
    /// （上游已保证 cid 来自源池，正常路径不会走到 fallback）。
    pub fn remap_from(&mut self, src: &ConstantPool, cid: ConstId) -> ConstId {
        match cid.tag() {
            ConstId::TAG_INT => match src.get_int(cid) {
                Some((v, bits)) => self.insert_int(v, bits),
                None => cid,
            },
            ConstId::TAG_FLOAT => match src.get_float_with_width(cid) {
                Some((bits, width)) => self.insert_float_typed(bits, width),
                None => cid,
            },
            ConstId::TAG_BIG => match src.get_big(cid) {
                Some(b) => self.insert_big(b.clone()),
                None => cid,
            },
            ConstId::TAG_VEC => match (src.get_vector(cid), src.get_vector_endian(cid)) {
                (Some(data), Some(endian)) => self.insert_vector_with_endian(data, endian),
                _ => cid,
            },
            _ => cid,
        }
    }

    /// 获取向量常量（原始字节切片）。若 ConstId 不是向量类型则返回 None。
    /// 段 i 范围 = vec_offsets[i] .. vec_offsets[i+1]（末段到 vec_data.len()）。
    pub fn get_vector(&self, id: ConstId) -> Option<&[u8]> {
        if id.tag() != ConstId::TAG_VEC {
            return None;
        }
        let i = id.index() as usize;
        let start = *self.vec_offsets.get(i)? as usize;
        let end = self
            .vec_offsets
            .get(i + 1)
            .copied()
            .unwrap_or(self.vec_data.len() as u32) as usize;
        self.vec_data.get(start..end)
    }

    /// 向量常量总数（段数）。
    pub fn vec_len(&self) -> usize {
        self.vec_offsets.len()
    }

    /// 获取向量常量的端序（Little/Big）。若 ConstId 不是向量类型则返回 None。
    /// DSL 还原（vc_pair/vc_cont 的 from_le/from_be）据此选择字节序。
    pub fn get_vector_endian(&self, id: ConstId) -> Option<Endianness> {
        if id.tag() != ConstId::TAG_VEC {
            return None;
        }
        self.vec_endian.get(id.index() as usize).copied()
    }

    /// 常量总数（含聚合池——历史实现漏了 aggregate，导致"池非空但 total_len 为 0"）。
    pub fn total_len(&self) -> usize {
        self.int_consts.len()
            + self.float_consts.len()
            + self.big_consts.len()
            + self.vec_offsets.len()
            + self.aggregates.len()
    }

    /// 是否为空（含聚合池）。
    pub fn is_empty(&self) -> bool {
        self.int_consts.is_empty()
            && self.float_consts.is_empty()
            && self.big_consts.is_empty()
            && self.vec_offsets.is_empty()
            && self.vec_endian.is_empty()
            && self.aggregates.is_empty()
    }

    /// 常量总数。
    pub fn len(&self) -> usize {
        self.total_len()
    }

    /// 解析 ConstId 为 i64 值 (自动根据 tag 分发到 int/big 池)。
    /// 超出 i64 范围的宽常量返回 None（不再静默截断）。
    pub fn resolve_int(&self, id: ConstId) -> Option<i64> {
        match id.tag() {
            ConstId::TAG_INT => self.get_int(id).and_then(|(v, _)| i64::try_from(v).ok()),
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

/// `Default` = `new()`（**含 bool 预置槽**）。
///
/// 历史实现是 `#[derive(Default)]`，绕过了 `new()` 的 `insert_int(0,1)/(1,1)`，
/// 而 `bool_const` 直接构造 `ConstId(index 0|1)`（不去重、不插入）——于是
/// `ConstantPool::default()` 造出的池里这两个 id 悬空（公开 API 陷阱）。
impl Default for ConstantPool {
    fn default() -> Self {
        Self::new()
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

    /// 扁平向量池：字节去重、offset 切片（段长 = next − start）、末段越界补空。
    #[test]
    fn test_vector_flat_pool() {
        let mut pool = ConstantPool::new();
        let a = pool.insert_vector(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let b = pool.insert_vector(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(a, b, "相同字节应去重");
        let c = pool.insert_vector(&[9, 9, 9]);
        assert_ne!(a, c);
        // 段切片正确（a 8 字节、c 3 字节；扁平池连续存储）
        assert_eq!(pool.get_vector(a), Some(&[1, 2, 3, 4, 5, 6, 7, 8][..]));
        assert_eq!(pool.get_vector(c), Some(&[9, 9, 9][..]));
        assert_eq!(pool.vec_len(), 2);
        // 非法 ConstId（tag 非 VEC）→ None
        let bad = ConstId::pack(ConstId::TAG_INT, 0);
        assert_eq!(pool.get_vector(bad), None);
    }

    /// 端序记录：insert_vector 默认 Little；with_endian 记录 Big；
    /// get_vector_endian 读回；非法 tag → None。
    #[test]
    fn test_vector_pool_endian() {
        let mut pool = ConstantPool::new();
        let le = pool.insert_vector(&[1, 2, 3, 4]);
        assert_eq!(pool.get_vector_endian(le), Some(Endianness::Little));
        let be = pool.insert_vector_with_endian(&[4, 3, 2, 1], Endianness::Big);
        assert_eq!(pool.get_vector_endian(be), Some(Endianness::Big));
        // 不同端序字节不同 → 不冲突去重
        assert_ne!(le, be);
        assert_eq!(pool.vec_len(), 2);
        // 同字节同端序去重
        let be2 = pool.insert_vector_with_endian(&[4, 3, 2, 1], Endianness::Big);
        assert_eq!(be, be2);
        // 非法 tag → None
        let bad = ConstId::pack(ConstId::TAG_FLOAT, 0);
        assert_eq!(pool.get_vector_endian(bad), None);
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

    /// f128 常量可入池（128 位全宽），与 f64 常量互不干扰。
    #[test]
    fn test_float128_pool() {
        let mut pool = ConstantPool::new();
        let f64_bits = 1.5f64.to_bits();
        let f128_bits: u128 = 0x3FFF_8000_0000_0000_0000_0000_0000_0000; // 1.5 in f128
        let a = pool.insert_float(f64_bits);
        let b = pool.insert_float128(f128_bits);
        assert_ne!(a, b, "f64 与 f128 常量应区分");
        assert_eq!(pool.get_float128(a), Some(f64_bits as u128));
        assert_eq!(pool.get_float128(b), Some(f128_bits));
        // f64 读取视角：f128 常量低 64 位 ≠ 完整 bits（调用方应感知类型）
        assert_eq!(pool.get_float(b), Some(f128_bits as u64));
        // 去重
        assert_eq!(pool.insert_float128(f128_bits), b);
    }

    /// resolve_int 对超出 i64 范围的宽常量返回 None（不再静默截断）。
    #[test]
    fn test_resolve_int_no_truncate() {
        let mut pool = ConstantPool::new();
        let ok = pool.insert_int(42, 64);
        assert_eq!(pool.resolve_int(ok), Some(42));
        let wide = pool.insert_int(1i128 << 100, 128);
        assert_eq!(
            pool.resolve_int(wide),
            None,
            "超 i64 范围应返回 None 而非截断"
        );
        // big 池超范围同样 None
        let big = pool.insert_big(Big::from_i128((1i128 << 100) + 1));
        assert_eq!(pool.resolve_int(big), None);
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
        // new() 预置 bool 0/1 槽位（index 0 = false，index 1 = true）
        let pool = ConstantPool::new();
        assert!(!pool.is_empty());
        assert_eq!(pool.total_len(), 2);
        assert_eq!(
            pool.get_int(ConstId::pack(ConstId::TAG_INT, 0)),
            Some((0, 1))
        );
        assert_eq!(
            pool.get_int(ConstId::pack(ConstId::TAG_INT, 1)),
            Some((1, 1))
        );
    }

    #[test]
    fn test_bool_const_slots() {
        let mut pool = ConstantPool::new();
        // bool_const 返回预置槽位，不新增池条目
        let f = pool.bool_const(false);
        let t = pool.bool_const(true);
        assert_ne!(f, t);
        assert_eq!(pool.get_int(f), Some((0, 1)));
        assert_eq!(pool.get_int(t), Some((1, 1)));
        // 反复调用不膨胀常量池（dedup 命中预置槽）
        for _ in 0..10 {
            pool.bool_const(true);
        }
        assert_eq!(pool.total_len(), 2);
        // 与 insert_int 去重互通：插入 (1,1) 命中预置 true 槽
        assert_eq!(pool.insert_int(1, 1), t);
        assert_eq!(pool.insert_int(0, 1), f);
    }
}
