//! rustc 类型 → forge-ir 类型映射。
//!
//! 调用方约定：`map_type` 对未知类型返回 `Err`，但多数调用点用
//! `unwrap_or(TypeId::I32)` 兜底——I32 是 x86-64 最常用寄存器宽度，
//! 未知类型（如 ZST/自定义类型）按 4 字节寄存器处理可保证大多数
//! 读写路径正确（栈槽分配另按真实布局大小）。需要精确类型语义的
//! 路径（sret/参数/返回值）必须检查 `Err` 而非兜底。

use crate::prelude::IrError;
use crate::prelude::*;

pub fn map_type<'tcx>(
    ty: rustc_middle::ty::Ty<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> Result<TypeId, IrError> {
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
        // **128 位整数仍 fail-closed**（v27 M5 · M-F P3 暂缓）：`wide_split` 的**常量/算术**
        // 路径已 JIT 验证（11 条），但"从内存 `load i128` 再运算"这条链路在 e2e 上仍不正确
        //（4 条用例：`u128_shift`/`u128_shift_var`/`u128_not`/`u128_neg`）⇒ 先回退，等
        // `load/store` 路径补上 JIT 覆盖再切。绝不静默错值。
        // **128 位整数（v27 M5 · M-F P3）**：映射到真正的 `i128` IR 类型。
        // 合法化由 `legalize::wide_split` 负责：宽值在 IR 内被拆成 **n 个原生宽 SSA 值**，
        // 因此 isel 永远看不到 16 字节标量。未覆盖形态（宽值块参数 phi、除/余）
        // ⇒ 明确 `[cap:WIDE_INT_SHAPE]`，不静默错值。
        ty::TyKind::Int(ty::IntTy::I128) | ty::TyKind::Uint(ty::UintTy::U128) => Ok(TypeId::I128),
        ty::TyKind::Float(ty::FloatTy::F32) => Ok(TypeId::F32),
        ty::TyKind::Float(ty::FloatTy::F64) => Ok(TypeId::F64),
        ty::TyKind::Char => Ok(TypeId::I32),
        ty::TyKind::Ref(..) | ty::TyKind::RawPtr(..) => Ok(TypeId::PTR),
        ty::TyKind::FnDef(..) | ty::TyKind::FnPtr(..) => Ok(TypeId::PTR),
        // SIMD 向量（B1/WA-37）：`#[repr(simd)]` 结构体在 rustc 中是 Adt 且
        // `ty.is_simd()` 为真。仅支持内置 f32×2/4/8 → V64/V128/V256——
        // 这些是 TypeStore 预填充的固定索引（TypeId 13/14/15），跨
        // TypeContext 一致（每函数一个 TypeContext，动态 intern 的向量
        // TypeId 会随函数不同而错位——ABI 跨函数传参会读到错误类型）。
        // 其余 SIMD 形态（i32×4 等）编译期 Unsupported（失败即报错）。
        _ if ty.is_simd() => {
            // Adt 的 SIMD：`#[repr(simd)]` 结构体只有一个字段 [T; N]。
            // 元素类型 = 该字段的数组元素类型（字段本身是 [T; N] Array，
            // 需解包取 T）；长度 = size / elem_size。
            let elem = match ty.kind() {
                rustc_middle::ty::TyKind::Adt(def, substs) => {
                    let variant = def.non_enum_variant();
                    if variant.fields.is_empty() {
                        return Err(IrError::Unsupported(format!(
                            "SIMD type {ty}: no fields (repr(simd) requires [T; N])"
                        )));
                    }
                    let f_ty = variant.fields.raw[0].ty(tcx, substs).skip_normalization();
                    // [T; N] → T
                    if let rustc_middle::ty::TyKind::Array(elem, _) = f_ty.kind() {
                        *elem
                    } else {
                        f_ty
                    }
                }
                _ => {
                    return Err(IrError::Unsupported(format!("SIMD type {ty}: not an Adt")));
                }
            };
            let elem_ty = map_type(elem, tcx)?;
            let total = tcx
                .layout_of(ty::PseudoCanonicalInput {
                    typing_env: ty::TypingEnv::fully_monomorphized(),
                    value: ty,
                })
                .map(|l| l.layout.size().bytes() as u32)
                .unwrap_or(0);
            let elem_sz = crate::layout::layout_bytes(tcx, elem);
            if elem_sz == 0 {
                return Err(IrError::Unsupported(format!(
                    "SIMD type {ty}: zero-sized element"
                )));
            }
            let n = total / elem_sz;
            match (elem_ty, n) {
                (TypeId::F32, 2) => Ok(TypeId::V64),
                (TypeId::F32, 4) => Ok(TypeId::V128),
                (TypeId::F32, 8) => Ok(TypeId::V256),
                _ => Err(IrError::Unsupported(format!(
                    "SIMD type <{n} x {elem}> (elem_ty={elem_ty:?}): only f32×2/4/8 \
                     (V64/V128/V256) are supported by the x86 backend"
                ))),
            }
        }
        ty::TyKind::Tuple(tys) if tys.is_empty() => Ok(TypeId::VOID),
        ty::TyKind::Never => Ok(TypeId::VOID),
        // 零大小类型用 I8 占位（不占栈空间，仅用于类型系统）
        _ => {
            // v27 M5 审计（`u128` 事件的精确防线）：兜底**可以**接聚合/闭包/分配器形态
            // （它们按指针形状表示，实测有 12+ 条用例依赖），但**标量**落到这里就是
            // "静默变成指针"——`u128`/`i128` 当年正是这样被当成 64 位、整条算术链算错。
            // 所以这里只钉标量：debug 构建（e2e 就是）一违反就炸，release 不受影响。
            debug_assert!(
                !matches!(
                    ty.kind(),
                    ty::TyKind::Int(_)
                        | ty::TyKind::Uint(_)
                        | ty::TyKind::Float(_)
                        | ty::TyKind::Bool
                        | ty::TyKind::Char
                ),
                "标量类型 `{ty}` 落到了 `map_type` 的兜底分支（会被静默当成指针）"
            );
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
