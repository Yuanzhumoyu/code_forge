//! Graph builder — convenience helpers for constructing IR graphs.
//!
//! These methods provide a more ergonomic API over the raw `IrGraph` methods.
//! They are used by the code generated from `define_lowering!` macro expansion.
//!
//! Users typically don't call these directly — the macro generates calls
//! to these methods.

use crate::atom::OpTag;
use crate::attr::{AttrValue, Symbol, sym_intern};
use crate::block::BlockId;
use crate::error::HirError;
use crate::graph::{GraphValue, IrGraph, NodeId};
use forge_ir::TypeId;
use std::collections::HashMap;

/// Extension methods for [`IrGraph`] providing common patterns.
impl IrGraph {
    /// Create a node and immediately append it to the current block.
    pub fn emit(
        &mut self,
        op: OpTag,
        operands: &[GraphValue],
        attrs: HashMap<Symbol, AttrValue>,
        result_tys: &[TypeId],
    ) -> Result<NodeId, HirError> {
        let node = self.create_node(op, operands, attrs, result_tys)?;
        self.append_to_current_block(node)?;
        Ok(node)
    }

    /// Create a node and immediately append it to the current block;
    /// then set it as the block's terminator.
    pub fn emit_terminator(
        &mut self,
        op: OpTag,
        operands: &[GraphValue],
        attrs: HashMap<Symbol, AttrValue>,
        result_tys: &[TypeId],
    ) -> Result<NodeId, HirError> {
        let node = self.create_node(op, operands, attrs, result_tys)?;
        let block = self
            .current_block()
            .ok_or_else(|| HirError::Internal("no current block".into()))?;
        self.append_to_block(block, node)?;
        self.set_terminator(block, node)?;
        Ok(node)
    }

    /// Create a constant i32 node and append it.
    pub fn iconst_i32(&mut self, value: i32) -> Result<GraphValue, HirError> {
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("value"), AttrValue::Int(value as i64));
        let node = self.emit(crate::atom::arith::iconst(), &[], attrs, &[TypeId::I32])?;
        Ok(self.node_results(node)[0])
    }

    /// Create a binary arithmetic node and append it.
    pub fn iadd(&mut self, lhs: GraphValue, rhs: GraphValue) -> Result<GraphValue, HirError> {
        let node = self.emit(
            crate::atom::arith::iadd(),
            &[lhs, rhs],
            HashMap::new(),
            &[TypeId::I32],
        )?;
        Ok(self.node_results(node)[0])
    }

    /// Create a subtraction node and append it.
    pub fn isub(&mut self, lhs: GraphValue, rhs: GraphValue) -> Result<GraphValue, HirError> {
        let node = self.emit(
            crate::atom::arith::isub(),
            &[lhs, rhs],
            HashMap::new(),
            &[TypeId::I32],
        )?;
        Ok(self.node_results(node)[0])
    }

    /// Create a multiplication node and append it.
    pub fn imul(&mut self, lhs: GraphValue, rhs: GraphValue) -> Result<GraphValue, HirError> {
        let node = self.emit(
            crate::atom::arith::imul(),
            &[lhs, rhs],
            HashMap::new(),
            &[TypeId::I32],
        )?;
        Ok(self.node_results(node)[0])
    }

    /// Create a signed division node and append it.
    pub fn sdiv(&mut self, lhs: GraphValue, rhs: GraphValue) -> Result<GraphValue, HirError> {
        let node = self.emit(
            crate::atom::arith::sdiv(),
            &[lhs, rhs],
            HashMap::new(),
            &[TypeId::I32],
        )?;
        Ok(self.node_results(node)[0])
    }

    /// Create a signed remainder node and append it.
    pub fn srem(&mut self, lhs: GraphValue, rhs: GraphValue) -> Result<GraphValue, HirError> {
        let node = self.emit(
            crate::atom::arith::srem(),
            &[lhs, rhs],
            HashMap::new(),
            &[TypeId::I32],
        )?;
        Ok(self.node_results(node)[0])
    }

    /// Create a load node (dereference a pointer) and append it.
    /// `ptr` is the address to load from, `ty` is the type of the loaded value.
    pub fn load(&mut self, ptr: GraphValue, ty: TypeId) -> Result<GraphValue, HirError> {
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("ty"), AttrValue::Type(ty));
        let node = self.emit(crate::atom::mem::load(), &[ptr], attrs, &[ty])?;
        Ok(self.node_results(node)[0])
    }

    /// Create a store node (write value to pointer) and append it.
    pub fn store(&mut self, value: GraphValue, ptr: GraphValue) -> Result<(), HirError> {
        self.emit(
            crate::atom::mem::store(),
            &[value, ptr],
            HashMap::new(),
            &[],
        )?;
        Ok(())
    }

    /// Emit a `mem.stack_addr` node for the given frame offset.
    pub fn stack_addr(&mut self, offset: i32) -> Result<GraphValue, HirError> {
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("offset"), AttrValue::Int(offset as i64));
        let node = self.emit(crate::atom::mem::stack_addr(), &[], attrs, &[TypeId::PTR])?;
        Ok(self.node_results(node)[0])
    }

    /// Allocate a local stack slot at `offset` and return its pointer.
    ///
    /// Emits a real `mem.stack_addr` node. (The old implementation built the
    /// address from `iconst(0) + iconst(offset) + iadd`, which lowered to
    /// constant arithmetic instead of a stack-address operation.)
    pub fn alloc_slot(&mut self, offset: i32) -> Result<GraphValue, HirError> {
        self.stack_addr(offset)
    }

    /// Create an integer comparison node (produces i1/BOOL) and append it.
    pub fn icmp(
        &mut self,
        cc: forge_ir::IntCC,
        lhs: GraphValue,
        rhs: GraphValue,
    ) -> Result<GraphValue, HirError> {
        let cond_str = match cc {
            forge_ir::IntCC::Equal => "eq",
            forge_ir::IntCC::NotEqual => "ne",
            forge_ir::IntCC::SignedLessThan => "slt",
            forge_ir::IntCC::SignedGreaterThan => "sgt",
            forge_ir::IntCC::SignedLessThanOrEqual => "sle",
            forge_ir::IntCC::SignedGreaterThanOrEqual => "sge",
            forge_ir::IntCC::UnsignedLessThan => "ult",
            forge_ir::IntCC::UnsignedGreaterThan => "ugt",
            forge_ir::IntCC::UnsignedLessThanOrEqual => "ule",
            forge_ir::IntCC::UnsignedGreaterThanOrEqual => "uge",
        };
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("cond"), AttrValue::str(cond_str));
        let node = self.emit(
            crate::atom::arith::icmp(),
            &[lhs, rhs],
            attrs,
            &[TypeId::BOOL],
        )?;
        Ok(self.node_results(node)[0])
    }

    /// Normalize a value to a boolean (i1): values already typed BOOL pass
    /// through unchanged; everything else is compared NotEqual to zero.
    fn normalize_bool(&mut self, v: GraphValue) -> Result<GraphValue, HirError> {
        if self.value_type(v) == Some(TypeId::BOOL) {
            return Ok(v);
        }
        let zero = self.iconst_i32(0)?;
        self.icmp(forge_ir::IntCC::NotEqual, v, zero)
    }

    /// Create a bitwise-and node and append it.
    pub fn band(&mut self, lhs: GraphValue, rhs: GraphValue) -> Result<GraphValue, HirError> {
        let node = self.emit(
            crate::atom::arith::band(),
            &[lhs, rhs],
            HashMap::new(),
            &[TypeId::I32],
        )?;
        Ok(self.node_results(node)[0])
    }

    /// Create a bitwise-or node and append it.
    pub fn bor(&mut self, lhs: GraphValue, rhs: GraphValue) -> Result<GraphValue, HirError> {
        let node = self.emit(
            crate::atom::arith::bor(),
            &[lhs, rhs],
            HashMap::new(),
            &[TypeId::I32],
        )?;
        Ok(self.node_results(node)[0])
    }

    /// Create a bitwise-xor node and append it.
    pub fn bxor(&mut self, lhs: GraphValue, rhs: GraphValue) -> Result<GraphValue, HirError> {
        let node = self.emit(
            crate::atom::arith::bxor(),
            &[lhs, rhs],
            HashMap::new(),
            &[TypeId::I32],
        )?;
        Ok(self.node_results(node)[0])
    }

    /// Create a bitwise-not node and append it.
    pub fn bnot(&mut self, val: GraphValue) -> Result<GraphValue, HirError> {
        let node = self.emit(
            crate::atom::arith::bnot(),
            &[val],
            HashMap::new(),
            &[TypeId::I32],
        )?;
        Ok(self.node_results(node)[0])
    }

    /// Create a shift-left node and append it.
    pub fn ishl(&mut self, lhs: GraphValue, rhs: GraphValue) -> Result<GraphValue, HirError> {
        let node = self.emit(
            crate::atom::arith::ishl(),
            &[lhs, rhs],
            HashMap::new(),
            &[TypeId::I32],
        )?;
        Ok(self.node_results(node)[0])
    }

    /// Create an arithmetic shift-right node and append it.
    pub fn sshr(&mut self, lhs: GraphValue, rhs: GraphValue) -> Result<GraphValue, HirError> {
        let node = self.emit(
            crate::atom::arith::sshr(),
            &[lhs, rhs],
            HashMap::new(),
            &[TypeId::I32],
        )?;
        Ok(self.node_results(node)[0])
    }

    /// Create a sign-extension node and append it.
    pub fn sextend(&mut self, val: GraphValue, to: TypeId) -> Result<GraphValue, HirError> {
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("to"), AttrValue::Type(to));
        let node = self.emit(crate::atom::arith::sextend(), &[val], attrs, &[to])?;
        Ok(self.node_results(node)[0])
    }

    /// Create a branch terminator.
    pub fn emit_branch(
        &mut self,
        cond: GraphValue,
        then_block: BlockId,
        else_block: BlockId,
    ) -> Result<NodeId, HirError> {
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("then_block"), AttrValue::BlockId(then_block));
        attrs.insert(sym_intern("else_block"), AttrValue::BlockId(else_block));
        self.emit_terminator(crate::atom::cf::branch(), &[cond], attrs, &[])
    }

    /// Create a jump terminator.
    pub fn emit_jump(&mut self, target: BlockId) -> Result<NodeId, HirError> {
        let mut attrs = HashMap::new();
        attrs.insert(sym_intern("target"), AttrValue::BlockId(target));
        self.emit_terminator(crate::atom::cf::jump(), &[], attrs, &[])
    }

    /// Create a return terminator.
    pub fn emit_ret(&mut self, value: GraphValue) -> Result<NodeId, HirError> {
        self.emit_terminator(crate::atom::cf::ret(), &[value], HashMap::new(), &[])
    }

    /// Build a structurizer: create a new block, switch to it, run the body,
    /// then switch back to the original block.
    pub fn in_block<R>(
        &mut self,
        params: &[(TypeId, &str)],
        body: impl FnOnce(&mut Self, BlockId) -> Result<R, HirError>,
    ) -> Result<(BlockId, R), HirError> {
        let prev = self.current_block();
        let block = self.create_block(params);
        self.set_current_block(block);
        let result = body(self, block)?;
        if let Some(p) = prev {
            self.set_current_block(p);
        }
        Ok((block, result))
    }

    /// Build an if/else structurizer.
    ///
    /// Creates then_block, else_block (optional), and merge_block.
    /// Emits the branch in the current block.
    /// Runs then_body in then_block, else_body in else_block.
    /// Both branches jump to merge_block automatically.
    /// Returns the merge block.
    pub fn build_if_else(
        &mut self,
        cond: GraphValue,
        then_body: impl FnOnce(&mut Self) -> Result<(), HirError>,
        else_body: Option<impl FnOnce(&mut Self) -> Result<(), HirError>>,
    ) -> Result<BlockId, HirError> {
        // Normalize the condition to a boolean (i1): integer values are
        // compared NotEqual to zero, matching direct-codegen semantics.
        let cond = self.normalize_bool(cond)?;
        let then_block = self.create_block(&[]);
        let has_else = else_body.is_some();
        let else_block = if has_else {
            self.create_block(&[])
        } else {
            BlockId::INVALID
        };
        let merge_block = self.create_block(&[]);

        // Emit branch in current block
        self.emit_branch(
            cond,
            then_block,
            if has_else { else_block } else { merge_block },
        )?;

        // Then body
        self.set_current_block(then_block);
        then_body(self)?;
        if !self.has_terminator(then_block) {
            self.emit_jump(merge_block)?;
        }

        // Else body
        if has_else {
            self.set_current_block(else_block);
            else_body.unwrap()(self)?;
            if !self.has_terminator(else_block) {
                self.emit_jump(merge_block)?;
            }
        }

        // Continue at merge
        self.set_current_block(merge_block);
        Ok(merge_block)
    }

    /// Build a while loop structurizer.
    ///
    /// Creates cond_block, body_block, exit_block.
    /// Jumps from current block to cond_block.
    /// cond_block: evaluates condition, branches to body_block or exit_block.
    /// body_block: runs body, jumps back to cond_block.
    /// Returns (cond_block, exit_block) for break/continue resolution.
    pub fn build_while_loop(
        &mut self,
        cond_fn: impl FnOnce(&mut Self) -> Result<GraphValue, HirError>,
        body_fn: impl FnOnce(&mut Self) -> Result<(), HirError>,
    ) -> Result<(BlockId, BlockId), HirError> {
        let cond_block = self.create_block(&[]);
        let body_block = self.create_block(&[]);
        let exit_block = self.create_block(&[]);

        // Jump to condition
        self.emit_jump(cond_block)?;

        // Condition block
        self.set_current_block(cond_block);
        let cond_val = cond_fn(self)?;
        let cond_val = self.normalize_bool(cond_val)?;
        self.emit_branch(cond_val, body_block, exit_block)?;

        // Body block
        self.set_current_block(body_block);
        body_fn(self)?;
        if !self.has_terminator(body_block) {
            self.emit_jump(cond_block)?;
        }

        // Continue at exit
        self.set_current_block(exit_block);
        Ok((cond_block, exit_block))
    }

    /// Build a for loop structurizer.
    ///
    /// Layout: `init` → cond_block → (branch body/exit) → body → update → cond_block.
    /// `cond_fn` returning `None` means "no condition" (always enter body).
    /// The `update` clause runs only when the body did not terminate early.
    /// Returns (cond_block, exit_block) for break/continue resolution.
    pub fn build_for_loop(
        &mut self,
        init: impl FnOnce(&mut Self) -> Result<(), HirError>,
        cond_fn: impl FnOnce(&mut Self) -> Result<Option<GraphValue>, HirError>,
        update: impl FnOnce(&mut Self) -> Result<(), HirError>,
        body_fn: impl FnOnce(&mut Self) -> Result<(), HirError>,
    ) -> Result<(BlockId, BlockId), HirError> {
        init(self)?;

        let cond_block = self.create_block(&[]);
        let body_block = self.create_block(&[]);
        let exit_block = self.create_block(&[]);

        // Jump to condition
        self.emit_jump(cond_block)?;

        // Condition block
        self.set_current_block(cond_block);
        if let Some(cond) = cond_fn(self)? {
            let cond = self.normalize_bool(cond)?;
            self.emit_branch(cond, body_block, exit_block)?;
        } else {
            self.emit_jump(body_block)?;
        }

        // Body block: body then update, jump back to condition
        self.set_current_block(body_block);
        body_fn(self)?;
        if !self.has_terminator(body_block) {
            update(self)?;
            self.emit_jump(cond_block)?;
        }

        // Continue at exit
        self.set_current_block(exit_block);
        Ok((cond_block, exit_block))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emit_iconst() {
        let mut graph = IrGraph::new();
        graph.create_block(&[]);
        let v = graph.iconst_i32(42).unwrap();
        assert!(graph.value_type(v).is_some());
    }

    #[test]
    fn test_build_if_else() {
        let mut graph = IrGraph::new();
        graph.create_block(&[]);
        let cond = graph.iconst_i32(1).unwrap();

        let merge = graph
            .build_if_else(
                cond,
                |g| {
                    g.iconst_i32(10)?;
                    Ok(())
                },
                Some(|g: &mut IrGraph| {
                    g.iconst_i32(20)?;
                    Ok(())
                }),
            )
            .unwrap();

        assert!(graph.block(merge).is_some());
    }

    #[test]
    fn test_build_while_loop() {
        let mut graph = IrGraph::new();
        graph.create_block(&[]);

        let (cond_blk, exit_blk) = graph
            .build_while_loop(
                |g| g.iconst_i32(1),
                |g| {
                    g.iconst_i32(0)?;
                    Ok(())
                },
            )
            .unwrap();

        assert!(graph.block(cond_blk).is_some());
        assert!(graph.block(exit_blk).is_some());
    }

    #[test]
    fn test_build_for_loop() {
        let mut graph = IrGraph::new();
        graph.create_block(&[]);

        let (cond_blk, exit_blk) = graph
            .build_for_loop(
                |g| {
                    g.iconst_i32(0)?;
                    Ok(())
                },
                |g| Ok(Some(g.iconst_i32(1)?)),
                |g| {
                    g.iconst_i32(2)?;
                    Ok(())
                },
                |g| {
                    g.iconst_i32(3)?;
                    Ok(())
                },
            )
            .unwrap();

        assert!(graph.block(cond_blk).is_some());
        assert!(graph.block(exit_blk).is_some());
        // entry + cond + body + exit
        assert_eq!(graph.block_count(), 4);
    }

    #[test]
    fn test_build_for_loop_no_cond() {
        let mut graph = IrGraph::new();
        graph.create_block(&[]);

        let (_, exit_blk) = graph
            .build_for_loop(
                |_| Ok(()),
                |_| Ok(None), // no condition -> always enter body
                |_| Ok(()),
                |_| Ok(()),
            )
            .unwrap();
        assert!(graph.block(exit_blk).is_some());
    }

    #[test]
    fn test_normalize_bool_passes_bool_through() {
        let mut graph = IrGraph::new();
        graph.create_block(&[]);
        let zero = graph.iconst_i32(0).unwrap();
        let one = graph.iconst_i32(1).unwrap();
        let cmp = graph.icmp(forge_ir::IntCC::NotEqual, one, zero).unwrap();
        assert_eq!(graph.value_type(cmp), Some(TypeId::BOOL));
        // BOOL values pass through normalize_bool unchanged: no extra nodes.
        let before = graph.node_count();
        let _ = graph.normalize_bool(cmp).unwrap();
        assert_eq!(graph.node_count(), before);
    }

    #[test]
    fn test_normalize_bool_emits_icmp_for_int() {
        let mut graph = IrGraph::new();
        graph.create_block(&[]);
        let v = graph.iconst_i32(5).unwrap();
        let before = graph.node_count();
        let b = graph.normalize_bool(v).unwrap();
        assert_eq!(graph.node_count(), before + 2, "iconst(0) + icmp");
        assert_eq!(graph.value_type(b), Some(TypeId::BOOL));
    }

    #[test]
    fn test_stack_addr_emits_real_node() {
        let mut graph = IrGraph::new();
        graph.create_block(&[]);
        let addr = graph.alloc_slot(-8).unwrap();
        assert_eq!(graph.value_type(addr), Some(TypeId::PTR));
        // alloc_slot must emit a single mem.stack_addr node, not iconst+iadd.
        let node_id = match graph.value_def(addr).unwrap() {
            crate::graph::ValueDef::NodeResult(nid, 0) => *nid,
            other => panic!("expected node result, got {:?}", other),
        };
        assert_eq!(
            *graph.node_op(node_id).unwrap(),
            crate::atom::mem::stack_addr()
        );
        assert_eq!(graph.node_operands(node_id).len(), 0);
    }

    #[test]
    fn test_load_result_type_matches_ty() {
        let mut graph = IrGraph::new();
        graph.create_block(&[]);
        let addr = graph.alloc_slot(0).unwrap();
        let v = graph.load(addr, TypeId::I32).unwrap();
        assert_eq!(graph.value_type(v), Some(TypeId::I32));
        let v64 = graph.load(addr, TypeId::I64).unwrap();
        assert_eq!(graph.value_type(v64), Some(TypeId::I64));
    }

    #[test]
    fn test_icmp_unsigned_mapping() {
        let mut graph = IrGraph::new();
        graph.create_block(&[]);
        let a = graph.iconst_i32(1).unwrap();
        let b = graph.iconst_i32(2).unwrap();
        let cmp = graph.icmp(forge_ir::IntCC::UnsignedLessThan, a, b).unwrap();
        let node_id = match graph.value_def(cmp).unwrap() {
            crate::graph::ValueDef::NodeResult(nid, 0) => *nid,
            other => panic!("expected node result, got {:?}", other),
        };
        let attr = graph.node_attr(node_id, "cond").unwrap().as_str().unwrap();
        assert_eq!(crate::attr::sym_lookup(attr), "ult");
    }
}
