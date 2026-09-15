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

/// 终结符用值的**唯一遍历模板**（v3 方案 S4-a）。
///
/// 共享遍历 [`Terminator::for_each_value`] 与可变遍历
/// [`Terminator::for_each_value_mut`] 由同一个模板展开：两份 match 不可能各自
/// 漂移，因此"记录 use 项时的下标序"与"改写时的下标序"**结构性一致**
/// （这是 [`crate::use_list::Use::operand_idx`] 的契约）。
///
/// - `$t`：`&Terminator` 或 `&mut Terminator`（默认绑定模式决定 `$slot` 类型）；
/// - `$slot`：绑定名——作为元变量传入 ⇒ 调用点卫生，调用方可在 `$body` 里直接引用；
/// - `$iter`：`iter`（共享）或 `iter_mut`（可变）；
/// - `$body`：对每个用值执行的语句块。
macro_rules! walk_terminator_values {
    ($t:expr, $slot:ident, $iter:ident, $body:block) => {
        match $t {
            Terminator::Branch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                let $slot = cond;
                $body
                for $slot in then_args.$iter() {
                    $body
                }
                for $slot in else_args.$iter() {
                    $body
                }
            }
            Terminator::Jump { args, .. } => {
                for $slot in args.$iter() {
                    $body
                }
            }
            Terminator::Return { values, .. } => {
                for $slot in values.$iter() {
                    $body
                }
            }
            Terminator::Switch {
                discriminant,
                default_args,
                cases,
                ..
            } => {
                let $slot = discriminant;
                $body
                for $slot in default_args.$iter() {
                    $body
                }
                for (_, _, case_args) in cases.$iter() {
                    for $slot in case_args.$iter() {
                        $body
                    }
                }
            }
            Terminator::Invoke {
                args,
                normal_args,
                unwind_args,
                ..
            } => {
                for $slot in args.$iter() {
                    $body
                }
                for $slot in normal_args.$iter() {
                    $body
                }
                for $slot in unwind_args.$iter() {
                    $body
                }
            }
            Terminator::Resume { value, .. } => {
                let $slot = value;
                $body
            }
            Terminator::Unreachable => {}
        }
    };
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

    /// 按**固定顺序**遍历终结符使用的全部值：`(平坦下标, 值)`。
    ///
    /// 下标序是 use-def 契约的一部分（见 [`crate::use_list::Use::operand_idx`]）：
    /// `UseLists` 记录、校验与改写都走这一序列，**不允许**各处自行 `match` 变体。
    ///
    /// 顺序（v3 方案 S4-a，2026-09-15 定型）：
    ///
    /// - `Branch`：`cond`、`then_args…`、`else_args…`
    /// - `Jump`：`args…`
    /// - `Return`：`values…`
    /// - `Switch`：`discriminant`、`default_args…`、各 case 的实参（声明序）
    /// - `Invoke`：`args…`、`normal_args…`、`unwind_args…`
    /// - `Resume`：`value`
    /// - `Unreachable`：无
    pub fn for_each_value(&self, mut f: impl FnMut(u32, Value)) {
        let mut idx = 0u32;
        walk_terminator_values!(self, slot, iter, {
            f(idx, *slot);
            idx += 1;
        });
        // 单槽变体（Resume）的末次自增后没有下一次读取——显式读一次，
        // 免得 `unused_assignments` 在宏展开上报警。
        let _ = idx;
    }

    /// [`Terminator::for_each_value`] 的可变版本——**下标序完全相同**。
    /// 用于 RAUW / 批量替换（改完值后调用方须重登记 use 项，见
    /// [`crate::Function::refresh_terminator_uses`]）。
    pub fn for_each_value_mut(&mut self, mut f: impl FnMut(u32, &mut Value)) {
        let mut idx = 0u32;
        walk_terminator_values!(self, slot, iter_mut, {
            f(idx, slot);
            idx += 1;
        });
        let _ = idx;
    }

    /// 用值个数（= [`Terminator::for_each_value`] 的下标上界）。
    pub fn value_count(&self) -> u32 {
        let mut n = 0u32;
        self.for_each_value(|idx, _| n = idx + 1);
        n
    }

    /// 返回此终止指令中使用的所有值（顺序 = [`Terminator::for_each_value`]，由后者实现）。
    pub fn used_values(&self) -> Vec<Value> {
        let mut values = Vec::new();
        self.for_each_value(|_, v| values.push(v));
        values
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

    /// 各变体的平坦遍历序**就是契约**（use-list 下标用它）。
    /// 期望值逐条写死：改遍历序必须同时改这里，提醒它是一次破坏性变更。
    #[test]
    fn test_flat_value_order_is_spec() {
        let (v0, v1, v2, v3, v4, v5) = (0u32, 1, 2, 3, 4, 5);
        let flat = |t: &Terminator| {
            let mut out = Vec::new();
            t.for_each_value(|idx, v| out.push((idx, v.0)));
            out
        };

        let branch = Terminator::Branch {
            cond: Value(v0),
            then_block: Block(1),
            then_args: smallvec::smallvec![Value(v1), Value(v2)],
            else_block: Block(2),
            else_args: smallvec::smallvec![Value(v3)],
            metadata: SmallVec::new(),
        };
        assert_eq!(flat(&branch), vec![(0, v0), (1, v1), (2, v2), (3, v3)]);

        let jump = Terminator::Jump {
            target: Block(1),
            args: smallvec::smallvec![Value(v1), Value(v2)],
            metadata: SmallVec::new(),
        };
        assert_eq!(flat(&jump), vec![(0, v1), (1, v2)]);

        let ret = Terminator::Return {
            values: smallvec::smallvec![Value(v4)],
            metadata: SmallVec::new(),
        };
        assert_eq!(flat(&ret), vec![(0, v4)]);

        let switch = Terminator::Switch {
            discriminant: Value(v0),
            default_block: Block(1),
            default_args: smallvec::smallvec![Value(v1)],
            cases: smallvec::smallvec![
                (0, Block(2), smallvec::smallvec![Value(v2), Value(v3)]),
                (1, Block(3), smallvec::smallvec![Value(v4)]),
            ],
            metadata: SmallVec::new(),
        };
        assert_eq!(
            flat(&switch),
            vec![(0, v0), (1, v1), (2, v2), (3, v3), (4, v4)]
        );

        let invoke = Terminator::Invoke {
            callee: FuncRef(0),
            args: smallvec::smallvec![Value(v1)],
            ret_ty: TypeId::VOID,
            normal_block: Block(1),
            normal_args: smallvec::smallvec![Value(v2)],
            unwind_block: Block(2),
            unwind_args: smallvec::smallvec![Value(v3)],
            metadata: SmallVec::new(),
        };
        assert_eq!(flat(&invoke), vec![(0, v1), (1, v2), (2, v3)]);

        let resume = Terminator::Resume {
            value: Value(v5),
            metadata: SmallVec::new(),
        };
        assert_eq!(flat(&resume), vec![(0, v5)]);

        assert!(flat(&Terminator::Unreachable).is_empty());
    }

    /// 共享遍历与可变遍历必须给出**同一序列**（同一模板展开 ⇒ 结构性保证；
    /// 本测试把该保证变成可执行的回归门）。
    #[test]
    fn test_shared_and_mut_walks_agree() {
        let samples = vec![
            Terminator::Branch {
                cond: Value(0),
                then_block: Block(1),
                then_args: smallvec::smallvec![Value(1)],
                else_block: Block(2),
                else_args: smallvec::smallvec![Value(2), Value(3)],
                metadata: SmallVec::new(),
            },
            Terminator::Jump {
                target: Block(1),
                args: smallvec::smallvec![Value(7)],
                metadata: SmallVec::new(),
            },
            Terminator::Return {
                values: smallvec::smallvec![Value(8)],
                metadata: SmallVec::new(),
            },
            Terminator::Switch {
                discriminant: Value(0),
                default_block: Block(1),
                default_args: smallvec::smallvec![Value(1)],
                cases: smallvec::smallvec![(3, Block(2), smallvec::smallvec![Value(2)])],
                metadata: SmallVec::new(),
            },
            Terminator::Invoke {
                callee: FuncRef(0),
                args: smallvec::smallvec![Value(4)],
                ret_ty: TypeId::VOID,
                normal_block: Block(1),
                normal_args: smallvec::smallvec![Value(5)],
                unwind_block: Block(2),
                unwind_args: smallvec::smallvec![Value(6)],
                metadata: SmallVec::new(),
            },
            Terminator::Resume {
                value: Value(9),
                metadata: SmallVec::new(),
            },
            Terminator::Unreachable,
        ];

        for t in &samples {
            let mut shared = Vec::new();
            t.for_each_value(|idx, v| shared.push((idx, v)));
            let mut mutable = Vec::new();
            let mut t2 = t.clone();
            t2.for_each_value_mut(|idx, v| mutable.push((idx, *v)));
            assert_eq!(shared, mutable, "walk divergence on {t:?}");
            assert_eq!(shared.len() as u32, t.value_count());
            assert_eq!(
                shared.iter().map(|(_, v)| *v).collect::<Vec<Value>>(),
                t.used_values()
            );
        }
    }

    /// `for_each_value_mut` 能真正改写槽位（RAUW 依赖它）。
    #[test]
    fn test_for_each_value_mut_rewrites_all_slots() {
        let mut t = Terminator::Switch {
            discriminant: Value(0),
            default_block: Block(1),
            default_args: smallvec::smallvec![Value(0)],
            cases: smallvec::smallvec![(1, Block(2), smallvec::smallvec![Value(0), Value(5)])],
            metadata: SmallVec::new(),
        };
        t.for_each_value_mut(|_, slot| {
            if *slot == Value(0) {
                *slot = Value(42);
            }
        });
        assert_eq!(
            t.used_values(),
            vec![Value(42), Value(42), Value(42), Value(5)]
        );
    }
}
