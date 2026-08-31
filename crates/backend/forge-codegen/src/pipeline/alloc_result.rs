//! AllocResult — 寄存器分配的完整输出。
//!
//! 替代旧 `AllocResult`，作为分配器与管线后续阶段之间的接口。
//! VCode 在分配后不变，所有分配信息在此 struct 中。

use forge_ir::*;
use std::collections::HashMap;

/// 寄存器分配的完整输出。
///
/// 管线后续阶段（encoder, frame lowering, spill emit）只读取此结构，
/// 不修改 VCode。这是唯一的分配结果类型；旧的 `AllocResult` 已在 v20 删除。
#[derive(Clone, Debug)]
pub struct AllocResult {
    /// XReg → PReg 分配映射
    pub assignments: HashMap<XReg, PReg>,
    /// XReg → 溢出槽（含偏移和宽度）
    pub spill_slots: HashMap<XReg, SpillSlot>,
    /// 函数参数 XReg 列表（用于 prologue 中从 ABI 寄存器复制参数）
    pub param_vregs: Vec<XReg>,
    /// 每个参数的浮点标记（与 param_vregs 对齐）——@move_args 类型分类收参用
    pub param_is_float: Vec<bool>,
    /// 每个参数是否按引用传参（by-ref：宽向量 >16 字节，ABI 传指针；
    /// 被调方入口从 [ptr] 加载到向量寄存器）——@move_args 收参用。
    pub param_by_ref: Vec<bool>,
    /// 是否 sret 返回（宽向量 >16 字节）——首 int 槽被 sret 指针占用，
    /// @move_args 收参 __gi 从 1 起（Windows x64 隐藏 sret 参数约定）。
    pub sret: bool,
    /// 栈参数区字节数（shadow space + 第 5+ 参数槽，见 LowerCtx.
    /// max_stack_arg_bytes）：move_args 收栈参数时计算 spill 槽地址
    ///（sp_base = -(frame) - callee_saved + stack_arg_bytes）需用它。
    pub stack_arg_bytes: u32,
    /// 参数是否为 32 位整数（i32/u32——收参需符号扩展 movsxd）。
    pub param_is_32: Vec<bool>,
    /// 需要在序言中保存的 callee-saved 物理寄存器（按 push 顺序）
    pub callee_saved_to_save: Vec<PReg>,
    /// 帧布局信息
    pub frame_info: FrameInfo,
}

/// 栈帧上的溢出槽。
#[derive(Clone, Copy, Debug)]
pub struct SpillSlot {
    /// 相对帧基准（FP 或 SP）的字节偏移（负数表示低于帧指针）
    pub offset: i32,
    /// 槽大小（字节）— 宽度感知，不再固定 8 字节
    pub size: u8,
}

/// 帧布局信息（由分配器计算，frame lowering 阶段可使用）。
#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    /// 溢出区域总大小（已对齐，字节）
    pub spill_area_size: u32,
    /// 单条指令最多需要的同时 scratch 寄存器数
    /// 用于 frame lowering 验证 scratch 池足够
    pub max_concurrent_spills: usize,
}

impl Default for AllocResult {
    fn default() -> Self {
        Self {
            assignments: HashMap::new(),
            spill_slots: HashMap::new(),
            param_vregs: Vec::new(),
            param_is_float: Vec::new(),
            param_by_ref: Vec::new(),
            sret: false,
            stack_arg_bytes: 0,
            param_is_32: Vec::new(),
            callee_saved_to_save: Vec::new(),
            frame_info: FrameInfo {
                spill_area_size: 0,
                max_concurrent_spills: 0,
            },
        }
    }
}

impl AllocResult {
    // ── 查询方法 ──

    /// 查询 XReg 分配的物理寄存器。
    #[inline]
    pub fn preg(&self, vreg: XReg) -> Option<PReg> {
        self.assignments.get(&vreg).copied()
    }

    /// XReg 是否被溢出到栈上。
    #[inline]
    pub fn is_spilled(&self, vreg: XReg) -> bool {
        self.spill_slots.contains_key(&vreg)
    }

    /// 获取 XReg 的溢出槽信息。
    #[inline]
    pub fn spill_slot(&self, vreg: XReg) -> SpillSlot {
        self.spill_slots[&vreg]
    }
    // ── 构造方法 ──

    /// 为尺寸估算创建假 AllocResult。
    /// VReg(0..n) 循环映射到 PReg(0..15, GPR)。
    pub fn dummy_for_sizing(n: u32) -> Self {
        let mut assignments = HashMap::new();
        for i in 0..n {
            assignments.insert(
                XReg::new(i, RegClass::GPR64),
                PReg::new(i % 16, RegClass::GPR64),
            );
        }
        AllocResult {
            assignments,
            spill_slots: HashMap::new(),
            param_vregs: Vec::new(),
            param_is_float: Vec::new(),
            param_by_ref: Vec::new(),
            sret: false,
            stack_arg_bytes: 0,
            param_is_32: Vec::new(),
            callee_saved_to_save: Vec::new(),
            frame_info: FrameInfo {
                spill_area_size: 0,
                max_concurrent_spills: 0,
            },
        }
    }

    /// 获取 XReg 的寄存器类（从分配信息推断）。
    pub fn vreg_class(&self, vreg: XReg) -> RegClass {
        self.assignments
            .get(&vreg)
            .map(|p| p.class)
            .unwrap_or(RegClass::GPR64)
    }

    // ── 核心 API ──

    /// 创建空 AllocResult。
    pub fn new() -> Self {
        Self::default()
    }

    /// 解析 XReg → PReg，已溢出/未分配时返回 IrError。
    /// 这是 DSL 生成的 emit_inst 代码使用的核心 API。
    pub fn resolve(&self, vreg: XReg) -> Result<PReg, IrError> {
        self.preg(vreg).ok_or_else(|| {
            if self.is_spilled(vreg) {
                IrError::RegAlloc(format!(
                    "vreg {} is spilled to stack (offset {}), not in a register",
                    vreg,
                    self.spill_slot(vreg).offset
                ))
            } else {
                IrError::RegAlloc(format!("unallocated vreg {}", vreg))
            }
        })
    }

    /// 插入 VReg → PReg 映射。
    pub fn insert(&mut self, vreg: XReg, preg: PReg) {
        self.assignments.insert(vreg, preg);
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_preg(idx: u32, class: RegClass) -> PReg {
        PReg::new(idx, class)
    }

    /// 构造第 n 号 GPR 临时寄存器（测试辅助：经 XRegAllocator 受控创建）。
    fn xgpr(n: u32) -> XReg {
        XReg::new(n, RegClass::GPR64)
    }

    /// 构造第 n 号 FPR 临时寄存器。
    fn xfpr(n: u32) -> XReg {
        let mut xa = XRegAllocator::new();
        for _ in 0..n {
            xa.alloc_default(RegClass::FPR64);
        }
        xa.alloc_default(RegClass::FPR64)
    }

    #[test]
    fn test_is_spilled_and_spill_slot() {
        let mut result = AllocResult::new();
        let v = xgpr(5);
        assert!(!result.is_spilled(v));

        result.spill_slots.insert(
            v,
            SpillSlot {
                offset: -16,
                size: 8,
            },
        );
        assert!(result.is_spilled(v));
        assert_eq!(result.spill_slot(v).offset, -16);
        assert_eq!(result.spill_slot(v).size, 8);
    }

    #[test]
    fn test_resolve_success() {
        let mut result = AllocResult::new();
        let p = make_preg(5, RegClass::FPR64);
        result.insert(xfpr(3), p);
        assert_eq!(result.resolve(xfpr(3)).unwrap(), p);
    }

    #[test]
    fn test_resolve_spilled_error() {
        let mut result = AllocResult::new();
        result.spill_slots.insert(
            xgpr(4),
            SpillSlot {
                offset: -8,
                size: 4,
            },
        );
        let err = result.resolve(xgpr(4)).unwrap_err();
        assert!(format!("{}", err).contains("spilled"));
    }

    #[test]
    fn test_resolve_unallocated_error() {
        let result = AllocResult::new();
        let err = result.resolve(xgpr(99)).unwrap_err();
        assert!(format!("{}", err).contains("unallocated"));
    }

    #[test]
    fn test_vreg_class() {
        let mut result = AllocResult::new();
        result.insert(xgpr(1), make_preg(0, RegClass::GPR64));
        result.insert(xfpr(2), make_preg(0, RegClass::FPR64));
        assert_eq!(result.vreg_class(xgpr(1)), RegClass::GPR64);
        assert_eq!(result.vreg_class(xfpr(2)), RegClass::FPR64);
        assert_eq!(result.vreg_class(xgpr(99)), RegClass::GPR64); // default
    }

    #[test]
    fn test_dummy_for_sizing() {
        let r = AllocResult::dummy_for_sizing(32);
        assert_eq!(r.assignments.len(), 32);
        assert_eq!(r.preg(xgpr(0)), Some(make_preg(0, RegClass::GPR64)));
        assert_eq!(r.preg(xgpr(16)), Some(make_preg(0, RegClass::GPR64)));
    }

    #[test]
    fn test_default() {
        let r = AllocResult::default();
        assert!(r.assignments.is_empty());
        assert_eq!(r.frame_info.spill_area_size, 0);
        assert_eq!(r.frame_info.max_concurrent_spills, 0);
    }
}
