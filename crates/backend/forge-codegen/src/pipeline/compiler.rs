//! FunctionCompiler v20 — staged compilation pipeline.
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
//! 5. Peephole Optimization  — TargetPeephole::optimize (optional)
//! 6. Register Allocation    — pluggable allocator
//! 7. Frame Layout           — compute spill slots, alignment, frame size
//! 8. Prologue               — TargetFrameLowering::emit_prologue
//! 9. Instruction Emission   — TargetEncoder + spill/label/fixup handling
//! 10. Epilogue              — TargetFrameLowering::emit_epilogue
//! 11. Fixup Resolution      — CodeSink::finish()

use crate::machine::encoder::TargetEncoder;
use crate::machine::lowering::TargetLowering;
use crate::machine::peephole::TargetPeephole;
use crate::machine::target::TargetMachine;
use crate::pipeline::alloc_config::{ClassConfig, RegAllocConfig as NewRegAllocConfig};

/// Map an `AtomicRmwOp` discriminant (as encoded by `builder.atomic_rmw` into
/// `Immediate::Uint`) back to the enum. Defaults to `Add` on unknown values.
fn atomic_op_from_u64(v: u64) -> AtomicRmwOp {
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
        _ => Umin,
    }
}
use crate::pipeline::alloc_result::AllocResult;
use crate::pipeline::regalloc_trait::RegisterAllocator;
use crate::{
    CodeSink, CompileError, CompiledFunction, LowerCtx, MachineInst, VBlockId, VCode, VCodeBlock,
};
use forge_ir::*;
use std::collections::HashMap;

/// Function compiler — drives the full IR → machine code pipeline.
pub struct FunctionCompiler<M: TargetMachine> {
    machine: M,
    reg_alloc: RegisterAllocator,
}

impl<M: TargetMachine> FunctionCompiler<M> {
    /// Create a new compiler with the default register allocator (backtracking).
    pub fn new(machine: M) -> Self {
        Self {
            machine,
            reg_alloc: RegisterAllocator,
        }
    }

    /// Create a compiler with a specific register allocator.
    /// Kept for API compatibility; currently only `RegisterAllocator` exists.
    pub fn with_reg_alloc(machine: M, reg_alloc: RegisterAllocator) -> Self {
        Self { machine, reg_alloc }
    }

    /// Compile an IR function (full pipeline).
    pub fn compile(&self, func: &Function) -> Result<CompiledFunction, CompileError> {
        self.compile_raw(func)
    }

    /// Compile an IR function (raw, no IR-level optimization passes).
    pub fn compile_raw(&self, func: &Function) -> Result<CompiledFunction, CompileError> {
        let mut state = CompileState::new(&self.machine, func);

        // Stage 1: Block mapping
        state.create_blocks(func);

        // Stage 2: VReg pre-allocation
        state.alloc_params(func);
        state.pre_allocate_phi_vregs(func);
        state.pre_allocate_block_param_xregs(func);

        // Stage 3: Pattern matching (TODO: integrate PatternMatcher)
        // Stage 4: Instruction selection
        state.lower_all_blocks(func, self.machine.lowering().as_ref())?;

        // Stage 5: Peephole optimization
        if let Some(peep) = self.machine.peephole() {
            state.run_peephole(peep.as_ref());
        }

        // Stage 6: Register allocation
        let alloc_result = state.run_regalloc(&self.reg_alloc, &self.machine)?;

        // Stage 7: Frame layout
        let frame_size = state.calculate_frame_size(&alloc_result, &self.machine);

        // Stage 8-11: Emission
        state.emit_code(func, &alloc_result, frame_size, &self.machine)
    }
}

// ============================================================
// CompileState — mutable pipeline state
// ============================================================

struct CompileState<I: MachineInst> {
    vcode: VCode<I>,
    /// XReg → 微指令寄存器字段映射（与 vcode 的指令序列平行）。
    /// 由各指令包的 xreg_map 聚合；分配器按此构建活区间并回写字段。
    xreg_map: Vec<smallvec::SmallVec<[XReg; 2]>>,
    value_to_xreg: HashMap<Value, XReg>,
    block_map: HashMap<Block, VBlockId>,
    ctx: LowerCtx,
    param_xregs: Vec<XReg>,
}

impl<I: MachineInst + 'static> CompileState<I> {
    fn new<M: TargetMachine>(_machine: &M, func: &Function) -> Self {
        let mut ctx = LowerCtx::new();
        ctx.call_conv = func.calling_convention;
        ctx.is_float_return = func
            .return_tys
            .iter()
            .any(|t| RegClass::from_type_id(*t).is_fp());
        ctx.constant_pool = Some(func.constants.clone());

        Self {
            vcode: VCode::new(),
            xreg_map: Vec::new(),
            value_to_xreg: HashMap::new(),
            block_map: HashMap::new(),
            ctx,
            param_xregs: Vec::new(),
        }
    }

    // ── Stage 1: Block Mapping ──

    fn create_blocks(&mut self, func: &Function) {
        for (i, _) in func.dfg.blocks.iter().enumerate() {
            let block_id = Block(i as u32);
            let vblock_id = self.vcode.create_block(block_id);
            self.block_map.insert(block_id, vblock_id);
        }
    }

    // ── Stage 2: VReg Pre-allocation ──

    fn alloc_params(&mut self, func: &Function) {
        if let Some(entry_id) = func.entry_block {
            let entry = func.dfg.block(entry_id);
            let param_values = func.dfg.block_param_values(entry_id);
            for (i, param_ty) in entry.params.iter().enumerate() {
                let class = if RegClass::from_type_id(*param_ty).is_fp() {
                    RegClass::FPR
                } else {
                    RegClass::GPR
                };
                // 位宽内嵌：i32 参数按 4 字节（32 位）——分配器与编码按宽度匹配，
                // 比较/算术指令按 32 位语义（如 cmp eax / setg 基于 32 位 SF/OF）。
                let width = if matches!(*param_ty, crate::prelude::TypeId::I32) {
                    4
                } else {
                    class.default_width()
                };
                let xreg = self.ctx.alloc_xreg_with_width(class, width);
                self.ctx.xreg_types.insert(xreg, *param_ty);

                self.param_xregs.push(xreg);
                if let Some(&param_val) = param_values.get(i) {
                    self.value_to_xreg.insert(param_val, xreg);
                }
            }
        }
    }

    fn pre_allocate_phi_vregs(&mut self, func: &Function) {
        for (block, _) in func.dfg.blocks() {
            for inst in func.dfg.block_inst_iter(block) {
                if matches!(inst.opcode, Opcode::Copy) {
                    let xreg = self.ctx.alloc_xreg(RegClass::GPR);
                    if let Some(v) = inst.results.first().copied() {
                        self.value_to_xreg.insert(v, xreg);
                    }
                    for v in &inst.operands {
                        self.value_to_xreg.insert(*v, xreg);
                    }
                }
            }
        }
    }

    fn pre_allocate_block_param_xregs(&mut self, func: &Function) {
        struct BlockParamInfo {
            block: Block,
            param_values: Vec<Value>,
            param_tys: Vec<TypeId>,
        }

        let mut block_params_list: Vec<BlockParamInfo> = Vec::new();
        for (block, block_data) in func.dfg.blocks() {
            if Some(block) == func.entry_block {
                continue;
            }
            let param_values: Vec<Value> = block_data.param_values.iter().copied().collect();
            if !param_values.is_empty() {
                block_params_list.push(BlockParamInfo {
                    block,
                    param_values,
                    param_tys: block_data.params.iter().copied().collect(),
                });
            }
        }

        if block_params_list.is_empty() {
            return;
        }

        let preds = func.predecessors();

        for info in &block_params_list {
            let param_xregs: Vec<XReg> = info
                .param_tys
                .iter()
                .map(|ty| {
                    let class = if RegClass::from_type_id(*ty).is_fp() {
                        RegClass::FPR
                    } else {
                        RegClass::GPR
                    };
                    let xreg = self.ctx.alloc_xreg(class);
                    self.ctx.xreg_types.insert(xreg, *ty);
                    xreg
                })
                .collect();

            for (i, &pv) in info.param_values.iter().enumerate() {
                if i < param_xregs.len() {
                    self.value_to_xreg.insert(pv, param_xregs[i]);
                }
            }

            if let Some(pred_list) = preds.get(&info.block) {
                for pred in pred_list {
                    let term = func.dfg.block_terminator(*pred);
                    self.map_terminator_args_to_params(
                        term,
                        info.block,
                        &param_xregs,
                        &info.param_tys,
                    );
                }
            }
        }
    }

    fn map_terminator_args_to_params(
        &mut self,
        term: Option<&Terminator>,
        target: Block,
        param_xregs: &[XReg],
        param_tys: &[TypeId],
    ) {
        match term {
            Some(Terminator::Jump { target: t, args }) if *t == target => {
                for (i, &arg) in args.iter().enumerate() {
                    if i < param_xregs.len() {
                        self.value_to_xreg.insert(arg, param_xregs[i]);
                        let ty = param_tys.get(i).copied().unwrap_or(TypeId::VOID);
                        self.ctx.xreg_types.insert(param_xregs[i], ty);
                    }
                }
            }
            Some(Terminator::Branch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            }) => {
                let args: &[Value] = if *then_block == target {
                    then_args
                } else if *else_block == target {
                    else_args
                } else {
                    return;
                };
                for (i, &arg) in args.iter().enumerate() {
                    if i < param_xregs.len() {
                        self.value_to_xreg.insert(arg, param_xregs[i]);
                        let ty = param_tys.get(i).copied().unwrap_or(TypeId::VOID);
                        self.ctx.xreg_types.insert(param_xregs[i], ty);
                    }
                }
            }
            _ => {}
        }
    }

    // ── Stage 4: Instruction Selection ──

    fn lower_all_blocks(
        &mut self,
        func: &Function,
        lowering: &dyn TargetLowering<Inst = I>,
    ) -> Result<(), CompileError> {
        for (i, block_data) in func.dfg.blocks.iter().enumerate() {
            self.lower_block(Block(i as u32), block_data, &func.dfg, lowering)?;
        }
        Ok(())
    }

    fn lower_block(
        &mut self,
        block: Block,
        block_data: &BlockData,
        dfg: &DataFlowGraph,
        lowering: &dyn TargetLowering<Inst = I>,
    ) -> Result<(), CompileError> {
        let vblock_id = self.block_map[&block];
        self.vcode.switch_to_block(vblock_id);

        let instructions: Vec<forge_ir::Instruction> =
            dfg.block_inst_iter(block).cloned().collect();

        for inst in &instructions {
            let args: Vec<XReg> = inst
                .operands
                .iter()
                .map(|v| {
                    let ty = dfg.value_type(*v).unwrap_or(TypeId::VOID);
                    let class = if RegClass::from_type_id(ty).is_fp() {
                        RegClass::FPR
                    } else {
                        RegClass::GPR
                    };
                    self.get_or_alloc_xreg(*v, class)
                })
                .collect();

            let result = if matches!(inst.opcode, Opcode::Copy) {
                inst.operands
                    .first()
                    .and_then(|v| self.value_to_xreg.get(v).copied())
                    .or_else(|| Some(self.ctx.alloc_xreg(RegClass::GPR)))
            } else {
                inst.results.first().copied().map(|v| {
                    self.value_to_xreg.get(&v).copied().unwrap_or_else(|| {
                        let result_ty = dfg.value_type(v).unwrap_or(TypeId::VOID);
                        let class = if RegClass::from_type_id(result_ty).is_fp() {
                            RegClass::FPR
                        } else {
                            RegClass::GPR
                        };
                        let xreg = self.ctx.alloc_xreg(class);
                        self.value_to_xreg.insert(v, xreg);
                        self.ctx.xreg_types.insert(xreg, result_ty);
                        xreg
                    })
                })
            };
            let _ = result;

            if matches!(inst.opcode, Opcode::Copy) {
                continue; // Phi doesn't generate machine instructions
            }

            // Nop instructions (used as tombstone by optimization passes such as
            // GVN/CSE/dead-code) generate no machine instructions. Skipping them
            // here keeps IR that has been through an optimization pipeline
            // compilable on every target.
            if matches!(inst.opcode, Opcode::Nop) {
                continue;
            }

            let result_ty = inst.results.first().and_then(|v| dfg.value_type(*v));
            self.ctx.default_opsize = match result_ty {
                Some(ty) => LowerCtx::opsize_from_type(&ty),
                None => 64,
            };

            if let Some(Immediate::Const(cid)) = inst.immediates.first() {
                self.ctx.current_const_index = cid.0;
            }
            if let Some(Immediate::Func(f)) = inst.immediates.first() {
                self.ctx.current_func_ref = Some(*f);
            }
            if let Some(Immediate::Global(g)) = inst.immediates.first() {
                self.ctx.current_global = Some(*g);
            }
            if let Some(Immediate::Int(v)) = inst.immediates.first() {
                self.ctx.current_offset = *v;
            }
            // AtomicRmw: immediates[0] = op (Uint(op as u64)), [1] = ordering
            if matches!(inst.opcode, Opcode::AtomicRmw)
                && let Some(Immediate::Uint(v)) = inst.immediates.first()
            {
                self.ctx.current_atomic_op = Some(atomic_op_from_u64(*v));
            }

            // 全部结果映射（多结果 op——如溢出 op 的 (result, flag)）
            let results: Vec<XReg> = inst
                .results
                .iter()
                .map(|v| {
                    self.value_to_xreg.get(v).copied().unwrap_or_else(|| {
                        let xreg = self.ctx.alloc_xreg(RegClass::GPR);
                        self.value_to_xreg.insert(*v, xreg);
                        let result_ty = dfg.value_type(*v).unwrap_or(TypeId::VOID);
                        self.ctx.xreg_types.insert(xreg, result_ty);
                        xreg
                    })
                })
                .collect();

            let machine_insts =
                lowering.lower_inst(&inst.opcode, &args, &results, &mut self.ctx)?;
            self.xreg_map.extend(machine_insts.xreg_map);
            for mi in machine_insts.insts {
                self.vcode.push_inst(mi);
            }
        }

        // Lower terminator
        let term_insts = lowering.lower_terminator(
            &block_data.terminator,
            &self.value_to_xreg,
            &self.block_map,
            &mut self.ctx,
        )?;
        self.xreg_map.extend(term_insts.xreg_map);
        for mi in term_insts.insts {
            self.vcode.push_inst(mi);
        }

        if matches!(block_data.terminator, Terminator::Return { .. })
            && let Some(vb) = self.vcode.block_mut(vblock_id)
        {
            vb.is_return_block = true;
        }

        Ok(())
    }

    fn get_or_alloc_xreg(&mut self, val: Value, class: RegClass) -> XReg {
        if let Some(xreg) = self.value_to_xreg.get(&val).copied() {
            xreg
        } else {
            let xreg = self.ctx.alloc_xreg(class);
            self.value_to_xreg.insert(val, xreg);
            xreg
        }
    }

    // ── Stage 5: Peephole ──

    fn run_peephole(&mut self, peephole: &dyn TargetPeephole<Inst = I>) {
        for block in self.vcode.blocks_mut() {
            peephole.optimize(&mut block.instructions);
        }
    }

    // ── Stage 6: Register Allocation ──

    fn run_regalloc<M2: TargetMachine<Inst = I>>(
        &mut self,
        allocator: &RegisterAllocator,
        machine: &M2,
    ) -> Result<AllocResult, CompileError> {
        let ri = machine.reg_info();

        // Build new RegAllocConfig from TargetRegInfo
        let mut classes = HashMap::new();
        classes.insert(
            RegClass::GPR,
            ClassConfig {
                allocatable: ri.allocatable_gp_order(),
                reg_width: ri.reg_class_width(RegClass::GPR),
            },
        );
        classes.insert(
            RegClass::FPR,
            ClassConfig {
                allocatable: ri.allocatable_fp_order(),
                reg_width: ri.reg_class_width(RegClass::FPR),
            },
        );

        let precolored: HashMap<XReg, PReg> = ri.precolored_xregs().into_iter().collect();

        let reg_info = NewRegAllocConfig {
            classes,
            sp_reg: ri.sp_reg().register_index().unwrap_or(0),
            fp_reg: ri.fp_reg().map(|r| r.to_index()),
            callee_saved: ri.callee_saved(),
            precolored,
            scratch_regs: ri
                .scratch_regs()
                .iter()
                .map(|&n| PReg::new(n, RegClass::GPR))
                .collect(),
            param_xregs: self.param_xregs.clone(),
        };

        let ctx = crate::pipeline::alloc_config::AllocContext {};

        let alloc_result = allocator.allocate(&self.vcode, &reg_info, &ctx, &self.xreg_map)?;

        // 分配后回写：按 xreg_map 把 XReg 的分配结果填入微指令寄存器字段（物理 Reg）
        let mut global_inst = 0usize;
        for block in self.vcode.blocks_mut() {
            for inst in block.instructions.iter_mut() {
                if let Some(slot) = self.xreg_map.get(global_inst) {
                    for (field_idx, &xreg) in slot.iter().enumerate() {
                        if let Some(preg) = alloc_result.preg(xreg) {
                            inst.set_reg_field(field_idx, preg.num);
                        }
                    }
                }
                global_inst += 1;
            }
        }
        let _ = std::mem::take(&mut self.param_xregs); // param_xregs 现在由 alloc_result 管理
        Ok(alloc_result)
    }

    // ── Stage 7: Frame Layout ──

    fn calculate_frame_size<M2: TargetMachine<Inst = I>>(
        &self,
        alloc_result: &AllocResult,
        machine: &M2,
    ) -> u32 {
        // spill 区域：使用 regalloc 报告的 spill_area_size（含对齐与全部槽位），
        // 而不是仅对分配到的槽求和——后者在槽释放/复用时会低估实际需要的栈帧。
        let size: u32 = alloc_result.frame_info.spill_area_size;
        let align = machine.abi().stack_align();
        // 栈对齐修正（align/2 = 8 字节）：prologue push rbp + callee-saved 后
        // rsp%16 == 8（入口 rsp%16==8 由 call 压入的返回地址造成），sub rsp 必须
        // 使 call 前 rsp%16 == 0（SysV/Windows x64 ABI）。spill 区 size 是 16 的
        // 倍数（%16==0），故 frame 需额外 +8 才能满足。无 call 的函数多 8 字节
        // 栈帧无害；有 call 的否则外部函数（Rust C ABI 的 movaps 保存）会 SEGV。
        size.div_ceil(align) * align + align / 2
    }

    // ── Stage 8-11: Emission ──

    fn emit_code<M2: TargetMachine<Inst = I>>(
        &self,
        _func: &Function,
        alloc_result: &AllocResult,
        frame_size: u32,
        machine: &M2,
    ) -> Result<CompiledFunction, CompileError> {
        let mut sink = CodeSink::new();
        if let Some(patcher) = machine.reloc_patcher() {
            sink.set_patcher(patcher);
        }
        let frame_lowering = machine.frame_lowering();
        let encoder = machine.encoder();
        // Bytes occupied by callee-saved registers pushed BELOW the frame
        // pointer. The frame pointer itself sits at [rbp] (above the callee
        // saves), so it must NOT be counted here — the spill area starts at
        // rbp - callee_saved_bytes - frame_size.
        let callee_saved_bytes = (machine.reg_info().callee_saved().len() as i32)
            * (machine.reg_info().reg_class_width(RegClass::GPR) as i32);

        // Stage 8: Prologue
        frame_lowering.emit_prologue(frame_size, alloc_result, &mut sink)?;

        // Stage 9: Instruction Emission
        // Emit non-return blocks first, then return blocks last.
        // This prevents forward JMP/JCC over return blocks from breaking
        // since return blocks' emit_epilogue_jump can interfere with
        // PC-relative offset calculations for forward branches.
        let scratch_regs = machine.reg_info().scratch_regs();
        let all_vblocks: Vec<_> = self.vcode.blocks().cloned().collect();
        let mut all_vblocks = all_vblocks;

        // Helper: emit one block (bind label, instructions, optional epilogue JMP)
        let mut global_inst = 0usize;
        let mut emit_one =
            |sink: &mut CodeSink, vb: &mut VCodeBlock<I>| -> Result<(), CompileError> {
                sink.bind_label(vb.ir_block);
                for inst in &mut vb.instructions {
                    let slot = self.xreg_map.get(global_inst).cloned().unwrap_or_default();
                    global_inst += 1;
                    self.emit_inst_with_spills(
                        inst,
                        encoder.as_ref(),
                        alloc_result,
                        frame_size,
                        &scratch_regs,
                        frame_lowering.as_ref(),
                        sink,
                        callee_saved_bytes,
                        &slot,
                    )?;
                }
                if vb.is_return_block && frame_lowering.needs_epilogue_label() {
                    frame_lowering.emit_epilogue_jump(
                        encoder,
                        alloc_result,
                        Block(0xFFFFFFFD),
                        sink,
                    )?;
                }
                Ok(())
            };

        // Pass 1: non-return blocks
        for vb in &mut all_vblocks {
            if !vb.is_return_block {
                emit_one(&mut sink, vb)?;
            }
        }
        // Pass 2: return blocks
        for vb in &mut all_vblocks {
            if vb.is_return_block {
                emit_one(&mut sink, vb)?;
            }
        }

        // Stage 10: Epilogue
        if frame_lowering.needs_epilogue_label() {
            let epilogue_block = Block(0xFFFFFFFD);
            sink.bind_label(epilogue_block);
        }
        frame_lowering.emit_epilogue(frame_size, alloc_result, &mut sink)?;

        // Stage 11: Fixup resolution
        let relocations = sink.relocations();
        let code = sink.finish().map_err(CompileError::Emit)?;

        Ok(CompiledFunction {
            code_size: code.len(),
            code,
            relocations,
        })
    }

    fn emit_inst(
        &self,
        inst: &I,
        encoder: &dyn TargetEncoder<Inst = I>,
        alloc_result: &AllocResult,
        sink: &mut CodeSink,
    ) -> Result<(), CompileError> {
        encoder
            .encode(inst, alloc_result, sink)
            .map_err(|e| CompileError::Emit(format!("encode error: {e}")))
    }

    /// Emit an instruction, handling spilled operands with load/store around the instruction.
    /// Uses scratch registers from TargetRegInfo and spill load/store from TargetFrameLowering.
    ///
    /// Each spilled VReg gets a scratch register of its correct class (Int or Float).
    /// If spilled operands exceed available scratch registers, returns an error.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn emit_inst_with_spills(
        &self,
        inst: &mut I,
        encoder: &dyn TargetEncoder<Inst = I>,
        alloc_result: &AllocResult,
        frame_size: u32,
        scratch_reg_indices: &[u8],
        frame_lowering: &dyn crate::machine::frame::TargetFrameLowering<Inst = I>,
        sink: &mut CodeSink,
        callee_saved_bytes: i32,
        inst_xregs: &[XReg],
    ) -> Result<(), CompileError> {
        // 去重收集 spilled XReg（同一 XReg 的 use/def 共用一个 scratch 寄存器）
        let mut spilled: Vec<XReg> = Vec::new();
        for &xreg in inst_xregs {
            if alloc_result.is_spilled(xreg) && !spilled.contains(&xreg) {
                spilled.push(xreg);
            }
        }
        if spilled.is_empty() {
            return self.emit_inst(inst, encoder, alloc_result, sink);
        }

        // Verify we have enough scratch registers
        if spilled.len() > scratch_reg_indices.len() {
            return Err(CompileError::RegAlloc(format!(
                "instruction needs {} scratch regs for spilled operands, but only {} available",
                spilled.len(),
                scratch_reg_indices.len()
            )));
        }

        // Build scratch register pool — each spilled XReg gets its own scratch
        // of the correct class.
        let spill_scratch: Vec<PReg> = spilled
            .iter()
            .enumerate()
            .map(|(i, &xreg)| PReg::new(scratch_reg_indices[i], xreg.class()))
            .collect();

        // Clone AllocResult and redirect spilled XRegs to scratch PRegs
        let mut local_rm = alloc_result.clone();
        // The spill area lives BELOW the pushed callee-saved registers, whose
        // saved slots occupy [rbp - callee_saved_bytes, rbp):
        //   spill area = [rbp - callee_saved_bytes - frame_size, rbp - callee_saved_bytes)
        // The frame pointer itself sits at [rbp] (pushed before the callee
        // saves), so callee_saved_bytes must NOT include it — otherwise spills
        // land 8 bytes below rsp and can hit an unmapped stack page in release
        // builds (test_spill_high_pressure / e2e_jit_execute hang).
        let sp_base: i32 = -(frame_size as i32) - callee_saved_bytes;

        // Load spilled operands into scratch registers (width-aware).
        // 纯 def 的 XReg 也 load（其值会被指令覆盖，无害但保证 use/def 语义统一）。
        for (i, &xreg) in spilled.iter().enumerate() {
            let spill_off = alloc_result.spill_slot(xreg).offset;
            let width = xreg.width();
            frame_lowering.emit_spill_load(
                spill_scratch[i].num,
                sp_base + spill_off,
                width,
                sink,
            )?;
            local_rm.insert(xreg, spill_scratch[i]);
        }

        // Emit 前把 spilled 字段重设为 scratch 寄存器：
        // 分配阶段回写的旧寄存器（如 reload 后的 R9）与 scratch 不一致，
        // 会导致 store 存回错误寄存器。重设后 emit 与 store 使用同一寄存器。
        for (fi, &xreg) in inst_xregs.iter().enumerate() {
            if let Some(preg) = local_rm.preg(xreg) {
                inst.set_reg_field(fi, preg.num);
            }
        }

        // Emit the instruction with the modified AllocResult
        let result = self.emit_inst(inst, encoder, &local_rm, sink);

        // Store spilled defs back to stack
        for (i, &xreg) in spilled.iter().enumerate() {
            let spill_off = alloc_result.spill_slot(xreg).offset;
            let width = xreg.width();
            if let Some(preg) = local_rm.preg(xreg) {
                frame_lowering.emit_spill_store(preg.num, sp_base + spill_off, width, sink)?;
            }
            let _ = i;
        }

        result
    }
}
