//! Stage 1-5 of the compilation pipeline: block mapping, VReg pre-allocation,
//! instruction selection and peephole optimization.
//!
//! These methods operate on [`super::compiler::CompileState`] and are split out
//! of `compiler.rs` to keep each pipeline stage's responsibilities in one file.

use crate::LowerCtx;
use crate::machine::lowering::TargetLowering;
use crate::machine::peephole::TargetPeephole;
use crate::machine::target::TargetMachine;
use crate::pipeline::compiler::{CompileState, atomic_op_from_u64};
use forge_ir::*;

impl<I: crate::machine::inst::MachineInst + 'static> CompileState<I> {
    // ── Stage 1: Block Mapping ──

    pub(crate) fn create_blocks(&mut self, func: &Function) {
        for (i, _) in func.dfg.blocks.iter().enumerate() {
            let block_id = Block(i as u32);
            let vblock_id = self.vcode.create_block(block_id);
            self.block_map.insert(block_id, vblock_id);
        }
    }

    // ── Stage 2: VReg Pre-allocation ──

    pub(crate) fn alloc_params(&mut self, func: &Function) {
        if let Some(entry_id) = func.entry_block {
            let entry = func.dfg.block(entry_id);
            let param_values = func.dfg.block_param_values(entry_id);
            for (i, param_ty) in entry.params.iter().enumerate() {
                // 按类型分派到多宽度类（I32→GPR(4) 池、F32/F64→FPR(8) 池、
                // 动态 vector → 位宽感知 VEC）
                let class = self.ctx.reg_class_for(param_ty);
                let xreg = self.ctx.alloc_xreg_with_width(class);
                self.ctx.xreg_types.insert(xreg, *param_ty);

                self.param_xregs.push(xreg);
                if let Some(&param_val) = param_values.get(i) {
                    self.value_to_xreg.insert(param_val, xreg);
                }
            }
        }
    }

    pub(crate) fn pre_allocate_phi_vregs(&mut self, func: &Function) {
        for (block, _) in func.dfg.blocks() {
            for inst in func.dfg.block_inst_iter(block) {
                if matches!(inst.opcode, Opcode::Copy) {
                    // operands 已有映射（参数/收参寄存器）时复用——否则（phi
                    // 回填路径）分配新寄存器并把 operands 一起映射（Copy 无
                    // 机器指令——同寄存器语义）。分配类按 result 类型（f64
                    // 位模式链——bitcast 结果 → Copy → 下游 Freg 字段——用
                    // GPR64 会与 FPR 物理编码别名冲突：r15 与 xmm15 的 reg
                    // 位相同，regalloc 跨池无法检测别名）。
                    let class = inst
                        .results
                        .first()
                        .and_then(|v| func.dfg.value_type(*v))
                        .map(|t| {
                            if self.ctx.reg_class_for(&t).is_fp() {
                                RegClass::FPR64
                            } else {
                                RegClass::GPR64
                            }
                        })
                        .unwrap_or(RegClass::GPR64);
                    let xreg = inst
                        .operands
                        .first()
                        .and_then(|v| self.value_to_xreg.get(v).copied())
                        .unwrap_or_else(|| self.ctx.alloc_xreg(class));
                    if let Some(v) = inst.results.first().copied() {
                        let ty = func.dfg.value_type(v).unwrap_or(TypeId::VOID);
                        self.value_to_xreg.insert(v, xreg);
                        self.ctx.xreg_types.insert(xreg, ty);
                    }
                    for v in &inst.operands {
                        let ty = func.dfg.value_type(*v).unwrap_or(TypeId::VOID);
                        self.value_to_xreg.insert(*v, xreg);
                        self.ctx.xreg_types.insert(xreg, ty);
                    }
                }
            }
        }
    }

    pub(crate) fn pre_allocate_block_param_xregs(&mut self, func: &Function) {
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
            let mut param_xregs: Vec<XReg> = info
                .param_tys
                .iter()
                .map(|ty| {
                    let class = self.ctx.reg_class_for(ty);
                    let xreg = self.ctx.alloc_xreg(class);
                    self.ctx.xreg_types.insert(xreg, *ty);
                    xreg
                })
                .collect();

            // 先处理所有 pred 的传参映射（map_terminator 可能复用 arg 的
            // 已有寄存器到 param_xregs，如循环中块参数 value 作为跳转 arg）
            if let Some(pred_list) = preds.get(&info.block) {
                for pred in pred_list {
                    let term = func.dfg.block_terminator(*pred);
                    self.map_terminator_args_to_params(
                        term,
                        info.block,
                        &mut param_xregs,
                        &info.param_tys,
                    );
                }
            }

            // 最后：块参数 value 映射到（复用后的）param_xregs——必须与
            // 传参寄存器一致，否则目标块读取参数时用旧寄存器拿到错误值
            // （write_bytes 循环 body 的 bi 读到 dst 的寄存器，[WA-14]）。
            // 注：多 pred 且各 pred 的 arg 寄存器不同时，param_xregs 取
            // 最后处理的 pred（预先存在的限制，单 pred 循环为实际修复点）。
            for (i, &pv) in info.param_values.iter().enumerate() {
                if i < param_xregs.len() {
                    self.value_to_xreg.insert(pv, param_xregs[i]);
                }
            }
        }
    }

    pub(crate) fn map_terminator_args_to_params(
        &mut self,
        term: Option<&Terminator>,
        target: Block,
        param_xregs: &mut [XReg],
        param_tys: &[TypeId],
    ) {
        // 统一经 Terminator::args_to 提取传向 target 的参数（Jump/Branch/
        // Switch 全覆盖；原实现漏了 Switch 分支）。
        if let Some(t) = term {
            let args = t.args_to(target);
            for (i, &arg) in args.iter().enumerate() {
                if i < param_xregs.len() {
                    // 传参寄存器复用：若 arg 已有映射（典型场景：循环中块参数
                    // value 作为跳转 arg，如 loop 的 i 传给 body），直接复用其
                    // 寄存器——避免 insert 覆盖导致源块对该 value 的读取错位
                    // （write_bytes 内联循环的 count/循环变量寄存器被覆盖 bug，
                    // [WA-14]）。arg 未映射时保持 param_xregs 原值（行为不变）。
                    if let Some(&existing) = self.value_to_xreg.get(&arg) {
                        param_xregs[i] = existing;
                    }
                    self.value_to_xreg.insert(arg, param_xregs[i]);
                    let ty = param_tys.get(i).copied().unwrap_or(TypeId::VOID);
                    self.ctx.xreg_types.insert(param_xregs[i], ty);
                }
            }
        }
    }

    // ── Stage 4: Instruction Selection ──

    /// Stage 3: IR 层模式融合（pattern isel）——按 ISA 门控的
    /// PatternMatcher 就地重写匹配序列（Imul+Iadd→LEA 等）。
    /// 消费指令标 Nop（不发射），代表指令带 isel_strategy 标签。
    /// 基于模式的优化 lowering（pattern_isel 匹配后调用）。
    pub(crate) fn run_pattern_matching<M: TargetMachine>(func: &mut Function, machine: &M) {
        let Some(matcher) = machine.pattern_matcher() else {
            return;
        };
        if matcher.pattern_count() == 0 {
            return;
        }
        let total = 0;
        for block_data in func.dfg.blocks.iter_mut() {
            let order = block_data.inst_order.clone();
            if order.is_empty() {
                continue;
            }
            // 收集该 block 的指令（clone 后重写）——第三十五轮实验:禁用写回
            let mut insts: Vec<Instruction> = order
                .iter()
                .map(|&i| func.dfg.insts[i.0 as usize].clone())
                .collect();
            let _ = matcher.apply(&mut insts, None);
        }
        if total > 0 && crate::pipeline::trace_enabled("FORGE_TRACE_ISEL") {
            eprintln!("[isel] pattern-matched {total} sequences in {}", func.name);
        }
    }

    pub(crate) fn lower_all_blocks(
        &mut self,
        func: &Function,
        lowering: &dyn TargetLowering<Inst = I>,
    ) -> Result<(), IrError> {
        // 预扫描：StackAddr 最大槽深 + Alloca 帧槽分配。
        // Alloca 槽从 StackAddr 区之下（更负）连续分配（8 字节对齐），避免与
        // 前段固定的 StackAddr 偏移（Immediate::Int，-4/-8...）重叠。
        let mut stackaddr_depth: i64 = 0;
        let mut allocas: Vec<(Inst, u32)> = Vec::new(); // (指令, 槽字节数)
        // 预扫描第二遍：识别 `Iadd(stack_addr(0), iconst(-N))` 模式——mini_c 的
        // alloc_slot 生成 `stack_addr(0) + iadd(iconst(offset))`,StackAddr 本身
        // immediate=0 不贡献深度,真正的槽偏移在 iconst 里。若不把这些负偏移
        // 计入 stackaddr_depth,locals 区不参与 frame 计算,spill 槽会从更浅的
        // 位置分配并覆盖局部变量(嵌套循环 s 累加丢失/死循环的根因)。
        let dfg = &func.dfg;
        let mut iadd_stack_offsets: Vec<i64> = Vec::new();
        for (_, bd) in dfg.blocks() {
            for &ii in &bd.inst_order {
                let inst = &dfg.insts[ii.0 as usize];
                if inst.opcode != Opcode::Iadd {
                    continue;
                }
                // Iadd 的操作数之一必须是 StackAddr 的值,另一个是负 Iconst。
                let mut has_stack = false;
                let mut const_val: i64 = 0;
                let mut has_const = false;
                for &op in &inst.operands {
                    let Some(val) = dfg.values.get(op.0 as usize) else {
                        continue;
                    };
                    let forge_ir::dfg::ValueDef::Inst(def_ii, _) = val.def else {
                        continue;
                    };
                    let def = &dfg.insts[def_ii.0 as usize];
                    match def.opcode {
                        Opcode::StackAddr => has_stack = true,
                        Opcode::Iconst => {
                            if let Some(Immediate::Const(cid)) = def.immediates.first()
                                && let Some((v, _)) = self
                                    .ctx
                                    .constant_pool
                                    .as_ref()
                                    .and_then(|cp| cp.get_int(*cid))
                            {
                                const_val = v as i64;
                                has_const = true;
                            }
                        }
                        _ => {}
                    }
                }
                if has_stack && has_const && const_val < 0 {
                    iadd_stack_offsets.push(const_val);
                }
            }
        }
        for v in iadd_stack_offsets {
            let depth = -v + 8;
            stackaddr_depth = stackaddr_depth.max(depth);
            // 与主循环 StackAddr immediate 的处理一致：负偏移槽深计入 locals
            // 帧需求（否则 spill 槽从过浅位置分配覆盖局部变量）。
            self.ctx.max_stack_bytes = self.ctx.max_stack_bytes.max(depth as u32);
        }
        for (_, bd) in func.dfg.blocks() {
            for &ii in &bd.inst_order {
                let inst = &func.dfg.insts[ii.0 as usize];
                match inst.opcode {
                    Opcode::StackAddr => {
                        if let Some(Immediate::Int(v)) = inst.immediates.first() {
                            let depth = if *v >= 0 { 0 } else { -*v + 8 };
                            stackaddr_depth = stackaddr_depth.max(depth);
                        }
                    }
                    Opcode::Alloca => {
                        let mut ty = None;
                        let mut count = 1u64;
                        for imm in &inst.immediates {
                            match imm {
                                Immediate::Type(t) => ty = Some(*t),
                                Immediate::Uint(c) => count = (*c).max(1),
                                _ => {}
                            }
                        }
                        if let Some(t) = ty {
                            let size = self
                                .ctx
                                .type_ctx
                                .as_ref()
                                .map(|tc| tc.borrow().size_bytes(t))
                                .unwrap_or(8)
                                .max(1) as u64;
                            let bytes = (size * count) as u32;
                            allocas.push((ii, bytes));
                        }
                    }
                    _ => {}
                }
            }
        }
        // 槽偏移从 -(stackaddr 区底 + 8) 起递减；总帧需求并入 max_stack_bytes。
        let has_allocas = !allocas.is_empty();
        let mut slot = -(stackaddr_depth + 8);
        for (ii, bytes) in allocas {
            self.alloca_offsets.insert(ii, slot);
            slot -= (((bytes as i64) + 7) / 8) * 8; // 8 字节对齐（stable 算术）
        }
        if has_allocas {
            let alloca_region = (-slot - stackaddr_depth - 8) as u32;
            self.ctx.max_stack_bytes = self.ctx.max_stack_bytes.max(alloca_region);
        }

        for (i, block_data) in func.dfg.blocks.iter().enumerate() {
            self.lower_block(Block(i as u32), block_data, &func.dfg, lowering)?;
        }
        Ok(())
    }

    pub(crate) fn lower_block(
        &mut self,
        block: Block,
        block_data: &BlockData,
        dfg: &DataFlowGraph,
        lowering: &dyn TargetLowering<Inst = I>,
    ) -> Result<(), IrError> {
        let vblock_id = self.block_map[&block];
        self.vcode.switch_to_block(vblock_id);

        // block_inst_iter 返回借用迭代器（dfg 是函数参数，与 self 的 &mut 借用
        // 不冲突）——原实现 cloned().collect() 每块深克隆整块指令，纯浪费。
        for &ii in &block_data.inst_order {
            let inst = &dfg.insts[ii.0 as usize];
            let args: smallvec::SmallVec<[XReg; 8]> = inst
                .operands
                .iter()
                .map(|v| {
                    let ty = dfg.value_type(*v).unwrap_or(TypeId::VOID);
                    // 与 results/参数分派一致：按类型映射多宽度类（I32→GPR(4)，
                    // 动态 vector → VEC），避免同一 value 因首次出现上下文不同
                    // 而落入不同宽度类。
                    let class = self.ctx.reg_class_for(&ty);
                    self.get_or_alloc_xreg(*v, class)
                })
                .collect();

            let result = if matches!(inst.opcode, Opcode::Copy) {
                // Phi/Copy 不生成机器指令（同寄存器语义）：result 必须指向
                // 操作数的 XReg 并写回映射——pre_allocate_phi_vregs 可能已把
                // result 映射到独立新寄存器（位模式转换链——bitcast 结果 → Copy
                // → 下游消费），不写回会导致下游读到空寄存器（浮点字段 ABI bug）。
                let x = inst
                    .operands
                    .first()
                    .and_then(|v| self.value_to_xreg.get(v).copied())
                    .or_else(|| Some(self.ctx.alloc_xreg(RegClass::GPR64)));
                if let (Some(xr), Some(r)) = (x, inst.results.first()) {
                    if crate::pipeline::trace_enabled("FORGE_TRACE_LOWER") {
                        eprintln!(
                            "[copy] result={r:?} -> xreg={xr:?} op0={:?}",
                            inst.operands.first()
                        );
                    }
                    self.value_to_xreg.insert(*r, xr);
                }
                x
            } else {
                inst.results.first().copied().map(|v| {
                    self.value_to_xreg.get(&v).copied().unwrap_or_else(|| {
                        let result_ty = dfg.value_type(v).unwrap_or(TypeId::VOID);
                        let class = if self.ctx.reg_class_for(&result_ty).is_fp() {
                            RegClass::FPR64
                        } else {
                            RegClass::GPR64
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

            let result_ty = inst
                .results
                .first()
                .and_then(|v| dfg.value_type(*v))
                // 无结果指令（如 Store）用第一个操作数（被存的值）推断宽度，
                // 否则默认 64 会让 i32 store 写 8 字节，覆盖相邻局部变量槽。
                .or_else(|| inst.operands.first().and_then(|v| dfg.value_type(*v)));
            self.ctx.default_opsize = match result_ty {
                Some(ty) => LowerCtx::opsize_from_type(&ty),
                None => 64,
            };
            // Load/Store 用真实内存宽度：opsize_from_type 对 <4 字节类型返回 32
            // 是寄存器安全折中（窄类型算术用 32 位避免残留高位），但 load/store
            // 按 32 位会越界读写内存——u8 元素读/写 4 字节（write_bytes 循环写
            // 8 次 × 4 字节破坏相邻槽；buf[0] 读 4 字节得到 0xABABABAB 垃圾）。
            // Load 用结果类型（读入宽度）、Store 用被存值（操作数 0）宽度。
            // mem_opsize_for：聚合类型（bits()=0）走 TypeStore::size_bytes，
            // 否则 {i32,i32} store 宽度为 0（机器码缺陷 → 执行 SEGV）。
            if matches!(inst.opcode, Opcode::Load) {
                if let Some(ty) = inst.results.first().and_then(|v| dfg.value_type(*v)) {
                    self.ctx.default_opsize = self.ctx.mem_opsize_for(&ty);
                }
            } else if matches!(inst.opcode, Opcode::Store)
                && let Some(v) = inst.operands.first()
                && let Some(ty) = dfg.value_type(*v)
            {
                self.ctx.default_opsize = self.ctx.mem_opsize_for(&ty);
            }
            // Icmp 的比较宽度取操作数（bool 结果会让 cmp/setcc 用 32 位比较
            // 64 位值——高位被截断，如 usize 比较 z=0x1FFFFFFFFFFFFFFF 时
            // 低 32 位 0xFFFFFFFF 被当 -1）。
            if matches!(inst.opcode, Opcode::Icmp { .. })
                && let Some(op0) = inst.operands.first()
                && let Some(oty) = dfg.value_type(*op0)
            {
                self.ctx.default_opsize = LowerCtx::opsize_from_type(&oty);
            }
            // ExtractValue/InsertValue：字段操作在聚合的 64 位域进行——
            // `mov rd, rs1` 必须完整拷贝（否则 32 位截断只剩字段 0）、
            // `shr/shl` 必须 64 位（32 位 shr 移位量 ≥32 时结果 0）。
            // default_opsize 取 rs1（聚合）的字节宽度。
            if matches!(inst.opcode, Opcode::ExtractValue | Opcode::InsertValue)
                && let Some(op0) = inst.operands.first()
                && let Some(oty) = dfg.value_type(*op0)
            {
                self.ctx.default_opsize = self.ctx.mem_opsize_for(&oty);
            }
            // Call/CallIndirect：返回 mov（`MOV_RM8_R64 rd, RAX`）的宽度取结果
            // 类型——聚合结果 bits()=0 会错成 32 位截断（只留字段 0）；用
            // size_bytes（{i32,i32} → 64 位完整拷贝）。
            if matches!(inst.opcode, Opcode::Call | Opcode::CallIndirect)
                && let Some(rt) = inst.results.first().and_then(|v| dfg.value_type(*v))
            {
                self.ctx.default_opsize = self.ctx.mem_opsize_for(&rt);
            }
            // Alloca：帧槽偏移注入（预扫描分配；`lea_off rd, alloca_offset` 规则
            // 取用；平移 callee-saved 区与 StackAddr 的 current_offset 一致——
            // lea 基准 rbp - callee_saved_bytes）。
            if matches!(inst.opcode, Opcode::Alloca) {
                let off = self.alloca_offsets.get(&ii).copied().unwrap_or(-8);
                self.ctx.current_alloca_offset = off - self.ctx.stack_slot_shift as i64;
            }

            if let Some(Immediate::Const(cid)) = inst.immediates.first() {
                self.ctx.current_const_index = cid.0;
            }
            // 全量 immediates 缓存：Uint/Int/Const 提取为 u64 值列表
            //（ShuffleVector 的 mask、Vextract/Vinsert 的 index 等）。
            self.ctx.current_immediates = inst
                .immediates
                .iter()
                .map(|imm| match imm {
                    Immediate::Uint(v) => *v,
                    Immediate::Int(v) => *v as u64,
                    Immediate::Const(c) => c.0 as u64,
                    Immediate::Global(g) => g.0 as u64,
                    Immediate::Func(f) => f.0 as u64,
                    _ => 0,
                })
                .collect();
            if let Some(Immediate::Func(f)) = inst.immediates.first() {
                self.ctx.current_func_ref = Some(*f);
            }
            // Vextract/Vinsert：idx 从操作数注入（LLVM idx 是 i32 操作数——
            // 常量折叠为立即数供 target 模板使用；变量 idx 折叠为 0）
            if matches!(inst.opcode, Opcode::Vextract | Opcode::Vinsert)
                && let Some(&idx_v) =
                    inst.operands
                        .get(if inst.opcode == Opcode::Vinsert { 2 } else { 1 })
            {
                let fold_idx = (|| {
                    let def = dfg.values.get(idx_v.0 as usize)?.def;
                    let forge_ir::dfg::ValueDef::Inst(ii, _) = def else {
                        return None;
                    };
                    let id = dfg.insts.get(ii.0 as usize)?;
                    if id.opcode != forge_ir::opcode::Opcode::Iconst {
                        return None;
                    }
                    let im = id.immediates.first()?;
                    match im {
                        Immediate::Uint(v) => Some(*v),
                        Immediate::Int(v) => Some(*v as u64),
                        Immediate::Const(c) => self
                            .ctx
                            .constant_pool
                            .as_ref()
                            .and_then(|cp| cp.get_int(*c))
                            .map(|(v, _)| v as u64),
                        _ => None,
                    }
                })();
                self.ctx.current_immediates.insert(0, fold_idx.unwrap_or(0));
                if crate::pipeline::trace_enabled("FORGE_TRACE_LOWER") {
                    let dbg = dfg.values.get(idx_v.0 as usize).map(|v| &v.def);
                    let idbg = dfg.insts.get(1).map(|d| (d.opcode, &d.immediates));
                    eprintln!(
                        "[vextract] op={:?} idx_operand={:?} def={:?} inst1={:?} folded={:?}",
                        inst.opcode, idx_v, dbg, idbg, fold_idx
                    );
                }
            }
            if let Some(Immediate::Global(g)) = inst.immediates.first() {
                self.ctx.current_global = Some(*g);
            }
            if let Some(Immediate::Int(v)) = inst.immediates.first() {
                // StackAddr 偏移平移：lea 基准从 rbp 改为 rbp - 栈槽平移
                //（stack_slot_shift：x86 = callee_saved_bytes；riscv =
                // fp_push_bytes），避免局部变量写在 callee-saved push 槽上
                //（覆盖调用者寄存器保存值）。
                // v 是负数（-4, -8, ...），槽深 = -v + 槽宽（8 字节对齐）；记录
                // 最大需求供 calculate_frame_size 分配（否则槽落在 rsp 之下）。
                if crate::pipeline::trace_enabled("FORGE_TRACE_STACK") {
                    eprintln!(
                        "[forge] StackAddr v={v} shift={} -> lea rbp{:+}",
                        self.ctx.stack_slot_shift,
                        v - self.ctx.stack_slot_shift as i64
                    );
                }
                self.ctx.current_offset = *v - self.ctx.stack_slot_shift as i64;
                // 正偏移（rbp 上方）不是本函数局部变量槽，不参与 locals 帧计算；
                // 负偏移槽深 = -v + 槽宽（8 字节对齐）。直接 (-v as u32) 对正偏移
                // 会下溢成巨大值导致帧大小溢出崩溃（io_stack_addr_distinct_offsets）。
                let depth = if *v >= 0 {
                    0
                } else {
                    (-*v as u32).saturating_add(8)
                };
                self.ctx.max_stack_bytes = self.ctx.max_stack_bytes.max(depth);
            }
            // AtomicRmw: immediates[0] = op (Uint(op as u64)), [1] = ordering
            if matches!(inst.opcode, Opcode::AtomicRmw)
                && let Some(Immediate::Uint(v)) = inst.immediates.first()
            {
                self.ctx.current_atomic_op = Some(atomic_op_from_u64(*v));
            }

            // 全部结果映射（多结果 op——如溢出 op 的 (result, flag)）
            let results: smallvec::SmallVec<[XReg; 8]> = inst
                .results
                .iter()
                .map(|v| {
                    self.value_to_xreg.get(v).copied().unwrap_or_else(|| {
                        let result_ty = dfg.value_type(*v).unwrap_or(TypeId::VOID);
                        // 按类型分派到多宽度类（I32→GPR(4) 池、F32/F64→FPR(8) 池、
                        // 动态 vector → VEC）
                        let xreg = self.ctx.alloc_xreg(self.ctx.reg_class_for(&result_ty));
                        self.value_to_xreg.insert(*v, xreg);
                        self.ctx.xreg_types.insert(xreg, result_ty);
                        xreg
                    })
                })
                .collect();

            if crate::pipeline::trace_enabled("FORGE_TRACE_LOWER") {
                eprintln!(
                    "[forge] lower op={:?} operands={:?} args={:?} results={:?}",
                    inst.opcode, inst.operands, args, results
                );
            }
            // 每次 lowering 前清空写死寄存器（生成代码在 arm 尾部设置）
            self.ctx.current_clobbers.clear();
            let mut machine_insts =
                lowering.lower_inst(&inst.opcode, &args, &results, &mut self.ctx)?;
            // 展开路径必须与 InstPacket::append 一致：若包内 XRegAllocator 分配过
            // 临时寄存器（编号从 0 起，与函数级 XReg 编号域重叠），先偏移重映射到
            // 函数级编号域的续接，再并入 xreg_map——否则包内 XReg 与函数级 XReg
            // 编号重叠，regalloc 把不同 XReg 当作同一个处理。
            let base = self.ctx.xregs.next_index();
            if machine_insts.xregs.next_index() > 0 {
                machine_insts.remap_internal(base);
                for _ in 0..machine_insts.xregs.next_index() {
                    self.ctx.alloc_xreg(RegClass::GPR64);
                }
            }
            self.xreg_map.extend(machine_insts.xreg_map);
            // 规则级写死寄存器（insts 自动推导，生成代码写入 ctx.current_clobbers）
            // 展开到包内每条指令（与 vcode.instructions 平行）。每次 lowering 前
            // 已 clear，此处取走。
            let pack_clobbers = std::mem::take(&mut self.ctx.current_clobbers);
            self.inst_clobbers.extend(std::iter::repeat_n(
                pack_clobbers,
                machine_insts.insts.len(),
            ));
            for mi in machine_insts.insts {
                self.vcode.push_inst(mi);
            }
        }

        // Lower terminator
        self.ctx.current_clobbers.clear();
        let mut term_insts = lowering.lower_terminator(
            &block_data.terminator,
            &self.value_to_xreg,
            &self.block_map,
            &mut self.ctx,
        )?;
        // terminator 包同样做包内 XReg 偏移重映射（与 append 语义一致）
        let term_base = self.ctx.xregs.next_index();
        if term_insts.xregs.next_index() > 0 {
            term_insts.remap_internal(term_base);
            for _ in 0..term_insts.xregs.next_index() {
                self.ctx.alloc_xreg(RegClass::GPR64);
            }
        }
        self.xreg_map.extend(term_insts.xreg_map);
        let term_clobbers = std::mem::take(&mut self.ctx.current_clobbers);
        self.inst_clobbers
            .extend(std::iter::repeat_n(term_clobbers, term_insts.insts.len()));
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

    pub(crate) fn get_or_alloc_xreg(&mut self, val: Value, class: RegClass) -> XReg {
        if let Some(xreg) = self.value_to_xreg.get(&val).copied() {
            xreg
        } else {
            let xreg = self.ctx.alloc_xreg(class);
            self.value_to_xreg.insert(val, xreg);
            xreg
        }
    }

    // ── Stage 5: Peephole ──

    pub(crate) fn run_peephole(&mut self, peephole: &dyn TargetPeephole<Inst = I>) {
        for block in self.vcode.blocks_mut() {
            peephole.optimize(&mut block.instructions);
        }
    }
}
