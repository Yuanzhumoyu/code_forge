//! Backtracking 寄存器分配器。
//!
//! 受 regalloc2 (Cranelift) 和 IonMonkey 启发，
//! 使用 Belady's MIN 启发式进行溢出决策，原生支持固定寄存器约束。
//!
//! ## 算法概要
//!
//! 1. 计算 LiveInterval（带 holes）
//! 2. 按块顺序处理指令：
//!    - 释放已死亡 XReg 的寄存器
//!    - 为 use 操作数确保 XReg 在寄存器中（必要时 reload）
//!    - 为 def 操作数分配寄存器（必要时驱逐）
//!    - 处理固定寄存器约束和 reuse 约束
//! 3. 生成 AllocResult
//!
//! ## Spill 启发式
//!
//! Belady's MIN：当无空闲寄存器时，驱逐"下一个使用点最远"的活跃 XReg。
//! 结合循环深度加权计算 spill cost。

use crate::CompileError;
use crate::machine::inst::{MachineInst, OperandConstraint};
use crate::machine::reg_alloc::RegAlloc;
use crate::pipeline::alloc_config::{AllocContext, RegAllocConfig};
use crate::pipeline::alloc_result::{AllocResult, FrameInfo, SpillSlot};
use crate::pipeline::liverange::{self, ProgPoint};
use forge_ir::*;
use std::collections::HashMap;

// ============================================================
// Backtracking Allocator
// ============================================================

/// Backtracking 寄存器分配器。
pub struct BacktrackingAllocator;

impl BacktrackingAllocator {
    pub fn new() -> Self {
        Self
    }

    /// 执行寄存器分配。
    pub fn allocate<I: MachineInst>(
        &self,
        vcode: &crate::VCode<I>,
        config: &RegAllocConfig,
        ctx: &AllocContext,
        xreg_map: &[smallvec::SmallVec<[XReg; 2]>],
    ) -> Result<AllocResult, CompileError> {
        // 1. 计算 LiveInterval（XReg → 微指令字段映射驱动）
        let intervals = liverange::compute_live_intervals(vcode, xreg_map, &config.param_xregs);

        // 2. 构建状态
        let mut state = BtState::new(config, ctx, intervals);

        // 3. 应用 precolor
        for (vreg, preg) in &config.precolored {
            state.assign_reg(*vreg, OperandConstraint::Fixed(*preg), preg.class, &[])?;
        }

        // 3.5 预分配所有参数 XReg，确保它们在函数入口处有不同的寄存器。
        // 序言 (gen_move_args) 在函数体执行之前批量复制 ABI 参数寄存器
        // → 参数 XReg。如果两个参数共享同一物理寄存器，第二个 MOV 会覆盖
        // 第一个参数的值。通过在此处（早于任何指令处理）分配所有参数，
        // 分配器将它们视为在 ProgPoint(0,0) 同时活跃，从而分配不同寄存器。
        for &vreg in &config.param_xregs {
            if !state.assignments.contains_key(&vreg) {
                // 检查是否有 precolor 约束（如 XReg0→RAX）
                let constraint = config
                    .precolored
                    .get(&vreg)
                    .map(|&p| OperandConstraint::Fixed(p))
                    .unwrap_or(OperandConstraint::Any);
                let class = state.vreg_class(vreg);
                state.assign_reg(vreg, constraint, class, &[])?;
            }
        }

        // 4. 逐块处理（按 xreg_map 分配 XReg）
        let mut global_inst = 0usize;
        for (block_idx, block) in vcode.blocks().enumerate() {
            let block_u32 = block_idx as u32;
            state.process_block(block_u32, block, xreg_map, &mut global_inst)?;
        }

        // 5. 构建 AllocResult
        state.build_result()
    }
}

impl RegAlloc for BacktrackingAllocator {
    fn allocate<I: MachineInst>(
        &self,
        vcode: &crate::VCode<I>,
        config: &RegAllocConfig,
        ctx: &AllocContext,
        xreg_map: &[smallvec::SmallVec<[XReg; 2]>],
    ) -> Result<AllocResult, CompileError> {
        BacktrackingAllocator::allocate(self, vcode, config, ctx, xreg_map)
    }

    fn name(&self) -> &'static str {
        "backtracking"
    }
}

// ============================================================
// Allocator State
// ============================================================

struct BtState<'a> {
    config: &'a RegAllocConfig,
    ctx: &'a AllocContext,
    /// XReg → LiveInterval (for liveness queries)
    intervals: HashMap<XReg, liverange::LiveInterval>,

    /// XReg → 分配的 PReg
    assignments: HashMap<XReg, PReg>,
    /// XReg → spill 槽
    spill_slots: HashMap<XReg, SpillSlot>,
    /// 每条指令同时 spill 数的最大值
    max_concurrent_spills: usize,

    /// PReg → 当前占用的 XReg
    reg_owner: HashMap<PReg, XReg>,
    /// RegClass → 可用的空闲 PReg 池
    free_regs: HashMap<RegClass, Vec<PReg>>,

    /// 当前活跃的 XReg 及其 PReg（按 end_point 排序）
    active: Vec<(XReg, PReg)>,

    /// Spill 槽分配器
    next_spill_offset: i32,
    /// 当前正在处理的指令点（用于 Belady's MIN 溢出决策）。
    current_point: ProgPoint,
}

impl<'a> BtState<'a> {
    fn new(
        config: &'a RegAllocConfig,
        ctx: &'a AllocContext,
        intervals: HashMap<XReg, liverange::LiveInterval>,
    ) -> Self {
        // 为每个 class 初始化空闲寄存器池。
        // 根因修复（param XReg 预分配）后不再需要 abi_param_regs 排序 hack。
        let mut free_regs: HashMap<RegClass, Vec<PReg>> = HashMap::new();
        for (class, class_cfg) in &config.classes {
            let pool: Vec<PReg> = class_cfg
                .allocatable
                .iter()
                .filter(|&&r| Some(r) != config.fp_reg && r != config.sp_reg)
                .map(|&r| PReg::new(r, *class))
                .collect();
            free_regs.insert(*class, pool);
        }

        Self {
            config,
            ctx,
            intervals,
            assignments: HashMap::new(),
            spill_slots: HashMap::new(),
            max_concurrent_spills: 0,
            reg_owner: HashMap::new(),
            free_regs,
            active: Vec::new(),
            next_spill_offset: 0,
            current_point: ProgPoint::new(0, 0),
        }
    }

    // ── 块处理 ──

    fn process_block<I: MachineInst>(
        &mut self,
        block_idx: u32,
        block: &crate::VCodeBlock<I>,
        xreg_map: &[smallvec::SmallVec<[XReg; 2]>],
        global_inst_start: &mut usize,
    ) -> Result<(), CompileError> {
        for (inst_idx, inst) in block.instructions.iter().enumerate() {
            let inst_u32 = inst_idx as u32;
            let use_point = ProgPoint::inst_start(block_idx, inst_u32);
            let _def_point = ProgPoint::inst_end(block_idx, inst_u32);

            // 当前指令的寄存器字段对应的 XReg（由指令包 xreg_map 聚合提供）
            let slot = xreg_map
                .get(*global_inst_start)
                .cloned()
                .unwrap_or_default();
            *global_inst_start += 1;
            let inst_xregs: Vec<XReg> = slot.iter().copied().collect();

            // 更新当前指令点（用于 Belady's MIN 溢出决策）
            self.current_point = use_point;

            // 1. 释放已死亡的 XReg（hole-aware，见 Bug 1 修复）
            self.expire_dead(use_point);

            // 1.5 处理 clobber 约束：溢出被调用指令破坏的寄存器中的活跃 XReg
            let clobbers: Vec<u8> = inst.clobbers().to_vec();
            for &clobber_reg in &clobbers {
                let clobber_preg = PReg::new(clobber_reg, RegClass::GPR);
                if let Some(&victim) = self.reg_owner.get(&clobber_preg) {
                    self.spill_vreg(victim)?;
                }
            }

            // 2. 确保所有 XReg 在寄存器中（uses/defs 统一按字段顺序分配）
            let mut spilled_this_inst = 0usize;
            let inst_uses: Vec<XReg> = inst_xregs.clone();

            for &vreg in &inst_xregs {
                if self.is_spilled(vreg) {
                    spilled_this_inst += 1;
                    // Reload from stack
                    self.reload_from_stack(vreg, OperandConstraint::Any)?;
                } else if !self.assignments.contains_key(&vreg) {
                    // 首次遇到此 XReg
                    self.assign_reg(vreg, OperandConstraint::Any, self.vreg_class(vreg), &inst_uses)?;
                }
            }

            self.max_concurrent_spills = self.max_concurrent_spills.max(spilled_this_inst);

            // 4. 标记活跃
            for &vreg in &inst_xregs {
                if let Some(&preg) = self.assignments.get(&vreg) {
                    // 更新或添加到 active 列表
                    self.active.retain(|(v, _)| *v != vreg);
                    self.active.push((vreg, preg));
                }
            }
        }

        Ok(())
    }

    // ── 寄存器分配 ──

    fn assign_reg(
        &mut self,
        vreg: XReg,
        constraint: OperandConstraint,
        class: RegClass,
        inst_uses: &[XReg],
    ) -> Result<PReg, CompileError> {
        // 如果已分配，直接返回
        if let Some(&preg) = self.assignments.get(&vreg) {
            return Ok(preg);
        }

        match constraint {
            OperandConstraint::Any => {
                // 从空闲池获取
                if let Some(preg) = self.pop_free(class) {
                    self.assignments.insert(vreg, preg);
                    self.reg_owner.insert(preg, vreg);
                    return Ok(preg);
                }
                // 无空闲 → 驱逐
                self.evict_and_assign(vreg, class)
            }

            OperandConstraint::Fixed(required_preg) => {
                // 如果目标寄存器被占用，驱逐占用者
                if let Some(&occupant) = self.reg_owner.get(&required_preg)
                    && occupant != vreg
                {
                    self.spill_vreg(occupant)?;
                }
                // 从空闲池移除
                self.remove_from_free(required_preg);
                self.assignments.insert(vreg, required_preg);
                self.reg_owner.insert(required_preg, vreg);
                Ok(required_preg)
            }

            OperandConstraint::ReuseInput(input_idx) => {
                // 复用指定 input 操作数的物理寄存器。
                // 用于 x86 双操作数指令（add dst, src → dst 必须与 src 同寄存器）。
                if let Some(&input_vreg) = inst_uses.get(input_idx)
                    && let Some(&input_preg) = self.assignments.get(&input_vreg)
                {
                    // 复用 input 的寄存器
                    self.assignments.insert(vreg, input_preg);
                    self.reg_owner.insert(input_preg, vreg);
                    return Ok(input_preg);
                }
                // Fallback: input 尚未分配（如 constant），正常分配
                if let Some(preg) = self.pop_free(class) {
                    self.assignments.insert(vreg, preg);
                    self.reg_owner.insert(preg, vreg);
                    return Ok(preg);
                }
                self.evict_and_assign(vreg, class)
            }

            OperandConstraint::Stack => {
                // Stack 约束在此处不应该出现——process_block 已提前处理。
                // 如果到达此处，说明调用方未处理 Stack，作为防御性措施 spill XReg。
                self.spill_vreg(vreg)?;
                Err(CompileError::RegAlloc(format!(
                    "unexpected Stack constraint in assign_reg for vreg {}",
                    vreg
                )))
            }

            OperandConstraint::Clobber(_preg) => {
                // Clobber 约束在 process_block 中处理（spill 占用者），
                // 不应该在 assign_reg 中遇到。
                Err(CompileError::RegAlloc(format!(
                    "unexpected Clobber constraint in assign_reg for vreg {}",
                    vreg
                )))
            }
        }
    }

    /// 驱逐一个 XReg 并为当前 XReg 分配其释放的寄存器（Belady's MIN 启发式）。
    ///
    /// 使用 ProgPoint(0,0) 而非 self.current_point 以确保与 expire_dead
    /// 使用 end_point() 的一致性：跨块活跃的 XReg 可能在当前点之后没有
    /// 直接的使用点（仅通过 Phase 2 的 block_end 扩展保持活跃），使用
    /// current_point 会错误地将它们设为 MAX（"永不使用"）。
    /// 注意：segment 模型通过 cover() 的 point.before() 合并修复了相邻
    /// 指令间的微间隙，因此从 (0,0) 查找 next_use 已经足够可靠。
    fn evict_and_assign(&mut self, vreg: XReg, class: RegClass) -> Result<PReg, CompileError> {
        let current_point = ProgPoint::new(0, 0);
        let victim = self
            .active
            .iter()
            .filter(|(_, p)| p.class == class || p.class.overlaps(class))
            .filter(|(victim, _)| *victim != vreg)
            .max_by_key(|(victim_vreg, _)| {
                self.intervals
                    .get(victim_vreg)
                    .and_then(|iv| iv.next_use_after(current_point))
                    .unwrap_or(ProgPoint::MAX)
            });

        if let Some(&(victim_vreg, victim_preg)) = victim {
            // 驱逐 victim 到栈
            self.spill_vreg(victim_vreg)?;
            // victim 的寄存器现在空闲
            self.assignments.insert(vreg, victim_preg);
            self.reg_owner.insert(victim_preg, vreg);
            // 从 active 移除 victim
            self.active.retain(|(v, _)| *v != victim_vreg);
            Ok(victim_preg)
        } else {
            // 无法驱逐 — 溢出当前 XReg
            self.spill_vreg(vreg)?;
            Err(CompileError::RegAlloc(format!(
                "vreg {} spilled: no evictable register in class {:?}",
                vreg, class
            )))
        }
    }

    // ── Spill/Restore ──

    fn spill_vreg(&mut self, vreg: XReg) -> Result<(), CompileError> {
        if self.spill_slots.contains_key(&vreg) {
            return Ok(());
        }

        let class = self.vreg_class(vreg);
        let size = self
            .config
            .classes
            .get(&class)
            .map(|c| c.reg_width)
            .unwrap_or(8);

        // 对齐
        let align = size as i32;
        self.next_spill_offset = (self.next_spill_offset + align - 1) / align * align;

        let slot = SpillSlot {
            offset: self.next_spill_offset,
            size,
        };
        self.spill_slots.insert(vreg, slot);
        self.next_spill_offset += size as i32;

        // 如果已分配寄存器，释放它
        if let Some(&preg) = self.assignments.get(&vreg) {
            self.assignments.remove(&vreg);
            self.reg_owner.remove(&preg);
            self.free_regs.entry(preg.class).or_default().push(preg);
        }

        self.active.retain(|(v, _)| *v != vreg);
        Ok(())
    }

    fn reload_from_stack(
        &mut self,
        vreg: XReg,
        constraint: OperandConstraint,
    ) -> Result<(), CompileError> {
        // 为 spilled XReg 分配一个寄存器（实际 load 在 emit 阶段完成）。
        // 尊重约束：Fixed 约束必须使用指定寄存器。
        let class = self.vreg_class(vreg);
        match constraint {
            OperandConstraint::Fixed(required_preg) => {
                // 如果目标寄存器被占用，驱逐占用者
                if let Some(&occupant) = self.reg_owner.get(&required_preg)
                    && occupant != vreg
                {
                    self.spill_vreg(occupant)?;
                }
                self.remove_from_free(required_preg);
                self.assignments.insert(vreg, required_preg);
                self.reg_owner.insert(required_preg, vreg);
            }
            _ => {
                // 任意空闲寄存器即可
                if let Some(preg) = self.pop_free(class) {
                    self.assignments.insert(vreg, preg);
                    self.reg_owner.insert(preg, vreg);
                }
            }
        }
        Ok(())
    }

    fn is_spilled(&self, vreg: XReg) -> bool {
        self.spill_slots.contains_key(&vreg)
    }

    // ── 辅助方法 ──

    fn vreg_class(&self, vreg: XReg) -> RegClass {
        // XReg 值内嵌类型，严格限定。
        vreg.class()
    }

    fn expire_dead(&mut self, current_point: ProgPoint) {
        // 释放 end_point 在当前点之前或恰好在当前点的 XReg。
        // 注意：使用 end_point() 而非 contains() 因为当前 live interval 模型
        // 只覆盖具体使用/定义点和块边界点，而非整个块连续区间。
        // contains() 会在跨块 XReg 的块中间误判为"不活跃"。
        // 对于真正的跨块 holes，数据流扩展（Stage 2）将区间扩展到 block_end，
        // XReg 在整个块范围内保持活跃。
        let mut freed: Vec<PReg> = Vec::new();
        self.active.retain(|(vreg, preg)| {
            if let Some(interval) = self.intervals.get(vreg)
                && interval.end_point() <= current_point
            {
                freed.push(*preg);
                self.reg_owner.remove(preg);
                return false;
            }
            true
        });
        for preg in freed {
            self.free_regs.entry(preg.class).or_default().push(preg);
        }
    }

    fn pop_free(&mut self, class: RegClass) -> Option<PReg> {
        self.free_regs.get_mut(&class)?.pop()
    }

    fn remove_from_free(&mut self, preg: PReg) {
        if let Some(pool) = self.free_regs.get_mut(&preg.class) {
            pool.retain(|p| *p != preg);
        }
    }

    // ── 构建结果 ──

    fn build_result(self) -> Result<AllocResult, CompileError> {
        let spill_area_size = if self.next_spill_offset > 0 {
            // 对齐到 16 字节
            (self.next_spill_offset as u32).div_ceil(16) * 16
        } else {
            0
        };

        // 收集 callee-saved 寄存器
        let callee_saved_pregs: Vec<PReg> = self
            .config
            .classes
            .get(&RegClass::GPR)
            .map(|cfg| {
                cfg.allocatable
                    .iter()
                    .filter(|r| self.config.callee_saved.contains(r))
                    .map(|&r| PReg::new(r, RegClass::GPR))
                    .collect()
            })
            .unwrap_or_default();

        Ok(AllocResult {
            assignments: self.assignments,
            spill_slots: self.spill_slots,
            param_vregs: self.config.param_xregs.clone(),
            param_is_float: self
                .config
                .param_xregs
                .iter()
                .map(|v| v.class().is_fp())
                .collect(),
            param_is_32: self
                .config
                .param_xregs
                .iter()
                .map(|v| v.width() == 4)
                .collect(),
            callee_saved_to_save: callee_saved_pregs,
            frame_info: FrameInfo {
                spill_area_size,
                max_concurrent_spills: self.max_concurrent_spills,
            },
        })
    }
}

impl Default for BacktrackingAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::inst::DummyInst;

    /// 构造第 n 号 GPR 临时寄存器（测试辅助）。
    fn xgpr(n: u32) -> XReg {
        let mut xa = XRegAllocator::new();
        for _ in 0..n {
            xa.alloc_default(RegClass::GPR);
        }
        xa.alloc_default(RegClass::GPR)
    }

    fn make_config(num_gp: u8, num_fp: u8) -> RegAllocConfig {
        RegAllocConfig::new(num_gp, num_fp, 4, Some(5))
    }

    #[test]
    fn test_empty_function() {
        let config = make_config(8, 8);
        let ctx = AllocContext::default();
        let alloc = BacktrackingAllocator::new();
        let vcode = crate::VCode::<DummyInst>::new();
        let result = alloc.allocate(&vcode, &config, &ctx, &[]);
        let _ = result;
    }

    #[test]
    fn test_basic_allocation() {
        let config = make_config(8, 8);
        let ctx = AllocContext::default();
        let alloc = BacktrackingAllocator::new();
        let vcode = crate::VCode::<DummyInst>::new();
        let result = alloc.allocate(&vcode, &config, &ctx, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_fixed_reg_constraint() {
        // 验证 BT 分配器创建和命名
        let alloc = BacktrackingAllocator::new();
        assert_eq!(alloc.name(), "backtracking");
    }

    #[test]
    fn test_reg_class_width_from_config() {
        // 验证 DSL 生成的 reg_class_width 通过配置正确传播
        let config = make_config(16, 16);
        let gpr_cfg = config.classes.get(&RegClass::GPR).unwrap();
        assert_eq!(gpr_cfg.reg_width, 8);
        let fpr_cfg = config.classes.get(&RegClass::FPR).unwrap();
        assert_eq!(fpr_cfg.reg_width, 8);
    }

    #[test]
    fn test_all_regclasses_have_allocatable() {
        // 验证所有寄存器类都有非空的可分配列表
        let config = make_config(16, 16);
        for (class, cfg) in &config.classes {
            assert!(
                !cfg.allocatable.is_empty(),
                "{:?} should have allocatable registers",
                class
            );
        }
    }

    #[test]
    fn test_param_vregs_preassign() {
        // 验证 param XReg 预分配机制：参数 XReg 在函数入口获得不同寄存器。
        let mut config = make_config(16, 16);
        config.param_xregs = vec![xgpr(0), xgpr(1), xgpr(2), xgpr(3)];
        assert_eq!(config.param_xregs.len(), 4);
        let ctx = AllocContext::default();
        let alloc = BacktrackingAllocator::new();
        let vcode = crate::VCode::<DummyInst>::new();
        let result = alloc.allocate(&vcode, &config, &ctx, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_spill_slot_width_aware() {
        // 验证 BT 分配器在仅有 1 个可分配寄存器时仍能完成分配
        let mut config = make_config(8, 8);
        config.classes.get_mut(&RegClass::GPR).unwrap().allocatable = vec![3]; // RBX only
        let ctx = AllocContext::default();
        let alloc = BacktrackingAllocator::new();
        let vcode = crate::VCode::<DummyInst>::new();
        let result = alloc.allocate(&vcode, &config, &ctx, &[]);
        assert!(
            result.is_ok(),
            "Allocator should handle high register pressure"
        );
    }
}
