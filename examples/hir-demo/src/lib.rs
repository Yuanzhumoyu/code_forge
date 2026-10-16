//! Mini C subset compiler library.
//!
//! Provides the public API for compiling Mini C source code and JIT-executing it.

pub mod atoms;
pub mod codegen;
pub mod codegen_hir;
pub mod compiler;
pub mod grammar;
pub mod schema;
