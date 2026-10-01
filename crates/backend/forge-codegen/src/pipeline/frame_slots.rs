//! **前端局部槽的预扫描**（v20 V3 抽出）：`StackAddr` 区与 `Alloca` 区的深度账。
//!
//! 这件事有**两个消费者**，而且必须给出同一个答案：
//!
//! 1. `pipeline/lowering.rs`——它据此给 `Alloca` 分具体槽位、并把深度并进 `max_stack_bytes`
//!    （帧尺寸）；
//! 2. `pipeline/va_expand.rs`——变参的 ABI 槽（寄存器保存区 + `va_list` 对象）必须排在这些
//!    前端局部槽**之下**，否则两者重叠（保存区被局部变量覆盖 ⇒ `va_arg` 读出垃圾）。
//!
//! 两处各写一遍就等于"两份实现要对齐"，迟早漂移（本文件落地前 `va_expand` 里就是一份镜像）。
//! 所以这里是**唯一实现**：三部分都要扫到，
//!
//! - `StackAddr` 的**立即数**偏移（`-v + slot_bytes`，v ≥ 0 不计）；
//! - `Iadd(stack_addr(0), iconst(-N))` 模式（mini_c 的 `alloc_slot` 把真偏移放在 iconst 里，
//!   StackAddr 本身的立即数是 0——漏了这条 locals 区就不参与帧计算）；
//! - `Alloca`（从"StackAddr 区底 + 一个槽单位"起连续向下分配）。

use forge_ir::ir::dfg::ValueDef;
use forge_ir::*;

/// 预扫描结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FrontendSlots {
    /// `StackAddr` 区深度（字节，正值；含 `-v + slot_bytes`）。
    pub stackaddr_depth: i64,
    /// `Alloca` 槽（指令 + 字节数，按 IR 序）。
    pub allocas: Vec<(Inst, u32)>,
}

impl FrontendSlots {
    /// 这些槽**覆盖到的最深字节**（ABI 槽该从这里往下排；也是帧尺寸的 locals 需求）。
    pub(crate) fn depth_bytes(&self, slot_bytes: u32) -> u32 {
        let unit = i64::from(slot_bytes.max(1));
        let mut slot = -(self.stackaddr_depth + unit);
        for (_, bytes) in &self.allocas {
            slot -= ((i64::from(*bytes) + unit - 1) / unit) * unit;
        }
        (-slot).max(self.stackaddr_depth).max(0) as u32
    }
}

/// 扫一遍 IR（唯一实现；`size_of` 由调用方给——lowering 用类型快照、展开用函数自己的上下文）。
pub(crate) fn scan_frontend_slots(
    func: &Function,
    slot_bytes: u32,
    size_of: impl Fn(TypeId) -> u32,
) -> FrontendSlots {
    let unit = i64::from(slot_bytes.max(1));
    let mut stackaddr_depth: i64 = 0;
    // ② `Iadd(StackAddr, Iconst<0)`
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = func.dfg.inst_data(ii);
            if inst.opcode != Opcode::Iadd {
                continue;
            }
            let (mut has_stack, mut has_const, mut cval) = (false, false, 0i64);
            for &op in &inst.operands {
                let Some(val) = func.dfg.value_data_opt(op) else {
                    continue;
                };
                let ValueDef::Inst(def_ii, _) = val.def else {
                    continue;
                };
                let def = func.dfg.inst_data(def_ii);
                match def.opcode {
                    Opcode::StackAddr => has_stack = true,
                    Opcode::Iconst => {
                        if let Some(Immediate::Const(cid)) = def.immediates.first()
                            && let Some((v, _)) = func.constants.get_int(*cid)
                        {
                            cval = v as i64;
                            has_const = true;
                        }
                    }
                    _ => {}
                }
            }
            if has_stack && has_const && cval < 0 {
                stackaddr_depth = stackaddr_depth.max(-cval + unit);
            }
        }
    }
    // ① 立即数 + ③ Alloca
    let mut allocas: Vec<(Inst, u32)> = Vec::new();
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = func.dfg.inst_data(ii);
            match inst.opcode {
                Opcode::StackAddr => {
                    if let Some(Immediate::Int(v)) = inst.immediates.first()
                        && *v < 0
                    {
                        stackaddr_depth = stackaddr_depth.max(-*v + unit);
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
                        let bytes =
                            (u64::from(size_of(t).max(1)) * count).min(u32::MAX as u64) as u32;
                        allocas.push((ii, bytes));
                    }
                }
                _ => {}
            }
        }
    }
    FrontendSlots {
        stackaddr_depth,
        allocas,
    }
}
