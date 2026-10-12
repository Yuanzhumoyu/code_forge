//! TargetEncoder — 单指令二进制编码接口。

use crate::AllocResult;

/// 编码错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum EncodeError {
    #[error("encode not supported")]
    Unsupported,
    #[error("{0}")]
    Other(String),
}

/// 单指令编码器。
///
/// **契约只有两条**（v23 M3 收敛）：把一条机器指令编进 `CodeSink`，以及"编成 `Vec<u8>`"
/// 这个由前者派生的便利方法。
///
/// 删掉的两个方法（`encoded_size`/`encode_into`，2026-10-13）是**死 API**：
/// 它们只为"预先算长度再编进调用方缓冲"存在，而调用方（`emission.rs`）一直用
/// `encode()` + `CodeSink`——`encoded_size` 还得伪造一份 `AllocResult::dummy_for_sizing`
/// （写死 `RegClass::GPR64`、`% 16` 编号）才能跑，等于把"分配器的事实"在编码器里重编一遍。
pub trait TargetEncoder: Send + Sync + 'static {
    type Inst: super::inst::MachineInst;

    fn encode(
        &self,
        inst: &Self::Inst,
        reg_map: &AllocResult,
        sink: &mut crate::CodeSink,
    ) -> Result<(), EncodeError>;

    fn encode_to_bytes(
        &self,
        inst: &Self::Inst,
        reg_map: &AllocResult,
    ) -> Result<Vec<u8>, EncodeError> {
        let mut sink = crate::CodeSink::new();
        self.encode(inst, reg_map, &mut sink)?;
        Ok(sink.bytes().to_vec())
    }
}
