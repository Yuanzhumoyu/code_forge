//! 常量池（v28 新设计，不兼容旧池）。
//!
//! ## 设计
//!
//! 池里只有**一种标量形态**：[`BitValue`]（位模式 + 位宽），整数与浮点共用一张按值去重的表
//! ——"这是整数还是浮点"由**使用点的类型/opcode**决定（`Iconst` vs `Fconst`），池不记录种类。
//! 于是 `i32 0x3FC0_0000` 与 `f32 1.5` 是同一个 [`ConstId`]，这正是合并的意图。
//!
//! 句柄 [`ConstId`] 是**四元枚举**（`Bits` / `Big` / `Vector` / `Agg`）：2 bit tag 恰好铺满，
//! 因此 `raw()`/`from_raw()` 全域定义、永不 panic。聚合常量与标量共享同一句柄空间
//! （旧设计的独立 `AggId` 已删除），聚合的子节点就是 [`ConstId`]——"子节点是不是聚合"
//! 由池自行判定，不需要额外的 `AggChild` 类型。
//!
//! ## 四条通道
//!
//! | tag | 变体 | 载荷 | 说明 |
//! | --- | --- | --- | --- |
//! | 0 | [`ConstId::Bits`] | [`BitValue`] | 整数/浮点标量；整数入池套 [`canonical_int`] |
//! | 1 | [`ConstId::Big`] | [`Big`] | 任意精度（`>128` 位整数、非标准浮点） |
//! | 2 | [`ConstId::Vector`] | 字节 + 端序 | 向量常量；**端序进去重键** |
//! | 3 | [`ConstId::Agg`] | [`AggConst`] | 树形聚合常量 |
//!
//! 池**没有预置槽**：`new()` 就是空池，`bool` 只是普通常量（去重后自然只有两条）。
//! 每个通道都用 interner 按值去重，句柄索引 = 插入顺序，因此二进制编码/解码按序重建即可
//! 保证句柄逐位不变。

pub(crate) mod as_const;

pub use as_const::AsConst;

use ahash::AHasher;
use xgx_intern::Interner;

use crate::Big;
use crate::entity::{ConstId, Endianness, TypeId};
use std::hash::BuildHasherDefault;

type FastBuildHasher = BuildHasherDefault<AHasher>;
type FastInterner<T> = Interner<T, FastBuildHasher>;

// ============================================================
// 位值的规范化
// ============================================================

/// 按位宽截断：只保留低 `bits` 位（`bits == 0` 恒 0；`bits >= 128` 原样）。
pub fn mask_to_bits(value: u128, bits: u32) -> u128 {
    if bits == 0 {
        return 0;
    }
    if bits >= 128 {
        return value;
    }
    value & ((1u128 << bits) - 1)
}

/// 整数常量的**规范形**：按位宽做**符号扩展**（two's-complement 解释）。
///
/// IR 整数是"无符号位图 + 符号由 opcode 表达"，而消费者普遍按**有符号**读
/// （`const_int(-1, i64)` 必须仍是 -1），文本打印也按有符号字面量输出。两者的交集就是
/// 符号扩展规范形 ⇒ 文本/二进制往返稳定。
///
/// `i1` 是 bool：取值域 0/1，**不做**符号扩展（扩展会得到 -1，与 bool 约定冲突）。
pub fn canonical_int(value: u128, bits: u32) -> u128 {
    if bits == 0 || bits >= 128 {
        return value;
    }
    if bits == 1 {
        return mask_to_bits(value, 1);
    }
    let low = mask_to_bits(value, bits);
    if low & (1u128 << (bits - 1)) != 0 {
        low | !((1u128 << bits) - 1)
    } else {
        low
    }
}

// ============================================================
// 值型
// ============================================================

/// 标量常量的**唯一**存储形态：位模式 + 位宽（`1..=128`；更宽走 [`Big`]）。
///
/// - 整数：`bits` 是 [`canonical_int`] 规范形（符号扩展后的位图）；
/// - 浮点：`bits` 是 IEEE 754 位模式（低 `width` 位有效），不做规范化。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BitValue {
    pub bits: u128,
    pub width: u32,
}

impl BitValue {
    pub const fn new(bits: u128, width: u32) -> Self {
        Self { bits, width }
    }
}

/// 聚合常量：类型 + 子常量句柄（标量或嵌套聚合）。
///
/// 循环结构**不可能**出现：插入时子句柄必然已存在，父句柄在插入后才分配。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AggConst {
    pub ty: TypeId,
    pub children: Vec<ConstId>,
}

/// 读取视图：[`ConstId`] 解析后的借用形态。
#[derive(Debug, PartialEq, Eq)]
pub enum ConstEntry<'a> {
    Bits(BitValue),
    Big(&'a Big),
    Vector(&'a [u8]),
    Agg(&'a AggConst),
}

/// 四条通道的条目数（二进制编码写头用）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChannelCounts {
    pub bits: usize,
    pub big: usize,
    pub vector: usize,
    pub agg: usize,
}

// ============================================================
// ConstantPool
// ============================================================

/// 常量池：四条按值去重的通道；句柄即索引。
#[derive(Clone, Debug, Default)]
pub struct ConstantPool {
    bits: FastInterner<BitValue>,
    bigs: FastInterner<BigKey>,
    vectors: FastInterner<VectorConst>,
    aggs: FastInterner<AggConst>,
}

impl ConstantPool {
    /// 空池（**没有**预置槽；`bool` 是普通常量）。
    pub fn new() -> Self {
        Self::default()
    }

    // ========================================================
    // 创建（全部按值去重）
    // ========================================================

    /// 插入位值（不做规范化；浮点与"已是规范形"的整数用它）。
    pub fn intern_bits(&mut self, value: BitValue) -> ConstId {
        debug_assert!(
            (1..=128).contains(&value.width),
            "位宽必须在 1..=128，实测 {}",
            value.width
        );
        ConstId::Bits(self.bits.intern_owned(value).expect("常量子数超出 u32"))
    }

    /// 插入有符号整数常量：入池前套 [`canonical_int`]（符号扩展规范形）。
    pub fn intern_int(&mut self, value: i128, width: u32) -> ConstId {
        let bits = canonical_int(value as u128, width);
        self.intern_bits(BitValue { bits, width })
    }

    /// 插入浮点常量（IEEE 754 位模式 + 值宽 16/32/64/128）。
    pub fn intern_float(&mut self, bits: u128, width: u16) -> ConstId {
        self.intern_bits(BitValue {
            bits,
            width: u32::from(width),
        })
    }

    /// 插入任意精度常量。
    pub fn intern_big(&mut self, value: Big) -> ConstId {
        let idx = self
            .bigs
            .intern_owned(BigKey(value))
            .expect("常量子数超出 u32");
        ConstId::Big(idx)
    }

    /// 插入向量常量（原始字节 + 端序；**端序进去重键**——同字节不同端序是不同常量）。
    pub fn intern_vector(&mut self, data: &[u8], endian: Endianness) -> ConstId {
        let idx = self
            .vectors
            .intern_owned(VectorConst {
                data: data.to_vec(),
                endian,
            })
            .expect("常量子数超出 u32");
        ConstId::Vector(idx)
    }

    /// 插入聚合常量（子节点是已有句柄）。
    pub fn intern_agg(&mut self, ty: TypeId, children: Vec<ConstId>) -> ConstId {
        let idx = self
            .aggs
            .intern_owned(AggConst { ty, children })
            .expect("常量子数超出 u32");
        ConstId::Agg(idx)
    }

    // ========================================================
    // 读取
    // ========================================================

    /// 解析句柄为读取视图；句柄不属于本池（或索引越界）时返回 `None`。
    pub fn get(&self, id: ConstId) -> Option<ConstEntry<'_>> {
        match id {
            ConstId::Bits(i) => self.bits.resolve(i).copied().map(ConstEntry::Bits),
            ConstId::Big(i) => self.bigs.resolve(i).map(|k| ConstEntry::Big(&k.0)),
            ConstId::Vector(i) => self
                .vectors
                .resolve(i)
                .map(|v| ConstEntry::Vector(&v.data[..])),
            ConstId::Agg(i) => self.aggs.resolve(i).map(ConstEntry::Agg),
        }
    }

    /// 位值视图（整数/浮点标量）。
    pub fn bits(&self, id: ConstId) -> Option<BitValue> {
        match self.get(id)? {
            ConstEntry::Bits(v) => Some(v),
            _ => None,
        }
    }

    /// 任意精度视图。
    pub fn big(&self, id: ConstId) -> Option<&Big> {
        match self.get(id)? {
            ConstEntry::Big(b) => Some(b),
            _ => None,
        }
    }

    /// 向量字节视图。
    pub fn vector(&self, id: ConstId) -> Option<&[u8]> {
        match self.get(id)? {
            ConstEntry::Vector(d) => Some(d),
            _ => None,
        }
    }

    /// 向量端序。
    pub fn vector_endian(&self, id: ConstId) -> Option<Endianness> {
        match id {
            ConstId::Vector(i) => self.vectors.resolve(i).map(|v| v.endian),
            _ => None,
        }
    }

    /// 聚合视图。
    pub fn agg(&self, id: ConstId) -> Option<&AggConst> {
        match self.get(id)? {
            ConstEntry::Agg(a) => Some(a),
            _ => None,
        }
    }

    // ========================================================
    // 视图强转（跨通道；生成代码的占位符解析用）
    // ========================================================

    /// 解析为 `i64`：位值按**原位宽**取补码有符号值（`i32::MIN` ⇒ -2147483648）；
    /// 超出 i64 范围（如 128 位大数）返回 `None`（**不截断**）。
    pub fn as_i64(&self, id: ConstId) -> Option<i64> {
        match self.get(id)? {
            ConstEntry::Bits(bv) => i64::try_from(signed_of(bv)).ok(),
            ConstEntry::Big(b) => b.try_to_i64(),
            _ => None,
        }
    }

    /// 解析为任意精度整数：位值按**原位宽**取补码有符号值（不丢高位语义）。
    pub fn as_big(&self, id: ConstId) -> Option<Big> {
        match self.get(id)? {
            ConstEntry::Bits(bv) => Some(Big::from_i128(signed_of(bv))),
            ConstEntry::Big(b) => Some(b.clone()),
            _ => None,
        }
    }

    /// 解析为 f64 的位模式（位值取低 64 位；`Big` 转浮点）。
    pub fn as_f64_bits(&self, id: ConstId) -> Option<u64> {
        match self.get(id)? {
            ConstEntry::Bits(v) => Some(v.bits as u64),
            ConstEntry::Big(b) => Some(b.to_f64().to_bits()),
            _ => None,
        }
    }

    // ========================================================
    // 池间导入（跨函数克隆 / 内联）
    // ========================================================

    /// 把 `src` 池里的常量重建到本池，返回本池句柄（聚合会递归导入子节点）。
    ///
    /// 句柄在池内自洽 ⇒ 悬空句柄属于**调用方缺陷**，这里直接 panic（旧实现的
    /// "解析失败就原样返回源句柄"会让跨池句柄悄悄泄漏到本池）。
    pub fn import(&mut self, src: &ConstantPool, id: ConstId) -> ConstId {
        match src.get(id) {
            Some(ConstEntry::Bits(v)) => self.intern_bits(v),
            Some(ConstEntry::Big(b)) => self.intern_big(b.clone()),
            Some(ConstEntry::Vector(d)) => {
                let endian = src.vector_endian(id).expect("向量常量与端序同生");
                self.intern_vector(d, endian)
            }
            Some(ConstEntry::Agg(a)) => {
                let (ty, children) = (a.ty, a.children.clone());
                let imported: Vec<ConstId> =
                    children.iter().map(|c| self.import(src, *c)).collect();
                self.intern_agg(ty, imported)
            }
            None => panic!("import：源池没有这个常量句柄 {id:?}"),
        }
    }

    // ========================================================
    // 规模与遍历（二进制编码用）
    // ========================================================

    /// 四条通道的条目数。
    pub(crate) fn counts(&self) -> ChannelCounts {
        ChannelCounts {
            bits: self.bits.len(),
            big: self.bigs.len(),
            vector: self.vectors.len(),
            agg: self.aggs.len(),
        }
    }

    /// 位值通道按索引序遍历。
    pub(crate) fn bits_iter(&self) -> impl Iterator<Item = BitValue> + '_ {
        self.bits.iter().copied()
    }

    /// Big 通道按索引序遍历。
    pub(crate) fn big_iter(&self) -> impl Iterator<Item = &Big> + '_ {
        self.bigs.iter().map(|k| &k.0)
    }

    /// 向量通道按索引序遍历。
    pub(crate) fn vector_iter(&self) -> impl Iterator<Item = (&[u8], Endianness)> + '_ {
        self.vectors.iter().map(|v| (&v.data[..], v.endian))
    }

    /// 聚合通道按索引序遍历。
    pub(crate) fn agg_iter(&self) -> impl Iterator<Item = &AggConst> + '_ {
        self.aggs.iter()
    }

    /// 条目总数（四通道之和）。
    pub fn len(&self) -> usize {
        let c = self.counts();
        c.bits + c.big + c.vector + c.agg
    }

    /// 是否为空池。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 位值的**有符号**解释：按 `width` 截断后做算术右移式的符号扩展。
///
/// 池里存的是 [`canonical_int`] 规范形（符号已扩展到 128 位），但这里仍按 `width`
/// 重新解释——调用方拿到的永远是"这个位宽下的补码值"（`i32::MIN` ⇒ -2147483648，
/// 而不是一个巨大的正 u128）。
fn signed_of(bv: BitValue) -> i128 {
    let width = bv.width.clamp(1, 128);
    let low = mask_to_bits(bv.bits, width) as i128;
    if width == 128 {
        return low;
    }
    let shift = 128 - width;
    (low << shift) >> shift
}

/// 向量常量的去重键：字节 + 端序。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct VectorConst {
    data: Vec<u8>,
    endian: Endianness,
}

/// [`Big`] 的哈希包装（dashu 的 `Real` 不实现 `Hash`；实数用规范字符串求哈希）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct BigKey(Big);

impl std::hash::Hash for BigKey {
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
                state.write(r.to_string().as_bytes());
            }
        }
    }
}
