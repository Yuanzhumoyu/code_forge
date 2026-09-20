//! 结构化谓词（迭代 4 框架）——纯 TOML 数据，替代 v11 的 `when` 字符串。
//!
//! 语法：`{ and = [ <pred>, ... ] }` / `{ or = [...] }` / `{ not = <pred> }` /
//! `{ eq = [属性, 值] }`（eq/ne/lt/le/gt/ge）。
//!
//! 求值上下文 = 属性名 → i64 值（如 "rs1_width"、"elem" 的类型 id、"rd"）。
//! 未知属性求值为 false（保守）。lowering 接入在迭代 5；本模块提供解析 + 求值。

/// 结构化谓词 AST。
#[derive(Debug, Clone, PartialEq)]
pub enum Pred {
    /// `{ and = [ ... ] }`：全部为真。
    And(Vec<Pred>),
    /// `{ or = [ ... ] }`：任一为真。
    Or(Vec<Pred>),
    /// `{ not = ... }`：取反。
    Not(Box<Pred>),
    /// `{ eq = [attr, value] }` 等比较。
    Cmp(CmpOp, String, i64),
    /// `{ in = [attr, [v1, v2, …]] }`：集合成员。
    ///
    /// 消灭"同 op 同 insts、只有 when 不同"的纯重复规则：x86 原有 33 组
    /// 66 条这类规则，绝大多数是同一份 insts 被拆成 `elem == 1` 与
    /// `elem == 3` 两条（因为 `when` 没有集合语义）。
    In(String, Vec<i64>),
}

/// 谓词可用的属性名——**与 `codegen/integration.rs::gen_lowering_attrs` 的
/// `__attr` 分派表逐项对应**（唯一事实源在此，生成器与校验器同读一份）。
///
/// 校验意义：`eval` 对未知属性返回 false（保守），于是 `when` 里写错属性名的
/// 规则**永远不命中**——既不报错也不生效。这类静默失效必须在编译期拒绝，
/// 见 `validate_lowering`。
pub const PRED_ATTRS: &[&str] = &[
    "rd",
    "rs1_width",
    "rs2_width",
    "rd_vec",
    "rs1_vec",
    "elem",
    "cond",
    "imm0",
    // `iconst`：当前指令常量池解析的真值（signed i64）。Iconst 的
    // `imm0` 是 ConstId 池索引（正数），判符号/大小必须用此池解析值。
    "iconst",
];

/// 收集谓词里出现的全部属性名（校验用）。
pub fn attrs_of(pred: &Pred, out: &mut Vec<String>) {
    match pred {
        Pred::And(ps) | Pred::Or(ps) => ps.iter().for_each(|p| attrs_of(p, out)),
        Pred::Not(p) => attrs_of(p, out),
        Pred::Cmp(_, a, _) | Pred::In(a, _) => out.push(a.clone()),
    }
}

/// 谓词叶子（比较/集合）个数——特异性度量：叶子越多越具体。
pub fn leaf_count(pred: &Pred) -> usize {
    match pred {
        Pred::And(ps) | Pred::Or(ps) => ps.iter().map(leaf_count).sum(),
        Pred::Not(p) => leaf_count(p),
        Pred::Cmp(..) | Pred::In(..) => 1,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn parse(s: &str) -> Option<CmpOp> {
        match s {
            "eq" => Some(CmpOp::Eq),
            "ne" => Some(CmpOp::Ne),
            "lt" => Some(CmpOp::Lt),
            "le" => Some(CmpOp::Le),
            "gt" => Some(CmpOp::Gt),
            "ge" => Some(CmpOp::Ge),
            _ => None,
        }
    }
}

/// 解析结构化谓词 TOML 值。
pub fn parse(value: &toml::Value) -> Result<Pred, String> {
    let table = value
        .as_table()
        .ok_or_else(|| format!("predicate must be a table, got {value:?}"))?;
    if table.len() != 1 {
        return Err(format!(
            "predicate table must have exactly one operator key (and/or/not/in/eq/ne/lt/le/gt/ge), got {} keys",
            table.len()
        ));
    }
    let (key, val) = table.iter().next().unwrap();
    match key.as_str() {
        "and" => Ok(Pred::And(parse_list(val)?)),
        "or" => Ok(Pred::Or(parse_list(val)?)),
        "not" => Ok(Pred::Not(Box::new(parse(val)?))),
        "in" => parse_in(val),
        op => {
            let cmp =
                CmpOp::parse(op).ok_or_else(|| format!("unknown predicate operator '{op}'"))?;
            parse_cmp(cmp, val)
        }
    }
}

/// `in = [attr, [v1, v2, …]]`。
fn parse_in(v: &toml::Value) -> Result<Pred, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| format!("`in` must be [attr, [values…]], got {v:?}"))?;
    if arr.len() != 2 {
        return Err(format!(
            "`in` must be [attr, [values…]], got {} elements",
            arr.len()
        ));
    }
    let attr = arr[0]
        .as_str()
        .ok_or_else(|| format!("`in` attr must be a string, got {:?}", arr[0]))?;
    let vals = arr[1]
        .as_array()
        .ok_or_else(|| format!("`in` values must be an array, got {:?}", arr[1]))?;
    if vals.is_empty() {
        return Err("`in` values must not be empty（空集合恒假，规则永不命中）".into());
    }
    let mut out = Vec::with_capacity(vals.len());
    for v in vals {
        out.push(
            v.as_integer()
                .ok_or_else(|| format!("`in` value must be an integer, got {v:?}"))?,
        );
    }
    Ok(Pred::In(attr.to_string(), out))
}

fn parse_list(v: &toml::Value) -> Result<Vec<Pred>, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| format!("and/or must be an array, got {v:?}"))?;
    arr.iter().map(parse).collect()
}

fn parse_cmp(op: CmpOp, v: &toml::Value) -> Result<Pred, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| format!("comparison must be [attr, value], got {v:?}"))?;
    if arr.len() != 2 {
        return Err(format!(
            "comparison must be [attr, value], got {} elements",
            arr.len()
        ));
    }
    let attr = arr[0]
        .as_str()
        .ok_or_else(|| format!("comparison attr must be a string, got {:?}", arr[0]))?;
    let val = arr[1]
        .as_integer()
        .ok_or_else(|| format!("comparison value must be an integer, got {:?}", arr[1]))?;
    Ok(Pred::Cmp(op, attr.to_string(), val))
}

/// 求值：`attrs(attr) -> Option<i64>`；未知属性/缺属性 → false。
pub fn eval(pred: &Pred, attrs: &dyn Fn(&str) -> Option<i64>) -> bool {
    match pred {
        Pred::And(ps) => ps.iter().all(|p| eval(p, attrs)),
        Pred::Or(ps) => ps.iter().any(|p| eval(p, attrs)),
        Pred::Not(p) => !eval(p, attrs),
        Pred::Cmp(op, attr, want) => {
            let Some(got) = attrs(attr) else {
                return false;
            };
            match op {
                CmpOp::Eq => got == *want,
                CmpOp::Ne => got != *want,
                CmpOp::Lt => got < *want,
                CmpOp::Le => got <= *want,
                CmpOp::Gt => got > *want,
                CmpOp::Ge => got >= *want,
            }
        }
        Pred::In(attr, vals) => attrs(attr).is_some_and(|got| vals.contains(&got)),
    }
}

// ─────────────────── 区间域（死规则/重复判定） ───────────────────

/// 单属性的允许值集合：升序、互不相交的闭区间列表。
type Ranges = Vec<(i64, i64)>;

/// 全域（属性未被约束）。
fn full() -> Ranges {
    vec![(i64::MIN, i64::MAX)]
}

/// 归一化：排序 + 合并相邻/重叠区间。
fn norm(mut rs: Ranges) -> Ranges {
    rs.retain(|(a, b)| a <= b);
    rs.sort_unstable();
    let mut out: Ranges = Vec::with_capacity(rs.len());
    for (a, b) in rs {
        match out.last_mut() {
            // 相邻（b+1 == a）也合并；用 saturating 防 i64::MAX 溢出
            Some((_, pb)) if a <= pb.saturating_add(1) => *pb = (*pb).max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn intersect(x: &Ranges, y: &Ranges) -> Ranges {
    let mut out = Vec::new();
    for &(a, b) in x {
        for &(c, d) in y {
            let lo = a.max(c);
            let hi = b.min(d);
            if lo <= hi {
                out.push((lo, hi));
            }
        }
    }
    norm(out)
}

/// `x ⊇ y`？（y 的每个区间都被 x 覆盖）
fn covers(x: &Ranges, y: &Ranges) -> bool {
    y.iter()
        .all(|&(c, d)| x.iter().any(|&(a, b)| a <= c && d <= b))
}

/// 一条规则的约束：属性 → 允许值区间集；**缺席 = 全域**。
///
/// 只对"纯合取"（`and` 嵌 `eq/ne/lt/le/gt/ge/in`）可判定；出现 `or`/`not`
/// 一律返回 `None`（放弃判定，保守——宁可漏报也不误报死规则）。
pub type Constraints = std::collections::BTreeMap<String, Ranges>;

pub fn constraints(pred: &Pred) -> Option<Constraints> {
    let mut out = Constraints::new();
    fn walk(p: &Pred, out: &mut Constraints) -> Option<()> {
        match p {
            Pred::And(ps) => {
                for q in ps {
                    walk(q, out)?;
                }
                Some(())
            }
            Pred::Or(_) | Pred::Not(_) => None,
            Pred::Cmp(op, a, v) => {
                let r = match op {
                    CmpOp::Eq => vec![(*v, *v)],
                    CmpOp::Ne => vec![
                        (i64::MIN, v.saturating_sub(1)),
                        (v.saturating_add(1), i64::MAX),
                    ],
                    CmpOp::Lt => vec![(i64::MIN, v.saturating_sub(1))],
                    CmpOp::Le => vec![(i64::MIN, *v)],
                    CmpOp::Gt => vec![(v.saturating_add(1), i64::MAX)],
                    CmpOp::Ge => vec![(*v, i64::MAX)],
                };
                let cur = out.remove(a).unwrap_or_else(full);
                out.insert(a.clone(), intersect(&cur, &norm(r)));
                Some(())
            }
            Pred::In(a, vs) => {
                let r = norm(vs.iter().map(|v| (*v, *v)).collect());
                let cur = out.remove(a).unwrap_or_else(full);
                out.insert(a.clone(), intersect(&cur, &r));
                Some(())
            }
        }
    }
    walk(pred, &mut out).map(|()| out)
}

/// 一条规则的取值域。
#[derive(Debug, Clone, PartialEq)]
pub enum RuleDomain {
    /// 无 `when`：全域，接受一切。
    Any,
    /// 纯合取（`and` 嵌 `eq/ne/lt/le/gt/ge/in`）：属性 → 允许区间集，
    /// **缺席属性 = 全域**。
    Conj(Constraints),
    /// 含 `or`/`not`：放弃判定（保守——宁可漏报也不误报死规则）。
    Opaque,
}

/// 谓词 → 取值域。
pub fn domain_of(pred: Option<&Pred>) -> RuleDomain {
    match pred {
        None => RuleDomain::Any,
        Some(p) => match constraints(p) {
            Some(c) => RuleDomain::Conj(c),
            None => RuleDomain::Opaque,
        },
    }
}

/// `a ⊇ b`？——a 接受的取值集合包含 b 的全部取值。若成立且 a 排在 b 之前，
/// 则 b 是**死规则**（永远轮不到）。任一侧 `Opaque` → false（不报）。
pub fn subsumes(a: &RuleDomain, b: &RuleDomain) -> bool {
    match (a, b) {
        (RuleDomain::Opaque, _) | (_, RuleDomain::Opaque) => false,
        (RuleDomain::Any, _) => true,
        (RuleDomain::Conj(ac), RuleDomain::Any) => ac.values().all(|r| covers(r, &full())),
        (RuleDomain::Conj(ac), RuleDomain::Conj(bc)) => {
            let keys: std::collections::BTreeSet<&String> = ac.keys().chain(bc.keys()).collect();
            keys.into_iter().all(|k| {
                let av = ac.get(k).cloned().unwrap_or_else(full);
                let bv = bc.get(k).cloned().unwrap_or_else(full);
                covers(&av, &bv)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(s: &str) -> Result<Pred, String> {
        parse(&toml::from_str(s).unwrap())
    }

    fn ctx() -> impl Fn(&str) -> Option<i64> {
        |a| match a {
            "rs1_width" => Some(256),
            "elem" => Some(1), // F32
            "rd" => Some(8),
            _ => None,
        }
    }

    #[test]
    fn parse_and_eval_basic() {
        let p = parse_str(r#"eq = ["rs1_width", 256]"#).unwrap();
        assert_eq!(p, Pred::Cmp(CmpOp::Eq, "rs1_width".into(), 256));
        assert!(eval(&p, &ctx()));
        let p2 =
            parse_str(r#"and = [ { eq = ["rs1_width", 256] }, { eq = ["elem", 1] } ]"#).unwrap();
        assert!(eval(&p2, &ctx()));
        let p3 = parse_str(r#"not = { lt = ["rs1_width", 128] }"#).unwrap();
        assert!(eval(&p3, &ctx()));
    }

    #[test]
    fn parse_or_and_missing_attr() {
        let p = parse_str(r#"or = [ { eq = ["unknown_attr", 1] }, { ge = ["rd", 8] } ]"#).unwrap();
        // unknown_attr → false；rd=8 ≥ 8 → true
        assert!(eval(&p, &ctx()));
        let p2 = parse_str(r#"eq = ["missing", 1]"#).unwrap();
        assert!(!eval(&p2, &ctx()));
    }

    #[test]
    fn parse_errors() {
        assert!(parse_str(r#"bad = [1, 2]"#).is_err());
        assert!(parse_str(r#"and = { eq = ["a", 1] }"#).is_err());
        assert!(parse_str(r#"eq = ["a"]"#).is_err());
        assert!(parse_str(r#"eq = ["a", "b"]"#).is_err());
        assert!(parse_str(r#"x = 42"#).is_err());
    }
}
