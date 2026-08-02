//! IR Graph — generic graph storage for the brick framework.
//!
//! `IrGraph` is the foundation (底板) of the brick framework. It manages:
//! - Nodes (operations in the graph)
//! - Blocks (basic blocks with instructions and terminators)
//! - Values (SSA values produced by nodes)
//!
//! `IrGraph` is operation-agnostic — it doesn't know what "iadd" or "branch"
//! means. It just stores nodes tagged with [`OpTag`] symbols and their
//! operands/attributes/results.

use crate::atom::OpTag;
use crate::attr::{AttrValue, Symbol, sym_intern};
use crate::block::{BlockData, BlockId};
use crate::error::HirError;
use forge_ir::TypeId;
use smallvec::SmallVec;
use std::collections::HashMap;

// ============================================================
// Entity handles
// ============================================================

/// Handle to a node in the IR graph.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(pub u32);

impl NodeId {
    /// Sentinel for an invalid/missing node.
    pub const INVALID: NodeId = NodeId(u32::MAX);
}

/// Handle to a value in the IR graph.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GraphValue(pub u32);

impl GraphValue {
    /// Sentinel for an invalid/missing value.
    pub const INVALID: GraphValue = GraphValue(u32::MAX);
}

// ============================================================
// NodeData
// ============================================================

/// A single node (operation) in the IR graph.
#[derive(Clone, Debug)]
pub struct NodeData {
    /// The symbolic operation tag (e.g., "arith.iadd").
    pub op: OpTag,
    /// Input value operands.
    pub operands: SmallVec<[GraphValue; 4]>,
    /// Compile-time attributes.
    pub attrs: HashMap<Symbol, AttrValue>,
    /// Output values produced by this node.
    pub results: SmallVec<[GraphValue; 2]>,
    /// Source location (optional, for debugging).
    pub loc: Option<forge_ir::debug_info::SourceLocation>,
}

// ============================================================
// ValueData
// ============================================================

/// Definition of a graph value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueDef {
    /// Produced as the `result_idx`-th result of a node.
    NodeResult(NodeId, u8),
    /// Block parameter (argument to a block).
    BlockParam(BlockId, u16),
}

/// Data for a single SSA value in the graph.
#[derive(Clone, Debug)]
pub struct GraphValueData {
    /// How this value is defined.
    pub def: ValueDef,
    /// The type of this value.
    pub ty: TypeId,
}

// ============================================================
// IrGraph
// ============================================================

/// The main IR graph — generic storage for all nodes, blocks, and values.
///
/// `IrGraph` is the "底板" (baseboard) that bricks plug into. It provides:
/// - `create_node()` — allocate a node with a given OpTag
/// - `create_block()` — allocate a basic block
/// - `create_value()` — allocate an SSA value
/// - Block cursor management — which block new instructions go into
///
/// The graph does NOT enforce any semantics on operations — that's the
/// responsibility of the brick definitions and the lowering registry.
#[derive(Clone, Debug)]
pub struct IrGraph {
    /// All nodes in the graph.
    nodes: Vec<NodeData>,
    /// All blocks in the graph.
    blocks: Vec<BlockData>,
    /// All values in the graph.
    values: Vec<GraphValueData>,
    /// Current block cursor (new nodes are appended here).
    current_block: Option<BlockId>,
    /// Name counter for anonymous blocks.
    block_counter: u32,
}

impl IrGraph {
    /// Create a new empty graph.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            blocks: Vec::new(),
            values: Vec::new(),
            current_block: None,
            block_counter: 0,
        }
    }

    // ── Node management ──

    /// Create a node with the given operation tag, operands, attributes,
    /// and result types.
    ///
    /// The node is NOT automatically appended to any block — call
    /// [`append_to_current_block`](Self::append_to_current_block) or
    /// [`append_to_block`](Self::append_to_block) to add it to a block's
    /// instruction sequence.
    pub fn create_node(
        &mut self,
        op: OpTag,
        operands: &[GraphValue],
        attrs: HashMap<Symbol, AttrValue>,
        result_tys: &[TypeId],
    ) -> Result<NodeId, HirError> {
        let node_id = NodeId(self.nodes.len() as u32);

        // Allocate result values
        let mut results = SmallVec::new();
        for (i, &ty) in result_tys.iter().enumerate() {
            let val = GraphValue(self.values.len() as u32);
            self.values.push(GraphValueData {
                def: ValueDef::NodeResult(node_id, i as u8),
                ty,
            });
            results.push(val);
        }

        self.nodes.push(NodeData {
            op,
            operands: operands.iter().copied().collect(),
            attrs,
            results: results.clone(),
            loc: None,
        });

        Ok(node_id)
    }

    /// Get a reference to a node's data.
    pub fn node(&self, id: NodeId) -> Option<&NodeData> {
        self.nodes.get(id.0 as usize)
    }

    /// Get a mutable reference to a node's data.
    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut NodeData> {
        self.nodes.get_mut(id.0 as usize)
    }

    /// Get the operation tag of a node.
    pub fn node_op(&self, id: NodeId) -> Option<&OpTag> {
        self.node(id).map(|n| &n.op)
    }

    /// Get the result values of a node.
    pub fn node_results(&self, id: NodeId) -> &[GraphValue] {
        self.node(id).map(|n| n.results.as_slice()).unwrap_or(&[])
    }

    /// Get the operands of a node.
    pub fn node_operands(&self, id: NodeId) -> &[GraphValue] {
        self.node(id).map(|n| n.operands.as_slice()).unwrap_or(&[])
    }

    /// Get an attribute of a node.
    pub fn node_attr(&self, id: NodeId, key: &str) -> Option<&AttrValue> {
        self.node(id)?.attrs.get(&sym_intern(key))
    }

    /// Total number of nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    // ── Value management ──

    /// Get a value's definition.
    pub fn value_def(&self, v: GraphValue) -> Option<&ValueDef> {
        self.values.get(v.0 as usize).map(|d| &d.def)
    }

    /// Get a value's type.
    pub fn value_type(&self, v: GraphValue) -> Option<TypeId> {
        self.values.get(v.0 as usize).map(|d| d.ty)
    }

    /// Total number of values.
    pub fn value_count(&self) -> usize {
        self.values.len()
    }

    // ── Block management ──

    /// Create a new block with optional parameters.
    ///
    /// Parameters are (TypeId, name) pairs. Block parameter values are
    /// automatically allocated.
    pub fn create_block(&mut self, params: &[(TypeId, &str)]) -> BlockId {
        let block_id = BlockId(self.blocks.len() as u32);

        let mut param_values = Vec::new();
        for (i, &(ty, _name)) in params.iter().enumerate() {
            let val = GraphValue(self.values.len() as u32);
            self.values.push(GraphValueData {
                def: ValueDef::BlockParam(block_id, i as u16),
                ty,
            });
            param_values.push(val);
        }

        let param_tys: Vec<TypeId> = params.iter().map(|(t, _)| *t).collect();

        let name = if params.is_empty() {
            self.block_counter += 1;
            format!("blk{}", self.block_counter - 1)
        } else {
            params
                .first()
                .map(|(_, n)| n.to_string())
                .unwrap_or_else(|| "entry".to_string())
        };

        self.blocks
            .push(BlockData::new(block_id, &name, &param_tys, param_values));

        // Auto-set as current block if this is the first block
        if self.current_block.is_none() {
            self.current_block = Some(block_id);
        }

        block_id
    }

    /// Get a reference to a block's data.
    pub fn block(&self, id: BlockId) -> Option<&BlockData> {
        self.blocks.get(id.0 as usize)
    }

    /// Get a mutable reference to a block's data.
    pub fn block_mut(&mut self, id: BlockId) -> Option<&mut BlockData> {
        self.blocks.get_mut(id.0 as usize)
    }

    /// Get block parameter values.
    pub fn block_params(&self, id: BlockId) -> Option<&[GraphValue]> {
        self.block(id).map(|b| b.param_values.as_slice())
    }

    /// Total number of blocks.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    // ── Block cursor ──

    /// Set the current block — new nodes appended via
    /// [`append_to_current_block`](Self::append_to_current_block) go here.
    pub fn set_current_block(&mut self, block: BlockId) {
        self.current_block = Some(block);
    }

    /// Get the current block.
    pub fn current_block(&self) -> Option<BlockId> {
        self.current_block
    }

    /// Append a node to the current block's instruction sequence.
    ///
    /// Returns an error if no current block is set.
    pub fn append_to_current_block(&mut self, node: NodeId) -> Result<(), HirError> {
        let block = self
            .current_block
            .ok_or_else(|| HirError::Internal("no current block set".into()))?;
        self.append_to_block(block, node)
    }

    /// Append a node to a specific block's instruction sequence.
    pub fn append_to_block(&mut self, block: BlockId, node: NodeId) -> Result<(), HirError> {
        let block_data = self
            .block_mut(block)
            .ok_or_else(|| HirError::Internal(format!("block {:?} not found", block.0)))?;

        if block_data.terminator.is_some() {
            return Err(HirError::AlreadyTerminated(block_data.name.clone()));
        }

        block_data.nodes.push(node);
        Ok(())
    }

    /// Set the terminator of a block.
    ///
    /// The terminator is just a node (with OpTag like "cf.branch", "cf.jump",
    /// "cf.ret") that is marked as the block's terminator. It should also be
    /// appended via [`append_to_block`](Self::append_to_block).
    pub fn set_terminator(&mut self, block: BlockId, node: NodeId) -> Result<(), HirError> {
        let block_data = self
            .block_mut(block)
            .ok_or_else(|| HirError::Internal(format!("block {:?} not found", block.0)))?;
        block_data.terminator = Some(node);
        Ok(())
    }

    /// Check if a block has a terminator.
    pub fn has_terminator(&self, block: BlockId) -> bool {
        self.block(block)
            .map(|b| b.terminator.is_some())
            .unwrap_or(true)
    }

    // ── Iteration ──

    /// Iterate over all nodes.
    pub fn iter_nodes(&self) -> impl Iterator<Item = (NodeId, &NodeData)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, nd)| (NodeId(i as u32), nd))
    }

    /// Iterate over all blocks.
    pub fn iter_blocks(&self) -> impl Iterator<Item = (BlockId, &BlockData)> {
        self.blocks
            .iter()
            .enumerate()
            .map(|(i, bd)| (BlockId(i as u32), bd))
    }

    /// Iterate over nodes in a specific block.
    pub fn block_nodes(&self, block: BlockId) -> impl Iterator<Item = NodeId> + '_ {
        struct BlockNodeIter<'a> {
            nodes: &'a [NodeId],
            idx: usize,
        }
        impl Iterator for BlockNodeIter<'_> {
            type Item = NodeId;
            fn next(&mut self) -> Option<Self::Item> {
                if self.idx < self.nodes.len() {
                    let n = self.nodes[self.idx];
                    self.idx += 1;
                    Some(n)
                } else {
                    None
                }
            }
        }
        let nodes = self.block(block).map(|b| b.nodes.as_slice()).unwrap_or(&[]);
        BlockNodeIter { nodes, idx: 0 }
    }
}

impl Default for IrGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atom::arith;

    #[test]
    fn test_create_node() {
        let iconst_tag = arith::iconst();
        let mut graph = IrGraph::new();
        let node = graph
            .create_node(iconst_tag, &[], HashMap::new(), &[TypeId::I32])
            .unwrap();
        assert_eq!(*graph.node_op(node).unwrap(), iconst_tag);
        assert_eq!(graph.node_results(node).len(), 1);
    }

    #[test]
    fn test_create_block() {
        let mut graph = IrGraph::new();
        let block = graph.create_block(&[(TypeId::I32, "x"), (TypeId::I32, "y")]);
        assert_eq!(graph.block_count(), 1);
        assert_eq!(graph.block_params(block).unwrap().len(), 2);
        assert_eq!(graph.current_block(), Some(block));
    }

    #[test]
    fn test_append_node_to_block() {
        let mut graph = IrGraph::new();
        let block = graph.create_block(&[]);

        let n1 = graph
            .create_node(arith::iconst(), &[], HashMap::new(), &[TypeId::I32])
            .unwrap();
        graph.append_to_current_block(n1).unwrap();

        let n2 = graph
            .create_node(arith::iadd(), &[], HashMap::new(), &[TypeId::I32])
            .unwrap();
        graph.append_to_current_block(n2).unwrap();

        let block_nodes: Vec<_> = graph.block_nodes(block).collect();
        assert_eq!(block_nodes, vec![n1, n2]);
    }

    #[test]
    fn test_terminator_prevents_append() {
        let mut graph = IrGraph::new();
        let block = graph.create_block(&[]);

        let ret_node = graph
            .create_node(crate::atom::cf::ret(), &[], HashMap::new(), &[])
            .unwrap();
        graph.append_to_current_block(ret_node).unwrap();
        graph.set_terminator(block, ret_node).unwrap();

        // Appending after termination should error
        let n = graph
            .create_node(arith::iconst(), &[], HashMap::new(), &[TypeId::I32])
            .unwrap();
        assert!(graph.append_to_current_block(n).is_err());
    }

    #[test]
    fn test_node_results_have_correct_types() {
        let mut graph = IrGraph::new();
        graph.create_block(&[]);

        let node = graph
            .create_node(
                arith::iconst(),
                &[],
                HashMap::new(),
                &[TypeId::I32, TypeId::I64],
            )
            .unwrap();

        let results = graph.node_results(node);
        assert_eq!(results.len(), 2);

        let v0 = graph.value_type(results[0]);
        let v1 = graph.value_type(results[1]);
        assert_eq!(v0, Some(TypeId::I32));
        assert_eq!(v1, Some(TypeId::I64));
    }
}
