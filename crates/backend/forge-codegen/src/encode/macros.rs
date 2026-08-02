//! `pack!` 和 `pack_label!` 宏 — 声明式位域编码。
//!
//! # 语法
//!
//! ```ignore
//! pack!(sink, 32, [(opcode, 7), (rd, 5), (funct3, 3), (rs1, 5), (rs2, 5), (funct7, 7)]);
//! pack_label!(sink, 32, [(0x63, 7), (0, 5), ...], target, Isa(1));
//! ```
//!
//! 字段从左到右 = 从 LSB 到 MSB。偏移量自动累加。

/// 将值列表打包为指令字并发射。字段从左到右对应 bit 0 到 bit N-1。
///
/// # 语法
///
/// ```ignore
/// pack!(sink, TOTAL_BITS, [(VALUE, WIDTH), ...]);
/// ```
///
/// # 示例
///
/// ```ignore
/// // RISC-V ADD: opcode=0x33, rd=1, funct3=0, rs1=2, rs2=3, funct7=0
/// pack!(sink, 32, [(0x33, 7), (rd, 5), (0, 3), (rs1, 5), (rs2, 5), (0, 7)]);
///
/// // SimpleISA ADD: opcode=0, rd=1, rs1=2, rs2=3
/// pack!(sink, 16, [(rs2, 4), (rs1, 4), (rd, 4), (0x0, 4)]);
///
/// // ARM64 RET (单字段 32-bit)
/// pack!(sink, 32, [(0xD65F03C0u64, 32)]);
/// ```
#[macro_export]
macro_rules! pack {
    ($sink:expr, $bits:expr, [$(($val:expr, $width:expr)),* $(,)?]) => {{
        #[allow(unused_mut)]
        let mut _off: u8 = 0;
        $crate::encode::pack_bits($sink, $bits, &[
            $({
                let o = _off;
                _off += $width;
                $crate::BitField::new(($val) as u64, o, $width)
            }),*
        ]);
    }};
}

/// 同 `pack!`，但记录标签 fixup 用于分支/跳转指令。
///
/// # 语法
///
/// ```ignore
/// pack_label!(sink, TOTAL_BITS, [(VALUE, WIDTH), ...], TARGET, RELOC);
/// ```
///
/// # 示例
///
/// ```ignore
/// // RISC-V BEQ
/// pack_label!(sink, 32, [(0x63, 7), (0, 5), (0, 3), (rs1, 5), (rs2, 5), (0, 7)], target, Isa(1));
/// ```
#[macro_export]
macro_rules! pack_label {
    ($sink:expr, $bits:expr, [$(($val:expr, $width:expr)),* $(,)?], $target:expr, $reloc:expr) => {{
        #[allow(unused_mut)]
        let mut _off: u8 = 0;
        $crate::encode::pack_bits_label(
            $sink, $bits,
            &[
                $({
                    let o = _off;
                    _off += $width;
                    $crate::BitField::new(($val) as u64, o, $width)
                }),*
            ],
            $target,
            $reloc,
        );
    }};
}

#[cfg(test)]
mod tests {
    use crate::RelocKind;
    use crate::pipeline::emit::CodeSink;

    #[test]
    fn test_pack_macro_rv_add() {
        let mut sink = CodeSink::new();
        let rd: u64 = 1;
        let rs1: u64 = 2;
        let rs2: u64 = 3;
        pack!(
            &mut sink,
            32,
            [(0x33, 7), (rd, 5), (0, 3), (rs1, 5), (rs2, 5), (0, 7)]
        );
        let bytes = sink.bytes();
        assert_eq!(bytes.len(), 4);
        let insn = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(insn, 0x0031_00B3);
    }

    #[test]
    fn test_pack_macro_simple_isa_add() {
        let mut sink = CodeSink::new();
        let rs2: u64 = 3;
        let rs1: u64 = 2;
        let rd: u64 = 1;
        pack!(&mut sink, 16, [(rs2, 4), (rs1, 4), (rd, 4), (0x0, 4)]);
        let bytes = sink.bytes();
        assert_eq!(bytes.len(), 2);
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0x0123);
    }

    #[test]
    fn test_pack_label_macro() {
        let mut sink = CodeSink::new();
        let target = forge_ir::Block(42);
        pack_label!(
            &mut sink,
            32,
            [(0x63, 7), (0, 5), (0, 3), (2u64, 5), (3u64, 5), (0, 7)],
            target,
            RelocKind::REL4
        );
        let bytes = sink.bytes();
        assert_eq!(bytes.len(), 4);
    }

    #[test]
    fn test_pack_macro_single_field() {
        let mut sink = CodeSink::new();
        pack!(&mut sink, 32, [(0xD65F03C0u64, 32)]);
        let bytes = sink.bytes();
        assert_eq!(bytes.len(), 4);
        let insn = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(insn, 0xD65F03C0);
    }
}
