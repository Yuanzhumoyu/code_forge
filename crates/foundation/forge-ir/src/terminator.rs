//! 基本块终止指令。
//!
//! 使用 Block Parameters 替代传统的 Phi 指令:
//! - Jump/Branch 携带跳转参数 (args)
//! - 目标块通过 block params 接收这些参数
//! - 不需要 Phi 指令

use super::entity::{Block, FuncRef, TypeId, Value};
use crate::metadata::AttachedMetadata;
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
        /// 附加 metadata（`br ..., !prof !0`；文本层 parse 填、display 输出）。
        metadata: SmallVec<[AttachedMetadata; 2]>,
    },

    /// 无条件跳转: → target(args)
    Jump {
        target: Block,
        args: SmallVec<[Value; 2]>,
        metadata: SmallVec<[AttachedMetadata; 2]>,
    },

    /// 函数返回
    Return {
        values: SmallVec<[Value; 2]>,
        metadata: SmallVec<[AttachedMetadata; 2]>,
    },

    /// 多路分支 (switch / jump table)
    Switch {
        discriminant: Value,
        /// Default target with block arguments (consistent with Jump/Branch semantics).
        default_block: Block,
        default_args: SmallVec<[Value; 2]>,
        #[allow(clippy::type_complexity)]
        cases: SmallVec<[(i64, Block, SmallVec<[Value; 2]>); 4]>,
        metadata: SmallVec<[AttachedMetadata; 2]>,
    },

    /// invoke 调用（LLVM `invoke <retty> @f(args) to label %ok unwind label %pad`）：
    /// 正常返回走 normal 块（返回值经 normal_args 传块参数），异常走 unwind 块。
    /// 文本层 P1.1：仅解析/展示；codegen 对含 invoke 函数报 Unsupported。
    Invoke {
        callee: FuncRef,
        args: SmallVec<[Value; 4]>,
        /// 返回类型（void = 无返回值）。
        ret_ty: TypeId,
        normal_block: Block,
        normal_args: SmallVec<[Value; 2]>,
        unwind_block: Block,
        unwind_args: SmallVec<[Value; 2]>,
        metadata: SmallVec<[AttachedMetadata; 2]>,
    },

    /// 重新抛出（LLVM `resume <ty> %l`；landingpad 结果传入）。终结符，无后继。
    Resume {
        value: Value,
        metadata: SmallVec<[AttachedMetadata; 2]>,
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
            Terminator::Invoke {
                normal_block,
                unwind_block,
                ..
            } => vec![*normal_block, *unwind_block],
            Terminator::Return { .. } | Terminator::Resume { .. } | Terminator::Unreachable => {
                Vec::new()
            }
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
            Terminator::Return { values, .. } => values.to_vec(),
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
            Terminator::Invoke {
                args,
                normal_args,
                unwind_args,
                ..
            } => {
                let mut v = args.to_vec();
                v.extend_from_slice(normal_args);
                v.extend_from_slice(unwind_args);
                v
            }
            Terminator::Resume { value, .. } => vec![*value],
            Terminator::Unreachable => Vec::new(),
        }
    }

    /// 终结符传给 `target` 块的参数切片（不跳向该块则空）。
    pub fn args_to(&self, target: Block) -> &[Value] {
        match self {
            Terminator::Branch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if *then_block == target {
                    then_args
                } else if *else_block == target {
                    else_args
                } else {
                    &[]
                }
            }
            Terminator::Jump {
                target: t, args, ..
            } if *t == target => args,
            Terminator::Switch {
                default_block,
                default_args,
                cases,
                ..
            } => {
                if *default_block == target {
                    default_args
                } else {
                    for (_, cb, cargs) in cases.iter() {
                        if *cb == target {
                            return cargs;
                        }
                    }
                    &[]
                }
            }
            Terminator::Invoke {
                normal_block,
                normal_args,
                unwind_block,
                unwind_args,
                ..
            } => {
                if *normal_block == target {
                    normal_args
                } else if *unwind_block == target {
                    unwind_args
                } else {
                    &[]
                }
            }
            _ => &[],
        }
    }
    /// 删除传给 `target` 块的第 `idx` 个参数（与块参数删除配套）。
    pub fn remove_arg(&mut self, target: Block, idx: usize) {
        match self {
            Terminator::Branch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if *then_block == target && idx < then_args.len() {
                    then_args.remove(idx);
                } else if *else_block == target && idx < else_args.len() {
                    else_args.remove(idx);
                }
            }
            Terminator::Jump {
                target: t, args, ..
            } if *t == target && idx < args.len() => {
                args.remove(idx);
            }
            Terminator::Switch {
                default_block,
                default_args,
                cases,
                ..
            } => {
                if *default_block == target && idx < default_args.len() {
                    default_args.remove(idx);
                }
                for (_, cb, cargs) in cases.iter_mut() {
                    if *cb == target && idx < cargs.len() {
                        cargs.remove(idx);
                    }
                }
            }
            Terminator::Invoke {
                normal_block,
                normal_args,
                unwind_block,
                unwind_args,
                ..
            } => {
                if *normal_block == target && idx < normal_args.len() {
                    normal_args.remove(idx);
                } else if *unwind_block == target && idx < unwind_args.len() {
                    unwind_args.remove(idx);
                }
            }
            _ => {}
        }
    }

    /// 把终结符中的 `old_target` 目标重定向到 `new_target`（参数原样保留）。
    pub fn retarget(&mut self, old_target: Block, new_target: Block) {
        match self {
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => {
                if *then_block == old_target {
                    *then_block = new_target;
                }
                if *else_block == old_target {
                    *else_block = new_target;
                }
            }
            Terminator::Jump { target, .. } => {
                if *target == old_target {
                    *target = new_target;
                }
            }
            Terminator::Switch {
                default_block,
                cases,
                ..
            } => {
                if *default_block == old_target {
                    *default_block = new_target;
                }
                for (_, cb, _) in cases.iter_mut() {
                    if *cb == old_target {
                        *cb = new_target;
                    }
                }
            }
            Terminator::Invoke {
                normal_block,
                unwind_block,
                ..
            } => {
                if *normal_block == old_target {
                    *normal_block = new_target;
                }
                if *unwind_block == old_target {
                    *unwind_block = new_target;
                }
            }
            _ => {}
        }
    }

    /// 把传给 `target` 块的参数整体替换为 `args`（不跳向该块则无操作）。
    pub fn replace_args(&mut self, target: Block, args: SmallVec<[Value; 2]>) {
        match self {
            Terminator::Branch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if *then_block == target {
                    *then_args = args.clone();
                }
                if *else_block == target {
                    *else_args = args.clone();
                }
            }
            Terminator::Jump {
                target: t, args: a, ..
            } if *t == target => {
                *a = args;
            }
            Terminator::Switch {
                default_block,
                default_args,
                cases,
                ..
            } => {
                if *default_block == target {
                    *default_args = args.clone();
                }
                for (_, cb, cargs) in cases.iter_mut() {
                    if *cb == target {
                        *cargs = args.clone();
                    }
                }
            }
            Terminator::Invoke {
                normal_block,
                normal_args,
                unwind_block,
                unwind_args,
                ..
            } => {
                if *normal_block == target {
                    *normal_args = args.clone();
                }
                if *unwind_block == target {
                    *unwind_args = args.clone();
                }
            }
            _ => {}
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
            metadata: SmallVec::new(),
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
            metadata: SmallVec::new(),
        };
        assert_eq!(t.successors(), vec![then, els]);
    }

    #[test]
    fn test_return_no_successors() {
        let t = Terminator::Return {
            values: SmallVec::new(),
            metadata: SmallVec::new(),
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
            metadata: SmallVec::new(),
        };
        let succs = t.successors();
        assert!(succs.contains(&default));
        assert!(succs.contains(&case1));
        assert!(succs.contains(&case2));
    }

    #[test]
    fn test_args_to_and_remove_arg() {
        let then = make_block(1);
        let els = make_block(2);
        let mut t = Terminator::Branch {
            cond: make_value(0),
            then_block: then,
            then_args: smallvec::smallvec![make_value(10), make_value(11)],
            else_block: els,
            else_args: smallvec::smallvec![make_value(20)],
            metadata: SmallVec::new(),
        };
        // args_to 只取对应分支
        assert_eq!(t.args_to(then), &[make_value(10), make_value(11)]);
        assert_eq!(t.args_to(els), &[make_value(20)]);
        assert_eq!(t.args_to(make_block(9)), &[] as &[Value]);
        // remove_arg
        t.remove_arg(then, 0);
        assert_eq!(t.args_to(then), &[make_value(11)]);
        t.remove_arg(els, 0);
        assert!(t.args_to(els).is_empty());
    }

    #[test]
    fn test_map_values_and_retarget() {
        let a = make_value(1);
        let b = make_value(2);
        let t1 = make_block(1);
        let t2 = make_block(2);
        let mut t = Terminator::Branch {
            cond: a,
            then_block: t1,
            then_args: smallvec::smallvec![b],
            else_block: t1,
            else_args: smallvec::smallvec![],
            metadata: SmallVec::new(),
        };
        // retarget：t1 → t2（两分支都重定向）
        t.retarget(t1, t2);
        assert!(t.successors().iter().all(|&s| s == t2));
        assert_eq!(t.args_to(t2), &[make_value(2)]);
    }
}
