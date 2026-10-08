//! v21 W4 **指令流段模型**（x86 那一族变长编码）：`form.segments = [{ kind = …, … }, …]`
//! 降级回既有的语义键（[`EncKeys`]）+ 段内位域并入 `form.fields`。
//!
//! 与 W2/W3 同一套路（**降级到既有内部表示**，编码器/生成物一行不改）：
//!
//! - **有序段 = 发射序**（内联表数组；比"名字列表 + 命名子表"少一处重复，
//!   且 `[[forms]]` 的子表路径在 TOML 里有二义性——见执行清单的偏离记录）；
//! - 段 kind 是**闭集**（[`SegKind`]）：写错由 serde 报 unknown variant 并列出闭集；
//! - 每段用 W2 的**同一套字段语法**声明自己的字节布局（`"u1[7]:w"`），
//!   段的位域并进 `form.fields`，因此"位域表 / `opcode_field` / `operand_fields`"
//!   仍由 [`crate::dsl::field_decl`] 一处展开。
//!
//! **本片覆盖**：`prefix` / `escape` / `rex` / `opcode` / `modrm` / `imm`。
//! 其余段（`opcode_reg` / `sib` / `disp` / `vex` / `evex`）**明确报错**而不是静默忽略
//! （fail-closed）：x86 谱要用它们，等下一片。

use super::field_decl::FieldDecl;
use super::model::{
    EncKeys, Form, IsaModel, ModrmMap, PrefixKey, RexW, SegmentBytes, SegKind,
};

/// 把 `kind = "stream"`（写了 `segments`）的 form 降级回语义键 + `fields`。
pub fn lower_stream_forms(m: &mut IsaModel) -> Result<(), String> {
    for form in &mut m.forms {
        if form.segments.is_none() {
            continue;
        }
        lower_one(form)?;
    }
    Ok(())
}

fn lower_one(form: &mut Form) -> Result<(), String> {
    let segs = form.segments.clone().unwrap_or_default();
    if segs.is_empty() {
        return Err(format!(
            "[[forms.{}]]: `segments` 不能为空（stream 形态至少要一段）",
            form.name
        ));
    }
    let fname = form.name.clone();
    let mut keys = EncKeys::default();
    let mut fields: Vec<FieldDecl> = form.fields.clone().unwrap_or_default();
    let mut seen: Vec<SegKind> = Vec::new();
    let mut has_modrm = false;
    let mut imm_bits: u32 = 0;
    for seg in &segs {
        if let Some(prev) = seen.iter().find(|k| **k == seg.kind) {
            return Err(format!(
                "[[forms.{fname}]]: 段 kind `{}` 出现两次（每段至多一次）",
                kind_name(*prev)
            ));
        }
        seen.push(seg.kind);
        // 段内位域并进 form 的字段表（W2 一处展开：位域表 / opcode_field / operand_fields）。
        if let Some(fs) = &seg.fields {
            fields.extend(fs.iter().cloned());
        }
        match seg.kind {
            SegKind::Prefix => {
                let effects = match seg.bytes.clone() {
                    Some(SegmentBytes::Effects(map)) => map,
                    Some(SegmentBytes::Raw(_)) => {
                        return Err(format!(
                            "[[forms.{fname}]].prefix: 前缀段要写「字节 → 效果名」的字典\
                             （`bytes = {{ \"0x66\" = \"opsize16\" }}`），不是裸字节列表"
                        ));
                    }
                    None => {
                        return Err(format!(
                            "[[forms.{fname}]].prefix: 前缀段必须给 `bytes`（字节 → 效果名）"
                        ));
                    }
                };
                // 效果名是**闭集**（[`PrefixEffect::NAMES`] 是唯一来源）：写错在这里报，
                // 不留给下游按字符串比对（三处硬编码名字集一旦不同步就是静默漏派发）。
                //
                // 降级到经典 `prefix` 键时口径不同：经典键只在 `"opsize"` 处表达语义
                //（`PrefixEffect::Opsize16` ⇔ 经典写法的 `"opsize"`），其余前缀按**字节**写。
                //
                // 已知限制（记入执行清单）：经典校验要求 `"opsize"` **单独出现**，所以
                // "同时有 opsize 与其它前缀"的 form 现在还降不过去；W4 的"前缀表按 form
                // 显式化"（段成为唯一事实源）会取代该限制。
                let mut parts = Vec::new();
                for (byte, effect) in effects {
                    if !super::model::PrefixEffect::NAMES.contains(&effect.as_str()) {
                        return Err(format!(
                            "[[forms.{fname}]].prefix: 未知的前缀效果名 `{effect}`\
                             （可用：{}）",
                            super::model::PrefixEffect::NAMES.join(" / ")
                        ));
                    }
                    parse_byte(&byte).map_err(|e| format!("[[forms.{fname}]].prefix: {e}"))?;
                    if effect == "opsize16" {
                        parts.push("opsize".to_string());
                    } else {
                        parts.push(byte);
                    }
                }
                // 单来源写 `One`（与经典写法/序列化往返一致），多来源写 `Many`。
                keys.prefix = Some(if parts.len() == 1 {
                    PrefixKey::One(parts.remove(0))
                } else {
                    PrefixKey::Many(parts)
                });
            }
            SegKind::Escape => {
                let raw = match seg.bytes.clone() {
                    Some(SegmentBytes::Raw(v)) => v,
                    Some(SegmentBytes::Effects(_)) => {
                        return Err(format!(
                            "[[forms.{fname}]].escape: 转义段要写裸字节列表（`bytes = [\"0x0F\"]`）"
                        ));
                    }
                    None => {
                        return Err(format!(
                            "[[forms.{fname}]].escape: 转义段必须给 `bytes`（裸字节列表）"
                        ));
                    }
                };
                let mut out = Vec::new();
                for b in raw {
                    out.push(parse_byte(&b).map_err(|e| format!("[[forms.{fname}]].escape: {e}"))?);
                }
                keys.escape = Some(out);
            }
            SegKind::Rex => {
                keys.rex = Some("auto".to_string());
                // REX 的高 4 位恒为 0x40——由段 kind **蕴含**，谱里不必再写一遍
                //（设计文档的例子写了 `u4[3:0]=0x4`；无名常量槽不在 W2 支持面内，而
                // "这堆字节是 REX"本来就是结构承载的事实 ⇒ 不再写第二遍）。
                //
                // W 位来源：段里 `w` 字段带显式默认值 ⇒ 恒 1 / 恒 0；否则按宽度规则（auto）。
                let w_default = seg.fields.as_ref().and_then(|fs| {
                    fs.iter()
                        .find(|f| f.name.as_deref() == Some("w"))
                        .and_then(|f| f.default)
                });
                keys.rex_w = Some(match w_default {
                    Some(0) => RexW::Auto,
                    Some(_) => RexW::Always,
                    None => RexW::Auto,
                });
            }
            SegKind::Opcode => {
                // 操作码字段必须叫 `opcode`（值由指令的 `match = { opcode = … }` 给）。
                match form_opcode_field(&fields) {
                    Some(name) => keys.opcode_field = Some(name),
                    None => {
                        return Err(format!(
                            "[[forms.{fname}]].opcode: 段里必须有名为 `opcode` 的字段\
                             （位宽 = 操作码字节数）；它的值由指令的 `match` 给"
                        ));
                    }
                }
            }
            SegKind::Modrm => {
                has_modrm = true;
                keys.modrm = Some(ModrmMap {
                    reg: None,
                    rm: None,
                });
            }
            SegKind::OpcodeReg => {
                // `+r` 形式：opcode 字节 = 基值 | (op0 & 7)，REX.B = op0>>3。
                let base = seg.value.ok_or_else(|| {
                    format!(
                        "[[forms.{fname}]].opcode_reg: `+r` 段必须给 `value`（操作码基值，\
                         低 3 位留给寄存器号）"
                    )
                })?;
                keys.opcode_reg = Some(base);
            }
            SegKind::Vex => {
                let spec = seg.vex_spec();
                if spec.map.is_none()
                    && spec.pp.is_none()
                    && spec.w.is_none()
                    && spec.l.is_none()
                {
                    return Err(format!(
                        "[[forms.{fname}]].vex: VEX 段至少要给一个来源键\
                         （`map`/`pp`/`w`/`l`）——空头等于没声明"
                    ));
                }
                keys.vex = Some(spec);
            }
            SegKind::Evex => {
                let spec = seg.vex_spec();
                if spec.map.is_none()
                    && spec.pp.is_none()
                    && spec.w.is_none()
                    && spec.l.is_none()
                {
                    return Err(format!(
                        "[[forms.{fname}]].evex: EVEX 段至少要给一个来源键\
                         （`map`/`pp`/`w`/`l`）——空头等于没声明"
                    ));
                }
                keys.evex = Some(spec);
            }
            SegKind::Sib | SegKind::Disp => {
                // 布局段：**没有**对应的语义键——SIB/位移字节由编码器按内存操作数派生
                //（base/index/scale/disp）。段在这里的价值是"把字节布局写进谱"、
                // 让段内字段进 `form.fields` 参与声明期校验与 `--bits` 清点。
            }
            SegKind::Imm => {
                let fs = seg.fields.as_ref().ok_or_else(|| {
                    format!("[[forms.{fname}]].imm: 立即数段必须给 `fields`（声明宽度）")
                })?;
                for f in fs {
                    imm_bits = imm_bits.max(f.bits());
                }
                keys.imm = Some(imm_bits);
            }
        }
    }
    if !has_modrm {
        // `imm` 之外没有操作数编码段时，不许悄悄留空：stream 形态至少要有 modrm 或 imm。
        let has_operand_seg = segs
            .iter()
            .any(|s| matches!(s.kind, SegKind::Modrm | SegKind::Imm));
        if !has_operand_seg {
            return Err(format!(
                "[[forms.{fname}]]: stream 形态至少要有一段操作数编码（`modrm` 或 `imm`）"
            ));
        }
    }
    form.fields = Some(fields);
    form.keys = keys;
    Ok(())
}

/// form 的字段里名为 `opcode` 者。
fn form_opcode_field(fields: &[FieldDecl]) -> Option<String> {
    fields
        .iter()
        .find(|f| f.name.as_deref() == Some("opcode"))
        .and_then(|f| f.name.clone())
}

fn kind_name(k: SegKind) -> &'static str {
    match k {
        SegKind::Prefix => "prefix",
        SegKind::Escape => "escape",
        SegKind::Rex => "rex",
        SegKind::Opcode => "opcode",
        SegKind::OpcodeReg => "opcode_reg",
        SegKind::Modrm => "modrm",
        SegKind::Sib => "sib",
        SegKind::Disp => "disp",
        SegKind::Imm => "imm",
        SegKind::Vex => "vex",
        SegKind::Evex => "evex",
    }
}

fn parse_byte(s: &str) -> Result<u8, String> {
    let t = s.trim();
    let v = if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).map_err(|_| format!("`{s}` 不是合法字节（0x00..0xFF）"))?
    } else {
        t.parse::<u16>()
            .map_err(|_| format!("`{s}` 不是合法字节（0x00..0xFF）"))?
    };
    u8::try_from(v).map_err(|_| format!("`{s}` 超出字节范围（0x00..0xFF）"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 段写法与"经典语义键"写法必须降级到**同一份** EncKeys（等价性证明）。
    const STREAM: &str = r#"
[meta]
name = "t"
[encoding]
kind = "prefix_scan"
max_len = 15
[reg.gpr64]
count = 16
[operand.g]
kind = "reg"
class = "gpr64"
[[forms]]
name = "S"
segments = [
  { kind = "prefix", bytes = { "0xF0" = "lock" } },
  { kind = "escape", bytes = ["0x0F"] },
  { kind = "rex", fields = ["u1[7]:w", "u1[6]:r", "u1[5]:x", "u1[4]:b"] },
  { kind = "opcode", fields = ["u8[7:0]:opcode"] },
  { kind = "modrm", fields = ["u2[7:6]:mod", "u3[5:3]:reg", "u3[2:0]:rm"] },
]
[[instructions]]
name = "N"
form = "S"
match = { opcode = 0x8B }
ops = ["dst:g:out", "src:g"]
bind = { reg = "dst", rm = "src" }
asm = "n {dst}, {src}"
"#;

    const CLASSIC: &str = r#"
[meta]
name = "t"
[encoding]
kind = "prefix_scan"
max_len = 15
[reg.gpr64]
count = 16
[operand.g]
kind = "reg"
class = "gpr64"
[[forms]]
name = "S"
prefix = "0xF0"
escape = [0x0F]
rex = "auto"
rex_w = "auto"
[[instructions]]
name = "N"
form = "S"
modrm = { reg = "dst", rm = "src" }
ops = ["dst:g:out", "src:g"]
asm = "n {dst}, {src}"
"#;

    fn form_of(src: &str) -> Form {
        let m = crate::dsl::parse_and_validate(src).expect("谱合法");
        m.forms.into_iter().find(|f| f.name == "S").expect("form S")
    }

    #[test]
    fn stream_form_lowers_to_the_same_keys_as_classic() {
        let s = form_of(STREAM);
        let c = form_of(CLASSIC);
        assert_eq!(s.keys.prefix, c.keys.prefix, "prefix");
        assert_eq!(s.keys.escape, c.keys.escape, "escape");
        assert_eq!(s.keys.rex, c.keys.rex, "rex");
        assert_eq!(s.keys.rex_w, c.keys.rex_w, "rex_w");
        assert_eq!(s.keys.opcode_field.as_deref(), Some("opcode"));
        assert!(s.keys.modrm.is_some(), "modrm 段应打开 modrm");
        // 段内位域并进了 form.fields（于是 W2 的展开照旧）
        let names: Vec<&str> = s
            .fields
            .as_ref()
            .unwrap()
            .iter()
            .filter_map(|f| f.name.as_deref())
            .collect();
        for n in ["w", "r", "x", "b", "opcode", "mod", "reg", "rm"] {
            assert!(names.contains(&n), "缺字段 {n}：{names:?}");
        }
    }

    #[test]
    fn unknown_segment_kind_lists_the_closed_set() {
        let bad = STREAM.replace("kind = \"escape\"", "kind = \"nope\"");
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("unknown variant"), "err: {err}");
        assert!(err.contains("prefix"), "闭集应列在消息里：{err}");
    }

    #[test]
    fn unknown_prefix_effect_is_rejected_with_the_closed_set() {
        let bad = STREAM.replace("\"0xF0\" = \"lock\"", "\"0xF0\" = \"locked\"");
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("locked"), "err: {err}");
        assert!(err.contains("opsize16"), "闭集应列在消息里：{err}");
    }

    /// `opsize16`（闭集名）降级到经典键的 `"opsize"`（经典口径），单独出现时合法。
    #[test]
    fn opsize16_effect_lowers_to_classic_opsize() {
        let stream = STREAM.replace(
            "{ kind = \"prefix\", bytes = { \"0xF0\" = \"lock\" } },",
            "{ kind = \"prefix\", bytes = { \"0x66\" = \"opsize16\" } },",
        );
        let classic = CLASSIC.replace("prefix = \"0xF0\"", "prefix = \"opsize\"");
        let s = form_of(&stream);
        let c = form_of(&classic);
        assert_eq!(s.keys.prefix, c.keys.prefix, "opsize16 ⇔ 经典 \"opsize\"");
    }

    #[test]
    fn vex_segment_without_any_source_key_is_rejected() {
        let bad = STREAM.replace(
            "{ kind = \"opcode\", fields = [\"u8[7:0]:opcode\"] }",
            "{ kind = \"opcode\", fields = [\"u8[7:0]:opcode\"] }, { kind = \"vex\" }",
        );
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("vex"), "err: {err}");
        assert!(err.contains("至少要给一个来源键"), "err: {err}");
    }

    #[test]
    fn opcode_reg_segment_needs_value() {
        let bad = STREAM.replace(
            "{ kind = \"opcode\", fields = [\"u8[7:0]:opcode\"] }",
            "{ kind = \"opcode_reg\" }",
        );
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("opcode_reg"), "err: {err}");
        assert!(err.contains("value"), "err: {err}");
    }

    /// VEX 段 + `+r` 段 + 布局段（sib/disp）与经典写法等价。
    #[test]
    fn vex_and_opcode_reg_segments_lower_like_classic_keys() {
        let src = r#"
[meta]
name = "t"
[encoding]
kind = "prefix_scan"
max_len = 15
[reg.gpr64]
count = 16
[reg.fpr128]
count = 16
[operand.g]
kind = "reg"
class = "gpr64"
[operand.f]
kind = "reg"
class = "fpr128"
[[forms]]
name = "V"
segments = [
  { kind = "vex", map = "field", pp = "0x0F", w = "0", l = "0" },
  { kind = "opcode", fields = ["u8[7:0]:opcode"] },
  { kind = "modrm", fields = ["u2[7:6]:mod", "u3[5:3]:reg", "u3[2:0]:rm"] },
  { kind = "sib", fields = ["u2[7:6]:scale", "u3[5:3]:index", "u3[2:0]:base"] },
  { kind = "disp", fields = ["u8[7:0]:disp8"] },
]
[[instructions]]
name = "N"
form = "V"
match = { opcode = 0x58 }
ops = ["dst:f:out", "src:f"]
bind = { reg = "dst", rm = "src" }
asm = "n {dst}, {src}"
"#;
        let m = crate::dsl::parse_and_validate(src).expect("谱合法");
        let f = m.forms.iter().find(|f| f.name == "V").unwrap();
        let vex = f.keys.vex.as_ref().expect("vex 键");
        assert_eq!(vex.map.as_deref(), Some("field"));
        assert_eq!(vex.pp.as_deref(), Some("0x0F"));
        assert_eq!(vex.w.as_deref(), Some("0"));
        // 布局段的字段也进了 form.fields（参与校验与 --bits 清点）
        let names: Vec<&str> = f
            .fields
            .as_ref()
            .unwrap()
            .iter()
            .filter_map(|x| x.name.as_deref())
            .collect();
        for n in ["opcode", "mod", "reg", "rm", "scale", "index", "base", "disp8"] {
            assert!(names.contains(&n), "缺字段 {n}：{names:?}");
        }
    }

    #[test]
    fn duplicate_segment_kind_is_rejected() {
        let bad = STREAM.replace(
            "{ kind = \"modrm\", fields = [\"u2[7:6]:mod\", \"u3[5:3]:reg\", \"u3[2:0]:rm\"] }",
            "{ kind = \"modrm\", fields = [\"u3[2:0]:rm\"] }, { kind = \"modrm\" }",
        );
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("modrm"), "err: {err}");
        assert!(err.contains("两次"), "err: {err}");
    }

    #[test]
    fn bad_byte_literal_is_reported() {
        let bad = STREAM.replace("bytes = [\"0x0F\"]", "bytes = [\"0x1FF\"]");
        let err = format!("{:?}", crate::dsl::parse_and_validate(&bad).unwrap_err());
        assert!(err.contains("0x1FF"), "err: {err}");
    }
}
