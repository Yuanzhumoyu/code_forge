//! IsaInfo trait — ISA 静态元数据。
//!
//! 提供运行时查询 ISA 名称、版本、能力、格式、寄存器类的标准接口。
//! 由 DSL 或手动实现。

use forge_ir::Endianness;

// ============================================================
// IsaCapabilities — ISA 能力声明
// ============================================================

/// ISA 架构能力声明。
#[derive(Debug, Clone)]
pub struct IsaCapabilities {
    pub variable_length: bool,
    pub fixed_inst_size: u32,
    pub prefix_layers: u8,
    pub addressing_modes: &'static [&'static str],
    pub simd_widths: &'static [u16],
    pub mask_registers: bool,
    pub broadcast: bool,
    pub rounding_mode: bool,
    pub endianness: Endianness,
    pub min_inst_len: u8,
    pub max_inst_len: u8,
}

impl IsaCapabilities {
    pub fn is_fixed_width(&self) -> bool {
        !self.variable_length
    }
    pub fn is_variable_width(&self) -> bool {
        self.variable_length
    }
    pub fn has_simd(&self) -> bool {
        !self.simd_widths.is_empty()
    }
    pub fn has_prefixes(&self) -> bool {
        self.prefix_layers > 0
    }
    pub fn has_mask_registers(&self) -> bool {
        self.mask_registers
    }
}

// ============================================================
// FormatInfo — 指令格式运行时信息
// ============================================================

#[derive(Debug, Clone)]
pub struct FormatInfo {
    pub name: &'static str,
    pub width_bits: u32,
    pub variable_width: bool,
    pub fields: &'static [FieldInfo],
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: &'static str,
    pub width: u8,
    pub offset: u32,
    pub fixed_value: Option<u64>,
}

// ============================================================
// RegisterClassInfo — 寄存器类运行时信息
// ============================================================

#[derive(Debug, Clone)]
pub struct RegisterClassInfo {
    pub name: &'static str,
    pub count: u16,
    pub width: u16,
    pub prefix: &'static str,
    pub reg_class: forge_ir::RegClass,
}

// ============================================================
// IsaInfo trait
// ============================================================

/// ISA 元信息 trait — 运行时反射 ISA 名称、能力和结构。
pub trait IsaInfo: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn version(&self) -> &'static str {
        ""
    }
    fn address_size(&self) -> u8 {
        64
    }
    fn endianness(&self) -> Endianness {
        Endianness::Little
    }
    fn capabilities(&self) -> IsaCapabilities;
    fn formats(&self) -> &'static [FormatInfo] {
        &[]
    }
    fn register_classes(&self) -> &'static [RegisterClassInfo] {
        &[]
    }
    fn num_instructions(&self) -> usize {
        0
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_class_info_basic() {
        let info = RegisterClassInfo {
            name: "GPR",
            count: 16,
            width: 8,
            prefix: "r",
            reg_class: forge_ir::RegClass::GPR,
        };
        assert_eq!(info.name, "GPR");
        assert_eq!(info.count, 16);
        assert_eq!(info.width, 8);
    }

    #[test]
    fn test_capabilities_defaults() {
        let caps = IsaCapabilities {
            variable_length: true,
            fixed_inst_size: 0,
            prefix_layers: 1,
            addressing_modes: &["reg", "reg+imm"],
            simd_widths: &[128, 256],
            mask_registers: false,
            broadcast: false,
            rounding_mode: false,
            endianness: Endianness::Little,
            min_inst_len: 1,
            max_inst_len: 1,
        };
        assert!(caps.variable_length);
        assert_eq!(caps.addressing_modes.len(), 2);
    }
}
