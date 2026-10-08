//! v21 W3 **操作数层**：`[operand.<名字>]` 声明 → 内部 [`OperandSlot`] 数组。
//!
//! 与 W2 同一套路（**降级到既有内部表示**，编码器/生成物一行不改）：
//!
//! 1. 键收敛：`bits`（原 `width`）、`range = [lo, hi]`（合 `min`+`max`）、
//!    `value`（原 `float`）、`literal`（原 `wrap`）、`enum`（原 `table`/`names`）、
//!    `suffix`（原 `arrangement`）、`symbol = { allow, require, modifiers }`
//!    （合 `symbols`/`require_symbol`/`imm_fns`）、`zero`/`sp`（原 `zr31`）；
//! 2. `[operand.<mem>].text` / `.size_words` → `[conventions.mem]`（内部仍是全局一份，
//!    两个槽声明不同文本 ⇒ 报错，不静默取一个）；
//! 3. `[operand.<槽>].byte = true`（原槽级 `byte_reg`）：**8 位寄存器编码**。设计文档原打算
//!    把它挂到 `[reg.*].rex_required`，但 x86 `setcc` 实证推翻该假设——它的 rm 用 64 位
//!    名字（`class = "gpr64"`）却按 8 位寄存器编码，所以这是**槽**的事实。
//!
//! 新键都是**用户面**键；旧键（`width`/`min`/`max`/`float`/`wrap`/`table`/`names`/
//! `arrangement`/`symbols`/`require_symbol`/`imm_fns`/`zr31`/`byte_reg`）与
//! `[[operand_slots]]` 一律不再接受。

use super::model::{
    IsaModel, LiteralKind, MemTemplate, OperandDecl, OperandKind, OperandSlot, ValueKind,
};

/// 把 `[operand.<名字>]` 表降级成内部的 `operand_slots` 数组。
pub fn lower_operand_layer(m: &mut IsaModel) -> Result<(), String> {
    if m.operands.is_empty() {
        return Ok(());
    }
    // ① 逐槽降级（BTreeMap ⇒ 按名字序，语义与声明序无关：下游一律按名字查）。
    let mut slots: Vec<OperandSlot> = Vec::new();
    for (name, d) in &m.operands {
        slots.push(decl_to_slot(name, d)?);
    }
    // ③ 内存文本形态 / 尺寸关键字：只能有一处声明。
    let mut text: Option<(String, Vec<String>)> = None;
    let mut words: Option<(String, Vec<String>)> = None;
    for (name, d) in &m.operands {
        if let Some(t) = &d.text {
            match &text {
                Some((prev, _)) if prev != name => {
                    return Err(format!(
                        "`[operand.{name}].text` 与 `[operand.{prev}].text` 重复：内存文本形态只能声明一处"
                    ));
                }
                _ => text = Some((name.clone(), t.clone())),
            }
        }
        if let Some(w) = &d.size_words {
            match &words {
                Some((prev, _)) if prev != name => {
                    return Err(format!(
                        "`[operand.{name}].size_words` 与 `[operand.{prev}].size_words` 重复：尺寸关键字只能声明一处"
                    ));
                }
                _ => words = Some((name.clone(), w.clone())),
            }
        }
    }
    if text.is_some() || words.is_some() {
        let mem = m.conventions.mem.get_or_insert_with(|| MemTemplate {
            templates: Vec::new(),
            size_keywords: Vec::new(),
        });
        if let Some((_, t)) = text {
            mem.templates = t;
        }
        if let Some((_, w)) = words {
            mem.size_keywords = w;
        }
    }
    m.operand_slots = slots;
    Ok(())
}

/// 单条声明 → 内部槽（含全部互斥/一致性检查）。
fn decl_to_slot(name: &str, d: &OperandDecl) -> Result<OperandSlot, String> {
    if d.zero == Some(true) && d.sp == Some(true) {
        return Err(format!(
            "`[operand.{name}]`: `zero` 与 `sp` 互斥（31 号寄存器只能是其一）"
        ));
    }
    let zr31 = match (d.zero, d.sp) {
        (Some(true), _) => Some(true),
        (_, Some(true)) => Some(false),
        _ => None,
    };
    let (symbols, require_symbol, imm_fns) = match &d.symbol {
        None => (None, None, None),
        Some(s) => (s.allow, s.require, s.modifiers.clone()),
    };
    if require_symbol == Some(true) && symbols != Some(true) {
        return Err(format!(
            "`[operand.{name}].symbol.require = true` 需要同时写 `allow = true`（只收符号却又不允许符号）"
        ));
    }
    let (min, max) = match d.range {
        Some((lo, hi)) => {
            if lo > hi {
                return Err(format!("`[operand.{name}].range = [{lo}, {hi}]` 的上下界反了"));
            }
            (Some(lo), Some(hi))
        }
        None => (None, None),
    };
    let (table, names) = match (&d.enum_table, d.kind) {
        (None, _) => (None, None),
        (Some(t), OperandKind::Bits) => (Some(t.clone()), None),
        (Some(t), OperandKind::Imm) => (None, Some(t.clone())),
        // `kind = "cond"` 的表由 `[enum.cond]` 提供（W3b），槽上只写 `enum = "cond"`。
        (Some(_), OperandKind::Cond) => (None, None),
        (Some(_), k) => {
            return Err(format!(
                "`[operand.{name}].enum` 只对 imm/bits/cond 有意义（kind = {})",
                k.kind_name()
            ));
        }
    };
    let float = match d.value {
        Some(ValueKind::Float) => Some(true),
        _ => None,
    };
    let wrap = match d.literal {
        Some(LiteralKind::Bits) => Some(true),
        _ => None,
    };
    Ok(OperandSlot {
        name: name.to_string(),
        kind: d.kind,
        class: d.class,
        classes: d.classes.clone(),
        zr31,
        byte_reg: d.byte,
        width: d.bits,
        signed: d.signed,
        float,
        min,
        max,
        wrap,
        unit: d.unit,
        roles: d.roles,
        encode: d.encode,
        fields: d.fields.clone(),
        symbols,
        require_symbol,
        imm_fns,
        arrangement: d.suffix.clone(),
        table,
        names,
        table_entries: Vec::new(),
        name_entries: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(toml_src: &str) -> OperandDecl {
        toml::from_str(toml_src).expect("声明合法")
    }

    #[test]
    fn key_convergence_maps_onto_internal_slot() {
        let d = decl(
            r#"
kind = "imm"
bits = 32
signed = true
range = [-16, 16]
literal = "bits"
value = "int"
unit = 2
"#,
        );
        let s = decl_to_slot("x", &d).unwrap();
        assert_eq!(s.width, Some(32));
        assert_eq!(s.signed, Some(true));
        assert_eq!((s.min, s.max), (Some(-16), Some(16)));
        assert_eq!(s.wrap, Some(true));
        assert_eq!(s.unit, Some(2));
        assert_eq!(s.float, None);
    }

    #[test]
    fn enum_lands_on_table_or_names_by_kind() {
        let bits = decl_to_slot("f", &decl("kind = \"bits\"\nenum = \"fence\"")).unwrap();
        assert_eq!(bits.table.as_deref(), Some("fence"));
        assert_eq!(bits.names, None);
        let imm = decl_to_slot("c", &decl("kind = \"imm\"\nenum = \"csr\"")).unwrap();
        assert_eq!(imm.names.as_deref(), Some("csr"));
        assert_eq!(imm.table, None);
        // reg 上没有 enum 的位置
        let bad = decl_to_slot("r", &decl("kind = \"reg\"\nenum = \"x\""));
        assert!(bad.unwrap_err().contains("只对 imm/bits/cond 有意义"));
    }

    #[test]
    fn symbol_sub_table_merges_three_old_keys() {
        let s = decl_to_slot(
            "sym",
            &decl(
                r#"
kind = "imm"
bits = 12
symbol = { allow = true, require = true, modifiers = ["abs_g0"] }
"#,
            ),
        )
        .unwrap();
        assert_eq!(s.symbols, Some(true));
        assert_eq!(s.require_symbol, Some(true));
        assert_eq!(s.imm_fns.as_deref(), Some(&["abs_g0".to_string()][..]));
        // require 而无 allow ⇒ 报错（旧口径维持）
        let bad = decl_to_slot("s", &decl("kind = \"imm\"\nsymbol = { require = true }"));
        assert!(bad.unwrap_err().contains("allow = true"));
    }

    #[test]
    fn zero_and_sp_are_exclusive() {
        assert_eq!(decl_to_slot("z", &decl("kind = \"reg\"\nzero = true")).unwrap().zr31, Some(true));
        assert_eq!(decl_to_slot("s", &decl("kind = \"reg\"\nsp = true")).unwrap().zr31, Some(false));
        let bad = decl_to_slot("b", &decl("kind = \"reg\"\nzero = true\nsp = true"));
        assert!(bad.unwrap_err().contains("互斥"));
    }

    #[test]
    fn suffix_is_the_old_arrangement() {
        let s = decl_to_slot(
            "v",
            &decl("kind = \"reg\"\nsuffix = { \"8b\" = 0, \"16b\" = 1 }"),
        )
        .unwrap();
        let a = s.arrangement.unwrap();
        assert_eq!(a["16b"], 1);
    }

    #[test]
    fn old_keys_are_rejected() {
        for bad in [
            "kind = \"imm\"\nwidth = 8",
            "kind = \"imm\"\nmin = 0",
            "kind = \"imm\"\nmax = 1",
            "kind = \"imm\"\nwrap = true",
            "kind = \"imm\"\nfloat = true",
            "kind = \"bits\"\ntable = \"t\"",
            "kind = \"imm\"\nnames = \"t\"",
            "kind = \"reg\"\narrangement = { \"8b\" = 0 }",
            "kind = \"imm\"\nsymbols = true",
            "kind = \"imm\"\nrequire_symbol = true",
            "kind = \"imm\"\nimm_fns = [\"a\"]",
            "kind = \"reg\"\nzr31 = true",
            "kind = \"reg\"\nbyte_reg = true",
        ] {
            assert!(
                toml::from_str::<OperandDecl>(bad).is_err(),
                "旧键必须被拒：{bad}"
            );
        }
    }

    #[test]
    fn mem_text_and_size_words_lower_into_conventions_mem() {
        let src = r#"
[meta]
name = "t"
[encoding]
kind = "fixed"
bits = 16
[reg.gpr32]
count = 4
[operand.mem]
kind = "mem"
text = ["[{base}+{disp}]", "{disp}[{base}]"]
size_words = ["word ptr"]
[operand.r]
kind = "reg"
class = "gpr32"
[[forms]]
name = "M"
fields = [
  "u8[15:8]:opcode",
  "u4[7:4]:dst",
  "u4[3:0]:src",
]
[[instructions]]
name = "LD"
form = "M"
opcode = 1
ops = ["dst:r:out", "src:mem"]
asm = "ld {dst}, {src}"
"#;
        let m = crate::dsl::parse_and_validate(src).expect("谱合法");
        assert_eq!(m.operand_slots.len(), 2);
        let mem = m.conventions.mem.expect("mem 约定由槽降级而来");
        assert_eq!(mem.templates.len(), 2);
        assert_eq!(mem.size_keywords, vec!["word ptr".to_string()]);
    }
}
