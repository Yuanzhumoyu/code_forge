//! `[[pattern]].match` 匹配树的递归下降解析器（S6）。
//!
//! 语法：`Fadd(Fmul(a, b), c)`——根必须是 Op 调用，叶是裸标识符变量。
//! Op 名 = forge-ir `Opcode` 的单元变体名（`Fadd`/`Fmul`…）；变量按树
//! **DFS 序**编号 0..（`a`→0、`b`→1、`c`→2），与运行期 `lower_pattern`
//! 收到的叶序、codegen 把模板 `{名字}` 改写成的 `{N}` 一一对应。
//!
//! 本模块**只做纯解析**：产出 [`MatchNode`] 树；变量去重、Opcode 合法性
//!（禁 Fcmp/Icmp/Copy/Nop）等语义校验在 `validate.rs` 完成。

/// 匹配树节点：IR op 节点或叶变量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchNode {
    /// IR op 节点（`Op(arg, …)`）。`name` = Opcode 单元变体名。
    Op { name: String, args: Vec<MatchNode> },
    /// 叶变量（`a`/`b`/`c`…）——绑定一个 IR 值。
    Var(String),
}

/// 解析 `Fadd(Fmul(a, b), c)`：根必须是 Op 调用，变量是裸标识符。
pub fn parse(s: &str) -> Result<MatchNode, String> {
    let mut p = Parser { s, i: 0 };
    let node = p.parse_node()?;
    p.skip_ws();
    if p.i != p.s.len() {
        return Err(format!(
            "匹配树末尾有多余字符 {:?}",
            &p.s[p.i..]
        ));
    }
    if matches!(node, MatchNode::Var(_)) {
        return Err("匹配树根必须是 Op 调用（如 Fadd(Fmul(a,b),c)）".into());
    }
    Ok(node)
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while let Some(&c) = self.s.as_bytes().get(self.i)
            && c.is_ascii_whitespace()
        {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }

    fn parse_ident(&mut self) -> Result<String, String> {
        self.skip_ws();
        let start = self.i;
        while let Some(&c) = self.s.as_bytes().get(self.i)
            && (c.is_ascii_alphanumeric() || c == b'_')
        {
            self.i += 1;
        }
        if self.i == start {
            return Err(format!(
                "期望标识符，位置 {}，got {:?}",
                self.i,
                &self.s[self.i..]
            ));
        }
        Ok(self.s[start..self.i].to_string())
    }

    fn parse_node(&mut self) -> Result<MatchNode, String> {
        let name = self.parse_ident()?;
        self.skip_ws();
        if self.peek() != Some(b'(') {
            return Ok(MatchNode::Var(name));
        }
        self.i += 1; // consume '('
        let mut args = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b')') {
            self.i += 1;
            return Ok(MatchNode::Op { name, args });
        }
        loop {
            let arg = self.parse_node()?;
            args.push(arg);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b')') => {
                    self.i += 1;
                    break;
                }
                _ => {
                    return Err(format!(
                        "期望 ',' 或 ')'，位置 {}，got {:?}",
                        self.i,
                        &self.s[self.i..]
                    ));
                }
            }
        }
        Ok(MatchNode::Op { name, args })
    }
}

/// 按 DFS 序收集叶变量名（重复出现不折叠——去重/查重在 validate 做）。
pub fn leaf_vars(node: &MatchNode, out: &mut Vec<String>) {
    match node {
        MatchNode::Op { args, .. } => args.iter().for_each(|a| leaf_vars(a, out)),
        MatchNode::Var(name) => out.push(name.clone()),
    }
}

/// Op 节点总数（root + 内部）——同根多形状模式的优先序（大者先试）。
pub fn op_node_count(node: &MatchNode) -> usize {
    match node {
        MatchNode::Op { args, .. } => 1 + args.iter().map(op_node_count).sum::<usize>(),
        MatchNode::Var(_) => 0,
    }
}

/// 深度优先遍历所有 Op 节点的名字（校验 Opcode 合法性用）。
pub fn op_names(node: &MatchNode, out: &mut Vec<String>) {
    if let MatchNode::Op { name, args } = node {
        out.push(name.clone());
        args.iter().for_each(|a| op_names(a, out));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_anchor_shape() {
        let t = parse("Fadd(Fmul(a, b), c)").unwrap();
        assert_eq!(
            t,
            MatchNode::Op {
                name: "Fadd".into(),
                args: vec![
                    MatchNode::Op {
                        name: "Fmul".into(),
                        args: vec![MatchNode::Var("a".into()), MatchNode::Var("b".into())],
                    },
                    MatchNode::Var("c".into()),
                ],
            }
        );
        let mut vars = Vec::new();
        leaf_vars(&t, &mut vars);
        assert_eq!(vars, ["a", "b", "c"]);
        assert_eq!(op_node_count(&t), 2);
    }

    #[test]
    fn parse_unary_and_whitespace() {
        let t = parse("  Fneg ( a ) ").unwrap();
        assert_eq!(
            t,
            MatchNode::Op {
                name: "Fneg".into(),
                args: vec![MatchNode::Var("a".into())],
            }
        );
    }

    #[test]
    fn rejects_var_root() {
        assert!(parse("a").is_err());
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse("Fadd(a, b) x").is_err());
        assert!(parse("Fadd(a, b))").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn rejects_bad_separator() {
        assert!(parse("Fadd(a b)").is_err());
    }
}
