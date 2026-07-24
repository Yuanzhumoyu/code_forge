//! IR（中间表示）模块。
//!
//! 提供 SSA 形式的 IR 类型、指令、构建器以及分析工具。
//!
//! # 子模块分类
//!
//! ## 核心类型
//! - [`types`] — 基础类型定义 (Type, VReg, PReg, BlockId, Value, Signature, CallConv)
//! - [`instructions`] — 指令与 Opcode 枚举
//! - [`big`] — 大整数/常量值 (Big)
//!
//! ## 数据结构
//! - [`function`] — Function, Block, Terminator, Instruction
//! - [`module`] — Module, FuncRef 跨函数引用
//! - [`constant_pool`] — 常量池
//! - [`context`] — 编译上下文 (类型注册表)
//!
//! ## 构建器与解析器
//! - [`builder`] — FunctionBuilder: 流式 IR 构建
//! - [`parser`] — LLVM IR 文本格式解析器
//!
//! ## 分析
//! - [`analysis`] — 支配树 (DominatorTree)、支配边界、CFG 辅助函数
//! - [`loop_info`] — 自然循环检测 (LoopInfo + LoopForest)
//!
//! ## 序列化
//! - [`serialize`] — 二进制序列化/反序列化
//! - [`bitcode`] — LRIR bitcode 格式
//!
//! ## 扩展
//! - [`debug_info`] — DWARF 调试信息
//! - [`eh`] — 异常处理 IR 扩展

// ============================================================
// 核心类型
// ============================================================
mod types;
mod instructions;
mod big;

// ============================================================
// 数据结构
// ============================================================
mod function;
mod module;
mod constant_pool;
mod context;

// ============================================================
// 构建器与解析器
// ============================================================
mod builder;
mod parser;

// ============================================================
// 分析
// ============================================================
pub mod analysis;
pub mod loop_info;

// ============================================================
// 序列化
// ============================================================
mod serialize;
pub mod bitcode;

// ============================================================
// 扩展
// ============================================================
mod debug_info;
pub mod eh;

// ============================================================
// 公开导出
// ============================================================

// 核心类型
pub use types::*;
pub use instructions::*;
pub use big::*;

// 数据结构
pub use function::*;
pub use module::*;
pub use constant_pool::*;
pub use context::*;

// 构建器与解析器
pub use builder::*;
pub use parser::*;

// 分析
pub use analysis::*;
pub use loop_info::*;

// 序列化
#[allow(unused_imports)]
pub use serialize::*;
pub use bitcode::*;

// 扩展
pub use debug_info::*;
pub use eh::*;
