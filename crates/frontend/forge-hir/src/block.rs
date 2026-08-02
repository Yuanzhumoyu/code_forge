//! Block and Region management for the IR graph.
//!
//! A [`BlockData`] is a basic block in the control flow graph — a sequence
//! of nodes ending with a terminator. A [`Region`] is a collection of blocks
//! that form a sub-graph (used for composite brick instantiation).

use crate::graph::{GraphValue, NodeId};
use forge_ir::TypeId;
use smallvec::SmallVec;

// ============================================================
// BlockId
// ============================================================

/// Handle to a block in the IR graph.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BlockId(pub u32);

impl BlockId {
    /// Sentinel for an invalid/missing block.
    pub const INVALID: BlockId = BlockId(u32::MAX);
}

// ============================================================
// BlockData
// ============================================================

/// A basic block — a sequence of non-terminator nodes followed by a
/// single terminator node.
#[derive(Clone, Debug)]
pub struct BlockData {
    /// Block name (for debugging).
    pub name: String,
    /// Parameter types.
    pub params: SmallVec<[TypeId; 2]>,
    /// Parameter values (SSA values representing each parameter).
    pub param_values: SmallVec<[GraphValue; 2]>,
    /// Nodes in this block (in order).
    pub nodes: Vec<NodeId>,
    /// The terminator node (must have a control-flow OpTag).
    pub terminator: Option<NodeId>,
}

impl BlockData {
    /// Create a new block.
    pub fn new(
        id: BlockId,
        name: &str,
        param_tys: &[TypeId],
        param_values: Vec<GraphValue>,
    ) -> Self {
        let _ = id; // used for ValueDef::BlockParam
        Self {
            name: name.to_string(),
            params: param_tys.iter().copied().collect(),
            param_values: param_values.into_iter().collect(),
            nodes: Vec::new(),
            terminator: None,
        }
    }

    /// Check if this block has been terminated.
    pub fn is_terminated(&self) -> bool {
        self.terminator.is_some()
    }

    /// Number of nodes in this block (excluding terminator).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// All nodes including the terminator (if present).
    pub fn all_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes
            .iter()
            .copied()
            .chain(self.terminator.iter().copied())
    }
}

// ============================================================
// Region
// ============================================================

/// A region is a named sub-graph containing one or more blocks.
///
/// When a composite brick is instantiated, each of its regions is filled
/// with a concrete sub-graph provided by the caller.
#[derive(Clone, Debug)]
pub struct Region {
    /// Region name (e.g., "then_body", "body").
    pub name: String,
    /// The entry block of this region.
    pub entry: Option<BlockId>,
    /// All blocks in this region (may include blocks not directly referenced
    /// by the entry if the region contains internal control flow).
    pub blocks: Vec<BlockId>,
}

impl Region {
    /// Create a new empty region.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            entry: None,
            blocks: Vec::new(),
        }
    }

    /// Create a region with a given entry block.
    pub fn with_entry(name: &str, entry: BlockId) -> Self {
        Self {
            name: name.to_string(),
            entry: Some(entry),
            blocks: vec![entry],
        }
    }

    /// Add a block to this region.
    pub fn add_block(&mut self, block: BlockId) {
        if self.entry.is_none() {
            self.entry = Some(block);
        }
        self.blocks.push(block);
    }
}
