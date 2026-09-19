//! IR metadata system — attachable key-value annotations.
//!
//! Metadata nodes form a parallel DAG outside the SSA value graph.
//! They carry optimization hints (TBAA, alias info, branch weights,
//! loop info) and debug information (DILocation, DISubprogram).
//!
//! # Design (LLVM-inspired)
//! - `Metadata` enum: values in the metadata DAG
//! - `MetadataId`: u32 handle into MetadataStore
//! - `MetadataStore`: interning deduplication table
//! - Attached to Instruction, Function, and Module

use crate::util::imm_str::ImmStr;
use smallvec::SmallVec;
use std::collections::HashMap;

// ============================================================
// MetadataId
// ============================================================

/// Handle to a metadata node in the MetadataStore.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct MetadataId(pub u32);

// ============================================================
// MetadataValue
// ============================================================

/// A value in the metadata DAG.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MetadataValue {
    /// String value.
    String(ImmStr),
    /// Unsigned integer.
    Uint(u64),
    /// Signed integer.
    Int(i64),
    /// 超 i64 范围大整数（第二十三轮 Big 化——原文字符串,display 精确输出）。
    IntBig(ImmStr),
    /// Floating-point value（以 bits 存储——保持 Eq/Hash）。
    Float(u64),
    /// Null literal（metadata 节点中的 `null`）。
    Null,
    /// Reference to another metadata node.
    Node(MetadataId),
    /// named 节点 key:value 字段（第二十一轮——DI 校验与 display roundtrip
    /// 需还原 `key: value` 形态）
    Field(ImmStr, Box<MetadataValue>),
}

// ============================================================
// MetadataKind
// ============================================================

/// Well-known metadata kind identifiers.
///
/// These are the standard LLVM metadata kinds used for optimization
/// hints and debug information.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MetadataKind {
    /// Debug location (DILocation).
    DebugLoc,
    /// Type-Based Alias Analysis.
    TBAA,
    /// TBAA struct path.
    TBAAStruct,
    /// Alias scope for memory operations.
    AliasScope,
    /// No-alias guarantee.
    NoAlias,
    /// Integer range: [lo, hi].
    Range,
    /// Non-null pointer guarantee.
    NonNull,
    /// Required alignment.
    Align,
    /// Dereferenceable bytes.
    Dereferenceable,
    /// No undef/poison guarantee.
    NoUndef,
    /// Loop metadata (unroll hint, etc.).
    Loop,
    /// Branch weights / profile data.
    Prof,
    /// Floating-point math flags.
    FpMath,
    /// Custom named kind (for user-defined metadata).
    Custom(ImmStr),
}

impl MetadataKind {
    /// Get the canonical string name for this kind.
    pub fn name(&self) -> &str {
        match self {
            MetadataKind::DebugLoc => "dbg",
            MetadataKind::TBAA => "tbaa",
            MetadataKind::TBAAStruct => "tbaa.struct",
            MetadataKind::AliasScope => "alias.scope",
            MetadataKind::NoAlias => "noalias",
            MetadataKind::Range => "range",
            MetadataKind::NonNull => "nonnull",
            MetadataKind::Align => "align",
            MetadataKind::Dereferenceable => "dereferenceable",
            MetadataKind::NoUndef => "noundef",
            MetadataKind::Loop => "loop",
            MetadataKind::Prof => "prof",
            MetadataKind::FpMath => "fpmath",
            // 自定义 kind 保留原始名（第二十九轮:原固定输出 "custom"
            // 丢失 !associated/!foo 等自定义 kind,roundtrip 不等）
            MetadataKind::Custom(s) => s.as_str(),
        }
    }
}

// ============================================================
// AttachedMetadata
// ============================================================

/// A (kind, node) pair attached to an instruction or function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachedMetadata {
    pub kind: MetadataKind,
    pub node: MetadataId,
}

// ============================================================
// MetadataStore
// ============================================================

/// Central metadata storage with interning deduplication.
///
/// Owned by `Module` (shared across functions). Metadata nodes
/// are interned by their content for deduplication.
#[derive(Clone, Debug, Default)]
pub struct MetadataStore {
    /// Metadata nodes indexed by MetadataId.
    nodes: Vec<MetadataNode>,
    /// Deduplication map: node → MetadataId.
    dedup: HashMap<MetadataNode, MetadataId>,
    /// 命名 metadata（LLVM：`!t = !{...}` → 名字 → id）。
    names: HashMap<ImmStr, MetadataId>,
}

/// A metadata node — a tuple of metadata values.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MetadataNode {
    /// A single metadata value.
    Leaf(MetadataValue),
    /// A tuple of values: !{val1, val2, ...}
    Tuple(SmallVec<[MetadataValue; 4]>),
    /// A named node: !name(val1, val2, ...) — distinct 前缀标志
    ///（第二十九轮:parse 时 Distinct 展开后标志丢失,display 无法还原
    /// `distinct !DICompileUnit(...)`,reparse 时 DI 校验拒绝）
    Named {
        name: ImmStr,
        ops: SmallVec<[MetadataValue; 4]>,
        distinct: bool,
    },
    /// **空洞占位**：解析器为显式 `!N` id 预留槽位时补齐的空位（文本里 `!15` 与 `!19`
    /// 并存 ⇒ 16..18 是空洞）。与用户写的空 tuple（`!0 = !{}` ⇒ [`MetadataNode::Tuple`]）
    /// **必须区分**：后者是显式定义、打印时必须原样输出（否则往返丢失），前者无人引用时
    /// 不打印（否则 id 会随解析顺序变化，往返不幂等）。
    Placeholder,
}

impl MetadataStore {
    /// Create an empty metadata store.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            dedup: HashMap::new(),
            names: HashMap::new(),
        }
    }

    /// 二进制序列化层：命名表，**按名字节序排序**（`HashMap` 迭代序不确定，
    /// 直接落盘会让字节流不确定）。
    pub(crate) fn names_sorted(&self) -> Vec<(ImmStr, MetadataId)> {
        let mut out: Vec<(ImmStr, MetadataId)> =
            self.names.iter().map(|(k, v)| (k.clone(), *v)).collect();
        out.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        out
    }

    /// Intern a metadata node and return its ID.
    pub fn intern(&mut self, node: MetadataNode) -> MetadataId {
        if let Some(&existing) = self.dedup.get(&node) {
            return existing;
        }
        let id = MetadataId(self.nodes.len() as u32);
        self.dedup.insert(node.clone(), id);
        self.nodes.push(node);
        id
    }

    /// 按名查命名 metadata。
    pub fn lookup_named(&self, name: &str) -> Option<MetadataId> {
        self.names.get(&ImmStr::from(name)).copied()
    }

    /// 注册命名 metadata（`!t = !{...}` → 名字 → id）。
    pub fn define_named(&mut self, name: &str, id: MetadataId) {
        self.names.insert(ImmStr::from(name), id);
    }

    /// 反查：id → 命名（display 还原 `!t`；未命名返回 None）。
    ///
    /// **确定性**：`names` 是 `HashMap`，"找第一个"会随实例的随机种子变化——
    /// 一个 id 若被多个名字指向（`!foo` 与 `!\23pragma` 内容相同时
    /// `insert_at`/`intern` 会去重成同一个节点），`name_of` 的输出就会时好时坏，
    /// display 也随之不确定（2026-09-19 二进制语料往返测试抓到）。这里取**字节序
    /// 最小**的名字，保证同输入同输出。
    ///
    /// 已知限制（**既有**，非本轮引入）：display 每个节点只打印一个名字，
    /// 因此"一个节点多个别名"的文本会丢名字；要修得让打印器遍历全部别名。
    pub fn name_of(&self, id: MetadataId) -> Option<String> {
        let mut best: Option<&ImmStr> = None;
        for (name, target) in &self.names {
            if *target == id && best.is_none_or(|b| name.as_str() < b.as_str()) {
                best = Some(name);
            }
        }
        best.map(|k| k.to_string())
    }

    /// 该 id 的**全部**名字，按字节序排序（诊断/测试用）。
    pub fn names_of(&self, id: MetadataId) -> Vec<String> {
        let mut out: Vec<String> = self
            .names
            .iter()
            .filter(|(_, v)| **v == id)
            .map(|(k, _)| k.to_string())
            .collect();
        out.sort();
        out
    }

    /// 遍历全部节点（display 序列化用）。
    pub fn iter(&self) -> impl Iterator<Item = (MetadataId, &MetadataNode)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (MetadataId(i as u32), n))
    }

    /// 按显式 id 插入（LLVM：`!5 = ...` 的 id 是文本数字；0..id 补空占位）。
    ///
    /// **去重表保持自洽**：覆盖槽位时先摘掉旧内容的映射（否则 `intern` 会命中
    /// 一个已被覆盖的节点）；新内容只在"尚无映射"时登记 —— 同一内容若已存在于
    /// 另一个 id，则保留先注册的映射（文本 id 保真优先，这是解析器的契约）。
    /// 历史实现两条路径（`intern` 去重 / `insert_at` 直插）互不更新 →
    /// 同内容可产生两个 id 且去重表指向被覆盖的节点。
    pub fn insert_at(&mut self, id: MetadataId, node: MetadataNode) {
        let old = self.nodes.get(id.0 as usize).cloned();
        if self.nodes.len() <= id.0 as usize {
            self.nodes
                // 空洞补 Placeholder（与显式 !{} 区分，见 MetadataNode::Placeholder）
                .resize(id.0 as usize + 1, MetadataNode::Placeholder);
        }
        if let Some(old) = old
            && self.dedup.get(&old) == Some(&id)
        {
            self.dedup.remove(&old);
        }
        self.dedup.entry(node.clone()).or_insert(id);
        self.nodes[id.0 as usize] = node;
    }

    /// 二进制序列化层：**按原样**把节点放到下一个 id（不做去重查询/不重排）。
    ///
    /// 为什么不能用 `intern` 回放：显式 `!N` 编号的模块经
    /// [`MetadataStore::insert_at`] 预分配槽位，arena 里**允许出现内容相同的两条**
    /// （各自的 id 都有引用者），`intern` 会把后一条折叠掉 ⇒ id 整体错位。
    /// 去重表按"先出现者胜"补齐，与 `intern`/`insert_at` 的口径一致。
    pub(crate) fn push_verbatim(&mut self, node: MetadataNode) -> MetadataId {
        let id = MetadataId(self.nodes.len() as u32);
        self.nodes.push(node.clone());
        self.dedup.entry(node).or_insert(id);
        id
    }

    /// Look up a metadata node by ID（越界返回 `None`——历史实现直接索引 panic）。
    pub fn get(&self, id: MetadataId) -> Option<&MetadataNode> {
        self.nodes.get(id.0 as usize)
    }

    /// Number of metadata nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_store_basic() {
        let store = MetadataStore::new();
        assert_eq!(store.len(), 0);
    }
}
