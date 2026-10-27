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
//! ## Spill 启发式（**与实现一致**，v23 M5 校正）
//!
//! 无空闲寄存器时按 `next_use_after` 做 Belady 选择，但**只驱逐"死值"**
//! （`next_use == ProgPoint::MAX`，即没有未来使用）：
//!
//! - 有死值 ⇒ 取 `(next_use, vreg index)` 最大的那个驱逐（确定性 tie-breaker）；
//! - 无死值且当前 vreg 是本指令的 **def** ⇒ spill 当前 vreg（emission 会把 def 结果
//!   store 回槽，安全）；
//! - 无死值且当前 vreg 是 **use** ⇒ **明确报错**（`IrError::RegAlloc`），不猜。
//!
//! **为什么不驱逐活跃值**：`spill_vreg` 只分配槽并释放寄存器，**不把寄存器当前值写进槽**
//! （emission 的 load/store 只围绕当前指令的操作数）。驱逐带活值的 victim 会让它下次
//! use 时从空槽读垃圾。要真正做到"取最远的下一次使用"（完整 Belady）并让 spill cost
//! （`LiveInterval::weight`——v23 M5 已删，见 `liverange.rs` 的说明）参与决策，
//! 必须先有 **`SpillEdit`**：分配器产出"把活跃值 store 回槽"的显式编辑序列，
//! 由 emission 按序发射（M5 的下一批）。在那之前，"Belady + 循环加权 spill cost"
//! 只是文档里的说法——本段即为校正后的实情。

use self::spill::build_spill_edits;
use crate::IrError;
use crate::machine::inst::{MachineInst, OperandConstraint};
use crate::pipeline::alloc_config::RegAllocConfig;
use crate::pipeline::alloc_result::AllocResult;
use crate::pipeline::liverange;

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
        inst_ops: &[smallvec::SmallVec<[crate::machine::lowering::OperandBind; 2]>],
        clobber_map: &[Vec<(u32, crate::prelude::RegClass)>],
    ) -> Result<AllocResult, IrError> {
        self.allocate_named("", vcode, config, inst_ops, clobber_map)
    }

    /// 同 [`Self::allocate`]，但带**函数名**——只服务诊断
    /// （`FORGE_DUMP_ALLOC` 的分段标题与过滤）。名字不参与任何分配决策。
    ///
    /// 为什么单独一个入口而不是加参数：分配器是**编译器共享字段**（`&self` 跨函数复用、
    /// 并行编译时还被多线程同时借用），把名字塞进 `self` 会造成数据竞争；而给
    /// `allocate` 加必填参数会逼所有测试调用点跟着改。
    pub fn allocate_named<I: MachineInst>(
        &self,
        func_name: &str,
        vcode: &crate::VCode<I>,
        config: &RegAllocConfig,
        inst_ops: &[smallvec::SmallVec<[crate::machine::lowering::OperandBind; 2]>],
        clobber_map: &[Vec<(u32, crate::prelude::RegClass)>],
    ) -> Result<AllocResult, IrError> {
        // 1. 计算 LiveInterval（XReg → 微指令字段映射驱动）
        let intervals =
            liverange::compute_live_intervals(vcode, inst_ops, &config.param_xregs, func_name);

        // 2. 构建状态
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
                state.peak_active = state.peak_active.max(state.active.len());
                if crate::pipeline::trace::knob("FORGE_TRACE_REGALLOC") {
                    eprintln!("[pre] v{} class={class:?} -> {preg:?}", vreg.index());
                }
            }
        }

        // 4. 逐块处理（按 inst_ops 分配 XReg）
        let mut global_inst = 0usize;
        for (block_idx, block) in vcode.blocks().enumerate() {
            let block_u32 = block_idx as u32;
            state.process_block(block_u32, block, inst_ops, clobber_map, &mut global_inst)?;
        }

        // 5. 构建 AllocResult，并补上**逐指令溢出编辑**（决策在分配器里做）
        //
        // 自检（常开；成本 O(区间数²) 仅在 debug 下做，release 只做快速版本）：
        // **同一物理寄存器上不得出现重叠的活区间**。分配器一旦把两个同时活着的值分到
        // 同一寄存器，机器码就会出现"store 的地址寄存器 == 上一条 load 的目标"这类形态
        // （v27 M6 实测：LICM 外提 `StackAddr` 触发），结果只有错值/AV 两种。
        // 有 spill 槽的 vreg 不参与（它没有物理寄存器）。
        // debug 构建常开；release 只在显式开关下做（O(段数²) 的自检不该进生产热路径）。
        if cfg!(debug_assertions) || std::env::var_os("FORGE_VERIFY_REGALLOC").is_some() {
            verify::verify_no_overlap(&state.assignments, &state.spill_slots, &state.intervals)?;
        }
        // 取证视图（`FORGE_DUMP_ALLOC=1`，可用 `FORGE_DUMP_ALLOC_FILTER=<名子串>` 过滤）。
        if std::env::var_os("FORGE_DUMP_ALLOC").is_some() {
            dump::dump_alloc(func_name, vcode, inst_ops, &state);
        }
        let mut result = state.build_result()?;
        result.edits = build_spill_edits(
            vcode,
            inst_ops,
            &result.assignments,
            &result.spill_slots,
            config,
        )?;
        Ok(result)
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

mod dump;
mod spill;
mod state;
mod verify;

use state::BtState;

#[cfg(test)]
mod clobber_map_tests;
#[cfg(test)]
mod tests;
