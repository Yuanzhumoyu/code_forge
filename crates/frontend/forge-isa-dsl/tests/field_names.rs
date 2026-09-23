//! 生成字段名 = **DSL 声明的操作数名**（v18 S7D-fix）。
//!
//! 背景：历史实现里 `ops = ["dst:r:out", "src:r"]` 的 `dst`/`src` **只**用于 asm 模板，
//! `Inst` 变体的字段名却另取一套——定宽 ISA 取位域名（`[forms].operand_fields` 的
//! `rd`/`rs1`）、变长 ISA 取语义角色名（`dest`/`src`）。作者写的名字在用户面
//! 完全没有意义（`Inst::Iadd { rd, rs1 }`），且位域名泄漏成 API。
//!
//! 现在：**Rust 字段名 = `ops` 里的名字**；位域名 / 语义角色名退回纯粹的内部
//! **编码键**（查 `[conventions.bitfields]`、modrm 角色、立即数编码表）。
//! 断言对象是生成出来的 token 文本（去空白后匹配），因此"退化成位置名 op0/op1"
//! 之类的回潮会直接失败。

use forge_isa_dsl::{ExpandOptions, Parts, expand_file, expand_str};

fn flat(ts: &proc_macro2::TokenStream) -> String {
    ts.to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// 最小定宽 ISA：位域名（`f0`/`f1`）与操作数声明名（`dst`/`src`）**故意不同**。
fn fixed_spec(ops: &str, asm: &str) -> String {
    format!(
        r#"
[meta]
name = "names_demo"
[encoding]
kind = "fixed"
bits = 16
[reg.gpr4]
count = 4
[conventions.bitfields]
op = {{ offset = 12, width = 4 }}
f0 = {{ offset = 8, width = 4 }}
f1 = {{ offset = 4, width = 4 }}
[[operand_slots]]
name = "g"
kind = "reg"
class = "gpr4"
[[forms]]
name = "RR"
opcode_field = "op"
operand_fields = ["f0", "f1"]
[[instructions]]
name = "ADD"
form = "RR"
opcode = 1
ops = {ops}
asm = "{asm}"
"#
    )
}

fn expand_inline(src: &str) -> String {
    flat(&expand_str(src, "names_demo", std::path::Path::new("names_demo.toml")).expect("展开"))
}

/// 定宽 ISA：字段名取自 `ops`（`dst`/`src`），**不是**位域名（`f0`/`f1`）。
#[test]
fn fixed_isa_fields_use_declared_names_not_bitfields() {
    let t = expand_inline(&fixed_spec(r#"["dst:g:out", "src:g"]"#, "add {dst}, {src}"));
    assert!(
        t.contains("Add{dst:Reg,src:Reg}"),
        "字段名应来自 ops 声明：{}",
        &t[t.find("pubenumInst").unwrap_or(0)..][..200.min(t.len())]
    );
    assert!(!t.contains("f0:Reg"), "位域名不得当字段名");
    assert!(!t.contains("f1:Reg"), "位域名不得当字段名");
}

/// 声明名是 Rust 关键字 → 原始标识符（`r#type`），不静默改名。
#[test]
fn keyword_operand_names_become_raw_idents() {
    let t = expand_inline(&fixed_spec(
        r#"["type:g:out", "match:g"]"#,
        "add {type}, {match}",
    ));
    assert!(t.contains("Add{r#type:Reg,r#match:Reg}"), "关键字应原始化");
}

/// 声明名以数字开头 → 前缀 `_`（仍是作者的名字，不是 op0/op1）。
#[test]
fn digit_leading_operand_name_is_prefixed() {
    let t = expand_inline(&fixed_spec(
        r#"["8bit:g:out", "src:g"]"#,
        "add {8bit}, {src}",
    ));
    assert!(t.contains("Add{_8bit:Reg,src:Reg}"), "数字开头应加前缀");
}

/// 变长 ISA（x86）同样按 `ops` 声明名：`MOV_RM_R` 的 `ops = ["src:gpr", "dst:gpr:inout"]`
/// ⇒ 字段 `src`/`dst`（历史实现是语义名 `dest`，与声明不符）。
#[test]
fn vlen_isa_fields_use_declared_names() {
    let opts = ExpandOptions {
        spec_tests: false,
        name: None,
        parts: Parts::all(),
    };
    let t = flat(&expand_file("isa/x86_v12.toml", &opts).expect("展开 x86"));
    assert!(
        t.contains("MovRmR{src:Reg,dst:Reg}"),
        "x86 字段应按 ops 声明命名"
    );
    assert!(!t.contains("MovRmR{dest:"), "旧的语义名 dest 不得再出现");
    // 条件码操作数：`ops = [..., "cc:cc"]` ⇒ 字段 `cc`（历史名 `cond`）。
    assert!(
        t.contains("JccRel32{cc:u8,target:i64}"),
        "条件码字段名 = 声明名"
    );
}
