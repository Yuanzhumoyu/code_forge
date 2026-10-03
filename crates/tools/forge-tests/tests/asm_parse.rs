//! **真实汇编语料 · 解析档**（v20 V9）。
//!
//! 把 `asm/parse/<isa>/<suite>/**` 的上游语料逐行喂给**生成物的汇编器**
//! （`Assembler::parse_insts`），按四桶归因（见 `forge_tests::asm::classify`）：
//!
//! - `Parsed`：线性扫描整条命中；
//! - `NoPrefix`：失败，且**没有任何候选的首段**能对上（上游比我们全，正常）；
//! - **`TailMismatch`：某条候选首段对得上、整条没对上**——门禁只盯这一桶；
//! - `CorpusOnly`：伪指令/宏/预处理/一行多语句/依赖外部标签（语料特性）。
//!
//! 桶判定**不问"首词是不是助记符"**，只问扫描器自己（`could_be_instruction`）。
//!
//! 门禁 = **计数棘轮**：各桶计数必须与 `asm/ratchet/<isa>.txt` 逐项相等（多一个 = 新缺陷，
//! 少一个 = 修好了/漏跑，两个方向都红）；红桶样例（人核过的方言缺口）留在棘轮文件里。
//!
//! 语料没下载（`asm/parse/...` 缺）⇒ 该档 `ASM-SKIP` 并说明原因，**不假绿**。

use forge_tests::asm::{self, IsaReport, SUITES};

/// 跑一个 ISA 的全部 suite。
fn run_isa(isa: &str) -> IsaReport {
    let target = asm::targets::target_for(isa).expect("三架构都有适配器");
    let mut report = IsaReport {
        isa: isa.to_string(),
        ..Default::default()
    };
    for suite in SUITES.iter().filter(|s| s.isa == isa) {
        match asm::corpus::read_suite(suite) {
            Ok(files) => {
                let reports: Vec<_> = files
                    .iter()
                    .map(|(f, src)| asm::classify_file(target, suite, f, src))
                    .collect();
                report
                    .suites
                    .push(IsaReport::from_files(suite.key, reports));
            }
            Err(e) => asm::report::emit_event(
                "ASM-SKIP",
                &format!("{isa}/{}: 语料不在（{e}）", suite.key),
            ),
        }
    }
    report.corpus_present = !report.suites.is_empty();
    report
}

/// 计数棘轮（三架构）。
#[test]
fn corpus_parse_counts_match_the_ratchet() {
    for isa in ["x86", "riscv64", "aarch64"] {
        let report = run_isa(isa);
        let line = report.summary_line();
        println!("{line}");
        asm::report::emit_event("ASM-SUMMARY", &line);
        if !report.corpus_present {
            println!("ASM-SKIP {isa}: 语料未下载——跑 `crates/tools/forge-tests/asm/fetch.ps1`");
            continue;
        }
        let path = asm::corpus_root()
            .join("ratchet")
            .join(format!("{isa}.txt"));
        // 重刷棘轮（bless）：`FORGE_ASM_WRITE_RATCHET=1`——只在**看过差异**后手工用，
        // CI 与日常跑都不设它（否则棘轮就白设了）。
        if std::env::var_os("FORGE_ASM_WRITE_RATCHET").is_some() {
            std::fs::write(&path, report.to_ratchet())
                .unwrap_or_else(|e| panic!("写 {} 失败：{e}", path.display()));
            println!("ASM-RATCHET-WRITTEN {}", path.display());
            continue;
        }
        if let Err(e) = report.check_ratchet(&path) {
            asm::report::emit_event("ASM-FAIL", &format!("{isa}: 棘轮不一致"));
            panic!("{e}");
        }
        // 机读记分板（每次跑都刷新，便于 CI artifact）。
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("target/asm-suite");
        let _ = std::fs::create_dir_all(&out);
        let _ = std::fs::write(out.join(format!("{isa}.json")), report.to_json());
    }
}

/// 正对照：这些**真实语法**的行必须解析成功（上游语料里大量行我们本就不支持，
/// 光看语料计数看不出"汇编器到底能吃什么"——这一组是钉子，回归时先红这里）。
///
/// 每条都取自上游语料的写法（寄存器名/内存形式/立即数写法），**不是我们自造语法**。
#[test]
fn curated_positive_lines_parse() {
    const CASES: &[(&str, &str)] = &[
        // x86：Intel 语法 + AT&T 式内存（GAS Intel 模式）——见 asm/parse/x86/gnu-gas-intel
        ("x86", "mov rax, rbx"),
        ("x86", "add rax, rcx"),
        ("x86", "sub rax, 1"),
        ("x86", "push rax"),
        ("x86", "pop rbx"),
        ("x86", "ret"),
        ("x86", "cmp rax, rbx"),
        ("x86", "test rax, rbx"),
        ("x86", "mov rax, [rbx]"),
        ("x86", "mov rax, [rbx+8]"),
        // 已知缺口（见 asm/README.md）：32 位目的地的立即数搬运（`mov eax, 0x1234`）谱里没有，
        // 只有 64 位的 `MOV_REG_IMM64`；这里钉住 64 位那条。
        ("x86", "mov rax, 0x1234"),
        // riscv64：x-名、偏移内存、符号立即数——见 asm/parse/riscv64/llvm-mc
        ("riscv64", "ld x10, -2048(x11)"),
        ("riscv64", "sd x16, -2048(x17)"),
        ("riscv64", "slli x22, x23, 45"),
        ("riscv64", "srai x26, x27, 31"),
        ("riscv64", "add x10, x11, x12"),
        ("riscv64", "sub x10, x11, x12"),
        ("riscv64", "beq x1, x2, 8"),
        ("riscv64", "jal x1, 8"),
        ("riscv64", "ret"),
        // aarch64：见 asm/parse/aarch64/llvm-mc
        ("aarch64", "mov x0, x1"),
        ("aarch64", "add x0, x1, x2"),
        ("aarch64", "ret"),
    ];
    let mut bad = Vec::new();
    for (isa, text) in CASES {
        let t = asm::targets::target_for(isa).expect("适配器存在");
        if let Err(e) = t.parse(text) {
            bad.push(format!("{isa}: `{text}` → {e:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "这些真实语法行解析失败了（{} 条）：\n  {}",
        bad.len(),
        bad.join("\n  ")
    );
}
#[test]
fn scan_probe_is_wired_for_all_three_isas() {
    // 探针 = 生成物的线性扫描第一步（`could_be_instruction`）：它是**必要条件**——
    // `false` 必须意味着"这段一定不是本 ISA 的指令"，`true` 不保证整条能汇编。
    for (isa, known, unknown) in [
        (
            "x86",
            "mov rax, rbx",
            "definitely_not_an_instruction rax, rbx",
        ),
        (
            "riscv64",
            "add x10, x11, x12",
            "definitely_not_an_instruction x10, x11",
        ),
        (
            "aarch64",
            "add x0, x1, x2",
            "definitely_not_an_instruction x0, x1",
        ),
    ] {
        let t = asm::targets::target_for(isa).expect("适配器存在");
        assert!(
            t.could_be_instruction(known),
            "{isa}: `{known}` 的首段应当能对上（探针说不可能）"
        );
        assert!(
            !t.could_be_instruction(unknown),
            "{isa}: `{unknown}` 不可能是本 ISA 的指令，探针却说可以"
        );
        assert!(
            !t.could_be_instruction(""),
            "{isa}: 空文本不该被探针判成可能是指令"
        );
        // 必要条件：探针说不行的，解析一定失败（反之不成立）。
        assert!(
            t.parse(unknown).is_err(),
            "{isa}: 探针说不是本 ISA 的指令，解析却成功了——两者矛盾"
        );
        assert!(
            t.could_be_instruction(known) && t.parse(known).is_ok(),
            "{isa}: `{known}` 应当既过探针又解析成功"
        );
    }
}
