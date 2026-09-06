//! 主机 CPU 特性检测。
//!
//! 检测主机 CPU 支持的指令集扩展（SSE、AVX、BMI 等），
//! 供 JIT 编译器选择最优指令编码。
//!
//! # Example
//! ```ignore
//! use codegen_lib::host_cpu::CpuFeatures;
//!
//! let features = CpuFeatures::detect();
//! if features.has_avx2() {
//!     // 使用 AVX2 指令编码
//! }
//! ```

/// CPU 特性位集。
#[derive(Clone, Debug, Default)]
pub struct CpuFeatures {
    /// x86: SSE2 支持
    pub has_sse2: bool,
    /// x86: SSE4.1 支持
    pub has_sse41: bool,
    /// x86: AVX 支持
    pub has_avx: bool,
    /// x86: AVX2 支持
    pub has_avx2: bool,
    /// x86: BMI2 支持（bzhi, mulx, pdep, pext）
    pub has_bmi2: bool,
    /// x86: POPCNT 支持
    pub has_popcnt: bool,
    /// ARM: NEON 支持
    pub has_neon: bool,
}

impl CpuFeatures {
    /// 检测主机 CPU 特性。
    pub fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            detect_x86_features()
        }
        #[cfg(target_arch = "aarch64")]
        {
            detect_arm_features()
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            CpuFeatures::default()
        }
    }

    /// 返回主机架构名称。
    pub fn host_arch_name() -> &'static str {
        if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else if cfg!(target_arch = "arm") {
            "arm"
        } else if cfg!(target_arch = "riscv64") {
            "riscv64"
        } else {
            "unknown"
        }
    }
}

/// 使用 CPUID 指令运行时检测 x86_64 特性。
#[cfg(target_arch = "x86_64")]
fn detect_x86_features() -> CpuFeatures {
    // CPUID leaf 1: ECX features
    let (_, _, ecx1, edx1) = cpuid(1, 0);
    // CPUID leaf 7 subleaf 0: EBX features
    let (_, ebx7, _, _) = cpuid(7, 0);

    // 检查 OSXSAVE (ECX[27]) 和 XCR0 中的 XMM/YMM state 是否启用
    let has_osxsave = (ecx1 >> 27) & 1 != 0;
    let xcr0_sse_avx = if has_osxsave {
        let xcr0 = xgetbv(0);
        (xcr0 & 0b110) == 0b110 // XMM[1] + YMM[2] state enabled
    } else {
        false
    };

    CpuFeatures {
        // CPUID.01h:EDX[26] — SSE2
        has_sse2: (edx1 >> 26) & 1 != 0,
        // CPUID.01h:ECX[19] — SSE4.1
        has_sse41: (ecx1 >> 19) & 1 != 0,
        // CPUID.01h:ECX[28] — AVX (需要 OSXSAVE + XCR0 支持)
        has_avx: ((ecx1 >> 28) & 1 != 0) && xcr0_sse_avx,
        // CPUID.07h:EBX[5] — AVX2
        has_avx2: ((ebx7 >> 5) & 1 != 0) && xcr0_sse_avx,
        // CPUID.07h:EBX[8] — BMI2
        has_bmi2: (ebx7 >> 8) & 1 != 0,
        // CPUID.01h:ECX[23] — POPCNT
        has_popcnt: (ecx1 >> 23) & 1 != 0,
        has_neon: false,
    }
}

/// 执行 CPUID 指令。
#[cfg(target_arch = "x86_64")]
fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let mut eax = leaf;
        let mut ebx: u32;
        let mut ecx = subleaf;
        let mut edx: u32;
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx:e}, ebx",
            "pop rbx",
            ebx = out(reg) ebx,
            inlateout("eax") eax => eax,
            inlateout("ecx") ecx => ecx,
            lateout("edx") edx,
            options(preserves_flags)
        );
        (eax, ebx, ecx, edx)
    }
}

/// 读取 XCR0 寄存器（用于检测 AVX 支持）。
#[cfg(target_arch = "x86_64")]
fn xgetbv(ecx: u32) -> u64 {
    let (eax, edx): (u32, u32);
    unsafe {
        core::arch::asm!(
            "xgetbv",
            inout("ecx") ecx => _,
            out("eax") eax,
            out("edx") edx,
            options(nostack, preserves_flags)
        );
    }
    (eax as u64) | ((edx as u64) << 32)
}

/// ARM 特性检测。
#[cfg(target_arch = "aarch64")]
fn detect_arm_features() -> CpuFeatures {
    CpuFeatures {
        // 大多数 AArch64 处理器都有 NEON
        has_neon: true,
        ..Default::default()
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_does_not_crash() {
        let features = CpuFeatures::detect();
        let _ = features.has_sse2;
    }

    #[test]
    fn test_host_arch_name() {
        let name = CpuFeatures::host_arch_name();
        assert!(!name.is_empty());
        // 架构自适应（非 x86 宿主不硬断言 x86_64——CI macOS 是 arm64）
        let expected = if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else if cfg!(target_arch = "arm") {
            "arm"
        } else if cfg!(target_arch = "riscv64") {
            "riscv64"
        } else {
            "unknown"
        };
        assert_eq!(name, expected);
    }

    #[test]
    fn test_default_features() {
        let features = CpuFeatures::default();
        assert!(!features.has_sse2);
        assert!(!features.has_avx2);
    }
}
