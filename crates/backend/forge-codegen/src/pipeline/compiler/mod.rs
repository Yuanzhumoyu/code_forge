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

use crate::machine::target::TargetMachine;

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
    machine: M,
    reg_alloc: BacktrackingAllocator,
    /// IR 级优化级别（None = 不优化，直接编译）。Some(level) 时
    /// `compile()`/`compile_with_alloc()` 先对函数克隆体跑对应 PassManager 管线再 lower。
    /// P0-1：把 forge-opt 优化管线接入生产编译（此前 PassManager 是死代码）。
    opt_level: Option<forge_opt::OptimizationLevel>,
    /// **模块级签名表**（变参 D6）：`FuncRef::index()` → 签名事实。
    ///
    /// 编译**整个模块**的入口（或装过表的单函数入口）填它（[`Self::with_module_sigs`]）；
    /// 没装 ⇒ 调用点按非变参处理。
    module_sigs: Option<std::sync::Arc<forge_ir::ModuleSigs>>,
}

impl<M: TargetMachine> CompileSession<M> {
    /// Create a new compiler with the default register allocator (backtracking).
    pub fn new(machine: M) -> Self {
        // 宿主级**一次**注册（幂等）：管线工厂 + **调用点布局钩子**（v20 A5-3）。
        // 生成物的 lowering 在调用点按 `(ISA, 约定, 实参形状)` 查后者拿被调方落点；
        // 没注册就 fail-closed——所以放在每个编译入口（OnceLock，热路径只是一次原子读）。
        crate::pipeline_hooks::ensure_registered();
        Self {
            machine,
            reg_alloc: BacktrackingAllocator,
            opt_level: opt_default::default_opt_level(),
            module_sigs: None,
        }
    }

    /// 宿主机器（整模块编排要用它的克隆；会话自己持有它，不再外借）。
    pub(crate) fn machine(&self) -> &M {
        &self.machine
    }

    /// 装上**模块级签名表**（变参 D6）。
    ///
    /// 表由 IR 模块自己给出（[`forge_ir::Module::signature_table`]）——别在宿主侧另抄一份。
    pub fn with_module_sigs(mut self, sigs: forge_ir::ModuleSigs) -> Self {
        self.module_sigs = Some(std::sync::Arc::new(sigs));
        self
    }

    /// **编译"住在某个模块里的函数"**——从模块自己取签名表装上（变参 D6）。
    ///
    /// 宿主侧的默认写法：只要函数住在某个模块里，就把模块一起传进来，
    /// 别让调用点去猜被调方是不是变参（[`Self::compile_module`] 就是这么做的）。
    pub fn for_module(machine: M, module: &forge_ir::Module) -> Self {
        Self::new(machine).with_module_sigs(module.signature_table())
    }

    /// 启用 IR 级优化（`compile`/`compile_with_alloc` 前跑对应 O 级别管线）。
    pub fn with_opt_level(mut self, level: forge_opt::OptimizationLevel) -> Self {
        self.opt_level = Some(level);
        self
    }

    /// 当前优化级别（None = 未启用）。
    pub fn opt_level(&self) -> Option<forge_opt::OptimizationLevel> {
        self.opt_level
    }

    /// Compile an IR function (full pipeline，含 IR 级优化若已启用 `with_opt_level`)。
    pub fn compile(&self, func: &Function) -> Result<CompiledFunction, IrError> {
        self.compile_with_alloc(func).map(|(cf, _)| cf)
    }

    /// 值池门：函数内每个值类型都必须能被 ISA 承载
    /// （`TargetRegInfo::class_for_type` 返回 `Some`）。否则报 `Unsupported`
    /// 并点名类型与值池宽度——**绝不**静默按某个宽度缺省生成寄存器类。
    fn check_value_pools(machine: &M, func: &Function) -> Result<(), IrError> {
        let ri = machine.reg_info();
        let gpr = ri.value_gpr_class();
        let fpr = ri.value_fpr_class();
        for (v, _) in func.dfg.values() {
            let Some(ty) = func.dfg.value_type(v) else {
                continue;
            };
            // 已删除/墓碑值：DCE 之后 `ValueData.ty` 被置为 `TypeId::VOID`
            // （见 `dfg.rs` 的 replace/tombstone 路径），它不承载任何寄存器——
            // 必须跳过，否则 1 字节值池的 ISA 上**任何**被优化掉的死值都会让
            // 整个函数编译失败（`from_type_id(VOID)` 兜底是 GPR(8)）。
            if matches!(ty, TypeId::VOID) {
                continue;
            }
            if ri.class_for_type(ty).is_none() {
                return Err(crate::IrError::capability(
                    "VALUE_POOL_TYPE",
                    format!(
                        "ISA 值池无法承载类型 {}（{} 位；值池 GPR {} 字节 / FPR {} 字节。\
                     需要 [meta].value_gpr_width / addr_width / value_fpr_width 声明更宽的\
                     寄存器组，或改用更窄的 IR 类型）",
                        crate::machine::reg_info::type_label(ty),
                        ty.builtin_scalar_bits()
                            .map(|b| b.to_string())
                            .unwrap_or_else(|| "非标量".to_string()),
                        gpr.bytes(),
                        fpr.bytes()
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Compile an IR function，额外返回寄存器分配明细（AllocResult：
    /// XReg → PReg assignments、spill_slots 等），供集成测试/工具断言
    /// 分配器行为（ABI 参数分配、clobber 写死保护等）。
    pub fn compile_with_alloc(
        &self,
        func: &Function,
    ) -> Result<(CompiledFunction, crate::AllocResult), IrError> {
        // **机读出口**（v25 M3）：成功路径的事件在 `compile_with_alloc_inner` 里发
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

    fn compile_with_alloc_inner(
        &self,
        func: &Function,
    ) -> Result<(CompiledFunction, crate::AllocResult), IrError> {
        // P0-1：IR 级优化管线接入——若启用，克隆函数跑对应 O 级别管线，
        // 用优化后的克隆体继续编译（原 func 不被修改；PassManager 需要
        // &mut Function）。未启用时不克隆（零开销）。
        // 注：**没有**"绕过优化"的旁路入口——要优化就 `with_opt_level`，不要就默认。
        let mut owned: Function;
        let func: &Function = if let Some(level) = self.opt_level {
            owned = func.clone();
            let pm = forge_opt::PassManager::for_level(level);
            pm.run_on_function(&mut owned)?;
            &owned
        } else {
            func
        };

        let _cf_t0 = std::time::Instant::now();
        let fname = func.name.to_string();
        events::emit("COMPILE-START", &[("fn", events::Field::Str(&fname))]);
        // 值池门（fail-closed，2026-09-12 去「宽度写死」）：函数里出现的每个值
        // 类型都必须落在 ISA 值池内（`TargetRegInfo::class_for_type`）。1 字节
        // 寄存器 ISA 上出现 i16/i32/i64/指针 等宽类型时**编译期拒绝**，而不是
        // 静默按 8 字节池生成不存在的寄存器类（历史行为：生成成功但语义错误）。
        Self::check_value_pools(&self.machine, func)?;
        // 宽向量参数/返回（>16 字节）ABI：
        // - 若 ISA 声明 `vector by-ref limit`（如 x86 limit=128 位）→ 宽向量按
        //   引用传参（调用方栈拷贝 + 传指针 GPR；被调方入口从 [ptr] 加载）。
        // - 否则拒绝（ABI 仅寄存器传值，宽向量会静默截断）。
        let by_ref_limit = self.machine.abi().vector_by_ref_limit();
        // **一次快照**（v3 S3：`TypeStoreRef` 是 owned，不与下面 `func` 的借用冲突）——
        // 原来每个参数/返回值/每个指令类型各 `borrow()` 一次（取锁次数 ∝ IR 规模）。
        let types = func.types.borrow();
        let param_tys = func.param_types();
        let return_tys = func.return_types();
        let wide = param_tys
            .iter()
            .chain(&return_tys)
            .find(|t| {
                (types.is_vector(**t) || types.is_scalable_vector(**t))
                    && types.size_bytes(**t) > 16
            })
            .copied();
        if let Some(ty) = wide {
            let bytes = types.size_bytes(ty);
            // S4：超过 16 字节的定宽向量要**本机能编能跑**才放行（V512 = EVEX 前提）——
            // 判据来自 ISA 后端（`TargetRegInfo::wide_vector_bytes` = 谱申报档位 ∩ 宿主上界），
            // 通用管线**不认识** `avx512` 这类能力名（v23 M3）。
            let wide_caps = self.machine.reg_info().wide_vector_bytes();
            if bytes > 16 && !wide_caps.contains(&(bytes as u16)) {
                return Err(crate::IrError::capability(
                    "SIMD_WIDE_VECTOR_TIER",
                    format!(
                        "SIMD 参数/返回 {bytes} 字节宽向量：本 ISA/宿主的宽向量档位是 {wide_caps:?}\
                     （type {ty:?}）"
                    ),
                ));
            }
            match by_ref_limit {
                Some(lim) if bytes <= lim => {
                    // 在寄存器传值范围内（≤16 字节）：走现有 XMM/GPR 传参路径。
                }
                Some(lim) => {
                    // 超过阈值：by-ref 传参（调用方栈拷贝 + 传指针；被调方入口
                    // 从 [ptr] 加载）。入口守卫通过——move_args/call lowering
                    // 的 by-ref 路径处理。V512（64 字节）等超宽向量若 ISA 未
                    // 声明足够 limit 时由 encode 侧能力决定（avx512 检测）。
                    let _ = lim;
                }
                None => {
                    return Err(crate::IrError::capability(
                        "SIMD_VECTOR_BYREF",
                        format!(
                            "SIMD 参数/返回 {bytes} 字节暂不支持（ABI 未声明 vector by-ref；type {ty:?}）"
                        ),
                    ));
                }
            }
        }
        // IR 层宽向量（>16B）能力门（W3，2026-09-12）：
        //  - 32B（V256）：VEX.256（如 vmovups ymm）——ISA 已有算术/常量/槽往返
        //    全套规则；与既有 V256 算术路径一致，不在此加硬件门（运行级用例
        //    自行按 AVX 能力 skip）。
        //  - 64B（V512）：EVEX（zmm）——**需 AVX-512F**；无则编译期拒绝
        //    （与上方 ABI 守卫同一判据），避免发射本机无法执行的 EVEX。
        //  - 其它 >16B 宽度（非 32/64）：无 lowering 规则 → 显式拒绝。
        // 旧行为：>16B 向量 Load/Store 一律拒绝（ISA 缺宽槽类 → 曾落到默认
        // 8 字节 MOV 静默截断）；现由 isa/x86.toml 的 rd_vec/rs1_vec
        // = 32/64 规则 + 本条硬件门共同把住。
        let wide_caps = self.machine.reg_info().wide_vector_bytes();
        for (_, inst) in func.dfg.insts() {
            let tys: Vec<TypeId> = inst
                .results
                .iter()
                .chain(inst.operands.iter())
                .filter_map(|v| func.dfg.value_type(*v))
                .collect();
            for ty in tys {
                if !(types.is_vector(ty) || types.is_scalable_vector(ty)) {
                    continue;
                }
                let bytes = types.size_bytes(ty);
                if bytes <= 16 {
                    continue;
                }
                if bytes > 16 && !wide_caps.contains(&(bytes as u16)) {
                    return Err(crate::IrError::capability(
                        "VECTOR_TIER_UNSUPPORTED",
                        format!(
                            "IR 层 {bytes} 字节宽向量：本 ISA/宿主的宽向量档位是 {wide_caps:?}\
                         （type {ty:?}）"
                        ),
                    ));
                }
                if bytes != 32 && bytes != 64 {
                    return Err(crate::IrError::capability(
                        "VECTOR_BYTES_LOWERING",
                        format!(
                            "IR {bytes} 字节向量暂不支持（仅 V256 32B / V512 64B 有 lowering \
                         规则；type {ty:?}）"
                        ),
                    ));
                }
            }
        }
        // 聚合参数/返回 >8 字节：由 expand_agg_call_args / expand_large_agg_params /
        // expand_large_agg_ret 处理（≤16 字节拆段；>16 字节在 pass 内拒绝）。
        // 异常处理（P1.1）：invoke/landingpad/resume 无机器指令映射，编译期拒绝
        for (_, inst) in func.dfg.insts() {
            if matches!(inst.opcode, Opcode::LandingPad) {
                return Err(crate::IrError::capability(
                    "EXCEPTION_LANDINGPAD",
                    "异常处理 landingpad 无机器指令映射（P1.1 仅文本层解析/展示）".to_string(),
                ));
            }
        }
        for (b, _bd) in func.dfg.blocks() {
            if matches!(
                func.dfg.term_kind(b),
                Some(TermKind::Invoke | TermKind::Resume)
            ) {
                return Err(crate::IrError::capability(
                    "EXCEPTION_INVOKE_RESUME",
                    "异常处理 invoke/resume 无机器指令映射（P1.1 仅文本层解析/展示）".to_string(),
                ));
            }
        }
        // 可变的 IR 副本：聚合 store/参数展开（agg_expand）需要 &mut Function。
        // 展开后读取 func_ref（Stage 4+ 全部只读原函数；S6 的树型 树型匹配
        // 匹配发生在 lowering 驱动内逐块预扫，不需要在编译期克隆整个 Function）。
        let needs_agg_expand = crate::pipeline::agg_expand::needs_agg_expand(func);
        let mut func_owned: Option<Function> = needs_agg_expand.then(|| func.clone());

        // **变参 IR 展开**（v20 变参 V3/V6）：`VaStart`/`VaArg` 在这里展开成显式 IR——按 plan 的
        // 形状数据（字段布局 + 保存区 + 取参规则）跑同一套算法，理由见 `pipeline/va_expand.rs`。
        //
        // 时机：与聚合展开同一处（**必须**在 `CompileState::new` 之前——它克隆常量池；展开会
        // 往池里插常量），也在 lowering 之前。
        //
        // **ABI 槽（保存区 + `va_list` 对象）的排布时机（v23 收尾 M7 修正）**：它必须排在
        // 前端局部槽之下，深度靠扫 IR 得到。**扫的必须是合法化之后的 IR**——聚合族 pass 会
        // 新增 `Alloca`（聚合槽），若在它们之前排定，lowering 看到的局部区更大 ⇒ 新槽可能
        // 落进 ABI 槽区（症状：字段各自读对、一起用就错、每次运行还不同）。所以：
        //
        // ```text
        //   ① 聚合族 pass（不含 va）  →  ② plan_abi_slots（此时含新建的 Alloca）
        //                             →  ③ Va pass（用排好的槽）
        // ```
        //
        // 这里先把 `va` 信息（**不含槽**）算出来，槽在 ② 之后补上。
        let mut abi_slots: Option<crate::pipeline::va_expand::AbiSlots> = None;
        let mut va_pending: Option<(
            forge_isa_runtime::machine::call_layout::VaInfo,
            i64, // stack_slot_shift
        )> = None;
        if crate::pipeline::va_expand::has_va_op(func) {
            match crate::pipeline::va_expand::va_info_for_expansion(
                &self.machine,
                func,
                self.module_sigs.clone(),
            )? {
                Some((va, layout)) => {
                    let fl = crate::pipeline::frame_layout::frame_layout_info(
                        &self.machine,
                        Some(&layout),
                    );
                    va_pending = Some((va, i64::from(fl.stack_slot_shift)));
                    // 变参也要就地改写 IR ⇒ 需要有可变的副本（与聚合展开同一份）。
                    func_owned.get_or_insert_with(|| func.clone());
                }
                None => {
                    return Err(crate::IrError::capability(
                        "VA_SHAPE_MISSING",
                        format!(
                            "变参：函数 `{}` 用了 VaStart/VaArg，但它的约定没声明 `hidden.va_list`\
                         （`va_list` 的形状）——变参需要形状数据，生成器不再有专用臂",
                            func.name
                        ),
                    ));
                }
            }
        }

        // Stage 0.5: 聚合 store 展开（聚合字面量 → 打包 i64 常量 store；
        // >8 字节聚合显式 Unsupported）。必须在 CompileState 创建**之前**
        // 执行：CompileState 克隆 func 的常量池，若先建 state 再展开，新常量
        // 只进 func_owned 副本的池，lowering 读 state 里旧池（idx 错位 → 0）。
        crate::pipeline::legalize::ensure_scalar_owned(&self.machine, func, &mut func_owned);
        if let Some(f) = func_owned.as_mut() {
            // **合法化单一驱动**（v23 M6）：定序、依赖传递与"改了什么"都收在驱动里。
            // 与"改了什么"都收在 `legalize::LegalizeDriver` 里，调用点不再抄一份次序。
            // 聚合按值传参的落点判据来自**约定数据**（只读单参探针）：win64 `>8B ⇒ 调用方
            // 副本 + 指针`、sysv64 `>16B ⇒ 传出参数区（未接）`。约定名解析失败 ⇒ fail-closed
            //（与 `abi_setup::setup_conv` 同口径）。
            let agg_conv = crate::pipeline::conv_registry::resolve(&f.calling_convention)?;
            let mut lctx = crate::pipeline::legalize::LegalizeCtx::new(&self.machine, agg_conv);
            let mut changes =
                crate::pipeline::legalize::LegalizeDriver::scalar().run(f, &mut lctx)?;
            // ① 聚合族先跑（不含 va）：它新增 `Alloca` ⇒ 必须在 ABI 槽排布前落定。
            changes.merge(
                crate::pipeline::legalize::LegalizeDriver::with_aggregates_except_va()
                    .run(f, &mut lctx)?,
            );
            // **①.5 宽值批**：必须在**聚合批之后**（跨界宽值已由计划相/聚合批改写成
            // 按引用指针、sret 缓冲或向量槽），在**收尾批之前**（物化相要看到已拆肢的
            // load/store）。见 `LegalizeDriver::wide` 的说明与 `WA-52`。
            changes.merge(crate::pipeline::legalize::LegalizeDriver::wide().run(f, &mut lctx)?);
            // **② 排 ABI 槽**：此时才扫 IR——深度里已含聚合族新建的槽（见上方时机注释）。
            if let Some((va, shift)) = va_pending.take() {
                let slots = crate::pipeline::va_expand::plan_abi_slots(
                    f,
                    &va,
                    crate::machine::target::TargetMachine::reg_info(&self.machine).slot_bytes()
                        as u32,
                    shift,
                );
                abi_slots = Some(slots);
                lctx.va = Some((va, slots));
                // **③ 再跑 va**（用排好的槽）。
                changes
                    .merge(crate::pipeline::legalize::LegalizeDriver::va_only().run(f, &mut lctx)?);
            }
            // ④ 收尾（物化相 + GEP）：物化须等 va 登记完位置。
            changes.merge(crate::pipeline::legalize::LegalizeDriver::finalize().run(f, &mut lctx)?);
            if crate::pipeline::trace::knob("FORGE_TRACE_AGG") {
                eprintln!(
                    "[forge] 聚合展开 {}：共 {} 项（+{} 指令 / +{} 值 / +{} 常量 / sret={}）",
                    f.name,
                    changes.total(),
                    changes.new_insts,
                    changes.new_values,
                    changes.new_consts,
                    changes.sret
                );
            }
            // 聚合展开是**就地改写 IR**（墓碑化/改 Copy/重写操作数）——debug 构建下
            // 立刻校验 **use-def 一致性**（2026-09-14 S6：这些站点此前只改字段、
            // 不更新 use-lists，留下陈旧 use 项）。
            //
            // 这里只跑 use-lists 契约而不是完整 `Verifier`：本阶段会**有意**造出
            // 类型不完全自洽的 IR —— `expand_geps` 的指针算术把 PTR 基址与 i64 偏移
            // 放进同一条 `Iadd`（结果标 PTR；源码注释也写着"verify 不跑"），
            // 而 DSL 后端没有 `Ptrtoint`/`Inttoptr` 的 lowering，无法用显式转换
            // 表达同一语义。该类型模型欠账记在 `docs/archive/forge-ir-v3-plan.md`
            // （S4/S5：指针算术的 typed 表示），不在本阶段用"放宽校验"掩盖。
            #[cfg(debug_assertions)]
            {
                if let Err(errors) = f.use_lists.verify(&f.dfg) {
                    return Err(IrError::Internal(format!(
                        "聚合展开后 use-lists 不一致（fn {}）：{errors:?}",
                        f.name
                    )));
                }
            }
        }
        // Stage 4+ 读取的 Function：有改写（pattern/聚合展开）时用副本。
        let func_ref: &Function = func_owned.as_ref().unwrap_or(func);

        let mut state =
            CompileState::new(&self.machine, func_ref, self.module_sigs.clone(), abi_slots)?;
        let _t = std::time::Instant::now();

        // Stage 1: Block mapping
        state.create_blocks(func_ref);
        let _t1 = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 2: VReg pre-allocation
        state.alloc_params(func_ref);
        state.pre_allocate_phi_vregs(func_ref);
        state.pre_allocate_block_param_xregs(func_ref);
        let _t2 = _t.elapsed();
        let _t = std::time::Instant::now();
        let _t2b = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 4: Instruction selection
        state.lower_all_blocks(func_ref, self.machine.lowering().as_ref())?;
        let _t3 = _t.elapsed();
        let _t = std::time::Instant::now();

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

        let _t4 = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 6: Register allocation
        let alloc_result = state.run_regalloc(&self.reg_alloc, &self.machine)?;
        let _t5 = _t.elapsed();
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
            ],
        );
        let _t = std::time::Instant::now();

        // Stage 7: Frame layout
        let frame_size = state.calculate_frame_size(&alloc_result, &self.machine);
        let _t6 = _t.elapsed();
        let _t = std::time::Instant::now();

        // Stage 8-11: Emission
        let result = state.emit_code(func_ref, &alloc_result, frame_size, &self.machine);
        let _t7 = _t.elapsed();
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
                    events::Field::U64(_cf_t0.elapsed().as_micros() as u64),
                ),
                ("blocks_us", events::Field::U64(_t1.as_micros() as u64)),
                ("vreg_us", events::Field::U64(_t2.as_micros() as u64)),
                ("lower_us", events::Field::U64(_t3.as_micros() as u64)),
                ("peep_us", events::Field::U64(_t4.as_micros() as u64)),
                ("regalloc_us", events::Field::U64(_t5.as_micros() as u64)),
                ("frame_us", events::Field::U64(_t6.as_micros() as u64)),
                ("emit_us", events::Field::U64(_t7.as_micros() as u64)),
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
                _cf_t0.elapsed(),
                _t1,
                _t2,
                _t3,
                _t4,
                _t5,
                _t6,
                _t7
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

pub mod opt_default;
mod state;

pub(crate) use state::CompileState;

#[cfg(test)]
mod alloc_integration_tests;
