//! 声明注释（`doc`）的解析与插值（v21，2026-10-08）。
//!
//! 三条声明面，优先级从高到低：
//!
//! 1. `[inst.<名>].doc`（指令自己的）；
//! 2. family 的 `body.doc`——`FamilyExpand::expand` 在展开时把它插值后写进该行的
//!    `Instruction.doc`，所以到这里已经是"指令自己的"，**且行键已经被替换过**；
//! 3. `[form.<名>].doc`（编码预设上的共享注释，`form = "…"` 的指令继承）。
//!
//! 三者都可缺。命中后按下面的**固定小集 + 操作数派生**插值：
//!
//! | 占位符 | 含义 |
//! | --- | --- |
//! | `{name}` | 指令名（`[inst.<名>]` 的表头） |
//! | `{form}` | 编码形式名（该指令没写 `form` ⇒ 报错） |
//! | `{family}` | 族名（`[family.<名>]`；不是族降级来的指令 ⇒ 报错） |
//! | `{width}` / `{data_width}` | 指令字长（位）/ 数据宽度（位）；没写 ⇒ 报错 |
//! | `{ops}` | 操作数名列表（逗号分隔；无操作数 ⇒ 空串） |
//! | `{<操作数名>.slot}` | 该操作数用的槽名 |
//! | `{<操作数名>.width}` | 该槽声明的值宽度（位）；槽没写 `width` ⇒ 报错 |
//! | `{<操作数名>.class}` | 该槽的寄存器类（如 `gpr`/`fpr`）；槽没声明 ⇒ 报错 |
//!
//! `{{` / `}}` 是字面花括号的转义。**未知占位符一律报错**：注释没有别的消费者，
//! 拼错必须当场暴露（不像 `asm` 模板——那里的 `{…}` 可能是内层 DSL 的占位符）。

/// 一个操作数在注释插值里可用的派生事实。
pub(crate) struct DocOp {
    /// `ops` 里声明的操作数名（也是生成物的字段名）。
    pub(crate) name: String,
    /// 槽名。
    pub(crate) slot: String,
    /// 槽声明的值宽度（位）。
    pub(crate) width: Option<u32>,
    /// 槽的寄存器类名（`gpr`/`fpr`/…）。
    pub(crate) class: Option<String>,
}

/// 一条指令在注释插值里可用的事实。
pub(crate) struct DocFacts<'a> {
    pub(crate) name: &'a str,
    pub(crate) form: Option<&'a str>,
    pub(crate) family: Option<&'a str>,
    pub(crate) width: Option<u32>,
    pub(crate) data_width: Option<u16>,
    pub(crate) ops: &'a [DocOp],
}

/// 把 `raw` 里的 `{…}` 换成事实值；未知占位符 / 没配对的花括号 ⇒ 报错。
///
/// `where_` 是诊断前缀（如 `[inst.ADD_RM_R]`）。
pub(crate) fn resolve_doc(
    raw: &str,
    facts: &DocFacts<'_>,
    where_: &str,
) -> Result<String, String> {
    let ch: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0usize;
    while i < ch.len() {
        match ch[i] {
            '{' => {
                if ch.get(i + 1) == Some(&'{') {
                    out.push('{');
                    i += 2;
                    continue;
                }
                let Some(close) = (i + 1..ch.len()).find(|&j| ch[j] == '}') else {
                    return Err(format!(
                        "{where_}: doc 里的 `{{` 没有配对的 `}}`（要写字面花括号请写 `{{{{`）"
                    ));
                };
                let key: String = ch[i + 1..close].iter().collect();
                out.push_str(&lookup(&key, facts, where_)?);
                i = close + 1;
            }
            '}' => {
                if ch.get(i + 1) == Some(&'}') {
                    out.push('}');
                    i += 2;
                    continue;
                }
                return Err(format!(
                    "{where_}: doc 里出现单个 `}}`（要写字面花括号请写 `}}}}`）"
                ));
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

const AVAILABLE: &str = "可用：{name} {form} {family} {width} {data_width} {ops} 与 \
                         {<操作数名>.slot|.width|.class}；字面花括号写 `{{`/`}}`";

fn lookup(key: &str, facts: &DocFacts<'_>, where_: &str) -> Result<String, String> {
    let simple: Option<&str> = match key {
        "name" => Some(facts.name),
        "form" => facts.form,
        "family" => facts.family,
        "ops" => {
            return Ok(facts
                .ops
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>()
                .join(", "));
        }
        _ => None,
    };
    if let Some(v) = simple {
        return Ok(v.to_string());
    }
    match key {
        "width" => {
            return facts.width.map(|w| w.to_string()).ok_or_else(|| {
                format!("{where_}: 占位符 `{{width}}` 用不了——这条指令没写 `width`（指令字长，位）")
            });
        }
        "data_width" => {
            return facts.data_width.map(|w| w.to_string()).ok_or_else(|| {
                format!("{where_}: 占位符 `{{data_width}}` 用不了——这条指令没写 `data_width`")
            });
        }
        _ => {}
    }
    // 操作数派生：`{<操作数名>.<字段>}`
    if let Some((op_name, field)) = key.split_once('.') {
        let op = facts.ops.iter().find(|o| o.name == op_name);
        match (op, field) {
            (Some(o), "slot") => return Ok(o.slot.clone()),
            (Some(o), "width") => {
                return o.width.map(|w| w.to_string()).ok_or_else(|| {
                    format!(
                        "{where_}: 占位符 `{{{key}}}` 用不了——操作数 `{op_name}` 的槽 `{}` 没声明 `width`",
                        o.slot
                    )
                });
            }
            (Some(o), "class") => {
                return o.class.clone().ok_or_else(|| {
                    format!(
                        "{where_}: 占位符 `{{{key}}}` 用不了——操作数 `{op_name}` 的槽 `{}` 没声明 `class`",
                        o.slot
                    )
                });
            }
            (Some(_), other) => {
                return Err(format!(
                    "{where_}: 占位符 `{{{key}}}` 的字段 `{other}` 未知（可用：slot / width / class）"
                ));
            }
            (None, _) => {}
        }
    }
    Err(format!("{where_}: doc 里的占位符 `{{{key}}}` 未知（{AVAILABLE}）"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(ops: &'a [DocOp]) -> DocFacts<'a> {
        DocFacts {
            name: "ADD_RM_R",
            form: Some("RR"),
            family: Some("ALU"),
            width: Some(32),
            data_width: Some(64),
            ops,
        }
    }

    #[test]
    fn simple_and_derived_placeholders_resolve() {
        let ops = vec![
            DocOp {
                name: "dst".into(),
                slot: "gprx".into(),
                width: Some(64),
                class: Some("gpr".into()),
            },
            DocOp {
                name: "src".into(),
                slot: "gprx".into(),
                width: None,
                class: None,
            },
        ];
        let f = facts(&ops);
        assert_eq!(
            resolve_doc("{name} 在 {form}/{family}：{dst.width} 位 {dst.class}（{ops}）", &f, "[inst.X]").unwrap(),
            "ADD_RM_R 在 RR/ALU：64 位 gpr（dst, src）"
        );
        assert_eq!(
            resolve_doc("字宽 {width} / 数据宽 {data_width}", &f, "[inst.X]").unwrap(),
            "字宽 32 / 数据宽 64"
        );
        assert_eq!(resolve_doc("宽 {{字面}} 括号", &f, "[inst.X]").unwrap(), "宽 {字面} 括号");
    }

    #[test]
    fn unknown_and_unavailable_placeholders_fail_closed() {
        let ops = vec![DocOp {
            name: "dst".into(),
            slot: "gprx".into(),
            width: None,
            class: None,
        }];
        let f = facts(&ops);
        let e = resolve_doc("{nope}", &f, "[inst.X]").unwrap_err();
        assert!(e.contains("未知") && e.contains("nope"), "{e}");
        let e = resolve_doc("{dst.width}", &f, "[inst.X]").unwrap_err();
        assert!(e.contains("没声明 `width`"), "{e}");
        let e = resolve_doc("{dst.class}", &f, "[inst.X]").unwrap_err();
        assert!(e.contains("没声明 `class`"), "{e}");
        let e = resolve_doc("{width", &f, "[inst.X]").unwrap_err();
        assert!(e.contains("没有配对"), "{e}");
    }
}
