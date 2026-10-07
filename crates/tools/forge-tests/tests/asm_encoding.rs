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
//!
//! 喂给汇编器的文本走**解析档同一份上下文**（`asm::corpus::with_prelude`：标签 + 该行
//! 之前的 `.equ`/`.set` 符号定义），否则依赖上游符号常量的用例只会被记成 "unparsed"
//! 混过去（`cmp eax, FOO` 的 `[0x83,0xf8,0x02]` 就是这么漏掉的）。

use forge_tests::asm::report::{EncodingFileRow, EncodingReport};
use forge_tests::asm::{self, SUITES};

/// 三架构的"上游字节 oracle"对拍 + 棘轮。
#[test]
fn encodings_of_parsable_corpus_lines_match_upstream() {
    if !asm::corpus::corpus_present() {
        eprintln!(
            "ASM-CORPUS-MISSING: asm/parse 下没有语料（B1 起语料是**本地缓存**，不入 git）——\
             先跑 `node crates/tools/forge-tests/asm/fetch.mjs` 拉取；本测试跳过"
        );
        return;
    }
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
            // 逐文件**并行**计算（文件多、逐例汇编+反汇编是主要开销）；结果按文件序回放，
            // 记分板/棘轮/事件序与串行**逐字节一致**（序由 `asm::par::map_parallel` 保证）。
            struct EncFileOut {
                row: EncodingFileRow,
                dropped: usize,
                variants: Vec<String>,
                known: Vec<String>,
                events: Vec<(asm::EncodingCase, String)>,
            }
            let outs = asm::par::map_parallel(&files, |(file, src)| {
                let extracted = asm::encoding::extract_cases(suite, file, src);
                // 与解析档同一套上下文（标签 + 该行之前的符号常量定义）：上游用例里
                // `cmp eax, FOO`（`.set FOO, 2`）这种必须先解出符号，才能拿上游注释里的
                // 期望字节真对拍——否则它只会被记成 "unparsed" 混过去。
                let corpus = asm::corpus::extract(suite, file, src);
                let mut o = EncFileOut {
                    row: EncodingFileRow { file: file.clone(), ..Default::default() },
                    dropped: extracted.dropped,
                    variants: Vec::new(),
                    known: Vec::new(),
                    events: Vec::new(),
                };
                for case in extracted.cases {
                    o.row.cases += 1;
                    let text = asm::corpus::with_prelude(&corpus, case.line_no, &case.text);
                    match target.parse(&text) {
                        Ok(_) => {}
                        Err(e) => {
                            o.row.unparsed += 1;
                            o.events.push((case, format!("unparsed {e:?}")));
                            continue;
                        }
                    }
                    let mut __verdict = "extracted".to_string();
                    let got = match asm::targets::assemble_to_bytes(isa, &text) {
                        Ok(b) => b,
                        Err(e) => {
                            o.known.push(format!(
                                "{}:{} `{}` 解析通过却汇编失败：{e}",
                                case.file, case.line_no, case.text
                            ));
                            o.row.known += 1;
                            o.events.push((case, __verdict));
                            continue;
                        }
                    };
                    o.row.checked += 1;
                    if got == case.bytes {
                        o.events.push((case, "ok".to_string()));
                        continue;
                    }
                    __verdict = "MISMATCH".to_string();
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
                            o.row.variants += 1;
                            o.row.variants += 1;
                            o.variants.push(format!("{what} —— 同为 `{a}`"));
                        }
                        _ => {
                            o.row.known += 1;
                            o.known.push(format!(
                                "{what}（我们解出 {ours_dis:?} / 上游解出 {upstream_dis:?}）"
                            ));
                        }
                    }
                    o.events.push((case, __verdict));
                }
                o
            });
            // 按文件序回放（累加 + 发事件），保证与串行一致。
            for o in outs {
                r.dropped += o.dropped;
                r.cases += o.row.cases;
                r.checked += o.row.checked;
                r.unparsed += o.row.unparsed;
                r.variants += o.row.variants;
                r.known.extend(o.known);
                variants.extend(o.variants);
                for (case, verdict) in o.events {
                    report_case(isa, &case, &verdict);
                }
                r.file_rows.push(o.row);
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

    // 逐文件记分板**先写**：`asm/fetch.mjs` 的语料取舍 = 「解析全绿 **且** 编码无差异」，
    // 判据从这里读；棘轮不一致会 panic，所以先写后判（与解析档同一条纪律）。
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("target/asm-suite");
    let _ = std::fs::create_dir_all(&out);
    let _ = std::fs::write(
        out.join("encoding-files.json"),
        asm::report::encoding_files_json(&reports),
    );

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
