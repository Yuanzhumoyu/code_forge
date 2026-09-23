//! lint 快照守卫（v19 V4a）：三份**发行谱**必须零结论。
//!
//! "零误报"是 V4 的硬判据（计划 §5）：这份快照把当时的结论集固定在 **0**——任何新规则一旦
//! 在真实谱上冒结论，这个测试立刻红，逼作者先人工核对是"真阳性"还是"误报"。
//!
//! 真阳性出现时的正确动作是**修谱**：V4a 落地时 x86 就报出一个没人引用的 `gpr32` 槽
//! （`LINT-UNUSED-SLOT`），核对后删掉了它（删死槽不改变任何指令行为——`cargo test -p
//! forge-codegen --lib` 仍是 1150 passed）。**不要**把快照改成非零来"修"测试。

use std::path::{Path, PathBuf};

use forge_isa_dsl::lint::lint_source;
use forge_isa_dsl::report;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[test]
fn shipped_specs_are_lint_clean() {
    for isa in ["x86_v12.toml", "arm64_v12.toml", "riscv64_v12.toml"] {
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
    let path = root().join("isa/arm64_v12.toml");
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
