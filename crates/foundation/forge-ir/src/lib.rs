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
// 模块树（2026-09-17 目录归类）
// ============================================================
//
// 顶层只剩三个文件（本文件 + `error.rs` + `verify.rs`），其余按职能归入目录：
//
// - `entity/`：句柄（`mod.rs`）与实体容器（`map.rs`）
// - `util/`：字符串与任意精度整数等基础设施（**不叫 `support`**：forge-opt 已有
//   同名模块，两者经 `code_forge::prelude` 的 glob 重导出会撞名报警）
// - `ir/`：核心 IR 数据模型（类型/常量/指令/函数/CFG/构建器/附件/符号）
// - `analysis/`：支配树、循环、use-list、别名、调试位置
// - `binary/`：二进制序列化（IR bitcode v1；**不随 feature 门控**，零依赖）
// - `text/`：文本层（parser + display，`features = ["text"]` 门控）
// - `verify.rs` / `error.rs`：校验与错误（单文件，保持顶层）

pub mod analysis;
pub mod binary;
pub mod entity;
pub mod error;
pub mod ir;
pub mod util;

/// 文本层：LLVM 文本 IR 的**解析器**（`parser`）与**打印机**（`display`）。
///
/// crate 里**唯一**的 `text` 门控点（目录归类后 `src/text/`）——关闭后核心 IR
/// （类型/函数/DFG/verifier/pass/二进制序列化）照常编译，`logos`/`lalrpop-util`
/// 可选依赖随之消失，`build.rs` 也不再生成 LALRPOP 表（生成开关读
/// `CARGO_FEATURE_TEXT`，因为 `lalrpop` 是 build-dependency、不可选）。
/// 边界守卫见 `tests/text_feature_gate.rs`。
#[cfg(feature = "text")]
pub mod text;

pub mod verify;

// ============================================================
// Re-exports — flat namespace for all public types
// ============================================================

// 实体句柄与容器（`SecondaryMap` 等容器直接扁平导出：密集句柄表的标准写法）
pub use entity::map::{EntityRef, EntitySet, PackedOption, PrimaryMap, SecondaryMap};
pub use entity::*;
pub use error::IrError;
pub use util::imm_str::*;
pub use util::string_pool::InternedStr;
pub use util::string_pool::StringPool;

// 类型系统与操作码
pub use ir::data_layout::*;
pub use ir::immediate::*;
pub use ir::inst_flags::*;
pub use ir::isel_strategy::IselStrategy;
pub use ir::opcode::*;
pub use ir::types::*;

// IR 数据结构
pub use ir::builder::*;
pub use ir::constant::*;
pub use ir::dfg::*;
pub use ir::function::*;
pub use ir::terminator::*;

// 分析
pub use analysis::alias::*;
pub use analysis::debug_info::*;
pub use analysis::loop_info::*;
pub use analysis::use_list::*;
pub use analysis::*;

// 校验
pub use verify::*;

// 二进制序列化（格式版本/兼容检查；`Module::{to_binary,to_binary_into,from_binary}`）
// + 文件级 IR 缓存（内容键、自愈；`IrCache`）
pub use binary::{
    BinaryCompat, CacheKey, IR_FORMAT_VERSION, IrCache, SectionId, check_binary_compat,
};

// 支撑类型
pub use util::big::*;
