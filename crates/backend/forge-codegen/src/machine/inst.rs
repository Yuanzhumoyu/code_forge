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
pub trait MachineInst: Clone + std::fmt::Debug + Send + Sync + Hash + Eq {
    /// Temporary registers (XReg) read by this instruction.
    fn uses(&self) -> SmallVec<[u32; 4]>;

    /// Temporary registers (XReg) written by this instruction.
    fn defs(&self) -> SmallVec<[u32; 2]>;

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

    /// 读取第 `i` 个 Ireg/Freg 寄存器字段的物理索引（与 uses()/defs() 顺序一致）。
    /// 微指令构造时以默认寄存器占位，分配器分配后经 [`Self::set_reg_field`] 回填。
    fn reg_field(&self, _i: usize) -> u32 {
        0
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

    /// 每个 def 操作数的寄存器约束（与 defs() 并行）。
    fn def_constraints(&self) -> SmallVec<[OperandConstraint; 2]> {
        std::iter::repeat_n(OperandConstraint::Any, self.defs().len()).collect()
    }

    /// 每个 use 操作数的寄存器约束（与 uses() 并行）。
    fn use_constraints(&self) -> SmallVec<[OperandConstraint; 4]> {
        std::iter::repeat_n(OperandConstraint::Any, self.uses().len()).collect()
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
    fn uses(&self) -> SmallVec<[u32; 4]> {
        SmallVec::new()
    }

    fn defs(&self) -> SmallVec<[u32; 2]> {
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
