//! ISA 信息与能力声明 — v9 运行时反射接口。
//!
//! 提供运行时查询 ISA 元信息、能力、格式和寄存器类的标准接口。
//! 这些类型由 ISA-DSL 编译生成，也可手动实现。

use crate::ir::Endianness;

// ============================================================
// IsaCapabilities — ISA 能力声明
// ============================================================

/// ISA 架构能力声明。
///
/// 对应于 DSL 中 `[meta.capabilities]` 节的运行时表示。
/// 用于驱动代码生成器的行为选择（如是否为变长指令、是否有前缀层等）。
#[derive(Debug, Clone)]
pub struct IsaCapabilities {
    /// 指令是否可变长度（x86 为 true，RISC-V/ARM 为 false）。
    pub variable_length: bool,
    /// 固定指令宽度（字节）。0 表示变长指令（此时应参考 min/max_inst_len）。
    pub fixed_inst_size: u32,
    /// 前缀层级数量（x86: 4，RISC-V: 0）。
    pub prefix_layers: u8,
    /// 支持的寻址模式列表。
    pub addressing_modes: &'static [&'static str],
    /// 支持的 SIMD 宽度列表（如 [128, 256, 512]）。
    pub simd_widths: &'static [u16],
    /// 是否支持掩码寄存器（AVX-512 k0-k7）。
    pub mask_registers: bool,
    /// 是否支持广播（AVX-512）。
    pub broadcast: bool,
    /// 是否支持舍入模式（AVX-512）。
    pub rounding_mode: bool,
    /// 端序。
    pub endianness: Endianness,
    /// 最小指令长度（字节）。
    pub min_inst_len: u8,
    /// 最大指令长度（字节）。
    pub max_inst_len: u8,
}

impl IsaCapabilities {
    /// 快速判断是否为定长指令集（RISC-V, ARM AArch64 等）。
    pub fn is_fixed_width(&self) -> bool {
        !self.variable_length
    }

    /// 快速判断是否为变长指令集（x86 等）。
    pub fn is_variable_width(&self) -> bool {
        self.variable_length
    }

    /// 是否有 SIMD 支持。
    pub fn has_simd(&self) -> bool {
        !self.simd_widths.is_empty()
    }

    /// 是否有前缀层。
    pub fn has_prefixes(&self) -> bool {
        self.prefix_layers > 0
    }

    /// 是否有掩码寄存器（AVX-512 风格）。
    pub fn has_mask_registers(&self) -> bool {
        self.mask_registers
    }
}

// ============================================================
// FormatInfo — 指令格式运行时信息
// ============================================================

/// 指令格式的运行时描述。
#[derive(Debug, Clone)]
pub struct FormatInfo {
    /// 格式名（如 "R", "I", "modrm"）。
    pub name: &'static str,
    /// 格式总宽度（位）。0 表示可变宽度。
    pub width_bits: u32,
    /// 是否可变宽度。
    pub variable_width: bool,
    /// 字段列表。
    pub fields: &'static [FieldInfo],
}

/// 指令格式中的字段信息。
#[derive(Debug, Clone)]
pub struct FieldInfo {
    /// 字段名。
    pub name: &'static str,
    /// 字段宽度（位）。
    pub width: u8,
    /// 在指令中的起始偏移（位）。
    pub offset: u32,
    /// 字段的固定值（None 表示可变字段/操作数）。
    pub fixed_value: Option<u64>,
}

// ============================================================
// RegisterClassInfo — 寄存器类运行时信息
// ============================================================

/// 寄存器类的运行时描述。
#[derive(Debug, Clone)]
pub struct RegisterClassInfo {
    /// 寄存器类名称（如 "gpr", "xmm", "ymm", "zmm", "k"）。
    pub name: &'static str,
    /// 该类的寄存器数量。
    pub count: u16,
    /// 每个寄存器的宽度（位）。
    pub width: u16,
    /// 寄存器前缀（如 "r", "xmm", "ymm", "zmm", "k"）。
    pub prefix: &'static str,
    /// 该类别对应的 RegClass 枚举值。
    pub reg_class: crate::ir::RegClass,
}

// ============================================================
// IsaInfo trait — ISA 元信息
// ============================================================

/// ISA 元信息 trait — 运行时反射 ISA 名称、能力和结构。
///
/// 由 ISA-DSL 编译生成的 ISA 结构体自动实现此 trait。
/// 手动实现的 ISA 也应实现此 trait 以提供自描述能力。
pub trait IsaInfo: Send + Sync + 'static {
    /// ISA 名称（如 "x86_64", "riscv32", "arm_aarch64"）。
    fn name() -> &'static str;

    /// ISA 版本字符串（如 "10.0"）。
    fn version() -> &'static str { "" }

    /// 地址大小（32 或 64 位）。
    fn address_size() -> u8 { 64 }

    /// 架构能力声明。
    fn capabilities() -> IsaCapabilities;

    /// 指令格式列表。
    fn formats() -> &'static [FormatInfo];

    /// 寄存器类列表。
    fn register_classes() -> &'static [RegisterClassInfo];

    /// 指令总数。
    fn num_instructions() -> usize;
}
