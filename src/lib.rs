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
//! use code_forge::backend::jit::JitCompiler;
//! use code_forge::ir::*;
//!
//! code_forge::backend::x86_v12::ensure_registered();
//! let mut jit = JitCompiler::new(code_forge::backend::x86_v12::TargetMachine::new());
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
pub use forge_codegen as backend;
pub use forge_dsl;
pub use forge_grammar;
pub use forge_ir as ir;
pub use forge_mem as mem;
pub use forge_opt as optimize;
pub use smallvec;

// Module re-exports for DSL-generated code (v12 唯一语法；encode/asm 由
// v12 生成模块内联实现，无需 forge-asm 运行时)
pub use forge_codegen::EncodeError;
#[cfg(feature = "jit")]
pub use forge_codegen::jit;
pub use forge_codegen::{AllocResult, CompiledFunction, RelocKind, Relocation};
pub use forge_codegen::{RelocPatcher, RiscvRelocPatcher, X86RelocPatcher};

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
