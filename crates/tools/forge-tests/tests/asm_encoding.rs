//! **真实汇编语料 · 编码对拍档**（v20 V9）。
//!
//! 上游 LLVM MC 用例把每条的期望字节写在注释里（四种写法见
//! `forge_tests::asm::encoding` 的模块文档）。本档把 `(指令原文, 上游字节)` 抽出来，
//! 凡是**本汇编器解析得动**的那条，就要求 `encode()` 出来的字节与上游**逐字节相等**。
//!
//! 这是不依赖模拟器、也不依赖我们自建黄金表的**外部**编码 oracle：谱里编码键写错
//! （位域位置/顺序/立即数打包）会在这里红，而不是等某个执行用例凑巧撞上。
//!
//! 三种结果分开记：
//!
//! - **一致**：直接过；
//! - **等价编码**（`variants`）：字节不同，但两边喂给**我们自己的解码器**再反汇编，
//!   渲染文本相同（如 x86 `xor rax, 12` 的 imm32 形式与符号扩展 imm8 形式）——记账，不算缺陷；
//! - **已知差异**（`known`）：字节不同且不满足等价条件（可能是我们**解不出来**的上游编码）
//!   ——逐条写进 `asm/ratchet/encoding.txt`，**新增一条红、少一条也红**
//!   （与解析档同一套"两个方向都红"的纪律，避免门禁永久红或静默放过）。
//!
//! 解析不动的行不算失败（那归解析档的棘轮管）；重刷棘轮：`FORGE_ASM_WRITE_RATCHET=1`。
//! 想看逐条配对结果：`FORGE_ASM_ENCODING_CASES=1 -- --nocapture`。

use forge_tests::asm::report::EncodingReport;
use forge_tests::asm::{self, SUITES};

/// 三架构的"上游字节 oracle"对拍 + 棘轮。
#[test]
fn encodings_of_parsable_corpus_lines_match_upstream() {
    let mut reports: Vec<EncodingReport> = Vec::new();
    let mut checked_total = 0usize;
    for isa in ["x86", "riscv64", "aarch64"] {
        let target = asm::targets::target_for(isa).expect("三架构都有适配器");
        let mut r = EncodingReport {
            isa: isa.to_string(),
            ..Default::default()
        };
        let mut variants: Vec<String> = Vec::new();
        for suite in SUITES.iter().filter(|s| s.isa == isa) {
            let Ok(files) = asm::corpus::read_suite(suite) else {
                continue;
            };
            for (file, src) in &files {
                let extracted = asm::encoding::extract_cases(suite, file, src);
                r.dropped += extracted.dropped;
                for case in extracted.cases {
                    r.cases += 1;
                    match target.parse(&case.text) {
                        Ok(_) => {}
                        Err(e) => {
                            report_case(isa, &case, &format!("unparsed {e:?}"));
                            r.unparsed += 1;
                            continue;
                        }
                    }
                    report_case(isa, &case, "extracted");
                    let got = match asm::targets::assemble_to_bytes(isa, &case.text) {
                        Ok(b) => b,
                        Err(e) => {
                            r.known.push(format!(
                                "{}:{} `{}` 解析通过却汇编失败：{e}",
                                case.file, case.line_no, case.text
                            ));
                            continue;
                        }
                    };
                    r.checked += 1;
                    if got == case.bytes {
                        report_case(isa, &case, "ok");
                        continue;
                    }
                    report_case(isa, &case, "MISMATCH");
                    let what = format!(
                        "{}:{} `{}`：我们 {} ≠ 上游 {}",
                        case.file,
                        case.line_no,
                        case.text,
                        hex(&got),
                        hex(&case.bytes)
                    );
                    // 同一指令的两种合法编码不算缺陷：两边都喂给**我们自己的解码器**，
                    // 渲染文本相同即等价（解不出来 ⇒ 不能算等价，见 `known`）。
                    let ours_dis = asm::targets::disassemble_bytes(isa, &got);
                    let upstream_dis = asm::targets::disassemble_bytes(isa, &case.bytes);
                    match (&ours_dis, &upstream_dis) {
                        (Ok(a), Ok(b)) if a == b => {
                            r.variants += 1;
                            variants.push(format!("{what} —— 同为 `{a}`"));
                        }
                        _ => r.known.push(format!(
                            "{what}（我们解出 {ours_dis:?} / 上游解出 {upstream_dis:?}）"
                        )),
                    }
                }
            }
        }
        let line = r.summary_line();
        println!("{line}");
        asm::report::emit_event("ASM-ENCODING-SUMMARY", &line);
        if r.cases == 0 {
            let why = format!("{isa}: 语料里没有 `encoding:` 期望字节（补齐见 asm/PROVENANCE.md）");
            println!("ASM-ENCODING-SKIP {why}");
            asm::report::emit_event("ASM-ENCODING-SKIP", &why);
        }
        checked_total += r.checked;
        for v in &variants {
            println!("ASM-ENCODING-VARIANT {v}");
            asm::report::emit_event("ASM-ENCODING-VARIANT", v);
        }
        reports.push(r);
    }

    // 棘轮：计数 + 已知差异清单，两个方向都红。
    let path = asm::corpus_root().join("ratchet").join("encoding.txt");
    if std::env::var_os("FORGE_ASM_WRITE_RATCHET").is_some() {
        std::fs::write(&path, asm::report::encoding_ratchet(&reports))
            .unwrap_or_else(|e| panic!("写 {} 失败：{e}", path.display()));
        println!("ASM-RATCHET-WRITTEN {}", path.display());
    } else if let Err(e) = asm::report::check_encoding_ratchet(&reports, &path) {
        asm::report::emit_event("ASM-FAIL", "编码对拍棘轮不一致");
        panic!("{e}");
    }
    assert!(
        checked_total > 0,
        "一条都没对拍上——上游 oracle 没接上（看上面的 ASM-ENCODING-SKIP）"
    );
}

/// 字节串的十六进制打印（`0x13,0x9b,…`），失败信息里对齐上游写法。
fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("0x{x:02x}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// 逐条打印**配对了什么**（`FORGE_ASM_ENCODING_CASES=1`）——配对规则出错时靠它定位。
fn report_case(isa: &str, case: &asm::EncodingCase, verdict: &str) {
    if std::env::var_os("FORGE_ASM_ENCODING_CASES").is_none() {
        return;
    }
    let line = format!(
        "{isa} {}:{} `{}` = {} [{verdict}]",
        case.file,
        case.line_no,
        case.text,
        hex(&case.bytes)
    );
    println!("ASM-ENCODING-CASE {line}");
    asm::report::emit_event("ASM-ENCODING-CASE", &line);
}
