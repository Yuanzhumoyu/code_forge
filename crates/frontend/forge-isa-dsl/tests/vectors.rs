//! `[[vectors]]` 谱内测试向量（v19 V3a）——机制守卫。
//!
//! 分两层钉住：
//!
//! 1. **形态校验**（`validate_source`）：非法组合必须报出可操作的错误（缺 asm/bytes、
//!    正负同给、定宽字长不符、字节越界、`partial` 用错、重复、空子串）；
//! 2. **生成形状**（`expand_str`）：四种形态各自发射出对应断言（token 文本级），
//!    用例名 `spec_vector_<下标>` 与谱里顺序一一对应，条数进 `SPEC_VECTORS`。
//!
//! "字节对不对"的证据不在这里：`isa/riscv64_v12.toml` 的 62 条向量由生成物
//! `__spec_tests::spec_vector_*` 真跑（`cargo test -p forge-codegen --lib`，
//! 迁移前后 903 → 965 passed）。

use std::path::Path;

use forge_isa_dsl::{expand_str, validate_source};

/// 最小可校验的谱骨架：16 位定宽、两条指令（位域 `op{12,4} rd{8,3} rs1{5,3} imm{0,5}`）。
fn spec(vectors: &str) -> String {
    format!(
        r#"
[meta]
name = "vtoy"
endian = "little"
mode = 16

[encoding]
kind = "fixed"
bits = 16

[reg.gpr1]
names = ["R0", "R1", "R2", "R3"]

[conventions.bitfields]
op  = {{ offset = 12, width = 4 }}
rd  = {{ offset = 8, width = 3 }}
rs1 = {{ offset = 5, width = 3 }}
imm = {{ offset = 0, width = 5 }}

[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr1"

[[operand_slots]]
name = "imm5"
kind = "imm"
width = 5
signed = true

[[forms]]
name = "RR"
opcode_field = "op"
operand_fields = ["rd", "rs1"]

[[forms]]
name = "RI"
opcode_field = "op"
operand_fields = ["rd", "imm"]

[[instructions]]
name = "ADD"
form = "RR"
opcode = 1
ops = ["dst:g:out", "src:g"]
asm = "add {{dst}}, {{src}}"
effect = ["Pure"]

[[instructions]]
name = "MOVI"
form = "RI"
opcode = 3
ops = ["dst:g:out", "imm:imm5"]
asm = "movi {{dst}}, {{imm}}"
effect = ["Pure"]
{vectors}
"#
    )
}

/// 去掉所有空白：`TokenStream::to_string()` 会在 token 之间插空格。
fn flat(ts: &proc_macro2::TokenStream) -> String {
    ts.to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// 校验失败的诊断文本（多行拼起来便于 `contains`）。
fn err(vectors: &str) -> String {
    validate_source(&spec(vectors), Path::new("vtoy.toml"))
        .expect_err("这份向量应当校验失败")
        .join("\n")
}

// ── 1. 生成形状：四种形态各一条 ──

#[test]
fn all_four_forms_expand_into_cases() {
    let src = spec(
        r#"
[[vectors]]
asm = "add R1, R2"
bytes = [0x40, 0x11]

[[vectors]]
asm = "movi R0, 999"
error = "立即数"

[[vectors]]
bytes = [0x00, 0x00]
error = "DECODE"
partial = 2

[[vectors]]
bytes = [0x05, 0x33]
"#,
    );
    let out = flat(&expand_str(&src, "vtoy", Path::new("vtoy.toml")).expect("展开"));

    for i in 0..4 {
        assert!(
            out.contains(&format!("fnspec_vector_{i}")),
            "用例名必须按谱里顺序生成（缺 spec_vector_{i}）"
        );
    }
    assert!(
        out.contains("SPEC_VECTORS:usize=4usize"),
        "条数要进 SPEC_VECTORS 常量"
    );

    // 正向：assemble → encode 逐字节比较 + decode 吃满
    assert!(
        out.contains("assert_eq!(got.as_slice(),want") && out.contains("letinst=assemble(text)"),
        "正向向量应发射 assemble→encode 的字节断言"
    );
    // 汇编负向：assemble 或 encode 失败且消息含子串
    assert!(
        out.contains("msg.contains(want)"),
        "汇编负向向量应断言错误消息含子串"
    );
    // 解码负向：decode 必失败 + decode_partial 的消费量（且**只有**这一条用 is_none）
    assert_eq!(
        out.matches("decode(want).is_none()").count(),
        1,
        "解码负向只应有一条（第 3 条）"
    );
    assert!(
        out.contains("decode_partial(want),Err(2)"),
        "`partial = 2` 要钉 decode_partial 的消费量（无后缀字面量）"
    );
    // 解码正向：decode 成功且再编码一致（不是 is_none）
    assert!(
        out.contains("let(inst,used)=decode(want)"),
        "纯 `bytes` 向量应发射解码正向断言"
    );
    // `comment` 缺省时自动生成摘要（进 doc 注释）
    assert!(
        out.contains("谱内向量#0：assemble"),
        "缺省 doc 摘要应由形态自动生成"
    );
}

// ── 2. 形态校验：每条规则一个反例 ──

#[test]
fn malformed_vectors_are_rejected_with_actionable_messages() {
    let cases: &[(&str, &str)] = &[
        ("[[vectors]]\nerror = \"x\"\n", "至少要有"),
        (
            "[[vectors]]\nasm = \"add R1, R2\"\nbytes = [0x40, 0x11]\nerror = \"x\"\n",
            "歧义",
        ),
        (
            "[[vectors]]\nasm = \"add R1, R2\"\nbytes = [0x40]\n",
            "2 字节",
        ),
        ("[[vectors]]\nbytes = [0x40, 0x300]\n", "不是字节"),
        (
            "[[vectors]]\nbytes = [0x40, 0x11]\nerror = \"NOPE\"\n",
            "只接受",
        ),
        (
            "[[vectors]]\nbytes = [0x40, 0x11]\nerror = \"DECODE\"\npartial = 0\n",
            "必须 > 0",
        ),
        (
            "[[vectors]]\nasm = \"add R1, R2\"\nbytes = [0x40, 0x11]\n\n\
             [[vectors]]\nasm = \"add R1, R2\"\nbytes = [0x40, 0x11]\n",
            "完全相同",
        ),
        (
            "[[vectors]]\nasm = \"add R1, R2\"\nerror = \"\"\n",
            "不能是空串",
        ),
        ("[[vectors]]\nbytes = []\n", "不能是空数组"),
        ("[[vectors]]\nasm = \"add R1, R2\"\npartial = 2\n", "只与"),
    ];
    for (vectors, want) in cases {
        let msg = err(vectors);
        assert!(
            msg.contains(want),
            "向量 {vectors:?} 的诊断应含 {want:?}，实际：{msg}"
        );
    }
}

/// 合法形态不该被拦：四种形态 + `comment`（含中文/十六进制字节）都能过校验。
#[test]
fn valid_forms_pass_validation() {
    let ok = spec(
        r#"
# 正向（带备注）
[[vectors]]
comment = "add R1, R2 的字节"
asm = "add R1, R2"
bytes = [0x40, 0x11]

# 汇编负向
[[vectors]]
asm = "movi R0, 999"
error = "立即数"

# 解码负向（钉消费量；**截断输入**也必须合法——那里不查整条字长）
[[vectors]]
bytes = [0x00]
error = "DECODE"
partial = 1

# 解码正向
[[vectors]]
bytes = [0x05, 0x33]
"#,
    );
    validate_source(&ok, Path::new("vtoy.toml")).expect("四种形态都必须合法");
}

/// 没有 `[[vectors]]` 的谱照常（不该新增必填节）。
#[test]
fn specs_without_vectors_still_validate() {
    validate_source(&spec(""), Path::new("vtoy.toml")).expect("空向量区段必须合法");
}
