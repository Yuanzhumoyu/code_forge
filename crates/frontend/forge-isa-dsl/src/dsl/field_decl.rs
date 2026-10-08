//! v21 字段声明（`[[forms]].fields` / `[[instructions]].fields`）——**就地**声明位域的
//! 类型、位区间、接口名与默认值，取代 `[conventions.bitfields]` + `opcode_field` +
//! `operand_fields` 三处往返（设计文档 §6.2）。
//!
//! 本模块只做**解析**与**降级**：把声明展开成内部已有的 [`Bitfield`] 表示。
//! **不接线到 TOML 反序列化**（W2.2a：纯新增、暂无读者；编码器/生成物一行不动）。
//!
//! # 语法
//!
//! ```text
//! field      = type ( "|" type )* "[" span [ "->" chunk ( "," chunk )* ] "]"
//!              [ ":" name ] [ "=" value ]
//! type       = "u" N | "i" N | <寄存器组名> | <操作数槽名>
//! span       = bit | bit ":" bit
//! chunk      = bit | bit ":" bit
//! ```
//!
//! 两种形态：
//!
//! - **连续**：`u7[6:0]:opcode=0x33` —— 词位段就是 `[6:0]`，值位段缺省 = `[6:0]`
//!   （值的最低位落在词的最低位）。
//! - **散射**：`i13[12:1 -> 31,30:25,11:8,7]:imm_b` —— 冒号前是**值位段**
//!   （本字段承载的值位，低位在前），`->` 后是若干**词位块**，按"值低位 → 值高位"
//!   依次放置。
//!
//! # 为什么需要 `->`（实测依据）
//!
//! riscv 的 S/B/J 型立即数是散射的，且**值自身 bit 0 不参与编码**（2 字节对齐），
//! 于是既有 `pieces.shift` 不是从 0 连续开始（`imm_b` = {1,5,11,12}、`imm_j` =
//! {1,11,12,20}，见 `isa/riscv64.toml`）。"值序从 bit 0 起"的隐含约定表达不了这个
//! 空缺，所以值位段显式写出来；`i13[12:1 -> 31,30:25,11:8,7]` 与既有
//! `pieces = [{31,1,12},{25,6,5},{8,4,1},{7,1,11}]` **逐段等价**（见本模块单测）。

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::model::{Bitfield, BitfieldPiece};

/// 字段的类型（联合用 `|` 分隔；W2 只做校验，不替换 `ops` 的类型来源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldType {
    /// `u<N>`：N 位无符号数据。
    U(u32),
    /// `i<N>`：N 位有符号数据。
    I(u32),
    /// 寄存器组名（`[reg.*]` 表头）或操作数槽名（`[[operand_slots]]`）——
    /// 具体是哪一个在接线后由 model 校验。
    Named(String),
}

/// 一条字段声明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDecl {
    /// 类型联合（至少一个）。
    pub types: Vec<FieldType>,
    /// **值位段** `(hi, lo)`：本字段承载的值位（含）。
    pub value: (u32, u32),
    /// **词位块**：按"值低位 → 值高位"排列；`(hi, lo)`，单 bit 时 `hi == lo`。
    pub chunks: Vec<(u32, u32)>,
    /// 接口名（`asm`/`Inst` 字段名/`match` 引用它）；缺省 = 匿名常量槽。
    pub name: Option<String>,
    /// 默认编码值；缺省 = 0。
    pub default: Option<i64>,
}

impl FieldDecl {
    /// 字段承载的位数（= 值位段宽度 = 各词位块宽度之和）。
    pub fn bits(&self) -> u32 {
        self.value.0 - self.value.1 + 1
    }

    /// 解析一条声明。错误消息只描述本声明（TOML 层没有 span，与既有诊断口径一致）。
    pub fn parse(src: &str) -> Result<Self, String> {
        let s = src.trim();
        if s.is_empty() {
            return Err("空声明".into());
        }
        // ① 类型与 `[...]` 分离：括号内不允许嵌套。
        let open = s
            .find('[')
            .ok_or_else(|| format!("缺少 `[位区间]`：`{s}`"))?;
        let close = s[open..]
            .find(']')
            .map(|i| open + i)
            .ok_or_else(|| format!("`[` 没有闭合：`{s}`"))?;
        let head = s[..open].trim();
        let body = s[open + 1..close].trim();
        let tail = s[close + 1..].trim();

        // ② 类型联合。
        let mut types = Vec::new();
        for t in head.split('|') {
            let t = t.trim();
            if t.is_empty() {
                return Err(format!("类型写法有空的联合分支：`{s}`"));
            }
            types.push(parse_type(t).map_err(|e| format!("{e}（`{s}`）"))?);
        }
        if types.is_empty() {
            return Err(format!("缺少字段类型：`{s}`"));
        }

        // ③ 值位段与词位块。
        let (value, chunks) = match body.split_once("->") {
            None => {
                // 连续形态：词位段就是 `[hi:lo]`，值位段**归一化**为 `[宽度-1 : 0]`
                //（值的最低位落在词的最低位）——`[11:7]` 是 5 位值、不是 shift 7。
                let w = parse_span(body.trim()).map_err(|e| format!("{e}（`{s}`）"))?;
                let width = w.0 - w.1 + 1;
                ((width - 1, 0), vec![w])
            }
            Some((v, ws)) => {
                let v = parse_span(v.trim()).map_err(|e| format!("值位段：{e}（`{s}`）"))?;
                let mut chunks = Vec::new();
                for c in ws.split(',') {
                    let c = c.trim();
                    if c.is_empty() {
                        return Err(format!("`->` 后有空词位块：`{s}`"));
                    }
                    chunks.push(parse_span(c).map_err(|e| format!("词位块：{e}（`{s}`）"))?);
                }
                (v, chunks)
            }
        };
        let width: u32 = chunks.iter().map(|(hi, lo)| hi - lo + 1).sum();
        let vwidth = value.0 - value.1 + 1;
        if width != vwidth {
            return Err(format!(
                "值位段宽 {vwidth} 位 ≠ 词位块总宽 {width} 位（`{s}`）"
            ));
        }

        // ④ `: 名字` 与 `= 默认值`。
        let (name, rest) = match tail.split_once('=') {
            Some((n, v)) => (n.trim(), Some(v.trim())),
            None => (tail, None),
        };
        let name = {
            let n = name.trim();
            if n.is_empty() {
                None
            } else if let Some(stripped) = n.strip_prefix(':') {
                let id = stripped.trim();
                if id.is_empty() {
                    return Err(format!("`:` 后没有名字：`{s}`"));
                }
                Some(id.to_string())
            } else {
                return Err(format!("`]` 之后只允许 `: 名字` 或 `= 值`：`{s}`"));
            }
        };
        let default = match rest {
            None => None,
            Some(v) => Some(parse_value(v).map_err(|e| format!("{e}（`{s}`）"))?),
        };

        Ok(Self {
            types,
            value,
            chunks,
            name,
            default,
        })
    }

    /// 降级成内部 [`Bitfield`]：值位段无前导空缺且只有一块 → `offset/width`；
    /// 否则 → `pieces`（`shift` = 值偏移，`offset`/`width` = 词位置）。
    pub fn to_bitfield(&self) -> Bitfield {
        if self.chunks.len() == 1 && self.value.1 == 0 {
            let (hi, lo) = self.chunks[0];
            return Bitfield {
                offset: Some(lo),
                width: Some(hi - lo + 1),
                pieces: None,
            };
        }
        let mut shift = self.value.1;
        let mut pieces = Vec::new();
        for (hi, lo) in &self.chunks {
            let width = hi - lo + 1;
            pieces.push(BitfieldPiece {
                offset: *lo,
                width,
                shift,
            });
            shift += width;
        }
        Bitfield {
            offset: None,
            width: None,
            pieces: Some(pieces),
        }
    }
}

impl fmt::Display for FieldDecl {
    /// 规范化重建（`parse` 的逆）：用于 serde 输出与诊断回显。
    /// 连续形态（值位段恰为 `[宽度-1:0]`）不写 `->`。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ty = self
            .types
            .iter()
            .map(|t| match t {
                FieldType::U(n) => format!("u{n}"),
                FieldType::I(n) => format!("i{n}"),
                FieldType::Named(n) => n.clone(),
            })
            .collect::<Vec<_>>()
            .join("|");
        let span = |(hi, lo): (u32, u32)| {
            if hi == lo {
                format!("{hi}")
            } else {
                format!("{hi}:{lo}")
            }
        };
        write!(f, "{ty}[")?;
        let plain = self.chunks.len() == 1
            && self.value.1 == 0
            && self.value.0 == self.chunks[0].0 - self.chunks[0].1;
        if plain {
            write!(f, "{}", span(self.chunks[0]))?;
        } else {
            write!(f, "{}", span(self.value))?;
            let ws = self
                .chunks
                .iter()
                .map(|c| span(*c))
                .collect::<Vec<_>>()
                .join(",");
            write!(f, " -> {ws}")?;
        }
        write!(f, "]")?;
        if let Some(n) = &self.name {
            write!(f, ":{n}")?;
        }
        if let Some(v) = self.default {
            write!(f, "={v}")?;
        }
        Ok(())
    }
}

impl Serialize for FieldDecl {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for FieldDecl {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        FieldDecl::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// 解析一个位区间：`hi:lo` 或单 bit `n`（`hi == lo`）。要求 `hi >= lo`。
fn parse_span(s: &str) -> Result<(u32, u32), String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("空的位区间".into());
    }
    let (hi, lo) = match s.split_once(':') {
        None => {
            let n = parse_u32(s)?;
            (n, n)
        }
        Some((h, l)) => (parse_u32(h.trim())?, parse_u32(l.trim())?),
    };
    if hi < lo {
        return Err(format!("位区间 `{s}` 的高位小于低位"));
    }
    Ok((hi, lo))
}

/// 解析字段类型：`u<N>` / `i<N>` / 名字。
fn parse_type(s: &str) -> Result<FieldType, String> {
    for (prefix, ctor) in [("u", 0u8), ("i", 1u8)] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if let Ok(n) = rest.parse::<u32>() {
                if n == 0 {
                    return Err(format!("类型 `{s}` 的位宽必须是正整数"));
                }
                return Ok(if ctor == 0 {
                    FieldType::U(n)
                } else {
                    FieldType::I(n)
                });
            }
            // `imm12` 这类以 u/i 开头的名字不是类型关键字（要求整串是 `u<数字>`）。
            if rest.is_empty() {
                return Err(format!("类型 `{s}` 缺少位宽"));
            }
        }
    }
    Ok(FieldType::Named(s.to_string()))
}

fn parse_u32(s: &str) -> Result<u32, String> {
    let t = s.trim();
    let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u32::from_str_radix(h, 16)
    } else if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        u32::from_str_radix(b, 2)
    } else {
        t.parse::<u32>()
    };
    v.map_err(|e| format!("`{t}` 不是合法位序号/位宽：{e}"))
}

/// 解析 `= 值`：十进制（可带负号）/ `0x` / `0b`。
fn parse_value(s: &str) -> Result<i64, String> {
    let t = s.trim();
    if let Some(r) = t.strip_prefix('-') {
        let v = parse_value(r)?;
        return Ok(-v);
    }
    let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i64::from_str_radix(h, 16)
    } else if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        i64::from_str_radix(b, 2)
    } else {
        t.parse::<i64>()
    };
    v.map_err(|e| format!("`{t}` 不是合法默认值：{e}"))
}

/// 把一个 form/指令的 `fields` 声明列表降级成**内部位域表**（§4.0 的 ①）。
///
/// 名字必填（匿名槽无法被 `match`/`ops` 引用）；重名报错。
pub fn lower_bitfields(decls: &[FieldDecl]) -> Result<BTreeMap<String, Bitfield>, String> {
    let mut out = BTreeMap::new();
    for (i, d) in decls.iter().enumerate() {
        let name = d
            .name
            .clone()
            .ok_or_else(|| format!("fields[{i}]：只能无名常量槽才允许省略名字（本片尚未支持）"))?;
        if out.insert(name.clone(), d.to_bitfield()).is_some() {
            return Err(format!("fields[{i}]：字段名 `{name}` 重复"));
        }
    }
    Ok(out)
}

/// 把操作数名绑定到字段名（§4.0 的 ②）：同名即绑定，`bind` 显式改名。
///
/// 返回按 `ops` 序排列的字段名列表（= 内部 `operand_fields`）。
pub fn bind_operands(
    decls: &[FieldDecl],
    ops_names: &[String],
    bind: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let names: Vec<&str> = decls.iter().filter_map(|d| d.name.as_deref()).collect();
    let mut out = Vec::new();
    for op in ops_names {
        let field = bind.get(op).map(String::as_str).unwrap_or(op.as_str());
        if !names.contains(&field) {
            return Err(format!(
                "操作数 `{op}` 绑定到字段 `{field}`，但该字段未在本 form 的 fields 里声明（已声明：{}）",
                names.join(", ")
            ));
        }
        out.push(field.to_string());
    }
    Ok(out)
}

/// W2 迁移算法（执行清单 §4.2）：把一个 form 的既有声明折叠成 v21 的 `fields` 列表。
///
/// - `opcode_field`：既有 `[[forms]].opcode_field`（`Some` ⇒ 定宽 ISA，本机制适用）
/// - `operand_fields`：既有位置绑定（第 i 个操作数 → 第 i 个位域名）
/// - `const_names`：用该 form 的指令在 `match` 里出现的**位域名**（首次出现序）
/// - `bitfields`：既有 `[conventions.bitfields]`
///
/// 输出顺序 = `opcode` → `operand_fields`（去重）→ 常量字段；类型统一写 `u<字段宽>`
/// （旧模型没有类型信息，`u` 是诚实的默认；语义类型由后续 W2b 的槽绑定校验补）。
pub fn form_field_decls(
    opcode_field: Option<&str>,
    operand_fields: &[String],
    const_names: &[String],
    bitfields: &BTreeMap<String, Bitfield>,
) -> Result<Vec<FieldDecl>, String> {
    let mut names: Vec<String> = Vec::new();
    let mut add = |n: &str, names: &mut Vec<String>| {
        if !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    };
    if let Some(o) = opcode_field {
        add(o, &mut names);
    }
    for n in operand_fields {
        add(n, &mut names);
    }
    for n in const_names {
        // 只收**位域名**：vlen ISA 的 `match` 里还有编码键（prefix/w/vex_map…），
        // 那些不属于本机制。
        if bitfields.contains_key(n) {
            add(n, &mut names);
        }
    }
    let mut out = Vec::new();
    for n in &names {
        let bf = bitfields
            .get(n)
            .ok_or_else(|| format!("字段 `{n}` 未在 [conventions.bitfields] 里声明"))?;
        out.push(bitfield_to_decl(n, bf)?);
    }
    Ok(out)
}

/// 既有 [`Bitfield`] → 声明：`offset/width` 走连续形态；`pieces` 按 `shift` 排序
/// （值低位 → 高位）走 `->` 散射形态。
pub fn bitfield_to_decl(name: &str, bf: &Bitfield) -> Result<FieldDecl, String> {
    match (&bf.offset, &bf.width, &bf.pieces) {
        (Some(off), Some(w), None) => {
            if *w == 0 {
                return Err(format!("字段 `{name}` 的 width 为 0"));
            }
            Ok(FieldDecl {
                types: vec![FieldType::U(*w)],
                value: (w - 1, 0),
                chunks: vec![(off + w - 1, *off)],
                name: Some(name.to_string()),
                default: None,
            })
        }
        (None, None, Some(ps)) => {
            if ps.is_empty() {
                return Err(format!("字段 `{name}` 的 pieces 为空"));
            }
            let mut ps = ps.clone();
            ps.sort_by_key(|p| p.shift);
            let total: u32 = ps.iter().map(|p| p.width).sum();
            let lo = ps[0].shift;
            Ok(FieldDecl {
                types: vec![FieldType::U(total)],
                value: (lo + total - 1, lo),
                chunks: ps
                    .iter()
                    .map(|p| (p.offset + p.width - 1, p.offset))
                    .collect(),
                name: Some(name.to_string()),
                default: None,
            })
        }
        _ => Err(format!(
            "字段 `{name}` 的声明形态不合法（`offset`+`width` 与 `pieces` 二选一）"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 位域的**规范形式**：`(词偏移, 宽度, 值偏移)` 列表，按值偏移排序。
    /// `offset/width` 等价于"单段、shift 0"的 `pieces`。
    fn canon(b: &Bitfield) -> Vec<(u32, u32, u32)> {
        match (&b.offset, &b.width, &b.pieces) {
            (Some(o), Some(w), None) => vec![(*o, *w, 0)],
            (None, None, Some(ps)) => {
                let mut v: Vec<(u32, u32, u32)> = ps.iter().map(|p| (p.offset, p.width, p.shift)).collect();
                v.sort_by_key(|x| x.2);
                v
            }
            _ => panic!("非法位域声明"),
        }
    }

    fn bf(offset: Option<u32>, width: Option<u32>) -> Bitfield {
        Bitfield {
            offset,
            width,
            pieces: None,
        }
    }

    #[test]
    fn parse_contiguous_with_name_and_default() {
        let d = FieldDecl::parse("u7[6:0]:opcode=0x33").unwrap();
        assert_eq!(d.types, vec![FieldType::U(7)]);
        assert_eq!(d.value, (6, 0));
        assert_eq!(d.chunks, vec![(6, 0)]);
        assert_eq!(d.name.as_deref(), Some("opcode"));
        assert_eq!(d.default, Some(0x33));
        assert_eq!(d.to_bitfield(), bf(Some(0), Some(7)));
    }

    #[test]
    fn parse_union_type_and_no_default() {
        let d = FieldDecl::parse("gpr64|gpr32[11:7]:rd").unwrap();
        assert_eq!(
            d.types,
            vec![FieldType::Named("gpr64".into()), FieldType::Named("gpr32".into())]
        );
        assert_eq!(d.name.as_deref(), Some("rd"));
        assert_eq!(d.default, None);
        assert_eq!(d.to_bitfield(), bf(Some(7), Some(5)));
    }

    #[test]
    fn parse_negative_and_binary_default() {
        assert_eq!(FieldDecl::parse("i8[7:0]:x=-8").unwrap().default, Some(-8));
        assert_eq!(
            FieldDecl::parse("u1[21]:one21=0b1").unwrap().default,
            Some(1)
        );
    }

    #[test]
    fn parse_anonymous_constant_slot() {
        let d = FieldDecl::parse("u1[21]=0").unwrap();
        assert_eq!(d.name, None);
        assert_eq!(d.default, Some(0));
    }

    /// **与既有 `pieces` 的逐段等价**（`isa/riscv64.toml` 的 `imm_b`）。
    ///
    /// 既有：`pieces = [{31,1,shift12},{25,6,shift5},{8,4,shift1},{7,1,shift11}]`
    /// 新语法：值位段 [12:1]（bit 0 不参与），词位块按值低位→高位列出。
    #[test]
    fn scatter_matches_riscv_imm_b_pieces() {
        let d = FieldDecl::parse("i13[12:1 -> 11:8,30:25,7,31]:imm_b").unwrap();
        assert_eq!(d.bits(), 12);
        let got = d.to_bitfield();
        let pieces = got.pieces.expect("散射");
        // 按"值低位 → 高位"：shift 1 → word [11:8]（4 位）；shift 5 → [30:25]（6 位）；
        // shift 11 → [7]；shift 12 → [31]。
        let want: Vec<(u32, u32, u32)> = vec![(8, 4, 1), (25, 6, 5), (7, 1, 11), (31, 1, 12)];
        let got3: Vec<(u32, u32, u32)> = pieces.iter().map(|p| (p.offset, p.width, p.shift)).collect();
        let mut want_sorted = want.clone();
        let mut got_sorted = got3.clone();
        want_sorted.sort();
        got_sorted.sort();
        assert_eq!(got_sorted, want_sorted, "imm_b 散射逐段等价");
    }

    /// J 型（`imm_j`：shift {1,11,12,20}）同样逐段等价。
    #[test]
    fn scatter_matches_riscv_imm_j_pieces() {
        let d = FieldDecl::parse("i21[20:1 -> 30:21,20,19:12,31]:imm_j").unwrap();
        let got = d.to_bitfield();
        let pieces = got.pieces.expect("散射");
        let mut got3: Vec<(u32, u32, u32)> = pieces.iter().map(|p| (p.offset, p.width, p.shift)).collect();
        let mut want: Vec<(u32, u32, u32)> = vec![(31, 1, 20), (21, 10, 1), (20, 1, 11), (12, 8, 12)];
        got3.sort();
        want.sort();
        assert_eq!(got3, want, "imm_j 散射逐段等价");
    }

    #[test]
    fn rejects_width_mismatch_and_bad_range() {
        assert!(FieldDecl::parse("u5[11:7 -> 3:0]").unwrap_err().contains("≠"));
        assert!(FieldDecl::parse("u5[7:11]").unwrap_err().contains("高位小于低位"));
        assert!(FieldDecl::parse("u5").unwrap_err().contains("缺少"));
        assert!(FieldDecl::parse("u0[3:0]").unwrap_err().contains("正整数"));
    }

    /// 连续形态的值位段**归一化**：`[11:7]` 是 5 位值（shift 0），不是 shift 7。
    #[test]
    fn contiguous_span_is_normalised() {
        let d = FieldDecl::parse("gpr64[11:7]:rd").unwrap();
        assert_eq!(d.value, (4, 0), "值位段 = [宽度-1 : 0]");
        assert_eq!(d.bits(), 5);
        // 单块但**有前导空缺**（值 bit 0 不参与）⇒ 必须走 pieces。
        let gap = FieldDecl::parse("i12[12:1 -> 11:0]:off").unwrap();
        let p = gap.to_bitfield().pieces.expect("有前导空缺 ⇒ pieces");
        assert_eq!((p[0].offset, p[0].width, p[0].shift), (0, 12, 1));
    }

    /// 规范化重建：`Display` 是**规范形式**（默认值写十进制），`parse∘Display` 稳定。
    #[test]
    fn display_round_trips() {
        for (src, canonical) in [
            ("u7[6:0]:opcode=0x33", "u7[6:0]:opcode=51"),
            ("gpr64|gpr32[11:7]:rd", "gpr64|gpr32[11:7]:rd"),
            (
                "i13[12:1 -> 11:8,30:25,7,31]:imm_b",
                "i13[12:1 -> 11:8,30:25,7,31]:imm_b",
            ),
            ("u1[21]", "u1[21]"),
        ] {
            let d = FieldDecl::parse(src).unwrap();
            assert_eq!(d.to_string(), canonical, "规范形式");
            let again = FieldDecl::parse(&d.to_string()).unwrap();
            assert_eq!(again, d, "parse∘Display 是恒等");
            assert_eq!(again.to_string(), canonical, "Display 幂等");
        }
    }

    /// **迁移算法在三份发行谱上的等价性证明**（W2.2b 的前置证据）。
    ///
    /// 对每个**定宽** form：把它折叠成 `fields` 声明、再降级回位域表，必须与既有
    /// `[conventions.bitfields]` **逐字段相等**；每条指令的 `operand_fields` 也必须能被
    /// "同名绑定 + `bind`" 还原。TOML 的实际改写由本算法产出的文本驱动 ⇒ 不可能偏离。
    #[test]
    fn migration_algorithm_is_equivalent_on_shipped_specs() {
        // 第三项 = 该谱是否应当有可校验的定宽 form（x86 是变长谱，全部 form 都没有
        // `opcode_field`，本机制不适用——它跟着 `match` 改名走，字段语法留给 W4 的 stream）。
        for (label, src, expect_fixed) in [
            ("x86", include_str!("../../../../../isa/x86.toml"), false),
            ("riscv64", include_str!("../../../../../isa/riscv64.toml"), true),
            ("arm64", include_str!("../../../../../isa/arm64.toml"), true),
        ] {
            let m = crate::dsl::parse_and_validate(src)
                .unwrap_or_else(|e| panic!("{label} 解析失败：{e}"));
            let bf = &m.conventions.bitfields;
            let (mut nf, mut ni) = (0usize, 0usize);
            for form in &m.forms {
                let Some(opcode_field) = form.keys.opcode_field.as_deref() else {
                    continue; // 变长 form（x86）不走本机制
                };
                let opf = form.keys.operand_fields.clone().unwrap_or_default();
                let insts: Vec<&crate::dsl::model::Instruction> = m
                    .instructions
                    .iter()
                    .filter(|i| i.form.as_deref() == Some(form.name.as_str()))
                    .collect();
                let mut consts: Vec<String> = Vec::new();
                for i in &insts {
                    if let Some(fs) = &i.fields {
                        for k in fs.keys() {
                            if !consts.iter().any(|x| x == k) {
                                consts.push(k.clone());
                            }
                        }
                    }
                }
                let decls = form_field_decls(Some(opcode_field), &opf, &consts, bf)
                    .unwrap_or_else(|e| panic!("{label}/{}: {e}", form.name));
                // ① 降级回位域表：**按语义**逐字段相等（`offset/width` 与"单段 shift=0 的
                // `pieces`"是同一种编码，规范形式统一后比较）。
                let lowered = lower_bitfields(&decls).unwrap();
                for (n, got) in &lowered {
                    let want = bf
                        .get(n)
                        .unwrap_or_else(|| panic!("{label}/{}: 字段 `{n}` 原本不存在", form.name));
                    assert_eq!(
                        canon(got),
                        canon(want),
                        "{label}/{}: 字段 `{n}` 迁移前后不等价",
                        form.name
                    );
                }
                // ② 绑定迁移：同名绑定 + bind 必须还原既有 operand_fields。
                for i in &insts {
                    let ops_names: Vec<String> = i
                        .ops
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|e| e.split(':').next().unwrap_or("").trim().to_string())
                        .collect();
                    let mut bind = BTreeMap::new();
                    for (k, op) in ops_names.iter().enumerate() {
                        if let Some(f) = opf.get(k)
                            && f != op
                        {
                            bind.insert(op.clone(), f.clone());
                        }
                    }
                    if ops_names.is_empty() || ops_names.len() > opf.len() {
                        continue;
                    }
                    let got = bind_operands(&decls, &ops_names, &bind)
                        .unwrap_or_else(|e| panic!("{label}/{}: {e}", i.name));
                    assert_eq!(
                        got,
                        opf[..ops_names.len()].to_vec(),
                        "{label}/{}: operand_fields 绑定迁移不等价",
                        i.name
                    );
                    ni += 1;
                }
                nf += 1;
            }
            assert!(
                !expect_fixed || nf > 0,
                "{label}: 没有可校验的定宽 form（expected={expect_fixed}）"
            );
            eprintln!("{label}: {nf} forms / {ni} insts 迁移等价 ✔");
        }
    }

    /// serde 面就绪（接线到 `Form`/`Instruction` 的前提）：字符串 ⇄ `FieldDecl`。
    #[test]
    fn serde_round_trips_through_toml() {
        #[derive(serde::Deserialize)]
        struct Holder {
            fields: Vec<FieldDecl>,
        }
        let src = r#"
fields = ["u7[6:0]:opcode=0x33", "gpr64[11:7]:rd", "i13[12:1 -> 11:8,30:25,7,31]:imm_b"]
"#;
        let h: Holder = toml::from_str(src).unwrap();
        assert_eq!(h.fields.len(), 3);
        assert_eq!(h.fields[0].name.as_deref(), Some("opcode"));
        assert_eq!(h.fields[2].bits(), 12);
        // 解析失败必须变成 TOML 反序列化错误（接线后即声明期诊断）。
        let bad: Result<Holder, _> = toml::from_str("fields = [\"u5[7:11]\"]");
        assert!(bad.is_err());
    }

    #[test]
    fn bind_by_same_name_and_by_explicit_map() {
        let decls = vec![
            FieldDecl::parse("gpr64[11:7]:rd").unwrap(),
            FieldDecl::parse("u3[14:12]:funct3").unwrap(),
            FieldDecl::parse("u5[19:15]:rs1").unwrap(),
        ];
        let ops = vec!["dst".to_string(), "rs1".to_string()];
        let mut bind = BTreeMap::new();
        bind.insert("dst".to_string(), "rd".to_string());
        let got = bind_operands(&decls, &ops, &bind).unwrap();
        assert_eq!(got, vec!["rd".to_string(), "rs1".to_string()]);
        // 绑定到一个不存在的字段 → 报错并列出已声明字段。
        let mut bad = BTreeMap::new();
        bad.insert("dst".to_string(), "nope".to_string());
        let msg = bind_operands(&decls, &ops, &bad).unwrap_err();
        assert!(msg.contains("nope") && msg.contains("funct3"), "msg: {msg}");
    }
}
