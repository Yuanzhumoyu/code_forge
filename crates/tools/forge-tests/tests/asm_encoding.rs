//! **真实汇编语料 · 编码对拍档**（v20 V9）。
//!
//! 上游 LLVM MC 用例把每条的期望字节写在注释里（`# CHECK-ASM: encoding: [0x…]`），
//! 紧跟在它要校验的那条指令**之前**。本档把 `(指令原文, 上游字节)` 抽出来，凡是
//! **本汇编器解析得动**的那条，就要求 `encode()` 出来的字节与上游**逐字节相等**。
//!
//! 这是不依赖模拟器、也不依赖我们自建黄金表的**外部**编码 oracle：谱里编码键写错
//! （位域位置/顺序/立即数打包）会在这里红，而不是等某个执行用例凑巧撞上。
//!
//! 解析不动的行**不算失败**（那归解析档的棘轮管）——本档只保证"能解析的就不能编错"。
//! 若某架构的语料根本没写 `encoding:` 注释，本档报 `ASM-ENCODING-SKIP` 并说明，**不假绿**。

use forge_tests::asm::{self, SUITES};

/// 三架构的"上游字节 oracle"对拍。
#[test]
fn encodings_of_parsable_corpus_lines_match_upstream() {
    let mut checked_total = 0usize;
    let mut mismatched: Vec<String> = Vec::new();
    for isa in ["x86", "riscv64", "aarch64"] {
        let target = asm::targets::target_for(isa).expect("三架构都有适配器");
        let mut checked = 0usize;
        let mut unparsed = 0usize;
        let mut cases = 0usize;
        for suite in SUITES.iter().filter(|s| s.isa == isa) {
            let Ok(files) = asm::corpus::read_suite(suite) else {
                continue;
            };
            for (file, src) in &files {
                for case in asm::encoding::extract_cases(suite, file, src) {
                    cases += 1;
                    if target.parse(&case.text).is_err() {
                        unparsed += 1;
                        continue;
                    }
                    let got = match asm::targets::assemble_to_bytes(isa, &case.text) {
                        Ok(b) => b,
                        Err(e) => {
                            mismatched.push(format!(
                                "{}:{} `{}` 解析通过却汇编失败：{e}",
                                case.file, case.line_no, case.text
                            ));
                            continue;
                        }
                    };
                    checked += 1;
                    if got != case.bytes {
                        mismatched.push(format!(
                            "{}:{} `{}`：我们 {} ≠ 上游 {}",
                            case.file,
                            case.line_no,
                            case.text,
                            hex(&got),
                            hex(&case.bytes)
                        ));
                    }
                }
            }
        }
        let line = format!(
            "{isa}: cases={cases} checked={checked} unparsed={unparsed} mismatched={}",
            mismatched.len()
        );
        println!("ASM-ENCODING-SUMMARY {line}");
        asm::report::emit_event("ASM-ENCODING-SUMMARY", &line);
        if cases == 0 {
            let why = format!("{isa}: 语料里没有 `encoding:` 期望字节（补齐见 asm/PROVENANCE.md）");
            println!("ASM-ENCODING-SKIP {why}");
            asm::report::emit_event("ASM-ENCODING-SKIP", &why);
        }
        checked_total += checked;
    }
    assert!(
        mismatched.is_empty(),
        "汇编结果与上游字节不符（{} 条）：\n  {}",
        mismatched.len(),
        mismatched.join("\n  ")
    );
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
