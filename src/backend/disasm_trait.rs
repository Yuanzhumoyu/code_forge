//! Disassembler trait — 独立的反汇编接口。

use crate::backend::isa_info::IsaInfo;
use crate::backend::machine_inst::MachineInst;

/// 反汇编器 trait — 可选的独立反汇编组件。
pub trait Disassembler: IsaInfo {
    /// 机器指令类型。
    type Inst: MachineInst;

    /// 反汇编：指令 → 汇编文本。
    fn disassemble(inst: &Self::Inst) -> String;

    /// 带地址的反汇编。
    fn disassemble_at(inst: &Self::Inst, address: u64) -> String {
        let _ = address;
        Self::disassemble(inst)
    }

    /// 反汇编并附带指令字节（hex）。
    fn disassemble_with_bytes(inst: &Self::Inst, bytes: &[u8]) -> String {
        let hex: String = bytes.iter().map(|b| format!("{:02X} ", b)).collect();
        format!("  {:24}{}", hex, Self::disassemble(inst))
    }

    /// 带完整上下文的反汇编（地址 + 字节 + 汇编）。
    fn disassemble_full(
        inst: &Self::Inst,
        address: u64,
        bytes: &[u8],
    ) -> String {
        let hex: String = bytes.iter().map(|b| format!("{:02X}", b)).collect();
        format!("{:08X}: {:20}  {}", address, hex, Self::disassemble_at(inst, address))
    }
}
