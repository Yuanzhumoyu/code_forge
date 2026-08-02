//! TargetDisassembler — 指令 → 文本反汇编接口。
//!
//! 可选组件：ISA 支持将机器指令转为人类可读的汇编文本时实现。

/// 反汇编器。
pub trait TargetDisassembler: Send + Sync + 'static {
    type Inst: super::inst::MachineInst;

    /// 反汇编：指令 → 汇编文本。
    fn disassemble(&self, inst: &Self::Inst) -> String;

    /// 带地址的反汇编。
    fn disassemble_at(&self, inst: &Self::Inst, address: u64) -> String {
        let _ = address;
        self.disassemble(inst)
    }

    /// 完整反汇编（地址 + 字节 + 汇编文本）。
    fn disassemble_full(&self, inst: &Self::Inst, address: u64, bytes: &[u8]) -> String {
        let hex: String = bytes.iter().map(|b| format!("{:02X}", b)).collect();
        format!(
            "{:08X}: {:20}  {}",
            address,
            hex,
            self.disassemble_at(inst, address)
        )
    }
}
