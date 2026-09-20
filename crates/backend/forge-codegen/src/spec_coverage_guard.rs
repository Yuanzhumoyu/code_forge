//! v18 S6 覆盖率守卫：把三条发行 ISA **生成期自测**的覆盖率与"文本歧义"名单钉死。
//!
//! 生成的自测（`arch::<isa>::<isa>::__spec_tests`）随 TOML 自动更新，因此它本身
//! 不需要维护；但**它是否还在覆盖全部指令**需要外部核对——本文件是那个"外部"：
//!
//! 1. `SPEC_COVERED + SPEC_SKIPPED == SPEC_TOTAL`（生成端自检的镜像）；
//! 2. **零跳过**：`SPEC_SKIPPED` 必须为空（S6 判据 = 全指令覆盖）；
//! 3. **指令总数钉死**：数字变了就说明指令谱动了——要么同步更新这里，要么把
//!    误删/误加的指令查清楚（防止"指令悄悄消失，自测跟着少跑"）；
//! 4. **文本歧义名单钉死**：名单上的指令"同名同形、编码不同"，自测对它们放宽为
//!    "文本幂等 + 自洽"（不要求回到同一字节）。名单增长 = 有指令新变成文本分不清的
//!    状态（可能是真加了歧义，也可能是新指令与老指令撞了 asm 文本）——必须显式确认，
//!    否则"强断言"会静默消失。

#![cfg(test)]

/// 单个 ISA 的自测覆盖报告。
struct Report {
    name: &'static str,
    total: usize,
    covered: usize,
    skipped: Vec<&'static str>,
    ambiguous: Vec<&'static str>,
}

macro_rules! report_of {
    ($name:literal, $path:path) => {{
        use $path as spec;
        Report {
            name: $name,
            total: spec::SPEC_TOTAL,
            covered: spec::SPEC_COVERED,
            skipped: spec::SPEC_SKIPPED.iter().map(|(n, _)| *n).collect(),
            ambiguous: spec::SPEC_TEXT_AMBIGUOUS.to_vec(),
        }
    }};
}

fn reports() -> Vec<Report> {
    vec![
        report_of!("x86_v12", crate::arch::x86_v12::x86_v12::__spec_tests),
        report_of!(
            "riscv64_v12",
            crate::arch::riscv64_v12::riscv64_v12::__spec_tests
        ),
        report_of!("arm64_v12", crate::arch::arm64_v12::arm64_v12::__spec_tests),
    ]
}

/// 生成端自检的镜像 + **零跳过**（S6 的核心判据）。
#[test]
fn generated_spec_tests_cover_every_instruction() {
    for r in reports() {
        assert!(r.total > 0, "{}: 指令总数为 0", r.name);
        assert_eq!(
            r.covered + r.skipped.len(),
            r.total,
            "{}: 覆盖计数与总数不符",
            r.name
        );
        assert!(
            r.skipped.is_empty(),
            "{}: {} 条指令没能自动构造操作数：{:?}",
            r.name,
            r.skipped.len(),
            r.skipped
        );
    }
}

/// 指令总数钉死（2026-09-20 实测；arm64 自 S3c 加 `b.cond` 16 行模板后 = 104）。
#[test]
fn spec_coverage_totals_are_pinned() {
    let totals: Vec<(&str, usize)> = reports().iter().map(|r| (r.name, r.total)).collect();
    assert_eq!(
        totals,
        vec![("x86_v12", 197), ("riscv64_v12", 116), ("arm64_v12", 104)],
        "指令总数变了：确认是谱的预期变更还是指令丢失"
    );
}

/// 文本歧义名单钉死（更新前先跑 `print_spec_coverage_report` 看当前值）。
#[test]
fn spec_text_ambiguity_lists_are_pinned() {
    let x86 = [
        "ADD64_R_IMM32",
        "ADD_R_IMM32",
        "LEA_R64_SIB",
        "LEA_RBP_OFF",
        "MOV64_MR",
        "MOV64_RM",
        "MOV64_RR",
        "MOVABS_GLOBAL",
        "MOVQ_FREG_XMM",
        "MOVQ_XMM_FREG",
        "MOVSD",
        "MOVSD_MR",
        "MOVSD_RM",
        "MOVSD_XMM_FREG",
        "MOVUPS_MR",
        "MOVUPS_RM",
        "MOVZX_R16_MEM",
        "MOVZX_R8_MEM",
        "MOV_R8_RM64",
        "MOV_REG_IMM64",
        "MOV_RM8_R64",
        "MOV_RM_R",
        "MOV_R_RM",
        "SUB64_R_IMM32",
        "SUB_R_IMM32",
        "VADDPS_ZMM_MASK",
        "VADDPS_ZMM_MASKZ",
        "VMOVUPS_MR",
        "VMOVUPS_RM",
        "VMOVUPS_ZMM_MEM",
        "VMOVUPS_ZMM_MR",
    ];
    let riscv = ["ADDI", "ADDI_GLOBAL", "AUIPC", "AUIPC_GLOBAL"];
    let pinned: &[(&str, &[&str])] = &[
        ("x86_v12", &x86),
        ("riscv64_v12", &riscv),
        ("arm64_v12", &[]),
    ];
    for r in reports() {
        let want = pinned
            .iter()
            .find(|(n, _)| *n == r.name)
            .map(|(_, l)| l.to_vec())
            .unwrap_or_default();
        let mut got = r.ambiguous.clone();
        got.sort_unstable();
        let mut want_sorted = want;
        want_sorted.sort_unstable();
        assert_eq!(
            got, want_sorted,
            "{}: 文本歧义名单变了——确认是否真有新歧义（若只是谱更名，同步本名单）",
            r.name
        );
    }
}

/// 打印当前报告（`--nocapture` 可见）：更新上面的钉死值前先看这里。
#[test]
fn print_spec_coverage_report() {
    for r in reports() {
        eprintln!(
            "SPEC-COVERAGE {}: total={} covered={} skipped={:?} ambiguous={} {:?}",
            r.name,
            r.total,
            r.covered,
            r.skipped,
            r.ambiguous.len(),
            r.ambiguous
        );
    }
}
