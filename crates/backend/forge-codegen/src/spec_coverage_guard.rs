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
        report_of!("x86", crate::arch::x86::x86::__spec_tests),
        report_of!("riscv64", crate::arch::riscv64::riscv64::__spec_tests),
        report_of!("arm64", crate::arch::arm64::arm64::__spec_tests),
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

/// 指令总数钉死（2026-09-20 实测；arm64 自 S3c 加 `b.cond` 16 行模板后 = 104，v20 A5 补浮点搬运 = 110；
/// riscv64 v20 V6+ 补 `fpr_mov` 的双精度档 `fsgnj.d` 后 = 117。
/// x86 v20 V10 补内存形式的 ALU 族（`*_MR` / `*_R_MEM` 各 8 条 → 237）、8 位 ALU 族
/// （`*_MR_8` / `*_R_MEM_8` / `*_RM8_IMM8` 各 8 条 → 261）、一元 `inc`/`dec`
/// （`INCDEC_RM` / `INCDEC_RM_8` 各 2 行 → 265）与**内存形式的 mov 族**
/// （`MOV_R_MEM{,_8}_AUTO` / `STORE_MEM_R{,_8}_AUTO` / `MOVSXD_R_MEM` / `XCHG_MEM_R_AUTO`
/// 6 条 → **271**：8/16/32 位 mov 全部走完整内存模板 + movsxd/xchg 的内存源形式）
/// 与 **SSE 比较谓词族**（`CMPPS`/`CMPPD`/`CMPSS`/`CMPSD_SCALAR` 4 条 → **275**；经典八谓词
/// 的 packed 别名是 `[[pseudo]]` 文本展开，不占指令数）后 = 275。
/// riscv64 补**字节/半字访存**（`LB`/`LH`/`LBU`/`LHU`/`SB`/`SH` 6 条）与 **W 立即数移位**
/// （`SLLIW`/`SRLIW`/`SRAIW` 3 条；`SLLW`/`SRLW`/`SRAW` 改成 R 型并进 `WW` 模板行，总数不变）
/// 后 = 128；再补 `BGEU`（`BB` 模板一行）、`FENCE_I`/`FENCE_TSO` 与 **Zicsr 六条**
/// （`CSRRW`/`CSRRS`/`CSRRC`/`CSRRWI`/`CSRRSI`/`CSRRCI`）→ **137**。`unimp` 是 `[[pseudo]]`
/// （别名 = `csrrw x0, cycle, x0`），不占指令数）。arm64 补**系统/异常生成族**
/// （`SVC`/`HVC`/`SMC`/`BRK`/`HLT`/`DCPS1..3` 8 条 + `ERET`/`DRPS` 两个整字常量）→ **120**。
/// arm64 再补**逻辑（移位寄存器）族**：带移位后缀的 8 助记符（`and`/`ands`/`bic`/`bics`/
/// `orr`/`orn`/`eor`/`eon`）× 4 种移位（`lsl`/`lsr`/`asr`/`ror`）× X/W = 64 条（移位种类
/// 是 2 位常量、移位量是操作数槽），外加 `ands`/`bics` 的**无后缀**形态 4 条 → **188**；
/// 再补 `TBZ`/`TBNZ` × X/W 4 条（位序号一个操作数摊到 `b40`+`b5` 两个位域）→ **192**；
/// 再补**逻辑（立即数）族**（`and`/`orr`/`eor`/`ands` × X/W = 8 条，值 → N/immr/imms 三分量）→ **200**；
/// 再补 `ret xN`（`RETR`）、`b.al`/`b.nv`（A64 保留码，上游汇编器收）与裸 `dcps1/2/3`（= `dcpsN #0`）6 条 → **206**）。`n/// `XZR`/`WZR`/`LR` 是 `[reg.*].aliases`（解析认、渲染出主名），不占指令数。
#[test]
fn spec_coverage_totals_are_pinned() {
    let totals: Vec<(&str, usize)> = reports().iter().map(|r| (r.name, r.total)).collect();
    assert_eq!(
        totals,
        vec![("x86", 275), ("riscv64", 137), ("arm64", 206)],
        "指令总数变了：确认是谱的预期变更还是指令丢失"
    );
}

/// 文本歧义名单钉死（更新前先跑 `print_spec_coverage_report` 看当前值）。
///
/// v20 V10 起 x86 少了 6 条：`MOV64_{MR,RM}` / `MOVSD_{MR,RM}` / `MOVUPS_{MR,RM}`——
/// 它们与「反方向」的同名指令（`mov {mem}, {src}` 对 `mov {dst}, {mem}`）**文本并不同**
/// （一个是 `mov [RAX], RBX`，一个是 `mov RAX, [RBX]`），此前被歧义键按"操作数声明序"
/// 建键误判成同形。键改成"按模板占位符序 + 不带操作数序号"后，这几条恢复**强断言**
/// （`disasm → asm → encode` 必须回到同一字节），实测全部通过。
#[test]
fn spec_text_ambiguity_lists_are_pinned() {
    let x86 = [
        "ADC_R_IMM32",
        "ADC_R_IMM8S",
        "ADD_R_IMM32",
        "ADD_R_IMM8S",
        "AND_R_IMM32",
        "AND_R_IMM8S",
        "CMP_R_IMM32",
        "CMP_R_IMM8S",
        "LEA_R64_SIB",
        "LEA_RBP_OFF",
        "MOV64_RR",
        "MOVABS_GLOBAL",
        "MOVQ_FREG_XMM",
        "MOVQ_XMM_FREG",
        "MOVSD",
        "MOVSD_XMM_FREG",
        "MOVZX_R16_MEM",
        "MOVZX_R8_MEM",
        "MOV_R8_RM64",
        "MOV_REG_IMM64",
        "MOV_RM8_R64",
        "MOV_RM_R",
        "MOV_R_RM",
        "OR_R_IMM32",
        "OR_R_IMM8S",
        "SBB_R_IMM32",
        "SBB_R_IMM8S",
        "SUB_R_IMM32",
        "SUB_R_IMM8S",
        "VADDPS_ZMM_MASK",
        "VADDPS_ZMM_MASKZ",
        "VMOVUPS_MR",
        "VMOVUPS_RM",
        "VMOVUPS_ZMM_MEM",
        "VMOVUPS_ZMM_MR",
        "XOR_R_IMM32",
        "XOR_R_IMM8S",
    ];
    let riscv = ["ADDI", "ADDI_GLOBAL", "AUIPC", "AUIPC_GLOBAL"];
    let pinned: &[(&str, &[&str])] = &[
        ("x86", &x86),
        ("riscv64", &riscv),
        // v20 A5：arm64 的 FP 寄存器组用统一的 `V0..V31` 命名（不像 x86 那样 S/D 名字本身带宽度），
        // 因此 `fmov v0, v1` / `ldur v0, [x29, #8]` 的 S/D 两种编码**汇编文本相同**——
        // 反汇编按声明序取第一条（S），文本往返对这两族只能取其一。这是**已知且刻意**的
        // 取舍（宽度在指令里、不在名字里）；要消掉就得给 S/D/Q 各开一组别名寄存器。
        (
            "arm64",
            &["FMOV_D", "FMOV_S", "LDURD", "LDURS", "STURD", "STURS"],
        ),
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
