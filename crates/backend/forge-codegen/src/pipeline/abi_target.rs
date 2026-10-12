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
use forge_isa_runtime::machine::call_layout::{VaInfo, VaInit, VaInitVal};
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

    /// 寄存器类：整数区用地址类、浮点区用主 FPR 类（`n_gp` 是两者的分界）。
    fn reg_class(&self, index: u32) -> RegClass {
        if self.is_fp(index) {
            self.fpr_class
        } else {
            self.gpr_class
        }
    }

    /// 类内号：整数类 = ABI 空间号；浮点类 = 减去整数区宽度。
    fn reg_slot(&self, index: u32) -> u32 {
        if self.is_fp(index) {
            index.saturating_sub(self.n_gp)
        } else {
            index
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

/// **按 [`CallRequest`]（形状面）算被调方的 plan**（v20 A5-3）：调用点只有形状，没有被调方的
/// `Function`，而落点必须由引擎给——这条入口就是那一步。名字只用于诊断（`a0`/`a1`…）。
///
/// 请求里带三件东西：`args`（实参形状）、`rets`（**全部**返回形状，多个 = 多值返回）、
/// `variadic`（被调方是不是变参、命名几个——调用点自己判不出"未命名实参在只走栈的约定里
/// 要改判到栈"，由宿主查模块签名表给出）。
///
/// 形状就是引擎的 [`TyView`](forge_abi::TyView)：**不再有"运行时形状 → 引擎视图"的桥**
/// （v23 M2——那份桥曾是第二份投影，退化策略与被调方一侧不一致会让 HFA 两边分叉）。
pub fn plan_for_shapes<M: TargetMachine>(
    machine: &M,
    registry: &AbiRegistry,
    req: &CallRequest,
) -> Result<AbiPlan, AbiError> {
    let params: Vec<(String, forge_abi::TyView)> = req
        .args
        .iter()
        .enumerate()
        .map(|(i, s)| (format!("a{i}"), s.clone()))
        .collect();
    let mut sig = Signature::with_rets(params, req.rets.clone());
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
        std::sync::Arc::new(move |req: &CallRequest| -> Result<AbiPlan, CallPlanError> {
            let reg = crate::pipeline::conv_registry::registry()
                .read()
                .map_err(|_| CallPlanError::Failed {
                    conv: req.conv.clone(),
                    why: "约定注册表被投毒（持锁线程 panic）".into(),
                })?;
            match plan_for_shapes(tm, &reg, req) {
                Ok(plan) => Ok(plan),
                Err(e) => Err(CallPlanError::Failed {
                    conv: req.conv.clone(),
                    why: e.to_string(),
                }),
            }
        }),
    );
}

/// `AbiPlan` → **宿主侧的变参外壳**（v23 M2）。
///
/// 发射端现在**直接读 `AbiPlan`**（plan 就是运行时与生成物共用的那份数据），这里只补
/// 生成物算不出来的东西：`va_list` 对象的**字段初值**（"已用掉几个参数寄存器 / 溢出区
/// 从哪开始"只有编译入口知道）。此前这里是整个 `AbiPlan → CallLayout` 的翻译（~260 行），
/// 现在只剩这一件事。
pub fn va_info<M: TargetMachine>(plan: &AbiPlan, machine: &M) -> Option<VaInfo> {
    let area = plan.va_area.as_ref()?;
    Some(VaInfo {
        shape: area.shape.clone(),
        size: area.size,
        align: area.align,
        stack_only: area.stack_only,
        save_contiguous: area.save_contiguous,
        fields: area.fields.clone(),
        save: area.save.clone(),
        init: va_init(plan, machine, area),
        arg_rules: area.arg_rules,
    })
}

/// **`va_list` 对象的初值**（v20 变参 V3/V6）：宿主按本函数的 plan + **取参规则**
/// （`arg_rules`）逐字段算好——生成物是各约定通用的，算不出"已用掉几个参数寄存器 /
/// 溢出区从哪开始"。
///
/// **不给任何约定开分支**：每个字段的值由它在规则里的**角色**决定——
///
/// - **游标**（`int.cursor`/`float.cursor`）：规则有基址（计数式）⇒ 初值 = 零点 ± 步长 ×
///   已用槽数（"从区域顶端往下数"的形态用负值）；没有基址（地址式）⇒ 初值 = 未命名实参区地址；
/// - **基址**（`base`）：保存区基址 + （往下数式 ⇒ 本类区域字节数；否则 0）；
/// - **溢出区**（`overflow`）：未命名实参区地址；
/// - 其余字段（该约定没用到）：写 0。
///
/// 未命名实参区起点（被调方视角、相对帧基址）：**有命名栈实参 ⇒ 最后一个栈槽之后**
/// （该 `offset` 已含 shadow，见 `forge_abi::engine::stack_place`）；**没有 ⇒
/// `first_arg_offset + shadow_bytes`——两个都要加**（少加 shadow 会读到 shadow 区里的垃圾）。
pub fn va_init<M: TargetMachine>(
    plan: &AbiPlan,
    machine: &M,
    area: &forge_abi::plan::VaArea,
) -> Option<VaInit> {
    let t = MachineAbiTarget::new(machine);
    let unnamed_off = plan
        .args
        .iter()
        .filter_map(|a| match &a.place {
            forge_abi::Placement::Stack { offset, size, .. } => {
                Some(i64::from(*offset) + i64::from(*size))
            }
            _ => None,
        })
        .max()
        .unwrap_or_else(|| {
            i64::from(plan.stack.first_arg_offset) + i64::from(plan.stack.shadow_bytes)
        });
    // 命名实参里**走寄存器**的个数（按类分）——计数式游标的初值要用。
    let (mut used_int, mut used_fp) = (0u64, 0u64);
    for a in &plan.args {
        if let forge_abi::Placement::Reg { reg: r, .. } = &a.place {
            let is_fp = t
                .reg_index(&r.name)
                .map(|i| t.is_fp(i))
                .unwrap_or(r.index >= t.n_gp);
            if is_fp {
                used_fp += 1;
            } else {
                used_int += 1;
            }
        }
    }
    // 保存区里**整数类的字节数**（= 浮点区起点）：槽表由引擎按"整数池在前"排，
    // 所以前导同类槽的长度就是它（不写死槽数/槽宽）。
    let int_region_end: u32 = area
        .save
        .as_ref()
        .map(|s| {
            s.slots
                .iter()
                .take_while(|sl| {
                    !t.reg_index(&sl.reg.name)
                        .map(|i| t.is_fp(i))
                        .unwrap_or(false)
                })
                .map(|sl| sl.size)
                .sum()
        })
        .unwrap_or(0);
    let save_end = area.save.as_ref().map(|s| s.size).unwrap_or(0);
    let r = &area.arg_rules;
    // 计数式游标的初值：`零点 ± 步长 × 已用槽数`（往下数式 = 负值，补码写进字段宽度）。
    let cursor_init = |rule: &forge_abi::plan::VaArgRule, used: u64| -> i64 {
        let span = i64::from(rule.step) * used as i64;
        if rule.cursor_counts_down {
            -span
        } else {
            i64::from(rule.cursor_origin) + span
        }
    };
    let mut fields = Vec::with_capacity(area.fields.len());
    for i in 0..area.fields.len() {
        let is_int_cursor = i == r.int.cursor;
        let is_fp_cursor = i == r.float.cursor;
        let val = if is_int_cursor {
            if r.int.base.is_some() {
                VaInitVal::Imm(cursor_init(&r.int, used_int) as u64)
            } else if area.save_contiguous {
                // **地址式游标 + 要求连续**（v20 V7，RISC-V）：`va_list` 指向"第一个还没被
                // 命名形参用掉的整数参数寄存器"在保存区里的位置——保存区是"全 8 个寄存器"
                // 的连续一份（`[入口 sp - save_size, 入口 sp)`），于是游标从保存区起点偏移
                // `已用整数寄存器数 × 槽宽`，走到末尾正好接上调用方写在栈上的实参。
                VaInitVal::SaveOff(used_int.saturating_mul(u64::from(r.int.step)) as u32)
            } else {
                VaInitVal::FrameOff(unnamed_off)
            }
        } else if is_fp_cursor {
            if r.float.base.is_some() {
                VaInitVal::Imm(cursor_init(&r.float, used_fp) as u64)
            } else if area.save_contiguous {
                // 连续的形态里浮点游标与整数游标是**同一条**（变参按整数约定传）：偏移按
                // 整数侧算（`step` 与整数侧相同，这里仍用 `used_int`）。
                VaInitVal::SaveOff(used_int.saturating_mul(u64::from(r.int.step)) as u32)
            } else {
                VaInitVal::FrameOff(unnamed_off)
            }
        } else if Some(i) == r.int.base {
            VaInitVal::SaveOff(if r.int.cursor_counts_down {
                int_region_end
            } else {
                0
            })
        } else if Some(i) == r.float.base {
            VaInitVal::SaveOff(if r.float.cursor_counts_down {
                save_end
            } else {
                0
            })
        } else if Some(i) == r.int.overflow || Some(i) == r.float.overflow {
            VaInitVal::FrameOff(unnamed_off)
        } else {
            VaInitVal::Imm(0)
        };
        fields.push(val);
    }
    Some(VaInit { fields })
}
