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

use super::entity::TypeId;
use super::entity::Value;
use super::string_pool::InternedStr;
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
    String(InternedStr),
    /// Unsigned integer.
    Uint(u64),
    /// Signed integer.
    Int(i64),
    /// Type reference.
    Type(TypeId),
    /// SSA value reference.
    Value(Value),
    /// Reference to another metadata node.
    Node(MetadataId),
}

impl MetadataValue {
    pub fn as_uint(&self) -> Option<u64> {
        match self {
            MetadataValue::Uint(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            MetadataValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<InternedStr> {
        match self {
            MetadataValue::String(s) => Some(*s),
            _ => None,
        }
    }

    pub fn as_node(&self) -> Option<MetadataId> {
        match self {
            MetadataValue::Node(id) => Some(*id),
            _ => None,
        }
    }
}

// ============================================================
// MetadataKind
// ============================================================

/// Well-known metadata kind identifiers.
///
/// These are the standard LLVM metadata kinds used for optimization
/// hints and debug information.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    Custom(InternedStr),
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
            MetadataKind::Custom(_) => "custom",
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
}

/// A metadata node — a tuple of metadata values.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MetadataNode {
    /// A single metadata value.
    Leaf(MetadataValue),
    /// A tuple of values: !{val1, val2, ...}
    Tuple(SmallVec<[MetadataValue; 4]>),
    /// A named node: !name(val1, val2, ...)
    Named {
        name: InternedStr,
        ops: SmallVec<[MetadataValue; 4]>,
    },
}

impl MetadataStore {
    /// Create an empty metadata store.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            dedup: HashMap::new(),
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

    /// Create a leaf node with a string value.
    pub fn string_node(&mut self, s: InternedStr) -> MetadataId {
        self.intern(MetadataNode::Leaf(MetadataValue::String(s)))
    }

    /// Create a tuple node.
    pub fn tuple_node(&mut self, vals: SmallVec<[MetadataValue; 4]>) -> MetadataId {
        self.intern(MetadataNode::Tuple(vals))
    }

    /// Create a named node.
    pub fn named_node(
        &mut self,
        name: InternedStr,
        ops: SmallVec<[MetadataValue; 4]>,
    ) -> MetadataId {
        self.intern(MetadataNode::Named { name, ops })
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

    /// Create a TBAA root node.
    pub fn tbaa_root(&mut self, name: InternedStr) -> MetadataId {
        self.tuple_node(smallvec::smallvec![MetadataValue::String(name),])
    }

    /// Create a TBAA access tag: !{!type_node, !type_node, i64 offset}
    pub fn tbaa_tag(
        &mut self,
        base_type: MetadataId,
        access_type: MetadataId,
        offset: i64,
    ) -> MetadataId {
        self.tuple_node(smallvec::smallvec![
            MetadataValue::Node(base_type),
            MetadataValue::Node(access_type),
            MetadataValue::Int(offset),
        ])
    }

    /// Create a branch_weights metadata node: !{!"branch_weights", i32 N, i32 M, ...}
    pub fn branch_weights(&mut self, weights: &[u32]) -> MetadataId {
        let mut vals: SmallVec<[MetadataValue; 4]> = SmallVec::new();
        vals.push(MetadataValue::String(InternedStr(0))); // placeholder, caller should intern "branch_weights"
        for &w in weights {
            vals.push(MetadataValue::Uint(w as u64));
        }
        self.tuple_node(vals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_store_basic() {
        let mut store = MetadataStore::new();
        let id1 = store.string_node(InternedStr(0));
        let id2 = store.string_node(InternedStr(0));
        assert_eq!(id1, id2); // dedup: same content → same ID
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_metadata_tuple_dedup() {
        let mut store = MetadataStore::new();
        let vals = smallvec::smallvec![MetadataValue::Uint(42), MetadataValue::Int(-1),];
        let id1 = store.tuple_node(vals.clone());
        let id2 = store.tuple_node(vals);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_metadata_tbaa_tag() {
        let mut store = MetadataStore::new();
        let root = store.tbaa_root(InternedStr(0));
        let tag = store.tbaa_tag(root, root, 0);
        assert!(tag.0 > 0);
    }

    #[test]
    fn test_metadata_named_node() {
        let mut store = MetadataStore::new();
        let ops = smallvec::smallvec![MetadataValue::Uint(1), MetadataValue::Uint(2),];
        let id = store.named_node(InternedStr(0), ops);
        match store.get(id) {
            MetadataNode::Named { .. } => {}
            _ => panic!("expected Named node"),
        }
    }
}
