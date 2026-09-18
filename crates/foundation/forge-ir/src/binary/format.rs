//! 二进制格式的**唯一事实源**：魔数、版本、段 id 与段名。
//!
//! 字节级规范见 `docs/plans/forge-ir-binary-serialization-plan.md` §2（参考文档
//! `docs/reference/binary-format.md` 在 B5 落地）。本文件只放**常量与枚举**，
//! 读写实现分别在 [`super::writer`] / [`super::reader`]。

/// 文件魔数（8 字节，含 NUL 终止）。
pub const MAGIC: [u8; 8] = *b"FORGEIR\0";

/// 当前格式版本。
///
/// 写侧写入、读侧比对；不相等即 `IrError::BinaryDecode`——**不做兼容层**
/// （仓库既有决策"无需兼容旧版本结构"）。演进规则：只允许"加段 + 升版本"，
/// 已发布段的字段顺序不再改。
pub const IR_FORMAT_VERSION: u16 = 1;

/// producer 串（写入头部，**仅用于诊断**；跨版本不保证字节流逐字节相同）。
///
/// 头部自包含（varint 长度 + UTF-8），不引用字符串表：头部诊断不依赖任何段。
pub const PRODUCER: &str = concat!("forge-ir ", env!("CARGO_PKG_VERSION"));

/// 段 id。
///
/// **顺序即写入顺序**（依赖：前者是后者的输入），也是段表与段体的排序键。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum SectionId {
    /// 兼容位标记（`flags` varint，当前恒为 0）。
    Compat = 0x00,
    /// 字符串表（索引 0 恒为空串）。
    Strings = 0x01,
    /// 类型存储：内建类型表 + 命名类型 + `DataLayout` + 签名表。
    Types = 0x02,
    /// 常量池 6 通道。
    Consts = 0x03,
    /// 元数据节点表。
    Metadata = 0x04,
    /// 函数表（签名 + `dfg` + `layout` + 属性）。
    Funcs = 0x05,
    /// 全局变量、别名、comdat、符号信息。
    Globals = 0x06,
    /// 模块级事实：目标三元组、`source_filename`、模块 asm、函数名表。
    Module = 0x07,
}

impl SectionId {
    /// 全部段（按写入顺序）。
    pub const ALL: [SectionId; 8] = [
        SectionId::Compat,
        SectionId::Strings,
        SectionId::Types,
        SectionId::Consts,
        SectionId::Metadata,
        SectionId::Funcs,
        SectionId::Globals,
        SectionId::Module,
    ];

    /// 由字节还原段 id；未知取值返回 `None`（读侧据此 fail-closed，**不跳过**）。
    pub const fn from_u8(byte: u8) -> Option<Self> {
        Some(match byte {
            0x00 => SectionId::Compat,
            0x01 => SectionId::Strings,
            0x02 => SectionId::Types,
            0x03 => SectionId::Consts,
            0x04 => SectionId::Metadata,
            0x05 => SectionId::Funcs,
            0x06 => SectionId::Globals,
            0x07 => SectionId::Module,
            _ => return None,
        })
    }

    /// 段 id 的字节值。
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// 段名（错误消息与 `BinaryCompat` 展示用）。
    pub const fn name(self) -> &'static str {
        match self {
            SectionId::Compat => "COMPAT",
            SectionId::Strings => "STRINGS",
            SectionId::Types => "TYPES",
            SectionId::Consts => "CONSTS",
            SectionId::Metadata => "METADATA",
            SectionId::Funcs => "FUNCS",
            SectionId::Globals => "GLOBALS",
            SectionId::Module => "MODULE",
        }
    }
}
