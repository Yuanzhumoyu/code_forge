//! 从 `compiler.rs` 拆出（v25 M4：逐字搬移，不改行为）。

use crate::machine::target::TargetMachine;
use crate::pipeline::alloc_config::{ClassConfig, RegAllocConfig as NewRegAllocConfig};
use crate::pipeline::alloc_result::AllocResult;
use crate::pipeline::regalloc_bt::BacktrackingAllocator;
use crate::{IrError, LowerCtx, MachineInst, VBlockId, VCode};
use forge_ir::entity::map::SecondaryMap;
use forge_ir::*;
use std::collections::HashMap;

pub(crate) struct CompileState<I: MachineInst> {
    pub(crate) vcode: VCode<I>,
    /// XReg → 微指令寄存器字段映射（与 vcode 的指令序列平行）。
    /// 由各指令包的 inst_ops 聚合；分配器按此构建活区间并回写字段。
    /// 本函数的符号名（诊断用：`FORGE_DUMP_ALLOC` 的分段标题与过滤都靠它）。
    pub(crate) fn_name: forge_ir::ImmStr,
    pub(crate) inst_ops: Vec<smallvec::SmallVec<[crate::machine::lowering::OperandBind; 2]>>,
    /// 每条 vcode 指令的写死物理寄存器集（与 inst_ops/instructions 平行；
    /// 来自 lower 规则的 clobbers 声明，包级展开到包内每条指令）。
    pub(crate) inst_clobbers: Vec<Vec<(u32, crate::prelude::RegClass)>>,
    /// IR 值 → XReg 映射（v3 S2：`Value` 是密集句柄 ⇒ `SecondaryMap`，不哈希）。
    pub(crate) value_to_xreg: SecondaryMap<Value, XReg>,
    /// IR 块 → VCode 块映射（同理：`Block` 是密集句柄）。
    pub(crate) block_map: SecondaryMap<Block, VBlockId>,
    /// **块参数各自的 XReg 与类**（`map_block_params` 填）：终结符要按这张表发拷贝。
    ///
    /// 为什么不能只看 `value_to_xreg`：参数值 → xreg 的映射一旦被"实参复用"改写，
    /// 就分不清"参数独立寄存器"与"实参寄存器别名"了；拷贝需要后者恒为独立。
    pub(crate) block_param_xregs: SecondaryMap<Block, Vec<(XReg, crate::prelude::RegClass)>>,
    pub(crate) ctx: LowerCtx,
    pub(crate) param_xregs: Vec<XReg>,
    /// Alloca 指令 → 帧槽偏移（预扫描分配；lowering 时经 ctx.current_alloca_offset
    /// 供 `lea_off rd, alloca_offset` 规则取用）。`Inst` 是密集句柄 ⇒ `SecondaryMap`。
    pub(crate) alloca_offsets: SecondaryMap<Inst, i64>,
    /// **该函数的 `AbiPlan`**（v20 A3b 预备 / A6 fail-closed）：编译入口按 IR 签名 +
    /// 约定数据算出来，供发射侧（`CallLayout`）与 `FORGE_TRACE_ABI=1` 诊断使用。
    ///
    /// v20 A6 起**算不出来 = 编译错误**（不再留个 note 继续走旧路径）：每条发射路径都必须
    /// 有 plan，否则调用点/收参处会各自 fail-closed，错误消息离现场远，且"算不出来"容易被
    /// 误当成"这条签名能编"。缺口清单见 `docs/archive/calling-convention-redesign-plan.md` 的 A6。
    pub(crate) abi_plan: forge_abi::AbiPlan,
    /// v27 M2.5/S1：**内联汇编文本 → 机器指令**。由本 ISA 的 `TargetAssembler` 提供；
    /// ISA 没接汇编器时为 `None`（此时遇到 `InlineAsm` 就 fail-closed）。
    /// 为什么存成闭包：`CompileState` 不能借住 `&M`（生命周期），而汇编器本身是
    /// `Arc<dyn TargetAssembler>`（`Send + Sync + 'static`）⇒ clone 一个 Arc 即可。
    pub(crate) asm_parse: Option<std::sync::Arc<dyn Fn(&str) -> Option<Vec<I>> + Send + Sync>>,
    /// v27 M2.5/S2a：寄存器编号 + 类 → 汇编拼写（内联 asm 替换占位符用）。
    /// v27 M2.5/S2b：汇编拼写 → 寄存器编号（显式寄存器名用）。
    pub(crate) reg_by_name_of:
        Option<std::sync::Arc<dyn Fn(&str, RegClass) -> Option<u32> + Send + Sync>>,
    pub(crate) reg_name_of:
        Option<std::sync::Arc<dyn Fn(u32, RegClass) -> Option<&'static str> + Send + Sync>>,
    /// v27 M2.5/S2a：**可分配的通用寄存器顺序**（内联 asm 挑约束寄存器用；挑中的会
    /// 写进 `inst_clobbers`，所以挑到 callee-saved 也只是多存一次、不会错）。
    pub(crate) gp_alloc_order: Vec<u32>,
    /// v27 S4：本 ISA 声明的寄存器类（纯 clobber 的显式名要**跨类**解析——
    /// `clobber_abi` 会展开出 FPR 之类的非 GPR 寄存器）。
    pub(crate) reg_classes: Vec<RegClass>,
}

impl<I: MachineInst + 'static> CompileState<I> {
    pub(super) fn new<M: TargetMachine<Inst = I>>(
        machine: &M,
        func: &Function,
        module_sigs: Option<std::sync::Arc<forge_ir::ModuleSigs>>,
        abi_slots: Option<crate::pipeline::va_expand::AbiSlots>,
    ) -> Result<Self, IrError> {
        // 取一次读锁（v3 S3 读路径纪律）：本函数只读类型，内部不再逐次取锁。
        // 先从 func.types clone 出 TypeContext（Arc，O(1)）再取**一次快照**——守卫借的是
        // 这个局部，不与 func 的 &mut 借用冲突；快照交给 LowerCtx（lowering 期间类型表
        // 只读，生成代码不再逐指令取锁，见 v3 S3 余项①）。
        // v27 M2.5/S1：把本 ISA 的汇编器 clone 进来（闭包转 `Vec<I>`；无汇编器 ⇒ None）。
        let asm_parse: Option<std::sync::Arc<dyn Fn(&str) -> Option<Vec<I>> + Send + Sync>> =
            machine.assembler().map(|a| {
                let a = std::sync::Arc::clone(a);
                let f = move |text: &str| a.parse_insts(text).ok();
                std::sync::Arc::new(f)
                    as std::sync::Arc<dyn Fn(&str) -> Option<Vec<I>> + Send + Sync>
            });
        // v27 M2.5/S2a：寄存器名表与可分配 GPR 顺序的快照（同上：不借住 &M）。
        let reg_by_name_of: Option<
            std::sync::Arc<dyn Fn(&str, RegClass) -> Option<u32> + Send + Sync>,
        > = Some({
            let ri = std::sync::Arc::clone(machine.reg_info());
            std::sync::Arc::new(move |name: &str, class: RegClass| ri.reg_by_name(name, class))
        });
        let reg_name_of: Option<
            std::sync::Arc<dyn Fn(u32, RegClass) -> Option<&'static str> + Send + Sync>,
        > = Some({
            // `reg_info()` 给 `&Arc<dyn TargetRegInfo>` ⇒ clone 成 'static 句柄，闭包才活得过本函数。
            let ri = std::sync::Arc::clone(machine.reg_info());
            std::sync::Arc::new(move |reg: u32, class: RegClass| ri.reg_name(reg, class))
        });
        let reg_classes: Vec<RegClass> = machine
            .reg_info()
            .register_classes()
            .into_iter()
            .map(|c| c.reg_class)
            .collect();
        let gp_alloc_order: Vec<u32> = machine
            .reg_info()
            .allocatable_gp_order()
            .into_iter()
            .collect();
        let types_ctx = func.types.clone();
        let store = types_ctx.borrow();
        let mut ctx = LowerCtx::new();
        ctx.type_store = Some(store.clone());
        // 值/地址寄存器类与栈槽单位：全部由 TargetRegInfo 元数据提供
        //（DSL 从 [meta].value_gpr_width/addr_width/slot_bytes 生成）——
        // lowering 里不再出现 RegClass::GPR64/FPR64 字面量（1 字节寄存器 ISA
        // 的值池是 GPR(1)，写死 8 字节会生成不存在的类）。
        {
            let ri = machine.reg_info();
            ctx.value_gpr_class = ri.value_gpr_class();
            ctx.value_fpr_class = ri.value_fpr_class();
            ctx.addr_class = ri.addr_class();
            ctx.slot_bytes = ri.slot_bytes();
            ctx.vector_tiers = ri.vector_tiers().to_vec();
            ctx.type_map = ri.type_map().to_vec();
        }
        // **调用约定装配**（v20）：名字解析（未注册 fail-closed）、约定级返回槽、函数级
        // plan（算不出 = 编译错误）、约定级破坏集、模块签名表——规则与消息都在
        // `pipeline/abi_setup.rs`，这里一个调用点。
        let abi_plan =
            crate::pipeline::abi_setup::setup_conv(&mut ctx, machine, func, module_sigs)?;

        // StackAddr 的 lea 基准需要跳过 callee-saved 区（局部变量不能写在 push
        // 槽上）：fp 保存槽（frame_pointer_overhead）+ callee-saved 寄存器区。
        // 之前只算了 callee-saved 区，漏掉 fp 的 8 字节，局部变量落进 push 槽
        //（覆盖调用者寄存器保存值 → mini_c JIT SEGV/逻辑错误）。
        // 帧布局三数值统一推导：min_frame / callee_saved_bytes / 栈槽平移
        // 都由 frame_layout_info() 从 [machine.frame].layout + callee-saved 表算出
        //（fp-inside：csb=0、栈槽平移=fp_push；fp-outside：csb=pushed、平移
        // 回退 csb）。compiler.rs 的 LowerCtx 与 emission.rs 共用同一来源；
        // **有 plan 时用 plan 的 callee-saved 表**（v20 A5-3 ④：同一台机器换约定，
        // 保存集与破坏集必须一起换）。
        // v23 M5（C6 帧记账单点）：三数值**只在这里算一次**，此后 emission / spill /
        // 序言账目全读 ctx 上这三个字段（min_frame 一并填，calculate_frame_size
        // 不再自己重算——同一份输入重算两次，口径一漂移就是帧字节数不一致）。
        let fl = crate::pipeline::frame_layout::frame_layout_info(machine, ctx.conv.layout());
        ctx.callee_saved_bytes = fl.callee_saved_bytes;
        ctx.stack_slot_shift = fl.stack_slot_shift;
        ctx.min_frame = fl.min_frame;
        ctx.is_float_return = func
            .return_types()
            .iter()
            .any(|t| ctx.reg_class_for(t).is_fp());
        // S2：sret 隐藏参数——函数返回宽向量（>16 字节）→ 首 int 槽被
        // sret 指针占用（move_args 收参 __gi 从 1 起；调用方 sret 约定）。
        ctx.is_sret_return = func.return_types().iter().any(|t| {
            let s = &store;
            (s.is_vector(*t) || s.is_scalable_vector(*t)) && s.size_bytes(*t) > 16
        });
        ctx.constant_pool = Some(func.constants.clone());

        // **变参 ABI 槽**（v20 变参 V3）：保存区与 `va_list` 对象两段的深度由
        // `plan_abi_slots` 在编译入口算好（排在前端局部槽之下）并经 `abi_slots` 传进来——
        // 这里只**播种**：抬 `max_stack_bytes`（帧尺寸要覆盖它们）并记下序言 spill 用的
        // 保存区偏移。展开出来的 `StackAddr` 会让 lowering 自己再统计一次（同一组深度）。
        //
        // **保存区要求与栈实参连续**时（v20 V7，形状数据）保存区改放**帧顶**
        // （`[入口 sp - save_size, 入口 sp)`）：`va_top` 把 ra/fp、callee-saved 的保存槽与
        // 局部槽一起下移一个 save_size（前两者由生成物读 `AllocResult.va_top`，后者由
        // `stack_slot_shift` 加同一份），帧尺寸也要跟着长。
        if let Some(sl) = abi_slots {
            ctx.va_save_off = (sl.save_depth > 0).then_some(sl.save_off);
            ctx.va_top = sl.va_top;
            // 帧顶给保存区让出的字节：局部槽（`StackAddr`）与 ra/fp、callee-saved 的保存槽
            // 一起下移同一份（后者由生成物读 `AllocResult.va_top`），帧尺寸也因此长一截。
            ctx.stack_slot_shift += sl.va_top as i32;
            ctx.max_stack_bytes = ctx.max_stack_bytes.max(sl.max_depth);
        }
        Ok(Self {
            fn_name: func.name.clone(),
            vcode: VCode::new(),
            inst_ops: Vec::new(),
            inst_clobbers: Vec::new(),
            value_to_xreg: SecondaryMap::new(),
            block_map: SecondaryMap::new(),
            block_param_xregs: SecondaryMap::new(),
            ctx,
            param_xregs: Vec::new(),
            alloca_offsets: SecondaryMap::new(),
            abi_plan,
            asm_parse,
            reg_name_of,
            reg_by_name_of,
            gp_alloc_order,
            reg_classes,
        })
    }

    // ── Stage 6: Register Allocation ──

    pub(super) fn run_regalloc<M2: TargetMachine<Inst = I>>(
        &mut self,
        allocator: &BacktrackingAllocator,
        machine: &M2,
    ) -> Result<AllocResult, IrError> {
        let ri = machine.reg_info();

        // Build RegAllocConfig from TargetRegInfo — 类表 = **ISA 声明**。
        // DSL 生成的 `RegInfo::register_classes()` 由 `[reg.*]`/`[meta]`/`[abi]`
        // 派生（每个可请求的类指向同族物理寄存器文件）；**不再有编造的 fallback
        // 类表**（2026-09-13，R1）：未声明的类不会出现在分配器配置里，请求即
        // 编译期 Unsupported（此前 GPR(2)/FPR(16)/VEC(16)… 会被凭空造出并借池）。
        let mut classes: HashMap<RegClass, ClassConfig> = HashMap::new();
        for info in ri.register_classes() {
            classes.insert(
                info.reg_class,
                ClassConfig {
                    allocatable: info.allocatable.clone(),
                    reg_width: info.width as u8,
                },
            );
        }

        // 主 GPR/FPR 类推导（元数据驱动）：由 ISA 的 default_*_class() 显式
        // 暴露（DSL 从 `[reg.*]` 与 `[meta]` 生成），compiler 不再猜测
        // GPR64/FPR64。例如 x86 浮点值主类为 FPR(8)（f64 值宽），即使其 XMM
        // 寄存器组是 FPR(16)。
        let main_gpr = ri.default_gpr_class();
        let main_fpr = ri.default_fpr_class();

        // 非 DSL 后端/测试替身（类表为空）时的兜底：至少让主类可用。
        if classes.is_empty() {
            classes.insert(
                main_gpr,
                ClassConfig {
                    allocatable: ri.allocatable_gp_order(),
                    reg_width: main_gpr.bytes() as u8,
                },
            );
            classes.insert(
                main_fpr,
                ClassConfig {
                    allocatable: ri.allocatable_fp_order(),
                    reg_width: main_fpr.bytes() as u8,
                },
            );
        }

        // v20 A5-3 ④：callee-saved 是**约定事实**——有 plan 就用 plan 的（同一台机器
        // 换约定必须跟着换：sysv64 的 RDI/RSI 不是 callee-saved，win64 是）。谱里那份
        // 是无 plan（未注册约定 / 合成夹具）时的兜底。
        //
        // v20 A6 起**两类都喂**：GPR 与 FPR 各占一项（编号空间分开，免得 X8 与 V8 同号
        // 相撞）。FPR 那半只有在"发射侧能按类保存"之后才敢开——`callee_save` 角色已带类
        // 限定（arm64 的 STURD），`callee_saved_to_save` 也扫主 FPR 类表，`frame_layout_info`
        // 的帧字节数按 plan 的表算（见那里的注释）。缺任何一环，FPR 值就会跨调用被静默破坏。
        // v23 M5（C4）：**(类, 号) 一张表** —— 以前这里按类分成两条平行 `Vec<u32>`，
        // 消费端"只比号不比类"（GPR 号与 FPR 号互相误判）。现在类随号一起走。
        let callee_saved: Vec<(forge_ir::RegClass, u32)> = match self.ctx.conv.layout() {
            Some(cl) => cl
                .callee_saved
                .regs
                .iter()
                .map(|r| (r.class, r.slot))
                .collect(),
            // 无 plan（未注册约定/合成夹具）：v21 W9 删除谱面 callee-saved 两键之后这里
            // 没有兜底表可读——按 A6 的口径，没有 plan 的签名在更早的入口就 fail-closed，
            // 因此空表即可（保持"不静默用别家寄存器名"的纪律）。
            None => Vec::new(),
        };

        let reg_info = NewRegAllocConfig {
            classes,
            main_gpr_class: main_gpr,
            main_fpr_class: main_fpr,
            sp_reg: ri.sp_reg().register_index().unwrap_or(0),
            fp_reg: ri.fp_reg().map(|r| r.to_index()),
            callee_saved,
            scratch_regs: ri
                .scratch_regs()
                .iter()
                // scratch 是值搬运临时寄存器 → 类 = ISA 值池类（x86 = GPR(8)）。
                .map(|&n| PReg::new(n, ri.value_gpr_class()))
                .collect(),
            param_xregs: self.param_xregs.clone(),
            // 寄存器参数数 = **布局里在寄存器里落一个值的形参个数**（v20 A5-3：谱面的
            // `[abi.arg_class]` 已删除，不再用"int 参数槽上限"这个生成期近似——
            // `Indirect { reg: Some(..) }` 也要算：by-ref 的指针在寄存器里、被调方还要把 `[ptr]` 的值
            // load 进自己的形参寄存器——不预分配就会走"spilled ⇒ 不收参"（实测宽向量 by-ref 全 0）。
            param_reg_count: self
                .ctx
                .conv
                .layout()
                .map(|cl| {
                    cl.args
                        .iter()
                        .filter(|a| {
                            // 受支持的落点 = 单寄存器 / 带指针的间接（v23 M2：走访问器，
                            // 不再穷举运行时镜像的枚举）。
                            a.place.as_reg().is_some() || a.place.indirect_ptr().is_some()
                        })
                        .count()
                })
                .unwrap_or(0),
        };

        // **折入绑定表：把值的描述类折进 `OperandBind` 一次（v24 M2 起描述已单一来源）。**
        //
        // 以前两个回填点各自把这张表推成"值宽类"，**且口径不同**：
        // `compiler.rs` 主回填走 `ctx.reg_class_for(ty)`（ISA 类型映射），
        // `emission.rs` 的 spill 回填走 `forge_ir::RegClass::from_type_id(ty)`。
        // 现场只有一处是对的、另一处是兜底——同一件事两种写法。现在在这里折成
        // `OperandBind::class`，两个回填点读同一个值。
        // 不再被读（写入面仍留在 lowering 期作草稿，见方案 §13 的余项）。
        for slot in self.inst_ops.iter_mut() {
            for bind in slot.iter_mut() {
                if let Some(d) = self.ctx.desc_of(&bind.vreg) {
                    // 类**就是描述里的类**（`set_value_desc` 的唯一推导处算好）——
                    // 不再由这里"从类型再推一次"（那正是两处口径分叉的来源）。
                    bind.class = Some(d.class);
                }
            }
        }

        // **v23 M4 收敛守卫**：旁表（`inst_ops`）与指令的**操作数表**必须逐条对齐。
        //
        // 批次 C 要删掉旁表，这条断言就是"删了以后行为不变"的判据——现在先当守卫跑起来。
        // 只对拍 `(field, role)`：旁表里存的是**虚拟寄存器**，而字段里放的是**占位物理
        // 寄存器**，两者本就不相等（让字段占位携带 vreg 号是批次 C 的前置条件，见方案 §13）。
        //
        // 判据的意义：`map_reg_field(xreg, inst, field, is_def)` 的 `(field, is_def)` 必须
        // 能在该指令的操作数表里找到同字段同角色的项；找不到就说明"旁表说的落点"与
        // "指令形状表说的落点"已经分叉——regalloc 会按旁表分配、发射按形状表回填。
        #[cfg(debug_assertions)]
        {
            use crate::machine::inst::OperandRole;
            let mut gi = 0usize;
            for block in self.vcode.blocks() {
                for inst in &block.instructions {
                    if let Some(slot) = self.inst_ops.get(gi) {
                        let ops = inst.operands();
                        for bind in slot.iter() {
                            let (xr, field) = (&bind.vreg, bind.field);
                            let want = bind.role;
                            let hit = ops.iter().any(|o| {
                                usize::from(o.field) == usize::from(field) && o.role == want
                            });
                            assert!(
                                hit,
                                "v23 M4：旁表与操作数表分叉——第 {gi} 条指令的字段 {field} \
                                 被登记为 {want:?}（vreg {}），但操作数表里没有这一项：{ops:?}",
                                xr.index()
                            );
                            // **多类槽的值宽类来源：可见但不强制**（v23 M4，2026-10-11 实测）。
                            //
                            // 形状表说该字段是**多类槽**（`class == None` = 宽度由运行期决定）
                            // 时，回填用的是绑定带的类；绑定没带（值描述表里没有这个 vreg
                            // 的记录）就退到 vreg 的**池宽类**。
                            //
                            // 实测：确实存在这种绑定（如 `test_jit_agg_by_value_stack_sysv64`
                            // 第 15 条指令、riscv 侧也有）——但这是**既有**行为：改动前的两个
                            // 回填点同样先查值描述、查不到就退池宽类，所以 M4 没有改变
                            // 它。这里**断言**会过强（等于要求代码具备它从来没有的性质），
                            // 因此改成**可观测**：`FORGE_TRACE_M4=1` 时报出来，留存量级。
                            if let Some(op) = ops
                                .iter()
                                .find(|o| o.field == bind.field && o.role == bind.role)
                                && op.class.is_none()
                                && bind.class.is_none()
                                && crate::pipeline::trace::knob("FORGE_TRACE_M4")
                            {
                                eprintln!(
                                    "[m4] 多类槽字段 {} 的 vreg {} 没有值宽类记录 ⇒ 回填退池宽类\
                                     （第 {gi} 条指令，指令形状：{ops:?}）",
                                    bind.field,
                                    xr.index()
                                );
                            }
                        }
                        // **旁表条数不是从形状表推导出来的（2026-10-11 实测，两次修正）**：
                        //
                        // - InOut 字段（如 x86 `add dst, src` 的 dst）在操作数表里**同时**进
                        //   `uses` 与 `defs`，但旁表里它可能 **0 条**（该槽是物理寄存器占位、
                        //   不参与 vreg 映射）或 **1 条 Def**（vreg 绑定）——实测两种都有。
                        // - 因此"删掉 `is_def`、一律按表派生角色"会把这些情况变成 use+def 各一条，
                        //   **改变 regalloc 的活区间口径**（属行为变更，不是纯重构）。
                        //
                        // 这里只钉一条**可证伪的**事实：旁表条数不得为形状表里该字段出现次数的
                        // 函数以外的值——即 InOut 字段最多 1 条（0 或 1），普通字段最多 1 条。
                        // 真正的判据仍是上面那条子集关系；本块是"改动会被看见"的哨兵。
                        for op in ops.iter() {
                            let in_uses = ops
                                .iter()
                                .any(|o| o.field == op.field && o.role == OperandRole::Use);
                            let in_defs = ops
                                .iter()
                                .any(|o| o.field == op.field && o.role == OperandRole::Def);
                            if in_uses && in_defs {
                                let n = slot.iter().filter(|b| b.field == op.field).count();
                                assert!(
                                    n <= 1,
                                    "v23 M4：InOut 字段 {} 的旁表条数 {n} > 1（实测契约是 0 或 1）\
                                     ——旁表语义变了，请当行为变更处理：{slot:?}",
                                    op.field
                                );
                            }
                        }
                    }
                    gi += 1;
                }
            }
        }

        let alloc_result = allocator.allocate_named(
            self.fn_name.as_str(),
            &self.vcode,
            &reg_info,
            &self.inst_ops,
            &self.inst_clobbers,
        )?;

        // S2：sret 隐藏参数标记（move_args 收参跳过首 int 槽）——
        // 由 CompileState::new 的 LowerCtx.is_sret_return 预计算。
        let mut alloc_result = alloc_result;
        alloc_result.sret = self.ctx.is_sret_return;
        // v20 A3b-2b：把中性调用布局一并交给帧件（序言/尾声只拿到 AllocResult）。
        alloc_result.call_layout = self.ctx.conv.layout.clone();
        // 栈参数区字节数（move_args 收栈参数时计算 spill 槽地址）
        alloc_result.stack_arg_bytes = self.ctx.max_stack_arg_bytes;
        // 寄存器保存区 / 对象槽偏移（v20 变参 V3）：序言 spill 用前者。
        alloc_result.va_save_off = self.ctx.va_save_off;
        // 帧顶保存区字节数（v20 V7）：fp-inside 的存取机制要按它把自己的偏移减掉一份。
        alloc_result.va_top = self.ctx.va_top;
        // 参数字节宽（IR 类型 size_bytes）——@move_args 的 by-ref 宽向量收参按
        // 真实字节宽分派 32B/64B load 变体。寄存器类宽对 >128 位向量恒为
        // VEC(32)（reg_class_for），无法区分 V256/V512：旧实现用 __pv.width()
        // 使 64B 分支永不可达 → V512 收参只 load 32B（WA-37 D5）。
        alloc_result.param_bytes = self
            .param_xregs
            .iter()
            .map(|v| {
                self.ctx
                    .desc_of(v)
                    .and_then(|d| self.ctx.type_store.as_ref().map(|s| s.size_bytes(d.ty)))
                    .map(|b| b as u16)
                    .unwrap_or(0)
            })
            .collect();

        // 分配后回写：按 inst_ops 把 XReg 的分配结果填入微指令寄存器字段（物理 Reg）
        let mut global_inst = 0usize;
        for block in self.vcode.blocks_mut() {
            for inst in block.instructions.iter_mut() {
                if let Some(slot) = self.inst_ops.get(global_inst) {
                    for bind in slot.iter() {
                        let (xreg, field_idx) = (bind.vreg, bind.field);
                        if let Some(preg) = alloc_result.preg(xreg) {
                            // class：值宽类已由上面的折入步骤写进绑定（原实现现场从
                            // 值描述推，与 emission 的 spill 回填口径不同）；
                            // `None`（该 vreg 没有类型记录）退到 vreg 自身类。
                            let cls = bind.class.unwrap_or_else(|| xreg.class());
                            inst.set_reg_field(field_idx as usize, preg.num, cls);
                        }
                    }
                }
                global_inst += 1;
            }
        }
        let _ = std::mem::take(&mut self.param_xregs); // param_xregs 现在由 alloc_result 管理

        // 回写自检（debug 常开 / release 由 `FORGE_VERIFY_FIELDS` 开）：字段里的寄存器
        // 必须来自"分配结果"或"溢出编辑"，不许停在 lowering 期的占位寄存器上。
        if cfg!(debug_assertions) || std::env::var_os("FORGE_VERIFY_FIELDS").is_some() {
            crate::pipeline::field_check::verify_field_bindings(
                self.fn_name.as_str(),
                &self.vcode,
                &self.inst_ops,
                &alloc_result,
            )?;
            // 槽初始化探针**只在显式开关下跑**：它目前会误报（序言写槽不在种子里），
            // 每编一个函数就刷几十行 stderr 会淹没真正要看的东西。
            if std::env::var_os("FORGE_VERIFY_SPILL_INIT").is_some() {
                crate::pipeline::spill_init::verify_spill_init("", &self.vcode, &alloc_result)?;
            }
        }
        Ok(alloc_result)
    }
}
