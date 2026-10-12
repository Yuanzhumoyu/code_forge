//! MachineInst trait — machine instruction interface.
//!
//! Defines the core trait that all machine instruction types must implement:
//! operand queries (uses/defs), effect tracking, branch/call/ret detection,
//! and memory access modeling.
//!
//! DSL-generated `Inst` enums implement this trait automatically.

use forge_ir::*;
use smallvec::SmallVec;
use std::hash::Hash;

// ============================================================
// EffectKind — instruction effect classification
// ============================================================

/// Instruction effect kind — maps to DSL `[effect]` system at runtime.
///
/// Guides the optimizer (dead code elimination, reordering) and register allocator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectKind {
    /// Pure computation (no side effects — can be deleted/reordered).
    Pure,
    /// Memory read.
    Read,
    /// Memory write.
    Write,
    /// Conditional branch.
    Branch,
    /// Unconditional jump.
    Jump,
    /// Function return.
    Ret,
    /// Function call.
    Call,
    /// Trap / system call.
    Trap,
    /// Custom extension effect (architecture-specific).
    Custom(u8),
}

impl EffectKind {
    /// Whether this effect modifies architectural state beyond destination registers.
    pub fn has_side_effects(&self) -> bool {
        !matches!(self, Self::Pure)
    }
}

// ============================================================
// MemAccess — memory access description
// ============================================================

/// Memory access description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemAccess {
    /// Access type: load / store / atomic read-modify-write.
    pub kind: MemAccessKind,
    /// Access size in bytes.
    pub size: u8,
}

/// Memory access type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemAccessKind {
    /// Load (read).
    Load,
    /// Store (write).
    Store,
    /// Atomic read-modify-write.
    LoadStore,
}

// ============================================================
// MachineInst trait
// ============================================================

/// Machine instruction trait.
///
/// Implemented by ISA-DSL generated `Inst` enums or hand-written ISA types.
/// 操作数的**角色**（v23 M4）：读还是写。
///
/// regalloc 的活区间口径依赖它：`Use` 在指令**起点**、`Def` 在指令**终点**——
/// 顺序错了活区间就错（静默错值）。因此 [`MachineInst::operands`] 的顺序有契约：
/// **uses 在前、defs 在后**，且各自保持字段序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OperandRole {
    /// 读（输入）。
    Use,
    /// 写（输出）。
    Def,
}

/// **一个操作数**（v23 M4）：字段序号 + 值寄存器 + 类 + 约束 + 角色 + 可改写性。
///
/// 为什么要有它：这些事实原本分居**两处**——`MachineInst` 的七个方法
/// （`uses`/`defs`/`use_constraints`/`def_constraints`/`reg_field`/`set_reg_field`/
/// `is_reg_field_settable`）与指令包里的平行旁表 `InstPacket::xreg_map`
/// （`Vec<SmallVec<[(XReg, field, is_def); 2]>>`，由生成物逐 arm `map_reg_field` 填）。
/// 两处必须逐条对齐，错一处就是活区间/回填错。合并成一张表后，"对齐"这件事
/// 在结构上不存在。
///
/// 字段口径：
/// - `field` = `reg_field`/`set_reg_field` 的 `i`（寄存器**字段**序号，不是操作数序号）；
/// - `reg` = 该字段当前的**值 XReg**（虚拟寄存器；分配后由 `set_reg_field` 回填物理号）；
/// - `class` = 该值的**类型宽度类**（如 I32 → `GPR(4)`）；`None` = "按槽声明类"，
///   调用方按指令声明的槽类重建（多类槽 `gprx` 必须用 `Some`，否则回退 64 位视图
///   ⇒ encode opsize 恒 64、auto 宽度分发失效，见 WA-35）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstOperand {
    /// 寄存器**字段**序号（`reg_field`/`set_reg_field` 的 `i`）。
    pub field: u8,
    /// 该字段**当前**的占位寄存器物理号——`None` = 该字段没有寄存器槽
    /// （旧 `uses()` 会跳过这类字段，语义照旧由这里表达）。
    ///
    /// **注意**：微指令字段里放的是**占位物理寄存器**，不是虚拟寄存器（vreg）。
    /// "第几个 vreg 落到这个字段"目前由 lowering 在 `InstPacket::map_reg_field`
    /// 时给出（管线侧的平行旁表）。把它并进本表的前提是**让字段占位直接携带 vreg 号**
    /// ——那是 M4 批次 C 的前置条件，已记在方案 §13。
    pub slot: Option<u32>,
    /// 该字段的**声明类**（多类槽才有值）；`None` = 单类槽/由指令声明的槽类决定。
    pub class: Option<RegClass>,
    pub constraint: OperandConstraint,
    pub role: OperandRole,
    /// `set_reg_field(field, …)` 会不会真的改写它（模板里写死的物理字段 = `false`）。
    pub settable: bool,
}

pub trait MachineInst: Clone + std::fmt::Debug + Send + Sync + Hash + Eq {
    /// **本指令的操作数表**（v23 M4 的单一模型）。
    ///
    /// 顺序契约：`Use` 在前、`Def` 在后，各组内保持字段序（regalloc 的
    /// "use 在 inst_start、def 在 inst_end"由此保证）。
    fn operands(&self) -> SmallVec<[InstOperand; 4]>;

    /// Temporary registers (XReg) read by this instruction.
    ///
    /// 缺省由 [`Self::operands`] 派生（生成物与手写实现都不必再各写一遍）。
    fn uses(&self) -> SmallVec<[u32; 4]> {
        self.operands()
            .iter()
            .filter(|o| o.role == OperandRole::Use)
            .filter_map(|o| o.slot)
            .collect()
    }

    /// Temporary registers (XReg) written by this instruction.
    fn defs(&self) -> SmallVec<[u32; 2]> {
        self.operands()
            .iter()
            .filter(|o| o.role == OperandRole::Def)
            .filter_map(|o| o.slot)
            .collect()
    }

    /// Effect tags for this instruction (derived from DSL `effect` system).
    ///
    /// Default implementation infers from uses/defs/memory_access:
    /// - Has defs and no memory access → Pure
    /// - Has memory access(Load) → Read
    /// - Has memory access(Store) → Write
    fn effects(&self) -> SmallVec<[EffectKind; 2]> {
        let mut e = SmallVec::new();
        if let Some(ma) = self.memory_access() {
            match ma.kind {
                MemAccessKind::Load => e.push(EffectKind::Read),
                MemAccessKind::Store => e.push(EffectKind::Write),
                MemAccessKind::LoadStore => {
                    e.push(EffectKind::Read);
                    e.push(EffectKind::Write);
                }
            }
        }
        if e.is_empty() && !self.defs().is_empty() {
            e.push(EffectKind::Pure);
        }
        e
    }

    /// Whether this is a branch instruction.
    fn is_branch(&self) -> bool;

    /// Branch target blocks.
    fn branch_targets(&self) -> SmallVec<[Block; 2]>;

    /// Whether this is a call instruction.
    fn is_call(&self) -> bool;

    /// Whether this is a return instruction.
    fn is_ret(&self) -> bool;

    /// Whether this is a terminator (branch, jump, return — ends a basic block).
    fn is_terminator(&self) -> bool {
        self.is_branch() || self.is_ret()
    }

    /// Memory access information, if any.
    fn memory_access(&self) -> Option<MemAccess> {
        None
    }

    /// Whether this instruction has side effects (cannot be deleted or reordered).
    fn has_side_effects(&self) -> bool {
        self.is_call() || self.is_ret() || self.is_branch() || self.memory_access().is_some()
    }

    /// Register-to-register move information. Returns (dst, src) if it's a move.
    fn is_move(&self) -> Option<(u32, u32)> {
        None
    }

    /// 读取第 `i` 个 Ireg/Freg 寄存器字段的物理索引。
    /// 微指令构造时以默认寄存器占位，分配器分配后经 [`Self::set_reg_field`] 回填。
    ///
    /// 缺省由 [`Self::operands`] 派生（`field == i` 的那一项）；没有该字段 ⇒ 0。
    fn reg_field(&self, i: usize) -> u32 {
        self.operands()
            .iter()
            .find(|o| usize::from(o.field) == i)
            .and_then(|o| o.slot)
            .unwrap_or(0)
    }

    /// 回写第 `i` 个 Ireg/Freg 寄存器字段的物理索引（寄存器分配后调用）。
    /// `class` 是该操作数的**类型宽度类**（调用方从 `LowerCtx::xreg_types`
    /// 的 IR 类型推导：I32 → `GPR(4)`、u8 → `GPR(1)`）——不是 XReg 的分配
    /// 类（值 XReg 恒池宽 GPR64，spill/ABI 依赖）。DSL 生成的实现：单类槽
    /// （如 `[gpr32]`）按槽声明的 class 重建（指令声明宽度，现状语义）；
    /// 多类槽（`gprx`）用此 class 重建——否则回退 64 位视图 → encode
    /// opsize 恒 64，auto 宽度分发失效（见 WA-35/DSL 回填缺口）。
    fn set_reg_field(&mut self, _i: usize, _preg_idx: u32, _class: RegClass) {}

    /// 第 `i` 个寄存器字段是否**可被 [`Self::set_reg_field`] 改写**。
    ///
    /// 默认 `true`（手写 machine 实现沿用旧契约）；DSL 生成实现按该变体的
    /// Reg 操作数表精确返回——固定物理字段（模板里写死的 RAX 等、不参与
    /// `map_reg_field` 的字段）返回 `false`。
    ///
    /// 用途：regalloc 对 **spilled def** 的 fail-closed 校验——spilled def 依赖
    /// emission 把该字段改写成 scratch 寄存器后 store 回槽；字段不可改写时，
    /// 指令实际写入物理寄存器而 store-back 从 scratch 读 → **静默写坏 spill 槽**
    /// （读回垃圾 → 偶发 AV/挂起）。此时宁可编译期报错（WORKAROUNDS WA-40）。
    fn is_reg_field_settable(&self, _i: usize) -> bool {
        true
    }

    /// Whether this instruction is foldable (no side effects, can be deleted).
    fn is_foldable(&self) -> bool {
        !self.has_side_effects()
    }

    /// Physical register indices clobbered by this instruction (used for calls).
    fn clobbers(&self) -> &[(u32, RegClass)] {
        &[]
    }

    // ── Operand constraints (for register allocator) ──

    /// 每个 def 操作数的寄存器约束（与 defs() 并行）。缺省由 [`Self::operands`] 派生。
    fn def_constraints(&self) -> SmallVec<[OperandConstraint; 2]> {
        self.operands()
            .iter()
            .filter(|o| o.role == OperandRole::Def)
            .map(|o| o.constraint)
            .collect()
    }

    /// 每个 use 操作数的寄存器约束（与 uses() 并行）。缺省由 [`Self::operands`] 派生。
    fn use_constraints(&self) -> SmallVec<[OperandConstraint; 4]> {
        self.operands()
            .iter()
            .filter(|o| o.role == OperandRole::Use)
            .map(|o| o.constraint)
            .collect()
    }

    /// **回写第 `i` 个操作数**（按 [`Self::operands`] 的序）的物理寄存器。
    ///
    /// 返回"是否真的改写了"：`false` = 该字段不可改写（固定物理字段）或序号越界——
    /// regalloc 对 spilled def 必须据此 fail-closed（WA-40：静默 no-op 会让
    /// store-back 从 scratch 读、spill 槽留下陈旧值）。
    ///
    /// v23 M4 起这是**唯一**的回写入口；`set_reg_field` 保留为"按字段号回写"的
    /// 底层方法，由生成物实现。
    fn set_operand_reg(&mut self, i: usize, preg_idx: u32, class: RegClass) -> bool {
        let ops = self.operands();
        let Some(op) = ops.get(i) else {
            return false;
        };
        if !op.settable {
            return false;
        }
        self.set_reg_field(usize::from(op.field), preg_idx, class);
        true
    }
}

// ============================================================
// OperandConstraint — per-operand register allocation constraint
// ============================================================

/// 单个操作数的寄存器分配约束。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OperandConstraint {
    /// 任意同类型寄存器。
    Any,
    /// 必须为指定物理寄存器。
    Fixed(PReg),
    /// 必须与第 N 个输入操作数共享同一物理寄存器
    /// （用于 x86 双操作数指令，如 `add dst, src`）。
    ReuseInput(usize),
    /// 必须在栈上（溢出槽）。
    Stack,
    /// Call 指令的 clobber 约束。
    Clobber(PReg),
}

// ============================================================
// DummyInst — 用于测试的占位机器指令
// ============================================================

/// 用于测试的占位机器指令。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DummyInst {
    pub id: u32,
}

impl MachineInst for DummyInst {
    /// 占位指令没有操作数（v23 M4 起这是唯一必答的方法）。
    fn operands(&self) -> SmallVec<[InstOperand; 4]> {
        SmallVec::new()
    }

    fn is_branch(&self) -> bool {
        false
    }

    fn branch_targets(&self) -> SmallVec<[Block; 2]> {
        SmallVec::new()
    }

    fn is_call(&self) -> bool {
        false
    }

    fn is_ret(&self) -> bool {
        false
    }
}
