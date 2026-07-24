//! 代码发射器 — CodeSink。
//!
//! 提供字节级代码输出、标签绑定和重定位记录功能。

use crate::ir::{BlockId, Endianness};
use crate::{RelocKind, Relocation};
use std::collections::HashMap;

/// 代码发射接收器。
///
/// 负责累积机器码字节、绑定标签位置、记录重定位信息，
/// 并在 `finish()` 时解析内部标签引用。
pub struct CodeSink {
    bytes: Vec<u8>,
    relocations: Vec<Relocation>,
    labels: HashMap<BlockId, usize>,
    label_fixups: Vec<(usize, BlockId, RelocKind)>,
    endianness: Endianness,
}

impl CodeSink {
    pub fn new() -> Self {
        Self {
            bytes: Vec::new(),
            relocations: Vec::new(),
            labels: HashMap::new(),
            label_fixups: Vec::new(),
            endianness: Endianness::default(),
        }
    }

    /// 设置目标端序（默认为小端）。
    pub fn set_endianness(&mut self, e: Endianness) {
        self.endianness = e;
    }

    /// 返回当前端序设置。
    pub fn endianness(&self) -> Endianness {
        self.endianness
    }

    /// 当前偏移量。
    pub fn offset(&self) -> usize {
        self.bytes.len()
    }

    /// 写入一个字节。
    pub fn put1(&mut self, byte: u8) {
        self.bytes.push(byte);
    }

    /// 写入两个字节（目标端序）。
    pub fn put2(&mut self, half: u16) {
        match self.endianness {
            Endianness::Little => self.bytes.extend_from_slice(&half.to_le_bytes()),
            Endianness::Big => self.bytes.extend_from_slice(&half.to_be_bytes()),
        }
    }

    /// 写入四个字节（目标端序）。
    pub fn put4(&mut self, word: u32) {
        match self.endianness {
            Endianness::Little => self.bytes.extend_from_slice(&word.to_le_bytes()),
            Endianness::Big => self.bytes.extend_from_slice(&word.to_be_bytes()),
        }
    }

    /// 写入八个字节（目标端序）。
    pub fn put8(&mut self, dword: u64) {
        match self.endianness {
            Endianness::Little => self.bytes.extend_from_slice(&dword.to_le_bytes()),
            Endianness::Big => self.bytes.extend_from_slice(&dword.to_be_bytes()),
        }
    }

    /// 写入原始字节切片。
    pub fn put_bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// 写入无符号 LEB128 编码的整数。
    pub fn put_uleb128(&mut self, mut value: u64) {
        loop {
            let mut byte = (value & 0x7F) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            self.put1(byte);
            if value == 0 {
                break;
            }
        }
    }

    /// 写入有符号 LEB128 编码的整数。
    pub fn put_sleb128(&mut self, mut value: i64) {
        loop {
            let mut byte = (value as u8) & 0x7F;
            value >>= 7;
            if (value == 0 && (byte & 0x40) == 0) || (value == -1 && (byte & 0x40) != 0) {
                self.put1(byte);
                break;
            }
            byte |= 0x80;
            self.put1(byte);
        }
    }

    /// 对齐到指定字节边界（用零填充）。
    pub fn align_to(&mut self, align: usize) {
        let offset = self.bytes.len();
        let pad = (align - (offset % align)) % align;
        self.bytes.resize(offset + pad, 0);
    }

    /// 在当前偏移处绑定标签。
    pub fn bind_label(&mut self, block: BlockId) {
        self.labels.insert(block, self.bytes.len());
    }

    /// 记录一个需要在 `finish()` 时解析的标签引用。
    /// 在指定偏移处预留占位空间后调用。
    pub fn use_label_at(&mut self, offset: usize, block: BlockId, kind: RelocKind) {
        self.label_fixups.push((offset, block, kind));
    }

    /// 返回所有 `Isa` 类型的标签 fixup，供 ISA 在 `finish()` 后处理。
    ///
    /// 返回 `Vec<(offset, block_id, isa_id)>`。
    pub fn isa_label_fixups(&self) -> Vec<(usize, BlockId, u8)> {
        self.label_fixups
            .iter()
            .filter_map(|(off, block, kind)| match kind {
                RelocKind::Isa(id) => Some((*off, *block, *id)),
                _ => None,
            })
            .collect()
    }

    /// 添加外部符号重定位。
    pub fn add_reloc(&mut self, offset: usize, kind: RelocKind, symbol: &str, addend: i64) {
        self.relocations.push(Relocation {
            offset,
            kind,
            symbol: symbol.to_string(),
            addend,
        });
    }

    /// 获取已发射的字节（尚未解析标签引用）。
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// 解析所有标签引用，返回最终的机器码字节。
    pub fn finish(mut self) -> Result<Vec<u8>, String> {
        for (offset, block, kind) in &self.label_fixups {
            let target = self
                .labels
                .get(block)
                .ok_or_else(|| format!("unresolved label: {:?}", block))?;
            let delta = *target as i64 - *offset as i64;

            match kind {
                RelocKind::Abs(w) => {
                    let val = *target as u64;
                    match w {
                        1 => self.bytes[*offset..*offset + 1]
                            .copy_from_slice(&(val as u8).to_le_bytes()),
                        2 => self.bytes[*offset..*offset + 2]
                            .copy_from_slice(&(val as u16).to_le_bytes()),
                        4 => self.bytes[*offset..*offset + 4]
                            .copy_from_slice(&(val as u32).to_le_bytes()),
                        8 => self.bytes[*offset..*offset + 8].copy_from_slice(&val.to_le_bytes()),
                        _ => return Err(format!("unsupported Abs width: {w}")),
                    }
                }
                RelocKind::Rel(w, adj) => {
                    let val = (delta + *adj as i64) as u64;
                    match w {
                        1 => self.bytes[*offset..*offset + 1]
                            .copy_from_slice(&(val as u8).to_le_bytes()),
                        2 => self.bytes[*offset..*offset + 2]
                            .copy_from_slice(&(val as u16).to_le_bytes()),
                        4 => self.bytes[*offset..*offset + 4]
                            .copy_from_slice(&(val as u32).to_le_bytes()),
                        8 => self.bytes[*offset..*offset + 8].copy_from_slice(&val.to_le_bytes()),
                        _ => return Err(format!("unsupported Rel width: {w}")),
                    }
                }
                RelocKind::Isa(_) => {
                    // ISA 特定重定位 — 不在这里处理。
                    // 调用方需在 finish() 后用 InstructionSet::encode_isa_reloc 处理。
                }
            }
        }
        Ok(self.bytes)
    }

    /// 获取重定位记录。
    pub fn relocations(&self) -> Vec<Relocation> {
        self.relocations.clone()
    }
}

impl Default for CodeSink {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::BlockId;

    #[test]
    fn test_emit_and_finish() {
        let mut sink = CodeSink::new();
        sink.put1(0x90); // nop
        sink.put2(0xABCD);
        sink.put4(0x12345678);
        sink.put8(0xDEADBEEF_CAFEBABE);

        let code = sink.finish().unwrap();
        assert_eq!(code.len(), 1 + 2 + 4 + 8);
        assert_eq!(code[0], 0x90);
    }

    #[test]
    fn test_label_fixup_rel4() {
        let mut sink = CodeSink::new();

        let target = BlockId(1);
        // 布局: [2 bytes pad] [4 bytes fixup slot] [0xCC] [0xDD] [label here] [0xC3]
        sink.put2(0); // offset 0..2
        let reloc_offset = sink.offset(); // offset = 2
        sink.put4(0xDEAD_BEEF); // offset 2..6 (占位符)
        sink.use_label_at(reloc_offset, target, RelocKind::REL4);

        sink.put1(0xCC); // offset 6
        sink.put1(0xDD); // offset 7
        sink.bind_label(target); // 标签绑定在偏移 8
        sink.put1(0xC3); // offset 8

        let code = sink.finish().unwrap();

        // label offset=8, reloc_offset=2, delta=2 (6 - 4 for x86 PC-relative)
        let expected_delta: i32 = 2;
        let resolved = i32::from_le_bytes([
            code[reloc_offset],
            code[reloc_offset + 1],
            code[reloc_offset + 2],
            code[reloc_offset + 3],
        ]);
        assert_eq!(resolved, expected_delta);
    }

    #[test]
    fn test_align_to() {
        let mut sink = CodeSink::new();
        sink.put1(0x90);
        sink.align_to(4);
        assert_eq!(sink.offset(), 4);
        assert_eq!(sink.bytes()[0], 0x90);
        assert_eq!(sink.bytes()[1], 0x00);
        assert_eq!(sink.bytes()[2], 0x00);
        assert_eq!(sink.bytes()[3], 0x00);
    }
}
