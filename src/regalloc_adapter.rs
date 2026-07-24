//! 寄存器分配适配器。
//!
//! 内置简单的线性扫描寄存器分配器，无需外部依赖即可使用。
//! 当启用 `regalloc2` feature 时，可切换为使用 regalloc2 crate。

use crate::CompileError;
use crate::backend::{MachineInst, RegMap, vcode::VCode};
use crate::ir::{PReg, RegClass, VReg};
use std::collections::HashMap;

/// 使用内置线性扫描分配器进行寄存器分配。
///
/// `vreg_classes` 提供 VReg → RegClass 映射（Int 或 Float）。
/// `callee_save_regs` 是被调用者保存的寄存器列表（不参与自由分配）。
/// `param_vregs` 是参数 VReg 列表，用于确保参数间寄存器不冲突。
/// `precolored` 是预先分配的 VReg→PReg 映射（如 RAX=VReg(0)）。
/// 返回 [`RegMap`]，包含 VReg→PReg 映射和溢出信息。
#[allow(clippy::too_many_arguments)]
pub fn allocate<I: MachineInst>(
    vcode: &VCode<I>,
    num_gp_regs: u8,
    num_fp_regs: u8,
    sp_index: u8,
    fp_index: Option<u8>,
    vreg_classes: &HashMap<VReg, RegClass>,
    callee_save_regs: &[u8],
    param_vregs: &[VReg],
    precolored: &HashMap<VReg, PReg>,
) -> Result<RegMap, CompileError> {
    let allocator = LinearScanAllocator::new(num_gp_regs, num_fp_regs, sp_index, fp_index);
    allocator.allocate(vcode, vreg_classes, callee_save_regs, param_vregs, precolored)
}

/// 简单的线性扫描寄存器分配器。
struct LinearScanAllocator {
    num_gp_regs: u8,
    num_fp_regs: u8,
    sp_index: u8,
    fp_index: Option<u8>,
}

impl LinearScanAllocator {
    fn new(num_gp_regs: u8, num_fp_regs: u8, sp_index: u8, fp_index: Option<u8>) -> Self {
        Self {
            num_gp_regs,
            num_fp_regs,
            sp_index,
            fp_index,
        }
    }

    fn allocate<I: MachineInst>(
        &self,
        vcode: &VCode<I>,
        vreg_classes: &HashMap<VReg, RegClass>,
        callee_save_regs: &[u8],
        param_vregs: &[VReg],
        precolored: &HashMap<VReg, PReg>,
    ) -> Result<RegMap, CompileError> {
        let mut reg_map = RegMap::new();

        // 预着色 VReg：直接插入映射并从空闲池中移除
        for (vreg, preg) in precolored {
            reg_map.insert(*vreg, *preg);
        }

        let mut live_ranges = self.compute_live_ranges(vcode);
        // 参数 VReg 从函数入口就开始活跃（prologue 中的 arg copy 引用它们），
        // 强制 start=0 确保它们不会与非重叠但实际并发的其他参数共享寄存器。
        for &pv in param_vregs {
            if let Some(range) = live_ranges.get_mut(&pv) {
                range.0 = 0; // 从函数入口开始活跃
            }
        }
        let mut sorted: Vec<_> = live_ranges.iter()
            .filter(|(v, _)| !precolored.contains_key(v)) // 跳过已预着色的
            .collect();
        // 稳定排序：先按起始位置，再按 VReg 编号确保确定性
        sorted.sort_by_key(|(vreg, (start, _))| (*start, vreg.0));

        // 排除 callee-save、栈指针和帧指针，以及已预着色的物理寄存器
        let all_int: Vec<u8> = (0..self.num_gp_regs)
            .filter(|&r| {
                r != self.sp_index && Some(r) != self.fp_index && !callee_save_regs.contains(&r)
                    && !precolored.values().any(|p| p.num == r && p.class == RegClass::Int)
            })
            .collect();
        let all_float: Vec<u8> = (0..self.num_fp_regs)
            .filter(|&r| {
                !precolored.values().any(|p| p.num == r && p.class == RegClass::Float)
            })
            .collect();

        let mut free_int: Vec<u8> = all_int.clone();
        let mut free_float: Vec<u8> = all_float.clone();
        // 活跃间隔列表: (vreg, end_pos, assigned_preg, class)
        let mut active: Vec<(VReg, usize, u8, RegClass)> = Vec::new();
        let mut next_spill_slot: i32 = 0;

        for (vreg, (start, end)) in &sorted {
            let class = vreg_classes.get(vreg).copied().unwrap_or(RegClass::Int);

            // 过期检查：释放已结束的活跃间隔中的寄存器
            active.retain(|(_av, aend, preg, aclass)| {
                if *aend < *start {
                    // 此间隔在当前位置之前已结束，释放寄存器
                    match aclass {
                        RegClass::Int => free_int.push(*preg),
                        RegClass::Float => free_float.push(*preg),
                    }
                    false // remove from active
                } else {
                    true // keep active
                }
            });

            let free_pool = match class {
                RegClass::Int => &mut free_int,
                RegClass::Float => &mut free_float,
            };

            if free_pool.is_empty() {
                reg_map.insert_spill(**vreg, next_spill_slot);
                next_spill_slot += 8;
            } else {
                let preg_num = free_pool.remove(0);
                reg_map.insert(**vreg, PReg::new(preg_num, class));
                active.push((**vreg, *end, preg_num, class));
            }
        }
        Ok(reg_map)
    }
    fn compute_live_ranges<I: MachineInst>(
        &self,
        vcode: &VCode<I>,
    ) -> HashMap<VReg, (usize, usize)> {
        let mut ranges: HashMap<VReg, (usize, usize)> = HashMap::new();
        for block in vcode.blocks() {
            for (j, inst) in block.instructions.iter().enumerate() {
                for &vreg in inst.uses().iter() {
                    let e = ranges.entry(vreg).or_insert((j, j));
                    if j > e.1 { e.1 = j; }
                }
                for &vreg in inst.defs().iter() {
                    let e = ranges.entry(vreg).or_insert((j, j));
                    if j < e.0 { e.0 = j; }
                    if j > e.1 { e.1 = j; }
                }
            }
        }
        ranges
    }
}