//! 对象文件写入器 — 将编译结果写入 ELF / PE / Mach-O 格式。
//!
//! 需要启用 `object-file` feature。
//!
//! # Example
//! ```ignore
//! use codegen_lib::prelude::*;
//! use codegen_lib::object_writer::ObjectWriter;
//! use codegen_lib::target::TargetConfig;
//!
//! let config = TargetConfig::host()?;
//! let mut writer = ObjectWriter::new(&config)?;
//! writer.add_function("add", &compiled_func)?;
//! writer.write_to_file("output.o")?;
//! ```

use forge_codegen::{CompiledFunction, RelocKind};
use forge_ir::CompileError;
use object::write::{
    Object, Relocation, SectionId, StandardSegment, Symbol, SymbolId, SymbolSection,
};
use object::{
    Architecture, BinaryFormat, Endianness, RelocationEncoding, RelocationFlags,
    RelocationKind as ObjRelocKind, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
};
use std::path::Path;
use target_lexicon::Triple;

/// 将编译完成的函数写入标准对象文件。
pub struct ObjectWriter<'a> {
    obj: Object<'a>,
    text_section: SectionId,
    data_section: Option<SectionId>,
    rodata_section: Option<SectionId>,
    /// 目标文件格式。
    format: BinaryFormat,
    /// 目标架构。
    arch: Architecture,
    /// 已写入的函数名集合（防止重复符号）。
    written_symbols: std::collections::HashSet<String>,
    /// 已声明的外部符号名 -> SymbolId 映射（避免重复创建 UNDEF 条目）。
    declared_externs: std::collections::HashMap<String, SymbolId>,
}

impl<'a> ObjectWriter<'a> {
    /// 创建新的对象文件写入器。
    ///
    /// 根据目标三元组自动选择格式（ELF/PE/Mach-O）和架构。
    pub fn new(config: &crate::target::TargetConfig) -> Result<Self, CompileError> {
        let (format, arch, endianness) = map_triple(&config.triple)?;

        let mut obj = Object::new(format, arch, endianness);

        // 创建标准的 text / data / rodata 段
        let text_section = obj.add_section(
            obj.segment_name(StandardSegment::Text).to_vec(),
            b".text".to_vec(),
            SectionKind::Text,
        );

        let data_section = obj.add_section(
            obj.segment_name(StandardSegment::Data).to_vec(),
            b".data".to_vec(),
            SectionKind::Data,
        );

        let rodata_section = obj.add_section(
            obj.segment_name(StandardSegment::Data).to_vec(),
            b".rodata".to_vec(),
            SectionKind::ReadOnlyData,
        );

        Ok(Self {
            obj,
            text_section,
            data_section: Some(data_section),
            rodata_section: Some(rodata_section),
            format,
            arch,
            written_symbols: std::collections::HashSet::new(),
            declared_externs: std::collections::HashMap::new(),
        })
    }

    /// 创建一个不带默认 data/rodata section 的最小对象文件写入器。
    ///
    /// 适合只写入 text 内容的场景。
    pub fn new_text_only(config: &crate::target::TargetConfig) -> Result<Self, CompileError> {
        let (format, arch, endianness) = map_triple(&config.triple)?;

        let mut obj = Object::new(format, arch, endianness);

        let text_section = obj.add_section(
            obj.segment_name(StandardSegment::Text).to_vec(),
            b".text".to_vec(),
            SectionKind::Text,
        );

        Ok(Self {
            obj,
            text_section,
            data_section: None,
            rodata_section: None,
            format,
            arch,
            written_symbols: std::collections::HashSet::new(),
            declared_externs: std::collections::HashMap::new(),
        })
    }

    /// 添加一个已编译函数到对象文件的 `.text` 段。
    ///
    /// 函数名将成为对象文件中的全局符号。
    pub fn add_function(
        &mut self,
        name: &str,
        func: &CompiledFunction,
    ) -> Result<(), CompileError> {
        self.add_function_with_alignment(name, func, 1)
    }

    /// 添加一个已编译函数并指定对齐要求。
    pub fn add_function_with_alignment(
        &mut self,
        name: &str,
        func: &CompiledFunction,
        alignment: u64,
    ) -> Result<(), CompileError> {
        if self.written_symbols.contains(name) {
            return Err(CompileError::Emit(format!(
                "duplicate symbol: '{name}' already exists in this object file"
            )));
        }

        // 创建符号（value/size 占位，后续由 add_symbol_data 更新）
        let sym_id = self.obj.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value: 0,
            size: func.code.len() as u64,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(self.text_section),
            flags: SymbolFlags::None,
        });

        // add_symbol_data 追加数据到 section，返回数据在 section 内的偏移量，
        // 同时自动更新符号的 value/size/section 字段
        let actual_offset =
            self.obj
                .add_symbol_data(sym_id, self.text_section, &func.code, alignment);

        // 处理重定位条目（偏移基于 actual_offset）
        for reloc in &func.relocations {
            self.add_relocation_to_section(
                self.text_section,
                actual_offset + reloc.offset as u64,
                &reloc.kind,
                &reloc.symbol,
                reloc.addend,
            )?;
        }

        self.written_symbols.insert(name.to_string());
        Ok(())
    }

    /// 添加只读数据到 `.rodata` 段。
    pub fn add_rodata(
        &mut self,
        name: &str,
        data: &[u8],
        alignment: u64,
    ) -> Result<(), CompileError> {
        let section = self.rodata_section.ok_or_else(|| {
            CompileError::Emit(
                "rodata section not available (use new() not new_text_only())".into(),
            )
        })?;

        let sym_id = self.obj.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value: 0,
            size: data.len() as u64,
            kind: SymbolKind::Data,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(section),
            flags: SymbolFlags::None,
        });

        // add_symbol_data 自动更新符号的 value/size/section
        self.obj.add_symbol_data(sym_id, section, data, alignment);
        Ok(())
    }

    /// 添加可读写数据到 `.data` 段。
    pub fn add_data(
        &mut self,
        name: &str,
        data: &[u8],
        alignment: u64,
    ) -> Result<(), CompileError> {
        let section = self.data_section.ok_or_else(|| {
            CompileError::Emit("data section not available (use new() not new_text_only())".into())
        })?;

        let sym_id = self.obj.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value: 0,
            size: data.len() as u64,
            kind: SymbolKind::Data,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(section),
            flags: SymbolFlags::None,
        });

        // add_symbol_data 自动更新符号的 value/size/section
        self.obj.add_symbol_data(sym_id, section, data, alignment);
        Ok(())
    }

    /// 将对象文件写入字节缓冲区。
    pub fn write(&self, writer: impl std::io::Write) -> Result<(), CompileError> {
        self.obj
            .write_stream(writer)
            .map_err(|e| CompileError::Emit(format!("failed to write object file: {e}")))
    }

    /// 将对象文件写入磁盘。
    pub fn write_to_file(&self, path: impl AsRef<Path>) -> Result<(), CompileError> {
        let mut file = std::fs::File::create(path.as_ref())
            .map_err(|e| CompileError::Emit(format!("failed to create object file: {e}")))?;
        self.write(&mut file)
    }

    /// 写入到 Vec<u8>。
    pub fn write_to_vec(&self) -> Result<Vec<u8>, CompileError> {
        let mut buf = Vec::new();
        self.write(&mut buf)?;
        Ok(buf)
    }

    // ---- 内部方法 ----

    fn add_relocation_to_section(
        &mut self,
        section: SectionId,
        offset: u64,
        kind: &RelocKind,
        symbol_name: &str,
        addend: i64,
    ) -> Result<(), CompileError> {
        // 确保目标符号已声明（外部符号）
        let target_sym = self.find_or_declare_symbol(symbol_name);

        let flags = map_reloc_flags(kind, self.format, self.arch);

        // 添加重定位到 section
        self.obj
            .add_relocation(
                section,
                Relocation {
                    offset,
                    symbol: target_sym,
                    addend,
                    flags,
                },
            )
            .map_err(|e| CompileError::Emit(format!("failed to add relocation: {e}")))
    }

    fn find_or_declare_symbol(&mut self, name: &str) -> SymbolId {
        use std::collections::hash_map::Entry;
        match self.declared_externs.entry(name.to_string()) {
            Entry::Occupied(entry) => *entry.get(), // 已存在，复用 SymbolId
            Entry::Vacant(entry) => {
                let sym_id = self.obj.add_symbol(Symbol {
                    name: name.as_bytes().to_vec(),
                    value: 0,
                    size: 0,
                    kind: SymbolKind::Unknown,
                    scope: SymbolScope::Linkage,
                    weak: false,
                    section: SymbolSection::Undefined,
                    flags: SymbolFlags::None,
                });
                entry.insert(sym_id);
                sym_id
            }
        }
    }
}

// ============================================================
// 辅助函数
// ============================================================

/// 将 target_lexicon::Triple 映射到 object crate 的类型。
fn map_triple(triple: &Triple) -> Result<(BinaryFormat, Architecture, Endianness), CompileError> {
    let format = match triple.binary_format {
        target_lexicon::BinaryFormat::Elf => BinaryFormat::Elf,
        target_lexicon::BinaryFormat::Coff => BinaryFormat::Coff,
        target_lexicon::BinaryFormat::Macho => BinaryFormat::MachO,
        target_lexicon::BinaryFormat::Xcoff => BinaryFormat::Xcoff,
        _ => {
            return Err(CompileError::Unsupported(format!(
                "unsupported binary format in triple: {}",
                triple
            )));
        }
    };

    let arch = map_architecture(&triple.architecture)?;

    let endianness = match triple
        .endianness()
        .unwrap_or(target_lexicon::Endianness::Little)
    {
        target_lexicon::Endianness::Little => Endianness::Little,
        target_lexicon::Endianness::Big => Endianness::Big,
    };

    Ok((format, arch, endianness))
}

/// 将 target_lexicon 的架构映射到 object crate。
fn map_architecture(arch: &target_lexicon::Architecture) -> Result<Architecture, CompileError> {
    match arch {
        target_lexicon::Architecture::X86_64 => Ok(Architecture::X86_64),
        target_lexicon::Architecture::Aarch64(_) => Ok(Architecture::Aarch64),
        target_lexicon::Architecture::Arm(_) => Ok(Architecture::Arm),
        target_lexicon::Architecture::Riscv64(_) => Ok(Architecture::Riscv64),
        target_lexicon::Architecture::Riscv32(_) => Ok(Architecture::Riscv32),
        target_lexicon::Architecture::X86_32(_) => Ok(Architecture::I386),
        _ => Err(CompileError::Unsupported(format!(
            "unsupported architecture: {:?}",
            arch
        ))),
    }
}

/// 将 codegen-lib 的 RelocKind 映射到 object crate 的 RelocationFlags。
fn map_reloc_flags(kind: &RelocKind, format: BinaryFormat, _arch: Architecture) -> RelocationFlags {
    fn size_from_width(w: u8) -> u8 {
        match w {
            1 => 8,
            2 => 16,
            4 => 32,
            8 => 64,
            _ => 32,
        }
    }
    match kind {
        RelocKind::Absolute(w) => {
            if format == BinaryFormat::Coff {
                RelocationFlags::Coff {
                    typ: match w {
                        4 => object::pe::IMAGE_REL_AMD64_ADDR32,
                        8 => object::pe::IMAGE_REL_AMD64_ADDR64,
                        _ => object::pe::IMAGE_REL_AMD64_ADDR32,
                    },
                }
            } else if format == BinaryFormat::MachO {
                RelocationFlags::MachO {
                    r_type: object::macho::X86_64_RELOC_UNSIGNED,
                    r_pcrel: false,
                    r_length: match w {
                        4 => 2, // 32-bit
                        8 => 3, // 64-bit
                        _ => 2,
                    },
                }
            } else {
                RelocationFlags::Generic {
                    kind: ObjRelocKind::Absolute,
                    encoding: RelocationEncoding::Generic,
                    size: size_from_width(*w),
                }
            }
        }
        RelocKind::Relative(w, _) => {
            if format == BinaryFormat::Coff {
                RelocationFlags::Coff {
                    typ: match w {
                        4 => object::pe::IMAGE_REL_AMD64_REL32,
                        _ => object::pe::IMAGE_REL_AMD64_REL32,
                    },
                }
            } else if format == BinaryFormat::MachO {
                RelocationFlags::MachO {
                    r_type: object::macho::X86_64_RELOC_BRANCH,
                    r_pcrel: true,
                    r_length: match w {
                        4 => 2, // 32-bit
                        _ => 2,
                    },
                }
            } else {
                RelocationFlags::Generic {
                    kind: ObjRelocKind::Relative,
                    encoding: RelocationEncoding::Generic,
                    size: size_from_width(*w),
                }
            }
        }
        // ISA-specific fixups are now plain PC-relative; the backend's
        // RelocPatcher handles the encoding, so object output treats them
        // like any relative branch.
        #[allow(unreachable_patterns)]
        _ => {
            if format == BinaryFormat::MachO {
                RelocationFlags::MachO {
                    r_type: object::macho::X86_64_RELOC_BRANCH,
                    r_pcrel: true,
                    r_length: 2,
                }
            } else {
                RelocationFlags::Generic {
                    kind: ObjRelocKind::Relative,
                    encoding: RelocationEncoding::Generic,
                    size: 32,
                }
            }
        }
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use forge_codegen::CompiledFunction;

    fn make_test_func(code: Vec<u8>) -> CompiledFunction {
        CompiledFunction {
            code,
            relocations: Vec::new(),
            code_size: 0,
        }
    }

    #[test]
    fn test_create_elf_object() {
        let config = crate::target::TargetConfig::from_triple("x86_64-unknown-linux-gnu")
            .expect("valid triple");

        let mut writer = ObjectWriter::new(&config).expect("create writer");

        let func = make_test_func(vec![0x90, 0xc3]); // nop; ret
        writer
            .add_function("test_func", &func)
            .expect("add function");

        let data = writer.write_to_vec().expect("write to vec");
        // ELF 文件以 0x7F 'E' 'L' 'F' 魔数开头
        assert_eq!(&data[0..4], &[0x7f, b'E', b'L', b'F']);
        assert!(data.len() > 64); // 至少包含 ELF header
    }

    #[test]
    fn test_create_macho_object() {
        let config =
            crate::target::TargetConfig::from_triple("x86_64-apple-darwin").expect("valid triple");

        let mut writer = ObjectWriter::new(&config).expect("create writer");

        let func = make_test_func(vec![0x90, 0xc3]);
        writer
            .add_function("test_func", &func)
            .expect("add function");

        let data = writer.write_to_vec().expect("write to vec");
        // Mach-O 64-bit LE 魔数: 0xFEEDFACF
        // Mach-O 64-bit BE 魔数: 0xCFFAEDFE
        assert!(
            data[0..4] == [0xcf, 0xfa, 0xed, 0xfe]   // MH_MAGIC_64 LE
                || data[0..4] == [0xfe, 0xed, 0xfa, 0xcf] // MH_MAGIC_64 BE
        );
        assert!(data.len() > 64);
    }

    #[test]
    fn test_create_pe_object() {
        let config = crate::target::TargetConfig::from_triple("x86_64-pc-windows-msvc")
            .expect("valid triple");

        let mut writer = ObjectWriter::new(&config).expect("create writer");

        let func = make_test_func(vec![0x90, 0xc3]);
        writer
            .add_function("test_func", &func)
            .expect("add function");

        let data = writer.write_to_vec().expect("write to vec");
        // PE/COFF 对象文件以 machine type 开头（x86_64 = 0x8664 = [0x64, 0x86] LE）
        assert_eq!(&data[0..2], &[0x64, 0x86]);
    }

    #[test]
    fn test_duplicate_symbol() {
        let config = crate::target::TargetConfig::from_triple("x86_64-unknown-linux-gnu")
            .expect("valid triple");

        let mut writer = ObjectWriter::new(&config).expect("create writer");

        let func = make_test_func(vec![0x90, 0xc3]);
        writer.add_function("dup_func", &func).expect("first add");
        let err = writer.add_function("dup_func", &func).unwrap_err();
        assert!(format!("{err}").contains("duplicate symbol"));
    }

    #[test]
    fn test_add_rodata() {
        let config = crate::target::TargetConfig::from_triple("x86_64-unknown-linux-gnu")
            .expect("valid triple");

        let mut writer = ObjectWriter::new(&config).expect("create writer");

        writer
            .add_rodata("my_const", b"hello\0", 1)
            .expect("add rodata");

        let data = writer.write_to_vec().expect("write to vec");
        // ELF 字符串表中应包含符号名
        let data_str = String::from_utf8_lossy(&data);
        assert!(data_str.contains("my_const"));
    }

    #[test]
    fn test_invalid_architecture() {
        let result = map_architecture(&target_lexicon::Architecture::Unknown);
        assert!(result.is_err());
    }

    #[test]
    fn test_map_reloc_flags() {
        // ELF: Generic flags
        let flags = map_reloc_flags(&RelocKind::ABS4, BinaryFormat::Elf, Architecture::X86_64);
        assert!(matches!(flags, RelocationFlags::Generic { .. }));

        let flags = map_reloc_flags(&RelocKind::REL4, BinaryFormat::Elf, Architecture::X86_64);
        assert!(matches!(flags, RelocationFlags::Generic { .. }));
    }

    #[test]
    fn test_map_reloc_flags_coff() {
        // PE/COFF: X86_64 specific flags
        let flags = map_reloc_flags(&RelocKind::ABS8, BinaryFormat::Coff, Architecture::X86_64);
        if cfg!(windows) {
            assert!(matches!(flags, RelocationFlags::Coff { .. }));
        }

        let flags = map_reloc_flags(&RelocKind::REL4, BinaryFormat::Coff, Architecture::X86_64);
        assert!(matches!(flags, RelocationFlags::Coff { .. }));
    }

    #[test]
    fn test_map_reloc_flags_macho() {
        // Mach-O: x86_64 specific flags
        let flags = map_reloc_flags(&RelocKind::ABS8, BinaryFormat::MachO, Architecture::X86_64);
        assert!(matches!(flags, RelocationFlags::MachO { .. }));
    }
}
