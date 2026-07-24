//! Simulator trait — 独立的指令模拟接口。

use crate::backend::isa_info::IsaInfo;
use crate::backend::machine_inst::MachineInst;
use crate::backend::instruction_set::SimError;
use crate::backend::instruction_set::SimulationState;

/// 模拟器 trait — 可选的独立模拟执行组件。
///
/// 提供 ISA 指令的模拟执行能力。由 DSL 编译器根据结构化语义生成，
/// 或由手动 ISA 实现。
pub trait Simulator: IsaInfo {
    /// 机器指令类型。
    type Inst: MachineInst;

    /// 模拟器状态类型（包含寄存器、内存等）。
    type State: SimulationState;

    /// 单步执行一条指令。
    fn step(state: &mut Self::State, inst: &Self::Inst) -> Result<(), SimError>;

    /// 批量执行指令序列。
    fn run(state: &mut Self::State, insts: &[Self::Inst]) -> Result<(), SimError> {
        for inst in insts {
            Self::step(state, inst)?;
        }
        Ok(())
    }

    /// 获取指令执行的效果（副作用）描述。
    fn describe_effect(inst: &Self::Inst) -> String {
        format!("{:?}", inst)
    }

    /// 是否支持此指令的模拟。
    fn is_supported(inst: &Self::Inst) -> bool {
        let _ = inst;
        true
    }
}
