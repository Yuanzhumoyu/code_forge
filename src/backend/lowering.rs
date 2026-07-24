//! 函数编译器 — 将 IR Function lowering 为机器码。
//!
//! 完整的编译流水线：
//! 1. 创建 VCode 块映射
//! 2. 分配虚拟寄存器给入口参数
//! 3. 基于模式的指令选择（IR → 机器指令）
//! 4. 寄存器分配
//! 5. 栈帧计算
//! 6. 代码发射

use super::pattern_isel::PatternMatcher;
use super::*;
use crate::{CompileError, CompiledFunction};
use std::collections::HashMap;

/// 函数编译器 — 驱动从 IR 到机器码的完整流程。
pub struct FunctionCompiler<I: InstructionSet> {
    vcode: VCode<I::Inst>,
    value_to_vreg: HashMap<Value, VReg>,
    block_map: HashMap<BlockId, VCodeBlockId>,
    ctx: LowerCtx,
    frame_size: u32,
    param_vregs: Vec<VReg>,
    /// 基于模式的指令选择匹配器。在 lowering 期间用于检测并重写 IR 模式。
    #[allow(dead_code)]
    pattern_matcher: PatternMatcher,
}

impl<I: InstructionSet> Default for FunctionCompiler<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: InstructionSet> FunctionCompiler<I> {
    pub fn new() -> Self {
        let mut matcher = PatternMatcher::new();
        matcher.register_x86_standard_patterns();
        Self {
            vcode: VCode::new(),
            value_to_vreg: HashMap::new(),
            block_map: HashMap::new(),
            ctx: LowerCtx::new(),
            frame_size: 0,
            param_vregs: Vec::new(),
            pattern_matcher: matcher,
        }
    }

    /// 编译一个 IR 函数（使用默认优化管道）。
    ///
    /// 自动运行常量折叠、CSE、死代码消除、复制传播。
    pub fn compile(func: &Function) -> Result<CompiledFunction, CompileError> {
        Self::compile_with_passes(func, &crate::optimize::PassManager::default())
    }

    /// 编译一个已优化的 IR 函数（跳过优化阶段，仅 lowering + 代码发射）。
    ///
    /// 当调用方已通过 Module 或其他方式优化了函数时使用此方法。
    ///
    /// # Example
    ///
    /// ```ignore
    /// module.optimize()?;  // 模块级优化
    /// let code = FunctionCompiler::<MyIsa>::compile_raw(module.get(f)?)?;
    /// ```
    pub fn compile_raw(func: &Function) -> Result<CompiledFunction, CompileError> {
        Self::compile_with_passes(func, &crate::optimize::PassManager::new())
    }

    /// 编译一个 IR 函数（使用自定义优化管道）。
    ///
    /// 如果 `pass_manager` 为空，跳过优化阶段。
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut module = Module::new();
    /// let pm = module.optimization_pipeline();
    /// let code = FunctionCompiler::<MyIsa>::compile_with_passes(&func, &pm)?;
    /// ```
    pub fn compile_with_passes(
        func: &Function,
        pass_manager: &crate::optimize::PassManager,
    ) -> Result<CompiledFunction, CompileError> {
        let mut compiler = Self::new();
        compiler.ctx.call_conv = func.signature.calling_convention;
        compiler.ctx.is_float_return = func
            .signature
            .returns
            .first()
            .map(|t| t.is_float())
            .unwrap_or(false);

        // 克隆并优化 IR
        let mut func = func.clone();
        // 将常量池传递给 lowering 上下文
        compiler.ctx.constant_pool = Some(func.constant_pool.clone());
        if !pass_manager.is_empty() {
            let opt_result = pass_manager.run_on_function(&mut func)?;
            if opt_result.changed {
                log::info!(
                    "Optimized '{}': {} insts removed, {} blocks removed",
                    func.name,
                    opt_result.instructions_removed,
                    opt_result.blocks_removed
                );
            }
        }

        // 验证 IR
        let validation = func.validate();
        if !validation.is_valid() {
            for err in &validation.errors {
                log::error!("IR validation error in '{}': {}", func.name, err);
            }
            return Err(CompileError::Internal(format!(
                "IR validation failed for '{}': {}",
                func.name,
                validation.errors.join("; ")
            )));
        }

        log::info!("Compiling function: {}", func.name);

        // 1. 创建 VCode 基本块映射
        compiler.create_blocks(&func);

        // 2. 分配虚拟寄存器给入口参数
        compiler.alloc_params(&func);

        // 2.5. 预分配 Phi 节点的 VReg
        compiler.pre_allocate_phi_vregs(&func);
        // 2.6. 预分配块参数的 VReg（处理隐式 phi，如 create_block_with_params）
        compiler.pre_allocate_block_param_vregs(&func);

        // 3. 指令选择：遍历每个基本块
        for block in func.iter_blocks() {
            compiler.lower_block(block)?;
        }

        // 3.5. 窥孔优化：VCode 级别指令模式匹配
        let _peephole_count = compiler.peephole_optimize();

        // 4. 寄存器分配
        let mut reg_map = compiler.run_regalloc()?;

        // 5. 计算栈帧大小
        compiler.frame_size = compiler.calculate_frame_size(&reg_map);

        // 6. 代码发射
        let mut sink = CodeSink::new();

        // 6.1 函数序言
        I::emit_prologue(compiler.frame_size, &reg_map, &mut sink);

        // 6.2 发射各基本块
        // scratch 物理寄存器用于 spill load/store (R10, R11)
        const SR1: u8 = 10; // R10
        const SR2: u8 = 11; // R11
        // spill 槽在栈上的 RBP 相对偏移基址
        // 栈布局: [RBP] = saved RBP, [RBP-8..RBP-40] = callee-saved regs (5×8),
        // [RBP-40-frame..RBP-40] = 对齐填充 + spill 区域
        let spill_rbp_base: i32 =
            -(I::callee_save_regs().len() as i32 * 8) - (compiler.frame_size as i32);

        for vblock in compiler.vcode.blocks() {
            sink.bind_label(vblock.ir_block);

            for inst in &vblock.instructions {
                // 收集溢出的 uses 和 defs
                let spilled_uses: Vec<(VReg, i32)> = inst
                    .uses()
                    .iter()
                    .filter_map(|&v| reg_map.spill_offset(v).map(|off| (v, off)))
                    .collect();
                let spilled_defs: Vec<(VReg, i32)> = inst
                    .defs()
                    .iter()
                    .filter_map(|&v| reg_map.spill_offset(v).map(|off| (v, off)))
                    .collect();

                if spilled_uses.is_empty() && spilled_defs.is_empty() {
                    // 快速路径：无溢出操作数
                    I::emit(inst, &reg_map, &mut sink)?;
                } else {
                    // 慢路径：溢出 load → emit → 溢出 store
                    let mut saved: smallvec::SmallVec<[(VReg, Option<PReg>); 4]> =
                        smallvec::SmallVec::new();

                    // 为溢出的 uses 发射 spill load
                    for (i, (vreg, offset)) in spilled_uses.iter().enumerate() {
                        let scratch = if i == 0 { SR1 } else { SR2 };
                        let rbp_off = spill_rbp_base + offset;
                        enc_spill_load(&mut sink, scratch, rbp_off);
                        let old = reg_map
                            .vreg_to_preg
                            .insert(*vreg, PReg::new(scratch, RegClass::Int));
                        saved.push((*vreg, old));
                    }

                    // 为不在 uses 中的溢出 defs 也分配 scratch（纯写操作数）
                    for (vreg, _offset) in &spilled_defs {
                        if !reg_map.vreg_to_preg.contains_key(vreg) {
                            // 使用第一个未被占用的 scratch reg
                            let used_scratch: Vec<u8> = saved
                                .iter()
                                .filter_map(|(_, old)| old.map(|p| p.num))
                                .collect();
                            let scratch = if !used_scratch.contains(&SR1) {
                                SR1
                            } else {
                                SR2
                            };
                            let old = reg_map
                                .vreg_to_preg
                                .insert(*vreg, PReg::new(scratch, RegClass::Int));
                            saved.push((*vreg, old));
                        }
                    }

                    // 发射主指令
                    I::emit(inst, &reg_map, &mut sink)?;

                    // 为溢出的 defs 发射 spill store
                    for (vreg, offset) in &spilled_defs {
                        if let Some(&preg) = reg_map.vreg_to_preg.get(vreg) {
                            let rbp_off = spill_rbp_base + offset;
                            enc_spill_store(&mut sink, preg.num, rbp_off);
                        }
                    }

                    // 恢复 reg_map 原始状态
                    for (vreg, old) in saved {
                        match old {
                            Some(p) => {
                                reg_map.vreg_to_preg.insert(vreg, p);
                            }
                            None => {
                                reg_map.vreg_to_preg.remove(&vreg);
                            }
                        }
                    }
                }
            }

            // 返回块需要跳到 epilogue 做 callee-save 恢复和 RET
            if vblock.is_return_block {
                let epilogue_block = crate::ir::BlockId(0xFFFFFFFD);
                sink.put1(0xE9); // JMP rel32
                let fixup = sink.offset();
                sink.put4(0u32);
                sink.use_label_at(fixup, epilogue_block, crate::RelocKind::REL4);
            }
        }

        // 6.3 函数尾声（绑定 epilogue 标签）
        let epilogue_block = crate::ir::BlockId(0xFFFFFFFD);
        sink.bind_label(epilogue_block);
        I::emit_epilogue(compiler.frame_size, &reg_map, &mut sink);

        // 6.4 提取 ISA 特定 fixup（在 finish 之前，因为 finish 会消耗 sink）
        let isa_fixups = sink.isa_label_fixups();

        let relocations = sink.relocations();
        let code = sink.finish().map_err(CompileError::Emit)?;

        // ISA 特定重定位 (RelocKind::Isa) 由 JIT linker 或 object writer 等
        // 外部消费者负责，不在 compile 阶段处理。
        let _ = isa_fixups;

        Ok(CompiledFunction {
            code_size: code.len(),
            code,
            relocations,
        })
    }

    fn create_blocks(&mut self, func: &Function) {
        for block in func.iter_blocks() {
            let vblock_id = self.vcode.create_block(block.id);
            self.block_map.insert(block.id, vblock_id);
        }
    }

    /// 预分配 Phi 指令的 VReg。
    /// Phi 的 result 和所有 operands 共享同一个 VReg，
    /// 这样 operands 的定义指令会直接写入共享寄存器。
    fn pre_allocate_phi_vregs(&mut self, func: &Function) {
        for block in func.iter_blocks() {
            for inst in &block.instructions {
                if matches!(inst.opcode, Opcode::Phi { .. }) {
                    let vreg = self.ctx.alloc_vreg();
                    if let Some(v) = inst.result {
                        self.value_to_vreg.insert(v, vreg);
                    }
                    for v in &inst.operands {
                        self.value_to_vreg.insert(*v, vreg);
                    }
                }
            }
        }
    }

    /// Pre-allocate VRegs for block parameters that don't have explicit Phi instructions.
    /// Maps predecessor jump/branch argument Values to the same VReg as the block parameter,
    /// ensuring values flow correctly through control flow edges.
    fn pre_allocate_block_param_vregs(&mut self, func: &Function) {
        for block in func.iter_blocks() {
            if block.params.is_empty() {
                continue;
            }
            for pred_block in func.iter_blocks() {
                match &pred_block.terminator {
                    Terminator::Jump { target, args } if *target == block.id => {
                        for (i, (param_val, _param_ty)) in block.params.iter().enumerate() {
                            if let Some(&arg_val) = args.get(i) {
                                let vreg = self.get_or_create_shared_vreg(*param_val, arg_val);
                                self.value_to_vreg.insert(*param_val, vreg);
                                self.value_to_vreg.insert(arg_val, vreg);
                            }
                        }
                    }
                    Terminator::Branch {
                        true_block,
                        false_block,
                        true_args,
                        false_args,
                        ..
                    } => {
                        let args_iter: Vec<&[Value]> = [
                            (*true_block == block.id).then_some(true_args.as_slice()),
                            (*false_block == block.id).then_some(false_args.as_slice()),
                        ]
                        .into_iter()
                        .flatten()
                        .collect();
                        for pred_args in &args_iter {
                            for (i, (param_val, _param_ty)) in block.params.iter().enumerate() {
                                if let Some(&arg_val) = pred_args.get(i) {
                                    let vreg = self.get_or_create_shared_vreg(*param_val, arg_val);
                                    self.value_to_vreg.insert(*param_val, vreg);
                                    self.value_to_vreg.insert(arg_val, vreg);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Get existing VReg from either value, or allocate a new one shared between them.
    fn get_or_create_shared_vreg(&mut self, a: Value, b: Value) -> VReg {
        if let Some(&vreg) = self.value_to_vreg.get(&a) {
            vreg
        } else if let Some(&vreg) = self.value_to_vreg.get(&b) {
            vreg
        } else {
            self.ctx.alloc_vreg()
        }
    }

    /// Get or allocate a VReg for a Value (avoids closure borrow issues).
    fn get_or_alloc_vreg(&mut self, val: Value) -> VReg {
        if let Some(&vreg) = self.value_to_vreg.get(&val) {
            vreg
        } else {
            let vreg = self.ctx.alloc_vreg();
            self.value_to_vreg.insert(val, vreg);
            vreg
        }
    }

    /// Get or allocate a VReg for an optional result Value.
    fn get_or_alloc_vreg_for_result(&mut self, result: Option<Value>) -> VReg {
        match result {
            Some(v) => self.get_or_alloc_vreg(v),
            None => self.ctx.alloc_vreg(),
        }
    }

    fn alloc_params(&mut self, func: &Function) {
        if let Some(entry) = func.entry_block() {
            for (_i, (param_val, param_ty)) in entry.params.iter().enumerate() {
                let class = if param_ty.is_float() {
                    RegClass::Float
                } else {
                    RegClass::Int
                };
                let vreg = self.ctx.alloc_vreg_with_class(class);
                self.value_to_vreg.insert(*param_val, vreg);
                self.param_vregs.push(vreg);
                // Note: actual ABI arg_reg → vreg copy is done by @move_args
                // in the prologue (see gen_move_args / emit_prologue_impl).
                // Reg type operands bypass regalloc and emit physical register numbers directly.
            }
        }
    }

    fn lower_block(&mut self, block: &Block) -> Result<(), CompileError> {
        let vblock_id = self.block_map[&block.id];
        self.vcode.switch_to_block(vblock_id);

        // 为基本块参数分配 vreg（仅当尚未映射时）
        // 入口块的参数已在 alloc_params 中映射，不应覆盖
        for (param_val, param_ty) in &block.params {
            if !self.value_to_vreg.contains_key(param_val) {
                let class = if param_ty.is_float() {
                    RegClass::Float
                } else {
                    RegClass::Int
                };
                let vreg = self.ctx.alloc_vreg_with_class(class);
                self.value_to_vreg.insert(*param_val, vreg);
            }
        }

        // Phase 1: find and validate lea-merge patterns.
        // TODO: When constants are provided, LEA pattern matches correctly detect scale values
        // but the LEA emission (Phase 2) creates new vregs for Iconst values that haven't been
        // lowered yet, causing uninitialized register reads. Fix: lower Iconst values before
        // LEA emission and reuse their result vregs, or move LEA emission after Iconst lowering.
        let constants_clone: Option<Vec<crate::ir::Big>> = self
            .ctx
            .constant_pool
            .as_ref()
            .map(|p| p.constants().to_vec());
        let lea_merges: Vec<(usize, Option<u8>)> = {
            // Use cloned constants to avoid borrow conflicts; set to None until vreg bug is fixed.
            let _constants_clone = constants_clone;
            let constants: Option<&[crate::ir::Big]> = None; // FIXME: use _constants_clone.as_deref() after vreg fix
            let matches = self
                .pattern_matcher
                .find_all_matches(&block.instructions, constants);
            let mut merges = Vec::new();
            for m in &matches {
                if m.pattern_name == "lea-merge-iadd-imul" && m.pos + 1 < block.instructions.len() {
                    let imul_inst = &block.instructions[m.pos];
                    let scale = imul_inst.operands.get(1).and_then(|sv| {
                        block.instructions[..m.pos].iter().find_map(|prev| {
                            if prev.result == Some(*sv) {
                                if let Opcode::Iconst { index } = prev.opcode {
                                    constants
                                        .and_then(|p| p.get(index as usize))
                                        .and_then(|b| b.try_to_i64())
                                        .and_then(|s| {
                                            if matches!(s, 1 | 2 | 4 | 8) {
                                                Some(s as u8)
                                            } else {
                                                None
                                            }
                                        })
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                    });
                    merges.push((m.pos, scale));
                }
            }
            merges
        };

        // Phase 2: emit optimized LEA for validated patterns
        let mut skip_set: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for &(pos, scale_opt) in &lea_merges {
            if let Some(scale) = scale_opt {
                let imul_inst = &block.instructions[pos];
                let iadd_inst = &block.instructions[pos + 1];
                let pname = match scale {
                    1 => "lea-merge-iadd-imul-1",
                    2 => "lea-merge-iadd-imul-2",
                    4 => "lea-merge-iadd-imul-4",
                    _ => "lea-merge-iadd-imul-8",
                };
                let index_val = imul_inst.operands[0];
                let base_val = iadd_inst.operands[1];
                let index_vreg = self.get_or_alloc_vreg(index_val);
                let base_vreg = self.get_or_alloc_vreg(base_val);
                let dest_vreg = self.get_or_alloc_vreg_for_result(iadd_inst.result);
                if let Some(r) = imul_inst.result {
                    self.value_to_vreg.insert(r, dest_vreg);
                }
                let pattern_insts = I::lower_pattern(
                    pname,
                    &[imul_inst.clone(), iadd_inst.clone()],
                    &[base_vreg, index_vreg],
                    Some(dest_vreg),
                    &mut self.ctx,
                )?;
                if !pattern_insts.is_empty() {
                    for mi in pattern_insts {
                        self.vcode.push_inst(mi);
                    }
                    skip_set.insert(pos);
                    skip_set.insert(pos + 1);
                }
            }
        }

        // Lower 普通指令
        let mut inst_idx = 0;
        while inst_idx < block.instructions.len() {
            if skip_set.contains(&inst_idx) {
                inst_idx += 1;
                continue;
            }

            let inst = &block.instructions[inst_idx];
            let args: Vec<VReg> = inst
                .operands
                .iter()
                .map(|v| {
                    self.value_to_vreg.get(v).copied().unwrap_or_else(|| {
                        // 如果尚未映射，动态分配
                        let vreg = self.ctx.alloc_vreg();
                        self.value_to_vreg.insert(*v, vreg);
                        vreg
                    })
                })
                .collect();
            let result = if matches!(inst.opcode, Opcode::Phi { .. }) {
                // Phi 节点：分配新 VReg，并将 result 映射到它
                let vreg = inst
                    .operands
                    .first()
                    .and_then(|v| self.value_to_vreg.get(v).copied())
                    .unwrap_or_else(|| {
                        let class = if inst.ty.is_float() {
                            RegClass::Float
                        } else {
                            RegClass::Int
                        };
                        self.ctx.alloc_vreg_with_class(class)
                    });
                if let Some(v) = inst.result {
                    self.value_to_vreg.insert(v, vreg);
                }
                for v in &inst.operands {
                    self.value_to_vreg.insert(*v, vreg);
                }
                Some(vreg)
            } else {
                inst.result.map(|v| {
                    self.value_to_vreg.get(&v).copied().unwrap_or_else(|| {
                        let class = if inst.ty.is_float() {
                            RegClass::Float
                        } else {
                            RegClass::Int
                        };
                        let vreg = self.ctx.alloc_vreg_with_class(class);
                        self.value_to_vreg.insert(v, vreg);
                        vreg
                    })
                })
            };

            if matches!(inst.opcode, Opcode::Phi { .. }) {
                inst_idx += 1;
                continue; // Phi 不生成机器指令
            }

            // 从 IR 指令类型推导操作数宽度，驱动宽度感知指令选择
            self.ctx.default_opsize = LowerCtx::opsize_from_type(&inst.ty);

            let machine_insts = I::lower(&inst.opcode, &args, result, &mut self.ctx)?;
            for mi in machine_insts {
                self.vcode.push_inst(mi);
            }
            inst_idx += 1;
        }

        // Lower 终止指令
        let term_insts = I::lower_terminator(
            &block.terminator,
            &self.value_to_vreg,
            &self.block_map,
            &mut self.ctx,
        )?;
        for mi in term_insts {
            self.vcode.push_inst(mi);
        }
        // Mark return blocks for epilogue routing
        if matches!(block.terminator, Terminator::Return { .. })
            && let Some(vb) = self.vcode.block_mut(vblock_id)
        {
            vb.is_return_block = true;
        }

        Ok(())
    }

    /// 窥孔优化：对每个 VCode 基本块运行 ISA 特定的窥孔优化。
    fn peephole_optimize(&mut self) -> usize {
        let mut total = 0;
        for block in self.vcode.blocks_mut() {
            total += I::peephole_optimize(&mut block.instructions);
        }
        total
    }

    fn calculate_frame_size(&self, reg_map: &RegMap) -> u32 {
        let mut size = 0u32;

        // 溢出槽区域
        if !reg_map.spills.is_empty() {
            size += reg_map.spills.len() as u32 * 8;
        }

        // 序言 push 已占用的字节数 (RBP + callee-saved regs × 8)
        // frame_size 需要使总偏移保持栈对齐
        let pushed_bytes = 8 + I::callee_save_regs().len() as u32 * 8;
        let total = pushed_bytes + size;
        let align = I::stack_align();
        let aligned_total = total.div_ceil(align) * align;
        let frame = aligned_total - pushed_bytes;

        // 红区：如果栈帧完全在红区内且无溢出槽，帧大小可为零
        if frame > 0
            && let Some(red_zone) = I::red_zone()
            && frame <= red_zone
            && reg_map.spills.is_empty()
        {
            return 0; // 叶函数，栈帧在红区内
        }

        frame
    }

    fn run_regalloc(&mut self) -> Result<RegMap, CompileError> {
        let sp_idx = match I::sp_reg() {
            FrameAccess::Register(r) => r.to_index(),
            _ => I::num_gp_regs(),
        };
        let fp_idx = I::fp_reg().register_index();
        let callee_save: Vec<u8> = I::callee_save_regs().iter().map(|r| r.to_index()).collect();
        let precolored = I::precolored_vregs();
        let mut reg_map = crate::regalloc_adapter::allocate::<I::Inst>(
            &self.vcode,
            I::num_gp_regs(),
            I::num_fp_regs(),
            sp_idx,
            fp_idx,
            &self.ctx.vreg_classes,
            &callee_save,
            &self.param_vregs,
            &precolored,
        )?;
        // 将参数 VReg 列表传递给 RegMap，供 prologue 使用
        reg_map.param_vregs = std::mem::take(&mut self.param_vregs);
        Ok(reg_map)
    }
}

// ============================================================
// Switch 跳转表分析 — 供后端 lowering 使用
// ============================================================

/// Switch 语句的 lowering 策略。
#[derive(Clone, Debug)]
pub enum SwitchStrategy {
    /// 使用 if-else 链（适合稀疏 case）。
    IfElse,
    /// 使用跳转表（适合密集 case）。
    /// `min_value` 是 case 的最小值，`targets` 按顺序对应 min_value..=max_value。
    JumpTable {
        min_value: i64,
        targets: Vec<BlockId>,
        default_block: BlockId,
    },
}

/// 分析 Switch 语句的 case 密集度，返回推荐的 lowering 策略。
///
/// 如果 case 值覆盖了连续范围内 >50% 的整数，推荐跳转表；
/// 否则推荐 if-else 链。
///
/// # 参数
/// - `min_ratio`: 跳转表阈值（0.0-1.0），case 覆盖率 >= 此值时使用跳转表。默认 0.5。
pub fn analyze_switch(
    cases: &[(i64, BlockId, smallvec::SmallVec<[Value; 2]>)],
    default_block: BlockId,
    min_ratio: f64,
) -> SwitchStrategy {
    if cases.is_empty() {
        return SwitchStrategy::IfElse;
    }

    let min_val = cases.iter().map(|(v, _, _)| *v).min().unwrap();
    let max_val = cases.iter().map(|(v, _, _)| *v).max().unwrap();
    let range = (max_val - min_val) as u64;

    // 避免过大的跳转表（超过 256 个条目）
    if range > 256 {
        return SwitchStrategy::IfElse;
    }

    let density = cases.len() as f64 / (range + 1) as f64;
    if density >= min_ratio && range >= 2 {
        // 构建跳转表：min_val 到 max_val 的每个位置
        let size = (range + 1) as usize;
        let mut targets = vec![default_block; size];
        for &(val, target, _) in cases {
            targets[(val - min_val) as usize] = target;
        }
        SwitchStrategy::JumpTable {
            min_value: min_val,
            targets,
            default_block,
        }
    } else {
        SwitchStrategy::IfElse
    }
}

#[cfg(test)]
mod switch_strategy_tests {
    use super::*;

    #[test]
    fn dense_switch_uses_jump_table() {
        let cases = vec![
            (0i64, BlockId(1), smallvec::smallvec![]),
            (1i64, BlockId(2), smallvec::smallvec![]),
            (2i64, BlockId(3), smallvec::smallvec![]),
            (3i64, BlockId(4), smallvec::smallvec![]),
        ];
        let result = analyze_switch(&cases, BlockId(0), 0.5);
        assert!(matches!(result, SwitchStrategy::JumpTable { .. }));
    }

    #[test]
    fn sparse_switch_uses_if_else() {
        let cases = vec![
            (0i64, BlockId(1), smallvec::smallvec![]),
            (100i64, BlockId(2), smallvec::smallvec![]),
        ];
        let result = analyze_switch(&cases, BlockId(0), 0.5);
        assert!(matches!(result, SwitchStrategy::IfElse));
    }

    #[test]
    fn empty_switch_uses_if_else() {
        let cases: Vec<(i64, BlockId, smallvec::SmallVec<[Value; 2]>)> = vec![];
        let result = analyze_switch(&cases, BlockId(0), 0.5);
        assert!(matches!(result, SwitchStrategy::IfElse));
    }

    #[test]
    fn large_range_falls_back_to_if_else() {
        let cases = vec![
            (0i64, BlockId(1), smallvec::smallvec![]),
            (500i64, BlockId(2), smallvec::smallvec![]),
        ];
        let result = analyze_switch(&cases, BlockId(0), 0.5);
        assert!(matches!(result, SwitchStrategy::IfElse));
    }
}
