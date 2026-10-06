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
    for isa in ["x86.toml", "arm64.toml", "riscv64.toml"] {
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
    let path = root().join("isa/arm64.toml");
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
    // 宿主管线 7 → 8（2026-10-01）：`VaStart` 由**管线**展开成显式 IR（谱里没有 `[[lowering]]`），
    // 归入 `HOST_PIPELINE_OPS`；覆盖数与三谱的真缺口数因此不变。
    // riscv 覆盖 61 → 63（2026-10-01）：补了 `Fload`/`Fstore` 的 lowering——变参的**浮点支**要在
    // riscv 上真跑，`va_arg(f64)` 展开出的 `Fload` 必须能降级（真缺口 42 → 40）。
    // riscv 覆盖 63 → 67 / 真缺口 40 → 36（2026-10-01）：补了浮点算术
    // `Fadd`/`Fsub`/`Fmul`/`Fdiv`（单/双精度各一条；此前这些 op 全是真缺口，
    // 矩阵里所有浮点算术用例因此整条 Skip）。
    for (isa, want) in [
        ("x86.toml", (100usize, 6usize, 8usize, 3usize)),
        ("riscv64.toml", (67, 6, 8, 36)),
        ("arm64.toml", (8, 6, 8, 95)),
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
    let x86 = coverage("x86.toml", &host_ops);
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
        ("x86.toml", 1usize),
        ("riscv64.toml", 0),
        ("arm64.toml", 27),
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
    let path = root().join("isa/arm64.toml");
    let spec = report::load_spec(&path).expect("加载 arm64");
    let injected = "[[instructions]]\nname = \"OVERLAPTOY\"\nform = \"PAIR\"\nopcode = 0xA9\n\
                    fields = { idx2 = 0, opc2 = 0 }\n\
                    ops = [\"t1:r64\", \"base:r64\", \"t2:r64\", \"imm:imm7x\"]\n\
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

/// V4d 的"未指定位"清单（**opt-in** `--bits`）：只对定宽 ISA 判。
///
/// 实测（2026-09-24）与人工核对结论：
///
/// - **x86 0 条**：`prefix_scan` 变长 ISA，前缀/REX/ModRM/VEX 由编码器发射、不在位域表里
///   建模——按表判必成误报，所以这条规则整个跳过它；
/// - **riscv64 7 条**：`NOP`/`ECALL`/`EBREAK`（2026-10-04 起 + `FENCE`/`FENCE_PS`/`FENCE_I`/`FENCE_TSO`
///   4 条——fence 族改成字段形后，rd/rs1/funct3/fm 是真正的固定 0 段）——系统指令的"固定位型"（高位恒 0），
///   故意只声明 `opcode` 而不逐位声明 0（语义上它们是固定字，不是带保留位的指令字）；
///   AMO/LR/SC 的 `aq`/`rl` 原本也在这张清单里（8 条），**已修谱**：显式声明两个位域
///   （RV64A 的 acquire/release）并在四条模板体里写 0——逐字节不变（`cargo test -p
///   forge-codegen --lib` 1265 passed）；
/// - **arm64 123 条**（v20 A5 补 FP 搬运后 67 → 71：`FMOV` 的保留位段 + `LDUR/STUR` FP 族的固定 0 段；
///   2026-10-04 补逻辑（移位寄存器）族后 71 → **123**，+52 条全部是新指令的**固定 0 位**：
///   X 形式无 N 位的 16 条（`and`/`ands`/`orr`/`eor` × 4 移位）只差 `[21,22)`（N=0，N=1 的
///   `bic`/`bics`/`orn`/`eon` 另声明了 `nbit`），W 形式 32 条多出 `[15,16)`（A64 的
///   "32 位形式 imm6<32" 那条约束位，故 W 用 5 位槽 `imm5`），另有 `ands`/`bics` 无后缀形态 4 条；
///   2026-10-04 补 `tbz`/`tbnz` 后 123 → **125**：X 形式（`TBZX`/`TBNZX`）的 `b5`[31] 是
///   位序号的一部分（多字段落点，已进位域视图），W 形式（`TBZW`/`TBNZW`）的 bit31
///   才是**真正的保留位**（32 位形式的位序号只有 5 位，bit31 必须为 0）——各 +1；
///   2026-10-04 收尾三条写法后 125 → **128**（`RETR` 的 [0,5)+[10,16)、`B_AL`/`B_NV` 的 bit4
///   ——B.cond 的固定 0 位）。
///   全是保留位（A64 里 `BR`/`RET`/`B.cond`/`LDUR` 一类的固定 0 段），
///   抽查 `[4,5)`（B.cond 的固定 0 位）、`[0,5)`+`[10,16)`（BR/BLR/RET）、`[10,12)`+`[21,24)`
///   （LDUR 族）都对得上参考编码——**不是谱的缺陷**，因此保持 opt-in 的评审清单。
///   2026-10-07（aarch64 前后索引批次）：128 → **120**——新加的 8 条前后索引形态与已有的
///   8 条 LDUR/STUR 系（GPR 4 + FP 4）都把 `mode`（[11:10]，unscaled = 00）与保留位
///   `zero21`（[21]）**显式声明**了（编码器本来就发 0 ⇒ 字节不变），这 16 条的未指定位
///   因此从清单里消失。
///   2026-10-07（aarch64 寄存器偏移批次）：120 → **128**——新加的 8 条 `ADD/SUB(S) …, lsl #N`
///   与**既有逻辑族同指纹**（`[21,22)` = 移位寄存器族的 N 位恒 0、W 形式还有 `[15,16)` =
///   imm6 的最高位恒 0），这几条按现状靠"缺省 0 发射"⇒ 清单 +8。要清掉得连整个移位寄存器族
///   一起显式声明（另有其批），不是本轮的缺陷。
///   2026-10-07（aarch64 扩展寄存器批次）：128 → **176**——新加的 48 条扩展寄存器指令各带
///   `[22,24)`（`sf op S 01011 00 1 …` 里那个常量 `00` 段），与既有 `ADDREG` 一族**同指纹**，
///   同样按缺省 0 发射 ⇒ 清单 +48。这是"未指定位清单"的正常增长（保留位/固定位型），
///   不是漏字段；真要清掉得连移位/扩展两族一起显式声明。
#[test]
fn unassigned_bits_inventory() {
    for (isa, want) in [
        ("x86.toml", 0usize),
        ("riscv64.toml", 7),
        ("arm64.toml", 176),
    ] {
        let path = root().join("isa").join(isa);
        let spec = report::load_spec(&path).expect("加载谱");
        let opts = LintOpts {
            check_unassigned_bits: true,
            ..Default::default()
        };
        let report = lint_source_opts(&spec.text, &opts).expect("谱合法");
        let n = report
            .findings
            .iter()
            .filter(|d| d.code == "LINT-UNASSIGNED-BITS")
            .count();
        assert_eq!(n, want, "{isa} 的未指定位条数变了：{:?}", report.findings);
        // 默认档必须仍然零结论。
        let plain = lint_source(&spec.text).expect("谱合法");
        assert!(
            plain.iter().all(|d| d.code != "LINT-UNASSIGNED-BITS"),
            "{isa} 默认档不该报未指定位"
        );
    }
}

/// V4d 的"可合并为 `vary` 的族"清单（**opt-in** `--suggest`，只建议、不当门槛）。
///
/// 数字变了 ⇒ 有人合并/拆分了 lowering 族（好事就同步本快照），或新增了同形状的规则。
/// riscv 22 → 24（2026-10-01）：补 `Fload`/`Fstore` 的 lowering（单/双精度两条同形状，
/// 与既有 `Load`/`Store` 一样是"建议合并"的候选，不强行 `vary`——宽窄两条写法更直观）。
/// riscv 24 → 28（2026-10-01）：同上，浮点算术 `Fadd`/`Fsub`/`Fmul`/`Fdiv` 各多一条同形状
/// 建议（S 版有 `when`、D 版是兜底，合并成 `vary` 会丢掉"哪条是兜底"的直观性）。
#[test]
fn vary_candidate_inventory() {
    for (isa, want) in [
        ("x86.toml", 55usize),
        ("riscv64.toml", 28),
        ("arm64.toml", 7),
    ] {
        let path = root().join("isa").join(isa);
        let spec = report::load_spec(&path).expect("加载谱");
        let opts = LintOpts {
            suggest_vary: true,
            ..Default::default()
        };
        let report = lint_source_opts(&spec.text, &opts).expect("谱合法");
        let n = report
            .findings
            .iter()
            .filter(|d| d.code == "LINT-VARY-CANDIDATE")
            .count();
        assert_eq!(n, want, "{isa} 的 vary 建议条数变了：{:?}", report.findings);
        // 默认档必须仍然零结论（建议不进门槛）。
        let plain = lint_source(&spec.text).expect("谱合法");
        assert!(
            plain.iter().all(|d| d.code != "LINT-VARY-CANDIDATE"),
            "{isa} 默认档不该给 vary 建议"
        );
    }
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
