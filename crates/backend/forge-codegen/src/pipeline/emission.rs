//! Stages 8-11 of the compilation pipeline: prologue, instruction emission,
//! epilogue and fixup resolution.
//!
//! Emits the machine-code stream through the TargetEncoder, handling
//! spills (via TargetFrameLowering), labels and relocations.

use crate::machine::encoder::TargetEncoder;
use crate::machine::target::TargetMachine;
use crate::pipeline::alloc_result::SpillEdit;
use crate::pipeline::compiler::CompileState;
use crate::pipeline::vcode::VCodeBlock;
use crate::{AllocResult, CodeSink, CompiledFunction};
use forge_ir::*;

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
        // v23 M5（C6 帧记账单点）：不再重算——`LowerCtx` 上那份就是同一输入的唯一结果
        // （重算口径一漂移，spill 基点就与序言实际推入量不一致 → 静默错值）。
        let callee_saved_bytes = self.ctx.callee_saved_bytes;
        // 栈参数区（shadow space + 第 5+ 参数槽）：调用方 store 到帧底之上，
        // spill 槽起始需上移该字节数（见 emit_inst_with_spills）。
        let stack_args = self.ctx.max_stack_arg_bytes as i32;

        // **v21 W9（选项 A）：序言按 plan 的整张表推** ⇒ 运行时保存数 == plan ⇒
        // 帧基准（plan）与运行时值**同源**，不再需要"少推的槽补进帧字节数"的补偿
        //（旧的 `cs_skipped` = 静态表长 − 实际推入数，随两键删除一并消失）。
        let frame_size_emitted = frame_size;

        // Stage 8: Prologue
        frame_lowering.emit_prologue(frame_size_emitted, alloc_result, &mut sink)?;

        // Stage 9: Instruction Emission（自然次序；重排 hack 已随 v23 M5 删除，见下）
        // 可变借用 vcode 收集块引用（替代整 VCode clone），inst_ops 单独
        // 借用字段——拆分借用避免闭包捕获 &mut self。
        // v23 M5：绑定表/溢出决策都不再在这里读了——编辑序列由分配器产出（AllocResult::edits）。
        // 回填宽度类**不再**在这里现推：值宽类已由管线折进 `OperandBind::class`
        // （v23 M4 批次 C 第二步，见 compiler.rs 的折入步骤）——此处只借绑定表。
        let vblocks: Vec<&mut VCodeBlock<I>> = self.vcode.blocks_mut().collect();
        let mut vblocks = vblocks;

        // 预计算每个 vcode 块在全局指令序列中的起始索引（与 regalloc 的遍历序
        // 一致）。emit 的 Pass1/Pass2 重排了发射顺序（非 return 块先发，避免
        // return 块的 epilogue_jump 干扰前向 PC-relative 分支），但 inst_ops 的
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

        // **统一尾声标签由 sink 分配**（v23 M5：替掉 `ExternalLabel::EPILOGUE` 哨兵）。
        // 只有"不内联尾声"的 ISA 才需要它（`needs_epilogue_label`）——那种形态下所有
        // return 块跳到同一段收尾代码，而那段代码在所有块之后发射，因此是**前向引用**，
        // 由 `CodeSink` 的 pending fixup 兜住。
        let epilogue_label = if frame_lowering.needs_epilogue_label() {
            debug_assert!(
                (vblocks.len() as u64) < crate::pipeline::emit::LabelRef::MACHINE_LABEL_BASE as u64,
                "块索引空间 0x{:X} 已逼近机器标签基址（块数 {}）",
                crate::pipeline::emit::LabelRef::MACHINE_LABEL_BASE,
                vblocks.len()
            );
            Some(sink.new_label())
        } else {
            None
        };

        // Helper: emit one block (bind label, instructions, optional epilogue JMP)
        // 闭包不捕获 self：只捕获 inst_ops 的借用 + 其余局部引用。
        // line_tables：debuginfo 开启（func 有行号或 line_entries 非空要求）
        // 时逐指令收集 (机器码偏移, 行)（同一条 IR 指令展开的微指令共享行，
        // 相邻同行合并）。
        let mut line_tables: Vec<(u32, u32)> = Vec::new();
        let mut emit_one = |sink: &mut CodeSink,
                            vb: &mut VCodeBlock<I>,
                            vcode_idx: usize|
         -> Result<(), IrError> {
            sink.bind_label(vb.ir_block);
            for (gi, inst) in (block_starts[vcode_idx]..).zip(vb.instructions.iter_mut()) {
                // v23 M5：绑定表已不再在这里消费（溢出的决策/编辑序列来自分配器），
                // 但 `inst_ops` 的取值仍需推进 `gi`——真正的读取在下面的 edits 里。
                let inst_local_idx = gi - block_starts[vcode_idx];
                let line = vb.inst_lines.get(inst_local_idx).copied().flatten();
                let start_off = sink.offset() as u32;
                Self::emit_inst_with_spills(
                    inst,
                    encoder.as_ref(),
                    alloc_result,
                    // 与序言实际分配的一致（含 push 少推的补偿），否则 spill 基点错位。
                    frame_size_emitted,
                    frame_lowering.as_ref(),
                    sink,
                    callee_saved_bytes,
                    stack_args,
                    // v23 M5：溢出编辑由**分配器**产出（谁溢出、用哪个 scratch、改写哪个
                    // 字段、要不要写回），发射侧只按序执行。
                    alloc_result
                        .edits
                        .get(gi)
                        .map(|e| e.as_slice())
                        .unwrap_or(&[]),
                    _func.name.as_str(),
                )?;
                if let Some(ln) = line {
                    // 相邻同源指令行合并（同一条语句的多条微指令 / 相邻
                    // 同语句行）——只记行变化处（每行起点）。
                    match line_tables.last() {
                        Some(&(_, last_line)) if last_line == ln => {}
                        _ => line_tables.push((start_off, ln)),
                    }
                }
            }
            if let (true, Some(epilogue)) = (vb.is_return_block, epilogue_label) {
                frame_lowering.emit_epilogue_jump(encoder, alloc_result, epilogue, sink)?;
            }
            Ok(())
        };

        // **按 vcode 的自然次序发射**（v23 M5：删掉"非返回块先发、返回块最后"的重排 hack）。
        // 重排当年是为了"返回块的 epilogue_jump 干扰前向 PC-relative 分支偏移计算"——那是
        // 编码器**急切算偏移**时代的遗留：现在 `emit_epilogue_jump` 把目标写成**标签号**，
        // 由 `CodeSink` 记 pending fixup，`bind_label`/`finish` 时统一打补丁（见 `emit.rs`），
        // 前向引用天然成立。回归判据 = 两矩阵（x86 前向跳转 / riscv 分支范围）+ `asm_exec`。
        for (vcode_idx, vb) in vblocks.iter_mut().enumerate() {
            emit_one(&mut sink, vb, vcode_idx)?;
        }

        // Stage 10: Epilogue
        if let Some(epilogue) = epilogue_label {
            // 把所有 return 块的前向跳转落到这里。标签由 `sink.new_label()` 分配（独立 id
            // 区间），不再有哨兵常量、也不需要"块数不撞保留区间"的空间断言。
            sink.bind_label(epilogue);
        }
        frame_lowering.emit_epilogue(frame_size, alloc_result, &mut sink)?;

        // Stage 11: Fixup resolution
        let relocations = sink.relocations();
        let code = sink.finish().map_err(IrError::Emit)?;
        if std::env::var("DBG_CODE").is_ok() {
            eprintln!("[code] ({:02x?})", code);
        }

        if std::env::var("FORGE_LINE_ROWS").is_ok() {
            eprintln!("[lines] {} {:?}", _func.name.as_str(), line_tables);
        }
        // M2：机器码完整后收集函数级 CFI（.debug_frame 行）。经 ISA 的
        // TargetMachine::function_cfi 派发（machine::cfi 按 ISA 名查扫描器）
        // ——本通用管线不引用任何 ISA 专属代码；x86_64 命中固定 prologue
        // 扫描（字节开头 = Stage 8 的 emit_prologue 最先写 sink），riscv64/
        // demo/未注册 ISA → None（不产 CFI，零回归）。scan 只比较 15 字节
        // 常量前缀，开销可忽略。
        let cfi = machine.function_cfi(&code);
        Ok(CompiledFunction {
            code_size: code.len(),
            code,
            relocations,
            line_entries: line_tables,
            cfi,
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

    /// Emit an instruction, **按分配器给的溢出编辑序列**执行（v23 M5 的 `SpillEdit`）。
    ///
    /// 决策（谁溢出、用哪个 scratch、哪个字段改写、要不要写回）全部在分配器里做完
    /// （`build_spill_edits`）；这里只剩三类动作，按序执行：
    /// `Reload`（装值）→ `Bind`（改写字段）→ 发指令 → `Store`（写回槽）。
    /// 因此**不再**在这里挑 scratch、也不再在这里做 `settable` 预检（都前移了）。
    ///
    /// `sp_base` 仍由发射侧算：它依赖帧的**发射期**事实（`frame_size`/callee-saved 区/
    /// 栈参数区），分配器看不到。
    #[allow(clippy::too_many_arguments)]
    fn emit_inst_with_spills(
        inst: &mut I,
        encoder: &dyn TargetEncoder<Inst = I>,
        alloc_result: &AllocResult,
        frame_size: u32,
        frame_lowering: &dyn crate::machine::frame::TargetFrameLowering<Inst = I>,
        sink: &mut CodeSink,
        callee_saved_bytes: i32,
        stack_args: i32,
        edits: &[crate::pipeline::alloc_result::SpillEdit],
        fname: &str,
    ) -> Result<(), IrError> {
        if edits.is_empty() {
            return Self::emit_inst(inst, encoder, alloc_result, sink);
        }
        // The spill area lives BELOW the pushed callee-saved registers, whose
        // saved slots occupy [rbp - callee_saved_bytes, rbp):
        //   spill area = [rbp - callee_saved_bytes - frame_size, rbp - callee_saved_bytes)
        // The frame pointer itself sits at [rbp] (pushed before the callee
        // saves), so callee_saved_bytes must NOT include it — otherwise spills
        // land 8 bytes below rsp and can hit an unmapped stack page in release
        // builds (test_spill_high_pressure / e2e_jit_execute hang).
        // 栈参数区（Windows x64 第 5+ 参数）在帧底之上 [rsp, rsp+stack_args)：
        // spill 槽起始必须上移 stack_args，否则与调用方 store 的栈参数重叠
        //（five_args_stack 场景 spill 覆盖已写入的参数槽 → 值错）。
        let sp_base: i32 = -(frame_size as i32) - callee_saved_bytes + stack_args;

        // 1) 装值：把槽里的值装进各自的 scratch（宽度档由 width 选、指令族由 class 选）。
        //    纯 def 的值也装（其值会被指令覆盖，无害，但保持 use/def 语义统一）。
        for e in edits {
            if let SpillEdit::Reload {
                slot,
                scratch,
                class,
                width,
            } = e
            {
                if crate::pipeline::trace_enabled("FORGE_TRACE_SPILL") {
                    eprintln!(
                        "[spill] {fname} load {} width={} off={} class={class:?}",
                        scratch.num,
                        width,
                        sp_base + slot.offset
                    );
                }
                frame_lowering.emit_spill_load(
                    scratch.num,
                    sp_base + slot.offset,
                    *width,
                    *class,
                    sink,
                )?;
            }
        }

        // 2) 改写字段：被溢出的字段改指 scratch（字段号来自编辑序列——用它而不是
        //    enumerate 位置：指令含物理寄存器字段时绑定表会跳过该字段，用位置会错位）。
        for e in edits {
            if let SpillEdit::Bind {
                field,
                scratch,
                class,
            } = e
            {
                inst.set_reg_field(usize::from(*field), scratch.num, *class);
            }
        }

        // 3) 发指令（encode 从指令字段读寄存器，已由上面的 Bind 改写）。
        let result = Self::emit_inst(inst, encoder, alloc_result, sink);

        // 4) 写回槽：spilled def 的结果 store 回它自己的槽。
        for e in edits {
            if let SpillEdit::Store {
                slot,
                scratch,
                class,
                width,
            } = e
            {
                if crate::pipeline::trace_enabled("FORGE_TRACE_SPILL") {
                    eprintln!(
                        "[spill] {fname} store {} width={} off={} class={class:?}",
                        scratch.num,
                        width,
                        sp_base + slot.offset
                    );
                }
                frame_lowering.emit_spill_store(
                    scratch.num,
                    sp_base + slot.offset,
                    *width,
                    *class,
                    sink,
                )?;
            }
        }

        result
    }
}
