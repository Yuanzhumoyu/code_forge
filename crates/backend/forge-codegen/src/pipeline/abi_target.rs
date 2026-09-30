//! **宿主 `AbiTarget` 适配器**（v20 A3）：把真实后端的 `TargetRegInfo`/`TargetMachine`
//! 喂给 `forge-abi` 的引擎，于是"IR 签名 + 约定数据"能算出真机 `AbiPlan`。
//!
//! ```text
//! Function ──sig_view──► forge_abi::Signature
//!                              │
//! M: TargetMachine ──本模块──► AbiTarget（寄存器文件 + 能力申报）
//!                              ▼
//!                        AbiRegistry::plan() → AbiPlan
//! ```
//!
//! 索引空间沿用 `forge-isa-runtime` 的口径：`0..n_gp` = GPR 区，`n_gp..n_gp+n_fp` = FP 区
//! （与 `forge-isa abi check` 的静态视图同序，因此绑定文件里的名字/整数选择子两边都认）。
//!
//! **本片只算不算用**：`plan_for_function` 产出 `AbiPlan` 供校验与诊断；按 plan 发射
//! 调用点/入口/序尾声是 A3b（验收：x86 生成物逐字节不变）。

use forge_abi::{AbiError, AbiPlan, AbiRegistry, AbiTarget, Capability, Signature};
use forge_ir::ir::function::Function;
use forge_ir::{PhysReg, RegClass};
use forge_isa_runtime::machine::call_layout::{
    ArgPlace, ArgShape, CallArg, CallLayout, Ext, RetPlace, ShapeKind,
};
use forge_isa_runtime::machine::call_plan::{CallPlanError, CallRequest};
use forge_isa_runtime::machine::target::TargetMachine;

/// `TargetMachine` 上的 `AbiTarget` 视图。
pub struct MachineAbiTarget<'a, M: TargetMachine> {
    machine: &'a M,
    n_gp: u32,
    n_fp: u32,
    gpr_class: RegClass,
    fpr_class: RegClass,
    alloc_gp: Vec<u32>,
    alloc_fp: Vec<u32>,
    scratch: Vec<u32>,
}

impl<'a, M: TargetMachine> MachineAbiTarget<'a, M> {
    pub fn new(machine: &'a M) -> Self {
        let ri = machine.reg_info();
        let n_gp = ri.num_gp_regs();
        let n_fp = ri.num_fp_regs();
        Self {
            machine,
            n_gp,
            n_fp,
            gpr_class: ri.addr_class(),
            fpr_class: ri.default_fpr_class(),
            alloc_gp: ri.allocatable_gp_order(),
            alloc_fp: ri.allocatable_fp_order(),
            scratch: ri.scratch_regs(),
        }
    }

    fn reg_at(&self, index: u32) -> Option<M::Reg> {
        if index < self.n_gp {
            Some(M::Reg::from_index(index, self.gpr_class))
        } else if index < self.n_gp + self.n_fp {
            Some(M::Reg::from_index(index - self.n_gp, self.fpr_class))
        } else {
            None
        }
    }

    /// 物理寄存器名（诊断/快照用）：DSL 生成的 `Reg` 枚举变体名就是 ISA 谱里的名字。
    fn name_of(&self, index: u32) -> Option<String> {
        self.reg_at(index).map(|r| format!("{r:?}"))
    }

    fn is_fp(&self, index: u32) -> bool {
        index >= self.n_gp
    }
}

impl<M: TargetMachine> AbiTarget for MachineAbiTarget<'_, M> {
    fn isa_name(&self) -> &str {
        self.machine.isa_info().name()
    }

    fn reg_count(&self) -> u32 {
        self.n_gp + self.n_fp
    }

    fn reg_name(&self, index: u32) -> Option<String> {
        self.name_of(index)
    }

    fn reg_class_name(&self, index: u32) -> String {
        if self.is_fp(index) {
            format!("{:?}", self.fpr_class)
        } else {
            format!("{:?}", self.gpr_class)
        }
    }

    /// 名字 → 索引（反向扫一遍；绑定里的 `"RCX"`/`"X10"` 就这样解析）。
    fn reg_index(&self, name: &str) -> Option<u32> {
        (0..self.reg_count()).find(|i| self.name_of(*i).as_deref() == Some(name))
    }

    fn reg_width(&self, index: u32) -> u8 {
        self.reg_at(index)
            .map(|r| (r.width() / 8).max(1) as u8)
            .unwrap_or_else(|| {
                self.machine
                    .reg_info()
                    .reg_class_width(if self.is_fp(index) {
                        self.fpr_class
                    } else {
                        self.gpr_class
                    })
                    .max(1)
            })
    }

    /// 固定用途 = **不在可分配表**里（sp/fp/zero/platform/链接寄存器都由生成器排除）。
    fn pinned(&self, index: u32) -> bool {
        if self.is_fp(index) {
            !self.alloc_fp.contains(&(index - self.n_gp))
        } else {
            !self.alloc_gp.contains(&index)
        }
    }

    fn allocatable(&self) -> Vec<u32> {
        let mut out: Vec<u32> = self.alloc_gp.clone();
        out.extend(self.alloc_fp.iter().map(|i| i + self.n_gp));
        out
    }

    fn spill_scratch(&self) -> Vec<u32> {
        self.scratch.clone()
    }

    /// 链接寄存器：`TargetRegInfo`/`TargetABI` 目前都没暴露 ra/lr（A5 随 `[machine]` 补）。
    fn link_reg(&self) -> Option<u32> {
        None
    }

    /// 能力来自 **ISA 自己的申报**（`TargetMachine::role_bits`，由 DSL 从 `roles` 生成）。
    fn cap(&self, cap: Capability) -> Option<u16> {
        self.machine.role_bits(cap.name())
    }
}

/// 把一个 IR 函数的签名算成 `AbiPlan`（宿主侧唯一入口）。
pub fn plan_for_function<M: TargetMachine>(
    machine: &M,
    registry: &AbiRegistry,
    conv: &str,
    func: &Function,
) -> Result<AbiPlan, AbiError> {
    let target = MachineAbiTarget::new(machine);
    let sig =
        crate::pipeline::sig_view::signature_view(func).map_err(|e| AbiError::Unsupported {
            conv: conv.to_string(),
            what: format!("IR 签名投影失败：{e}"),
        })?;
    registry.plan(&target, conv, &sig)
}

/// 同 [`plan_for_function`]，但签名由调用方直接给（调用点/外部声明的场景）。
pub fn plan_for_signature<M: TargetMachine>(
    machine: &M,
    registry: &AbiRegistry,
    conv: &str,
    sig: &Signature,
) -> Result<AbiPlan, AbiError> {
    let target = MachineAbiTarget::new(machine);
    registry.plan(&target, conv, sig)
}

/// **形状 → `TyView`**（v20 A5-3「调用点 plan」的桥）：运行时侧的中性形状摊成引擎认识的
/// 类型视图。聚合的成员逐个递归——HFA/HVA 判定需要成员（`homogeneous_float_agg`）。
pub fn shape_to_ty(s: &ArgShape) -> forge_abi::TyView {
    use forge_abi::{Elem, TyKind, TyView};
    match &s.kind {
        ShapeKind::Int => TyView::int(s.size, s.align),
        ShapeKind::Ptr => TyView::new(s.size, s.align, TyKind::Ptr),
        ShapeKind::Float => TyView::float(s.size),
        // **用形状里的真实 size/align**：`TyView::vector` 会把 size 算成 `lanes × elem_bytes`，
        // 而调用点未必知道元素宽度/lane 数（生成物只从 IR 类型摊出 size/align/族）——
        // 退化成 size=0 会让引擎把 v256 判成"≤16B 按值向量"，从而漏掉 sret/by-ref。
        ShapeKind::Vector {
            elem_is_float,
            lanes,
            elem_bytes: _,
        } => TyView::new(
            s.size,
            s.align,
            TyKind::Vector {
                elem: if *elem_is_float {
                    Elem::Float
                } else {
                    Elem::Int
                },
                lanes: *lanes,
            },
        ),
        // 用**真实** size/align（`TyView::agg` 会把 size 算成成员之和——packed/尾部填充
        // 的对不上）。
        ShapeKind::Aggregate { members } => TyView::new(
            s.size,
            s.align,
            TyKind::Aggregate {
                members: members.iter().map(shape_to_ty).collect(),
            },
        ),
        ShapeKind::Other => TyView::new(s.size, s.align, TyKind::Other),
    }
}

/// **按 [`CallRequest`]（形状面）算被调方的 plan**（v20 A5-3）：调用点只有形状，没有被调方的
/// `Function`，而落点必须由引擎给——这条入口就是那一步。名字只用于诊断（`a0`/`a1`…）。
///
/// 请求里带三件东西：`args`（实参形状）、`rets`（**全部**返回形状，多个 = 多值返回）、
/// `variadic`（被调方是不是变参、命名几个——调用点自己判不出"未命名实参在只走栈的约定里
/// 要改判到栈"，由宿主查模块签名表给出）。
pub fn plan_for_shapes<M: TargetMachine>(
    machine: &M,
    registry: &AbiRegistry,
    req: &CallRequest,
) -> Result<AbiPlan, AbiError> {
    let params: Vec<(String, forge_abi::TyView)> = req
        .args
        .iter()
        .enumerate()
        .map(|(i, s)| (format!("a{i}"), shape_to_ty(s)))
        .collect();
    let mut sig = Signature::with_rets(params, req.rets.iter().map(shape_to_ty).collect());
    // 被调方是变参 ⇒ 告诉引擎"前 `fixed` 个是命名的"，其余走 `variadic_stack_only` 的规则。
    if let Some((true, fixed)) = req.variadic {
        sig = sig.variadic(fixed as usize);
    }
    plan_for_signature(machine, registry, &req.conv, &sig)
}

/// **给一台具体机器装调用点布局钩子**（`Box::leak` 到进程生命周期；每个 ISA 一次）。
///
/// 生成物在 lowering 时按 `(ISA, CallRequest)` 查它拿**被调方**的落点；未注册 ⇒
/// `plan_call` 返回 [`CallPlanError::NoPlanner`]，调用点 **fail-closed**（不按谱面顺序猜）。
/// 失败时把引擎的原文**原样带出去**（[`CallPlanError::Failed`]）——用户据此能定位到
/// "池不够 / 缺绑定 / 能力缺口"，而不是只看到一句"不可得"。
pub fn register_isa_call_planner<M: TargetMachine + 'static>(isa: &str, machine: M) {
    let tm: &'static M = Box::leak(Box::new(machine));
    forge_isa_runtime::machine::call_plan::register_call_planner(
        isa,
        std::sync::Arc::new(
            move |req: &CallRequest| -> Result<CallLayout, CallPlanError> {
                let reg = crate::pipeline::conv_registry::registry()
                    .read()
                    .map_err(|_| CallPlanError::Failed {
                        conv: req.conv.clone(),
                        why: "约定注册表被投毒（持锁线程 panic）".into(),
                    })?;
                match plan_for_shapes(tm, &reg, req) {
                    Ok(plan) => Ok(call_layout(&plan, tm)),
                    Err(e) => Err(CallPlanError::Failed {
                        conv: req.conv.clone(),
                        why: e.to_string(),
                    }),
                }
            },
        ),
    );
}

/// `AbiPlan` → **运行时侧的中性调用布局**（v20 A3b-2 的桥）。
///
/// 寄存器从"ABI 空间号"折回 **(类, 类内号)**：`0..n_gp` 是 GPR 类、`n_gp..` 是 FPR 类，
/// 生成物用 `Reg::from_index(i, class)` 就能还原成自己的物理寄存器——运行时因此
/// **不需要依赖 forge-abi**，只认这份数据。
pub fn call_layout<M: TargetMachine>(plan: &AbiPlan, machine: &M) -> CallLayout {
    let t = MachineAbiTarget::new(machine);
    let reg = |r: &forge_abi::plan::RegRef| -> (RegClass, u32) {
        // 名字是权威（`RegRef.index` 是 ABI 空间号）。名字解析不到时退回按空间号折算，
        // 但**不猜类**：GPR 区用地址类、FP 区用主 FPR 类。
        if let Some(i) = t.reg_index(&r.name) {
            (
                if t.is_fp(i) { t.fpr_class } else { t.gpr_class },
                if t.is_fp(i) { i - t.n_gp } else { i },
            )
        } else if r.index < t.n_gp {
            (t.gpr_class, r.index)
        } else {
            (t.fpr_class, r.index - t.n_gp)
        }
    };
    let ext = |e: forge_abi::Extension| match e {
        forge_abi::Extension::None => Ext::None,
        forge_abi::Extension::ZeroExt => Ext::Zero,
        forge_abi::Extension::SignExt => Ext::Sign,
    };
    let place = |p: &forge_abi::Placement| -> ArgPlace {
        match p {
            forge_abi::Placement::Reg { reg: r, ext: e, .. } => ArgPlace::Reg {
                class: reg(r).0,
                index: reg(r).1,
                ext: ext(*e),
                sret: false,
            },
            forge_abi::Placement::RegPair { lo, hi } => ArgPlace::Pair {
                lo: reg(lo),
                hi: reg(hi),
            },
            forge_abi::Placement::RegGroup { regs } => ArgPlace::Group {
                regs: regs.iter().map(reg).collect(),
            },
            forge_abi::Placement::Stack {
                offset,
                size,
                align,
            } => ArgPlace::Stack {
                offset: *offset,
                size: *size,
                align: *align,
            },
            forge_abi::Placement::Indirect { ptr, at, on_stack } => ArgPlace::Indirect {
                reg: ptr.as_ref().map(reg),
                at: *at,
                on_stack: *on_stack,
            },
            forge_abi::Placement::Ignore => ArgPlace::Ignore,
        }
    };
    let hidden_sret = plan.hidden.sret.as_ref().map(reg);
    let args: Vec<CallArg> = plan
        .args
        .iter()
        .map(|a| {
            let mut place = place(&a.place);
            // `sret` 语义标在落点上（收参侧据此知道"这个寄存器里是返回缓冲指针"）。
            if let (
                Some(h),
                ArgPlace::Reg {
                    class, index, sret, ..
                },
            ) = (hidden_sret, &mut place)
                && (*class, *index) == h
            {
                *sret = true;
            }
            CallArg {
                index: a.index.map(|i| i as u32),
                size: a.size,
                place,
            }
        })
        .collect();
    let ret = match &plan.ret {
        forge_abi::RetLoc::Void => Some(RetPlace::Void),
        forge_abi::RetLoc::Reg { reg: r } => {
            let (class, index) = reg(r);
            Some(RetPlace::Reg {
                class,
                index,
                ext: Ext::None,
            })
        }
        forge_abi::RetLoc::RegPair { lo, hi } => Some(RetPlace::Pair {
            lo: reg(lo),
            hi: reg(hi),
        }),
        forge_abi::RetLoc::RegGroup { regs } => Some(RetPlace::Group {
            regs: regs.iter().map(reg).collect(),
        }),
        forge_abi::RetLoc::Indirect { size, align } => Some(RetPlace::Indirect {
            size: *size,
            align: *align,
        }),
    };
    CallLayout {
        conv: plan.conv.clone(),
        variadic: plan.variadic,
        args,
        ret,
        hidden_sret,
        stack_align: plan.stack.align,
        slot_bytes: plan.stack.slot_bytes,
        shadow_bytes: plan.stack.shadow_bytes,
        first_arg_offset: plan.stack.first_arg_offset,
        arg_area_bytes: plan.stack.arg_area_bytes,
        byval_area_bytes: plan.stack.byval_area_bytes,
        red_zone: plan.stack.red_zone,
        callee_saved: plan.callee_saved.regs.iter().map(reg).collect(),
        clobbers: plan.clobbers.iter().map(reg).collect(),
        frame_padding: plan.stack.frame_padding,
        callee_pop_bytes: plan.callee_pop_bytes,
        widen_to_bits: plan.widen_to_bits,
    }
}
