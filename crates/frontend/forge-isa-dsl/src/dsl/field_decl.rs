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
                let w = parse_span(body.trim()).map_err(|e| format!("{e}（`{s}`）"))?;
                (w, vec![w])
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

    /// 降级成内部 [`Bitfield`]：连续 → `offset/width`；散射 → `pieces`
    ///（`shift` = 值偏移，`offset`/`width` = 词位置）。
    pub fn to_bitfield(&self) -> Bitfield {
        if self.chunks.len() == 1 {
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

#[cfg(test)]
mod tests {
    use super::*;

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
