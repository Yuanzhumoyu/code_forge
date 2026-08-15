//! 集中 re-export（对齐 rustc_codegen_cranelift 的 `mod prelude` 模式）。
//!
//! 单文件拆多模块后，各模块只需 `use crate::prelude::*;` 即可拿到
//! rustc 常用类型 + code_forge 公共项，消除重复 import 噪音。
//!
//! rustc 内部 crate 通过 lib.rs 顶部的 `extern crate` 声明引入，
//! 子模块可直接用 `rustc_*` 名称，无需在 prelude 重复。

pub use std::any::Any;
pub use std::collections::HashMap;

pub use rustc_abi::FieldIdx;
pub use rustc_codegen_ssa::traits::CodegenBackend;
pub use rustc_codegen_ssa::{CompiledModule, CompiledModules, CrateInfo, ModuleKind};
pub use rustc_index::Idx;
pub use rustc_middle::dep_graph::{WorkProduct, WorkProductId, WorkProductMap};
pub use rustc_middle::mir::{self, Body, Operand, Rvalue, StatementKind, TerminatorKind};
pub use rustc_middle::mono::MonoItem;
pub use rustc_middle::ty::vtable::VtblEntry;
pub use rustc_middle::ty::{self, Instance, Ty, TyCtxt};
pub use rustc_session::config::OutputFilenames;
pub use rustc_session::{IncrCompSession, Session};

pub use code_forge::backend::*;
pub use code_forge::forge_object::ObjectWriter;
pub use code_forge::forge_object::TargetConfig;
pub use code_forge::ir::*;
