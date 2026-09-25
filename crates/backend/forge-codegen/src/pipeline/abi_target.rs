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
