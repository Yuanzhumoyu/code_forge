//! NEON 排列的**一致性测试**（B5）：同一族的两种拼法必须编出同一批字节。
//!
//! **为什么需要它**：本会话把 NEON 排列从"族 × 排列各一条指令"改成"一条指令 + 排列槽"
//! （`VEC3RA` + `varr`/`varr_q`），并**保留**助记符后缀拼法 ⇒ 同一语义有**两条**指令面。
//! 批量铺开时曾出现过"新拼法编出的字节与助记符后缀兄弟不一致"（全集 `known` +6 ✗），
//! 当时**没有任何测试能拦住**它——只能靠事后对比全集数字 ✗。本测试把它变成硬判据：
//! 逐族、逐排列断言两条路径的字节**完全相同**。新增族时自动纳入（没实现的一侧跳过 ✓）。
//!
//! 未实现的一侧（例如语料没给字节、因而没生成的排列）**跳过而不失败** ✓：本测试只保证
//! "两边都在时一定一致"，覆盖面由 `pairs` 下限兜底 ✓。
use forge_tests::asm::targets::assemble_to_bytes;

/// 族 + 排列：新拼法（排列写在寄存器后缀上）对 助记符后缀拼法（排列写在助记符上）。
fn pairs() -> Vec<(String, String)> {
    let arrs = ["8b", "16b", "4h", "8h", "2s", "4s"];
    let fams = [
        "add", "sub", "and", "orr", "eor", "bic", "orn", "smax", "smin", "umax", "umin", "mul", "mla",
    ];
    let mut out = Vec::new();
    for fam in fams {
        for arr in arrs {
            out.push((
                format!("{fam} v0.{arr}, v1.{arr}, v2.{arr}"),
                format!("{fam}.{arr} v0, v1, v2"),
            ));
        }
    }
    out
}

#[test]
fn neon_new_spelling_matches_mnemonic_suffix_siblings() {
    let mut compared = 0usize;
    let mut skipped = 0usize;
    let mut bad: Vec<String> = Vec::new();
    for (new_spelling, old_spelling) in pairs() {
        let a = assemble_to_bytes("aarch64", &new_spelling);
        let b = assemble_to_bytes("aarch64", &old_spelling);
        match (a, b) {
            (Ok(a), Ok(b)) => {
                compared += 1;
                if a != b {
                    bad.push(format!("{new_spelling} -> {a:02x?} ≠ {old_spelling} -> {b:02x?}"));
                }
            }
            // 一侧未实现 ⇒ 记账跳过（本测试只判"两边都在时是否一致"）。
            _ => skipped += 1,
        }
    }
    assert!(
        bad.is_empty(),
        "新拼法与助记符后缀兄弟的字节不一致（{} 对）：\n{}",
        bad.len(),
        bad.join("\n")
    );
    assert!(
        compared >= 6,
        "一致性对太少（只比了 {compared} 对、跳过 {skipped} 对）——谱面可能大面积缺失"
    );
    eprintln!("ASM-ARR-PAIRS: compared={compared} skipped={skipped}");
}