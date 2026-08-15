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

use super::imm_str::ImmStr;
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
    pub fn name_of(&self, id: MetadataId) -> Option<String> {
        self.names
            .iter()
            .find(|(_, v)| **v == id)
            .map(|(k, _)| k.to_string())
    }

    /// 遍历全部节点（display 序列化用）。
    pub fn iter(&self) -> impl Iterator<Item = (MetadataId, &MetadataNode)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (MetadataId(i as u32), n))
    }

    /// 按显式 id 插入（LLVM：`!5 = ...` 的 id 是文本数字；0..id 补空占位）。
    pub fn insert_at(&mut self, id: MetadataId, node: MetadataNode) {
        if self.nodes.len() <= id.0 as usize {
            self.nodes
                .resize(id.0 as usize + 1, MetadataNode::Tuple(SmallVec::new()));
        }
        self.nodes[id.0 as usize] = node;
    }

    /// Look up a metadata node by ID.
    pub fn get(&self, id: MetadataId) -> &MetadataNode {
        &self.nodes[id.0 as usize]
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
