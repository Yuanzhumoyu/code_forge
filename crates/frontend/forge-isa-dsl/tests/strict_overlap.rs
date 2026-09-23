//! `validate --strict-overlap` 的**清单快照**（v19 V6b）。
//!
//! 判据与结论（2026-09-23 实测，计划 §5 V6b）：
//!
//! - 三份发行谱的"同 op 两条规则取值域相交、但互不包含"共 **61** 条
//!   （x86 32 / arm64 8 / riscv64 21），抽查全部是**故意的"特化 + 兜底"**
//!   （例：riscv `Iadd` 的 `{rs1_width=32}` 特化规则 + 无 `when` 的通用规则）；
//! - 因此**不能**把它当硬错误/默认开（那会把 61 条合法写法变成错误），
//!   只作为 `--strict-overlap` 的**评审清单**；CI 不开这个档。
//! - 本文件把清单**钉成数字**：数字变了就说明有人改了 lowering 的取值域形状，
//!   必须人工复核是"新增的合法特化"还是"when 写窄/priority 乱用"。
//!
//! 注意：**默认档必须仍然零结论**（严格档是纯 opt-in，不得影响历史行为）。

use std::path::{Path, PathBuf};

use forge_isa_dsl::report;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn overlaps(isa: &str, strict: bool) -> usize {
    let path = root().join("isa").join(isa);
    let (diags, _) = report::validate_file_opts(&path, strict);
    diags.iter().filter(|d| d.code == "DSL-OVERLAP").count()
}

#[test]
fn strict_overlap_inventory_matches_snapshot() {
    for (isa, want) in [
        ("x86_v12.toml", 32),
        ("arm64_v12.toml", 8),
        ("riscv64_v12.toml", 21),
    ] {
        assert_eq!(
            overlaps(isa, true),
            want,
            "{isa} 的部分重叠条数变了——请人工复核（新增的合法特化？还是 when 写窄/priority 乱用？），\
             复核后同步更新本快照数字与计划 §5 的结论"
        );
    }
}

#[test]
fn strict_overlap_is_opt_in_only() {
    for isa in ["x86_v12.toml", "arm64_v12.toml", "riscv64_v12.toml"] {
        assert_eq!(
            overlaps(isa, false),
            0,
            "{isa} 默认档不该出现 DSL-OVERLAP（严格档必须纯 opt-in）"
        );
    }
}
