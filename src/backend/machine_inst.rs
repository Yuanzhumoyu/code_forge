//! MachineInst trait — v9 重构的机器指令接口。
//!
//! 替代旧的 `MachineInstruction` trait，新增效果系统、内存访问建模、
//! 副作用标签等能力。

use crate::ir::*;
use smallvec::SmallVec;
use std::hash::Hash;

// ============================================================
// EffectKind — 效果种类
// ============================================================

/// 指令效果种类 — 对应 DSL 中 `[effect]` 系统的运行时表示。
///
/// 用于指导优化器（如哪些指令可删除、重排）和寄存器分配器。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectKind {
    /// 纯计算（无副作用，可删除/重排）。
    Pure,
    /// 内存读取。
    Read,
    /// 内存写入。
    Write,
    /// 条件分支。
    Branch,
    /// 无条件跳转。
    Jump,
    /// 陷阱/系统调用。
    Trap,
    /// 自定义扩展效果（架构特定）。
    Custom(u8),
}

impl EffectKind {
    /// 是否有内存访问。
    pub fn is_memory_access(&self) -> bool {
        matches!(self, Self::Read | Self::Write)
    }

    /// 是否可能修改架构状态（除目标寄存器外）。
    pub fn has_side_effects(&self) -> bool {
        !matches!(self, Self::Pure)
    }
}

// ============================================================
// MemAccess — 内存访问描述
// ============================================================

/// 内存访问描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemAccess {
    /// 访问类型：读/写/读写。
    pub kind: MemAccessKind,
    /// 访问大小（字节）。
    pub size: u8,
}

/// 内存访问类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemAccessKind {
    /// 加载（读）。
    Load,
    /// 存储（写）。
    Store,
    /// 原子读-修改-写。
    LoadStore,
}

// ============================================================
// MachineInst trait
// ============================================================

/// 机器指令 trait — v9 重构版本。
///
/// 替代旧的 `MachineInstruction`。新增效果系统、内存访问信息。
/// 由 ISA-DSL 生成的 Inst 枚举实现，或由手动 ISA 实现。
pub trait MachineInst: Clone + std::fmt::Debug + Send + Sync + Hash + Eq {
    /// 指令使用的虚拟寄存器（读取的寄存器）。
    fn uses(&self) -> SmallVec<[VReg; 4]>;

    /// 指令定义的虚拟寄存器（写入的寄存器）。
    fn defs(&self) -> SmallVec<[VReg; 2]>;

    /// 指令的效果标签列表（来自 DSL 的 effect 系统）。
    ///
    /// 默认实现检查 uses/defs/memory_access 推导：
    /// - 有 defs 且无 memory_access → Pure
    /// - 有 memory_access(Load) → Read
    /// - 有 memory_access(Store) → Write
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

    /// 是否为分支指令。
    fn is_branch(&self) -> bool;

    /// 分支目标块列表。
    fn branch_targets(&self) -> SmallVec<[BlockId; 2]>;

    /// 是否为调用指令。
    fn is_call(&self) -> bool;

    /// 是否为返回指令。
    fn is_ret(&self) -> bool;

    /// 是否为终止指令（分支、跳转、返回等控制流结束指令）。
    fn is_terminator(&self) -> bool {
        self.is_branch() || self.is_ret()
    }

    /// 内存访问信息（如果有）。
    fn memory_access(&self) -> Option<MemAccess> {
        None
    }

    /// 是否有副作用（不能删除或重排）。
    fn has_side_effects(&self) -> bool {
        self.is_call() || self.is_ret() || self.is_branch() || self.memory_access().is_some()
    }

    /// 寄存器移动信息。如果是 reg-to-reg 移动，返回 (dst, src)。
    fn is_move(&self) -> Option<(VReg, VReg)> {
        None
    }

    /// 指令是否可折叠（无副作用且可删除）。
    fn is_foldable(&self) -> bool {
        !self.has_side_effects()
    }

    /// 指令可能 clobber 的物理寄存器列表（用于调用指令）。
    fn clobbers(&self) -> &[u8] {
        &[]
    }
}
