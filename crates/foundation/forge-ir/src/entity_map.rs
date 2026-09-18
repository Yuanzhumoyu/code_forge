//! 实体容器与密集索引（v3 方案 **S2**）。
//!
//! 背景（审计 §2.5）：本 crate 的实体句柄（[`crate::Value`]、[`crate::Inst`]、
//! [`crate::Block`] …）**本身就是密集下标**，但辅助数据长期挂在
//! `HashMap<句柄, _>` 上——每次访问一次哈希 + 探测，且丢失了"句柄 → 槽位"的
//! 稠密性。Cranelift 的做法是 `PrimaryMap`/`SecondaryMap`/`EntitySet`：
//! 用 `Vec` 直接索引，`Option<T>` 表示"未设置"，并**刻意不支持删除**
//! （删除会让句柄失效，而句柄在 IR 里到处被引用——墓碑语义见 `Layout`）。
//!
//! 本模块提供四个容器，语义都写死在类型上：
//!
//! - [`PrimaryMap`]：**主存**。`push` 分配句柄，下标即句柄，只增不删。
//! - [`SecondaryMap`]：**辅存**。按句柄稀疏/密集地挂数据，缺省为"未设置"，
//!   `get` 返回 `Option`；未设置与空值可区分。可 `clear`（复用槽位）。
//! - [`EntitySet`]：**句柄集合**。密集位图，`O(1)` 插入/查询/删除。
//! - [`PackedOption`]：**句柄的可空压缩**。`Option<Value>` 是 8 字节
//!   （`Value` 没有 niche），`PackedOption<Value>` 是 4 字节（`u32::MAX` = 空）。
//!
//! 这里**不做** `ListPool`：本仓库的"列表"用途（块内指令序、支配树子节点）
//! 目前都是短生命周期局部量，交给它反而增加一层间接；等出现真正的
//! 长生命周期共享列表再引入（方案 §4-C 的其余项同理按需推进）。

use std::fmt::{self, Debug};
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

// ============================================================
// EntityRef
// ============================================================

/// 可作为密集索引键的实体句柄。
///
/// 约定：`as_u32()` 是**稳定的密集下标**（0 起、连续、只增不删），
/// `from_u32(i)` 是它的逆。新句柄类型实现本 trait 即可直接用这些容器。
pub trait EntityRef: Copy + Eq + std::hash::Hash {
    /// 句柄作为密集下标。
    fn as_u32(self) -> u32;
    /// 由密集下标还原句柄。
    fn from_u32(index: u32) -> Self;
}

/// 为 `pub struct X(pub u32)` 形态的句柄批量实现 [`EntityRef`]。
#[macro_export]
macro_rules! entity_ref_impls {
    ($($t:ty),* $(,)?) => {
        $(
            impl $crate::entity_map::EntityRef for $t {
                #[inline]
                fn as_u32(self) -> u32 {
                    self.0
                }
                #[inline]
                fn from_u32(index: u32) -> Self {
                    Self(index)
                }
            }
        )*
    };
}

// ============================================================
// PrimaryMap
// ============================================================

/// 主存：`push` 分配句柄，下标即句柄（只增不删）。
///
/// 与 `Vec<V>` 的唯一区别是**类型层面绑定了键**：`push` 返回 `K`，
/// 取值/改值都用 `K`，杜绝"用 Inst 下标去索引 Values 表"这一类串键。
pub struct PrimaryMap<K, V> {
    elems: Vec<V>,
    _marker: PhantomData<fn() -> K>,
}

impl<K, V> PrimaryMap<K, V>
where
    K: EntityRef,
{
    /// 空表。
    pub fn new() -> Self {
        Self {
            elems: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// 预留容量的空表。
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            elems: Vec::with_capacity(capacity),
            _marker: PhantomData,
        }
    }

    /// 追加一个元素并返回它的句柄。
    pub fn push(&mut self, value: V) -> K {
        let index = self.elems.len() as u32;
        self.elems.push(value);
        K::from_u32(index)
    }

    /// 元素个数（= 下一个可用句柄）。
    pub fn len(&self) -> usize {
        self.elems.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.elems.is_empty()
    }

    /// 按句柄取引用（越界返回 `None`）。
    pub fn get(&self, key: K) -> Option<&V> {
        self.elems.get(key.as_u32() as usize)
    }

    /// 按句柄取可变引用（越界返回 `None`）。
    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        self.elems.get_mut(key.as_u32() as usize)
    }

    /// 全部句柄（按分配序）。
    pub fn keys(&self) -> impl Iterator<Item = K> + '_ {
        (0..self.elems.len() as u32).map(K::from_u32)
    }

    /// 全部值（按分配序）。
    pub fn values(&self) -> std::slice::Iter<'_, V> {
        self.elems.iter()
    }

    /// 全部值（可变）。
    pub fn values_mut(&mut self) -> std::slice::IterMut<'_, V> {
        self.elems.iter_mut()
    }

    /// `(句柄, 值)` 迭代。
    pub fn iter(&self) -> impl Iterator<Item = (K, &V)> + '_ {
        self.elems
            .iter()
            .enumerate()
            .map(|(i, v)| (K::from_u32(i as u32), v))
    }

    /// `(句柄, 值)` 可变迭代。
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (K, &mut V)> + '_ {
        self.elems
            .iter_mut()
            .enumerate()
            .map(|(i, v)| (K::from_u32(i as u32), v))
    }

    /// 底层切片（只读）。
    pub fn as_slice(&self) -> &[V] {
        &self.elems
    }

    /// 底层切片（可写）——供已有代码按 `&mut [V]` 批量处理的场景。
    pub fn as_mut_slice(&mut self) -> &mut [V] {
        &mut self.elems
    }
}

impl<K: EntityRef, V> Default for PrimaryMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: EntityRef, V> Index<K> for PrimaryMap<K, V> {
    type Output = V;
    fn index(&self, key: K) -> &V {
        self.get(key).unwrap_or_else(|| {
            panic!(
                "PrimaryMap: 句柄 {} 越界（len = {}）",
                key.as_u32(),
                self.elems.len()
            )
        })
    }
}

impl<K: EntityRef, V> IndexMut<K> for PrimaryMap<K, V> {
    fn index_mut(&mut self, key: K) -> &mut V {
        let len = self.elems.len();
        self.get_mut(key)
            .unwrap_or_else(|| panic!("PrimaryMap: 句柄 {} 越界（len = {len}）", key.as_u32()))
    }
}

impl<K: EntityRef, V: Debug> Debug for PrimaryMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.elems.iter()).finish()
    }
}

// 手写 Clone/PartialEq/Eq：派生会多带无用的 `K: Clone/Eq` 约束
// （`PhantomData<fn() -> K>` 本身对 K 无要求）。
impl<K, V: Clone> Clone for PrimaryMap<K, V> {
    fn clone(&self) -> Self {
        Self {
            elems: self.elems.clone(),
            _marker: PhantomData,
        }
    }
}

impl<K, V: PartialEq> PartialEq for PrimaryMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.elems == other.elems
    }
}

impl<K, V: Eq> Eq for PrimaryMap<K, V> {}

impl<K: EntityRef, V> FromIterator<V> for PrimaryMap<K, V> {
    fn from_iter<I: IntoIterator<Item = V>>(iter: I) -> Self {
        Self {
            elems: iter.into_iter().collect(),
            _marker: PhantomData,
        }
    }
}

impl<K: EntityRef, V> IntoIterator for PrimaryMap<K, V> {
    type Item = V;
    type IntoIter = std::vec::IntoIter<V>;
    fn into_iter(self) -> Self::IntoIter {
        self.elems.into_iter()
    }
}

// ============================================================
// SecondaryMap
// ============================================================

/// 辅存：按句柄挂数据，**未设置**与"设置了空值"可区分。
///
/// 内部是 `Vec<Option<V>>`，按需增长到键的下标；句柄本身就是下标，
/// 因此 `get`/`insert` 都是 `O(1)` 且无哈希。
///
/// 语义与 `HashMap<句柄, V>` 的差别（都是刻意的）：
/// - 缺失不是错误：`get` 返回 `None`；`Index` 越界或缺失才 panic；
/// - 允许 `clear`（保留容量）与 `remove`（留下空洞，句柄不复用）；
/// - `iter` 只产出已设置的项。
pub struct SecondaryMap<K, V> {
    elems: Vec<Option<V>>,
    _marker: PhantomData<fn() -> K>,
}

impl<K, V> SecondaryMap<K, V>
where
    K: EntityRef,
{
    /// 空表。
    pub fn new() -> Self {
        Self {
            elems: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// 至少能容纳 `keys` 个句柄的空表。
    pub fn with_capacity(keys: usize) -> Self {
        Self {
            elems: (0..keys).map(|_| None).collect(),
            _marker: PhantomData,
        }
    }

    /// 设置句柄对应的值（覆盖旧值，必要时增长）。
    ///
    /// 返回被覆盖的旧值（与 `HashMap::insert` 一致）。
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let idx = key.as_u32() as usize;
        if idx >= self.elems.len() {
            self.elems.resize_with(idx + 1, || None);
        }
        self.elems[idx].replace(value)
    }

    /// 取引用（未设置返回 `None`）。
    pub fn get(&self, key: K) -> Option<&V> {
        self.elems.get(key.as_u32() as usize)?.as_ref()
    }

    /// 取可变引用（未设置返回 `None`）。
    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        self.elems.get_mut(key.as_u32() as usize)?.as_mut()
    }

    /// 取可变引用，未设置则插入 `default()`（等价 `HashMap::entry().or_default()`）。
    pub fn get_mut_or_default(&mut self, key: K) -> &mut V
    where
        V: Default,
    {
        self.get_mut_or_insert_with(key, V::default)
    }

    /// 取可变引用，未设置则插入 `default()` 的求值结果
    /// （等价 `HashMap::entry().or_insert_with(..)`）。
    pub fn get_mut_or_insert_with(&mut self, key: K, default: impl FnOnce() -> V) -> &mut V {
        let idx = key.as_u32() as usize;
        if idx >= self.elems.len() {
            self.elems.resize_with(idx + 1, || None);
        }
        self.elems[idx].get_or_insert_with(default)
    }

    /// 是否已设置。
    pub fn contains_key(&self, key: K) -> bool {
        self.get(key).is_some()
    }

    /// 清除某个句柄的值（留下空洞，其它句柄不受影响）。
    pub fn remove(&mut self, key: K) -> Option<V> {
        self.elems.get_mut(key.as_u32() as usize)?.take()
    }

    /// 清空全部项（保留容量，槽位可复用）。
    pub fn clear(&mut self) {
        for slot in &mut self.elems {
            *slot = None;
        }
    }

    /// 已设置项的数量。
    pub fn len(&self) -> usize {
        self.elems.iter().filter(|s| s.is_some()).count()
    }

    /// 是否没有任何已设置项。
    pub fn is_empty(&self) -> bool {
        self.elems.iter().all(|s| s.is_none())
    }

    /// 内部槽位总数（含空洞）——用于容量诊断。
    pub fn capacity(&self) -> usize {
        self.elems.len()
    }

    /// 全部已设置项。
    pub fn iter(&self) -> impl Iterator<Item = (K, &V)> + '_ {
        self.elems
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|v| (K::from_u32(i as u32), v)))
    }

    /// 全部已设置项（可变）。
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (K, &mut V)> + '_ {
        self.elems
            .iter_mut()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_mut().map(|v| (K::from_u32(i as u32), v)))
    }

    /// 全部已设置的值。
    pub fn values(&self) -> impl Iterator<Item = &V> + '_ {
        self.elems.iter().filter_map(|s| s.as_ref())
    }

    /// 全部已设置的值（可变）。
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> + '_ {
        self.elems.iter_mut().filter_map(|s| s.as_mut())
    }

    /// 全部已设置的键。
    pub fn keys(&self) -> impl Iterator<Item = K> + '_ {
        self.iter().map(|(k, _)| k)
    }
}

impl<K: EntityRef, V> Default for SecondaryMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: EntityRef, V> Index<K> for SecondaryMap<K, V> {
    type Output = V;
    fn index(&self, key: K) -> &V {
        self.get(key).unwrap_or_else(|| {
            panic!(
                "SecondaryMap: 句柄 {} 未设置（槽位 {}）",
                key.as_u32(),
                self.elems.len()
            )
        })
    }
}

impl<K: EntityRef, V> IndexMut<K> for SecondaryMap<K, V> {
    fn index_mut(&mut self, key: K) -> &mut V {
        let idx = key.as_u32() as usize;
        self.get_mut(key)
            .unwrap_or_else(|| panic!("SecondaryMap: 句柄 {} 未设置（槽位 {idx}）", key.as_u32()))
    }
}

impl<K: EntityRef + Debug, V: Debug> Debug for SecondaryMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V: Clone> Clone for SecondaryMap<K, V> {
    fn clone(&self) -> Self {
        Self {
            elems: self.elems.clone(),
            _marker: PhantomData,
        }
    }
}

/// `(句柄, 值)` 迭代器 → 辅存（与 `HashMap` 的 `collect()` 同形；重复键后者胜）。
///
/// 用于把"按句柄顺序生成的一批映射"直接收集成密集表
/// （如支配树的 `postorder_rank: Block → usize`）。
impl<K: EntityRef, V> FromIterator<(K, V)> for SecondaryMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = SecondaryMap::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

impl<K, V: PartialEq> PartialEq for SecondaryMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.elems == other.elems
    }
}

impl<K, V: Eq> Eq for SecondaryMap<K, V> {}

// ============================================================
// EntitySet
// ============================================================

/// 句柄集合：密集位图（`O(1)` 插入/查询/删除，无哈希）。
pub struct EntitySet<K> {
    words: Vec<u64>,
    len: usize,
    _marker: PhantomData<fn() -> K>,
}

impl<K: EntityRef> EntitySet<K> {
    /// 空集合。
    pub fn new() -> Self {
        Self {
            words: Vec::new(),
            len: 0,
            _marker: PhantomData,
        }
    }

    /// 插入；返回 `true` 表示此前不在集合里。
    pub fn insert(&mut self, key: K) -> bool {
        let idx = key.as_u32() as usize;
        let word = idx / 64;
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        let bit = 1u64 << (idx % 64);
        let was_absent = self.words[word] & bit == 0;
        if was_absent {
            self.words[word] |= bit;
            self.len += 1;
        }
        was_absent
    }

    /// 是否在集合里。
    pub fn contains(&self, key: K) -> bool {
        let idx = key.as_u32() as usize;
        self.words
            .get(idx / 64)
            .is_some_and(|w| w & (1u64 << (idx % 64)) != 0)
    }

    /// 删除；返回 `true` 表示此前在集合里。
    pub fn remove(&mut self, key: K) -> bool {
        let idx = key.as_u32() as usize;
        let Some(word) = self.words.get_mut(idx / 64) else {
            return false;
        };
        let bit = 1u64 << (idx % 64);
        let was_present = *word & bit != 0;
        if was_present {
            *word &= !bit;
            self.len -= 1;
        }
        was_present
    }

    /// 清空（保留容量）。
    pub fn clear(&mut self) {
        self.words.fill(0);
        self.len = 0;
    }

    /// 元素个数。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 迭代全部元素（按下标升序）。
    pub fn iter(&self) -> impl Iterator<Item = K> + '_ {
        self.words.iter().enumerate().flat_map(|(wi, word)| {
            let word = *word;
            (0..64)
                .filter(move |b| word & (1u64 << b) != 0)
                .map(move |b| K::from_u32((wi * 64 + b) as u32))
        })
    }
}

impl<K: EntityRef> Default for EntitySet<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: EntityRef + Debug> Debug for EntitySet<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<K> Clone for EntitySet<K> {
    fn clone(&self) -> Self {
        Self {
            words: self.words.clone(),
            len: self.len,
            _marker: PhantomData,
        }
    }
}

impl<K> PartialEq for EntitySet<K> {
    fn eq(&self, other: &Self) -> bool {
        self.words == other.words && self.len == other.len
    }
}

impl<K> Eq for EntitySet<K> {}

// ============================================================
// PackedOption
// ============================================================

/// 句柄的可空压缩：`Option<句柄>` 占 4 字节（`u32::MAX` 表示空）。
///
/// 用途：DOM 树的 `idom`、别名分析的 memo、支配边界等"每个句柄一个可选句柄"
/// 的表——`Option<Value>` 是 8 字节（`Value` 无 niche），这里是 4 字节。
///
/// `Value(u32::MAX)` 因此**不是合法句柄**（密集下标不可能到达）；这一点由
/// [`EntityRef`] 的约定保证（句柄是连续分配的密集下标）。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackedOption<K>(u32, PhantomData<fn() -> K>);

impl<K: EntityRef> PackedOption<K> {
    /// 空值。
    pub const NONE_RAW: u32 = u32::MAX;

    /// 由 `Option<K>` 构造。
    pub fn new(value: Option<K>) -> Self {
        match value {
            None => Self::none(),
            Some(k) => Self::some(k),
        }
    }

    /// 空。
    pub fn none() -> Self {
        Self(Self::NONE_RAW, PhantomData)
    }

    /// 有值。
    pub fn some(key: K) -> Self {
        assert_ne!(
            key.as_u32(),
            Self::NONE_RAW,
            "PackedOption: u32::MAX 是空值哨兵，不是合法句柄"
        );
        Self(key.as_u32(), PhantomData)
    }

    /// 是否为空。
    pub fn is_none(self) -> bool {
        self.0 == Self::NONE_RAW
    }

    /// 是否有值。
    pub fn is_some(self) -> bool {
        !self.is_none()
    }

    /// 展开成 `Option<K>`。
    pub fn expand(self) -> Option<K> {
        if self.is_none() {
            None
        } else {
            Some(K::from_u32(self.0))
        }
    }

    /// 取值（空则 panic——与 `Option::unwrap` 同义）。
    pub fn unwrap(self) -> K {
        self.expand()
            .expect("PackedOption::unwrap: 空值（用 expand()/is_some() 先判断）")
    }

    /// 取值或给默认句柄。
    pub fn unwrap_or(self, default: K) -> K {
        self.expand().unwrap_or(default)
    }
}

impl<K: EntityRef> Default for PackedOption<K> {
    fn default() -> Self {
        Self::none()
    }
}

impl<K: EntityRef> From<Option<K>> for PackedOption<K> {
    fn from(value: Option<K>) -> Self {
        Self::new(value)
    }
}

impl<K: EntityRef> From<PackedOption<K>> for Option<K> {
    fn from(value: PackedOption<K>) -> Self {
        value.expand()
    }
}

impl<K: EntityRef> Debug for PackedOption<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.expand() {
            Some(k) => write!(f, "PackedOption::some({})", k.as_u32()),
            None => write!(f, "PackedOption::none"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, Inst, Value};

    #[test]
    fn primary_map_assigns_dense_handles() {
        let mut m: PrimaryMap<Value, u32> = PrimaryMap::new();
        let a = m.push(10);
        let b = m.push(20);
        assert_eq!(a, Value(0));
        assert_eq!(b, Value(1));
        assert_eq!(m.len(), 2);
        assert_eq!(m[a], 10);
        assert_eq!(m.get(Value(2)), None);
        *m.get_mut(b).unwrap() += 1;
        assert_eq!(m[b], 21);
        assert_eq!(m.keys().collect::<Vec<_>>(), vec![a, b]);
        assert_eq!(
            m.iter().map(|(k, v)| (k.as_u32(), *v)).collect::<Vec<_>>(),
            vec![(0, 10), (1, 21)]
        );
        assert_eq!(m.into_iter().collect::<Vec<_>>(), vec![10, 21]);
    }

    #[test]
    #[should_panic(expected = "越界")]
    fn primary_map_index_out_of_range_panics() {
        let m: PrimaryMap<Value, u32> = PrimaryMap::new();
        let _ = m[Value(0)];
    }

    #[test]
    fn secondary_map_distinguishes_absent_and_set() {
        let mut m: SecondaryMap<Inst, u8> = SecondaryMap::new();
        assert!(m.is_empty());
        assert_eq!(m.get(Inst(3)), None);
        assert_eq!(m.insert(Inst(3), 7), None);
        assert_eq!(m.get(Inst(3)), Some(&7));
        assert_eq!(m.len(), 1);
        assert_eq!(m.capacity(), 4);
        // 覆盖返回旧值
        assert_eq!(m.insert(Inst(3), 9), Some(7));
        assert_eq!(m[Inst(3)], 9);
        // 空洞不影响其它键
        assert_eq!(m.remove(Inst(3)), Some(9));
        assert_eq!(m.get(Inst(3)), None);
        assert!(!m.contains_key(Inst(3)));
        assert_eq!(m.remove(Inst(3)), None);
        // clear 保留容量
        m.insert(Inst(1), 1);
        m.insert(Inst(2), 2);
        m.clear();
        assert!(m.is_empty());
        assert_eq!(m.capacity(), 4);
        assert_eq!(m.iter().count(), 0);
    }

    #[test]
    fn secondary_map_iterates_only_set_entries() {
        let mut m: SecondaryMap<Block, &'static str> = SecondaryMap::new();
        m.insert(Block(2), "b2");
        m.insert(Block(0), "b0");
        assert_eq!(
            m.iter().map(|(k, v)| (k.as_u32(), *v)).collect::<Vec<_>>(),
            vec![(0, "b0"), (2, "b2")]
        );
        assert_eq!(m.keys().map(Block::as_u32).collect::<Vec<_>>(), vec![0, 2]);
        assert_eq!(m.values().copied().collect::<Vec<_>>(), vec!["b0", "b2"]);
    }

    #[test]
    #[should_panic(expected = "未设置")]
    fn secondary_map_index_unset_panics() {
        let m: SecondaryMap<Value, u32> = SecondaryMap::new();
        let _ = m[Value(0)];
    }

    #[test]
    fn entity_set_is_a_dense_bitset() {
        let mut s: EntitySet<Value> = EntitySet::new();
        assert!(s.is_empty());
        assert!(s.insert(Value(0)));
        assert!(!s.insert(Value(0)), "重复插入返回 false");
        assert!(s.insert(Value(70)));
        assert_eq!(s.len(), 2);
        assert!(s.contains(Value(70)));
        assert!(!s.contains(Value(69)));
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![Value(0), Value(70)]);
        assert!(s.remove(Value(0)));
        assert!(!s.remove(Value(0)));
        assert_eq!(s.len(), 1);
        s.clear();
        assert!(s.is_empty());
        assert!(!s.contains(Value(70)));
    }

    #[test]
    fn packed_option_is_four_bytes_and_reversible() {
        assert_eq!(std::mem::size_of::<PackedOption<Value>>(), 4);
        assert_eq!(std::mem::size_of::<Option<Value>>(), 8);
        let none: PackedOption<Value> = PackedOption::none();
        assert!(none.is_none() && !none.is_some());
        assert_eq!(none.expand(), None);
        let some = PackedOption::some(Value(7));
        assert!(some.is_some());
        assert_eq!(some.expand(), Some(Value(7)));
        assert_eq!(some.unwrap(), Value(7));
        assert_eq!(none.unwrap_or(Value(3)), Value(3));
        assert_eq!(
            PackedOption::from(Some(Value(1))),
            PackedOption::some(Value(1))
        );
        assert_eq!(Option::<Value>::from(none), None);
        assert_eq!(PackedOption::<Value>::default(), none);
    }

    #[test]
    fn entity_ref_roundtrip_for_handle_types() {
        assert_eq!(Value::from_u32(3).as_u32(), 3);
        assert_eq!(Inst::from_u32(9).as_u32(), 9);
        assert_eq!(Block::from_u32(1).as_u32(), 1);
        assert_eq!(crate::TypeId::from_u32(4).as_u32(), 4);
    }
}
