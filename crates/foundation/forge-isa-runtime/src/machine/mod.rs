//! Machine backend trait system — v19 redesign.
//!
//! Each component is a focused trait with a single responsibility.
//! [`TargetMachine`] composes them into a complete backend.
//!
//! # Trait Hierarchy
//!
//! ```text
//! TargetMachine (composes all below)
//!   ├── IsaInfo          (static ISA metadata)
//!   ├── TargetRegInfo    (register file description)
//!   ├── TargetABI        (calling convention)
//!   ├── TargetLowering   (IR → machine instructions)
//!   ├── TargetEncoder    (machine inst → bytes)
//!   ├── TargetFrameLowering (prologue / epilogue)
//!   │
//!   └── Optional: TargetDecoder, TargetDisassembler,
//!                 TargetAssembler
//! ```

pub mod abi;
/// **调用约定模型**（v23 M2）：`AbiPlan`/`Placement`/`RetLoc`/`TyView`…
/// 由 leaf crate `forge-abi-model` 提供，这里**再导出**——生成物与管线的路径因此稳定
/// （`crate::machine::abi_model::TyView`），不再有手抄的第二份形状。
pub use forge_abi_model as abi_model;
pub mod assembler;
/// 调用布局（中性数据；v20 A3b-2 起生成物与管线读它）。
pub mod call_layout;
/// 调用点布局查询（v20 A5-3）：宿主按 `(ISA, 约定, 实参形状)` 算**被调方**的布局。
pub mod call_plan;
pub mod cfi;
pub mod decoder;
pub mod disasm;
pub mod encoder;
pub mod frame;
pub mod inst;
pub mod isa_info;
pub mod lowering;
pub mod pattern;
pub mod reg_info;
pub mod reloc_patcher;
pub mod target;
