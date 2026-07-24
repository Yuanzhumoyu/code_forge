//! 异常处理帧信息 (.eh_frame) 发射器。
//!
//! 生成 DWARF CFI (Call Frame Information) 用于栈展开。
//! 支持 Itanium C++ ABI 异常处理 (Linux/macOS) 和 Windows SEH。
//!
//! ## 格式
//!
//! ```text
//! .eh_frame section:
//!   CIE (Common Information Entry) — 全局帧信息模板
//!   FDE (Frame Description Entry) × N — 每个函数的帧信息
//!   Terminator: CIE length = 0
//!
//! .gcc_except_table section:
//!   LSDA (Language Specific Data Area) — 每个函数的异常处理表
//! ```
//!
//! ## 参考
//! - DWARF 5 标准 (Section 6.4: Call Frame Information)
//! - Linux Standard Base Core Specification 4.1 (Chapter 10: Exception Frames)
//! - x86-64 ABI (Section 3.7: Stack Unwind)
#![allow(dead_code)] // 常量和方法保留供未来扩展

use std::collections::HashMap;

// ============================================================
// DWARF CFI 常量
// ============================================================

/// DWARF CFI 指令 opcode。
mod cfi {
    // CFA (Canonical Frame Address) 操作
    pub const DW_CFA_ADVANCE_LOC: u8 = 0x40; // 0x40-0x4F: advance loc
    pub const DW_CFA_OFFSET: u8 = 0x80; // 0x80-0x8F: offset
    pub const DW_CFA_RESTORE: u8 = 0xC0; // 0xC0-0xCF: restore

    // 标准操作码
    pub const DW_CFA_NOP: u8 = 0x00;
    pub const DW_CFA_SET_LOC: u8 = 0x01;
    pub const DW_CFA_ADVANCE_LOC1: u8 = 0x02;
    pub const DW_CFA_ADVANCE_LOC2: u8 = 0x03;
    pub const DW_CFA_ADVANCE_LOC4: u8 = 0x04;
    pub const DW_CFA_OFFSET_EXTENDED: u8 = 0x05;
    pub const DW_CFA_RESTORE_EXTENDED: u8 = 0x06;
    pub const DW_CFA_UNDEFINED: u8 = 0x07;
    pub const DW_CFA_SAME_VALUE: u8 = 0x08;
    pub const DW_CFA_REGISTER: u8 = 0x09;
    pub const DW_CFA_REMEMBER_STATE: u8 = 0x0A;
    pub const DW_CFA_RESTORE_STATE: u8 = 0x0B;
    pub const DW_CFA_DEF_CFA: u8 = 0x0C;
    pub const DW_CFA_DEF_CFA_REGISTER: u8 = 0x0D;
    pub const DW_CFA_DEF_CFA_OFFSET: u8 = 0x0E;
    pub const DW_CFA_DEF_CFA_EXPRESSION: u8 = 0x0F;
    pub const DW_CFA_EXPRESSION: u8 = 0x10;
    pub const DW_CFA_OFFSET_EXTENDED_SF: u8 = 0x11;
    pub const DW_CFA_DEF_CFA_SF: u8 = 0x12;
    pub const DW_CFA_DEF_CFA_OFFSET_SF: u8 = 0x13;
    pub const DW_CFA_VAL_OFFSET: u8 = 0x14;
    pub const DW_CFA_VAL_OFFSET_SF: u8 = 0x15;
    pub const DW_CFA_VAL_EXPRESSION: u8 = 0x16;

    // x86-64 寄存器编号 (DWARF)
    pub const DW_REG_RAX: u8 = 0;
    pub const DW_REG_RDX: u8 = 1;
    pub const DW_REG_RCX: u8 = 2;
    pub const DW_REG_RBX: u8 = 3;
    pub const DW_REG_RSI: u8 = 4;
    pub const DW_REG_RDI: u8 = 5;
    pub const DW_REG_RBP: u8 = 6;
    pub const DW_REG_RSP: u8 = 7;
    pub const DW_REG_R8: u8 = 8;
    pub const DW_REG_R9: u8 = 9;
    pub const DW_REG_R10: u8 = 10;
    pub const DW_REG_R11: u8 = 11;
    pub const DW_REG_R12: u8 = 12;
    pub const DW_REG_R13: u8 = 13;
    pub const DW_REG_R14: u8 = 14;
    pub const DW_REG_R15: u8 = 15;
    pub const DW_REG_RA: u8 = 16; // Return Address
}

// ============================================================
// .eh_frame 结构
// ============================================================

/// CIE (Common Information Entry) — 帧描述模板。
///
/// 每个 .eh_frame section 包含一个或多个 CIE，每个 CIE 后跟零或多个 FDE。
#[derive(Clone, Debug)]
pub struct CommonInfoEntry {
    /// CIE 版本 (1 或 3 for DWARF4, 4 for DWARF5)。
    pub version: u8,
    /// 增强字符串 (augmentation string) — 对于 EH，通常是 "zR"。
    pub augmentation: String,
    /// 代码对齐因子 (code alignment factor) — 指令地址的最小对齐。
    pub code_alignment_factor: u32,
    /// 数据对齐因子 (data alignment factor) — 栈偏移的对齐。
    pub data_alignment_factor: i32,
    /// 返回地址寄存器 (DWARF 寄存器编号)。
    pub return_address_register: u8,
    /// 初始指令 (CFI 操作码序列)。
    pub initial_instructions: Vec<u8>,
    /// 增强数据 (augmentation data) — 仅在 augmentation 包含 "z" 时出现。
    pub augmentation_data: Vec<u8>,
    /// LSDA 指针编码 (augmentation 包含 "L" 时)。
    pub lsda_pointer_encoding: u8,
    /// FDE 指针编码 (augmentation 包含 "R" 时)。
    pub fde_pointer_encoding: u8,
    /// personality 函数编码 (augmentation 包含 "P" 时)。
    pub personality_encoding: u8,
    /// personality 函数指针。
    pub personality_routine: Option<u64>,
}

impl Default for CommonInfoEntry {
    fn default() -> Self {
        Self {
            version: 1,
            augmentation: "zR".to_string(),
            code_alignment_factor: 1,
            data_alignment_factor: -8, // x86-64: 栈向下增长，8 字节对齐
            return_address_register: cfi::DW_REG_RA, // x86-64: return address
            initial_instructions: Vec::new(),
            augmentation_data: Vec::new(),
            lsda_pointer_encoding: 0,
            fde_pointer_encoding: 0x1B, // DW_EH_PE_pcrel | DW_EH_PE_sdata4
            personality_encoding: 0,
            personality_routine: None,
        }
    }
}

impl CommonInfoEntry {
    /// 创建 x86-64 Linux/macOS 的默认 CIE。
    pub fn x86_64_default() -> Self {
        Self {
            initial_instructions: vec![
                cfi::DW_CFA_DEF_CFA,
                cfi::DW_REG_RSP, // CFA = RSP
                8,               // offset = 8
            ],
            ..Self::default()
        }
    }

    /// 创建包含 personality routine 的 CIE (用于 EH)。
    pub fn with_personality(personality_ptr: u64) -> Self {
        let mut cie = Self::x86_64_default();
        cie.augmentation = "zPLR".to_string();
        cie.personality_encoding = 0x1B; // DW_EH_PE_pcrel | DW_EH_PE_sdata4
        cie.personality_routine = Some(personality_ptr);
        cie
    }

    /// 将 CIE 编码为字节。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // CIE body
        let cie_id: u32 = 0; // CIE id = 0
        let version = self.version;

        let aug_bytes = self.augmentation.as_bytes();
        let _aug_len = aug_bytes.len() + 1; // +1 for null terminator

        // Encode initial instructions
        let mut cie_body = Vec::new();
        cie_body.push(version);
        cie_body.extend_from_slice(aug_bytes);
        cie_body.push(0); // null terminator

        // code_alignment_factor 用 LEB128
        encode_uleb128(&mut cie_body, self.code_alignment_factor as u64);
        // data_alignment_factor 用 SLEB128
        encode_sleb128(&mut cie_body, self.data_alignment_factor as i64);
        // return address register 用 ULEB128
        encode_uleb128(&mut cie_body, self.return_address_register as u64);

        // Augmentation data (z 前缀)
        if self.augmentation.contains('z') {
            // 计算 augmentation data 的大小
            let mut aug_data = Vec::new();
            if self.augmentation.contains('R') {
                aug_data.push(self.fde_pointer_encoding);
            }
            if self.augmentation.contains('L') {
                aug_data.push(self.lsda_pointer_encoding);
            }
            if self.augmentation.contains('P') {
                aug_data.push(self.personality_encoding);
                // personality routine pointer (PC-relative, sdata4)
                if let Some(p) = self.personality_routine {
                    aug_data.extend_from_slice(&(p as u32).to_le_bytes());
                }
            }
            encode_uleb128(&mut cie_body, aug_data.len() as u64);
            cie_body.extend_from_slice(&aug_data);
        }

        // Initial instructions
        cie_body.extend_from_slice(&self.initial_instructions);

        // Length = CIE_id(4) + cie_body
        let length = (4 + cie_body.len()) as u32;

        buf.extend_from_slice(&length.to_le_bytes());
        buf.extend_from_slice(&cie_id.to_le_bytes());
        buf.extend_from_slice(&cie_body);

        // Padding to alignment
        while buf.len() % 4 != 0 {
            buf.push(cfi::DW_CFA_NOP);
        }

        buf
    }
}

/// FDE (Frame Description Entry) — 单个函数的帧信息。
#[derive(Clone, Debug)]
pub struct FrameDescriptionEntry {
    /// 函数起始地址 (相对于 CIE 指针的偏移，若 fde_pointer_encoding 为 pcrel)。
    pub initial_location: u64,
    /// 函数地址范围 (字节数)。
    pub address_range: u64,
    /// CFI 指令序列。
    pub instructions: Vec<u8>,
    /// LSDA 指针 (augmentation 包含 "L" 时)。
    pub lsda_pointer: Option<u64>,
}

impl FrameDescriptionEntry {
    /// 创建新 FDE。
    pub fn new(initial_location: u64, address_range: u64) -> Self {
        Self {
            initial_location,
            address_range,
            instructions: Vec::new(),
            lsda_pointer: None,
        }
    }

    /// 添加 CFI 指令。
    pub fn add_instruction(&mut self, byte: u8) {
        self.instructions.push(byte);
    }

    /// 编码 DW_CFA_OFFSET: register = CFA + offset。
    pub fn add_offset(&mut self, register: u8, offset: i32) {
        if register <= 0x3F && offset >= 0 && offset % 8 == 0 {
            // Use compact encoding: DW_CFA_OFFSET + register
            let factored_offset = (offset / 8) as u8;
            self.instructions.push(cfi::DW_CFA_OFFSET | register);
            encode_uleb128(&mut self.instructions, factored_offset as u64);
        } else {
            self.instructions.push(cfi::DW_CFA_OFFSET_EXTENDED);
            encode_uleb128(&mut self.instructions, register as u64);
            encode_uleb128(&mut self.instructions, offset as u64);
        }
    }

    /// 编码 DW_CFA_DEF_CFA_OFFSET: CFA = 寄存器（不变）+ 新偏移。
    pub fn def_cfa_offset(&mut self, offset: i32) {
        self.instructions.push(cfi::DW_CFA_DEF_CFA_OFFSET);
        encode_uleb128(&mut self.instructions, offset as u64);
    }

    /// 将 FDE 编码为字节 (需要 CIE 指针位置来计算 PC-relative 偏移)。
    pub fn encode(&self, cie_pointer: u64, augmentation: &str) -> Vec<u8> {
        let mut buf = Vec::new();

        // FDE body (excluding length field)
        let mut fde_body = Vec::new();

        // CIE pointer (PC-relative offset to CIE)
        let cie_offset: i32 = -(cie_pointer as i32);
        fde_body.extend_from_slice(&cie_offset.to_le_bytes());

        // Initial location (PC-relative, sdata4)
        fde_body.extend_from_slice(&(self.initial_location as u32).to_le_bytes());

        // Address range
        fde_body.extend_from_slice(&(self.address_range as u32).to_le_bytes());

        // Augmentation data length (z 前缀)
        if augmentation.contains('z') {
            let mut aug_data = Vec::new();
            if augmentation.contains('L') {
                if let Some(lsda) = self.lsda_pointer {
                    aug_data.extend_from_slice(&(lsda as u32).to_le_bytes());
                } else {
                    aug_data.extend_from_slice(&0u32.to_le_bytes());
                }
            }
            encode_uleb128(&mut fde_body, aug_data.len() as u64);
            fde_body.extend_from_slice(&aug_data);
        }

        // CFI instructions
        fde_body.extend_from_slice(&self.instructions);

        // Length = CIE_pointer(4) + FDE body
        let length = (4 + fde_body.len()) as u32;

        buf.extend_from_slice(&length.to_le_bytes());
        buf.extend_from_slice(&fde_body);

        // Padding
        while buf.len() % 4 != 0 {
            buf.push(cfi::DW_CFA_NOP);
        }

        buf
    }
}

// ============================================================
// .eh_frame section 构建器
// ============================================================

/// `.eh_frame` section 构建器。
///
/// 管理 CIE 和 FDE 的集合，生成完整的 `.eh_frame` section。
pub struct EhFrameBuilder {
    /// 已注册的 CIE。
    cies: Vec<CommonInfoEntry>,
    /// FDE 列表 (CIE 索引, FDE)。
    fdes: Vec<(usize, FrameDescriptionEntry)>,
    /// CIE 到其起始偏移的缓存 (用于计算 FDE PC-relative 引用)。
    cie_offsets: HashMap<usize, u64>,
}

impl EhFrameBuilder {
    pub fn new() -> Self {
        Self {
            cies: Vec::new(),
            fdes: Vec::new(),
            cie_offsets: HashMap::new(),
        }
    }

    /// 注册 CIE，返回其索引。
    pub fn add_cie(&mut self, cie: CommonInfoEntry) -> usize {
        let idx = self.cies.len();
        self.cies.push(cie);
        idx
    }

    /// 添加 FDE (关联到指定 CIE)。
    pub fn add_fde(&mut self, cie_idx: usize, fde: FrameDescriptionEntry) {
        self.fdes.push((cie_idx, fde));
    }

    /// 构建 x86-64 标准 prologue 的 FDE 指令。
    ///
    /// 标准 x86-64 函数 prologue:
    /// ```asm
    /// push rbp          ; CFA += 8, RBP saved at CFA-8
    /// mov rbp, rsp      ; CFA = RBP + 16
    /// ```
    pub fn x86_64_prologue_cfi(frame_size: u32) -> Vec<u8> {
        let mut insns = vec![
            cfi::DW_CFA_ADVANCE_LOC | 1, // push rbp (1 byte)
            cfi::DW_CFA_OFFSET | cfi::DW_REG_RBP, // RBP saved at CFA offset
            2, // factored offset = 16 / 8
        ];

        // DW_CFA_ADVANCE_LOC: 3 (mov rbp, rsp)
        insns.push(cfi::DW_CFA_ADVANCE_LOC | 3);

        // DW_CFA_DEF_CFA_REGISTER: rbp(6) — CFA 现在基于 RBP
        insns.push(cfi::DW_CFA_DEF_CFA_REGISTER);
        encode_uleb128(&mut insns, cfi::DW_REG_RBP as u64);

        // If frame_size > 0, emit CFA offset
        if frame_size > 0 {
            insns.push(cfi::DW_CFA_ADVANCE_LOC | 4); // sub rsp, N
            // CFA offset = frame_size + 16 (return address + saved rbp)
            insns.push(cfi::DW_CFA_DEF_CFA_OFFSET);
            // SLEB128: offset = frame_size + 16
            encode_sleb128(&mut insns, (frame_size + 16) as i64);
        } else {
            insns.push(cfi::DW_CFA_DEF_CFA_OFFSET);
            encode_uleb128(&mut insns, 16); // CFA = RBP + 16
        }

        insns
    }

    /// 生成完整的 `.eh_frame` section 字节。
    pub fn build(&mut self) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut current_offset: u64 = 0;

        for (idx, cie) in self.cies.iter().enumerate() {
            self.cie_offsets.insert(idx, current_offset);

            let cie_bytes = cie.encode();
            let _cie_end = current_offset + cie_bytes.len() as u64;
            buf.extend_from_slice(&cie_bytes);
            current_offset += cie_bytes.len() as u64;

            // Emit all FDEs for this CIE
            for (cie_i, fde) in &self.fdes {
                if *cie_i == idx {
                    let fde_bytes = fde.encode(current_offset, &cie.augmentation);
                    buf.extend_from_slice(&fde_bytes);
                    current_offset += fde_bytes.len() as u64;
                }
            }
        }

        // Terminator: CIE length = 0
        buf.extend_from_slice(&0u32.to_le_bytes());

        buf
    }
}

impl Default for EhFrameBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// .gcc_except_table section
// ============================================================

/// LSDA (Language Specific Data Area) header for .gcc_except_table.
#[derive(Clone, Debug)]
pub struct LsdaHeader {
    /// Landing pad base pointer encoding.
    pub lp_base_encoding: u8,
    /// Landing pad base (PC-relative to start of LSDA).
    pub lp_base: Option<u64>,
    /// Type table pointer encoding.
    pub tt_encoding: u8,
    /// Type table entries.
    pub type_table: Vec<LsdaTypeEntry>,
    /// Call site records.
    pub call_sites: Vec<LsdaCallSite>,
    /// Action table entries.
    pub action_table: Vec<LsdaAction>,
}

/// Type table entry for LSDA.
#[derive(Clone, Debug)]
pub struct LsdaTypeEntry {
    /// Type info pointer (filter value, or 0 for catch-all).
    pub type_info: u64,
}

/// Call site record — maps a PC range to a landing pad.
#[derive(Clone, Debug)]
pub struct LsdaCallSite {
    /// Call site start (PC-relative to function start).
    pub cs_start: u32,
    /// Call site length.
    pub cs_len: u32,
    /// Landing pad offset (0 if no landing pad).
    pub cs_lp: u32,
    /// Action record offset (0 if no action).
    pub cs_action: u32,
}

/// Action record — sequence of type filters for a landing pad.
#[derive(Clone, Debug)]
pub struct LsdaAction {
    /// Type filter index in the type table.
    pub type_filter: i32,
    /// Offset to next action (0 = end of chain).
    pub next_action: i32,
}

impl LsdaHeader {
    pub fn new() -> Self {
        Self {
            lp_base_encoding: 0xFF, // DW_EH_PE_omit
            lp_base: None,
            tt_encoding: 0x1B, // DW_EH_PE_pcrel | DW_EH_PE_sdata4
            type_table: Vec::new(),
            call_sites: Vec::new(),
            action_table: Vec::new(),
        }
    }

    /// Encode LSDA header for `.gcc_except_table`.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Header
        buf.push(self.lp_base_encoding);
        if self.lp_base_encoding != 0xFF
            && let Some(base) = self.lp_base
        {
            buf.extend_from_slice(&(base as u32).to_le_bytes());
        }
        buf.push(self.tt_encoding);

        // Type table
        let tt_size = self.type_table.len() as u32;
        if self.tt_encoding != 0xFF {
            encode_uleb128(&mut buf, tt_size as u64);
            for entry in &self.type_table {
                buf.extend_from_slice(&(entry.type_info as u32).to_le_bytes());
            }
        }

        // Call site table
        encode_uleb128(&mut buf, self.call_sites.len() as u64);
        for cs in &self.call_sites {
            encode_uleb128(&mut buf, cs.cs_start as u64);
            encode_uleb128(&mut buf, cs.cs_len as u64);
            encode_uleb128(&mut buf, cs.cs_lp as u64);
            encode_uleb128(&mut buf, cs.cs_action as u64);
        }

        // Action table (not encoded unless needed)
        buf
    }
}

impl Default for LsdaHeader {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// LEB128 编码辅助函数
// ============================================================

/// 编码无符号 LEB128。
fn encode_uleb128(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// 编码有符号 LEB128 (SLEB128)。
fn encode_sleb128(buf: &mut Vec<u8>, mut value: i64) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        let sign_bit = byte & 0x40;
        if (value == -1 && sign_bit != 0) || (value == 0 && sign_bit == 0) {
            buf.push(byte);
            break;
        }
        byte |= 0x80;
        buf.push(byte);
    }
}

// ============================================================
// 顶层 API
// ============================================================

/// 生成 x86-64 的最小 `.eh_frame` section。
///
/// 为单个函数生成 CIE + FDE，用标准 x86-64 prologue。
/// `func_start` 是函数起始地址 (section-relative 偏移)。
/// `func_size` 是函数总大小。
/// `frame_size` 是函数分配的栈帧大小。
pub fn generate_x86_64_eh_frame(func_start: u64, func_size: u64, frame_size: u32) -> Vec<u8> {
    let mut builder = EhFrameBuilder::new();

    // 创建 CIE
    let cie = CommonInfoEntry::x86_64_default();
    let cie_idx = builder.add_cie(cie);

    // 创建 FDE
    let mut fde = FrameDescriptionEntry::new(func_start, func_size);
    fde.instructions = EhFrameBuilder::x86_64_prologue_cfi(frame_size);
    builder.add_fde(cie_idx, fde);

    builder.build()
}

/// 生成包含 personality 函数的最小 `.eh_frame` section。
///
/// 额外的 `personality_ptr` 参数指定 personality routine 的地址。
pub fn generate_x86_64_eh_frame_with_personality(
    func_start: u64,
    func_size: u64,
    frame_size: u32,
    personality_ptr: u64,
) -> Vec<u8> {
    let mut builder = EhFrameBuilder::new();

    // 创建带 personality 的 CIE
    let cie = CommonInfoEntry::with_personality(personality_ptr);
    let cie_idx = builder.add_cie(cie);

    // 创建 FDE
    let mut fde = FrameDescriptionEntry::new(func_start, func_size);
    fde.instructions = EhFrameBuilder::x86_64_prologue_cfi(frame_size);
    builder.add_fde(cie_idx, fde);

    builder.build()
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_eh_frame() {
        let frame = generate_x86_64_eh_frame(0, 64, 0);
        // 应该包含: CIE + FDE + terminator
        assert!(frame.len() > 24); // Minimum: length(4) + CIE body + length(4) + FDE body + term(4)
        // Terminator = 4 zero bytes
        assert_eq!(&frame[frame.len() - 4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn test_eh_frame_with_frame_size() {
        let frame = generate_x86_64_eh_frame(0x1000, 128, 32);
        assert!(frame.len() > 24);
        assert_eq!(&frame[frame.len() - 4..], &[0, 0, 0, 0]); // terminator
    }

    #[test]
    fn test_cie_encode_decode() {
        let cie = CommonInfoEntry::x86_64_default();
        let bytes = cie.encode();
        // CIE length > 0
        let length = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert!(length > 0);
        // After length, CIE_id = 0
        let cie_id = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        assert_eq!(cie_id, 0);
    }

    #[test]
    fn test_fde_structure() {
        let fde = FrameDescriptionEntry::new(0x100, 0x200);
        let bytes = fde.encode(0x40, "zR");
        assert!(bytes.len() >= 16);
        // First 4 bytes = length
        let length = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert!(length > 0);
    }

    #[test]
    fn test_leb128_encoding() {
        let mut buf = Vec::new();
        encode_uleb128(&mut buf, 0);
        assert_eq!(buf, vec![0]);

        buf.clear();
        encode_uleb128(&mut buf, 127);
        assert_eq!(buf, vec![127]);

        buf.clear();
        encode_uleb128(&mut buf, 128);
        assert_eq!(buf, vec![0x80, 0x01]);

        buf.clear();
        encode_uleb128(&mut buf, 0xFFFFFFFF);
        assert_eq!(buf.len(), 5); // 32-bit value → 5 byte ULEB128
    }

    #[test]
    fn test_sleb128_encoding() {
        let mut buf = Vec::new();
        encode_sleb128(&mut buf, 0);
        assert_eq!(buf, vec![0]);

        buf.clear();
        encode_sleb128(&mut buf, -1);
        assert_eq!(buf, vec![0x7F]);

        buf.clear();
        encode_sleb128(&mut buf, 64);
        assert_eq!(buf, vec![0xC0, 0x00]);

        buf.clear();
        encode_sleb128(&mut buf, -64);
        assert_eq!(buf, vec![0x40]);
    }

    #[test]
    fn test_x86_64_eh_frame_integration() {
        // Generate a realistic eh_frame for a 64-byte function with 16-byte frame
        let func_start = 0x401000u64;
        let func_size = 64u64;
        let frame_size = 16u32;
        let frame = generate_x86_64_eh_frame(func_start, func_size, frame_size);
        assert!(frame.len() > 32, "eh_frame should have CIE + FDE + terminator");

        // Verify CIE structure at offset 0
        let cie_len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
        assert!(cie_len > 0, "CIE length should be > 0");
        let cie_id = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
        assert_eq!(cie_id, 0, "CIE_id should be 0");

        // Verify FDE follows CIE
        let fde_offset = 4 + cie_len as usize;
        assert!(fde_offset < frame.len() - 8, "FDE should exist after CIE");
        let fde_len = u32::from_le_bytes([
            frame[fde_offset], frame[fde_offset + 1],
            frame[fde_offset + 2], frame[fde_offset + 3],
        ]);
        assert!(fde_len > 0, "FDE length should be > 0");

        // Verify terminator at end of frame (last 4 bytes = CIE length 0)
        assert_eq!(
            &frame[frame.len() - 4..],
            &[0u8, 0, 0, 0],
            "eh_frame should end with zero-length CIE terminator"
        );
    }

    #[test]
    fn test_different_frame_sizes() {
        for &fs in &[0u32, 16, 32, 128, 256] {
            let frame = generate_x86_64_eh_frame(0x1000, 128, fs);
            assert!(frame.len() > 24, "eh_frame for frame_size={fs} should be valid");
            let cie_len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
            assert!(cie_len > 0, "frame_size={fs}: CIE length should be > 0");
        }
    }

    /// JIT pipeline integration: compile a real function and generate eh_frame.
    #[test]
    fn test_eh_frame_from_compiled_function() {
        use crate::backend::x86_64::{X86Isa, ensure_registered};
        use crate::backend::FunctionCompiler;
        use crate::ir::*;

        ensure_registered();

        // Compile a simple function
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut builder = crate::FunctionBuilder::new("eh_test", sig);
        let (entry, params) = builder.create_block_with_params(&[(Type::I32, "x")]);
        builder.switch_to_block(entry);
        let two = builder.iconst_i32(2);
        let result = builder.imul(params[0], two);
        builder.return_(&[result]); // Doubles the input

        let func = builder.finish();
        let compiled = FunctionCompiler::<X86Isa>::compile_raw(&func)
            .expect("compile should succeed");

        // Generate eh_frame for the compiled function
        let eh_frame = generate_x86_64_eh_frame(
            0x1000,                      // hypothetical load address
            compiled.code.len() as u64,  // function size
            16,                          // frame size (RBP push + alignment)
        );

        // Verify eh_frame structure
        assert!(eh_frame.len() > 32, "eh_frame for compiled func should be valid");
        let cie_len = u32::from_le_bytes([eh_frame[0], eh_frame[1], eh_frame[2], eh_frame[3]]);
        assert!(cie_len > 0, "CIE length should be > 0");
        let cie_id = u32::from_le_bytes([eh_frame[4], eh_frame[5], eh_frame[6], eh_frame[7]]);
        assert_eq!(cie_id, 0, "CIE_id should be 0");

        // Verify FDE exists after CIE
        let fde_offset = 4 + cie_len as usize;
        let fde_len = u32::from_le_bytes([
            eh_frame[fde_offset], eh_frame[fde_offset + 1],
            eh_frame[fde_offset + 2], eh_frame[fde_offset + 3],
        ]);
        assert!(fde_len > 0, "FDE length should be > 0");

        // Verify terminator at end of frame
        assert_eq!(
            &eh_frame[eh_frame.len() - 4..],
            &[0u8, 0, 0, 0],
            "eh_frame should end with zero-length CIE terminator"
        );
    }
}
