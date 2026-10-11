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

use super::model::{CondBank, CondEntry, CondTables, EnumEntry, EnumKind, IsaModel};

/// 把 `[enum.<表名>]` 降级回内部三处。
pub fn lower_enum_layer(m: &mut IsaModel) -> Result<(), String> {
    if m.enums.is_empty() {
        return Ok(());
    }
    let mut cond_tables = CondTables::default();
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
                // v21 W6+ 丙：条件码表按**银行**归类（固定两类），同类只能一张。
                let bank = CondBank::detect(&t, name)?;
                if cond_tables.set(bank, t).is_some() {
                    return Err(format!(
                        "`[enum.{name}]`：`{}` 类条件码表已经有一张了（固定两类，各一张）",
                        bank.label()
                    ));
                }
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
    if let Some(t) = cond_tables.int.clone() {
        m.conventions.cond = Some(t);
    }
    m.cond_tables = cond_tables;
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
[form.R]
fields = ["u4[15:12]:opcode", "u4[11:8]:rd"]
[inst.N]
form = "R"
opcode = 1
asm = "n {dst:g:out}"
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
        assert_eq!(
            fset.table_entries,
            vec![("i".to_string(), 8), ("w".to_string(), 1)]
        );
        let csr = m.operand_slots.iter().find(|s| s.name == "csr12").unwrap();
        assert_eq!(csr.name_entries, vec![("mstatus".to_string(), 0x300)]);
    }

    #[test]
    fn two_cond_tables_are_rejected() {
        let bad = BASE.replace(
            "[enum.csr]",
            "[enum.cond2]\ne = { code = 4, ir = \"eq\" }\n",
        );
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("条件码表"), "err: {err}");
    }

    /// v21 W6+ 丙：条件码表**固定两类**（整数 / 浮点，各一张），cond 槽用 `cond_bank` 选；
    /// 银行由表内容判定（只看两个枚举**各自独有**的助记符）。
    #[test]
    fn cond_banks_are_fixed_and_inferred_from_content() {
        let fcond = r#"
[enum.fcond]
p  = { code = 10, ir = "uno" }
np = { code = 11, ir = "ord" }
[operand.fcc]
kind = "cond"
bits = 4
cond_bank = "float"
"#;
        let m = model(&format!("{BASE}{fcond}"));
        assert_eq!(
            m.operand_slots
                .iter()
                .find(|s| s.name == "fcc")
                .expect("operand.fcc")
                .cond_bank,
            CondBank::Float
        );
        let ic = m.cond_ir_codes_of(CondBank::Float);
        assert!(
            ic.contains(&("uno", 10)) && ic.contains(&("ord", 11)),
            "{ic:?}"
        );
        assert_eq!(
            m.cond_ir_codes_of(CondBank::Int).len(),
            1,
            "整数表仍是 `e = eq`"
        );

        // 同一类两张 ⇒ 报错（固定两类，各一张）。
        let two_int = BASE.replace(
            "[enum.csr]",
            "[enum.cond3]\ne2 = { code = 5, ir = \"ne\" }\n",
        );
        let err = format!(
            "{:?}",
            crate::dsl::parse_and_validate(&two_int).unwrap_err()
        );
        assert!(err.contains("条件码表"), "err: {err}");

        // 一张表里混装两类 ⇒ 报错（固定两类，各一张；没定论时按约定名当整数表）。
        let mixed = BASE.replace(
            "e = { code = 4, ir = \"eq\" }",
            "e = { code = 4, ir = \"eq\" }\nf = { code = 12, ir = \"olt\" }",
        );
        let err = format!("{:?}", crate::dsl::parse_and_validate(&mixed).unwrap_err());
        assert!(err.contains("同时含"), "err: {err}");
    }

    #[test]
    fn bits_table_rejects_negative_and_missing_values() {
        let bad = BASE.replace("i = 8", "i = -1");
        assert!(
            crate::dsl::parse_and_validate(&bad).is_err(),
            "位集合的值不能为负"
        );
    }
}
