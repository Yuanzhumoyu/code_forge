//! CompileSession v20 — staged compilation pipeline.
//!
//! Drives IR → machine code compilation through the TargetMachine component traits.
//! Each stage is a self-contained method; the pipeline is explicit and debuggable.
//!
//! # Pipeline Stages
//!
//! 1. Block Mapping          — IR Block → VBlockId
//! 2. VReg Pre-allocation    — params, phi, block params → VReg
//! 3. Pattern Matching       — detect IR patterns for optimized lowering
//! 4. Instruction Selection  — TargetLowering::lower_inst / lower_terminator
//! 6. Register Allocation    — pluggable allocator
//! 7. Frame Layout           — compute spill slots, alignment, frame size
//! 8. Prologue               — TargetFrameLowering::emit_prologue
//! 9. Instruction Emission   — TargetEncoder + spill/label/fixup handling
//! 10. Epilogue              — TargetFrameLowering::emit_epilogue
//! 11. Fixup Resolution      — CodeSink::finish()

use std::sync::Arc;

use crate::machine::target::TargetMachine;
use crate::pipeline::error::CompileError;
use crate::pipeline::options::CompileOptions;
use crate::pipeline::stage::Stage;

/// Map an `AtomicRmwOp` discriminant (as encoded by `builder.atomic_rmw` into
/// `Immediate::Uint`) back to the enum. Defaults to `Add` on unknown values.
pub(crate) fn atomic_op_from_u64(v: u64) -> AtomicRmwOp {
    use AtomicRmwOp::*;
    match v {
        0 => Xchg,
        1 => Add,
        2 => Sub,
        3 => And,
        4 => Nand,
        5 => Or,
        6 => Xor,
        7 => Max,
        8 => Min,
        9 => Umax,
        10 => Umin,
        11 => Fadd,
        _ => Fsub,
    }
}
use crate::pipeline::events;
use crate::pipeline::regalloc_bt::BacktrackingAllocator;
use crate::{CompiledFunction, IrError};
use forge_ir::*;

/// **编译会话**——IR → 机器码的唯一编译实现（单函数与整模块两条入口都走它）。
///
/// 单函数：`new`/`for_module` → `compile`（或 `compile_with_alloc`）。
/// 整模块：`for_module` → `compile_module`（见 `pipeline/session.rs` 的 `CompiledModule`）。
pub struct CompileSession<M: TargetMachine> {
    /// 目标机器（`Arc` 持有：整模块编排按函数克隆一份引用，不再要求 `M: Clone`）。
    machine: Arc<M>,
    reg_alloc: BacktrackingAllocator,
    /// 编译选项（v30 M1）：优化级别 + 并行策略。构造后不可变。
    options: CompileOptions,
    /// **模块级签名表**（变参 D6）：`FuncRef::index()` → 签名事实。
    ///
    /// 编译**整个模块**的入口（或装过表的单函数入口）填它（[`Self::with_module_sigs`]）；
    /// 没装 ⇒ 调用点按非变参处理。
    module_sigs: Option<Arc<forge_ir::ModuleSigs>>,
}

impl<M: TargetMachine> CompileSession<M> {
    /// Create a new compiler with the default register allocator (backtracking).
    pub fn new(machine: M) -> Self {
        Self::from_arc(Arc::new(machine))
    }

    /// 从已共享的机器构造（JIT 装载器持 `Arc<M>`，避免每个函数冷克隆整台机器）。
    pub fn from_arc(machine: Arc<M>) -> Self {
        // 宿主级**一次**注册（幂等）：管线工厂 + **调用点布局钩子**（v20 A5-3）。
        // 生成物的 lowering 在调用点按 `(ISA, 约定, 实参形状)` 查后者拿被调方落点；
        // 没注册就 fail-closed——所以放在每个编译入口（OnceLock，热路径只是一次原子读）。
        crate::pipeline_hooks::ensure_registered();
        Self {
            machine,
            reg_alloc: BacktrackingAllocator,
            options: CompileOptions::new(),
            module_sigs: None,
        }
    }

    /// 宿主机器（整模块编排要用它的克隆；会话自己持有它，不再外借）。
    pub(crate) fn machine(&self) -> &M {
        self.machine.as_ref()
    }

    /// 共享机器句柄（整模块编排按函数分发 worker 用）。
    pub(crate) fn machine_arc(&self) -> &Arc<M> {
        &self.machine
    }

    /// 装上**模块级签名表**（变参 D6）。
    ///
    /// 表由 IR 模块自己给出（[`forge_ir::Module::signature_table`]）——别在宿主侧另抄一份。
    pub fn with_module_sigs(mut self, sigs: forge_ir::ModuleSigs) -> Self {
        self.module_sigs = Some(Arc::new(sigs));
        self
    }

    /// **装上编译选项**（v30 M1：取代 `with_opt_level` 的散装 setter）。
    pub fn with_options(mut self, options: CompileOptions) -> Self {
        self.options = options;
        self
    }

    /// 当前编译选项。
    pub fn options(&self) -> &CompileOptions {
        &self.options
    }

    /// **编译"住在某个模块里的函数"**——从模块自己取签名表装上（变参 D6）。
    ///
    /// 宿主侧的默认写法：只要函数住在某个模块里，就把模块一起传进来，
    /// 别让调用点去猜被调方是不是变参（[`Self::compile_module`] 就是这么做的）。
    pub fn for_module(machine: M, module: &forge_ir::Module) -> Self {
        Self::new(machine).with_module_sigs(module.signature_table())
    }

    /// Compile an IR function (full pipeline，含 IR 级优化若 `options` 指定了级别)。
    pub fn compile(&self, func: &Function) -> Result<CompiledFunction, CompileError> {
        self.compile_with_alloc(func).map(|(cf, _)| cf)
    }

    /// Compile an IR function，额外返回寄存器分配明细（AllocResult：
    /// XReg → PReg assignments、spill_slots 等），供集成测试/工具断言
    /// 分配器行为（ABI 参数分配、clobber 写死保护等）。
    pub fn compile_with_alloc(
        &self,
        func: &Function,
    ) -> Result<(CompiledFunction, crate::AllocResult), CompileError> {
        // **机读出口**（v25 M3）：成功路径的事件在 `compile_stages` 里发
        //（那里才有阶段耗时），这里只负责**失败路径**——缺口 ID 是 CI 最想按类分的字段。
        let r = self.compile_with_alloc_inner(func);
        if let Err(e) = &r {
            let gap = e.gap_id();
            events::emit(
                "CAPABILITY-GAP",
                &[
                    ("fn", events::Field::Str(func.name.as_ref())),
                    ("gap", events::Field::Str(gap.as_deref().unwrap_or(""))),
                ],
            );
        }
        r
    }

    /// 单函数管线入口：给错误补上**阶段 + 函数**（v30 M1）。
    ///
    /// 阶段标签由 `compile_stages` 随执行推进；出错时停在最后进入的阶段。
    fn compile_with_alloc_inner(
        &self,
        func: &Function,
    ) -> Result<(CompiledFunction, crate::AllocResult), CompileError> {
        let mut stage = Stage::Legalize;
        let name = func.name.clone();
        self.compile_stages(func, &mut stage)
            .map_err(|e| CompileError::at(stage, &name, e))
    }

    fn compile_stages(
        &self,
        func: &Function,
        stage: &mut Stage,
    ) -> Result<(CompiledFunction, crate::AllocResult), IrError> {
        // P0-1：IR 级优化管线接入——若启用，克隆函数跑对应 O 级别管线，
        // 用优化后的克隆体继续编译（原 func 不被修改；PassManager 需要
        // &mut Function）。未启用时不克隆（零开销）。
        // 注：**没有**"绕过优化"的旁路入口——要优化就 `with_options(CompileOptions::speed(..))`，
        // 不要就用 `CompileOptions::off()`（或进程级默认）。
        let mut owned: Function;
        let func: &Function = if let Some(level) = self.options.opt_level() {
            owned = func.clone();
            let pm = forge_opt::PassManager::for_level(level);
            pm.run_on_function(&mut owned)?;
            &owned
        } else {
            func
        };

        let mut profile = crate::pipeline::profile::Profile::start();
        let fname = func.name.to_string();
        events::emit("COMPILE-START", &[("fn", events::Field::Str(&fname))]);
        // 前置能力门（v30 M2：值池 / 宽向量 / 异常——只读，fail-closed）。
        crate::pipeline::precheck::run(self.machine.as_ref(), func)?;
        // 合法化批次编排（v30 M2：外提到 `legalize::schedule`；次序与理由见那里）。
        let crate::pipeline::legalize::schedule::Output {
            func: func_owned,
            abi_slots,
        } = crate::pipeline::legalize::schedule::run(
            self.machine.as_ref(),
            func,
            self.module_sigs.clone(),
        )?;
        // Stage 4+ 读取的 Function：有改写（pattern/聚合展开）时用副本。
        let func_ref: &Function = func_owned.as_ref().unwrap_or(func);

        *stage = Stage::ISel;
        let mut state = CompileState::new(
            self.machine.as_ref(),
            func_ref,
            self.module_sigs.clone(),
            abi_slots,
        )?;
        // Stage 1: Block mapping
        state.create_blocks(func_ref);
        profile.mark("blocks");

        // Stage 2: VReg pre-allocation
        state.alloc_params(func_ref);
        state.pre_allocate_phi_vregs(func_ref);
        state.pre_allocate_block_param_xregs(func_ref);
        profile.mark("vreg");

        // Stage 4: Instruction selection
        state.lower_all_blocks(func_ref, self.machine.lowering().as_ref())?;
        profile.mark("lower");

        // v20 A3b 预备：`FORGE_TRACE_ABI=1` 打印该函数的调用计划（引擎产物）——发射尚未切换，
        // 但"计划长什么样"现在可诊断（与既有 ABI 路径对照时这是唯一证据面）。
        if crate::pipeline::trace::knob("FORGE_TRACE_ABI") {
            eprintln!("[abi] === fn: {}", func_ref.name);
            for line in state.abi_plan.to_text().lines() {
                eprintln!("[abi] {line}");
            }
        }

        if crate::pipeline::trace::knob("FORGE_TRACE_VCODE") {
            eprintln!("[vcode] === fn: {}", func_ref.name);
            for (bi, block) in state.vcode.blocks().enumerate() {
                eprintln!("[vcode] block {bi}: {:?}", block.ir_block);
                for inst in &block.instructions {
                    eprintln!("[vcode]   {:?}", inst);
                }
            }
        }

        profile.mark("peep");

        // Stage 6: Register allocation
        *stage = Stage::RegAlloc;
        let alloc_result = state.run_regalloc(&self.reg_alloc, self.machine.as_ref())?;
        profile.mark("regalloc");
        // **溢出决策事件化**（v27 M6-87，P4）：逐值报出"谁被溢出、落在哪个槽"。
        // 只是可见性：`emit` 未设置出口环境变量时零开销（见 `pipeline::events` 契约）。
        for (vreg_idx, slot) in alloc_result.spill_slots.iter().enumerate() {
            if let Some(slot) = slot {
                events::emit(
                    "SPILL-VALUE",
                    &[
                        ("fn", events::Field::Str(&fname)),
                        ("vreg", events::Field::Usize(vreg_idx)),
                        ("offset", events::Field::I64(i64::from(slot.offset))),
                        ("size", events::Field::Usize(usize::from(slot.size))),
                    ],
                );
            }
        }
        // **逐指令溢出编辑事件化**（v27 M6-90，P4）：`SPILL-VALUE` 只说"谁溢出到哪"，
        // 这里补上"在哪条指令前后搬、搬到哪个 scratch"——排查错值的顺序从此可机读
        // （reload → bind → store 与发射侧的分段顺序一一对应）。
        for (inst_idx, edits) in alloc_result.edits.iter().enumerate() {
            for e in edits {
                let (kind, offset, scratch, width) = match e {
                    crate::alloc_result::SpillEdit::Reload {
                        slot,
                        scratch,
                        width,
                        ..
                    } => ("reload", i64::from(slot.offset), scratch, *width),
                    crate::alloc_result::SpillEdit::Bind { scratch, .. } => {
                        ("bind", 0, scratch, 0u16)
                    }
                    crate::alloc_result::SpillEdit::Store {
                        slot,
                        scratch,
                        width,
                        ..
                    } => ("store", i64::from(slot.offset), scratch, *width),
                };
                events::emit(
                    "SPILL-EDIT",
                    &[
                        ("fn", events::Field::Str(&fname)),
                        ("inst", events::Field::Usize(inst_idx)),
                        ("kind", events::Field::Str(kind)),
                        ("offset", events::Field::I64(offset)),
                        ("scratch", events::Field::U64(u64::from(scratch.num))),
                        ("width", events::Field::Usize(usize::from(width))),
                    ],
                );
            }
        }
        events::emit(
            "ALLOC-SUMMARY",
            &[
                ("fn", events::Field::Str(&fname)),
                (
                    "assigned",
                    events::Field::Usize(
                        alloc_result
                            .assignments
                            .iter()
                            .filter(|a| a.is_some())
                            .count(),
                    ),
                ),
                (
                    "spills",
                    events::Field::Usize(
                        alloc_result
                            .spill_slots
                            .iter()
                            .filter(|s| s.is_some())
                            .count(),
                    ),
                ),
                (
                    "edits",
                    events::Field::Usize(alloc_result.edits.iter().map(|e| e.len()).sum()),
                ),
                ("peak_live", events::Field::Usize(alloc_result.peak_live)),
            ],
        );
        // Stage 7: Frame layout
        let frame_size = state.calculate_frame_size(&alloc_result, self.machine.as_ref());
        profile.mark("frame");

        // Stage 8-11: Emission
        *stage = Stage::Emit;
        let result = state.emit_code(func_ref, &alloc_result, frame_size, self.machine.as_ref());
        profile.mark("emit");
        // **无条件**发（不受诊断开关影响）：CI 靠它判断"这一轮编了什么、快慢如何"。
        // 时间单位统一微秒（`as_micros()`），避免不同平台的 Debug 格式混进 JSON。
        let code_bytes = result.as_ref().map(|r| r.code.len()).unwrap_or(0);
        let relocs = result.as_ref().map(|r| r.relocations.len()).unwrap_or(0);
        events::emit(
            "COMPILE-DONE",
            &[
                ("fn", events::Field::Str(&fname)),
                (
                    "total_us",
                    events::Field::U64(profile.total().as_micros() as u64),
                ),
                (
                    "blocks_us",
                    events::Field::U64(profile.get("blocks").as_micros() as u64),
                ),
                (
                    "vreg_us",
                    events::Field::U64(profile.get("vreg").as_micros() as u64),
                ),
                (
                    "lower_us",
                    events::Field::U64(profile.get("lower").as_micros() as u64),
                ),
                (
                    "peep_us",
                    events::Field::U64(profile.get("peep").as_micros() as u64),
                ),
                (
                    "regalloc_us",
                    events::Field::U64(profile.get("regalloc").as_micros() as u64),
                ),
                (
                    "frame_us",
                    events::Field::U64(profile.get("frame").as_micros() as u64),
                ),
                (
                    "emit_us",
                    events::Field::U64(profile.get("emit").as_micros() as u64),
                ),
                ("code_bytes", events::Field::Usize(code_bytes)),
            ],
        );
        events::emit(
            "EMIT-SUMMARY",
            &[
                ("fn", events::Field::Str(&fname)),
                ("code_bytes", events::Field::Usize(code_bytes)),
                ("relocs", events::Field::Usize(relocs)),
            ],
        );
        // v26 M4：原名 `CF_CODEGEN_TIMING`——它属于**第三个命名族**、绕过了观测开关台账
        // （M1 的守卫只认 `FORGE_TRACE_*`，所以它一直没被发现）。改名并登记进 `KNOBS`。
        if crate::pipeline::trace::knob("FORGE_TRACE_TIMING") {
            eprintln!(
                "[codegen {}] total={:?} blocks={:?} vreg={:?} lower={:?} peep={:?} regalloc={:?} frame={:?} emit={:?}",
                func_ref.name,
                profile.total(),
                profile.get("blocks"),
                profile.get("vreg"),
                profile.get("lower"),
                profile.get("peep"),
                profile.get("regalloc"),
                profile.get("frame"),
                profile.get("emit")
            );
        }
        if crate::pipeline::trace::knob("FORGE_TRACE_FN_SIZE") {
            let sz = result.as_ref().map(|r| r.code.len()).unwrap_or(0);
            eprintln!("[codegen] fn={} code_bytes={sz}", func_ref.name);
            if crate::pipeline::trace::knob("FORGE_TRACE_CODE_HEX")
                && let Ok(cf) = result.as_ref()
            {
                let hex: String = cf
                    .code
                    .iter()
                    .take(64)
                    .map(|b| format!("{b:02x}"))
                    .collect();
                eprintln!("[codegen] fn={} head={hex}", func_ref.name);
            }
        }
        Ok((result?, alloc_result))
    }
}

// ============================================================
// CompileState — mutable pipeline state
// ============================================================

mod state;

pub(crate) use state::CompileState;

#[cfg(test)]
mod alloc_integration_tests;
