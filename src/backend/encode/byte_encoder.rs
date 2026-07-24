//! `ByteEncoder` — x86/CISC 变长编码构建器。
//!
//! 提供链式 API 用于组合 REX 前缀、操作码、ModRM、SIB、位移和立即数。
//! 每个方法调用直接发射字节到 CodeSink（无中间缓冲）。
#![allow(unused_mut)]

use crate::RelocKind;
use crate::backend::emit::CodeSink;
use crate::ir::BlockId;

use super::{EncFlags, emit_modrm_disp, emit_pop_reg, emit_push_reg, emit_rex, emit_rex_w, modrm};

/// x86 变长指令编码构建器。
pub struct ByteEncoder<'a> {
    sink: &'a mut CodeSink,
    count: usize,
}

#[allow(unused_mut)]
impl<'a> ByteEncoder<'a> {
    pub fn new(sink: &'a mut CodeSink) -> Self {
        let cur = sink.offset();
        Self { sink, count: cur }
    }

    pub fn bytes_emitted(&self) -> usize {
        self.sink.offset() - self.count
    }

    pub fn rex(mut self, flags: EncFlags) -> Self {
        emit_rex(self.sink, flags);
        self
    }

    pub fn rex_w_regs(mut self, r_reg: u8, b_reg: u8) -> Self {
        emit_rex_w(self.sink, r_reg, b_reg);
        self
    }

    pub fn rex_sse_regs(mut self, rd: u8, rs: u8) -> Self {
        let rex = 0x40u8
            | if (rd & 0x8) != 0 { 0x04 } else { 0 }
            | if (rs & 0x8) != 0 { 0x01 } else { 0 };
        if rex != 0x40 {
            self.sink.put1(rex);
        }
        self
    }

    pub fn opcode(mut self, bytes: &[u8]) -> Self {
        self.sink.put_bytes(bytes);
        self
    }

    pub fn opcode1(mut self, byte: u8) -> Self {
        self.sink.put1(byte);
        self
    }

    pub fn opcode_plus_reg(mut self, base: u8, reg: u8) -> Self {
        self.sink.put1(base | (reg & 0x7));
        self
    }

    pub fn modrm_direct(mut self, reg: u8, rm: u8) -> Self {
        self.sink.put1(modrm(3, reg, rm));
        self
    }

    pub fn modrm_disp(mut self, reg: u8, base: u8, offset: i32) -> Self {
        emit_modrm_disp(self.sink, reg, base, offset);
        self
    }

    pub fn modrm_sib(mut self, reg: u8) -> Self {
        self.sink.put1(modrm(0, reg, 0x04));
        self
    }

    pub fn sib(mut self, scale: u8, index: u8, base: u8) -> Self {
        let s = match scale {
            1 => 0,
            2 => 1,
            4 => 2,
            8 => 3,
            _ => 0,
        };
        self.sink
            .put1(((s & 0x3) << 6) | ((index & 0x7) << 3) | (base & 0x7));
        self
    }

    pub fn imm8(mut self, v: u8) -> Self {
        self.sink.put1(v);
        self
    }
    pub fn imm32(mut self, v: u32) -> Self {
        self.sink.put4(v);
        self
    }
    pub fn imm64(mut self, v: u64) -> Self {
        self.sink.put8(v);
        self
    }

    pub fn rel32_label(mut self, target: BlockId) -> Self {
        let off = self.sink.offset();
        self.sink.put4(0u32);
        self.sink.use_label_at(off, target, RelocKind::REL4);
        self
    }

    pub fn rel32_symbol(mut self, symbol: &str) -> Self {
        let off = self.sink.offset();
        self.sink.put4(0u32);
        self.sink.add_reloc(off, RelocKind::CALL, symbol, 0);
        self
    }

    pub fn abs64_label(mut self, target: BlockId) -> Self {
        let off = self.sink.offset();
        self.sink.put8(0u64);
        self.sink.use_label_at(off, target, RelocKind::ABS8);
        self
    }

    pub fn push_reg(mut self, reg: u8) -> Self {
        emit_push_reg(self.sink, reg);
        self
    }
    pub fn pop_reg(mut self, reg: u8) -> Self {
        emit_pop_reg(self.sink, reg);
        self
    }

    pub fn emit(self) -> usize {
        self.bytes_emitted()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::emit::CodeSink;

    #[test]
    fn test_mov_rr() {
        let mut sink = CodeSink::new();
        ByteEncoder::new(&mut sink)
            .rex_w_regs(0, 1)
            .opcode(&[0x89])
            .modrm_direct(1, 0)
            .emit();
        assert_eq!(sink.bytes(), &[0x48, 0x89, 0xC8]);
    }

    #[test]
    fn test_mov_ri64() {
        let mut sink = CodeSink::new();
        ByteEncoder::new(&mut sink)
            .rex_w_regs(0, 0)
            .opcode_plus_reg(0xB8, 0)
            .imm64(42)
            .emit();
        let b = sink.bytes();
        assert_eq!(b.len(), 10);
        assert_eq!(u64::from_le_bytes(b[2..10].try_into().unwrap()), 42);
    }

    #[test]
    fn test_mov_rm() {
        let mut sink = CodeSink::new();
        ByteEncoder::new(&mut sink)
            .rex_w_regs(0, 5)
            .opcode(&[0x8B])
            .modrm_disp(0, 5, -8)
            .emit();
        assert_eq!(sink.bytes().len(), 4);
    }

    #[test]
    fn test_sse_addsd() {
        let mut sink = CodeSink::new();
        ByteEncoder::new(&mut sink)
            .rex_sse_regs(0, 1)
            .opcode(&[0xF2, 0x0F, 0x58])
            .modrm_direct(0, 1)
            .emit();
        assert_eq!(&sink.bytes()[0..3], &[0xF2, 0x0F, 0x58]);
    }

    #[test]
    fn test_call_rel32() {
        let mut sink = CodeSink::new();
        ByteEncoder::new(&mut sink)
            .opcode1(0xE8)
            .rel32_symbol("f")
            .emit();
        assert_eq!(sink.relocations()[0].symbol, "f");
    }

    #[test]
    fn test_jmp_rel32() {
        let mut sink = CodeSink::new();
        ByteEncoder::new(&mut sink)
            .opcode1(0xE9)
            .rel32_label(BlockId(1))
            .emit();
        assert_eq!(sink.bytes().len(), 5);
    }

    #[test]
    fn test_lea_sib() {
        let mut sink = CodeSink::new();
        ByteEncoder::new(&mut sink)
            .rex_w_regs(0, 2)
            .opcode1(0x8D)
            .modrm_sib(0)
            .sib(1, 1, 2)
            .emit();
        assert_eq!(sink.bytes().len(), 4);
    }

    #[test]
    fn test_chained_sequence() {
        let mut sink = CodeSink::new();
        ByteEncoder::new(&mut sink)
            .rex_w_regs(0, 1)
            .opcode(&[0x89])
            .modrm_direct(1, 0)
            .emit();
        ByteEncoder::new(&mut sink)
            .rex_w_regs(0, 2)
            .opcode1(0x01)
            .modrm_direct(2, 0)
            .emit();
        assert_eq!(sink.bytes().len(), 6);
    }
}
