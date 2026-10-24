//! 链接层重定位类型表 —— **目标约定数据**（v27 M6-98）。
//!
//! 对象文件里"这段补丁对应哪种链接器重定位"是**目标工具链事实**（LLVM 的
//! `*ELFObjectWriter::getRelocType`、rustc 的 `Target`/`TargetOptions` 都在目标层），
//! 不是 ISA 编码事实，也不该在通用库（`forge-object`）里按 `Architecture` 写分支。
//!
//! 数据来源：`conventions/*.toml` 的 `[link.<format>]` 段（见
//! `docs/reference/calling-conventions.md`）；`forge-abi` 解析后产出本表，宿主
//! （`forge-rustc`）把它交给 `forge_object::ObjectWriter`。表里没有的
//! `(format, 重定位名)` 回退到 `object` crate 的 Generic 映射（x86 现状）。
//!
//! 条目字段（与 TOML 一一对应）：
//!
//! - `name`：**ISA 侧语义名** = `[asm.reloc.<名>]` 的表头（如 `call26`），不是宽度；
//!   同名最多一条（按 format 分开查）。
//! - `rtype`：链接器重定位类型号（ELF `r_type` / COFF `typ` / MachO `r_type`）。
//! - `clear_mask`：链接器要重写的位段（如 A64 `BL` 的 imm26 = `0x03FF_FFFF`）。
//!   写对象前按 `word &= !clear_mask` 清零——ELF 的 addend 在重定位项（RELA），
//!   COFF/MachO 的 addend 在字段里，我们的语义名条目一律按"字段已清零"处理。

/// 对象文件格式（与 `conventions/*.toml` 的 `[link.<format>]` 段同名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LinkFormat {
    Elf,
    Coff,
    MachO,
}

impl LinkFormat {
    /// TOML 段名 / 诊断名。
    pub fn as_str(self) -> &'static str {
        match self {
            LinkFormat::Elf => "elf",
            LinkFormat::Coff => "coff",
            LinkFormat::MachO => "macho",
        }
    }

    /// 从段名解析（`conventions` 校验用）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "elf" => Some(LinkFormat::Elf),
            "coff" => Some(LinkFormat::Coff),
            "macho" => Some(LinkFormat::MachO),
            _ => None,
        }
    }
}

/// 一条 `(格式, ISA 语义名) → 链接器重定位类型`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkReloc {
    pub format: LinkFormat,
    /// ISA `[asm.reloc.<名>]` 的语义名（如 `call26` / `abs64`）。
    pub name: String,
    /// 链接器重定位类型号。
    pub rtype: u32,
    /// 链接器会重写的位段（写对象前清零）；0 = 不需要清零。
    pub clear_mask: u32,
}

/// 目标约定的链接层重定位表（空表 = 全部走 Generic 映射）。
#[derive(Debug, Clone, Default)]
pub struct LinkRelocTable {
    entries: Vec<LinkReloc>,
}

impl LinkRelocTable {
    pub fn new(entries: Vec<LinkReloc>) -> Self {
        Self { entries }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[LinkReloc] {
        &self.entries
    }

    /// 按 `(格式, 语义名)` 查；同名重复以**先插入**为准（`forge-abi` 注册时已去重）。
    pub fn lookup(&self, format: LinkFormat, name: &str) -> Option<&LinkReloc> {
        self.entries
            .iter()
            .find(|e| e.format == format && e.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_is_format_scoped() {
        let t = LinkRelocTable::new(vec![
            LinkReloc {
                format: LinkFormat::Elf,
                name: "call26".into(),
                rtype: 283,
                clear_mask: 0x03FF_FFFF,
            },
            LinkReloc {
                format: LinkFormat::Coff,
                name: "call26".into(),
                rtype: 0x000B,
                clear_mask: 0x03FF_FFFF,
            },
        ]);
        assert_eq!(t.lookup(LinkFormat::Elf, "call26").unwrap().rtype, 283);
        assert_eq!(t.lookup(LinkFormat::Coff, "call26").unwrap().rtype, 0x000B);
        assert!(t.lookup(LinkFormat::MachO, "call26").is_none());
        assert!(t.lookup(LinkFormat::Elf, "jump26").is_none());
    }
}
