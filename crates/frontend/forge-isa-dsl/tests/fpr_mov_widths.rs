//! 浮点搬移**按位宽查表**，不是"写死 32/64"（v20 V6+）。
//!
//! 背景：`fpr_mov` 是"把浮点值从一个寄存器搬到另一个"的能力角色。历史上生成器只认
//! `f32`/`f64` 两档（`[abi].fpr_mov_inst32` / `fpr_mov_inst`），于是：
//!
//! - 定宽 ISA 若只有 **128 位**（整寄存器搬移）或 **16 位** 的搬移指令，就没有落点；
//! - riscv 谱里曾经只有单精度 `fsgnj.s`，`f64` 的参数/返回/变参分支**全部编不出来**——
//!   这不是"riscv 的洞"，是"宽度被写死"的必然结果。
//!
//! 现在 `roles = [{ role = "fpr_mov", bits = N }]`（任意 N ≥ 1）由 ISA 声明，生成器
//! **按结果位宽查表发指令**，未声明的位宽在生成物里明确 `Unsupported`（列出已声明档）。
//!
//! 断言对象是**真实谱展开后的 token 文本**（去空白后匹配）：把宽度当数据这一条一旦
//! 回潮成 `if size == 4 { MOVSS } else { MOVSD }`，下面的断言直接变红。

use std::path::Path;

use forge_isa_dsl::{expand_str, read_isa_file};

fn flat_file(path: &str, mod_name: &str) -> (String, String) {
    let (src, _) = read_isa_file(path).unwrap_or_else(|e| panic!("读 {path} 失败：{e}"));
    let t: String = expand_str(&src, mod_name, Path::new(path))
        .unwrap_or_else(|e| panic!("展开 {path} 失败：{e}"))
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    (src, t)
}

fn flat_inline(src: &str, name: &str) -> String {
    expand_str(src, name, Path::new("fprdemo.toml"))
        .unwrap_or_else(|e| panic!("展开失败：{e}"))
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// 取 `t` 中下标 `at` 前后各若干字节（对齐到字符边界，仅用于失败时的诊断打印）。
fn win(t: &str, at: usize, before: usize, after: usize) -> &str {
    let mut lo = at.saturating_sub(before);
    while lo < t.len() && !t.is_char_boundary(lo) {
        lo += 1;
    }
    let mut hi = (at + after).min(t.len());
    while hi > lo && !t.is_char_boundary(hi) {
        hi -= 1;
    }
    &t[lo..hi]
}

/// 分派臂的文本形状：`if (<位宽表达式>) as u32 == <N>u16 as u32 { <构造> }`。
/// `quote` 把 `u16` 渲染成 `<N>u16`，去空白后即 `<N>u16asu32`。
fn arm(bits: u32) -> String {
    format!("=={bits}u16asu32")
}

/// ① **位宽是谱的数据**：把 x86 的 `fpr_mov` 两档从 32/64 改成 16/128，生成器必须跟着走。
///
/// 这条同时覆盖两处生成点——降低侧（`lowering.rs`：返回/实参/取值）与收参侧
/// （`frame.rs`：`move_args`）——所以同一档位宽常量应出现**多次**。
#[test]
fn x86_fpr_mov_widths_follow_the_spec() {
    let (src, _) = read_isa_file("isa/x86_v12.toml").expect("读 x86 谱");
    let mutated = src
        .replacen(
            r#"roles = [{ role = "fpr_mov", bits = 32 }]"#,
            r#"roles = [{ role = "fpr_mov", bits = 16 }]"#,
            1,
        )
        .replacen(
            r#"roles = [{ role = "fpr_mov", bits = 64 }]"#,
            r#"roles = [{ role = "fpr_mov", bits = 128 }]"#,
            1,
        );
    assert!(
        mutated.contains("bits = 16 }]") && mutated.contains("bits = 128 }]"),
        "变异没生效"
    );
    let t: String = expand_str(&mutated, "x86_v12", Path::new("isa/x86_v12.toml"))
        .expect("改了位宽也该照样展开")
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    // 两档都在，且各自指向**它自己**那条指令（16 位档 = MOVSS，128 位档 = MOVSD）。
    for (bits, vn) in [(16u32, "Movss"), (128u32, "Movsd")] {
        let arm = arm(bits);
        let at = t
            .find(&arm)
            .unwrap_or_else(|| panic!("缺少 {bits} 位的分派臂 `{arm}`"));
        assert!(
            win(&t, at, 0, 260).contains(vn),
            "`{arm}` 的臂应发 `{vn}`：{}",
            win(&t, at, 0, 260)
        );
    }
    // 写死 32/64 就会留下这两个臂——一个都不许有。
    for hard in [32u32, 64u32] {
        let a = arm(hard);
        assert!(!t.contains(&a), "位宽是谱的数据，不该出现写死的 `{a}`");
    }
    // 未声明的位宽 ⇒ 生成物里的明确失败（列出已声明档），不是静默回退。
    assert!(
        t.contains("已声明位宽：16/128"),
        "未声明的位宽必须 fail-closed 且列出已声明档"
    );
    // 两处生成点（降低侧 + 收参侧）都按表发指令，不是只改一处。
    assert!(
        t.matches(&arm(128)).count() >= 2,
        "128 位档应在降低侧与收参侧各出现一次（实测 {}）",
        t.matches(&arm(128)).count()
    );
}

/// ② 三操作数搬移：**第三槽自动填成源**（`fsgnj rd, rs, rs` 就是寄存器搬移）。
///
/// riscv 的 `FSGNJ_S`/`FSGNJ_D` 正是这个形状——三槽、且第三槽 = 源。这条同时钉住
/// riscv 谱里"双精度档真的申报了 `fpr_mov`"（缺了它 f64 的参数/返回全编不出来）。
#[test]
fn riscv_three_operand_move_fills_the_third_slot() {
    let (src, t) = flat_file("isa/riscv64_v12.toml", "riscv64_v12");
    // 谱侧：单/双精度两个档都在。
    for needle in [
        r#"roles = [{ role = "fpr_mov", bits = 32 }]"#,
        r#"roles = [{ role = "fpr_mov", bits = 64 }]"#,
    ] {
        assert!(src.contains(needle), "riscv 谱应申报 `{needle}`");
    }
    // 生成侧：两档分派都在。
    assert!(
        t.contains(&arm(32)) && t.contains(&arm(64)),
        "两档都应有分派臂"
    );
    // 收参侧（`frame.rs`）：结构体字面量里第三槽 = 源寄存器。
    assert!(
        t.contains("src2:__src"),
        "收参侧应把第三槽填成源（`src2: __src`）"
    );
    // 降低侧（`lowering.rs`）：同一个值也被填进槽 2。
    assert!(
        t.contains("2u8,false"),
        "降低侧应把源值也填进槽 2（`map_reg_field(…, 2u8, false)`）"
    );
}

// ───────────────── ③/④ 谱自相矛盾 ⇒ 生成期报错（不猜、不静默取一条）─────────────────

/// 最小定宽 ISA：GPR + FPR 两族、两条 form（2 操作数 / 3 操作数）。
/// `movs` 处填 `[[instructions]]` 声明块。
fn spec_with(movs: &str) -> String {
    format!(
        r#"
[meta]
name = "fprdemo"
[encoding]
kind = "fixed"
bits = 16
[reg.gpr4]
count = 4
[reg.fpr4]
count = 4
[conventions.bitfields]
op  = {{ offset = 12, width = 4 }}
rd  = {{ offset = 8,  width = 4 }}
rs1 = {{ offset = 4,  width = 4 }}
rs2 = {{ offset = 0,  width = 4 }}
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr4"
[[operand_slots]]
name = "f"
kind = "reg"
class = "fpr4"
[[forms]]
name = "RR"
opcode_field = "op"
operand_fields = ["rd", "rs1"]
[[forms]]
name = "RR3"
opcode_field = "op"
operand_fields = ["rd", "rs1", "rs2"]
{movs}
"#
    )
}

fn instr(name: &str, form: &str, opcode: u32, ops: &str, asm: &str, bits: u32) -> String {
    format!(
        r#"
[[instructions]]
name = "{name}"
form = "{form}"
opcode = {opcode}
ops = [{ops}]
asm = "{asm}"
roles = [{{ role = "fpr_mov", bits = {bits} }}]
"#
    )
}

fn two_op(name: &str, opcode: u32, bits: u32) -> String {
    instr(
        name,
        "RR",
        opcode,
        r#""dst:f:out", "src:f""#,
        "movf {dst}, {src}",
        bits,
    )
}

fn three_op(name: &str, opcode: u32, bits: u32) -> String {
    instr(
        name,
        "RR3",
        opcode,
        r#""dst:f:out", "src:f", "src2:f""#,
        "movf3 {dst}, {src}, {src2}",
        bits,
    )
}

/// ③ 一档两操作数、另一档三操作数 ⇒ **生成期报错**（第三槽在档与档之间不一致）。
#[test]
fn inconsistent_third_slot_is_rejected_at_generation_time() {
    let mut movs = two_op("MOVF32A", 1, 32);
    movs.push_str(&three_op("MOVF64B", 2, 64));
    let err = expand_str(&spec_with(&movs), "fprdemo", Path::new("fprdemo_bad.toml"))
        .expect_err("两操作数与三操作数混用必须报错");
    assert!(err.contains("第三槽"), "错误应点出第三槽不一致：{err}");
}

/// ④ 都是三操作数、但第三槽**字段名不同** ⇒ 同样生成期报错（不按声明序猜）。
#[test]
fn mismatched_third_slot_name_is_rejected_at_generation_time() {
    let mut movs = three_op("MOVF32A", 1, 32);
    movs.push_str(&instr(
        "MOVF64B",
        "RR3",
        2,
        r#""dst:f:out", "src:f", "extra:f""#,
        "movf3 {dst}, {src}, {extra}",
        64,
    ));
    let err = expand_str(&spec_with(&movs), "fprdemo", Path::new("fprdemo_bad2.toml"))
        .expect_err("第三槽字段名不一致必须报错");
    assert!(
        err.contains("第三槽") && err.contains("字段名"),
        "错误应点出第三槽字段名不一致：{err}"
    );
    // 正向对照：同一份最小谱、单一档位，展开必须成功（上面两条错误来自"混用"，不是谱本身编不出）。
    let ok = flat_inline(&spec_with(&two_op("MOVF128", 1, 128)), "fprdemo");
    assert!(ok.contains("Movf128"));
}
