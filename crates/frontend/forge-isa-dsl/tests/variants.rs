//! 参数化变体投影的行为守卫（v19 V5）。
//!
//! 判据（计划 §5 V5，G4「一份源谱 → 多个位宽变体」）：
//!
//! 1. **默认档不受影响**：不传 `params` 时不过滤、不替换——同一份谱的指令清单
//!    与"没有变体机制"时逐项相同（这里用 `riscv64_v12.toml` 的 116/110 钉住）；
//! 2. **参数必须在 `[meta].variants` 里声明且落在声明域内**（拼错参数名 = 报错，
//!    不静默无事发生——否则用户会以为"投影生效了"）；
//! 3. `only_variants` 命中 ⇒ 声明消失，且**引用了被投影掉指令的 `[[lowering]]`
//!    规则连带消失**；未命中的参数不构成排除；
//! 4. `{参数名}` 在 `asm` 与 lowering 里替换成取值（宽度是数据，不是硬编码）。
//!
//! 数字快照（`riscv64_v12` 的 RV32 投影）变了 ⇒ 有人改了 W 族/帧件的变体标注，
//! 必须人工复核"这是有意的 xlen 依赖吗"，复核后同步更新本文件与计划 §5 的结论。

use std::path::{Path, PathBuf};

use forge_isa_dsl::report;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// 展开一份发行谱（`params` 为空 = 默认档）。
fn insts(
    isa: &str,
    params: &[&str],
) -> (report::IsaSummary, Vec<report::InstRow>, report::Projection) {
    let path = root().join("isa").join(isa);
    let spec = report::load_spec(&path).unwrap_or_else(|d| panic!("加载 {isa} 失败：{d:?}"));
    let list: Vec<String> = params.iter().map(|s| (*s).to_string()).collect();
    let opts = report::RunOpts::with_params(&list).expect("参数形态合法");
    report::insts_projected_loaded(&spec, &opts)
        .unwrap_or_else(|d| panic!("展开 {isa}（params = {params:?}）失败：{d:?}"))
}

/// 一份谱的校验诊断（`params` 为空 = 默认档）。
fn diags(isa: &str, params: &[&str]) -> Vec<report::DiagLine> {
    let path = root().join("isa").join(isa);
    let list: Vec<String> = params.iter().map(|s| (*s).to_string()).collect();
    let opts = report::RunOpts::with_params(&list).expect("参数形态合法");
    report::validate_file_opts(&path, &opts).0
}

/// ① 默认档：三份发行谱不传 `params` 时**零投影**（不丢、不替换）。
#[test]
fn default_has_no_projection() {
    for isa in ["x86_v12.toml", "arm64_v12.toml", "riscv64_v12.toml"] {
        let (isa_sum, rows, p) = insts(isa, &[]);
        assert!(
            p.params.is_empty(),
            "{isa} 默认档不该有参数：{:?}",
            p.params
        );
        assert!(p.dropped_insts.is_empty(), "{isa} 默认档不该丢指令");
        assert!(p.dropped_decls.is_empty(), "{isa} 默认档不该丢声明");
        assert_eq!(p.inst_count, rows.len(), "{isa} 报告条数须与清单一致");
        assert_eq!(
            isa_sum.instructions,
            rows.len(),
            "{isa} 摘要条数须与清单一致"
        );
    }
}

/// ② 参数声明域：未声明 / 越界都必须在**校验期**报错（不静默）。
#[test]
fn undeclared_or_out_of_domain_param_is_an_error() {
    let undeclared = diags("riscv64_v12.toml", &["nope=1"]);
    assert!(
        undeclared
            .iter()
            .any(|d| d.code == "DSL-META" && d.msg.contains("nope") && d.msg.contains("没有声明")),
        "未声明参数必须报 DSL-META 并点名参数：{undeclared:?}"
    );
    let out_of_domain = diags("riscv64_v12.toml", &["xlen=128"]);
    assert!(
        out_of_domain
            .iter()
            .any(|d| d.msg.contains("xlen = 128") && d.msg.contains("不在声明域")),
        "越界取值必须报错并给声明域：{out_of_domain:?}"
    );
    // 没有变体机制的谱（x86/arm64）传参同样 fail-closed。
    let none_declared = diags("x86_v12.toml", &["xlen=32"]);
    assert!(
        none_declared
            .iter()
            .any(|d| d.msg.contains("没有声明任何变体参数")),
        "无变体谱传参必须 fail-closed：{none_declared:?}"
    );
}

/// ③ 发行谱投影账目快照：RV32 视角 = 116 → 104 条指令，且连带丢 14 条 lowering。
#[test]
fn riscv_rv32_projection_snapshot() {
    let (_, rows, p) = insts("riscv64_v12.toml", &["xlen=32"]);
    assert_eq!(p.dropped_insts.len(), 12, "丢掉 12 条 RV64 专属指令");
    for name in [
        "LD",
        "SD", // 64 位访存（帧/ABI 结构件的引用虽被整节丢掉，指令本身仍按 xlen 过滤）
        "ADDW", "SUBW", "MULW", "DIVW", "DIVUW", "REMW", "REMUW", // W 族 R 型
        "SLLW", "SRLW", "SRAW", // W 族移位
    ] {
        assert!(
            p.dropped_insts.contains(&name.to_string()),
            "投影必须丢掉 {name}：{:?}",
            p.dropped_insts
        );
        assert!(
            !rows.iter().any(|r| r.name == name),
            "投影后清单里不该有 {name}"
        );
    }
    assert_eq!(p.inst_count, 104, "RV32 投影剩 104 条");
    assert_eq!(rows.len(), 104);
    assert_eq!(p.lowering_count, 96, "RV32 投影的 lowering 剩 96 条");
    let dropped: usize = p.dropped_decls.iter().map(|(_, n)| n).sum();
    assert_eq!(dropped, 17, "逐节丢弃合计 17 项：{:#?}", p.dropped_decls);
    for what in [
        "[emit.prologue]",
        "[emit.epilogue]",
        "[spill.GPR]",
        "[[lowering]]",
    ] {
        assert!(
            p.dropped_decls.iter().any(|(w, _)| w == what),
            "必须报到 {what}：{:#?}",
            p.dropped_decls
        );
    }
    // 默认档对照：同一份谱不传参数 = 116/110（投影是纯 opt-in）。
    let (_, def_rows, def_p) = insts("riscv64_v12.toml", &[]);
    assert_eq!(def_rows.len(), 116);
    assert_eq!(def_p.lowering_count, 110);
}

/// ④ 级联的**边界**：未被投影的指令其 lowering 规则必须留下（不许多丢）。
#[test]
fn cascade_keeps_unrelated_lowering() {
    let (_, _, p) = insts("riscv64_v12.toml", &["xlen=32"]);
    // RV64 的 64 位 Iadd/Isub/Imul/Load/Store 与帧件的通用规则都不该因投影消失过头：
    // 110 - 96 = 14 条全是"点了被投影掉的引用名"的规则。
    assert_eq!(
        110 - p.lowering_count,
        14,
        "只该丢那 14 条：{:#?}",
        p.dropped_decls
    );
}

/// ⑤ 只对**调用方传了的参数**做排除：传 `xlen=64` 时 W 族指令照常在。
#[test]
fn only_supplied_params_gate() {
    let (_, rows, p) = insts("riscv64_v12.toml", &["xlen=64"]);
    assert_eq!(rows.len(), 116, "xlen=64 是原生视角，一条都不该丢");
    assert!(p.dropped_insts.is_empty());
    assert_eq!(p.lowering_count, 110);
}

/// ⑥ `{参数名}` 替换：`asm` 与 `[[lowering]].insts` 里的 `{参数名}` 换成本次取值。
///
/// 用一个**最小合成谱**验证替换语义本身（发行谱的 `asm` 目前不含参数占位符）。
#[test]
fn param_substitution_in_asm_and_lowering() {
    let src = r#"
[meta]
name = "vtoy"
variants = { width = [16, 32] }
endian = "little"
mode = 16

[encoding]
kind = "fixed"
bits = 16

[reg.gpr1]
names = ["R0", "R1", "R2", "R3"]

[conventions.bitfields]
op  = { offset = 12, width = 4 }
rd  = { offset = 8, width = 3 }
rs1 = { offset = 5, width = 3 }

[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr1"

[[forms]]
name = "RR"
opcode_field = "op"
operand_fields = ["rd", "rs1"]

[[instructions]]
name = "ADD"
form = "RR"
opcode = 1
ops = ["dst:g:out", "src:g"]
asm = "add{width} {dst}, {src}"
effect = ["Pure"]

[[lowering]]
op = "Iadd"
insts = ["ADD {out}, {0}"]
"#;
    // 默认档：参数占位符**不许留下**（生成的汇编会打印出字面 `{width}`）——
    // 报出可操作的错误，点名这是变体参数与怎么传值。
    let err = report::insts_projected_opts(src, &report::RunOpts::default())
        .expect_err("默认档的 asm 留着参数占位符必须报错");
    let msg = err
        .iter()
        .map(|d| d.msg.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        msg.contains("变体参数") && msg.contains("width") && msg.contains("--params"),
        "默认档的报错必须点名参数与传法：{msg}"
    );
    // 传了 `width=32` ⇒ 文本被替换成取值（宽度是数据，不是硬编码）。
    // 报告里的 `asm` 是**规范化后**的形态（命名是作者面语法，下游只见 `{序号}`）。
    let opts = report::RunOpts::with_params(&["width=32".to_string()]).expect("形态合法");
    let (_, rows32, p32) = report::insts_projected_opts(src, &opts).expect("投影合法");
    assert_eq!(rows32[0].asm, "add32 {0}, {1}");
    assert_eq!(p32.params.get("width"), Some(&32));
    assert!(
        p32.dropped_insts.is_empty(),
        "没标 only_variants ⇒ 一条都不丢"
    );
}

/// ⑦ `only_variants` 的声明一致性：未声明的参数名 / 越界取值都必须在**校验期**报错。
///
/// 不做这条的后果：标了 `only_variants` 却永远不生效（参数根本没声明 ⇒ 永远不会传进来），
/// 作者会以为"变体机制没起作用"而查错方向。
#[test]
fn gate_must_reference_declared_params() {
    let base = |gate: &str, meta_variants: &str| {
        format!(
            r#"
[meta]
name = "vtoy"
{meta_variants}
endian = "little"
mode = 16

[encoding]
kind = "fixed"
bits = 16

[reg.gpr1]
names = ["R0", "R1"]

[conventions.bitfields]
op = {{ offset = 12, width = 4 }}
rd = {{ offset = 8, width = 3 }}

[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr1"

[[forms]]
name = "RR"
opcode_field = "op"
operand_fields = ["rd"]

[[instructions]]
name = "ADD"
form = "RR"
opcode = 1
ops = ["dst:g:out"]
asm = "add {{dst}}"
effect = ["Pure"]
{gate}
"#
        )
    };
    let err_of = |src: &str| {
        report::validate_opts(src, &report::RunOpts::default())
            .iter()
            .map(|d| format!("{}: {}", d.code, d.msg))
            .collect::<Vec<_>>()
            .join("\n")
    };
    // 未声明的参数名。
    let e = err_of(&base("only_variants = { nope = [64] }", ""));
    assert!(
        e.contains("DSL-META") && e.contains("nope") && e.contains("未声明"),
        "未声明参数必须报错：{e}"
    );
    // 取值超出声明域。
    let e = err_of(&base(
        "only_variants = { xlen = [128] }",
        "variants = { xlen = [32, 64] }",
    ));
    assert!(
        e.contains("128") && e.contains("不在声明域"),
        "越界取值必须报错：{e}"
    );
    // 合法的标记：默认档零诊断（`only_variants` 只在不传参数时"不排除"）。
    let ok = base(
        "only_variants = { xlen = [64] }",
        "variants = { xlen = [32, 64] }",
    );
    assert!(
        report::validate_opts(&ok, &report::RunOpts::default()).is_empty(),
        "合法标记不该有诊断"
    );
    // `[meta].variants` 自身：空取值域 = 任何取值都不合法，声明处就要拒。
    let e = err_of(&base("", "variants = { xlen = [] }"));
    assert!(e.contains("取值域为空"), "空取值域必须报错：{e}");
}
