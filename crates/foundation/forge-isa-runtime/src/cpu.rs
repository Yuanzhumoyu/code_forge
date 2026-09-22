//! CPU 能力探测（AVX / AVX2 / AVX-512F）——生成物与测试共用。

/// AVX 可用性（OnceLock 缓存 cpuid 检测，供 VEX 编码宏断言）。
/// 非 x86_64 或检测失败 → false（V256 指令编码时 panic 提示）。
pub fn avx_available() -> bool {
    static AVX: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVX.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            std::arch::is_x86_feature_detected!("avx")
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    })
}

/// AVX2 可用性（整数 256 位 SIMD 的前提：VPADDD/VPADDQ ymm 等）。
/// 当前 x86 后端整数向量仅支持 ≤128 位（SSE2 PADDD/PADDQ）；整数 V256 落地时
/// 需在对应编码宏断言此函数（与 avx_available 同模式）。
pub fn avx2_available() -> bool {
    static AVX2: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVX2.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            std::arch::is_x86_feature_detected!("avx2")
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    })
}

/// AVX-512F 可用性（64 字节 ZMM 向量、EVEX 编码的前提——V512 by-ref/sret
/// 栈拷贝与收参）。非 x86_64 或检测失败 → false。
///
/// `FORGE_ASSUME_AVX512=1` 覆盖：**仅供测试**在无 AVX-512 的宿主上验证
/// EVEX 编码/收参分派（如 V512 by-ref 的 64B load 选择）——它只放开可行性
/// 守卫，**不会**让 EVEX 指令在该 CPU 上可执行（真执行需硬件，见
/// [`avx512_hardware_available`]）。
///
/// 会读写该环境变量的测试必须持有 [`AVX512_ENV_LOCK`]（进程内串行化，
/// 否则并行测试互相覆盖 env → 时过时败）；**要执行 EVEX 的测试必须用
/// [`avx512_hardware_available`] 判skip**（否则会被别的测试留下的 env
/// 带进非法指令，2026-09-12 实测 0xC000001D）。
pub static AVX512_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// **真实硬件** AVX-512F 能力——**不读** `FORGE_ASSUME_AVX512`。
/// 任何会**执行** EVEX 指令的测试/代码路径必须用本函数（env 只放开生成期的
/// 可行性门，不能让本机真的跑得动 EVEX）。
pub fn avx512_hardware_available() -> bool {
    static AVX512: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVX512.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            std::arch::is_x86_feature_detected!("avx512f")
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    })
}

pub fn avx512_available() -> bool {
    // 覆盖开关每次直读（不缓存）——测试可在进程内任意时刻打开它；
    // 真实检测结果才走 OnceLock 缓存。
    if std::env::var_os("FORGE_ASSUME_AVX512").is_some() {
        return true;
    }
    avx512_hardware_available()
}
