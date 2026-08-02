//! TargetSimulator — 指令模拟执行接口。
//!
//! 可选组件：ISA 支持指令级模拟执行时实现。
//! 模拟器需要具体 State 类型，通过泛型方法或具体实现处理。

/// 模拟器状态 trait。
pub trait SimulationState: std::fmt::Debug + Clone + Default {
    fn read_reg(&self, reg: u8) -> u64;
    fn write_reg(&mut self, reg: u8, val: u64);
    #[allow(clippy::result_unit_err)]
    fn read_mem(&self, addr: u64, size: u8) -> Result<u64, ()>;
    #[allow(clippy::result_unit_err)]
    fn write_mem(&mut self, addr: u64, size: u8, val: u64) -> Result<(), ()>;
}

/// 模拟错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum SimError {
    #[error("simulation not supported")]
    Unsupported,
    #[error("unimplemented instruction")]
    Unimplemented,
    #[error("{0}")]
    Other(String),
}

/// 指令模拟器。
///
/// ISA 实现者应实现 `step()` 方法，使用具体状态类型。
/// 框架通过具体类型而非 `dyn Trait` 使用模拟器。
pub trait TargetSimulator: Send + Sync + 'static {
    type Inst: super::inst::MachineInst;

    /// 获取指令执行的效果描述。
    fn describe_effect(&self, inst: &Self::Inst) -> String {
        format!("{:?}", inst)
    }

    /// 是否支持此指令的模拟。
    fn is_supported(&self, _inst: &Self::Inst) -> bool {
        true
    }
}
