//! **调用约定层**（v20 A1）：约定是**使用者提供的数据** + 通用引擎；ISA 只提供能力。
//!
//! ## 为什么有这个 crate
//!
//! 历史上调用约定被写在 ISA 谱的 `[abi]` 节里（arg 寄存器池、callee-saved、栈参数布局、
//! 帧填充、sret 规则…），于是：① IR 里那个 `CallConv` 成了死值（全仓无人读）；② 同一份
//! 约定在每个 ISA 里被重复声明一遍，且各写各的（x86 有 `push`/`wide_vec_*`/`stack_arg_*`
//! 角色，arm64 一个都没有）；③ 换 ISA 就错——例如 sret 指针永远取"首 int 参数槽"
//! （Windows x64 的 RCX 语义），AAPCS64 的间接结果寄存器其实是 **x8**；④ 变参在模型里
//! **无处可写**。
//!
//! 本 crate 把这件事拆成三层，**约定与 ISA 不再互相污染**：
//!
//! ```text
//! 使用者/前端 ── CallConvId + 每条实参/形参的属性 ──► 本 crate 的引擎
//! ISA（能力面）── AbiTarget（哪些寄存器/宽度/能力可用）──► 同上
//!                                      │
//!                                      ▼
//!                                  AbiPlan（纯数据）
//!                                      │
//!                     管线按 plan 发射调用点 / 入口 / 序尾声
//! ```
//!
//! ## 三层数据
//!
//! 1. [`rules::AbiRules`]：**约定本身**（平台无关的规则：位置计数、栈布局、分类规则、
//!    hidden 槽、变参形态、callee-saved、被叫方弹栈…）。可序列化成 TOML；
//!    内置 `win64`/`sysv64`/`aapcs64`/`lp64d`（见 [`builtin`]），使用者可加自己的。
//! 2. [`binding::AbiBinding`]：**(ISA, 约定) 的寄存器绑定**——把规则里抽象的池名
//!    （`int`/`float`/`vector`/`sret`…）绑到该 ISA 的具体寄存器（类 + 序号或名字）。
//! 3. [`plan::AbiPlan`]：引擎产物（每个实参/形参的位置、返回值、栈布局、callee-saved、
//!    hidden 槽、clobber）。调用方与被调方**用同一个引擎**得到互补的 plan，
//!    不再靠两处手写序列对齐。
//!
//! ## 数据表达不了的怎么办
//!
//! [`registry::AbiHooks`]：宿主钩子（分类回调 / plan 后调整）。Swift 的 `self`/`error`、
//! Go 的 context 寄存器与 GC 安全点、GC 帧描述这类**语言专属**约定走钩子；
//! **绝不**把"某个约定"硬编码进引擎。
//!
//! ## 现状（A1 范围）
//!
//! 本片**只新增**：模型 + 引擎 + 内置约定 + 快照/不变量测试 + 复现用的数据文件。
//! 管线与 ISA 谱尚未切换（A3–A5 做），因此现有行为**逐字节不变**。
//!
//! 未表达的部分一律 **fail-closed**（[`error::AbiError::Unsupported`]），不猜、不降级。

pub mod binding;
pub mod builtin;
pub mod engine;
pub mod error;
pub mod plan;
pub mod registry;
pub mod rules;
pub mod ty;

pub use binding::{AbiBinding, RegSelector};
pub use engine::{AbiTarget, Capability, ClassDir, Signature, plan_fn};
pub use error::AbiError;
pub use plan::{
    AbiPlan, ArgLoc, CalleeSaveMechanism, CalleeSavedPlan, Extension, HiddenSlots, Placement,
    Purpose, RetLoc, StackLayout, VaArea,
};
pub use registry::{AbiHooks, AbiRegistry};
pub use rules::{
    AbiRules, CalleePop, ClassAction, ClassRule, IndirectVia, PositionRule, SlotsSpec,
};
pub use ty::{Elem, TyKind, TyView};
