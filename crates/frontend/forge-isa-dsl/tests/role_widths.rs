//! S9 结构不变量：**角色宽度是数据**。
//!
//! v18 S9 之前，宽度被编进角色名（`fpr_mov_f32`/`fpr_mov_f64`、`wide_vec_load_32`/`_64`），
//! 别的位宽的 ISA 无法接入。现在宽度写在角色声明里
//! （`roles = [{ role = "fpr_mov", bits = 32 }]`），因此：
//!
//! 1. **任意位宽都合法**（没有白名单/上限）——16/24 位也照样通过校验；
//! 2. **(角色, 位宽) 必须唯一**——同一角色的同一宽度声明两次必须在**编译期**报错，
//!    且错误里要点出角色名与冲突的两条指令（否则生成器只会静默取第一条）。
//!
//! 用真实谱做变异（改 `bits` 数值）而不是另写一份夹具：这样同时钉住
//! "`isa/x86_v12.toml` 里 fpr_mov 的宽度就是数据"这一事实。

use std::path::Path;

use forge_isa_dsl::{read_isa_file, validate_source};

fn x86() -> String {
    read_isa_file("isa/x86_v12.toml").expect("读 x86 谱失败").0
}

#[test]
fn arbitrary_role_widths_are_accepted() {
    // fpr_mov 的两条声明改成 16/24 位（非 32/64）——宽度无白名单，必须照样合法。
    let src = x86()
        .replace("bits = 32 }]", "bits = 16 }]")
        .replace("bits = 64 }]", "bits = 24 }]");
    assert!(
        src.contains("bits = 16 }]") && src.contains("bits = 24 }]"),
        "变异没生效"
    );
    validate_source(&src, Path::new("x86_role_bits_any.toml"))
        .unwrap_or_else(|e| panic!("任意位宽都应合法，却报了：{e:#?}"));
}

#[test]
fn duplicate_role_width_is_rejected() {
    // 把 32 改成 64 ⇒ MOVSS 与 MOVSD 撞成 (fpr_mov, 64)，必须在编译期拒绝。
    let src = x86().replace("bits = 32 }]", "bits = 64 }]");
    let errs = validate_source(&src, Path::new("x86_role_dup.toml"))
        .expect_err("同角色同宽度重复必须报错");
    let joined = errs.join("\n");
    for needle in ["fpr_mov", "MOVSS"] {
        assert!(
            joined.contains(needle),
            "错误应点出 `{needle}`（角色名 / 冲突指令名）：{joined}"
        );
    }
}
