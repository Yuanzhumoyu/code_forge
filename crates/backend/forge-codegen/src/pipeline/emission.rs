//! Stages 8-11 of the compilation pipeline: prologue, instruction emission,
//! epilogue and fixup resolution.
//!
//! Emits the machine-code stream through the TargetEncoder, handling
//! spills (via TargetFrameLowering), labels and relocations.

use crate::machine::encoder::TargetEncoder;
use crate::machine::target::TargetMachine;
use crate::pipeline::compiler::CompileState;
use crate::pipeline::vcode::VCodeBlock;
use crate::{AllocResult, CodeSink, CompiledFunction};
use forge_ir::*;
use smallvec::SmallVec;

impl<I: crate::machine::inst::MachineInst + 'static> CompileState<I> {
    // ── Stage 8-11: Emission ──

    pub(crate) fn emit_code<M2: TargetMachine<Inst = I>>(
        &mut self,
        _func: &Function,
        alloc_result: &AllocResult,
        frame_size: u32,
        machine: &M2,
    ) -> Result<CompiledFunction, IrError> {
        let mut sink = CodeSink::new();
        if let Some(patcher) = machine.reloc_patcher() {
            sink.set_patcher(patcher);
        }
        let frame_lowering = machine.frame_lowering();
        let encoder = machine.encoder();
        // Bytes occupied by callee-saved registers pushed BELOW the frame
        // Bytes between the frame pointer and the local/spill area top:
        // fp save slot (frame_pointer_overhead) + callee-saved pushes. The
        // spill area starts at rbp - callee_saved_bytes - frame_size.
        // (Old code counted only callee-saved bytes, shifting locals/spills
        // 8 bytes into the pushed registers.)
        let callee_saved_bytes =
            crate::pipeline::frame_layout::callee_saved_bytes(machine.reg_info().as_ref());

        // Stage 8: Prologue
        frame_lowering.emit_prologue(frame_size, alloc_result, &mut sink)?;

        // Stage 9: Instruction Emission
        // Emit non-return blocks first, then return blocks last.
        // This prevents forward JMP/JCC over return blocks from breaking
        // since return blocks' emit_epilogue_jump can interfere with
        // PC-relative offset calculations for forward branches.
        let scratch_regs = machine.reg_info().scratch_regs();
        // 可变借用 vcode 收集块引用（替代整 VCode clone），xreg_map 单独
        // 借用字段——拆分借用避免闭包捕获 &mut self。
        let xreg_map = &self.xreg_map;
        let vblocks: Vec<&mut VCodeBlock<I>> = self.vcode.blocks_mut().collect();
        let mut vblocks = vblocks;

        // 预计算每个 vcode 块在全局指令序列中的起始索引（与 regalloc 的遍历序
        // 一致）。emit 的 Pass1/Pass2 重排了发射顺序（非 return 块先发，避免
        // return 块的 epilogue_jump 干扰前向 PC-relative 分支），但 xreg_map 的
        // 全局索引必须按 vcode 块序——否则 return 块不在末尾时，emit 的 global_inst
        // 与 regalloc 回写的字段索引错位，所有指令的寄存器回写错乱。
        let block_starts: Vec<usize> = {
            let mut starts = Vec::with_capacity(vblocks.len());
            let mut acc = 0usize;
            for vb in vblocks.iter() {
                starts.push(acc);
                acc += vb.instructions.len();
            }
            starts
        };

        // Helper: emit one block (bind label, instructions, optional epilogue JMP)
        // 闭包不捕获 self：只捕获 xreg_map 的借用 + 其余局部引用。
        let emit_one = |sink: &mut CodeSink,
                        vb: &mut VCodeBlock<I>,
                        vcode_idx: usize|
         -> Result<(), IrError> {
            sink.bind_label(vb.ir_block);
            for (gi, inst) in (block_starts[vcode_idx]..).zip(vb.instructions.iter_mut()) {
                let slot: &[(XReg, u8, bool)] =
                    xreg_map.get(gi).map(|s| s.as_slice()).unwrap_or(&[]);
                let inst_xregs: smallvec::SmallVec<[XReg; 8]> =
                    slot.iter().map(|(x, ..)| *x).collect();
                Self::emit_inst_with_spills(
                    inst,
                    encoder.as_ref(),
                    alloc_result,
                    frame_size,
                    &scratch_regs,
                    frame_lowering.as_ref(),
                    sink,
                    callee_saved_bytes,
                    &inst_xregs,
                    _func.name.as_str(),
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
        for (vcode_idx, vb) in vblocks.iter_mut().enumerate() {
            if !vb.is_return_block {
                emit_one(&mut sink, vb, vcode_idx)?;
            }
        }
        // Pass 2: return blocks
        for (vcode_idx, vb) in vblocks.iter_mut().enumerate() {
            if vb.is_return_block {
                emit_one(&mut sink, vb, vcode_idx)?;
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
        let code = sink.finish().map_err(IrError::Emit)?;
        if std::env::var("DBG_CODE").is_ok() {
            eprintln!("[code] ({:02x?})", code);
        }

        Ok(CompiledFunction {
            code_size: code.len(),
            code,
            relocations,
        })
    }

    fn emit_inst(
        inst: &I,
        encoder: &dyn TargetEncoder<Inst = I>,
        alloc_result: &AllocResult,
        sink: &mut CodeSink,
    ) -> Result<(), IrError> {
        if crate::pipeline::trace_enabled("FORGE_TRACE_EMIT") {
            eprintln!("[forge] emit {inst:?} alloc={alloc_result:?}");
        }
        encoder
            .encode(inst, alloc_result, sink)
            .map_err(|e| IrError::Emit(format!("encode error: {e}")))
    }

    /// Emit an instruction, handling spilled operands with load/store around the instruction.
    /// Uses scratch registers from TargetRegInfo and spill load/store from TargetFrameLowering.
    ///
    /// Each spilled VReg gets a scratch register of its correct class (Int or Float).
    /// If spilled operands exceed available scratch registers, returns an error.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn emit_inst_with_spills(
        inst: &mut I,
        encoder: &dyn TargetEncoder<Inst = I>,
        alloc_result: &AllocResult,
        frame_size: u32,
        scratch_reg_indices: &[u32],
        frame_lowering: &dyn crate::machine::frame::TargetFrameLowering<Inst = I>,
        sink: &mut CodeSink,
        callee_saved_bytes: i32,
        inst_xregs: &[XReg],
        fname: &str,
    ) -> Result<(), IrError> {
        // 去重收集 spilled XReg（同一 XReg 的 use/def 共用一个 scratch 寄存器）
        let mut spilled: smallvec::SmallVec<[XReg; 8]> = smallvec::SmallVec::new();
        for &xreg in inst_xregs {
            if alloc_result.is_spilled(xreg) && !spilled.contains(&xreg) {
                spilled.push(xreg);
            }
        }
        if spilled.is_empty() {
            return Self::emit_inst(inst, encoder, alloc_result, sink);
        }

        // Verify we have enough scratch registers
        if spilled.len() > scratch_reg_indices.len() {
            return Err(IrError::RegAlloc(format!(
                "instruction needs {} scratch regs for spilled operands, but only {} available",
                spilled.len(),
                scratch_reg_indices.len()
            )));
        }

        // Build scratch register pool — each spilled XReg gets its own scratch
        // of the correct class.
        let spill_scratch: smallvec::SmallVec<[PReg; 8]> = spilled
            .iter()
            .enumerate()
            .map(|(i, &xreg)| PReg::new(scratch_reg_indices[i], xreg.class()))
            .collect();

        // 稀疏覆盖：spilled XReg → scratch PReg（≤2 项），取代原整 AllocResult
        // 深克隆（assignments+spill_slots 两个 HashMap 每 spill 指令克隆一次）。
        // encode 从指令字段读物理寄存器（set_reg_field 已写入），reg_map 查询
        // 只发生在本地 set_reg_field/store 回写，overrides 足够。
        let mut overrides: SmallVec<[(XReg, PReg); 2]> = smallvec::SmallVec::new();
        for (i, &xreg) in spilled.iter().enumerate() {
            overrides.push((xreg, spill_scratch[i]));
        }
        let xreg_preg = |v: XReg| -> Option<PReg> {
            overrides
                .iter()
                .find(|(x, _)| *x == v)
                .map(|&(_, p)| p)
                .or_else(|| alloc_result.preg(v))
        };
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
                xreg.class().is_fp(),
                sink,
            )?;
        }

        // Emit 前把 spilled 字段重设为 scratch 寄存器：
        // 分配阶段回写的旧寄存器（如 reload 后的 R9）与 scratch 不一致，
        // 会导致 store 存回错误寄存器。重设后 emit 与 store 使用同一寄存器。
        for (fi, &xreg) in inst_xregs.iter().enumerate() {
            if let Some(preg) = xreg_preg(xreg) {
                inst.set_reg_field(fi, preg.num);
            }
        }

        // Emit the instruction（reg_map 传 base——encode 从指令字段读寄存器，
        // 不依赖覆盖表；若某 ISA arm 回退 resolve 需覆盖，spill 测试会捕获）
        let result = Self::emit_inst(inst, encoder, alloc_result, sink);

        // Store spilled defs back to stack
        for (i, &xreg) in spilled.iter().enumerate() {
            let spill_off = alloc_result.spill_slot(xreg).offset;
            let width = xreg.width();
            if crate::pipeline::trace_enabled("FORGE_TRACE_SPILL") {
                eprintln!(
                    "[spill] {fname} store x{} width={} off={} fp={}",
                    xreg.index(),
                    width,
                    sp_base + spill_off,
                    xreg.class().is_fp()
                );
            }
            if let Some(preg) = xreg_preg(xreg) {
                frame_lowering.emit_spill_store(
                    preg.num,
                    sp_base + spill_off,
                    width,
                    preg.class.is_fp(),
                    sink,
                )?;
            }
            let _ = i;
        }

        result
    }
}
