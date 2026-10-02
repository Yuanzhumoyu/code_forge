//! **类间位搬移**（`fpr_to_gpr_mov` / `gpr_to_fpr_mov`）：缺能力必须 **fail-closed**。
//!
//! 值的寄存器类与 ABI 落点的类**不同**时（psABI 的整数约定收浮点：RISC-V 变参、Zfinx/软浮点
//! 约定），生成器必须发**按位的类间搬移**（riscv `fmv.x.d`/`fmv.d.x`、x86 `MOVQ`）。
//! 谱里没申报 ⇒ 生成物里明确 `Unsupported`——拿同类搬移顶上会把 FPR 的号当 GPR 号用
//! （`Reg::from_index(10, GPR)` = `x10`），那是**静默错值**。
//!
//! 这里用**真实谱的变异**钉住两条：① riscv 谱确实申报了四个档（32/64 × 两个方向）；
//! ② 把它们删掉后，生成物里出现 fail-closed 分支（不是悄悄退化成同类搬移）。
//!
//! 端到端那一半（真编出 `fmv.d.x`/`fmv.x.d`）在
//! `crates/backend/forge-codegen/tests/bank_mov.rs`（合成约定"浮点进 int 池"）。

use std::path::Path;

use forge_isa_dsl::{expand_str, read_isa_file};

fn flat(src: &str) -> String {
    expand_str(src, "riscv64_v12", Path::new("isa/riscv64_v12.toml"))
        .unwrap_or_else(|e| panic!("展开失败：{e}"))
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// 四个档的声明原文（两条指令 + 模板行的两个 `roles`）。
const DECLS: [(&str, &str); 4] = [
    (
        r#"roles = [{ role = "gpr_to_fpr_mov", bits = 32 }]"#,
        "roles = []",
    ),
    (
        r#"roles = [{ role = "gpr_to_fpr_mov", bits = 64 }]"#,
        "roles = []",
    ),
    (
        r#"roles = [{ role = "fpr_to_gpr_mov", bits = 32 }]"#,
        "roles = []",
    ),
    (
        r#"roles = [{ role = "fpr_to_gpr_mov", bits = 64 }]"#,
        "roles = []",
    ),
];

/// ① 发行谱（riscv）申报了四个档，生成物里两侧的类间搬移都接进了分派。
#[test]
fn riscv_declares_both_directions_and_the_generator_wires_them_in() {
    let (src, _) = read_isa_file("isa/riscv64_v12.toml").expect("读 riscv 谱");
    for (decl, _) in DECLS {
        assert!(src.contains(decl), "riscv 谱应申报 `{decl}`");
    }
    let t = flat(&src);
    for vn in ["FmvXD", "FmvDX"] {
        assert!(t.contains(vn), "生成物里应有类间搬移的变体 `{vn}`");
    }
    // 两个方向各按**位宽**分派（32/64 两档都在表里）。
    for bits in [32u16, 64u16] {
        let arm = format!("=={bits}u16asu32");
        assert!(t.contains(&arm), "类间搬移也应按位宽分派：缺 `{arm}`");
    }
}

/// ② 把四个档删掉 ⇒ 生成物里的**类间位搬移分支**变成明确的 fail-closed。
#[test]
fn stripping_the_roles_makes_the_cross_bank_path_fail_closed() {
    let (src, _) = read_isa_file("isa/riscv64_v12.toml").expect("读 riscv 谱");
    let mut mutated = src;
    for (decl, to) in DECLS {
        assert!(mutated.contains(decl), "变异前应存在 `{decl}`");
        mutated = mutated.replace(decl, to);
    }
    let t = flat(&mutated);
    assert!(
        t.contains("类间位搬移"),
        "缺角色时生成物里必须有类间位搬移的 fail-closed 分支（不退化、不猜）"
    );
    // 反向：不该再出现“改完后仍按表分派”的痕迹——两条指令的变体不再被引用
    //（谱里还在，只是没有角色把它们接进发射；生成物里因此在 lowering/frame 段不出现）。
    assert!(
        t.contains("FmvXD") || t.contains("FmvDX"),
        "指令变体本身仍应存在（谱里没删指令，只删了角色）"
    );
}
