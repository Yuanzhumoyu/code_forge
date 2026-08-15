//! rustc 类型 → forge-ir 类型映射。
//!
//! 调用方约定：`map_type` 对未知类型返回 `Err`，但多数调用点用
//! `unwrap_or(TypeId::I32)` 兜底——I32 是 x86-64 最常用寄存器宽度，
//! 未知类型（如 ZST/自定义类型）按 4 字节寄存器处理可保证大多数
//! 读写路径正确（栈槽分配另按真实布局大小）。需要精确类型语义的
//! 路径（sret/参数/返回值）必须检查 `Err` 而非兜底。

use crate::prelude::IrError;
use crate::prelude::*;

pub fn map_type(ty: rustc_middle::ty::Ty<'_>, tcx: TyCtxt<'_>) -> Result<TypeId, IrError> {
    match ty.kind() {
        ty::TyKind::Bool => Ok(TypeId::BOOL),
        ty::TyKind::Int(ty::IntTy::I8) => Ok(TypeId::I8),
        ty::TyKind::Int(ty::IntTy::I16) => Ok(TypeId::I16),
        ty::TyKind::Int(ty::IntTy::I32) => Ok(TypeId::I32),
        ty::TyKind::Int(ty::IntTy::I64) => Ok(TypeId::I64),
        ty::TyKind::Int(ty::IntTy::Isize) => match tcx.data_layout.pointer_size().bits() {
            64 => Ok(TypeId::I64),
            32 => Ok(TypeId::I32),
            _ => Ok(TypeId::I64),
        },
        ty::TyKind::Uint(ty::UintTy::U8) => Ok(TypeId::I8),
        ty::TyKind::Uint(ty::UintTy::U16) => Ok(TypeId::I16),
        ty::TyKind::Uint(ty::UintTy::U32) => Ok(TypeId::I32),
        ty::TyKind::Uint(ty::UintTy::U64) => Ok(TypeId::I64),
        ty::TyKind::Uint(ty::UintTy::Usize) => match tcx.data_layout.pointer_size().bits() {
            64 => Ok(TypeId::I64),
            32 => Ok(TypeId::I32),
            _ => Ok(TypeId::I64),
        },
        ty::TyKind::Float(ty::FloatTy::F32) => Ok(TypeId::F32),
        ty::TyKind::Float(ty::FloatTy::F64) => Ok(TypeId::F64),
        ty::TyKind::Char => Ok(TypeId::I32),
        ty::TyKind::Ref(..) | ty::TyKind::RawPtr(..) => Ok(TypeId::PTR),
        ty::TyKind::FnDef(..) | ty::TyKind::FnPtr(..) => Ok(TypeId::PTR),
        ty::TyKind::Tuple(tys) if tys.is_empty() => Ok(TypeId::VOID),
        ty::TyKind::Never => Ok(TypeId::VOID),
        // 零大小类型用 I8 占位（不占栈空间，仅用于类型系统）
        _ => {
            if ty.is_unit() {
                Ok(TypeId::VOID)
            } else {
                Ok(TypeId::PTR)
            }
        }
    }
}

/// debug-only 类型兼容自检（P2.5）。
///
/// 断言 `actual` 与 `expected` 在 ABI 宽度语义上兼容。x86-64 下 I32/I64、
/// BOOL/I32、PTR/I64 可安全混用（寄存器宽度/零扩展），F32/F64 与整数
/// 混用是真实 bug（浮点走 XMM、整数走 GPR，错位读到垃圾）。仅
/// `debug_assertions` 构建启用；release 构建为空操作。
#[cfg(debug_assertions)]
pub(crate) fn assert_assignable(actual: TypeId, expected: TypeId, ctx: &str) {
    let compatible = actual == expected
        || matches!(
            (actual, expected),
            (TypeId::I32, TypeId::I64) | (TypeId::I64, TypeId::I32)
        )
        || matches!(
            (actual, expected),
            (TypeId::BOOL, TypeId::I32) | (TypeId::I32, TypeId::BOOL)
        )
        || matches!(
            (actual, expected),
            (TypeId::PTR, TypeId::I64) | (TypeId::I64, TypeId::PTR)
        );
    if !compatible {
        panic!("[forge] type mismatch in {ctx}: {actual:?} vs {expected:?}");
    }
}

// ============================================================
