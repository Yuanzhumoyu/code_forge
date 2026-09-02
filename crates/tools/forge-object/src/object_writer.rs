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
use forge_ir::ImmStr;
use forge_ir::IrError;
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
    written_symbols: std::collections::HashSet<ImmStr>,
    /// 已声明的外部符号名 -> SymbolId 映射（避免重复创建 UNDEF 条目）。
    declared_externs: std::collections::HashMap<ImmStr, SymbolId>,
    /// 已定义的符号名 -> SymbolId（本文件内 add_function/add_data 定义；
    /// reloc 引用同文件符号时复用定义而非重复声明 UNDEF——否则 COFF
    /// 符号表同名定义+UNDEF 重复，链接器不解析内部 reloc → call/jmp
    /// rel32 留 0 → 死循环。跨函数 call（monomorphized core 函数）触发）。
    defined_symbols: std::collections::HashMap<ImmStr, SymbolId>,
}

impl<'a> ObjectWriter<'a> {
    /// 创建新的对象文件写入器。
    ///
    /// 根据目标三元组自动选择格式（ELF/PE/Mach-O）和架构。
    pub fn new(config: &crate::target::TargetConfig) -> Result<Self, IrError> {
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
            defined_symbols: std::collections::HashMap::new(),
        })
    }

    /// 创建或升级符号为定义（Text/Data）：
    /// - 已定义（defined_symbols）→ 复用（防御；add_function 前有 duplicate 检查）；
    /// - 已声明 UNDEF（declared_externs，reloc 先引用）→ **升级复用**（改
    ///   section/kind/size），避免 COFF 同名定义+UNDEF 重复；
    /// - 否则新建定义符号。
    fn define_symbol(
        &mut self,
        name: &str,
        section: SectionId,
        kind: SymbolKind,
        size: u64,
    ) -> SymbolId {
        let key = ImmStr::from(name);
        if let Some(&id) = self.defined_symbols.get(&key) {
            return id;
        }
        let id = match self.declared_externs.remove(&key) {
            Some(id) => {
                let s = self.obj.symbol_mut(id);
                s.section = SymbolSection::Section(section);
                s.kind = kind;
                s.size = size;
                s.value = 0;
                s.scope = SymbolScope::Linkage;
                id
            }
            None => self.obj.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value: 0,
                size,
                kind,
                scope: SymbolScope::Linkage,
                section: SymbolSection::Section(section),
                flags: SymbolFlags::None,
                weak: false,
            }),
        };
        self.defined_symbols.insert(key, id);
        id
    }

    /// 添加一个已编译函数到对象文件的 `.text` 段。
    ///
    /// 函数名将成为对象文件中的全局符号。
    pub fn add_function(&mut self, name: &str, func: &CompiledFunction) -> Result<(), IrError> {
        self.add_function_with_alignment(name, func, 1)
    }

    /// 添加一个已编译函数并指定对齐要求。
    pub fn add_function_with_alignment(
        &mut self,
        name: &str,
        func: &CompiledFunction,
        alignment: u64,
    ) -> Result<(), IrError> {
        if self.written_symbols.contains(name) {
            return Err(IrError::Emit(format!(
                "duplicate symbol: '{name}' already exists in this object file"
            )));
        }

        // 创建符号（value/size 占位，后续由 add_symbol_data 更新）——
        // 已声明 UNDEF（reloc 先引用）则升级复用，避免同名定义+UNDEF 重复
        let sym_id = self.define_symbol(
            name,
            self.text_section,
            SymbolKind::Text,
            func.code.len() as u64,
        );

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

        self.written_symbols.insert(ImmStr::from(name));
        Ok(())
    }

    /// 添加只读数据到 `.rodata` 段。
    pub fn add_rodata(&mut self, name: &str, data: &[u8], alignment: u64) -> Result<(), IrError> {
        let section = self.rodata_section.ok_or_else(|| {
            IrError::Emit("rodata section not available (use new() not new_text_only())".into())
        })?;

        let sym_id = self.define_symbol(name, section, SymbolKind::Data, data.len() as u64);
        let offset = self.obj.add_symbol_data(sym_id, section, data, alignment);
        self.written_symbols.insert(ImmStr::from(name));
        let _ = offset;
        Ok(())
    }

    /// 添加可读写数据到 `.data` 段。
    pub fn add_data(&mut self, name: &str, data: &[u8], alignment: u64) -> Result<(), IrError> {
        self.add_data_with_relocs(name, data, &[], alignment)
    }

    /// 添加可读写数据到 `.data` 段，并附加数据内符号引用重定位。
    /// 注：vtable 指针表用 .data（而非 .rodata）——MSVC 链接器对 .rodata
    /// 段的 ADDR64 重定位不应用（实测链接后全 0），.data 段正常。
    pub fn add_data_with_relocs(
        &mut self,
        name: &str,
        data: &[u8],
        relocs: &[(usize, RelocKind, &str, i64)],
        alignment: u64,
    ) -> Result<(), IrError> {
        let section = self.data_section.ok_or_else(|| {
            IrError::Emit("data section not available (use new() not new_text_only())".into())
        })?;

        let sym_id = self.define_symbol(name, section, SymbolKind::Data, data.len() as u64);

        let actual_offset = self.obj.add_symbol_data(sym_id, section, data, alignment);

        for (off, kind, sym, addend) in relocs {
            self.add_relocation_to_section(
                section,
                actual_offset + *off as u64,
                kind,
                sym,
                *addend,
            )?;
        }
        Ok(())
    }

    /// 将对象文件写入字节缓冲区。
    pub fn write(&self, writer: impl std::io::Write) -> Result<(), IrError> {
        self.obj
            .write_stream(writer)
            .map_err(|e| IrError::Emit(format!("failed to write object file: {e}")))
    }

    /// 添加 DWARF 段（C1 DebugInfo line-tables-only）。
    ///
    /// `sections` 为 `(段名, 数据, reloc: (段内偏移, 符号名))` 列表。
    /// 调试段的地址属性（low_pc/行号 set_address）占位 0 + ADDR64 重定位
    /// 到函数符号——object crate 的 `add_relocation` 对 COFF 调试段同样
    /// 发射 section reloc（IMAGE_REL_AMD64_ADDR64），链接器解析为函数
    /// 地址。符号必须已定义（add_function 已登记）。
    pub fn add_dwarf(
        &mut self,
        sections: &[(&str, Vec<u8>, Vec<(usize, String)>)],
    ) -> Result<(), IrError> {
        for (name, data, relocs) in sections {
            let seg = self.obj.segment_name(StandardSegment::Data).to_vec();
            let id =
                self.obj.add_section(seg, name.as_bytes().to_vec(), SectionKind::Debug);
            let offset = self.obj.append_section_data(id, data, 1);
            for (off, sym) in relocs {
                let sym_id = self.defined_symbols.get(&ImmStr::from(sym.as_str())).copied();
                let Some(sym_id) = sym_id else {
                    return Err(IrError::Emit(format!(
                        "add_dwarf: symbol '{sym}' not defined (add_function first)"
                    )));
                };
                self.obj.add_relocation(
                    id,
                    Relocation {
                        offset: offset + *off as u64,
                        symbol: sym_id,
                        addend: 0,
                        flags: RelocationFlags::Generic {
                            kind: ObjRelocKind::Absolute,
                            encoding: RelocationEncoding::Generic,
                            size: 64,
                        },
                    },
                )
                .map_err(|e| {
                    IrError::Emit(format!("add_dwarf reloc for '{sym}': {e}"))
                })?;
            }
        }
        Ok(())
    }

    /// 将对象文件写入磁盘。
    pub fn write_to_file(&self, path: impl AsRef<Path>) -> Result<(), IrError> {
        let mut file = std::fs::File::create(path.as_ref())
            .map_err(|e| IrError::Emit(format!("failed to create object file: {e}")))?;
        self.write(&mut file)
    }

    /// 写入到 Vec<u8>。
    pub fn write_to_vec(&self) -> Result<Vec<u8>, IrError> {
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
    ) -> Result<(), IrError> {
        // 确保目标符号已声明（外部符号）
        let target_sym = self.find_or_declare_symbol(symbol_name);

        let flags = map_reloc_flags(kind, self.format, self.arch);

        // COFF 重定位的 addend 是**隐式**的——存在被重定位字段的原始数据里
        // （链接器按 S + A - P 系列公式计算，A 取字段初值）。我们的编码器在
        // 重定位槽写入占位值（call/jmp 的 rel32 槽 = -(f+1)；GlobalAddr 的
        // imm64 槽 = -(g+1)；均为 -1 量级），该占位会作为隐式 addend 残留：
        // - REL32：目标偏 -1（实测 0x10d0 vs wrapping_add 实际 0x10d1）；
        // - ADDR64：符号地址偏 -1（实测 0x140002fff vs .rodata 起始 0x3000）。
        //
        // 修复分两步（对 COFF 重定位统一清零字段 + 补偿 addend）：
        // 1. 清零被重定位字段（覆盖编码器占位，使隐式 addend = 0）；
        // 2. REL32 传 addend-4 给 object crate：其 coff_adjust_addend 对 REL32
        //    自动 +4（适配 MSVC target = S + A - (P+4) 公式），净 addend = 0 →
        //    不覆盖字段（write_relocation_addend 仅在 addend != 0 时写）→ 字段
        //    保持 0。若不反向补偿 -4，addend=4 会被写入，链接目标偏 +4。
        //    ADDR64/ADDR32 的 coff_adjust 为 0 → addend 不变（0）→ 同样不覆盖。
        let coff_flags = if self.format == BinaryFormat::Coff {
            match flags {
                RelocationFlags::Coff { typ } => Some(typ),
                _ => None,
            }
        } else {
            None
        };
        if let Some(typ) = coff_flags {
            let bytes = match typ {
                object::pe::IMAGE_REL_AMD64_REL32 => Some(4),
                object::pe::IMAGE_REL_AMD64_ADDR64 => Some(8),
                object::pe::IMAGE_REL_AMD64_ADDR32 => Some(4),
                _ => None,
            };
            if let Some(n) = bytes {
                let section = self.obj.section_mut(section);
                let data = section.data_mut();
                let start = offset as usize;
                if start + n <= data.len() {
                    data[start..start + n].copy_from_slice(&vec![0u8; n]);
                }
            }
        }
        let addend = if coff_flags == Some(object::pe::IMAGE_REL_AMD64_REL32) {
            addend - 4
        } else {
            addend
        };

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
            .map_err(|e| IrError::Emit(format!("failed to add relocation: {e}")))
    }

    fn find_or_declare_symbol(&mut self, name: &str) -> SymbolId {
        use std::collections::hash_map::Entry;
        let key = ImmStr::from(name);
        // 本文件已定义的符号（add_function/add_data）→ 复用定义，
        // 不重复声明 UNDEF（否则 COFF 同名定义+UNDEF → 链接器不解析内部 reloc）
        if let Some(&id) = self.defined_symbols.get(&key) {
            return id;
        }
        match self.declared_externs.entry(key) {
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
fn map_triple(triple: &Triple) -> Result<(BinaryFormat, Architecture, Endianness), IrError> {
    let format = match triple.binary_format {
        target_lexicon::BinaryFormat::Elf => BinaryFormat::Elf,
        target_lexicon::BinaryFormat::Coff => BinaryFormat::Coff,
        target_lexicon::BinaryFormat::Macho => BinaryFormat::MachO,
        target_lexicon::BinaryFormat::Xcoff => BinaryFormat::Xcoff,
        _ => {
            return Err(IrError::Unsupported(format!(
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
fn map_architecture(arch: &target_lexicon::Architecture) -> Result<Architecture, IrError> {
    match arch {
        target_lexicon::Architecture::X86_64 => Ok(Architecture::X86_64),
        target_lexicon::Architecture::Aarch64(_) => Ok(Architecture::Aarch64),
        target_lexicon::Architecture::Arm(_) => Ok(Architecture::Arm),
        target_lexicon::Architecture::Riscv64(_) => Ok(Architecture::Riscv64),
        target_lexicon::Architecture::Riscv32(_) => Ok(Architecture::Riscv32),
        target_lexicon::Architecture::X86_32(_) => Ok(Architecture::I386),
        _ => Err(IrError::Unsupported(format!(
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

    /// 构造一个模拟 x86 call 的函数：`e8 ?? ?? ?? ??`（rel32 槽占位 -1，
    /// 即编码器对函数符号写入的 -(f+1)）+ 一条 reloc（offset 1，REL4，
    /// 指向 callee 符号，addend 0）。
    fn make_call_func(callee: &str) -> forge_codegen::CompiledFunction {
        forge_codegen::CompiledFunction {
            code: vec![0xe8, 0xff, 0xff, 0xff, 0xff, 0xc3],
            relocations: vec![forge_codegen::Relocation {
                offset: 1,
                kind: RelocKind::REL4,
                symbol: ImmStr::from(callee),
                addend: 0,
            }],
            code_size: 0,
            line_entries: vec![],
        }
    }

    #[test]
    fn test_coff_rel32_clears_placeholder_addend() {
        // 回归测试（2026-09 e2e i64_wrapping_add 值错）：COFF REL32 的 addend
        // 是**隐式**的——存在被重定位字段（rel32 槽）的原始数据里。编码器在
        // rel32 槽写入占位 -(f+1)（本测试 -1），若不显式清零，该占位作为隐式
        // addend 残留，链接后 call 目标偏 -1（实测 0x10d0 vs wrapping_add 实际
        // 0x10d1）。修复：add_relocation_to_section 对 COFF REL32 显式清零槽。
        let config = crate::target::TargetConfig::from_triple("x86_64-pc-windows-msvc")
            .expect("valid triple");

        let mut writer = ObjectWriter::new(&config).expect("create writer");
        // callee 先定义（作为 .text 第一个函数），caller 后添加（其 rel32 槽
        // 相对 callee 偏移固定，便于断言）
        writer
            .add_function("callee", &make_test_func(vec![0x90, 0xc3]))
            .expect("add callee");
        writer
            .add_function("caller", &make_call_func("callee"))
            .expect("add caller");

        // 读取 .text 段数据 + 重定位表，验证 rel32 槽被清零
        let text = writer.obj.section(writer.text_section);
        let data = text.data();
        let caller_start = data.len() - 6; // caller 是最后一个函数：6 字节
        // caller: e8 ?? ?? ?? ?? c3 —— rel32 槽（caller_start+1..+5）必须为 0
        assert_eq!(
            &data[caller_start + 1..caller_start + 5],
            &[0, 0, 0, 0],
            "COFF REL32 槽应被清零（隐式 addend = 0），而非编码器占位 -1"
        );

        // 序列化后解析回读，验证 rel32 槽仍为 0 且 reloc 类型为 REL32
        let data = writer.write_to_vec().expect("write to vec");
        use object::Object;
        use object::ObjectSection;
        let parsed = object::read::File::parse(&*data).expect("parse COFF");
        let text = parsed.section_by_name(".text").expect(".text section");
        assert_eq!(
            &text.data().unwrap()[caller_start + 1..caller_start + 5],
            &[0, 0, 0, 0],
            "序列化后 rel32 槽仍为 0（补偿 -4 未被 object crate 覆盖）"
        );
        let rel32_count = text
            .relocations()
            .filter(|(_off, r)| {
                matches!(
                    r.flags(),
                    RelocationFlags::Coff { typ } if typ == object::pe::IMAGE_REL_AMD64_REL32
                )
            })
            .count();
        assert_eq!(rel32_count, 1, "应恰好一条 REL32 重定位");
    }

    #[test]
    fn test_coff_rel32_addend_compensation_passes_neg4() {
        // 补偿验证：COFF REL32 必须传 addend-4 给 object crate（其 coff_adjust
        // +4 抵消后净 0，避免 write_relocation_addend 覆盖已清零的槽）。
        let config = crate::target::TargetConfig::from_triple("x86_64-pc-windows-msvc")
            .expect("valid triple");
        let mut writer = ObjectWriter::new(&config).expect("create writer");
        writer
            .add_function("callee", &make_test_func(vec![0x90, 0xc3]))
            .expect("add callee");
        writer
            .add_function("caller", &make_call_func("callee"))
            .expect("add caller");
        let data = writer.write_to_vec().expect("write to vec");
        // 序列化后解析回来，rel32 槽仍应为 0（addend 净 0 → 不覆盖）
        use object::Object;
        use object::ObjectSection;
        let parsed = object::read::File::parse(&*data).expect("parse COFF");
        let text = parsed
            .section_by_name(".text")
            .expect(".text section")
            .data()
            .unwrap_or(&[]);
        let caller_start = text.len() - 6;
        assert_eq!(
            &text[caller_start + 1..caller_start + 5],
            &[0, 0, 0, 0],
            "序列化后 rel32 槽仍为 0（补偿 -4 未被 object crate 覆盖）"
        );
    }

    #[test]
    fn test_map_reloc_flags_macho() {
        // Mach-O: x86_64 specific flags
        let flags = map_reloc_flags(&RelocKind::ABS8, BinaryFormat::MachO, Architecture::X86_64);
        assert!(matches!(flags, RelocationFlags::MachO { .. }));
    }
}
