//! Data layout — target-specific type sizing and alignment.
//!
//! Replaces the hardcoded 8-byte pointer size and 64-bit assumptions
//! with a configurable data layout model inspired by LLVM's DataLayout.
//!
//! # LLVM DataLayout string format (subset supported)
//!
//! ```text
//! e-m:e-p:64:64-i64:64-f80:128-n8:16:32:64-S128
//! ```
//!
//! - `e` / `E`: little / big endian
//! - `m:<mangling>`: name mangling (e=ELF, w=Windows COFF, o=Mach-O)
//! - `p:<size>:<abi>:<pref>`: pointer size/alignment for addr space
//! - `i<size>:<abi>:<pref>`: integer alignment by bit width
//! - `f<size>:<abi>:<pref>`: float alignment by bit width
//! - `v<size>:<abi>:<pref>`: vector alignment by bit width
//! - `a:<abi>:<pref>`: aggregate alignment
//! - `n<size>:<size>:...`: native integer widths
//! - `S<size>`: stack alignment
use crate::error::IrError;

use super::entity::Endianness;
use super::imm_str::ImmStr;
use std::collections::HashMap;

// ============================================================
// DataLayout
// ============================================================

/// Target data layout — controls type sizes, alignments, and pointer width.
///
/// Stored in `Module` (shared across functions) and referenced by `TypeStore`
/// for size/alignment queries.
#[derive(Clone, Debug)]
pub struct DataLayout {
    /// Target endianness.
    pub endianness: Endianness,

    /// Name mangling scheme.
    pub mangling: Mangling,

    /// Pointer size and alignment per address space.
    /// Key: address space (0 = default/generic).
    /// Value: (size in bytes, ABI alignment in bytes).
    pub pointer_layout: HashMap<u32, (u32, u32)>,

    /// Integer alignment overrides by bit width.
    pub integer_alignments: HashMap<u32, u32>,

    /// Float alignment overrides by bit width.
    pub float_alignments: HashMap<u16, u32>,

    /// Vector alignment overrides by total bit width.
    pub vector_alignments: HashMap<u32, u32>,

    /// Aggregate (struct/array) alignment.
    pub aggregate_align: u32,

    /// Maximum alignment for any type.
    pub max_alignment: u32,

    /// Native integer widths (in bits), sorted.
    pub native_integer_widths: Vec<u32>,

    /// Native vector widths (in bits), e.g. [128, 256, 512] for x86-64.
    pub native_vector_widths: Vec<u32>,

    /// Stack alignment in bytes.
    pub stack_align: u32,
}

/// LLVM DataLayout 字符串格式（与 [`DataLayout::parse`] 对称）。
///
/// 仅输出已配置字段；`m:`/`e` 恒输出（parse 的起始状态即 x86_64_linux 默认）。
impl std::fmt::Display for DataLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        match self.endianness {
            Endianness::Little => parts.push("e".to_string()),
            Endianness::Big => parts.push("E".to_string()),
        }
        parts.push(
            match self.mangling {
                Mangling::Elf => "m:e",
                Mangling::MachO => "m:o",
                Mangling::WindowsCoff => "m:w",
            }
            .to_string(),
        );
        // pointer layout（按地址空间排序保证确定性）
        let mut ptrs: Vec<_> = self.pointer_layout.iter().collect();
        ptrs.sort_by_key(|(as_, _)| **as_);
        for (as_, &(size_bytes, abi_bytes)) in ptrs {
            parts.push(if *as_ == 0 {
                format!("p:{}:{}", size_bytes * 8, abi_bytes * 8)
            } else {
                format!("p{}:{}:{}", as_, size_bytes * 8, abi_bytes * 8)
            });
        }
        // 整数/浮点/向量对齐：`i{bits}:{abi_bits}`（abi 字节 → bit）
        let mut ints: Vec<_> = self.integer_alignments.iter().collect();
        ints.sort_by_key(|(b, _)| **b);
        for (bits, align) in ints {
            parts.push(format!("i{}:{}", bits, align * 8));
        }
        let mut floats: Vec<_> = self.float_alignments.iter().collect();
        floats.sort_by_key(|(b, _)| **b);
        for (bits, align) in floats {
            parts.push(format!("f{}:{}", bits, align * 8));
        }
        let mut vecs: Vec<_> = self.vector_alignments.iter().collect();
        vecs.sort_by_key(|(b, _)| **b);
        for (bits, align) in vecs {
            parts.push(format!("v{}:{}", bits, align * 8));
        }
        if self.aggregate_align != 0 {
            parts.push(format!("a:{}", self.aggregate_align * 8));
        }
        if !self.native_integer_widths.is_empty() {
            let mut w = self.native_integer_widths.clone();
            w.sort();
            parts.push(format!(
                "n:{}",
                w.iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(":")
            ));
        }
        if self.stack_align != 0 {
            parts.push(format!("S:{}", self.stack_align));
        }
        write!(f, "{}", parts.join("-"))
    }
}

impl DataLayout {
    /// 是否全默认（未配置任何字段——display 省略 datalayout 行）。
    pub fn is_default(&self) -> bool {
        self.endianness == Endianness::Little
            && self.mangling == Mangling::Elf
            && self.pointer_layout.is_empty()
            && self.integer_alignments.is_empty()
            && self.float_alignments.is_empty()
            && self.vector_alignments.is_empty()
            && self.aggregate_align == 0
            && self.max_alignment == 0
            && self.stack_align == 0
    }

    /// Create the default 64-bit little-endian data layout (x86-64 Linux).
    pub fn x86_64_linux() -> Self {
        Self {
            endianness: Endianness::Little,
            mangling: Mangling::Elf,
            pointer_layout: {
                let mut m = HashMap::new();
                m.insert(0, (8, 8)); // addr space 0: 8-byte ptr
                m
            },
            integer_alignments: {
                let mut m = HashMap::new();
                m.insert(1, 1); // i1 → 1
                m.insert(8, 1);
                m.insert(16, 2);
                m.insert(32, 4);
                m.insert(64, 8);
                m.insert(128, 16);
                m
            },
            float_alignments: {
                let mut m = HashMap::new();
                m.insert(16, 2); // f16 → 2
                m.insert(32, 4); // f32 → 4
                m.insert(64, 8); // f64 → 8
                m.insert(128, 16); // f128 → 16
                m
            },
            vector_alignments: {
                let mut m = HashMap::new();
                m.insert(64, 8);
                m.insert(128, 16);
                m.insert(256, 32);
                m.insert(512, 64);
                m
            },
            aggregate_align: 0, // 0 = use max field alignment
            max_alignment: 16,
            native_integer_widths: vec![8, 16, 32, 64],
            native_vector_widths: vec![128, 256, 512],
            stack_align: 16,
        }
    }

    /// Create the default 64-bit little-endian data layout (x86-64 Windows).
    pub fn x86_32_linux() -> Self {
        let mut dl = Self::x86_64_linux();
        dl.pointer_layout.insert(0, (4, 4)); // 4-byte ptr
        dl.max_alignment = 8;
        dl.native_vector_widths = vec![64, 128, 256];
        dl.stack_align = 8;
        // Update vector alignments for 32-bit
        dl.vector_alignments.clear();
        dl.vector_alignments.insert(64, 8);
        dl.vector_alignments.insert(128, 16);
        dl.vector_alignments.insert(256, 16);
        dl
    }

    /// Create a 64-bit little-endian data layout (AArch64).
    ///
    /// Supported specifiers: `e`/`E`, `m:`, `p:`, `i:`, `f:`, `v:`, `a:`,
    /// `n`, `S`. The `-` prefix separator and `:pref` alignment parts are
    /// accepted but the preferred alignment is currently ignored.
    ///
    /// Returns `Err(msg)` for unparseable or unsupported specifiers.
    pub fn parse(s: &str) -> Result<Self, IrError> {
        let mut dl = Self::x86_64_linux();
        dl.pointer_layout.clear();
        dl.integer_alignments.clear();
        dl.float_alignments.clear();
        dl.vector_alignments.clear();
        dl.native_integer_widths.clear();

        for part in s.split('-') {
            if part.is_empty() {
                continue;
            }
            let mut chars = part.chars().peekable();
            let spec = chars.next().unwrap_or('\0');
            // Consume optional colon separator
            let rest: String = if chars.peek() == Some(&':') {
                chars.next();
                chars.collect()
            } else {
                chars.collect()
            };

            match spec {
                'e' => dl.endianness = Endianness::Little,
                'E' => dl.endianness = Endianness::Big,
                'm' => {
                    dl.mangling = match rest.as_str() {
                        "e" => Mangling::Elf,
                        "o" => Mangling::MachO,
                        "w" | "x" => Mangling::WindowsCoff,
                        _ => return Err(IrError::Parse(format!("unknown mangling: '{}'", rest))),
                    };
                }
                'p' => {
                    // Format: p[<AS>]:<size_in_bits>:<abi>[:<pref>]
                    // The part before the spec character is already consumed.
                    // We have the raw segment, e.g. "p:64:64" or "p1:32:32"
                    let seg = part; // the full segment like "p:64:64"
                    let after_p = &seg[1..]; // strip 'p', get ":64:64" or "1:32:32"
                    let (as_num, size_abi_str) = if let Some(str) = after_p.strip_prefix(':') {
                        // p:size:abi — no addr space
                        (0u32, str) // strip leading ':'
                    } else if let Some(colon_pos) = after_p.find(':') {
                        // p1:32:32 — AS number then size:abi
                        let as_str = &after_p[..colon_pos];
                        let as_num: u32 = as_str.parse().unwrap_or(0);
                        (as_num, &after_p[colon_pos + 1..])
                    } else {
                        return Err(IrError::Parse(format!("invalid p spec: '{}'", seg)));
                    };
                    let (size_bits, abi) = parse_size_align(size_abi_str)
                        .ok_or(format!("invalid p spec: '{}'", seg))?;
                    // Convert from bits to bytes
                    let size_bytes = size_bits / 8;
                    let abi_bytes = if abi >= 8 { abi / 8 } else { size_bytes };
                    dl.pointer_layout.insert(as_num, (size_bytes, abi_bytes));
                }
                'i' => {
                    let (bits, abi) =
                        parse_size_align(&rest).ok_or(format!("invalid i spec: '{}'", part))?;
                    // i spec: <size> is bit width (key), <abi> is in bits → convert to bytes
                    dl.integer_alignments.insert(bits, bits_to_bytes(abi));
                }
                'f' => {
                    let (bits, abi) =
                        parse_size_align(&rest).ok_or(format!("invalid f spec: '{}'", part))?;
                    dl.float_alignments.insert(bits as u16, bits_to_bytes(abi));
                }
                'v' => {
                    let (bits, abi) =
                        parse_size_align(&rest).ok_or(format!("invalid v spec: '{}'", part))?;
                    // v spec: <size> is total bit width, <abi> is in bits → convert
                    dl.vector_alignments.insert(bits, bits_to_bytes(abi));
                }
                'a' => {
                    // a spec: `<abi>[:<pref>]`——pref 可省略(默认 = abi)
                    //（第二十九轮:display 输出单值 `a:<abi>`,parser 原要求
                    // 双值;LLVM 语法单值合法,roundtrip 不等）
                    let (abi, _pref) = if let Some(v) = parse_size_align(&rest) {
                        v
                    } else if let Ok(abi) = rest.parse::<u32>() {
                        (abi, abi)
                    } else {
                        return Err(IrError::Parse(format!("invalid a spec: '{part}'")));
                    };
                    // a spec: <abi> is in bits → convert to bytes
                    dl.aggregate_align = bits_to_bytes(abi);
                }
                'n' => {
                    for w in rest.split(':') {
                        if let Ok(w) = w.parse::<u32>() {
                            dl.native_integer_widths.push(w);
                        }
                    }
                }
                'S' => {
                    dl.stack_align = rest.parse::<u32>().unwrap_or(16);
                }
                // A<AS>: allocator address space（忽略存储，仅接受语法；值域 < 2^24）
                'A' => {
                    let as_: u32 = rest
                        .parse()
                        .map_err(|_| IrError::Parse(format!("invalid A spec: '{}'", part)))?;
                    if as_ >= (1 << 24) {
                        return Err(IrError::Parse(format!(
                            "A spec address space {as_} exceeds maximum 2^24-1"
                        )));
                    }
                }
                // P<AS>: program address space（忽略存储，仅接受语法）
                'P' => {
                    let as_: u32 = rest
                        .parse()
                        .map_err(|_| IrError::Parse(format!("invalid P spec: '{}'", part)))?;
                    if as_ >= (1 << 24) {
                        return Err(IrError::Parse(format!(
                            "P spec address space {as_} exceeds maximum 2^24-1"
                        )));
                    }
                }
                // 未知 specifier（'F' 等 LLVM 扩展——宽松忽略；第十二轮）
                _ => {}
            }
        }

        // Ensure at least one address space (default: 0)
        if dl.pointer_layout.is_empty() {
            dl.pointer_layout.insert(0, (8, 8));
        }

        // Set max alignment
        dl.max_alignment = dl.compute_max_alignment();
        dl.native_vector_widths = dl.compute_native_vector_widths();

        Ok(dl)
    }

    // ============================================================
    // Queries
    // ============================================================

    /// Get pointer size for a given address space.
    /// Falls back to address space 0 if the requested space is not found.
    pub fn pointer_size(&self, addr_space: u32) -> u32 {
        self.pointer_layout
            .get(&addr_space)
            .or_else(|| self.pointer_layout.get(&0))
            .map(|&(size, _)| size)
            .unwrap_or(8)
    }

    /// Get pointer ABI alignment for a given address space.
    pub fn pointer_align(&self, addr_space: u32) -> u32 {
        self.pointer_layout
            .get(&addr_space)
            .or_else(|| self.pointer_layout.get(&0))
            .map(|&(_, align)| align)
            .unwrap_or(8)
    }

    /// Get the ABI alignment for an integer of the given bit width.
    /// Falls back to natural alignment (bits/8 rounded up to power-of-2).
    pub fn integer_align(&self, bits: u32) -> u32 {
        self.integer_alignments
            .get(&bits)
            .copied()
            .unwrap_or_else(|| natural_align(bits))
    }

    /// Get the ABI alignment for a float of the given bit width.
    pub fn float_align(&self, bits: u16) -> u32 {
        self.float_alignments
            .get(&bits)
            .copied()
            .unwrap_or_else(|| natural_align(bits as u32))
    }

    /// Get the ABI alignment for a vector of the given total bit width.
    pub fn vector_align(&self, total_bits: u32) -> u32 {
        self.vector_alignments
            .get(&total_bits)
            .copied()
            .unwrap_or_else(|| natural_align(total_bits))
    }

    // ============================================================
    // Internal helpers
    // ============================================================

    fn compute_max_alignment(&self) -> u32 {
        let mut max = self.stack_align;
        for &(_, align) in self.pointer_layout.values() {
            max = max.max(align);
        }
        for &align in self.integer_alignments.values() {
            max = max.max(align);
        }
        for &align in self.float_alignments.values() {
            max = max.max(align);
        }
        for &align in self.vector_alignments.values() {
            max = max.max(align);
        }
        max
    }

    fn compute_native_vector_widths(&self) -> Vec<u32> {
        if self.vector_alignments.is_empty() {
            return vec![128, 256];
        }
        let mut widths: Vec<u32> = self.vector_alignments.keys().copied().collect();
        widths.sort();
        widths
    }
}

impl Default for DataLayout {
    /// Default to x86-64 Linux (most common dev target).
    fn default() -> Self {
        Self::x86_64_linux()
    }
}

// ============================================================
// Mangling
// ============================================================

/// Name mangling scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mangling {
    /// ELF name mangling (used on Linux, BSD, etc.)
    #[default]
    Elf,
    /// Windows COFF name mangling (MSVC style)
    WindowsCoff,
    /// Mach-O name mangling (macOS/iOS)
    MachO,
}

// ============================================================
// TargetTriple
// ============================================================

/// Target triple — identifies the target architecture, vendor, OS, and environment.
///
/// Format: `<arch>-<vendor>-<os>[-<environment>]`
///
/// Examples:
/// - `x86_64-unknown-linux-gnu`
/// - `x86_64-pc-windows-msvc`
/// - `aarch64-apple-darwin`
/// - `riscv64-unknown-elf`
/// - `wasm32-unknown-unknown`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetTriple {
    /// Architecture: x86_64, aarch64, riscv64, riscv32, wasm32, etc.
    pub arch: ImmStr,
    /// Vendor: unknown, pc, apple, ibm, etc.
    pub vendor: ImmStr,
    /// Operating system: linux, windows, macos, none, etc.
    pub os: ImmStr,
    /// Environment: gnu, msvc, musl, elf, etc.
    pub environment: ImmStr,
}

impl TargetTriple {
    /// Parse from a string like `"x86_64-pc-windows-msvc"`.
    pub fn parse(s: &str) -> Self {
        let parts: Vec<&str> = s.split('-').collect();
        Self {
            arch: ImmStr::from(parts.first().copied().unwrap_or("unknown")),
            vendor: ImmStr::from(parts.get(1).copied().unwrap_or("unknown")),
            os: ImmStr::from(parts.get(2).copied().unwrap_or("unknown")),
            environment: ImmStr::from(parts.get(3).copied().unwrap_or("")),
        }
    }

    /// Check if this target uses 32-bit pointers.
    pub fn is_32bit(&self) -> bool {
        matches!(
            self.arch.as_str(),
            "i386"
                | "i486"
                | "i586"
                | "i686"
                | "arm"
                | "armv7"
                | "thumbv7"
                | "mips"
                | "mipsel"
                | "wasm32"
        )
    }

    /// Check if this target uses 64-bit pointers.
    pub fn is_64bit(&self) -> bool {
        matches!(
            self.arch.as_str(),
            "x86_64"
                | "aarch64"
                | "arm64"
                | "riscv64"
                | "powerpc64"
                | "powerpc64le"
                | "mips64"
                | "sparc64"
                | "s390x"
        )
    }

    /// Get the default OS name.
    pub fn os_name(&self) -> &str {
        match self.os.as_str() {
            "linux" => "linux",
            "windows" | "win32" => "windows",
            "macos" | "darwin" => "macos",
            "none" | "unknown" => "bare",
            _ => "unknown",
        }
    }
}

impl std::fmt::Display for TargetTriple {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}-{}", self.arch, self.vendor, self.os)?;
        if !self.environment.is_empty() {
            write!(f, "-{}", self.environment)?;
        }
        Ok(())
    }
}

// ============================================================
// Helpers
// ============================================================

/// Convert from bits to bytes, rounding up.
fn bits_to_bytes(bits: u32) -> u32 {
    if bits == 0 { 1 } else { bits.div_ceil(8) }
}

/// Parse "size:abi" or "size:abi:pref" from a data layout spec part.
fn parse_size_align(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.split(':');
    let size_str = parts.next()?;
    let abi_str = parts.next()?;
    let size: u32 = size_str.parse().ok()?;
    let abi: u32 = if abi_str.is_empty() {
        natural_align(size)
    } else {
        abi_str.parse().ok()?
    };
    Some((size, abi))
}

/// Natural alignment for a given size: rounded up to next power of 2.
fn natural_align(size: u32) -> u32 {
    if size == 0 {
        return 1;
    }
    let mut align = 1;
    while align < size {
        align <<= 1;
    }
    align
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_default() {
        // default() = x86_64_linux（pointer_layout 等已配置）→ 非"全默认"
        assert!(!DataLayout::default().is_default());
        // 空构造（无任何配置）→ 全默认
        let empty = DataLayout {
            endianness: Endianness::Little,
            mangling: Mangling::Elf,
            pointer_layout: HashMap::new(),
            integer_alignments: HashMap::new(),
            float_alignments: HashMap::new(),
            vector_alignments: HashMap::new(),
            aggregate_align: 0,
            max_alignment: 0,
            stack_align: 0,
            native_integer_widths: vec![],
            native_vector_widths: vec![],
        };
        assert!(empty.is_default(), "empty layout should be default");
    }

    #[test]
    fn test_default_layout() {
        let dl = DataLayout::default();
        assert_eq!(dl.endianness, Endianness::Little);
        assert_eq!(dl.pointer_size(0), 8);
        assert_eq!(dl.pointer_align(0), 8);
        assert_eq!(dl.integer_align(32), 4);
        assert_eq!(dl.integer_align(64), 8);
    }

    #[test]
    fn test_x86_32() {
        let dl = DataLayout::x86_32_linux();
        assert_eq!(dl.pointer_size(0), 4);
        assert_eq!(dl.pointer_align(0), 4);
        assert_eq!(dl.stack_align, 8);
    }

    #[test]
    fn test_parse_basic() {
        let dl = DataLayout::parse("e-m:e-p:64:64-i64:64-f80:128-n8:16:32:64-S128").unwrap();
        assert_eq!(dl.endianness, Endianness::Little);
        // p:64:64 → 64-bit ptr, stored as 8 bytes
        assert_eq!(dl.pointer_size(0), 8);
        assert_eq!(dl.pointer_align(0), 8);
        // i64:64 → 64-bit int alignment = 64 bits / 8 = 8 bytes
        assert_eq!(dl.integer_align(64), 8);
        // f80:128 → 80-bit float alignment = 128 bits / 8 = 16 bytes
        assert_eq!(dl.float_align(80), 16);
        assert_eq!(dl.native_integer_widths, vec![8, 16, 32, 64]);
        assert_eq!(dl.stack_align, 128);
    }

    #[test]
    fn test_parse_arm64() {
        let dl = DataLayout::parse("e-m:e-i8:8:32-i16:16:32-i64:64-i128:128-n32:64-S128").unwrap();
        assert_eq!(dl.endianness, Endianness::Little);
        assert_eq!(dl.stack_align, 128);
        // pointer defaults: 8 bytes
        assert_eq!(dl.pointer_size(0), 8);
    }

    #[test]
    fn test_parse_big_endian() {
        let dl = DataLayout::parse("E-m:e-p:64:64").unwrap();
        assert_eq!(dl.endianness, Endianness::Big);
    }

    #[test]
    fn test_different_addr_spaces() {
        let mut dl = DataLayout::default();
        // Add custom addr space 1 (e.g., GPU global memory with 32-bit ptrs)
        dl.pointer_layout.insert(1, (4, 4));
        dl.pointer_layout.insert(3, (8, 8)); // addr space 3 (shared)

        assert_eq!(dl.pointer_size(0), 8); // default
        assert_eq!(dl.pointer_size(1), 4); // GPU global
        assert_eq!(dl.pointer_size(3), 8); // shared
        assert_eq!(dl.pointer_size(99), 8); // fallback to addr 0
    }

    #[test]
    fn test_target_triple() {
        let t = TargetTriple::parse("x86_64-pc-windows-msvc");
        assert_eq!(t.arch.as_str(), "x86_64");
        assert!(t.is_64bit());

        let t = TargetTriple::parse("wasm32-unknown-unknown");
        assert!(t.is_32bit());

        let t = TargetTriple::parse("x86_64-unknown-linux-gnu");
        assert!(t.is_64bit());
        assert_eq!(t.os_name(), "linux");
    }
}
