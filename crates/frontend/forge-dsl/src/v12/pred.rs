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
];

/// 收集谓词里出现的全部属性名（校验用）。
pub fn attrs_of(pred: &Pred, out: &mut Vec<String>) {
    match pred {
        Pred::And(ps) | Pred::Or(ps) => ps.iter().for_each(|p| attrs_of(p, out)),
        Pred::Not(p) => attrs_of(p, out),
        Pred::Cmp(_, a, _) => out.push(a.clone()),
    }
}

/// 谓词叶子（比较）个数——特异性度量：叶子越多越具体。
pub fn leaf_count(pred: &Pred) -> usize {
    match pred {
        Pred::And(ps) | Pred::Or(ps) => ps.iter().map(leaf_count).sum(),
        Pred::Not(p) => leaf_count(p),
        Pred::Cmp(..) => 1,
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
            "predicate table must have exactly one operator key (and/or/not/eq/ne/lt/le/gt/ge), got {} keys",
            table.len()
        ));
    }
    let (key, val) = table.iter().next().unwrap();
    match key.as_str() {
        "and" => Ok(Pred::And(parse_list(val)?)),
        "or" => Ok(Pred::Or(parse_list(val)?)),
        "not" => Ok(Pred::Not(Box::new(parse(val)?))),
        op => {
            let cmp =
                CmpOp::parse(op).ok_or_else(|| format!("unknown predicate operator '{op}'"))?;
            parse_cmp(cmp, val)
        }
    }
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
