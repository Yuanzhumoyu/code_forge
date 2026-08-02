//! 基本块终止指令。
//!
//! 使用 Block Parameters 替代传统的 Phi 指令:
//! - Jump/Branch 携带跳转参数 (args)
//! - 目标块通过 block params 接收这些参数
//! - 不需要 Phi 指令

use super::entity::{Block, Value};
use smallvec::SmallVec;

/// 基本块终止指令。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Terminator {
    /// 条件分支: cond ? then_block(then_args) : else_block(else_args)
    Branch {
        cond: Value,
        then_block: Block,
        then_args: SmallVec<[Value; 2]>,
        else_block: Block,
        else_args: SmallVec<[Value; 2]>,
    },

    /// 无条件跳转: → target(args)
    Jump {
        target: Block,
        args: SmallVec<[Value; 2]>,
    },

    /// 函数返回
    Return { values: SmallVec<[Value; 2]> },

    /// 多路分支 (switch / jump table)
    Switch {
        discriminant: Value,
        /// Default target with block arguments (consistent with Jump/Branch semantics).
        default_block: Block,
        default_args: SmallVec<[Value; 2]>,
        #[allow(clippy::type_complexity)]
        cases: SmallVec<[(i64, Block, SmallVec<[Value; 2]>); 4]>,
    },

    /// 不可达 (死代码、未完成的构建等)
    #[default]
    Unreachable,
}

impl Terminator {
    /// 返回此终止指令的所有后继块。
    pub fn successors(&self) -> Vec<Block> {
        match self {
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            Terminator::Jump { target, .. } => vec![*target],
            Terminator::Switch {
                default_block,
                cases,
                ..
            } => {
                let mut succs = vec![*default_block];
                for (_, target, _) in cases {
                    if succs.last() != Some(target) {
                        succs.push(*target);
                    }
                }
                succs
            }
            Terminator::Return { .. } | Terminator::Unreachable => Vec::new(),
        }
    }

    /// 返回此终止指令中使用的所有值。
    pub fn used_values(&self) -> Vec<Value> {
        match self {
            Terminator::Branch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                let mut v = vec![*cond];
                v.extend_from_slice(then_args);
                v.extend_from_slice(else_args);
                v
            }
            Terminator::Jump { args, .. } => args.to_vec(),
            Terminator::Return { values } => values.to_vec(),
            Terminator::Switch {
                discriminant,
                default_args,
                cases,
                ..
            } => {
                let mut v = vec![*discriminant];
                v.extend_from_slice(default_args);
                for (_, _, args) in cases {
                    v.extend_from_slice(args);
                }
                v
            }
            Terminator::Unreachable => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::SmallVec;

    fn make_value(n: u32) -> Value {
        Value(n)
    }
    fn make_block(n: u32) -> Block {
        Block(n)
    }

    #[test]
    fn test_jump_successors() {
        let target = make_block(1);
        let t = Terminator::Jump {
            target,
            args: SmallVec::new(),
        };
        assert_eq!(t.successors(), vec![target]);
    }

    #[test]
    fn test_branch_successors() {
        let then = make_block(1);
        let els = make_block(2);
        let t = Terminator::Branch {
            cond: make_value(0),
            then_block: then,
            then_args: SmallVec::new(),
            else_block: els,
            else_args: SmallVec::new(),
        };
        assert_eq!(t.successors(), vec![then, els]);
    }

    #[test]
    fn test_return_no_successors() {
        let t = Terminator::Return {
            values: SmallVec::new(),
        };
        assert!(t.successors().is_empty());
    }

    #[test]
    fn test_unreachable_no_successors() {
        let t = Terminator::Unreachable;
        assert!(t.successors().is_empty());
    }

    #[test]
    fn test_switch_successors() {
        let default = make_block(1);
        let case1 = make_block(2);
        let case2 = make_block(3);
        let t = Terminator::Switch {
            discriminant: make_value(0),
            default_block: default,
            default_args: SmallVec::new(),
            cases: smallvec::smallvec![(0, case1, SmallVec::new()), (1, case2, SmallVec::new()),],
        };
        let succs = t.successors();
        assert!(succs.contains(&default));
        assert!(succs.contains(&case1));
        assert!(succs.contains(&case2));
    }
}
