//! 编码基元（Encoding Primitives）— x86-64 / 通用位域。
//!
//! 被 bitstring.rs 原语和 lowering 管线调用。
//!
//! - **pack_bits** / **pack_bits_label** — 通用位域打包 (packer.rs)
//! - **modrm** / **sib** — x86 ModR/M, SIB 字节构造
//! - **enc_*** — 被 @sse_op / @lea_sib / @imul_r64_rm64 等原语调用

pub mod byte_encoder;
pub mod macros;
pub mod packer;

pub use byte_encoder::ByteEncoder;
pub use packer::*;

use crate::backend::emit::CodeSink;

// ============================================================
// ModR/M, SIB
// ============================================================

/// 构造 ModR/M 字节: mod(2) | reg(3) | rm(3)
pub fn modrm(mod_bits: u8, reg: u8, rm: u8) -> u8 {
    (mod_bits << 6) | ((reg & 0x7) << 3) | (rm & 0x7)
}

/// 构造 SIB 字节: scale(2) | index(3) | base(3)
pub fn sib(scale: u8, index: u8, base: u8) -> u8 {
    let scale_bits = match scale { 1 => 0, 2 => 1, 4 => 2, 8 => 3, _ => 0 };
    (scale_bits << 6) | ((index & 0x7) << 3) | (base & 0x7)
}

// ============================================================
// REX prefix helpers
// ============================================================

/// REX prefix flags. Only the rex path is active; VEX/EVEX removed.
#[derive(Default, Clone, Copy)]
pub struct EncFlags { pub w: bool, pub r: bool, pub x: bool, pub b: bool }

/// 发射 REX.W 前缀（64-bit 操作数大小）。
pub fn emit_rex_w(sink: &mut CodeSink, rd: u8, rs: u8) {
    let mut rex = 0x48u8;
    if (rd & 0x8) != 0 { rex |= 0x04; }
    if (rs & 0x8) != 0 { rex |= 0x01; }
    sink.put1(rex);
}

/// 发射 REX 前缀（含 W/R/X/B 标志位）。
pub fn emit_rex(sink: &mut CodeSink, f: EncFlags) {
    let mut rex = 0x40u8;
    if f.w { rex |= 0x08; }
    if f.r { rex |= 0x04; }
    if f.x { rex |= 0x02; }
    if f.b { rex |= 0x01; }
    if rex != 0x40 || f.w { sink.put1(rex); }
}

/// 发射 push reg（REX.B + 50+rd）。Called by byte_encoder.
pub fn emit_push_reg(sink: &mut CodeSink, reg: u8) {
    if reg >= 8 { sink.put1(0x41); }
    sink.put1(0x50 + (reg & 0x7));
}

/// 发射 pop reg（REX.B + 58+rd）。Called by byte_encoder.
pub fn emit_pop_reg(sink: &mut CodeSink, reg: u8) {
    if reg >= 8 { sink.put1(0x41); }
    sink.put1(0x58 + (reg & 0x7));
}

/// 发射带位移的 ModR/M，自动选择 disp8/disp32。
fn emit_modrm_disp(sink: &mut CodeSink, reg: u8, base: u8, offset: i32) {
    const RBP: u8 = 5;
    if offset == 0 && (base & 0x7) != RBP {
        sink.put1(modrm(0, reg, base & 0x7));
    } else if (-128..=127).contains(&offset) {
        sink.put1(modrm(1, reg, base & 0x7));
        sink.put1(offset as u8);
    } else {
        sink.put1(modrm(2, reg, base & 0x7));
        sink.put4(offset as u32);
    }
}

// ============================================================
// Alive encoding functions — called by bitstring.rs primitives
// ============================================================

/// REX + 0F + opcode + ModRM(mod=3, reg, rm). Called by @imul_r64_rm64.
pub fn enc_rr_0f(sink: &mut CodeSink, opcode: u8, reg: u8, rm: u8, w: bool) {
    let base: u8 = if w { 0x48 } else { 0x40 };
    let rex = base | if reg >= 8 { 0x04 } else { 0 } | if rm >= 8 { 0x01 } else { 0 };
    if rex != 0x40 || w { sink.put1(rex); }
    sink.put1(0x0F);
    sink.put1(opcode);
    sink.put1(modrm(3, reg, rm));
}

/// SSE reg-reg: mandatory_prefix + REX + 0F + opcode + ModRM. Called by @sse_op.
pub fn enc_sse_rr(sink: &mut CodeSink, prefix: u8, opcode: u8, w: bool, reg: u8, rm: u8) {
    sink.put1(prefix);
    let base: u8 = if w { 0x48 } else { 0x40 };
    let rex = base | if reg >= 8 { 0x04 } else { 0 } | if rm >= 8 { 0x01 } else { 0 };
    if rex != 0x40 || w { sink.put1(rex); }
    sink.put1(0x0F);
    sink.put1(opcode);
    sink.put1(modrm(3, reg, rm));
}

/// LEA with SIB: REX.W + 0x8D + ModRM + SIB + disp. Called by @lea_sib.
pub fn enc_lea_sib(sink: &mut CodeSink, rd: u8, base: u8, index: u8, scale: u8, offset: i32) {
    emit_rex(sink, EncFlags { w: true, r: (rd & 0x8) != 0, x: (index & 0x8) != 0, b: (base & 0x8) != 0 });
    sink.put1(0x8D);
    const RBP: u8 = 5;
    let need_disp = offset != 0 || (base & 0x7) == RBP;
    if !need_disp {
        sink.put1(modrm(0, rd, 0x04));
        sink.put1(sib(scale, index, base));
    } else if (-128..=127).contains(&offset) {
        sink.put1(modrm(1, rd, 0x04));
        sink.put1(sib(scale, index, base));
        sink.put1(offset as u8);
    } else {
        sink.put1(modrm(2, rd, 0x04));
        sink.put1(sib(scale, index, base));
        sink.put4(offset as u32);
    }
}

/// Spill load: MOV rd, [rbp+offset]. Called by lowering.
pub fn enc_spill_load(sink: &mut CodeSink, dst: u8, offset: i32) {
    const RBP: u8 = 5;
    emit_rex_w(sink, dst, RBP);
    sink.put1(0x8B);
    emit_modrm_disp(sink, dst, RBP, offset);
}

/// Spill store: MOV [rbp+offset], src. Called by lowering.
pub fn enc_spill_store(sink: &mut CodeSink, src: u8, offset: i32) {
    const RBP: u8 = 5;
    emit_rex_w(sink, src, RBP);
    sink.put1(0x89);
    emit_modrm_disp(sink, src, RBP, offset);
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modrm_encoding() {
        assert_eq!(modrm(3, 0, 0), 0xC0);
        assert_eq!(modrm(3, 1, 2), 0xCA);
        assert_eq!(modrm(0, 0, 4), 0x04);
    }

    #[test]
    fn test_rex_w_encoding() {
        let mut sink = CodeSink::new();
        emit_rex_w(&mut sink, 8, 0);
        assert_eq!(sink.bytes()[0], 0x4C);
    }

    #[test]
    fn test_enc_lea_sib() {
        let mut sink = CodeSink::new();
        enc_lea_sib(&mut sink, 0, 1, 2, 1, 0);
        let bytes = sink.bytes();
        assert_eq!(bytes[0], 0x48);
        assert_eq!(bytes[1], 0x8D);
        assert_eq!(bytes[2], 0x04);
        assert_eq!(bytes[3], 0x11); // SIB: index=2, base=1
    }

    #[test]
    fn test_enc_spill_load_store() {
        let mut sink = CodeSink::new();
        enc_spill_load(&mut sink, 11, -8);
        let bytes = sink.bytes();
        assert_eq!(bytes.len(), 4);
        assert_eq!(bytes[0], 0x4C);
        assert_eq!(bytes[1], 0x8B);
        assert_eq!(bytes[2], 0x5D);
        assert_eq!(bytes[3], 0xF8); // disp8 = -8

        let mut sink2 = CodeSink::new();
        enc_spill_store(&mut sink2, 11, -16);
        let bytes2 = sink2.bytes();
        assert_eq!(bytes2.len(), 4);
        assert_eq!(bytes2[0], 0x4C);
        assert_eq!(bytes2[1], 0x89);
        assert_eq!(bytes2[2], 0x5D);
        assert_eq!(bytes2[3], 0xF0); // disp8 = -16
    }
}
