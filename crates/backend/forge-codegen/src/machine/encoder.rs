//! TargetEncoder — 单指令二进制编码接口。

use crate::AllocResult; // 类型别名 → AllocResult
use crate::MachineInst;

/// 编码错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum EncodeError {
    #[error("encode not supported")]
    Unsupported,
    #[error("{0}")]
    Other(String),
}

/// 单指令编码器。
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

    fn encoded_size(&self, inst: &Self::Inst) -> Result<usize, EncodeError> {
        let mut rm = crate::pipeline::alloc_result::AllocResult::dummy_for_sizing(16);
        for &vreg in inst.uses().iter().chain(inst.defs().iter()) {
            let xreg = forge_ir::XReg::new(vreg, forge_ir::RegClass::GPR64);
            if !rm.assignments.contains_key(&xreg) {
                rm.insert(
                    xreg,
                    forge_ir::PReg::new(vreg % 16, forge_ir::RegClass::GPR64),
                );
            }
        }
        self.encode_to_bytes(inst, &rm).map(|v| v.len())
    }

    fn encode_into(
        &self,
        inst: &Self::Inst,
        buf: &mut [u8],
        reg_map: &AllocResult,
    ) -> Result<usize, EncodeError> {
        let bytes = self.encode_to_bytes(inst, reg_map)?;
        let len = bytes.len();
        if buf.len() < len {
            return Err(EncodeError::Other("buffer too small".into()));
        }
        buf[..len].copy_from_slice(&bytes);
        Ok(len)
    }
}
