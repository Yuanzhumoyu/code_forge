//! **调用约定模型**（纯数据，v23 M2）：引擎产物与它依赖的几个规则枚举。
//!
//! ## 为什么单独一个 crate
//!
//! `AbiPlan` 是**调用方与被调方共用的唯一数据形状**：引擎（`forge-abi`）产出它，编译管线
//! 与**生成物**（只依赖 `forge-isa-runtime`）读它。此前运行时侧另有一份**手抄镜像**
//! （`machine::call_layout::CallLayout` + `ArgPlace`/`RetPlace`/`VaInfo`…）与一份 300 行
//! 翻译，于是"同一事实两处表示"、加一个 ABI 特性要改四处。把产物抽成这个 leaf crate 后：
//!
//! ```text
//! forge-abi-model ──► forge-abi（规则/绑定/引擎，产出 AbiPlan）
//!        │
//!        └──────────► forge-isa-runtime（生成物与管线直接读 AbiPlan）──► 生成物
//! ```
//!
//! 依赖纪律：`serde`（`PositionRule`/`CalleePop` 是 TOML 面枚举）与 `forge-ir`
//! （[`TyView::from_ir_type`](ty::TyView::from_ir_type) 是**唯一的** IR → 类型视图投影）；
//! **不**依赖 `toml`、**不**依赖引擎——把规则加载层拖进每一个宿主是刻意避免的。
//!
//! 类型只是数据：没有引擎、没有规则求值、没有寄存器映射（映射需要 ISA 侧的
//! `AbiTarget`，那是 `forge-abi` 的事）。
//!
//! `TyView` 本身仍然**只是视图**（引擎要对"任意真实编译器给的布局"——rustc 的 `Layout`、
//! C 的 ABI 布局——都能工作）；`from_ir_type` 只是本仓 IR 的适配器。

pub mod ir_view;
pub mod link;
pub mod plan;
pub mod rules_pod;
pub mod ty;

pub use ir_view::{MAX_DEPTH, MAX_MEMBERS};
pub use link::{LinkFormat, LinkReloc, LinkRelocTable};
pub use plan::{
    AbiPlan, ArgLoc, CalleeSaveMechanism, CalleeSavedPlan, DeclAttrs, Extension, HiddenSlots,
    Placement, Purpose, RegRef, RetLoc, StackLayout, VaArea, VaArgRule, VaArgRules, VaField,
    VaSaveArea, VaSaveSlot, ty_text,
};
pub use rules_pod::{CalleePop, PositionRule, resolve_callee_pop};
pub use ty::{Elem, TyKind, TyView};
