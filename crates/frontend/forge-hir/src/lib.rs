//! Forge HIR — 积木式编译框架
//!
//! # 三层架构
//!
//! 1. **底板 (Framework)**: [`IrGraph`] — 通用图存储、Block/Value/Type 管理
//! 2. **原子积木 (Atom)**: [`AtomSpec`] — 编译期定义，映射到后端 Opcode
//! 3. **统一 lowering 上下文**: [`HirCtx`] — 手写 AST→IR 降级代码的唯一可变借用对象
//!
//! (`CompositeSpec` 结构积木层目前是 experimental，尚无实例化引擎。)
//!
//! # 使用模式
//!
//! 1. 用 `define_lowering!` 宏（`forge-hir-macro`）声明语言的 **op 目录**：
//!    每个 `atom` 生成 `xxx_tag() -> OpTag`、类型安全的
//!    `build_xxx(&mut IrGraph, 属性先, 操作数后) -> Result<…, HirError>`
//!    构造方法（仿 Cranelift `InstBuilder`），以及 `register_atoms()`。
//! 2. **手写 AST→IR 降级**：所有 lowering 函数统一签名
//!    `fn(…, ctx: &mut HirCtx, node: AstRef) -> Result<…, HirError>`，
//!    `HirCtx` 把图 + 符号表 + 循环栈 + 内联状态合并为一个对象，消除
//!    字段拆分/闭包适配器等借用体操。op 发射通过 `build_xxx` 构造方法。
//! 3. 图构建完成后调用 [`lower_into_module`] 一步降级为 forge-ir 函数。
//!
//! 完整示例见 `examples/mini_c`（`codegen_hir.rs`）。
//!
//! # 核心类型
//!
//! - [`OpTag`] — 操作标识 = 方言名 + 操作名（运行时符号）
//! - [`IrGraph`] — 通用 IR 图存储
//! - [`HirCtx`] — 手写 lowering 的统一上下文（图 + 前端状态）
//! - [`AtomSpec`] — 原子积木编译期元数据
//! - [`BrickRegistry`] — 积木注册表（op 目录）
//! - [`lower_into_module`] — IrGraph → forge-ir Function 的一次性降级入口

pub mod atom;
pub mod attr;
pub mod block;
pub mod builder;
#[doc(hidden)]
pub mod composite;
pub mod ctx;
pub mod error;
pub mod graph;
pub mod lowering;
pub mod registry;
pub mod span;

// Re-export the proc-macro (so users only need one dependency)
pub use forge_hir_macro::define_lowering;

// Re-exports from forge_ir (so macro-generated code has access via ::forge_hir)
pub use forge_ir::{
    self as ir, Block as IrBlock, FuncRef, FunctionSignature, IntCC, Module, TypeId,
    opcode::{self as ir_opcode, FloatCC, IntCC as IrIntCC, Opcode},
};

// Re-export forge_grammar for AstRef (used in macro-generated dispatch functions)
pub use forge_grammar::{self, AstRef};

// Re-exports from this crate
pub use atom::{AtomSpec, AttrSpec, OpTag, PortSpec, RegionSpec};
pub use attr::AttrValue;
pub use block::{BlockData, BlockId, Region};
pub use composite::{CompositeNode, CompositeSpec};
pub use ctx::HirCtx;
pub use error::HirError;
pub use graph::{GraphValue, IrGraph, NodeId};
pub use lowering::{LoweringContext, lower_into_module};
pub use registry::BrickRegistry;
pub use span::SourceSpan;
