//! Forge IR v2: SSA-form intermediate representation with entity-component separation.
//!
//! # Architecture
//!
//! Inspired by Cranelift's entity system:
//! - Entities (`Value`, `Inst`, `Block`, `TypeId`, ...)  are Copy handles (u32 newtypes)
//! - Data is stored in `PrimaryMap` tables inside `DataFlowGraph`
//! - `TypeStore` provides type interning and size/alignment queries
//! - `UseLists` provides incrementally-maintained def-use chains
//! - Block Parameters replace traditional Phi instructions
//!
//! # Key differences from v1
//!
//! - No `Phi` instruction — use Block Parameters instead
//! - No `StackLoad`/`StackStore` — use `Load`/`Store` + `StackAddr`
//! - `FastMathFlags` moved from `Opcode` variants to `InstFlags` bitmask
//! - `ConstantPool` uses native hashing instead of String-based dedup
//! - `Module::function_table()` returns `&Function` references, no cloning
//! - `Call` supports multiple return values
//! - Analysis (DomTree, LoopForest) stored in lazy `AnalysisCache`, not on `Function`

// ============================================================
// Legacy support — kept for dependent crates
// ============================================================
pub mod big;
pub mod debug_info;

// ============================================================
// New module declarations
// ============================================================
pub mod analysis;
pub mod builder;
pub mod constant;
pub mod data_layout;
pub mod dfg;
pub mod display;
pub mod entity;
pub mod error;
pub mod function;
pub mod imm_str;
pub mod immediate;
pub mod inst_flags;
pub mod ir_parser;
pub mod loop_info;
pub mod mem_flags;
pub mod metadata;
pub mod opcode;
pub mod string_pool;
pub mod symbol;
pub mod terminator;
pub mod types;
pub mod use_list;
pub mod verify;

// ============================================================
// Re-exports — flat namespace for all public types
// ============================================================

// Entity types
pub use entity::*;
pub use error::IrError;
pub use imm_str::*;
pub use string_pool::InternedStr;
pub use string_pool::StringPool;

// Types
pub use data_layout::*;
pub use types::*;

// Opcode + related
pub use immediate::*;
pub use inst_flags::*;
pub use opcode::*;

// Data structures
pub use constant::*;
pub use dfg::*;
pub use function::*;
pub use terminator::*;
pub use use_list::*;

// Builder
pub use builder::*;

// Display (nothing to re-export, just Display impls)

// Analysis
pub use analysis::*;
pub use loop_info::*;

// Verifier
pub use verify::*;

// Legacy re-exports
pub use big::*;
pub use debug_info::*;
