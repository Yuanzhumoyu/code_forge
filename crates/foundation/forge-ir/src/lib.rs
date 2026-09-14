//! Forge IR v2: SSA-form intermediate representation with entity-component separation.
//!
//! # Architecture
//!
//! Inspired by Cranelift's entity system:
//! - Entities (`Value`, `Inst`, `Block`, `TypeId`, ...)  are Copy handles (u32 newtypes)
//! - Data is stored in arena `Vec`s on `DataFlowGraph`（句柄 .0 即下标）
//! - `TypeStore` provides type interning and size/alignment queries
//! - `UseLists` provides incrementally-maintained def-use chains — **仅覆盖指令
//!   操作数**；终结符用值不在其中（见 `Function::replace_all_uses` 文档与
//!   `docs/plans/forge-ir-v3-plan.md` S4）
//! - Block Parameters replace traditional Phi instructions
//!
//! # 已知结构欠账（v3 方案）
//!
//! 本 crate 的公开面与表示层缺口已由 `docs/plans/forge-ir-v3-plan.md` 记录并排期：
//! 指令元数据单一事实源（S1）、实体容器 `PrimaryMap/SecondaryMap`（S2）、类型
//! 上下文去锁与所有权（S3）、终结符并入指令流（S4）、附件强类型化与可见性收紧
//! （S5）、校验与 pass 契约（S6）。`entity.rs` 的 doc 注释声称已有
//! `PrimaryMap/SecondaryMap`——**尚未实现**，当前就是裸 `Vec` + `HashMap` 辅表。
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
pub mod alias;
pub mod analysis;
pub mod builder;
pub mod constant;
pub mod data_layout;
pub mod dfg;
pub mod display;
pub mod entity;
pub mod entity_map;
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
pub mod type_rules;
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
pub use alias::*;
pub use analysis::*;
pub use loop_info::*;

// Verifier
pub use verify::*;

// Legacy re-exports
pub use big::*;
pub use debug_info::*;
