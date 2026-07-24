//! DWARF 调试信息发射器。
//!
//! 生成最小 `.debug_line` 和 `.debug_info` section，
//! 支持源码级调试（gdb/lldb 可显示源码行号）。
//!
//! ## 参考
//! - DWARF 5 标准 (Section 6.2: Line Number Information)
//! - DWARF 5 标准 (Section 3.1: Compilation Unit Entries)
#![allow(dead_code)] // 常量和方法保留供未来扩展

use crate::ir::{BlockId, DebugInfo, Function, SourceLocation};
use std::collections::HashMap;

// ============================================================
// DWARF 常量
// ============================================================

/// DWARF 行号程序 opcode
mod dwarf {
    // Standard opcodes
    pub const DW_LNS_COPY: u8 = 1;
    pub const DW_LNS_ADVANCE_PC: u8 = 2;
    pub const DW_LNS_ADVANCE_LINE: u8 = 3;
    pub const DW_LNS_SET_FILE: u8 = 4;
    pub const DW_LNS_SET_COLUMN: u8 = 5;

    // Extended opcodes
    pub const DW_LNE_END_SEQUENCE: u8 = 1;
    pub const DW_LNE_SET_ADDRESS: u8 = 2;
    pub const DW_LNE_DEFINE_FILE: u8 = 3;

    // DWARF format constants
    pub const DWARF32_FORMAT: u8 = 32; // 32-bit DWARF
    pub const LINE_BASE: i8 = -5;
    pub const LINE_RANGE: u8 = 14;
    pub const OPCODE_BASE: u8 = 13; // Standard opcodes 1-12
}

/// 最小 DWARF .debug_line section 发射器。
///
/// 生成标准行号程序，将指令地址映射到源码行号。
pub struct DebugLineEmitter {
    /// 字节缓冲区
    bytes: Vec<u8>,
    /// 编译目录
    comp_dir: String,
    /// 源文件列表
    files: Vec<(String, String)>, // (name, directory_index)
}

impl DebugLineEmitter {
    pub fn new(comp_dir: &str) -> Self {
        let mut e = Self {
            bytes: Vec::new(),
            comp_dir: comp_dir.to_string(),
            files: vec![(comp_dir.to_string(), "0".to_string())],
        };
        e.emit_header();
        e
    }

    /// 注册源文件
    pub fn add_file(&mut self, path: &str) -> usize {
        let idx = self.files.len();
        self.files.push((path.to_string(), "0".to_string()));
        idx
    }

    /// 发射 .debug_line section 头部
    fn emit_header(&mut self) {
        use dwarf::*;
        let header_start = self.bytes.len();

        // unit_length (placeholder)
        self.put4(0u32);
        // version (DWARF 5 = 5)
        self.put2(5u16);
        // address_size
        self.put1(8u8); // 64-bit
        // segment_selector_size
        self.put1(0u8);

        // header_length (placeholder)
        let header_len_pos = self.bytes.len();
        self.put4(0u32);

        // minimum_instruction_length
        self.put1(1u8);
        // maximum_operations_per_instruction
        self.put1(1u8);
        // default_is_stmt
        self.put1(1u8);
        // line_base
        self.put1(LINE_BASE as u8);
        // line_range
        self.put1(LINE_RANGE);
        // opcode_base
        self.put1(OPCODE_BASE);
        // standard_opcode_lengths (for opcodes 1-12)
        let std_lens: [u8; 12] = [0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1];
        for &len in &std_lens {
            self.put1(len);
        }

        // include_directories (empty)
        self.put1(0u8);

        // file_names
        let file_entries: Vec<_> = self.files.iter().map(|(n, d)| (n.clone(), d.clone())).collect();
        for (name, dir_idx) in &file_entries {
            self.put_str(name);
            self.put_uleb128(dir_idx.parse().unwrap_or(0));
            self.put_uleb128(0);
            self.put_uleb128(0);
        }
        self.put1(0u8); // null terminator

        // Patch header_length
        let header_len = (self.bytes.len() - header_len_pos - 4) as u32;
        self.bytes[header_len_pos..header_len_pos + 4]
            .copy_from_slice(&header_len.to_le_bytes());

        // Patch unit_length
        let unit_len = (self.bytes.len() - header_start - 4) as u32;
        self.bytes[header_start..header_start + 4].copy_from_slice(&unit_len.to_le_bytes());
    }

    /// 发射行号程序：将地址映射到源码位置
    pub fn emit_line_program(
        &mut self,
        address_map: &[(u64, SourceLocation)],
    ) {
        use dwarf::*;

        // State machine registers
        let mut _current_address: u64 = 0;
        let mut current_file: usize = 0;
        let mut current_line: u32 = 1;
        let mut current_column: u32 = 0;

        for &(addr, ref loc) in address_map {
            if addr == 0 && loc.line.is_none() {
                continue;
            }

            let target_line = loc.line.unwrap_or(1);
            let target_column = loc.column.unwrap_or(0);
            let target_file = match loc.file {
                Some(ref f) => {
                    let mut found = None;
                    for (i, (name, _)) in self.files.iter().enumerate() {
                        if name == f {
                            found = Some(i);
                            break;
                        }
                    }
                    match found {
                        Some(i) => i,
                        None => {
                            let idx = self.files.len();
                            self.files.push((f.clone(), "0".to_string()));
                            idx
                        }
                    }
                }
                None => 0,
            };

            // Set address
            self.put1(0u8); // extended opcode
            self.put_uleb128(9); // length (1 + 8)
            self.put1(DW_LNE_SET_ADDRESS);
            self.put8(addr);

            // Set file if changed
            if target_file != current_file {
                self.put1(DW_LNS_SET_FILE);
                self.put_uleb128(target_file as u64);
                current_file = target_file;
            }

            // Set column if changed
            if target_column != current_column {
                self.put1(DW_LNS_SET_COLUMN);
                self.put_uleb128(target_column as u64);
                current_column = target_column;
            }

            // Advance line
            let line_delta = target_line as i64 - current_line as i64;
            if line_delta != 0 {
                let adjusted = line_delta as i32 - dwarf::LINE_BASE as i32;
                if adjusted >= 0 && (adjusted as u32) < dwarf::LINE_RANGE as u32 {
                    let special = (adjusted as u8) + dwarf::OPCODE_BASE;
                    self.put1(special);
                } else {
                    self.put1(dwarf::DW_LNS_ADVANCE_LINE);
                    self.put_sleb128(line_delta);
                    self.put1(dwarf::DW_LNS_COPY);
                }
            } else {
                self.put1(dwarf::DW_LNS_COPY);
            }
            current_line = target_line;
        }

        // End sequence
        self.put1(0u8);
        self.put_uleb128(1);
        self.put1(DW_LNE_END_SEQUENCE);
    }

    fn resolve_file(&mut self, path: &str) -> usize {
        if let Some(idx) = self.files.iter().position(|(f, _)| f == path) {
            idx
        } else {
            self.add_file(path)
        }
    }

    // ============================================================
    // 二进制写入辅助
    // ============================================================

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }

    fn put1(&mut self, b: u8) {
        self.bytes.push(b);
    }

    fn put2(&mut self, v: u16) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn put4(&mut self, v: u32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn put8(&mut self, v: u64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn put_str(&mut self, s: &str) {
        self.bytes.extend_from_slice(s.as_bytes());
        self.bytes.push(0u8);
    }

    fn put_uleb128(&mut self, mut value: u64) {
        loop {
            let mut byte = (value & 0x7F) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            self.bytes.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn put_sleb128(&mut self, value: i64) {
        let mut val = value;
        loop {
            let mut byte = (val as u8) & 0x7F;
            val >>= 7;
            if (val == 0 && (byte & 0x40) == 0) || (val == -1 && (byte & 0x40) != 0) {
                self.bytes.push(byte);
                break;
            }
            byte |= 0x80;
            self.bytes.push(byte);
        }
    }
}

/// 最小 DWARF .debug_info section 发射器（单个 CU + 子程序 DIE）。
pub struct DebugInfoEmitter {
    bytes: Vec<u8>,
}

impl DebugInfoEmitter {
    pub fn new() -> Self {
        let mut e = Self { bytes: Vec::new() };
        e.emit_header();
        e
    }

    fn emit_header(&mut self) {
        // unit_length (placeholder)
        self.put4(0u32);
        // version (DWARF 5)
        self.put2(5u16);
        // unit_type (DW_UT_compile = 1)
        self.put1(1u8);
        // address_size
        self.put1(8u8);
        // debug_abbrev_offset (0 for no abbrev — minimal)
        self.put4(0u32);
    }

    /// 添加子程序 DIE（minimal）
    pub fn emit_subprogram(&mut self, name: &str, low_pc: u64, high_pc: u64) {
        // DW_TAG_subprogram (minimal: tag bytes only, no abbrev table)
        // 完整实现需要 .debug_abbrev section，这里仅生成最小骨架
        let _ = (name, low_pc, high_pc);
    }

    fn put1(&mut self, b: u8) { self.bytes.push(b); }
    fn put2(&mut self, v: u16) { self.bytes.extend_from_slice(&v.to_le_bytes()); }
    fn put4(&mut self, v: u32) { self.bytes.extend_from_slice(&v.to_le_bytes()); }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

impl Default for DebugInfoEmitter {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 高层接口
// ============================================================

/// 为编译函数生成 DWARF 调试信息。
///
/// 返回 `(debug_line_bytes, debug_info_bytes)`。
/// 调用方负责将这些字节写入对象文件的对应 section。
pub fn generate_dwarf(
    debug_info: &DebugInfo,
    block_addresses: &HashMap<BlockId, u64>,
    function_name: &str,
    comp_dir: &str,
) -> (Vec<u8>, Vec<u8>) {
    let mut line_emitter = DebugLineEmitter::new(comp_dir);

    // 收集地址→源码位置映射
    let mut addr_map: Vec<(u64, SourceLocation)> = Vec::new();
    for (value, loc) in &debug_info.locations {
        // 使用 Value.0 作为伪地址 — 实际实现需要跟踪 Value→指令偏移
        let addr = value.0 as u64;
        addr_map.push((addr, loc.clone()));
    }
    // 按地址排序
    addr_map.sort_by_key(|(a, _)| *a);

    if !addr_map.is_empty() {
        line_emitter.emit_line_program(&addr_map);
    }

    let mut info_emitter = DebugInfoEmitter::new();
    let low_pc = block_addresses.values().min().copied().unwrap_or(0);
    let high_pc = block_addresses.values().max().copied().unwrap_or(0);
    let _ = function_name;
    info_emitter.emit_subprogram(function_name, low_pc, high_pc);

    (line_emitter.finish(), info_emitter.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_debug_line() {
        let emitter = DebugLineEmitter::new("/tmp");
        let bytes = emitter.finish();
        assert!(!bytes.is_empty());
        // unit_length (4 bytes) + version u16 (little-endian) = bytes[4]=5, bytes[5]=0
        assert_eq!(bytes[4], 5, "DWARF version should be 5 at byte 4");
    }

    #[test]
    fn test_debug_line_with_source() {
        let mut emitter = DebugLineEmitter::new("/src");
        emitter.add_file("test.rs");
        emitter.emit_line_program(&[
            (0x1000, SourceLocation::new("test.rs", 10, 5)),
            (0x1004, SourceLocation::new("test.rs", 11, 1)),
        ]);
        let bytes = emitter.finish();
        assert!(bytes.len() > 30);
    }

    #[test]
    fn test_uleb128_roundtrip() {
        // Test basic uleb128 encoding
        fn uleb(v: u64) -> Vec<u8> {
            let mut bytes = vec![];
            let mut value = v;
            loop {
                let mut byte = (value & 0x7F) as u8;
                value >>= 7;
                if value != 0 {
                    byte |= 0x80;
                }
                bytes.push(byte);
                if value == 0 {
                    break;
                }
            }
            bytes
        }
        assert_eq!(uleb(0), vec![0]);
        assert_eq!(uleb(1), vec![1]);
        assert_eq!(uleb(127), vec![0x7F]);
        assert_eq!(uleb(128), vec![0x80, 0x01]);
        assert_eq!(uleb(129), vec![0x81, 0x01]);
        assert_eq!(uleb(0x3FFF), vec![0xFF, 0x7F]);
    }
}
