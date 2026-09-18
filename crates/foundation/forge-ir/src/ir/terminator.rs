//! 基本块终止指令（v3 方案 S4 主体起：**终结符就是一条指令**）。
//!
//! 历史：终结符曾是独立枚举 `Terminator`，存在 `BlockData.terminator` 字段里，
//! 与指令流平行——于是有了"指令操作数 / 终结符实参"两套 use-def、两套遍历序、
//! 两套写入口。S4 主体把它并入指令流：
//!
//! - 终结符的载体是**一条普通指令**（opcode ∈ {`Ret`, `Jmp`, `Br`, `Switch`,
//!   `Unreachable`, `Invoke`, `Resume`}），存 `DataFlowGraph::insts`，
//!   由 `BlockData.terminator: Option<Inst>` 引用；
//! - 块实参就是这条指令的 **operands**，目标块是它的 **immediates**
//!   （编码约定见 `ops.toml` 的「终结符」节与 `dfg.rs` 的投影访问器）；
//! - 它**不进 `inst_order`**：块内指令列表仍然只含非终结符指令，
//!   因此"遍历块内指令"的既有语义与全部迭代点不变；
//! - 由此 use-def 天然完整（终结符操作数就是普通操作数），
//!   `Terminator`/`UseSite::Term`/两套遍历序/两套写入口全部消失。
//!
//! 本模块只保留**判别**（[`TermKind`]）：种类是无载荷的，用于读取方分派。

use crate::ir::opcode::Opcode;

/// 终结符种类判别（**无载荷**）。
///
/// 读取方只依赖"种类 + 投影访问器"（`DataFlowGraph::term_branch` 等），
/// 不直接 match 终结符的编码形态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermKind {
    Branch,
    Jump,
    Return,
    Switch,
    Invoke,
    Resume,
    Unreachable,
}

impl TermKind {
    /// 该种类是否可能有多于一个后继（CFG 形状查询用）。
    pub fn has_multiple_successors(self) -> bool {
        matches!(self, TermKind::Branch | TermKind::Switch | TermKind::Invoke)
    }
}

/// `opcode` 是否为终结符指令；是则给出其种类。
///
/// **唯一映射**：`dfg` 的编码/解码与 `Function` 的按形式写入口都以它为准。
pub fn term_kind_of(opcode: &Opcode) -> Option<TermKind> {
    Some(match opcode {
        Opcode::Ret => TermKind::Return,
        Opcode::Jmp => TermKind::Jump,
        Opcode::Br => TermKind::Branch,
        Opcode::Switch => TermKind::Switch,
        Opcode::Unreachable => TermKind::Unreachable,
        Opcode::Invoke => TermKind::Invoke,
        Opcode::Resume => TermKind::Resume,
        _ => return None,
    })
}

/// `opcode` 是否为终结符指令。
pub fn is_terminator_opcode(opcode: &Opcode) -> bool {
    term_kind_of(opcode).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 种类判别与实际 opcode 一致（切换表示时这条最容易悄悄漂移）。
    #[test]
    fn term_kind_matches_opcode() {
        assert_eq!(term_kind_of(&Opcode::Ret), Some(TermKind::Return));
        assert_eq!(term_kind_of(&Opcode::Jmp), Some(TermKind::Jump));
        assert_eq!(term_kind_of(&Opcode::Br), Some(TermKind::Branch));
        assert_eq!(term_kind_of(&Opcode::Switch), Some(TermKind::Switch));
        assert_eq!(
            term_kind_of(&Opcode::Unreachable),
            Some(TermKind::Unreachable)
        );
        assert_eq!(term_kind_of(&Opcode::Invoke), Some(TermKind::Invoke));
        assert_eq!(term_kind_of(&Opcode::Resume), Some(TermKind::Resume));
        assert_eq!(term_kind_of(&Opcode::Iadd), None);
        assert_eq!(term_kind_of(&Opcode::Nop), None);
        assert!(!is_terminator_opcode(&Opcode::Call));
    }

    /// `ops.toml` 里声明的终结符类别与判别表**等势**：加了终结符 opcode 却漏改
    /// 判别表（或反之）都会在这里失败。
    #[test]
    fn terminator_category_matches_kind_table() {
        let mut from_category: Vec<&'static str> = Vec::new();
        let mut from_table: Vec<&'static str> = Vec::new();
        for op in Opcode::ALL {
            if op.info().category == "terminator" {
                from_category.push(op.name());
            }
            if is_terminator_opcode(op) {
                from_table.push(op.name());
            }
        }
        from_category.sort_unstable();
        from_table.sort_unstable();
        assert_eq!(
            from_category, from_table,
            "ops.toml 的 terminator 类别与 term_kind_of 判别表不一致"
        );
        assert_eq!(from_table.len(), 7, "终结符只应有 7 个");
    }

    #[test]
    fn multiple_successor_kinds() {
        assert!(TermKind::Branch.has_multiple_successors());
        assert!(TermKind::Switch.has_multiple_successors());
        assert!(TermKind::Invoke.has_multiple_successors());
        assert!(!TermKind::Jump.has_multiple_successors());
        assert!(!TermKind::Return.has_multiple_successors());
        assert!(!TermKind::Resume.has_multiple_successors());
        assert!(!TermKind::Unreachable.has_multiple_successors());
    }
}
