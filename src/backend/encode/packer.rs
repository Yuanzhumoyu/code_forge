//! 通用位域打包器 — 服务所有固定宽度 ISA。
//!
//! 提供 `pack_bits()` 和 `pack_bits_label()` 两个核心函数，
//! 将 (value, bit_offset, bit_width) 元组打包为 u16/u32/u64 指令字。
//!
//! # 适用范围
//!
//! - RISC-V (32-bit 定长)
//! - ARM64 / AArch64 (32-bit 定长)
//! - SimpleISA (16-bit 定长)
//! - WASM (变长但可用此原语)
//! - 任何未来的固定宽度 ISA

use crate::RelocKind;
use crate::backend::emit::CodeSink;
use crate::ir::BlockId;

/// 位域描述符 — 一段指令编码中的一个字段。
#[derive(Debug, Clone, Copy)]
pub struct BitField {
    /// 字段值（会被 mask 到 width 位）
    pub value: u64,
    /// 字段在指令字中的起始 bit 位置（0 = LSB）
    pub offset: u8,
    /// 字段宽度（bits）
    pub width: u8,
}

impl BitField {
    /// 创建一个新的位域描述符。
    pub const fn new(value: u64, offset: u8, width: u8) -> Self {
        Self {
            value,
            offset,
            width,
        }
    }
}

/// 将位域列表打包为一个指令字并发射到 CodeSink。
///
/// `total_bits` 决定输出宽度: 8 → put1, 16 → put2, 32 → put4, 64 → put8。
///
/// # Panics (debug only)
///
/// - 如果某个字段超出 total_bits 范围
/// - 如果 total_bits 不是 {8, 16, 32, 64} 之一
pub fn pack_bits(sink: &mut CodeSink, total_bits: u8, fields: &[BitField]) {
    let mut word: u64 = 0;
    for f in fields {
        debug_assert!(
            f.offset + f.width <= total_bits,
            "field at offset {} with width {} exceeds total_bits {}",
            f.offset,
            f.width,
            total_bits
        );
        let mask = if f.width == 64 {
            u64::MAX
        } else {
            (1u64 << f.width) - 1
        };
        word |= (f.value & mask) << f.offset;
    }
    match total_bits {
        8 => sink.put1(word as u8),
        16 => sink.put2(word as u16),
        32 => sink.put4(word as u32),
        64 => sink.put8(word),
        _ => {
            // 非标准宽度：逐字节写入
            let bytes = total_bits.div_ceil(8);
            for i in 0..bytes {
                sink.put1((word >> (i * 8)) as u8);
            }
        }
    }
}

/// 同 `pack_bits`，但额外记录一个标签 fixup（用于分支/跳转指令）。
///
/// `reloc` 指定重定位类型。对于 RISC 风格 ISA（非连续立即数位域），
/// 使用 `RelocKind::Isa(id)` 并在 ISA 端实现 `encode_isa_reloc`。
pub fn pack_bits_label(
    sink: &mut CodeSink,
    total_bits: u8,
    fields: &[BitField],
    target: BlockId,
    reloc: RelocKind,
) {
    let offset = sink.offset();
    pack_bits(sink, total_bits, fields);
    sink.use_label_at(offset, target, reloc);
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::emit::CodeSink;

    #[test]
    fn test_pack_16bit() {
        let mut sink = CodeSink::new();
        // opcode=0x0 (bits 12-15), rd=1 (8-11), rs1=2 (4-7), rs2=3 (0-3)
        pack_bits(
            &mut sink,
            16,
            &[
                BitField::new(0x0, 12, 4), // opcode
                BitField::new(1, 8, 4),    // rd
                BitField::new(2, 4, 4),    // rs1
                BitField::new(3, 0, 4),    // rs2
            ],
        );
        let bytes = sink.bytes();
        // Expected: 0x0123 (little-endian: 0x23, 0x01)
        assert_eq!(bytes.len(), 2);
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0x0123);
    }

    #[test]
    fn test_pack_32bit_rv_rtype() {
        let mut sink = CodeSink::new();
        // add x1, x2, x3: opcode=0x33, rd=1, funct3=0, rs1=2, rs2=3, funct7=0
        pack_bits(
            &mut sink,
            32,
            &[
                BitField::new(0x33, 0, 7), // opcode
                BitField::new(1, 7, 5),    // rd
                BitField::new(0, 12, 3),   // funct3
                BitField::new(2, 15, 5),   // rs1
                BitField::new(3, 20, 5),   // rs2
                BitField::new(0, 25, 7),   // funct7
            ],
        );
        let bytes = sink.bytes();
        assert_eq!(bytes.len(), 4);
        let insn = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(insn, 0x0031_00B3);
    }

    #[test]
    fn test_pack_32bit_arm64_add() {
        let mut sink = CodeSink::new();
        // add x1, x2, x3: sf=1, op=ADD, Rm=x3, sh=0, Rn=x2, Rd=x1
        pack_bits(
            &mut sink,
            32,
            &[
                BitField::new(1, 0, 5),     // Rd=x1
                BitField::new(2, 5, 5),     // Rn=x2
                BitField::new(0, 10, 6),    // shift=0
                BitField::new(3, 16, 5),    // Rm=x3
                BitField::new(0x8B, 24, 8), // sf=1, opcode=ADD
            ],
        );
        let bytes = sink.bytes();
        assert_eq!(bytes.len(), 4);
        let insn = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        // 0x8B << 24 | 3 << 16 | 0 << 10 | 2 << 5 | 1
        assert_eq!(insn, 0x8B03_0041);
    }

    #[test]
    fn test_pack_64bit() {
        let mut sink = CodeSink::new();
        pack_bits(
            &mut sink,
            64,
            &[BitField::new(0xDEAD, 0, 16), BitField::new(0xBEEF, 48, 16)],
        );
        let bytes = sink.bytes();
        assert_eq!(bytes.len(), 8);
        let val = u64::from_le_bytes(bytes.try_into().unwrap());
        assert_eq!(val & 0xFFFF, 0xDEAD);
        assert_eq!((val >> 48) & 0xFFFF, 0xBEEF);
    }

    #[test]
    fn test_pack_bits_label() {
        let mut sink = CodeSink::new();
        let target = BlockId(42);
        pack_bits_label(
            &mut sink,
            32,
            &[BitField::new(0x63, 0, 7), BitField::new(0, 7, 25)],
            target,
            RelocKind::Isa(1),
        );
        let bytes = sink.bytes();
        assert_eq!(bytes.len(), 4);
        assert_eq!(bytes[0], 0x63); // opcode preserved
        // Remaining bytes are zero (placeholder for immediate)
    }

    #[test]
    fn test_pack_8bit() {
        let mut sink = CodeSink::new();
        pack_bits(&mut sink, 8, &[BitField::new(0x90, 0, 8)]);
        assert_eq!(sink.bytes(), &[0x90]);
    }

    #[test]
    fn test_bitfield_mask() {
        // Verify that values wider than the field are masked
        let mut sink = CodeSink::new();
        pack_bits(
            &mut sink,
            32,
            &[
                BitField::new(0xFFFFFFFF, 0, 4), // should be masked to 0xF
                BitField::new(0, 4, 28),
            ],
        );
        let bytes = sink.bytes();
        assert_eq!(bytes[0], 0x0F); // only 4 bits
    }
}
