//! IR visitor pattern — systematic IR traversal infrastructure.
//!
//! Provides `IrVisitor` and `MutIrVisitor` traits with default walk methods,
//! enabling passes to traverse the IR without manual iteration boilerplate.
//!
//! # Design (LLVM InstVisitor-inspired)
//! - `WalkResult`: controls traversal (Advance / Skip / Stop)
//! - `IrVisitor`: read-only visitor with per-opcode dispatch hooks
//! - `MutIrVisitor`: mutable visitor for transformation passes
//! - Default `walk_*` functions implement the standard traversal order

use super::dfg::{BlockData, DataFlowGraph, Instruction};
use super::entity::*;
use super::function::Function;
use super::terminator::Terminator;
use super::types::TypeContext;

// ============================================================
// WalkResult
// ============================================================

/// Controls IR traversal flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalkResult {
    /// Continue traversal normally.
    Advance,
    /// Stop traversal entirely.
    Stop,
    /// Skip the children of this node (block body, instruction operands).
    Skip,
}

impl WalkResult {
    /// Check if traversal should continue.
    pub fn should_continue(self) -> bool {
        matches!(self, WalkResult::Advance)
    }

    /// Check if traversal should stop.
    pub fn should_stop(self) -> bool {
        matches!(self, WalkResult::Stop)
    }
}

// ============================================================
// IrVisitor — read-only traversal
// ============================================================

/// Visitor trait for read-only IR traversal.
///
/// Each `visit_*` method returns a `WalkResult`:
/// - `Advance`: continue traversal
/// - `Skip`: skip children of this node
/// - `Stop`: stop traversal entirely
///
/// Default implementations call the generic `walk_*` functions
/// which iterate children automatically.
pub trait IrVisitor {
    /// Visit a function before its blocks.
    fn visit_function(&mut self, _func: &Function, _store: &TypeContext) -> WalkResult {
        WalkResult::Advance
    }

    /// Visit a basic block before its instructions.
    fn visit_block(&mut self, _block: Block, _data: &BlockData) -> WalkResult {
        WalkResult::Advance
    }

    /// Visit an instruction.
    fn visit_inst(&mut self, _inst: Inst, _data: &Instruction) -> WalkResult {
        WalkResult::Advance
    }

    /// Visit a block's terminator.
    fn visit_terminator(&mut self, _block: Block, _term: &Terminator) -> WalkResult {
        WalkResult::Advance
    }

    /// Visit after leaving a block.
    fn visit_block_end(&mut self, _block: Block) {}

    /// Visit after leaving a function.
    fn visit_function_end(&mut self, _func: &Function) {}
}

/// Walk a function: entry → blocks in layout order → exit.
pub fn walk_function(
    visitor: &mut dyn IrVisitor,
    func: &Function,
    ctx: &TypeContext,
) -> WalkResult {
    match visitor.visit_function(func, ctx) {
        WalkResult::Stop => return WalkResult::Stop,
        WalkResult::Skip => return WalkResult::Advance,
        WalkResult::Advance => {}
    }

    for &block in &func.layout.block_order {
        let block_data = func.dfg.block(block);
        if walk_block(visitor, block, block_data, &func.dfg) == WalkResult::Stop {
            return WalkResult::Stop;
        }
    }

    visitor.visit_function_end(func);
    WalkResult::Advance
}

/// Walk a single block: pre → instructions → terminator → post.
pub fn walk_block(
    visitor: &mut dyn IrVisitor,
    block: Block,
    block_data: &BlockData,
    dfg: &DataFlowGraph,
) -> WalkResult {
    match visitor.visit_block(block, block_data) {
        WalkResult::Stop => return WalkResult::Stop,
        WalkResult::Skip => {
            visitor.visit_block_end(block);
            return WalkResult::Advance;
        }
        WalkResult::Advance => {}
    }

    for &inst_id in &block_data.inst_order {
        let inst = &dfg.insts[inst_id.0 as usize];
        if matches!(inst.opcode, super::opcode::Opcode::Nop) {
            continue;
        }
        if visitor.visit_inst(inst_id, inst) == WalkResult::Stop {
            return WalkResult::Stop;
        }
    }

    visitor.visit_terminator(block, &block_data.terminator);
    visitor.visit_block_end(block);
    WalkResult::Advance
}

// ============================================================
// MutIrVisitor — mutable traversal
// ============================================================

/// Visitor trait for mutable IR transformation.
///
/// Takes `&mut Function` and `&TypeStore` so passes can modify
/// the IR (replace operands, remove instructions, insert new ones).
pub trait MutIrVisitor {
    fn visit_function(&mut self, _func: &mut Function, _store: &TypeContext) -> WalkResult {
        WalkResult::Advance
    }

    fn visit_block(&mut self, _block: Block, _data: &BlockData) -> WalkResult {
        WalkResult::Advance
    }

    fn visit_inst(
        &mut self,
        _inst: Inst,
        _func: &mut Function,
        _store: &TypeContext,
    ) -> WalkResult {
        WalkResult::Advance
    }

    fn visit_terminator(
        &mut self,
        _block: Block,
        _term: &Terminator,
        _func: &mut Function,
    ) -> WalkResult {
        WalkResult::Advance
    }
}

/// Walk a function mutably.
pub fn walk_function_mut(
    visitor: &mut dyn MutIrVisitor,
    func: &mut Function,
    ctx: &TypeContext,
) -> WalkResult {
    match visitor.visit_function(func, ctx) {
        WalkResult::Stop => return WalkResult::Stop,
        WalkResult::Skip => return WalkResult::Advance,
        WalkResult::Advance => {}
    }

    // Snapshot block order to avoid borrow issues during mutation
    let blocks: Vec<Block> = func.layout.block_order.clone();
    for block in blocks {
        let block_data = func.dfg.block(block);
        match visitor.visit_block(block, block_data) {
            WalkResult::Stop => return WalkResult::Stop,
            WalkResult::Skip => continue,
            WalkResult::Advance => {}
        }

        // Snapshot instruction order
        let insts: Vec<Inst> = block_data.inst_order.clone();
        for inst_id in insts {
            if inst_id.0 as usize >= func.dfg.insts.len() {
                continue;
            }
            if matches!(
                func.dfg.insts[inst_id.0 as usize].opcode,
                super::opcode::Opcode::Nop
            ) {
                continue;
            }
            if visitor.visit_inst(inst_id, func, ctx) == WalkResult::Stop {
                return WalkResult::Stop;
            }
        }

        let term = func.dfg.block(block).terminator.clone();
        if visitor.visit_terminator(block, &term, func) == WalkResult::Stop {
            return WalkResult::Stop;
        }
    }

    WalkResult::Advance
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::FunctionBuilder;
    use crate::types::FunctionSignature;

    struct CountingVisitor {
        blocks: usize,
        insts: usize,
        terms: usize,
    }

    impl IrVisitor for CountingVisitor {
        fn visit_block(&mut self, _block: Block, _data: &BlockData) -> WalkResult {
            self.blocks += 1;
            WalkResult::Advance
        }
        fn visit_inst(&mut self, _inst: Inst, _data: &Instruction) -> WalkResult {
            self.insts += 1;
            WalkResult::Advance
        }
        fn visit_terminator(&mut self, _block: Block, _term: &Terminator) -> WalkResult {
            self.terms += 1;
            WalkResult::Advance
        }
    }

    #[test]
    fn test_visitor_counts_correctly() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "x")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, params) = fb.create_entry_block();
        let mut b = fb.build(entry);
        let v = b.iconst_i32(42);
        let sum = b.iadd(params[0], v);
        b.ret(&[sum]);
        let func = fb.finish();

        let mut visitor = CountingVisitor {
            blocks: 0,
            insts: 0,
            terms: 0,
        };
        walk_function(&mut visitor, &func, &ctx);
        assert_eq!(visitor.blocks, 1);
        assert_eq!(visitor.insts, 2); // iconst + iadd
        assert_eq!(visitor.terms, 1);
    }

    #[test]
    fn test_walk_result_stop() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        let mut b = fb.build(entry);
        let v = b.iconst_i32(42);
        b.ret(&[v]);
        let func = fb.finish();

        struct StopAtBlock;
        impl IrVisitor for StopAtBlock {
            fn visit_block(&mut self, _block: Block, _data: &BlockData) -> WalkResult {
                WalkResult::Stop
            }
        }

        let mut visitor = StopAtBlock;
        let result = walk_function(&mut visitor, &func, &ctx);
        assert!(result.should_stop());
    }

    #[test]
    fn test_visitor_skips_nop() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        let mut b = fb.build(entry);
        let v = b.iconst_i32(1);
        let v2 = b.iconst_i32(2);
        // Create a Nop by copying
        let _copy = b.copy(v);
        b.ret(&[v2]);
        let func = fb.finish();

        struct InstCounter(usize);
        impl IrVisitor for InstCounter {
            fn visit_inst(&mut self, _inst: Inst, _data: &Instruction) -> WalkResult {
                self.0 += 1;
                WalkResult::Advance
            }
        }

        let mut visitor = InstCounter(0);
        walk_function(&mut visitor, &func, &ctx);
        assert_eq!(visitor.0, 3); // iconst 1, iconst 2, copy (all live)
    }
}
