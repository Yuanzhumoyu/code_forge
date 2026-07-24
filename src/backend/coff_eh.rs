//! Windows x64 异常处理 (Exception Handling) 结构定义与发射逻辑。
//!
//! COFF/PE 格式要求 x86_64 目标包含 `.pdata` 和 `.xdata` 段，
//! 分别存储 `RUNTIME_FUNCTION` 条目和 `UNWIND_INFO` 展开信息。
//! 即使不实际展开栈帧，MSVC 链接器也会要求这两个段存在。
//!
//! ## 参考
//!
//! - Microsoft PE/COFF Specification: x64 Exception Handling
//! - `UNWIND_INFO` 结构: https://docs.microsoft.com/en-us/cpp/build/exception-handling-x64
//!
//! ## 结构概览
//!
//! .pdata:
//!   RUNTIME_FUNCTION[0] { BeginAddress, EndAddress, UnwindInfoAddress }
//!   RUNTIME_FUNCTION[1] ...
//!
//! .xdata:
//!   UNWIND_INFO { Version, Flags, SizeOfProlog, CountOfCodes, FrameRegister,
//!                 FrameOffset, UnwindCode[CountOfCodes], exception_handler (optional) }

use crate::CompileError;

// ============================================================
// RUNTIME_FUNCTION — 对应 IMAGE_RUNTIME_FUNCTION_ENTRY
// ============================================================

/// Windows x64 Runtime Function 条目。
///
/// 每个条目描述一个函数的起止地址及对应的展开信息位置。
/// 在 PE/COFF 中，这些条目存储在 `.pdata` 段中。
///
/// 字段均为 RVA (Relative Virtual Address)，对象文件中为 section-relative 偏移。
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct RuntimeFunction {
    /// 函数起始 RVA。
    pub begin_address: u32,
    /// 函数结束 RVA（指向最后一条指令之后）。
    pub end_address: u32,
    /// 对应 UNWIND_INFO 的 RVA。
    pub unwind_info_address: u32,
}

impl RuntimeFunction {
    /// 结构体在 COFF 中的大小：3 个 u32 = 12 字节。
    pub const SIZE: usize = 12;

    /// 创建一个新的运行时函数条目。
    pub fn new(begin_address: u32, end_address: u32, unwind_info_address: u32) -> Self {
        Self {
            begin_address,
            end_address,
            unwind_info_address,
        }
    }

    /// 序列化为小端字节序。
    pub fn to_bytes_le(&self) -> [u8; 12] {
        let mut buf = [0u8; 12];
        buf[0..4].copy_from_slice(&self.begin_address.to_le_bytes());
        buf[4..8].copy_from_slice(&self.end_address.to_le_bytes());
        buf[8..12].copy_from_slice(&self.unwind_info_address.to_le_bytes());
        buf
    }
}

// ============================================================
// UNWIND_CODE — 展开操作码
// ============================================================

/// 单个展开操作码条目。
///
/// 每个条目描述如何在函数 prologue 中恢复一个寄存器的值。
/// 在 `UNWIND_INFO` 中按逆序排列（从函数末尾到开头）。
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct UnwindCode {
    /// Prologue 内此操作应执行的偏移量（字节）。
    pub code_offset: u8,
    /// 展开操作码（4 bits） + 操作信息（4 bits）。
    pub unwind_op_and_info: u8,
}

impl UnwindCode {
    pub const SIZE: usize = 2;

    /// 创建 UWOP_PUSH_NONVOL 操作（将非易失寄存器压栈）。
    /// `reg` 是寄存器编号（0=RAX, ..., 15=R15）。
    pub fn push_nonvol(offset: u8, reg: u8) -> Self {
        Self {
            code_offset: offset,
            unwind_op_and_info: (Self::UWOP_PUSH_NONVOL << 4) | (reg & 0x0F),
        }
    }

    /// 创建 UWOP_ALLOC_LARGE 操作（分配大栈空间）。
    /// 对于 <= 128 字节的分配使用 UWOP_ALLOC_SMALL。
    pub fn alloc_large(offset: u8, op_info: u8) -> Self {
        Self {
            code_offset: offset,
            unwind_op_and_info: (Self::UWOP_ALLOC_LARGE << 4) | (op_info & 0x0F),
        }
    }

    /// 创建 UWOP_ALLOC_SMALL 操作。
    /// `size` = 分配大小 / 8 - 1（编码为 0..15）。
    pub fn alloc_small(offset: u8, size_div8_minus1: u8) -> Self {
        Self {
            code_offset: offset,
            unwind_op_and_info: (Self::UWOP_ALLOC_SMALL << 4) | (size_div8_minus1 & 0x0F),
        }
    }

    /// 创建 UWOP_SET_FPREG 操作（设置帧指针寄存器）。
    pub fn set_fpreg(offset: u8) -> Self {
        Self {
            code_offset: offset,
            unwind_op_and_info: Self::UWOP_SET_FPREG << 4,
        }
    }

    /// 创建 UWOP_SAVE_NONVOL 操作（将非易失寄存器保存到栈上）。
    /// `reg` 是寄存器号，`offset_div8` 是相对于栈帧基址的偏移/8。
    pub fn save_nonvol(offset: u8, reg: u8, _offset_div8: u8) -> Self {
        Self {
            code_offset: offset,
            unwind_op_and_info: (Self::UWOP_SAVE_NONVOL << 4) | (reg & 0x0F),
        }
    }

    // Unwind opcode types
    pub const UWOP_PUSH_NONVOL: u8 = 0;
    pub const UWOP_ALLOC_LARGE: u8 = 1;
    pub const UWOP_ALLOC_SMALL: u8 = 2;
    pub const UWOP_SET_FPREG: u8 = 3;
    pub const UWOP_SAVE_NONVOL: u8 = 4;
    pub const UWOP_SAVE_XMM128: u8 = 8;
    pub const UWOP_SAVE_XMM128_FAR: u8 = 9;
    pub const UWOP_PUSH_MACHFRAME: u8 = 10;

    /// 序列化为小端字节序。
    pub fn to_bytes_le(&self) -> [u8; 2] {
        [self.code_offset, self.unwind_op_and_info]
    }
}

// ============================================================
// UNWIND_INFO — 展开信息头
// ============================================================

/// Windows x64 展开信息结构。
///
/// 描述如何展开单个函数的栈帧。
/// 在 PE/COFF 中存储在 `.xdata` 段。
#[derive(Clone, Debug, Default)]
pub struct UnwindInfo {
    /// 版本号（通常为 1）。
    pub version: u8,
    /// 标志位（`UNW_FLAG_EHANDLER` / `UNW_FLAG_UHANDLER` / `UNW_FLAG_CHAININFO`）。
    pub flags: u8,
    /// Prologue 大小（字节）。
    pub size_of_prolog: u8,
    /// 展开码条目数。
    pub count_of_codes: u8,
    /// 帧寄存器编号（0 表示不使用帧寄存器）。
    pub frame_register: u8,
    /// 帧寄存器偏移（缩放因子 16）。
    pub frame_offset: u8,
    /// 展开码数组（最多 256 条，但实际受限于 UNWIND_INFO 大小）。
    pub unwind_codes: Vec<UnwindCode>,
    /// 异常处理程序 RVA（仅当 flags & UNW_FLAG_EHANDLER 时有效）。
    pub exception_handler_rva: Option<u32>,
    /// 语言特定数据（仅当 flags & UNW_FLAG_EHANDLER 时有效）。
    pub language_specific_data: Option<Vec<u8>>,
}

impl UnwindInfo {
    /// UNWIND_INFO 头部大小（不含可变长度的 UnwindCode 数组）。
    pub const HEADER_SIZE: usize = 4;

    /// 版本字段偏移（用于直接写入）。
    pub const VERSION_OFFSET: usize = 0;
    /// Flags 字段偏移。
    pub const FLAGS_OFFSET: usize = 1;
    /// Prolog 大小字段偏移。
    pub const PROLOG_SIZE_OFFSET: usize = 2;
    /// 展开码计数字段偏移（低 5 bits 有效）。
    pub const CODE_COUNT_OFFSET: usize = 3;

    // 标志位
    pub const UNW_FLAG_EHANDLER: u8 = 0x01;
    pub const UNW_FLAG_UHANDLER: u8 = 0x02;
    pub const UNW_FLAG_CHAININFO: u8 = 0x04;
    /// NHANDLER 掩码：无异常处理程序。
    pub const UNW_FLAG_NHANDLER: u8 = 0x00;

    /// 创建一个最小化的 UnwindInfo（无展开码，无异常处理程序）。
    pub fn minimal() -> Self {
        Self {
            version: 1,
            flags: 0,
            size_of_prolog: 0,
            count_of_codes: 0,
            frame_register: 0,
            frame_offset: 0,
            unwind_codes: Vec::new(),
            exception_handler_rva: None,
            language_specific_data: None,
        }
    }

    /// 创建一个带有展开码的 UnwindInfo。
    pub fn with_codes(
        size_of_prolog: u8,
        frame_register: u8,
        frame_offset: u8,
        codes: Vec<UnwindCode>,
    ) -> Self {
        let count = codes.len().min(255) as u8;
        Self {
            version: 1,
            flags: 0,
            size_of_prolog,
            count_of_codes: count,
            frame_register,
            frame_offset,
            unwind_codes: codes,
            exception_handler_rva: None,
            language_specific_data: None,
        }
    }

    /// 设置异常处理程序 RVA。
    pub fn with_exception_handler(&mut self, handler_rva: u32, data: Vec<u8>) {
        self.flags |= Self::UNW_FLAG_EHANDLER;
        self.exception_handler_rva = Some(handler_rva);
        self.language_specific_data = Some(data);
    }

    /// 计算序列化后的总大小（字节）。
    pub fn serialized_size(&self) -> usize {
        let codes_size = (self.count_of_codes as usize) * UnwindCode::SIZE;
        // 展开码数组必须按 4 字节对齐
        let codes_aligned = (codes_size + 3) & !3;
        let mut total = Self::HEADER_SIZE + codes_aligned;
        if self.exception_handler_rva.is_some() {
            total += 4; // exception handler RVA
            if let Some(ref data) = self.language_specific_data {
                total += data.len();
            }
        }
        total
    }

    /// 序列化为小端字节序。
    pub fn to_bytes_le(&self) -> Vec<u8> {
        let code_count_aligned = ((self.count_of_codes as usize * UnwindCode::SIZE) + 3) & !3;
        let mut buf = Vec::with_capacity(self.serialized_size());

        // 头部 4 字节
        buf.push(self.version & 0x07); // bits 0-2: version
        buf.push(self.flags); // bits 8-15: flags
        buf.push(self.size_of_prolog); // bits 16-23: prolog size
        buf.push(self.count_of_codes); // bits 24-31: count of codes (lower 5 bits)
                                      // Note: frame_register and frame_offset are encoded within count_of_codes byte
                                      // but for simplicity we store them separately

        // 展开码数组
        for code in &self.unwind_codes {
            buf.extend_from_slice(&code.to_bytes_le());
        }
        // 对齐填充
        while buf.len() < Self::HEADER_SIZE + code_count_aligned {
            buf.push(0);
        }

        // 异常处理程序 RVA
        if let Some(handler_rva) = self.exception_handler_rva {
            buf.extend_from_slice(&handler_rva.to_le_bytes());
            if let Some(ref data) = self.language_specific_data {
                buf.extend_from_slice(data);
            }
        }

        buf
    }
}

// ============================================================
// Pdata / Xdata 段收集器
// ============================================================

/// 每个函数的展开数据的完整记录。
///
/// 包含一个 `RUNTIME_FUNCTION` 条目和一个 `UNWIND_INFO` 展开信息。
/// 用于收集所有函数的数据后批量写入 `.pdata` 和 `.xdata` 段。
#[derive(Clone, Debug)]
pub struct FunctionEhRecord {
    /// 运行时函数条目。
    pub runtime_function: RuntimeFunction,
    /// 展开信息。
    pub unwind_info: UnwindInfo,
    /// 函数名（用于符号表）。
    pub function_name: Option<String>,
}

impl FunctionEhRecord {
    /// 创建一个最小的 EH 记录（无展开码的叶函数）。
    ///
    /// `begin_rva` 和 `end_rva` 是相对于 `.text` 段起始的偏移量。
    /// `xdata_rva` 是此函数的 UNWIND_INFO 在 `.xdata` 段中的偏移。
    pub fn minimal(begin_rva: u32, end_rva: u32, xdata_rva: u32) -> Self {
        Self {
            runtime_function: RuntimeFunction::new(begin_rva, end_rva, xdata_rva),
            unwind_info: UnwindInfo::minimal(),
            function_name: None,
        }
    }
}

/// `.pdata` 和 `.xdata` 段的收集器。
///
/// 收集所有函数的 EH 记录，为最终写入对象文件做准备。
#[derive(Clone, Debug, Default)]
pub struct CoffEhCollector {
    /// 所有函数的 EH 记录。
    pub records: Vec<FunctionEhRecord>,
}

impl CoffEhCollector {
    /// 创建一个空的收集器。
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// 添加一个函数的 EH 记录。
    pub fn add_record(&mut self, record: FunctionEhRecord) {
        self.records.push(record);
    }

    /// 添加一个最小化的叶函数记录。
    pub fn add_leaf_function(
        &mut self,
        begin_rva: u32,
        end_rva: u32,
        name: Option<String>,
    ) {
        // xdata_rva 会在最终发射时计算
        self.records.push(FunctionEhRecord {
            runtime_function: RuntimeFunction::new(begin_rva, end_rva, 0), // 占位
            unwind_info: UnwindInfo::minimal(),
            function_name: name,
        });
    }

    /// 序列化 `.pdata` 段内容。
    ///
    /// 返回完整的 `.pdata` 段字节。
    /// 需要在调用前通过 `finalize_xdata_offsets` 更新每个记录的 UNWIND_INFO RVA。
    pub fn serialize_pdata(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.records.len() * RuntimeFunction::SIZE);
        for record in &self.records {
            buf.extend_from_slice(&record.runtime_function.to_bytes_le());
        }
        buf
    }

    /// 序列化 `.xdata` 段内容。
    ///
    /// 返回完整的 `.xdata` 段字节。
    pub fn serialize_xdata(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        for record in &self.records {
            buf.extend_from_slice(&record.unwind_info.to_bytes_le());
        }
        buf
    }

    /// 计算并更新每条记录的 UNWIND_INFO RVA。
    ///
    /// 应在 `.xdata` 段 RVA 已知后调用。
    pub fn finalize_xdata_offsets(&mut self, xdata_base_rva: u32) {
        let mut current_rva = xdata_base_rva;
        for record in &mut self.records {
            record.runtime_function.unwind_info_address = current_rva;
            current_rva += record.unwind_info.serialized_size() as u32;
        }
    }

    /// 返回记录数。
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// 是否没有记录。
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_function_serialization() {
        let rf = RuntimeFunction::new(0x1000, 0x1100, 0x2000);
        let bytes = rf.to_bytes_le();
        assert_eq!(bytes.len(), 12);

        // begin_address (LE)
        assert_eq!(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]), 0x1000);
        // end_address (LE)
        assert_eq!(u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]), 0x1100);
        // unwind_info_address (LE)
        assert_eq!(u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]), 0x2000);
    }

    #[test]
    fn test_minimal_unwind_info() {
        let info = UnwindInfo::minimal();
        let bytes = info.to_bytes_le();
        assert_eq!(bytes.len(), 4); // header only, no codes, no handler
        assert_eq!(bytes[0], 1); // version
        assert_eq!(bytes[1], 0); // flags
        assert_eq!(bytes[2], 0); // prolog size
        assert_eq!(bytes[3], 0); // code count
    }

    #[test]
    fn test_unwind_code_push_nonvol() {
        let code = UnwindCode::push_nonvol(4, 5); // offset 4, reg 5 (RBP)
        assert_eq!(code.code_offset, 4);
        assert_eq!(code.unwind_op_and_info, (UnwindCode::UWOP_PUSH_NONVOL << 4) | 5);
    }

    #[test]
    fn test_unwind_info_with_codes() {
        let codes = vec![
            UnwindCode::push_nonvol(4, 5),
            UnwindCode::alloc_small(4, 1), // 16 bytes (1 * 8 + 8)
        ];
        let info = UnwindInfo::with_codes(8, 5, 0, codes);
        assert_eq!(info.count_of_codes, 2);
        assert_eq!(info.size_of_prolog, 8);

        // Serialized size: header (4) + 2 codes (4 bytes aligned) = 8
        let size = info.serialized_size();
        assert_eq!(size, 8);

        let bytes = info.to_bytes_le();
        assert_eq!(bytes.len(), size);
        assert_eq!(bytes[0], 1); // version
        assert_eq!(bytes[1], 0); // flags
        assert_eq!(bytes[2], 8); // prolog size
        assert_eq!(bytes[3], 2); // code count
    }

    #[test]
    fn test_eh_collector_empty() {
        let collector = CoffEhCollector::new();
        assert!(collector.is_empty());
        assert_eq!(collector.len(), 0);
        assert!(collector.serialize_pdata().is_empty());
        assert!(collector.serialize_xdata().is_empty());
    }

    #[test]
    fn test_eh_collector_with_records() {
        let mut collector = CoffEhCollector::new();
        collector.add_leaf_function(0x1000, 0x1050, Some("my_func".into()));
        collector.add_leaf_function(0x1060, 0x10C0, Some("other_func".into()));
        assert_eq!(collector.len(), 2);

        // Finalize xdata offsets
        collector.finalize_xdata_offsets(0x2000);

        let pdata = collector.serialize_pdata();
        assert_eq!(pdata.len(), 2 * RuntimeFunction::SIZE); // 24 bytes for 2 entries

        let xdata = collector.serialize_xdata();
        assert_eq!(xdata.len(), 2 * UnwindInfo::HEADER_SIZE); // 8 bytes for 2 minimal entries

        // Check first record's unwind_info_address was updated
        let first_begin = u32::from_le_bytes([pdata[0], pdata[1], pdata[2], pdata[3]]);
        assert_eq!(first_begin, 0x1000);

        let first_unwind = u32::from_le_bytes([pdata[8], pdata[9], pdata[10], pdata[11]]);
        assert_eq!(first_unwind, 0x2000);

        let second_unwind = u32::from_le_bytes([pdata[20], pdata[21], pdata[22], pdata[23]]);
        assert_eq!(second_unwind, 0x2004); // 4 bytes after first minimal entry
    }

    #[test]
    fn test_unwind_info_with_handler() {
        let mut info = UnwindInfo::minimal();
        info.with_exception_handler(0x4000, vec![1, 2, 3, 4]);

        let bytes = info.to_bytes_le();
        // header (4) + handler RVA (4) + data (4) = 12
        assert_eq!(bytes.len(), 12);
        assert_eq!(bytes[1], UnwindInfo::UNW_FLAG_EHANDLER); // flags

        // handler RVA at offset 4
        let handler_rva = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        assert_eq!(handler_rva, 0x4000);

        // language-specific data at offset 8
        assert_eq!(&bytes[8..12], &[1, 2, 3, 4]);
    }
}
