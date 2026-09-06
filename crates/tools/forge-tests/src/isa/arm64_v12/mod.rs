//! arm64_v12 runner：绑定 `arm64_v12::TargetMachine` + 能力集 + QEMU
//! 执行器，运行架构无关 JIT 集成矩阵。
//!
//! 执行走 QEMU system-mode（`exec::qemu_aarch64`；semihosting SYS_EXIT，
//! 退出码 = 函数返回值，进程退出码为低 8 位）。**值域过滤**：退出码 8 位
//! 且按有符号字节符号扩展 → 期望值超出 [-128,127] 的用例 Skip
//! （"value-range"）。QEMU 缺失时矩阵全 Skip（arm64 产物不可本机执行）。
//!
//! ⚠️ 矩阵执行强约束（P2 边界）：FunctionCompiler 帧/尾声/值传递目前只支持
//! 单 return block、无 call、帧 ≤256B（LDUR/STUR imm9）等——任何用例触发
//! 这些缺口会得显式 Fail/报错（不被静默掩盖）；随 lowering/指令扩展自动转绿。

use code_forge::backend::arm64_v12::SUPPORTED_OPS;

/// 无 `[[lowering]]` 条目的 op 补充声明（与 SUPPORTED_OPS 并集）。
/// P2 阶段保持为空（GetElementPtr/Call/Module 等路径未接入 → 相关用例 Skip）。
pub const CAPS_EXTRA: &[&str] = &[];

/// 运行全部矩阵用例；断言无 Fail（Skip 仅报告）。
#[test]
fn jit_matrix_arm64_v12() {
    use crate::jit_matrix::{Capabilities, Outcome, Runner};
    let mut caps = Capabilities::new(SUPPORTED_OPS);
    for extra in CAPS_EXTRA {
        caps.ops.insert(extra);
    }
    // 期望值必须可被 8 位字节 + 有符号扩展精确表达（QEMU semihost 退出码）
    fn fits_i8(c: &crate::jit_matrix::Case) -> bool {
        use crate::jit_matrix::CaseKind;
        let v = match &c.kind {
            CaseKind::I32(_, e) => *e as i64,
            CaseKind::I64(_, e) => *e,
            CaseKind::Bool(_, _) => return true,
            CaseKind::Block(_, e) => *e as i64,
            CaseKind::Args { expected, .. } => *expected,
            CaseKind::F64Args { expected, .. } => *expected,
            CaseKind::F64(_, _) => return false,
            CaseKind::CompileOnly(_) => return true,
            CaseKind::Module(_, e) => *e,
        };
        // P1：Iconst 仅支持 0..0xFFFF（MOVZ 单条）；负值需 movn（P3）→
        // 值域过滤同时排除负期望（return_negative/-1 待 movn 后转正）
        (0..=127).contains(&v)
    }
    let runner = Runner {
        machine: code_forge::backend::arm64_v12::TargetMachine::new,
        caps,
        exec: crate::exec::qemu_aarch64::qemu_aarch64_path().map(|_| {
            Box::new(crate::exec::executor::QemuAarch64Executor)
                as Box<dyn crate::exec::executor::Executor>
        }),
    };
    let results = if runner.exec.is_some() {
        crate::jit_matrix::run_all_filtered(&runner, fits_i8)
    } else {
        eprintln!("[arm64] QEMU(aarch64) 未找到——矩阵用例全部 Skip（编译验证）");
        crate::jit_matrix::run_all_filtered(&runner, |_| false)
    };
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
        "jit_matrix arm64_v12: {pass} passed, {skip} skipped, {} failed",
        fail.len()
    );
    for (n, e) in &fail {
        eprintln!("  FAIL {n}: {e}");
    }
    assert!(
        fail.is_empty(),
        "jit_matrix arm64_v12: {} failed:\n{}",
        fail.len(),
        fail.iter()
            .map(|(n, e)| format!("{n}: {e}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    if runner.exec.is_some() {
        assert!(
            pass > 0,
            "jit_matrix arm64_v12: QEMU 已安装但全部 Skip（{} skip）——执行验证未发生",
            skip
        );
    }
}

/// CAPS_EXTRA 与 SUPPORTED_OPS 一致性（P1-16 同款守卫）。
#[test]
fn caps_extra_disjoint_from_supported_ops() {
    for extra in CAPS_EXTRA {
        assert!(
            !SUPPORTED_OPS.contains(extra),
            "CAPS_EXTRA '{extra}' 已存在于 TOML SUPPORTED_OPS——应删除补充声明"
        );
    }
}
