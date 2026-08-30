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
use crate::span::SourceSpan;
use forge_grammar::{AstRef, TypedAst};
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
    /// 当前内联链（函数名集合）——递归检测：callee 已在链中 → 拒绝内联。
    pub inlining: Vec<String>,
    /// 最近 lower 的 AST 节点源区间——错误定位用（诊断升级）。
    pub last_span: Option<SourceSpan>,
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
            inlining: Vec::new(),
            last_span: None,
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

    /// 记录当前 AST 节点的源区间——lowering 分发函数入口调用，
    /// 深层错误经 [`HirCtx::locate`] 附加最近节点的源位置。
    pub fn set_span(&mut self, node: AstRef<'_>) {
        self.last_span = Some(SourceSpan::from(node.span()));
    }

    /// 把错误附加最近节点的源位置（1-based 行/列 + 行文本预览）。
    /// 已定位的错误（内层更精确）原样返回，不重复包装；无节点上下文
    /// 时原样返回。
    pub fn locate(&self, err: HirError) -> HirError {
        if matches!(err, HirError::Located { .. }) {
            return err;
        }
        let Some(span) = self.last_span else {
            return err;
        };
        let (line, col) = span.line_col(self.source);
        let line_text = span.line_text(self.source).to_string();
        HirError::Located {
            error: Box::new(err),
            line,
            col,
            line_text,
        }
    }
}
