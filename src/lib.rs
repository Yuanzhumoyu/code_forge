//! # Code Forge — specification-driven compiler infrastructure
//!
//! Write a **language grammar** file and an **ISA TOML** file, get a working compiler
//! frontend and backend with JIT execution.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────┐  ┌──────────────────┐
//! │ forge-grammar   │  │ forge-dsl        │
//! │ (parser/CST)    │  │ (TOML→Rust code) │
//! └────────┬────────┘  └────────┬─────────┘
//!          │                    │
//! ┌────────▼────────────────────▼─────────┐
//! │           forge-codegen               │
//! │  (lowering, regalloc, emit, JIT)      │
//! └────────┬──────────────────────────────┘
//!          │
//! ┌────────▼────────┐  ┌──────────────────┐
//! │   forge-opt     │  │   forge-ir       │
//! │   (15+ passes)  │  │   (SSA IR types) │
//! └─────────────────┘  └──────────────────┘
//! ```
//!
//! ## Quick Start
//!
//! ```ignore
//! use code_forge::prelude::*;
//! use code_forge::backend::x86_64::X86Isa;
//!
//! let mut jit = JitCompiler::<X86Isa>::new();
//! jit.add_function("add", &FunctionSignature::new(
//!     &[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]
//! ), |b| {
//!     let (entry, params) = b.create_block_here_with(&[
//!         (TypeId::I32, "a"), (TypeId::I32, "b")
//!     ]);
//!     b.switch_to_block(entry);
//!     let sum = b.iadd(params[0], params[1]);
//!     b.ret(&[sum]);
//! })?;
//!
//! let add: extern "C" fn(i32, i32) -> i32 = jit.get_fn("add")?;
//! assert_eq!(unsafe { add(3, 4) }, 7);
//! ```

// ============================================================
// Re-export all forge-* crates
// ============================================================
pub use forge_asm;
pub use forge_codegen as backend;
pub use forge_dsl;
pub use forge_grammar;
pub use forge_ir as ir;
pub use forge_mem as mem;
pub use forge_opt as optimize;
pub use smallvec;

// Module re-exports for isa_from_file! compatibility
// The DSL-generated code uses crate::encode::*, crate::assembler::*, etc.
pub use forge_asm as assembler;
pub use forge_codegen::EncodeError;
pub use forge_codegen::encode;
#[cfg(feature = "jit")]
pub use forge_codegen::jit;
pub use forge_codegen::{AllocResult, CompiledFunction, RelocKind, Relocation};

// Optional tools
#[cfg(feature = "object-file")]
pub use forge_object;
#[cfg(feature = "plugins")]
pub use forge_plugin;

// ============================================================
// Convenience prelude
// ============================================================
pub mod prelude {
    pub use forge_codegen::prelude::*;
    pub use forge_ir::*;
    pub use forge_opt::*;
}
