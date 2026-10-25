//! `validate --strict-overlap` 的**清单快照**（v19 V6b）。
//!
//! 判据与结论（2026-09-23 实测，计划 §5 V6b）：
//!
//! - 三份发行谱的"同 op 两条规则取值域相交、但互不包含"共 **54** 条
//!   （x86 32 / arm64 8 / riscv64 **20**），抽查全部是**故意的"特化 + 兜底"**
//!   （例：riscv `Iadd` 的 `{rs1_width=32}` 特化规则 + 无 `when` 的通用规则；
//!   2026-10-01 riscv 21 → 23：补 `Fload`/`Fstore` 的 lowering 各多一对"单/双精度"规则，
//!   与既有 `Load`/`Store`/`Fcmp` 同一形态；同日 riscv 23 → 27：补浮点算术
//!   `Fadd`/`Fsub`/`Fmul`/`Fdiv`，同样是"{rs1_width=32} 特化 + 无 when 兜底"这一对形状，
//!   与 `Iadd`/`Fptosi`/`Fload` 完全同款——**不是** when 写窄）；
//! - 因此**不能**把它当硬错误/默认开（那会把 63 条合法写法变成错误），
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
    let opts = report::RunOpts {
        strict_overlap: strict,
        ..Default::default()
    };
    let (diags, _) = report::validate_file_opts(&path, &opts);
    diags.iter().filter(|d| d.code == "DSL-OVERLAP").count()
}

#[test]
fn strict_overlap_inventory_matches_snapshot() {
    // arm64 2 → 0（2026-10-24，v27 M6-98）：`Iconst` 的 6 条规则去掉 `rd` 谓词、
    // 按值域单一分派（16 位内 MOVN/MOVZ + 域外 movz/movk 全片），原先
    // "rd=64 特化 + rd 无关兜底" 的两处部分重叠随之消失——不是 when 写窄。
    // arm64 0 → 9（2026-10-24，v27 M6-99）：`Sextend`/`Uextend`/`Select`/`Band`/
    // `Bor`/`Bxor` 新增"窄宽度特化 + 无 when 兜底"成对规则（与 riscv 的
    // `{rs1_width=32} + 兜底` 同款合法写法；兜底覆盖的是未显式列出的窄宽度）。
    // arm64 9 → 16（2026-10-25，v27 M6-100）：`Ishl`/`Ushr`/`Sshr`/`Sdiv`/`Udiv`/
    // `Srem`/`Urem` 各按 64/其余 成对（同样"特化 + 兜底"）。`Fma`/`Fceil`/`Ffloor`/`Fconst`\n    // 又各多一对（M6-102）⇒ 9 → 16 → 18。
    for (isa, want) in [("x86.toml", 32), ("arm64.toml", 18), ("riscv64.toml", 17)] {
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
    for isa in ["x86.toml", "arm64.toml", "riscv64.toml"] {
        assert_eq!(
            overlaps(isa, false),
            0,
            "{isa} 默认档不该出现 DSL-OVERLAP（严格档必须纯 opt-in）"
        );
    }
}
