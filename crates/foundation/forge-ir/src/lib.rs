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
// pub mod eh;  // Removed: legacy EH module, zero external callers

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
pub mod function;
pub mod immediate;
pub mod inst_flags;
pub mod ir_parser;
pub mod loop_info;
pub mod mem_flags;
pub mod metadata;
pub mod opcode;
pub mod stack_slot;
pub mod string_pool;
pub mod symbol;
pub mod terminator;
pub mod types;
pub mod use_list;
pub mod verify;
pub mod visit;
// ============================================================
// Type constants — fixed TypeId values matching TypeStore preset order
// ============================================================
/// Common type constants. These TypeId values match TypeStore's preset indices.
/// Usage: `Type::I32`, `Type::I64`, `Type::Ptr`, etc.
impl TypeId {
    pub const VOID: TypeId = TypeId(0);
    pub const BOOL: TypeId = TypeId(1);
    pub const I8: TypeId = TypeId(2);
    pub const I16: TypeId = TypeId(3);
    pub const I32: TypeId = TypeId(4);
    pub const I64: TypeId = TypeId(5);
    pub const F32: TypeId = TypeId(6);
    pub const F64: TypeId = TypeId(7);
    pub const PTR: TypeId = TypeId(8);
    // Extended types (registered on demand in TypeStore)
    pub const I128: TypeId = TypeId(10);
    pub const F16: TypeId = TypeId(11);
    pub const F128: TypeId = TypeId(12);
    pub const V64: TypeId = TypeId(13);
    pub const V128: TypeId = TypeId(14);
    pub const V256: TypeId = TypeId(15);
    // NOTE: All type construction must go through TypeStore methods.
    // These placeholder constructors were removed — use TypeStore::struct_named,
    // TypeStore::struct_anon, TypeStore::array_ty, etc. instead.
}

// ============================================================
// Re-exports — flat namespace for all public types
// ============================================================

// Entity types
pub use entity::*;
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

// ============================================================
// CompileError
// ============================================================

/// Compilation error — unified error type for the entire Forge pipeline.
#[derive(thiserror::Error, Debug)]
pub enum CompileError {
    #[error("Lowering failed: {0}")]
    Lowering(String),

    #[error("Register allocation failed: {0}")]
    RegAlloc(String),

    #[error("Code emission failed: {0}")]
    Emit(String),

    #[error("Backend not found: {0}")]
    BackendNotFound(String),

    #[error("Unsupported operation: {0}")]
    Unsupported(String),

    #[error("Type mismatch: {0}")]
    TypeError(String),

    #[error("ABI error: {0}")]
    AbiError(String),

    #[error("Link error: {0}")]
    LinkError(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Not yet implemented: {0}")]
    Unimplemented(String),
}
