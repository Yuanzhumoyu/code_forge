//! Compilation output types — result of the code generation pipeline.

use crate::machine::cfi::FunctionCfi;
use forge_ir::ImmStr;

/// A compiled function with its machine code and relocation information.
#[derive(Debug, Clone)]
pub struct CompiledFunction {
    pub code: Vec<u8>,
    pub relocations: Vec<Relocation>,
    pub code_size: usize,
    /// 行号表（B1 per-statement line-tables，debuginfo 开启时收集）：
    /// (机器码偏移, 源码行 1-based)。forge-rustc 按 -C debuginfo 决定是否
    /// 生成 .debug_line 的 per-statement 条目。
    pub line_entries: Vec<(u32, u32)>,
    /// DWARF `.debug_frame` CFI 行（M2）：emission 完成后经
    /// `TargetMachine::function_cfi` 按 ISA 名派发扫描（machine::cfi——
    /// x86_64 命中；非 x86 后端/形态不符 → None，安全退化不产 CFI）。
    /// forge-rustc 在 debuginfo 开启时据此生成每函数一条 FDE（gdb 无 SEH
    /// 时靠 .debug_frame 解栈——bt/info args 的根因修复）。
    pub cfi: Option<FunctionCfi>,
}

/// A relocation record within compiled code.
#[derive(Debug, Clone)]
pub struct Relocation {
    pub offset: usize,
    pub kind: RelocKind,
    /// 谱面 `[asm.reloc.<名>]` 的**语义名**（v27 M6-98）。空 = 未命名（x86 的
    /// Call/Global 走对象库的 Generic 映射；arm64 的 `BL` 声明 `call26`，由
    /// **目标约定数据**把它映射成链接器重定位类型 —— 对象层不再有 arch 分支）。
    pub reloc: ImmStr,
    pub symbol: ImmStr,
    pub addend: i64,
}

/// Relocation type — data-driven design, no per-architecture variants needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocKind {
    /// Absolute address relocation. `width_bytes` 由 ISA 声明（**无白名单**：
    /// 任意 ≥ 1 字节；通用写入路径按 `reloc_bytes` 补 0）。
    Absolute(u32),
    /// PC-relative relocation. `width_bytes` 由 ISA 声明（任意 ≥ 1 字节；超出 8
    /// 字节的部分按符号扩展补位）。`adjustment` 是偏移修正（x86: -width_bytes，
    /// ARM: 0）。ISA-specific encoding (e.g. AArch64 BL imm26, RISC-V JAL UJ
    /// layout) is applied by the backend's `RelocPatcher`, not by this enum.
    Relative(u32, i8),
}

impl RelocKind {
    pub const ABS4: Self = RelocKind::Absolute(4);
    pub const ABS8: Self = RelocKind::Absolute(8);
    pub const REL4: Self = RelocKind::Relative(4, -4);
    pub const REL8: Self = RelocKind::Relative(8, -8);
    pub const REL1: Self = RelocKind::Relative(1, -1);
    pub const CALL: Self = RelocKind::Relative(4, -4);
    pub const CALL_IND: Self = RelocKind::Absolute(8);

    /// **重定位补丁值的唯一口径**（v23 M5）：把"目标符号地址 + patch 站点地址"折成写进
    /// 机器码的值。
    ///
    /// - `Relative(_, adj)`：PC 相对 ⇒ `target - site + adj`（x86 `Rel(4, -4)` 即
    ///   `target - site - 4`；`adj` 由 ISA 声明，不在这里写死）。
    /// - `Absolute(_)`：绝对地址 ⇒ `target`。
    ///
    /// 以前这段语义在 JIT 里有一份私有副本（`compute_patch_value`），ObjectWriter 侧则
    /// 以对象文件 `RelocationKind` + addend 的形式再表达一次——同一件事三处口径，改一处
    /// 就容易漂移。现在放在 `RelocKind` 自己的方法上：**谁能拿到 `target`/`site` 谁就调它**。
    pub fn patch_value(self, target_addr: u64, patch_site: u64) -> u64 {
        match self {
            RelocKind::Relative(_w, adj) => {
                (target_addr as i64 - patch_site as i64 + adj as i64) as u64
            }
            RelocKind::Absolute(_) => target_addr,
        }
    }
    /// 补丁宽度（字节）——由 ISA/后端声明，**不设白名单/上限**。
    pub fn width_bytes(self) -> u32 {
        match self {
            RelocKind::Absolute(w) | RelocKind::Relative(w, _) => w,
        }
    }

    /// 补丁值 → 内存字节（LE）：前 `min(width, 8)` 字节取自 `value`，其余按
    /// `Absolute` 补 0 / `Relative` 符号扩展补位——因此任意宽度都不会 panic 或
    /// 静默丢字节（宽度是 ISA 数据，不是 1/4/8 三选一）。
    pub fn encode_value(self, value: u64) -> Vec<u8> {
        match self {
            RelocKind::Absolute(w) => reloc_bytes(value, w, false),
            RelocKind::Relative(w, _) => reloc_bytes(value, w, true),
        }
    }
}

/// 把补丁值写成 `width` 字节（LE；`width` 任意 ≥ 1）。
/// `signed` = true 时超出 8 字节的部分按符号位补 0xFF/0x00（PC 相对偏移），
/// false 时补 0（绝对地址）。
pub fn reloc_bytes(value: u64, width: u32, signed: bool) -> Vec<u8> {
    let n = width as usize;
    let raw = value.to_le_bytes();
    let mut out = Vec::with_capacity(n);
    out.extend_from_slice(&raw[..n.min(8)]);
    let fill = if signed && (value as i64) < 0 {
        0xFF
    } else {
        0x00
    };
    out.resize(n, fill);
    out
}
