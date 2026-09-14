//! 逐指令**类型规则**的实现（规则本身声明在 `ops.toml` 的 `type_rule`）。
//!
//! 这里只放**不需要类型上下文**的形状规则（`verify.rs` 用它做校验、`builder.rs`
//! 用它做 debug 断言——同一份实现，不是两份拷贝）：
//!
//! - [`TypeRule::BinopSame`]：前两个值操作数同类型、且非聚合
//! - [`TypeRule::Same3`]：三个操作数同类型
//! - [`TypeRule::CmpInt`]/[`TypeRule::CmpFloat`]：同类型 + 类别
//! - [`TypeRule::Select`]：cond 为 bool、两分支同类型、结果同分支类型
//!
//! 需要 `TypeContext` 的族（`Convert`/`Load`/`Store`/`Call*`）留在 `verify.rs`：
//! 它们的判定依赖 `size_bytes`/指针/向量事实，builder 侧没有等价上下文。

use crate::TypeId;
use crate::opcode::{Opcode, TypeRule};

/// 形状规则违规（与具体错误类型无关——由调用方决定报错还是 panic）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeViolation {
    /// 两个（或三个）操作数类型不一致。
    OperandTypeMismatch { expected: TypeId, found: TypeId },
    /// 操作数是聚合类型（算术/比较不接受聚合）。
    NotScalar { found: TypeId },
    /// `select` 的条件不是 bool。
    CondNotBool { found: TypeId },
    /// 结果类型与要求的不一致。
    ResultTypeMismatch { expected: TypeId, found: TypeId },
    /// 比较指令的操作数类别不对（`want_int` = 需要整型）。
    CompareClass { want_int: bool },
}

/// 按 `type_rule` 检查形状；返回全部违规（空 = 通过）。
///
/// `operand_tys[i]` 为 `None` 表示该操作数类型未知（未解析）——与 verifier 的
/// 既有行为一致：**未知则跳过对应检查**，不误报。
///
/// `is_aggregate`：类型是否为聚合（只有 `BinopSame` 用；调用方无类型上下文时
/// 可传 `|_| false`，此时不检查聚合）。
pub fn check_shape(
    opcode: Opcode,
    operand_tys: &[Option<TypeId>],
    result_ty: Option<TypeId>,
    is_aggregate: &dyn Fn(TypeId) -> bool,
) -> Vec<ShapeViolation> {
    let mut out = Vec::new();
    let two = || -> Option<(TypeId, TypeId)> {
        match (
            operand_tys.first().copied().flatten(),
            operand_tys.get(1).copied().flatten(),
        ) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        }
    };
    match opcode.type_rule() {
        TypeRule::None
        | TypeRule::Convert
        | TypeRule::CmpxchgPair
        | TypeRule::Load
        | TypeRule::Store
        | TypeRule::Call
        | TypeRule::CallIndirect => {}
        TypeRule::BinopSame => {
            if let Some((a, b)) = two() {
                if a != b {
                    out.push(ShapeViolation::OperandTypeMismatch {
                        expected: a,
                        found: b,
                    });
                }
                for t in [a, b] {
                    if is_aggregate(t) {
                        out.push(ShapeViolation::NotScalar { found: t });
                    }
                }
            }
        }
        TypeRule::Same3 => {
            let tys: Vec<TypeId> = operand_tys.iter().take(3).filter_map(|t| *t).collect();
            if tys.len() == 3 && (tys[0] != tys[1] || tys[0] != tys[2]) {
                out.push(ShapeViolation::OperandTypeMismatch {
                    expected: tys[0],
                    found: tys[2],
                });
            }
        }
        TypeRule::CmpInt | TypeRule::CmpFloat => {
            if let Some((a, b)) = two() {
                if a != b {
                    out.push(ShapeViolation::OperandTypeMismatch {
                        expected: a,
                        found: b,
                    });
                }
                let want_int = opcode.type_rule() == TypeRule::CmpInt;
                let ok = if want_int {
                    a.is_int() && b.is_int()
                } else {
                    a.is_float() && b.is_float()
                };
                if !ok {
                    out.push(ShapeViolation::CompareClass { want_int });
                }
            }
        }
        TypeRule::Select => {
            if let Some(t) = operand_tys.first().copied().flatten()
                && t != TypeId::BOOL
            {
                out.push(ShapeViolation::CondNotBool { found: t });
            }
            if let (Some(t1), Some(t2)) = (
                operand_tys.get(1).copied().flatten(),
                operand_tys.get(2).copied().flatten(),
            ) {
                if t1 != t2 {
                    out.push(ShapeViolation::OperandTypeMismatch {
                        expected: t1,
                        found: t2,
                    });
                }
                if let Some(rt) = result_ty
                    && rt != t1
                {
                    out.push(ShapeViolation::ResultTypeMismatch {
                        expected: t1,
                        found: rt,
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::Opcode;

    fn viol(op: Opcode, ops: &[Option<TypeId>], res: Option<TypeId>) -> Vec<ShapeViolation> {
        check_shape(op, ops, res, &|_| false)
    }

    #[test]
    fn binop_same_accepts_equal_types_and_rejects_mismatch() {
        let i32ty = TypeId::I32;
        let i64ty = TypeId::I64;
        assert!(viol(Opcode::Iadd, &[Some(i32ty), Some(i32ty)], Some(i32ty)).is_empty());
        assert_eq!(
            viol(Opcode::Iadd, &[Some(i32ty), Some(i64ty)], Some(i32ty)),
            vec![ShapeViolation::OperandTypeMismatch {
                expected: i32ty,
                found: i64ty
            }]
        );
        // 未知操作数类型不误报
        assert!(viol(Opcode::Iadd, &[Some(i32ty), None], Some(i32ty)).is_empty());
    }

    #[test]
    fn cmp_class_is_checked() {
        assert!(
            viol(
                Opcode::Icmp,
                &[Some(TypeId::F32), Some(TypeId::F32)],
                Some(TypeId::BOOL)
            )
            .contains(&ShapeViolation::CompareClass { want_int: true })
        );
        assert!(
            viol(
                Opcode::Fcmp,
                &[Some(TypeId::I32), Some(TypeId::I32)],
                Some(TypeId::BOOL)
            )
            .contains(&ShapeViolation::CompareClass { want_int: false })
        );
        assert!(
            viol(
                Opcode::Fcmp,
                &[Some(TypeId::F32), Some(TypeId::F32)],
                Some(TypeId::BOOL)
            )
            .is_empty()
        );
    }

    #[test]
    fn select_checks_cond_and_branches() {
        let t = TypeId::I32;
        let v = viol(Opcode::Select, &[Some(t), Some(t), Some(t)], Some(t));
        assert!(v.contains(&ShapeViolation::CondNotBool { found: t }));
        assert!(
            viol(
                Opcode::Select,
                &[Some(TypeId::BOOL), Some(t), Some(t)],
                Some(t)
            )
            .is_empty()
        );
        assert_eq!(
            viol(
                Opcode::Select,
                &[Some(TypeId::BOOL), Some(t), Some(TypeId::I64)],
                Some(t)
            )
            .len(),
            1
        );
    }

    #[test]
    fn rules_without_shape_checks_are_silent() {
        // 转换/内存/调用族在 verify.rs（需要类型上下文），这里不报
        for op in [Opcode::Sextend, Opcode::Load, Opcode::Call, Opcode::Nop] {
            assert!(
                viol(
                    op,
                    &[Some(TypeId::I32), Some(TypeId::I64)],
                    Some(TypeId::I32)
                )
                .is_empty()
            );
        }
    }
}
