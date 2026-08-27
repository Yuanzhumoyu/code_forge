//! rustc codegen backend powered by `code-forge`.
//!
//! Usage:
//! ```bash
//! cargo +nightly build --release
//! rustc +nightly -Zcodegen-backend=./target/release/forge_rustc.dll hello.rs
//! ```

#![feature(rustc_private)]
#![feature(box_patterns)]
// forge-rustc 排除在 workspace clippy 门禁之外（--exclude forge-rustc）——
// 但 forge-tests `nightly` feature 经 --all-features 会把它作为依赖拉进
// 构建，clippy 会连带 lint 它。这些是既有代码的风格性 lint（非本次改动），
// 用 crate 级 allow 避免门禁被依赖侧放倒。
#![allow(
    clippy::needless_borrow,
    clippy::collapsible_if,
    clippy::type_complexity,
    clippy::clone_on_copy,
    clippy::needless_lifetimes
)]

extern crate rustc_abi;
extern crate rustc_codegen_ssa;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_metadata;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;
extern crate rustc_symbol_mangling;
extern crate rustc_target;

mod abi;
mod alloc_runtime;
mod backend;
mod compile;
mod error;
mod func_ref;
mod global_data;
mod layout;
mod prelude;
mod rustc_compat;
mod trace;
mod types;

use crate::prelude::*;
mod lower;

// ============================================================
// 导出符号
// ============================================================

#[unsafe(no_mangle)]
pub fn __rustc_codegen_backend() -> Box<dyn CodegenBackend> {
    Box::new(backend::CodegenLibBackend)
}

/// 返回 codegen backend 名称——不泄漏 `rustc_private` 类型的公开 API，
/// 供下游 crate（如 forge-tests）在不启用 `rustc_private` 的情况下验证 backend 存在。
pub fn codegen_backend_name() -> &'static str {
    backend::CodegenLibBackend.name()
}

// P1.7 模块化后 re-export：forge-tests 等下游 crate 仍通过 crate 根引用
//（compile.rs 为私有 mod，pub fn 不自动暴露到 crate 根）
pub use crate::compile::{auto_register_isa_for_target, isa_name_for_target};

// ============================================================
// MIR → code-forge IR 转换
// ============================================================
