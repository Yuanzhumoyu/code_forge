//! Unified lowering context (`HirCtx`) — the single mutable object that
//! hand-written AST→IR lowering code works against.
//!
//! `HirCtx` bundles the IR graph with the frontend state a lowering pass
//! needs (symbol table, loop targets, stack-slot cursor, inlining state).
//! Every lowering function has the shape
//! `fn(…, ctx: &mut HirCtx, node: AstRef) -> Result<…>` — one mutable
//! borrow, no field-splitting hacks.

use crate::block::BlockId;
use crate::error::HirError;
use crate::graph::{GraphValue, IrGraph};
use forge_grammar::TypedAst;
use std::collections::HashMap;

/// Unified lowering context.
///
/// `S` is the frontend's own symbol-table type (e.g. mini_c's `SymTable`).
pub struct HirCtx<'a, S> {
    /// The IR graph being built.
    pub graph: &'a mut IrGraph,
    /// The typed AST being lowered (used for function-call inlining etc.).
    pub ast: &'a TypedAst,
    /// The original source text (for extracting operator tokens from spans).
    pub source: &'a str,
    /// Frontend symbol table (functions, enums, structs…).
    pub syms: &'a mut S,
    /// Variable name → stack-slot pointer ("p.x" keys for struct fields).
    pub locals: HashMap<String, GraphValue>,
    /// Stack of (cond_block, exit_block) for nested loops.
    pub loops: Vec<(BlockId, BlockId)>,
    /// Stack offset for the next local slot (grows downward).
    pub next_offset: i32,
    /// Inlining: return-value temp slot. None = emit a real ret.
    pub return_slot: Option<GraphValue>,
    /// Inlining: block to jump to on return. None = emit a real ret.
    pub return_block: Option<BlockId>,
}

impl<'a, S> HirCtx<'a, S> {
    /// Create a fresh context. Locals/loops/inlining state start empty.
    pub fn new(
        graph: &'a mut IrGraph,
        ast: &'a TypedAst,
        source: &'a str,
        syms: &'a mut S,
    ) -> Self {
        Self {
            graph,
            ast,
            source,
            syms,
            locals: HashMap::new(),
            loops: Vec::new(),
            next_offset: -4,
            return_slot: None,
            return_block: None,
        }
    }

    /// Allocate a new stack slot, returning its address.
    ///
    /// Emits a real `mem.stack_addr` node (offset attribute) — not the old
    /// `iconst(0) + iconst(offset)` address-arithmetic hack.
    pub fn alloc_slot(&mut self) -> Result<GraphValue, HirError> {
        let addr = self.graph.stack_addr(self.next_offset)?;
        self.next_offset -= 4;
        Ok(addr)
    }

    /// Look up a local variable, erroring if undefined.
    pub fn lookup(&self, name: &str) -> Result<GraphValue, HirError> {
        self.locals
            .get(name)
            .copied()
            .ok_or_else(|| HirError::Lowering(format!("undefined variable: {}", name)))
    }
}
