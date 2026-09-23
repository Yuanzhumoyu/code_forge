//! lint 快照守卫（v19 V4a/V4b/V4c）：三份**发行谱**必须零结论（默认档）。
//!
//! "零误报"是 V4 的硬判据（计划 §5）：这份快照把当时的结论集固定在 **0**——任何新规则一旦
//! 在真实谱上冒结论，这个测试立刻红，逼作者先人工核对是"真阳性"还是"误报"。
//!
//! 真阳性出现时的正确动作是**修谱**：V4a 落地时 x86 就报出一个没人引用的 `gpr32` 槽
//! （`LINT-UNUSED-SLOT`），核对后删掉了它（删死槽不改变任何指令行为——`cargo test -p
//! forge-codegen --lib` 仍是 1150 passed）。**不要**把快照改成非零来"修"测试。
//!
//! V4c 追加三条规则，各自带自己的判据：
//!
//! - `LINT-BITFIELD-OVERLAP`（默认档）：arm64 首次就报 4 条 `idx3`×`op8`（STP/LDP 的 X/W
//!   四条）——核实为**真阳性**（`idx3` 的 bit24 与 `op8` 常量重复写同一位；两者取值恰好一致
//!   所以黄金字节没暴露它）⇒ 已修谱：`idx3`(offset 22,width 3) → `idx2`(offset 22,width 2)，
//!   值 4/5 → 0/1（`op8` 常量给 bit24=1）。修后 `cargo test -p forge-codegen --lib` 仍
//!   **1264 passed**（89 条 arm64 向量逐字节不变）。
//! - `LINT-OP-GAP`（需 `--ops`）：口径必须与计划 §10.3 逐数字一致（见下）。
//! - `LINT-REF-UNUSED`（**opt-in** `--refs`）：预留引用名是合法写法，只钉数字不做门槛。

use std::path::{Path, PathBuf};

use forge_isa_dsl::lint::{
    LintOpts, OpsCoverage, host_ops_from_toml, lint_source, lint_source_opts,
};
use forge_isa_dsl::report;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[test]
fn shipped_specs_are_lint_clean() {
    for isa in ["x86_v12.toml", "arm64_v12.toml", "riscv64_v12.toml"] {
        let path = root().join("isa").join(isa);
        let spec = report::load_spec(&path).unwrap_or_else(|d| panic!("加载 {isa} 失败：{d:?}"));
        let found = lint_source(&spec.text).unwrap_or_else(|d| panic!("{isa} 校验不过：{d:?}"));
        assert!(
            found.is_empty(),
            "{isa} 不该有 lint 结论（真阳性请修谱，别改快照）：{found:?}"
        );
    }
}

/// 守卫本身有效：追加一个没人引用的槽 ⇒ 必须报 `LINT-UNUSED-SLOT`。
#[test]
fn the_guard_can_fail() {
    let path = root().join("isa/arm64_v12.toml");
    let spec = report::load_spec(&path).expect("加载 arm64");
    let mutated = format!(
        "{}\n[[operand_slots]]\nname = \"dead_slot\"\nkind = \"imm\"\nwidth = 4\n",
        spec.text
    );
    let found = lint_source(&mutated).expect("变异谱仍合法");
    assert!(
        found
            .iter()
            .any(|d| d.code == "LINT-UNUSED-SLOT" && d.msg.contains("dead_slot")),
        "变异后必须报出来：{found:?}"
    );
}

/// V4c 的能力缺口口径必须与方案 §10.3 逐数字一致（这是"三类分开报"的判据）。
///
/// 真缺口变了（补了 lowering / 宿主加了 op）⇒ 必须人工复核后同步本快照与 §10.3。
#[test]
fn op_gap_matches_section_10_3() {
    let host_ops = host_ops();
    // (ISA, 覆盖, 终结, 宿主管线, 真缺口数)
    for (isa, want) in [
        ("x86_v12.toml", (100usize, 6usize, 7usize, 3usize)),
        ("riscv64_v12.toml", (61, 6, 7, 42)),
        ("arm64_v12.toml", (8, 6, 7, 95)),
    ] {
        let cov = coverage(isa, &host_ops);
        assert_eq!(
            (
                cov.covered,
                cov.terminators,
                cov.host_pipeline,
                cov.gaps.len()
            ),
            want,
            "{isa} 的能力缺口口径与 §10.3 不一致（补了 lowering？宿主加了 op？）"
        );
    }
    // x86 的三条真缺口点名（宿主与谱都没有）——其余两类不算缺口。
    let x86 = coverage("x86_v12.toml", &host_ops);
    assert_eq!(
        x86.gaps,
        vec!["AddrSpaceCast", "Resume", "VaArg"],
        "{x86:?}"
    );
}

/// V4c 的"未引用 `ref`"清单（**opt-in 档**）：预留名字是合法写法，因此只钉数字。
///
/// arm64 的 27 条是该谱"还没写的整数 lowering"预留的多态名（arm64 只有 8 条 lowering）；
/// x86 的 1 条（`vmovups`）疑似残留。数字变了 ⇒ 人工复核是"补了引用"还是"新残留"。
#[test]
fn unreferenced_ref_inventory() {
    for (isa, want) in [
        ("x86_v12.toml", 1usize),
        ("riscv64_v12.toml", 0),
        ("arm64_v12.toml", 27),
    ] {
        let path = root().join("isa").join(isa);
        let spec = report::load_spec(&path).expect("加载谱");
        let opts = LintOpts {
            check_unused_refs: true,
            ..Default::default()
        };
        let report = lint_source_opts(&spec.text, &opts).expect("谱合法");
        let n = report
            .findings
            .iter()
            .filter(|d| d.code == "LINT-REF-UNUSED")
            .count();
        assert_eq!(
            n, want,
            "{isa} 的未引用 ref 条数变了：{:?}",
            report.findings
        );
        // 默认档必须仍然零结论（预留 ref 不进默认档）。
        let plain = lint_source(&spec.text).expect("谱合法");
        assert!(
            plain.iter().all(|d| d.code != "LINT-REF-UNUSED"),
            "{isa} 默认档不该报未引用 ref"
        );
    }
}

/// 守卫本身有效（重叠规则）：给 arm64 加一条"两个字段抢同一批位"的指令 ⇒ 必须报。
#[test]
fn overlap_guard_can_fail() {
    let path = root().join("isa/arm64_v12.toml");
    let spec = report::load_spec(&path).expect("加载 arm64");
    let injected = "[[instructions]]\nname = \"OVERLAPTOY\"\nform = \"PAIR\"\nopcode = 0xA9\n\
                    fields = { idx2 = 0, opc2 = 0 }\n\
                    ops = [\"t1:r64\", \"base:r64\", \"t2:r64\", \"imm:imm7u\"]\n\
                    asm = \"stp {t1}, {t2}, [{base}, #{imm}]\"\n\n";
    let mutated = spec.text.replace(
        "[[instructions]]\nname = \"STPX\"",
        &format!("{injected}[[instructions]]\nname = \"STPX\""),
    );
    assert!(mutated.contains("OVERLAPTOY"), "注入点没找到");
    let found = lint_source(&mutated).expect("变异谱仍合法");
    assert!(
        found
            .iter()
            .any(|d| d.code == "LINT-BITFIELD-OVERLAP" && d.msg.contains("OVERLAPTOY")),
        "变异后必须报重叠：{found:?}"
    );
}

fn host_ops() -> Vec<String> {
    let text = std::fs::read_to_string(root().join("crates/foundation/forge-ir/ops.toml"))
        .expect("读宿主 op 表");
    host_ops_from_toml(&text).expect("解析宿主 op 表")
}

fn coverage(isa: &str, host_ops: &[String]) -> OpsCoverage {
    let path = root().join("isa").join(isa);
    let spec = report::load_spec(&path).expect("加载谱");
    let opts = LintOpts {
        host_ops: host_ops.to_vec(),
        ..Default::default()
    };
    lint_source_opts(&spec.text, &opts)
        .expect("谱合法")
        .ops_coverage
        .expect("给了 host_ops 就有覆盖率口径")
}
