//! 核心 IR 数据模型：类型系统、常量、指令/操作码、函数与 CFG、构建器、附件与符号。
//!
//! 目录归类（2026-09-17）时从 `src/` 顶层移入。各类型的**公开路径**仍由 `lib.rs`
//! 扁平重导出（`forge_ir::Function` / `TypeId` / `Opcode` …）；模块路径为
//! `forge_ir::ir::{types, dfg, function, opcode, constant, …}`。

pub mod builder;
pub mod constant;
pub mod data_layout;
pub mod dfg;
pub mod function;
pub mod immediate;
pub mod inst_flags;
pub mod isel_strategy;
pub mod mem_flags;
pub mod metadata;
pub mod opcode;
pub mod symbol;
pub mod terminator;
pub mod type_rules;
pub mod types;
