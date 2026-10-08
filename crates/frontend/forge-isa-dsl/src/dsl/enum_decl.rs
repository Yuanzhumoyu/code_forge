//! v21 W3b **命名取值表**：`[enum.<表名>]` → 内部三处（`conventions.cond` /
//! `conventions.bitsets` / `conventions.imm_names`）。
//!
//! 与 W2/W3a 同一套路（**降级到既有内部结构**，编码器/生成物一行不改）：
//!
//! - `kind = "bits"` ⇒ 位集合表（名字拼接、编码取按位或）；
//! - 条目里出现 `ir` ⇒ **条件码表**（`code` + `ir`；全局只能有一张——内部只有一处 `cond`）；
//! - 否则 ⇒ 命名立即数表（一个名字 = 一个值）。
//!
//! 表是**数据**：DSL 不认识任何具体名字（`iorw`/`mstatus`/`e` 只是某份谱里的一行）。

use std::collections::BTreeMap;

use super::model::{CondEntry, EnumDecl, EnumEntry, EnumKind, IsaModel};

/// 把 `[enum.<表名>]` 降级回内部三处。
pub fn lower_enum_layer(m: &mut IsaModel) -> Result<(), String> {
    if m.enums.is_empty() {
        return Ok(());
    }
    let mut cond: Option<(String, BTreeMap<String, CondEntry>)> = None;
    let mut bitsets: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut imm_names: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    for (name, decl) in &m.enums {
        let kind = decl.kind.unwrap_or(EnumKind::Value);
        let has_ir = decl
            .entries
            .values()
            .any(|e| matches!(e, EnumEntry::Table { ir: Some(_), .. }));
        // 条件码表 = 表名叫 `cond`（约定名）**或**任一条目带 `ir`。两者都要认：
        // 纯别名条目（`eq = 4`）没有 `ir`，只看 `ir` 会把这类表误判成命名立即数表。
        let is_cond = kind == EnumKind::Value && (name == "cond" || has_ir);
        match kind {
            EnumKind::Bits => {
                let mut t = BTreeMap::new();
                for (k, e) in &decl.entries {
                    let v = match e {
                        EnumEntry::Value(v) => *v,
                        EnumEntry::Table { code, .. } => code.ok_or_else(|| {
                            format!("`[enum.{name}].{k}`：bits 表的条目必须给值（`{k} = <位>`）")
                        })?,
                    };
                    if v < 0 {
                        return Err(format!("`[enum.{name}].{k}`：位集合的值不能为负（{v}）"));
                    }
                    t.insert(k.clone(), v as u64);
                }
                bitsets.insert(name.clone(), t);
            }
            EnumKind::Value if is_cond => {
                if let Some((prev, _)) = &cond {
                    return Err(format!(
                        "`[enum.{name}]` 与 `[enum.{prev}]` 都是条件码表（表名叫 `cond` 或条目带 `ir`）——\
                         内部只能有一张（条件码是全 ISA 共享的一张表）"
                    ));
                }
                let mut t = BTreeMap::new();
                for (k, e) in &decl.entries {
                    let (code, ir) = match e {
                        EnumEntry::Value(v) => (*v, None),
                        EnumEntry::Table { code, ir } => (
                            code.ok_or_else(|| {
                                format!("`[enum.{name}].{k}`：条件码条目必须给 `code`")
                            })?,
                            ir.clone(),
                        ),
                    };
                    t.insert(
                        k.clone(),
                        CondEntry {
                            code: code as u64,
                            ir,
                        },
                    );
                }
                cond = Some((name.clone(), t));
            }
            EnumKind::Value => {
                let mut t = BTreeMap::new();
                for (k, e) in &decl.entries {
                    let v = match e {
                        EnumEntry::Value(v) => *v,
                        EnumEntry::Table { code, .. } => code.ok_or_else(|| {
                            format!("`[enum.{name}].{k}`：值表的条目必须给值（`{k} = <值>`）")
                        })?,
                    };
                    t.insert(k.clone(), v);
                }
                imm_names.insert(name.clone(), t);
            }
        }
    }
    if let Some((_, t)) = cond {
        m.conventions.cond = Some(t);
    }
    if !bitsets.is_empty() {
        m.conventions.bitsets = Some(bitsets);
    }
    if !imm_names.is_empty() {
        m.conventions.imm_names = Some(imm_names);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(src: &str) -> IsaModel {
        crate::dsl::parse_and_validate(src).expect("谱合法")
    }

    const BASE: &str = r#"
[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 16
[reg.gpr32]
count = 4
[enum.cond]
o = { code = 0 }
e = { code = 4, ir = "eq" }
[enum.fence]
kind = "bits"
i = 8
w = 1
[enum.csr]
mstatus = 0x300
[operand.g]
kind = "reg"
class = "gpr32"
[operand.fset]
kind = "bits"
bits = 4
enum = "fence"
[operand.csr12]
kind = "imm"
bits = 12
enum = "csr"
[[forms]]
name = "R"
fields = ["u4[15:12]:opcode", "u4[11:8]:rd"]
[[instructions]]
name = "N"
form = "R"
opcode = 1
ops = ["dst:g:out"]
asm = "n {dst}"
"#;

    #[test]
    fn three_table_kinds_land_in_their_internal_slots() {
        let m = model(BASE);
        let cond = m.conventions.cond.as_ref().expect("条件码表");
        assert_eq!(cond["e"].code, 4);
        assert_eq!(cond["e"].ir.as_deref(), Some("eq"));
        let bits = m.conventions.bitsets.as_ref().expect("位集合表");
        assert_eq!(bits["fence"]["i"], 8);
        let names = m.conventions.imm_names.as_ref().expect("命名立即数表");
        assert_eq!(names["csr"]["mstatus"], 0x300);
        // 槽侧摊平（读的是降级后的表）
        let fset = m.operand_slots.iter().find(|s| s.name == "fset").unwrap();
        assert_eq!(fset.table.as_deref(), Some("fence"));
        assert_eq!(fset.table_entries, vec![("i".to_string(), 8), ("w".to_string(), 1)]);
        let csr = m.operand_slots.iter().find(|s| s.name == "csr12").unwrap();
        assert_eq!(csr.name_entries, vec![("mstatus".to_string(), 0x300)]);
    }

    #[test]
    fn two_cond_tables_are_rejected() {
        let bad = BASE.replace("[enum.csr]", "[enum.cond2]\ne = { code = 4, ir = \"eq\" }\n");
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("条件码表"), "err: {err}");
    }

    #[test]
    fn bits_table_rejects_negative_and_missing_values() {
        let bad = BASE.replace("i = 8", "i = -1");
        assert!(crate::dsl::parse_and_validate(&bad).is_err(), "位集合的值不能为负");
    }
}
