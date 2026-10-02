//! x86 runner：绑定 `x86::TargetMachine` + 能力集，运行架构无关
//! JIT 集成矩阵。能力集 = `x86::SUPPORTED_OPS`（P1-16：由 TOML
//! `[[lowering]].op` 唯一集生成，不再手写同步）∪ 本文件补充的
//! 非 lowering 路径 op；未实现 op 的用例自动 Skip（后续阶段补齐后转绿，
//! Skip 数收敛到 0）。

/// 无 `[[lowering]]` 条目的 op（P1-16 补充声明；与生成的 SUPPORTED_OPS 并集
/// 构成完整能力集）：
/// - Call/CallIndirect：ABI 专用生成路径（gen_call_lowering，非 [[lowering]]）；
/// - GetElementPtr：地址计算在 lowering 内联（gep 展开为 add），无独立规则。
/// - VaStart/VaArg（v20 变参 V2–V4）：同样是**生成器专用臂**（`va_list` 的形态/落点是约定
///   数据，静态模板表达不了），谱里没有也不该有 `[[lowering]]`。声明了它们，变参用例才会
///   真跑而不是被能力门控 Skip。
pub const CAPS_EXTRA: &[&str] = &["Call", "CallIndirect", "GetElementPtr"];

/// 运行全部矩阵用例；断言无 Fail（Skip 仅报告）。
/// P1-15 + 2026-09 修正：x86 矩阵依赖本机 ExecutableMemory 执行 x86 机器码，
/// 且机器码按 **Windows x64 ABI**（RCX 首参）生成——仅 Windows x86_64 宿主
/// 正确（macOS arm64 SIGILL；Linux SysV 首参在 RDI → 参数寄存器错位结果错/
/// SEGV）。SysV 支持是 x86 后端的独立工程，未做；本矩阵由 test-windows 覆盖，
/// riscv 矩阵（QEMU）在各宿主覆盖执行语义。
#[cfg(all(target_arch = "x86_64", windows))]
#[test]
fn jit_matrix_x86() {
    use crate::jit_matrix::{Capabilities, Outcome, Runner};
    // P1-16：能力集 = TOML 生成的 SUPPORTED_OPS ∪ CAPS_EXTRA（非 lowering 路径）
    let mut caps = Capabilities::new(code_forge::backend::x86::SUPPORTED_OPS);
    for extra in CAPS_EXTRA {
        caps.ops.insert(extra);
    }
    // AVX 可用 → 标记 "AVX" 伪能力（V256 用例门控；无 AVX 机器自动 Skip）
    if code_forge::backend::avx_available() {
        caps.ops.insert("AVX");
    }
    let runner = Runner {
        machine: code_forge::backend::x86::TargetMachine::new,
        caps,
        exec: None, // 本机 ExecutableMemory 快路径
    };
    let results = crate::jit_matrix::run_all(&runner);
    crate::jit_matrix::emit_events("x86", &results);
    let mut pass = 0;
    let mut skip = 0;
    let mut fail: Vec<(String, String)> = Vec::new();
    for (name, outcome) in &results {
        match outcome {
            Outcome::Pass => pass += 1,
            Outcome::Skip(_) => skip += 1,
            Outcome::Fail(e) => fail.push((name.clone(), e.clone())),
        }
    }
    eprintln!(
        "jit_matrix x86: {pass} passed, {skip} skipped, {} failed",
        fail.len()
    );
    for (n, e) in &fail {
        eprintln!("  FAIL {n}: {e}");
    }
    assert!(
        fail.is_empty(),
        "jit_matrix: {} failed:\n{}",
        fail.len(),
        fail.iter()
            .map(|(n, e)| format!("{n}: {e}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// P1-16 一致性：CAPS_EXTRA 与 TOML 生成的 SUPPORTED_OPS 必须不相交
/// （重合 → 补充声明多余应删除；漏声明 → 矩阵用例误 Skip 丢覆盖）。
#[test]
fn caps_extra_disjoint_from_supported_ops() {
    let generated = code_forge::backend::x86::SUPPORTED_OPS;
    for extra in CAPS_EXTRA {
        assert!(
            !generated.contains(extra),
            "CAPS_EXTRA '{extra}' 已存在于 TOML SUPPORTED_OPS——应删除补充声明"
        );
    }
}
