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

use crate::IrError;
use crate::machine::inst::{MachineInst, OperandConstraint};
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
        xreg_map: &[smallvec::SmallVec<[(XReg, u8, bool); 2]>],
        clobber_map: &[Vec<(u32, crate::prelude::RegClass)>],
    ) -> Result<AllocResult, IrError> {
        // 1. 计算 LiveInterval（XReg → 微指令字段映射驱动）
        let intervals = liverange::compute_live_intervals(vcode, xreg_map, &config.param_xregs);

        // 2. 构建状态
        let _ = ctx; // AllocContext 预留（将来约束上下文）
        let mut state = BtState::new(config, intervals);

        // 3. 预分配前 param_reg_count 个参数 XReg，确保它们在函数入口处有
        // 不同的寄存器。序言 (gen_move_args) 在函数体执行之前批量复制 ABI
        // 参数寄存器 → 参数 XReg。如果两个参数共享同一物理寄存器，第二个
        // MOV 会覆盖第一个参数的值。通过在此处（早于任何指令处理）分配
        // 这些参数，分配器将它们视为在 ProgPoint(0,0) 同时活跃，从而分配
        // 不同寄存器。
        // 注意：dead 参数（函数体无 use 的 XReg）区间为 [0,0]，与活参数的重叠
        // 判定（end > start）不成立，会被复用活参数的寄存器——但 @move_args
        // 的收参 mov 实际写入该寄存器，在活参数 store 前覆盖其值（混合参数
        // 场景：f(i32, f64) 的 b 覆盖 a → a=0）。因此 dead 参数不预分配，
        // gen_move_args 依 assignments 跳过其收参 mov。
        // 栈参数（下标 ≥ param_reg_count，Windows x64 第 5+）不预分配寄存器：
        // 直接 spill 到栈槽（gen_move_args 从 ABI 栈槽 load 到 spill 槽），
        // 避免参数占满寄存器 → 函数体无寄存器可驱逐（grow_impl_runtime
        // 7 参数 regalloc 失败）。AllocResult.spill_slot 可查其槽偏移。
        for (pi, &vreg) in config.param_xregs.iter().enumerate() {
            if pi >= config.param_reg_count {
                // 栈参数：强制 spill（分配槽 + 不占寄存器）
                if !state.spill_slots.contains_key(&vreg) {
                    let _ = state.spill_vreg(vreg);
                }
                continue;
            }
            if !state.assignments.contains_key(&vreg) {
                let live = state.intervals.get(&vreg);
                let is_dead = live
                    .map(|iv| iv.uses.is_empty() && iv.defs.is_empty())
                    .unwrap_or(false);
                if is_dead {
                    continue;
                }
                let constraint = OperandConstraint::Any;
                let class = state.vreg_class(vreg);
                let preg = state.assign_reg(vreg, constraint, class, &[], true)?;
                state.active.insert(vreg, preg); // 将预分配的参数 XReg 添加到 active 列表中
                if crate::pipeline::trace_enabled("FORGE_TRACE_REGALLOC") {
                    eprintln!("[pre] v{} class={class:?} -> {preg:?}", vreg.index());
                }
            }
        }

        // 4. 逐块处理（按 xreg_map 分配 XReg）
        let mut global_inst = 0usize;
        for (block_idx, block) in vcode.blocks().enumerate() {
            let block_u32 = block_idx as u32;
            state.process_block(block_u32, block, xreg_map, clobber_map, &mut global_inst)?;
        }

        // 5. 构建 AllocResult
        state.build_result()
    }
}

// ============================================================
// Allocator State
// ============================================================

struct BtState<'a> {
    config: &'a RegAllocConfig,
    /// XReg → LiveInterval (for liveness queries)
    intervals: HashMap<XReg, liverange::LiveInterval>,

    /// XReg → 分配的 PReg
    assignments: HashMap<XReg, PReg>,
    /// XReg → spill 槽
    spill_slots: HashMap<XReg, SpillSlot>,
    /// 每条指令同时 spill 数的最大值
    max_concurrent_spills: usize,

    /// 物理寄存器编号 → 该编号上的占用者（跨类；同编号的 GPR 与 FPR 是不同
    /// 物理寄存器、可共存，冲突类（如 GPR(4)/GPR(8)）只能有一个）。
    /// 数组索引 = PReg.num，槽位内 SmallVec 通常 ≤2 项 —— 冲突检测 O(1)。
    reg_owner: Vec<smallvec::SmallVec<[(RegClass, XReg); 2]>>,
    /// RegClass → 可用的空闲 PReg 池
    free_regs: HashMap<RegClass, Vec<PReg>>,

    /// 当前活跃的 XReg 及其 PReg（插入/移除 O(1)）
    active: HashMap<XReg, PReg>,

    /// Spill 槽分配器
    next_spill_offset: i32,
    /// 当前正在处理的指令点（用于 Belady's MIN 溢出决策）。
    current_point: ProgPoint,
    /// 当前指令点的写死物理寄存器（来自 clobber_map）：本点内分配必须避开。
    current_clobbers: smallvec::SmallVec<[PReg; 4]>,
}

impl<'a> BtState<'a> {
    fn new(config: &'a RegAllocConfig, intervals: HashMap<XReg, liverange::LiveInterval>) -> Self {
        // 为每个 class 初始化空闲寄存器池。
        // 根因修复（param XReg 预分配）后不再需要 abi_param_regs 排序 hack。
        let mut free_regs: HashMap<RegClass, Vec<PReg>> = HashMap::new();
        for (class, class_cfg) in &config.classes {
            let mut pool: Vec<PReg> = class_cfg
                .allocatable
                .iter()
                .filter(|&&r| Some(r) != config.fp_reg && r != config.sp_reg)
                .map(|&r| PReg::new(r, *class))
                .collect();
            // FPR 池反向（从低编号 XMM0 起分配），与 GPR 池（高编号 r15 起）错开。
            // 原因：MOVQ_XMM_FREG 的编码（66 REX.W 0F 7E）把 dest 的 XMM 寄存器
            // 作为 GPR r/m 写入——XMM15 实际写 r15。若 GPR 与 FPR 同号（如 a→r15、
            // b→XMM15），浮点参数收参的 movq r15,xmm0 会覆盖整数参数 a（混合参数
            // 场景 f(i32,f64) 的 a 被 b 覆盖 → a=0）。
            if *class == config.main_fpr_class {
                pool.reverse();
            }
            free_regs.insert(*class, pool);
        }

        Self {
            config,
            intervals,
            assignments: HashMap::new(),
            spill_slots: HashMap::new(),
            max_concurrent_spills: 0,
            reg_owner: Vec::new(),
            free_regs,
            active: HashMap::new(),
            next_spill_offset: 0,
            current_point: ProgPoint::new(0, 0),
            current_clobbers: smallvec::SmallVec::new(),
        }
    }

    // ── 块处理 ──

    fn process_block<I: MachineInst>(
        &mut self,
        block_idx: u32,
        block: &crate::VCodeBlock<I>,
        xreg_map: &[smallvec::SmallVec<[(XReg, u8, bool); 2]>],
        clobber_map: &[Vec<(u32, crate::prelude::RegClass)>],
        global_inst_start: &mut usize,
    ) -> Result<(), IrError> {
        for (inst_idx, inst) in block.instructions.iter().enumerate() {
            let inst_u32 = inst_idx as u32;
            let use_point = ProgPoint::inst_start(block_idx, inst_u32);
            let _def_point = ProgPoint::inst_end(block_idx, inst_u32);

            // 当前指令的寄存器字段对应的 XReg（由指令包 xreg_map 聚合提供）
            // 借用 slice，避免每指令克隆 SmallVec
            let slot: &[(XReg, u8, bool)] = xreg_map
                .get(*global_inst_start)
                .map(|s| s.as_slice())
                .unwrap_or(&[]);
            *global_inst_start += 1;
            let inst_xregs: smallvec::SmallVec<[XReg; 8]> =
                slot.iter().map(|&(x, _fi, _d)| x).collect();
            let inst_defs: smallvec::SmallVec<[XReg; 8]> = slot
                .iter()
                .filter(|&&(_, _fi, d)| d)
                .map(|&(x, _fi, _d)| x)
                .collect();

            // 更新当前指令点（用于 Belady's MIN 溢出决策）
            self.current_point = use_point;

            if crate::pipeline::trace_enabled("FORGE_TRACE_ALLOC") {
                eprintln!(
                    "[inst {:?}] {:?} xregs={:?} defs={:?}",
                    use_point,
                    inst,
                    inst_xregs.iter().map(|x| x.index()).collect::<Vec<_>>(),
                    inst_defs.iter().map(|x| x.index()).collect::<Vec<_>>()
                );
            }

            // 1. 释放已死亡的 XReg（hole-aware，见 Bug 1 修复）
            self.expire_dead(use_point);

            // 1.5 处理 clobber 约束：溢出被写死物理寄存器占用处破坏的活跃 XReg
            // 规则级（clobber_map：insts 显式物理寄存器自动推导）+ 指令级
            // （inst.clobbers()：implicit 声明的隐式破坏，如 cqo 的 RDX）合并。
            // 先构建到局部 Vec（迭代 self.current_clobbers 时需 &mut self），
            // 处理完后再写入字段供 Belady 决策使用。
            let clobbers: smallvec::SmallVec<[PReg; 4]> = {
                let mut v = smallvec::SmallVec::new();
                if let Some(cs) = clobber_map.get(*global_inst_start - 1) {
                    v.extend(cs.iter().map(|&(c, class)| PReg::new(c, class)));
                }
                v.extend(
                    inst.clobbers()
                        .iter()
                        .map(|&(c, class)| PReg::new(c, class)),
                );
                v
            };
            for &clobber_reg in &clobbers {
                if let Some(victim) = self.phys_owner(clobber_reg) {
                    // 占用者若是本指令的 use（值需在寄存器被读取），不能直接
                    // spill——spill_vreg 只标记槽并释放寄存器，不 store 当前
                    // 寄存器值，随后 reload_from_stack 会从空槽读到垃圾
                    // （shift 的 RCX clobber 场景：v 在 RCX 且是 MOV_R_RM
                    // {out},{0} 的 use，被 spill 后 reload 读到垃圾 → 崩溃）。
                    // 跳过：use 在寄存器中读旧值（clobber 写坏发生在指令
                    // 写入阶段，use 在读旧值阶段），之后该 XReg 若再活跃，
                    // 后续指令的 phys_conflicts 会避开已 clobber 的寄存器。
                    if inst_xregs.contains(&victim) {
                        continue;
                    }
                    self.spill_vreg(victim)?;
                }
            }
            self.current_clobbers = clobbers;

            // 2. 确保所有 XReg 在寄存器中——先 use 后 def（use-before-def）。
            // 同一条指令内 def 与 use 若分配同一寄存器，必须保证"先读旧值再写新值"；
            // use 字段先占用寄存器，def 字段后分配（可能换寄存器），避免 def 抢走
            // use 的寄存器导致 load/lea 链中 base 地址被 def 覆盖。
            let mut spilled_this_inst = 0usize;
            let inst_uses = &inst_xregs;

            // use 字段（读旧值）：先确保分配
            for &vreg in inst_xregs.iter().filter(|v| !inst_defs.contains(v)) {
                if self.is_spilled(vreg) {
                    spilled_this_inst += 1;
                    // Reload from stack
                    self.reload_from_stack(vreg, OperandConstraint::Any)?;
                } else if !self.assignments.contains_key(&vreg) {
                    if crate::pipeline::trace_enabled("FORGE_TRACE_ALLOC") {
                        let was_def = self
                            .intervals
                            .get(&vreg)
                            .map(|iv| !iv.defs.is_empty())
                            .unwrap_or(false);
                        if was_def {
                            eprintln!(
                                "[USE-LOST] v{} at {:?} had def but no reg {:?}",
                                vreg.index(),
                                use_point,
                                inst
                            );
                        }
                    }
                    // 首次遇到此 XReg（use：读旧值）
                    self.assign_reg(
                        vreg,
                        OperandConstraint::Any,
                        self.vreg_class(vreg),
                        inst_uses,
                        false,
                    )?;
                }
            }
            // def 字段（写新值）：后分配
            for &vreg in &inst_defs {
                if self.is_spilled(vreg) {
                    spilled_this_inst += 1;
                    self.reload_from_stack(vreg, OperandConstraint::Any)?;
                } else if !self.assignments.contains_key(&vreg) {
                    self.assign_reg(
                        vreg,
                        OperandConstraint::Any,
                        self.vreg_class(vreg),
                        inst_uses,
                        true,
                    )?;
                }
            }

            // 1.9 fail-closed（WORKAROUNDS WA-40）：spilled def 的正确性依赖
            // emission 用 `set_reg_field` 把该 def 的字段改写成 scratch 后 store
            // 回槽。字段若是**固定物理字段**（`is_reg_field_settable == false`，
            // 如模板里写死的 RAX——DSL 对这类字段不生成 set_reg_field 分支，
            // 命中通配 `_ => {}` 静默 no-op），指令会写物理寄存器而 store-back
            // 从 scratch 读 → spill 槽静默写入陈旧值（读回垃圾 → 偶发 AV/挂起）。
            // 宁可编译期报错，不产出静默错码。
            for &vreg in &inst_defs {
                if !self.is_spilled(vreg) || self.assignments.contains_key(&vreg) {
                    continue;
                }
                let field = slot
                    .iter()
                    .find(|&&(x, _fi, d)| x == vreg && d)
                    .map(|&(_, fi, _d)| fi);
                if let Some(fi) = field
                    && !inst.is_reg_field_settable(fi as usize)
                {
                    return Err(IrError::RegAlloc(format!(
                        "spilled def v{} at {:?}: field {} of {:?} is a fixed physical \
                         register (not settable) — emission cannot redirect it to a \
                         scratch register, the spill slot would silently hold stale \
                         data (WORKAROUNDS WA-40)",
                        vreg.index(),
                        use_point,
                        fi,
                        inst
                    )));
                }
            }

            self.max_concurrent_spills = self.max_concurrent_spills.max(spilled_this_inst);

            // 4. 标记活跃（HashMap 插入 O(1)，替代 Vec retain+push）
            for &vreg in &inst_xregs {
                if let Some(&preg) = self.assignments.get(&vreg) {
                    self.active.insert(vreg, preg);
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
        is_def: bool,
    ) -> Result<PReg, IrError> {
        // 如果已分配，直接返回
        if let Some(&preg) = self.assignments.get(&vreg) {
            return Ok(preg);
        }

        match constraint {
            OperandConstraint::Any => {
                // 从空闲池获取
                if let Some(preg) = self.pop_free(class) {
                    if crate::pipeline::trace_enabled("FORGE_TRACE_ALLOC") {
                        eprintln!(
                            "[alloc] v{} -> {} at {:?} (free)",
                            vreg.index(),
                            preg,
                            self.current_point
                        );
                    }
                    self.assignments.insert(vreg, preg);
                    self.set_reg_owner(preg, vreg);
                    return Ok(preg);
                }
                // 无空闲 → 驱逐
                if crate::pipeline::trace_enabled("FORGE_TRACE_ALLOC") {
                    eprintln!(
                        "[alloc] v{} evict at {:?}",
                        vreg.index(),
                        self.current_point
                    );
                }
                self.evict_and_assign(vreg, class, is_def)
            }

            OperandConstraint::Fixed(required_preg) => {
                // 如果目标寄存器被占用（含跨宽度类重叠，如 GPR(4) 与 GPR(8) 同编号），
                // 驱逐占用者
                if let Some(occupant) = self.phys_owner(required_preg)
                    && occupant != vreg
                {
                    self.spill_vreg(occupant)?;
                }
                self.remove_from_free(required_preg);
                self.assignments.insert(vreg, required_preg);
                self.set_reg_owner(required_preg, vreg);
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
                    self.set_reg_owner(input_preg, vreg);
                    return Ok(input_preg);
                }
                // Fallback: input 尚未分配（如 constant），正常分配
                if let Some(preg) = self.pop_free(class) {
                    self.assignments.insert(vreg, preg);
                    self.set_reg_owner(preg, vreg);
                    return Ok(preg);
                }
                self.evict_and_assign(vreg, class, is_def)
            }

            OperandConstraint::Stack => {
                // Stack 约束在此处不应该出现——process_block 已提前处理。
                // 如果到达此处，说明调用方未处理 Stack，作为防御性措施 spill XReg。
                self.spill_vreg(vreg)?;
                Err(IrError::RegAlloc(format!(
                    "unexpected Stack constraint in assign_reg for vreg {}",
                    vreg
                )))
            }

            OperandConstraint::Clobber(_preg) => {
                // Clobber 约束在 process_block 中处理（spill 占用者），
                // 不应该在 assign_reg 中遇到。
                Err(IrError::RegAlloc(format!(
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
    fn evict_and_assign(
        &mut self,
        vreg: XReg,
        class: RegClass,
        is_def: bool,
    ) -> Result<PReg, IrError> {
        let current_point = self.current_point;
        // 驱逐候选的 next_use 预计算（2026-08-31）：active 是 HashMap，iter
        // 顺序跨进程随机；max_by_key 在平局时返回先遇到的元素 → 偶发不同的
        // 驱逐决策。预收集 (next_use, vreg) 再用 max_by_key + 确定性
        // tie-breaker（next_use_after 相同按 vreg index 递增），保证跨进程
        // 确定性；且每个候选只查一次 next_use（max_by 的惰性比较是 O(active²)
        // 次查询，预计算降为 O(active) 次）。
        //
        // P0-8 修复（spill store 语义）：`spill_vreg` 只分配槽并释放寄存器，
        // **不把寄存器当前值 store 进槽**（emission 的 load/store 只围绕
        // 指令自身操作数）。因此只能驱逐 **next_use == MAX（无真实未来
        // 使用）的死值**——驱逐带活值的 victim 会在其下次 use 时从空槽读
        // 垃圾（长命值被驱逐后直接 use 的组合，此前靠 callee-saved 优先
        // 取号规避而非根治）。若所有候选都有真实 next_use，则 spill 当前
        // vreg 自己（emission 会正确 store 当前指令的 spilled def）。
        let mut dead_victims: Vec<(ProgPoint, XReg, PReg)> = self
            .active
            .iter()
            .filter(|(_, p)| p.class == class || p.class.overlaps(class))
            .filter(|(victim, _)| **victim != vreg)
            .map(|(&v, &p)| {
                let next_use = self
                    .intervals
                    .get(&v)
                    .and_then(|iv| iv.next_use_after(current_point))
                    .unwrap_or(ProgPoint::MAX);
                (next_use, v, p)
            })
            .collect();
        // 只保留死值（无未来使用）；按 (next_use, vreg index) 确定性取最大。
        dead_victims.retain(|&(next_use, _, _)| next_use == ProgPoint::MAX);
        dead_victims.sort_by_key(|&(nu, v, _)| (nu, v.index()));
        let victim = dead_victims.pop().map(|(_, v, p)| (ProgPoint::MAX, v, p));

        if let Some((_, victim_vreg, victim_preg)) = victim {
            if crate::pipeline::trace_enabled("FORGE_TRACE_ALLOC")
                || crate::pipeline::trace_enabled("FORGE_TRACE_SPILL")
            {
                eprintln!(
                    "[evict] v{} at {:?} evicts v{} from {} (victim next_use={:?}, cur={:?})",
                    vreg.index(),
                    self.current_point,
                    victim_vreg.index(),
                    victim_preg,
                    self.intervals
                        .get(&victim_vreg)
                        .and_then(|iv| iv.next_use_after(self.current_point)),
                    self.current_point,
                );
            }
            // 驱逐 victim 到栈
            self.spill_vreg(victim_vreg)?;
            // victim 的寄存器现在空闲：spill_vreg 会把它 push 回 free_regs，
            // 这里必须从空闲池移除，否则后续 reload/分配会重复拿到已占寄存器
            self.remove_from_free(victim_preg);
            self.assignments.insert(vreg, victim_preg);
            self.set_reg_owner(victim_preg, vreg);
            // 从 active 移除 victim
            self.active.remove(&victim_vreg);
            Ok(victim_preg)
        } else if is_def {
            // 无死值可驱逐，但当前 vreg 是本指令的 **def**（写新值）：
            // spill 自己安全——emission 的 emit_inst_with_spills 对 spilled
            // 字段用 scratch 寄存器写入（set_reg_field 覆盖为 scratch），
            // 指令执行后把 def 结果 store 回槽；后续 use 经 reload_from_stack
            // 从槽 load。因此 spilled def **不需要真实寄存器分配**。
            self.spill_vreg(vreg)?;
            if crate::pipeline::trace_enabled("FORGE_TRACE_ALLOC")
                || crate::pipeline::trace_enabled("FORGE_TRACE_SPILL")
            {
                eprintln!(
                    "[spill-def] v{} at {:?} -> spilled (def, slot={:?})",
                    vreg.index(),
                    self.current_point,
                    self.spill_slots.get(&vreg).map(|s| s.offset)
                );
            }
            // 返回占位 PReg（不 insert assignments——emission 对 spilled
            // 字段用 scratch 覆盖，该值仅满足 Result 签名）。若 free 池有
            // 空闲则正常分配（后续指令少一次 reload）。
            if let Some(preg) = self.pop_free(class) {
                self.assignments.insert(vreg, preg);
                self.set_reg_owner(preg, vreg);
                self.active.insert(vreg, preg);
                Ok(preg)
            } else {
                // 占位 PReg（不 insert assignments——emission 对 spilled
                // 字段用 scratch 覆盖，该值仅满足 Result 签名）。
                // 注：vec_push 崩溃现场（槽 1144 = v312692 def-spill 槽）值 0，
                // 疑似 ret_move/call 的 spilled def store 与 scratch 交互——
                // 见 docs/plans/forge-rustc-vec_push-plan.md E1 深挖。
                Ok(PReg::new(0, class))
            }
        } else {
            // 无法驱逐死值（所有候选都有真实 next_use）——不能 spill 当前
            // vreg（若它是 use，emission 会从空槽 load 读垃圾；若它是 def
            // 可 spill，但分配器无法在此区分）。保守终止编译——不产出
            // 错误代码。P0-8：旧代码驱逐"next_use 最大"的活 victim 不 store
            // → 读垃圾；现在只驱逐死值，无可驱逐时显式报错。
            if crate::pipeline::trace_enabled("FORGE_TRACE_ALLOC")
                || crate::pipeline::trace_enabled("FORGE_TRACE_SPILL")
            {
                eprintln!(
                    "[evict-fail] v{} class={:?} free={:?} active={}",
                    vreg.index(),
                    class,
                    self.free_regs.get(&class).map(|v| v.len()),
                    self.active.len()
                );
            }
            Err(IrError::RegAlloc(format!(
                "vreg {} spilled: no dead (unused) register to evict in class {:?}",
                vreg, class
            )))
        }
    }

    // ── Spill/Restore ──

    fn spill_vreg(&mut self, vreg: XReg) -> Result<(), IrError> {
        if crate::pipeline::trace_enabled("FORGE_TRACE_ALLOC")
            || crate::pipeline::trace_enabled("FORGE_TRACE_SPILL")
        {
            eprintln!(
                "[spill] v{} at {:?} (preg={:?})",
                vreg.index(),
                self.current_point,
                self.assignments.get(&vreg)
            );
        }
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
            self.clear_reg_owner(preg);
            self.free_regs.entry(preg.class).or_default().push(preg);
        }

        self.active.remove(&vreg);
        Ok(())
    }

    fn reload_from_stack(
        &mut self,
        vreg: XReg,
        constraint: OperandConstraint,
    ) -> Result<(), IrError> {
        // 为 spilled XReg 分配一个寄存器（实际 load 在 emit 阶段完成）。
        // 尊重约束：Fixed 约束必须使用指定寄存器。
        let class = self.vreg_class(vreg);
        let trace = crate::pipeline::trace_enabled("FORGE_TRACE_ALLOC")
            || crate::pipeline::trace_enabled("FORGE_TRACE_SPILL");
        match constraint {
            OperandConstraint::Fixed(required_preg) => {
                // 如果目标寄存器被占用（含跨宽度类重叠，如 GPR(4) 与 GPR(8) 同编号），
                // 驱逐占用者
                if let Some(occupant) = self.phys_owner(required_preg)
                    && occupant != vreg
                {
                    self.spill_vreg(occupant)?;
                }
                if trace {
                    eprintln!(
                        "[reload] v{} at {:?} -> fixed {}",
                        vreg.index(),
                        self.current_point,
                        required_preg
                    );
                }
                self.assignments.insert(vreg, required_preg);
                self.set_reg_owner(required_preg, vreg);
            }
            _ => {
                // 任意空闲寄存器即可
                if let Some(preg) = self.pop_free(class) {
                    if trace {
                        eprintln!(
                            "[reload] v{} at {:?} -> {}",
                            vreg.index(),
                            self.current_point,
                            preg
                        );
                    }
                    self.assignments.insert(vreg, preg);
                    self.set_reg_owner(preg, vreg);
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

    // ── reg_owner 访问（按物理寄存器编号索引，O(1) 冲突检测）──

    /// 取物理寄存器编号 num 上的占用者列表（只读）。
    #[inline]
    fn reg_owner_slot(&self, num: u32) -> &[(RegClass, XReg)] {
        self.reg_owner
            .get(num as usize)
            .map(|s| s.as_slice())
            .unwrap_or(&[])
    }

    /// 记录 preg 被 vreg 占用（移除同冲突类的旧占用者——通常不存在）。
    #[inline]
    fn set_reg_owner(&mut self, preg: PReg, vreg: XReg) {
        let idx = preg.num as usize;
        if self.reg_owner.len() <= idx {
            self.reg_owner.resize(idx + 1, smallvec::SmallVec::new());
        }
        let slot = &mut self.reg_owner[idx];
        slot.retain(|(c, _)| !c.overlaps(preg.class));
        slot.push((preg.class, vreg));
    }

    /// 移除 preg 的占用者（含跨宽度类重叠的占用者）。
    #[inline]
    fn clear_reg_owner(&mut self, preg: PReg) {
        if let Some(slot) = self.reg_owner.get_mut(preg.num as usize) {
            slot.retain(|(c, _)| !c.overlaps(preg.class));
        }
    }

    /// 物理寄存器冲突检测：同编号且类重叠（如 GPR(4) 与 GPR(8) 的 0 都是
    /// RAX/EAX 同一物理寄存器；FPR 与 VEC 同编号共享 XMM）。
    /// reg_owner 按 (num, class) 精确键存储，多宽度类（GPR(4)/GPR(8)）的
    /// 相同编号必须视为同一物理寄存器，否则两个 XReg 会分到同一物理寄存器。
    #[inline]
    fn phys_conflicts(&self, preg: PReg) -> bool {
        if self
            .current_clobbers
            .iter()
            .any(|c| c.num == preg.num && c.class.overlaps(preg.class))
        {
            return true;
        }
        self.reg_owner_slot(preg.num)
            .iter()
            .any(|(c, _)| c.overlaps(preg.class))
    }

    /// 物理寄存器占用者查询（跨类重叠）。
    #[inline]
    fn phys_owner(&self, preg: PReg) -> Option<XReg> {
        self.reg_owner_slot(preg.num)
            .iter()
            .find(|(c, _)| c.overlaps(preg.class))
            .map(|&(_, v)| v)
    }

    fn expire_dead(&mut self, current_point: ProgPoint) {
        // 释放 end_point 在当前点之前或恰好在当前点的 XReg。
        // 注意：使用 end_point() 而非 contains() 因为当前 live interval 模型
        // 只覆盖具体使用/定义点和块边界点，而非整个块连续区间。
        // contains() 会在跨块 XReg 的块中间误判为"不活跃"。
        // 对于真正的跨块 holes，数据流扩展（Stage 2）将区间扩展到 block_end，
        // XReg 在整个块范围内保持活跃。
        let mut freed: Vec<PReg> = Vec::new();
        self.active.retain(|vreg, preg| {
            if let Some(interval) = self.intervals.get(vreg)
                && interval.end_point() <= current_point
            {
                if crate::pipeline::trace_enabled("FORGE_TRACE_EXPIRE") {
                    eprintln!(
                        "[expire] v{} end={:?} cur={:?} preg={:?}",
                        vreg.index(),
                        interval.end_point(),
                        current_point,
                        preg
                    );
                }
                freed.push(*preg);
                false
            } else {
                true
            }
        });
        for preg in freed {
            self.clear_reg_owner(preg);
            self.free_regs.entry(preg.class).or_default().push(preg);
        }
    }

    fn pop_free(&mut self, class: RegClass) -> Option<PReg> {
        loop {
            // 先取号（结束 free_regs 的可变借用），再检查物理占用
            let preg = {
                let pool = self.free_regs.get_mut(&class)?;
                // 2026-08 确定性修复：入池路径（expire_dead 的 active.retain
                // HashMap 随机 iter → freed 批量 push；spill/evict 的 push）使
                // free_regs 池序跨进程随机 → pop() 取到的寄存器随机 → 分配/
                // spill 决策连锁非确定（EP/函数布局每次编译不同）。取号前按
                // PReg index 确定性选择：callee-saved 优先（见下），否则取
                // 最大编号——跨进程一致。
                //
                // **callee-saved 优先**：跨调用存活的 vreg 必须落在被保存的
                // 寄存器上（否则 call 点被 clobber 强制 spill，参数/长命值丢
                // 失——riscv fib 递归：参数 n 先分到 t 系（高编号 caller-
                // saved）→ spill → @move_args 写的寄存器值与 emission 从槽
                // 重载不一致 → 递归结果错）。x86 的 callee-saved 恰在高编号
                // （R15-R12），纯 max 优先天然命中；riscv 的 callee-saved
                //（X9/X18-X27）与 caller-saved 交错（X28-X31 更高）→ 需显式
                // 优先 callee_saved。非跨调用 vreg 用 callee-saved 无害（函数
                // 自身保存）。
                //
                // 有序池优化（2026-08-31）：不再每次 sort_by_key（O(n log n)，
                // 每指令分配都触发）——callee-saved 与最大编号均用单次线性
                // 扫描（O(n)，n = 池大小 ≤ 寄存器数 16，常数级），确定性保持。
                if let Some(max_cs) = pool
                    .iter()
                    .filter(|p| self.config.callee_saved.contains(&p.num))
                    .max_by_key(|p| p.num)
                    .copied()
                {
                    pool.retain(|p| *p != max_cs);
                    Some(max_cs)
                } else if let Some((max_idx, _)) =
                    pool.iter().enumerate().max_by_key(|(_, p)| p.num)
                {
                    Some(pool.swap_remove(max_idx))
                } else {
                    None
                }
            }?;
            // 跳过已被占用的寄存器（spill/驱逐可能把已占寄存器残留进空闲池）
            // 跨宽度类重叠（GPR(4) vs GPR(8) 同编号）也视为占用。
            //
            // 泄漏分析（2026-08-06）：入池路径（spill_vreg/expire_dead）均先
            // clear_reg_owner 再 push，池中不应存在被占用寄存器；此处丢弃仅
            // 防御历史/边角污染，且被丢弃寄存器在占用者释放时（clear + push）
            // 会自然回池，不构成永久丢失。
            if !self.phys_conflicts(preg) {
                return Some(preg);
            }
        }
    }

    fn remove_from_free(&mut self, preg: PReg) {
        if let Some(pool) = self.free_regs.get_mut(&preg.class) {
            pool.retain(|p| *p != preg);
        }
    }

    // ── 构建结果 ──

    fn build_result(self) -> Result<AllocResult, IrError> {
        let spill_area_size = if self.next_spill_offset > 0 {
            // 对齐到 16 字节
            (self.next_spill_offset as u32).div_ceil(16) * 16
        } else {
            0
        };

        // 收集 callee-saved 寄存器：**只列实际分配到的**（assignments 中出现
        // 的 callee-saved，按声明序）——否则每函数全量保存 11 个 s 系，
        // 小函数帧也 ≥104B（阶段 G 按需保存；未用的 s 系无需 push/pop）。
        // **class 通配**：i32 值以 GPR(4) 类分配（num 仍 27=s11），过滤只看
        // num——否则 GPR(4) 的 s11 漏保存 → 递归覆盖（fib 实测 got -15）。
        let callee_saved_pregs: Vec<PReg> = self
            .config
            .classes
            .get(&self.config.main_gpr_class)
            .map(|cfg| {
                cfg.allocatable
                    .iter()
                    .filter(|r| self.config.callee_saved.contains(r))
                    .filter(|r| self.assignments.values().any(|p| p.num == **r))
                    .map(|&r| PReg::new(r, self.config.main_gpr_class))
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
            param_by_ref: self
                .config
                .param_xregs
                .iter()
                .map(|v| v.class().is_fp() && v.width() > 16)
                .collect(),
            // S2：sret 由 CompileState 在分配后填充（LowerCtx.is_sret_return）
            sret: false,
            stack_arg_bytes: 0,
            param_is_32: self
                .config
                .param_xregs
                .iter()
                .map(|v| v.width() == 4)
                .collect(),
            // 参数字节宽由 CompileState 分配后按 xreg_types 填充（regalloc
            // 不持有 IR 类型；见 AllocResult::param_bytes 文档）。
            param_bytes: Vec::new(),
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
            xa.alloc_default(RegClass::GPR64);
        }
        xa.alloc_default(RegClass::GPR64)
    }

    fn make_config(num_gp: u32, num_fp: u32) -> RegAllocConfig {
        RegAllocConfig::new(num_gp, num_fp, 4, Some(5))
    }

    #[test]
    fn test_empty_function() {
        let config = make_config(8, 8);
        let ctx = AllocContext::default();
        let alloc = BacktrackingAllocator::new();
        let vcode = crate::VCode::<DummyInst>::new();
        let result = alloc.allocate(&vcode, &config, &ctx, &[], &[]);
        let _ = result;
    }

    #[test]
    fn test_basic_allocation() {
        let config = make_config(8, 8);
        let ctx = AllocContext::default();
        let alloc = BacktrackingAllocator::new();
        let vcode = crate::VCode::<DummyInst>::new();
        let result = alloc.allocate(&vcode, &config, &ctx, &[], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_fixed_reg_constraint() {
        // 验证 BT 分配器创建
        let alloc = BacktrackingAllocator::new();
        let _ = alloc;
    }

    #[test]
    fn test_phys_conflicts_cross_width_classes() {
        // 跨宽度类物理重叠：GPR(4) 与 GPR(8) 的相同编号是同一物理寄存器
        //（RAX=EAX），分配器必须视为冲突；FPR 的 0（XMM0）与 GPR 的 0（RAX）
        // 是不同物理寄存器，不冲突。
        let config = make_config(8, 8);
        let mut state = BtState::new(&config, HashMap::new());
        let gpr4 = PReg::new(0, RegClass::GPR(4));
        let gpr8 = PReg::new(0, RegClass::GPR(8));
        let fpr8 = PReg::new(0, RegClass::FPR(8));
        let v4 = XReg::new(512, RegClass::GPR(4));
        let v8 = XReg::new(513, RegClass::GPR(8));

        assert!(!state.phys_conflicts(gpr4));
        state.set_reg_owner(gpr4, v4);
        // 同编号跨宽度类（GPR(4) vs GPR(8)）冲突
        assert!(state.phys_conflicts(gpr8), "GPR(4) 与 GPR(8) 同编号应冲突");
        assert_eq!(state.phys_owner(gpr8), Some(v4));
        // 同编号同 class 冲突
        assert!(state.phys_conflicts(gpr4));
        // FPR(8) 的 0（XMM0）与 GPR 的 0（RAX）不冲突
        assert!(!state.phys_conflicts(fpr8), "FPR 与 GPR 同编号不应冲突");
        // 反向：GPR(8) 占用后 GPR(4) 同编号也冲突
        state.set_reg_owner(gpr8, v8);
        assert!(state.phys_conflicts(gpr4));
    }

    #[test]
    fn test_reg_class_width_from_config() {
        // 验证 DSL 生成的 reg_class_width 通过配置正确传播
        let config = make_config(16, 16);
        let gpr_cfg = config.classes.get(&RegClass::GPR64).unwrap();
        assert_eq!(gpr_cfg.reg_width, 8);
        let fpr_cfg = config.classes.get(&RegClass::FPR64).unwrap();
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
        let result = alloc.allocate(&vcode, &config, &ctx, &[], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_spill_slot_width_aware() {
        // 验证 BT 分配器在仅有 1 个可分配寄存器时仍能完成分配
        let mut config = make_config(8, 8);
        config
            .classes
            .get_mut(&RegClass::GPR64)
            .unwrap()
            .allocatable = vec![3]; // RBX only
        let ctx = AllocContext::default();
        let alloc = BacktrackingAllocator::new();
        let vcode = crate::VCode::<DummyInst>::new();
        let result = alloc.allocate(&vcode, &config, &ctx, &[], &[]);
        assert!(
            result.is_ok(),
            "Allocator should handle high register pressure"
        );
    }
}

#[cfg(test)]
mod clobber_map_tests {
    use super::*;
    use crate::machine::inst::DummyInst;

    fn xgpr(n: u32) -> XReg {
        let mut xa = XRegAllocator::new();
        for _ in 0..n {
            xa.alloc_default(RegClass::GPR64);
        }
        xa.alloc_default(RegClass::GPR64)
    }

    fn make_config(num_gp: u32, num_fp: u32) -> RegAllocConfig {
        RegAllocConfig::new(num_gp, num_fp, 4, Some(5))
    }

    fn clobber_vcode() -> crate::VCode<DummyInst> {
        let mut vcode = crate::VCode::<DummyInst>::new();
        let bid = vcode.create_block(Block(0));
        vcode.switch_to_block(bid);
        for i in 0..3 {
            vcode.push_inst(DummyInst { id: i });
        }
        vcode
    }

    /// clobber 点占用者若同时是本指令 use：值保留在寄存器（不 spill）——
    /// spill_vreg 不 store 当前寄存器值，reload 会读空槽垃圾。
    #[test]
    fn test_clobber_use_owner_not_spilled() {
        let mut config = make_config(16, 16);
        config
            .classes
            .get_mut(&RegClass::GPR64)
            .unwrap()
            .allocatable = vec![0, 1]; // RAX + RCX
        let x0 = xgpr(0);
        let vcode = clobber_vcode();
        let xreg_map = vec![
            smallvec::smallvec![(x0, 0u8, true)],  // inst0: def
            smallvec::smallvec![(x0, 0u8, false)], // inst1: use（clobber 点）
            smallvec::smallvec![(x0, 0u8, false)], // inst2: use 结束
        ];
        let clobber_map = vec![vec![], vec![(0u32, RegClass::GPR64)], vec![]]; // inst1 写死 RAX
        let ctx = AllocContext::default();
        let alloc = BacktrackingAllocator::new();
        let result = alloc
            .allocate(&vcode, &config, &ctx, &xreg_map, &clobber_map)
            .expect("alloc should succeed");
        // x0 在 clobber 点被使用（值需在寄存器）→ 不应被 spill
        assert!(
            !result.spill_slots.contains_key(&x0),
            "clobber 点 use 的占用者不应 spill（值保留在寄存器）"
        );
    }

    /// phys_conflicts 避开路径：唯一可分配寄存器在 clobber 点被写死 → 新分配失败。
    #[test]
    fn test_clobber_blocks_fresh_alloc() {
        let mut config = make_config(16, 16);
        config
            .classes
            .get_mut(&RegClass::GPR64)
            .unwrap()
            .allocatable = vec![0];
        let x0 = xgpr(0);
        let vcode = clobber_vcode();
        let xreg_map = vec![smallvec::smallvec![(x0, 0u8, true)]]; // inst0: def
        let clobber_map = vec![vec![(0u32, RegClass::GPR64)]]; // inst0 写死 RAX → def 无法分配
        let ctx = AllocContext::default();
        let alloc = BacktrackingAllocator::new();
        let result = alloc.allocate(&vcode, &config, &ctx, &xreg_map, &clobber_map);
        // 2026-09 语义变更：def（写新值）在唯一寄存器被 clobber 时**spill 到栈**
        // 而非报错——emission 的 emit_inst_with_spills 用 scratch 写入 def、
        // 随后 store 回槽，后续 use 经 reload_from_stack 从槽 load，值正确。
        // 这只对 def 安全（写新值，不读旧寄存器）；use 场景仍保守报错。
        assert!(
            result.is_ok(),
            "def 在 clobber 时 spill 到栈（emission scratch 写入）"
        );
        assert!(
            result.unwrap().spill_slots.contains_key(&x0),
            "def 应被 spill 到栈槽"
        );
    }

    /// 对照：无 clobber 时 1 个 XReg 配唯一寄存器 RAX 成功且不 spill。
    #[test]
    fn test_no_clobber_allocates_rax() {
        let mut config = make_config(16, 16);
        config
            .classes
            .get_mut(&RegClass::GPR64)
            .unwrap()
            .allocatable = vec![0];
        let x0 = xgpr(0);
        let vcode = clobber_vcode();
        let xreg_map = vec![
            smallvec::smallvec![(x0, 0u8, true)],
            smallvec::smallvec![(x0, 0u8, false)],
            smallvec::smallvec![(x0, 0u8, false)],
        ];
        let clobber_map = vec![vec![], vec![], vec![]];
        let ctx = AllocContext::default();
        let alloc = BacktrackingAllocator::new();
        let result = alloc
            .allocate(&vcode, &config, &ctx, &xreg_map, &clobber_map)
            .expect("no clobber: RAX 可用，分配成功");
        assert!(
            !result.spill_slots.contains_key(&x0),
            "无 clobber 时 x0 应留在 RAX 不 spill"
        );
    }

    /// W1 正向守卫（WORKAROUNDS WA-40）：def-spill 在**可被 set_reg_field 改写**
    /// 的字段上合法——emission 会把该字段重设 scratch 后 store 回槽，值仍正确。
    /// 与 `test_clobber_blocks_fresh_alloc` 同形，但显式声明"可改写性"前提。
    #[test]
    fn test_def_spill_on_settable_field_ok() {
        let mut config = make_config(16, 16);
        config
            .classes
            .get_mut(&RegClass::GPR64)
            .unwrap()
            .allocatable = vec![0]; // 仅 RAX，且被 clobber → def 只能 spill
        let x0 = xgpr(0);
        let vcode = clobber_vcode();
        let xreg_map = vec![smallvec::smallvec![(x0, 0u8, true)]];
        let clobber_map = vec![vec![(0u32, RegClass::GPR64)]];
        let ctx = AllocContext::default();
        let alloc = BacktrackingAllocator::new();
        let result = alloc
            .allocate(&vcode, &config, &ctx, &xreg_map, &clobber_map)
            .expect("def-spill 在可改写字段上必须成功（emission 用 scratch 写入 + store 回槽）");
        assert!(
            result.spill_slots.contains_key(&x0),
            "该构造下 def 应被 spill（守卫不得误报）"
        );
        // DummyInst 未覆写 → 默认字段可改写（旧契约）。
        assert!(
            DummyInst { id: 0 }.is_reg_field_settable(0),
            "默认 MachineInst 实现必须保持字段可改写（向后兼容）"
        );
    }

    /// W1 fail-closed（WORKAROUNDS WA-40）：def-spill 落在**固定物理字段**
    /// （`is_reg_field_settable == false`，DSL 对这类字段不生成 set_reg_field
    /// 分支 → 静默 no-op）时必须编译期报错——否则指令写物理寄存器、store-back
    /// 从 scratch 读，spill 槽静默留下陈旧值（后续 reload 读垃圾 → 偶发 AV/挂起）。
    #[test]
    fn test_def_spill_on_fixed_field_errors() {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        struct FixedFieldInst {
            #[allow(dead_code)]
            id: u32,
        }
        impl MachineInst for FixedFieldInst {
            fn uses(&self) -> smallvec::SmallVec<[u32; 4]> {
                smallvec::SmallVec::new()
            }
            fn defs(&self) -> smallvec::SmallVec<[u32; 2]> {
                smallvec::SmallVec::new()
            }
            fn is_branch(&self) -> bool {
                false
            }
            fn branch_targets(&self) -> smallvec::SmallVec<[Block; 2]> {
                smallvec::SmallVec::new()
            }
            fn is_call(&self) -> bool {
                false
            }
            fn is_ret(&self) -> bool {
                false
            }
            /// 模拟"模板里写死的物理字段"：无字段可被改写。
            fn is_reg_field_settable(&self, _i: usize) -> bool {
                false
            }
        }

        let mut config = make_config(16, 16);
        config
            .classes
            .get_mut(&RegClass::GPR64)
            .unwrap()
            .allocatable = vec![0];
        let x0 = xgpr(0);
        let mut vcode = crate::VCode::<FixedFieldInst>::new();
        let bid = vcode.create_block(Block(0));
        vcode.switch_to_block(bid);
        vcode.push_inst(FixedFieldInst { id: 0 });
        let xreg_map = vec![smallvec::smallvec![(x0, 0u8, true)]];
        let clobber_map = vec![vec![(0u32, RegClass::GPR64)]];
        let ctx = AllocContext::default();
        let alloc = BacktrackingAllocator::new();
        let err = alloc
            .allocate(&vcode, &config, &ctx, &xreg_map, &clobber_map)
            .expect_err("固定物理字段上的 def-spill 必须 fail-closed 报错");
        let msg = format!("{err}");
        assert!(
            msg.contains("WA-40") && msg.contains("fixed physical"),
            "错误信息应说明固定物理字段并指向 WA-40：{msg}"
        );
    }
}
