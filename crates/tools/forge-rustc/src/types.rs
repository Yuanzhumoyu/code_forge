//! rustc 类型 → forge-ir 类型映射。
//!
//! 调用方约定：`map_type` 对未知类型返回 `Err`，但多数调用点用
//! `unwrap_or(TypeId::I32)` 兜底——I32 是 x86-64 最常用寄存器宽度，
//! 未知类型（如 ZST/自定义类型）按 4 字节寄存器处理可保证大多数
//! 读写路径正确（栈槽分配另按真实布局大小）。需要精确类型语义的
//! 路径（sret/参数/返回值）必须检查 `Err` 而非兜底。

use crate::prelude::IrError;
use crate::prelude::*;

/// 整数位宽 → IR 整数类型（指针/`usize` 也走这里；8/16 位目标不再回落 I64）。
pub fn int_ty_for_width(bits: u16) -> TypeId {
    match bits {
        0..=8 => TypeId::I8,
        9..=16 => TypeId::I16,
        17..=32 => TypeId::I32,
        _ => TypeId::I64,
    }
}

/// 构造目标 IR 数据布局（纯函数，单测可直接用）。
pub fn data_layout_for(
    pointer_bits: u16,
    endianness: Endianness,
    mangling: Mangling,
) -> DataLayout {
    let mut dl = DataLayout::default();
    let bytes = u32::from(pointer_bits.max(8) / 8);
    dl.endianness = endianness;
    dl.mangling = mangling;
    dl.pointer_layout.insert(0, (bytes, bytes));
    let max = pointer_bits.max(32);
    dl.native_integer_widths = [8u32, 16, 32, 64]
        .into_iter()
        .filter(|w| *w <= u32::from(max))
        .collect();
    dl
}

/// 按 rustc target 构造 IR 数据布局（v28 M4）。
///
/// 此前 `forge-rustc` 从不设置 DataLayout，`PTR` 恒为 64 位——32/16 位目标上
/// 指针/尺寸类型全错。现在每个函数的 `FunctionBuilder` 在发指令前设置它。
pub fn ir_data_layout(tcx: TyCtxt<'_>) -> DataLayout {
    let bits = tcx.data_layout.pointer_size().bits() as u16;
    let endianness = if tcx.data_layout.endian == rustc_abi::Endian::Big {
        Endianness::Big
    } else {
        Endianness::Little
    };
    let mangling = if tcx.sess.target.is_like_windows {
        Mangling::WindowsCoff
    } else if matches!(
        format!("{:?}", tcx.sess.target.os).as_str(),
        "Macos" | "Ios" | "Tvos" | "Watchos"
    ) {
        Mangling::MachO
    } else {
        Mangling::Elf
    };
    data_layout_for(bits, endianness, mangling)
}

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
        ty::TyKind::Int(ty::IntTy::Isize) => Ok(int_ty_for_width(
            tcx.data_layout.pointer_size().bits() as u16,
        )),
        ty::TyKind::Uint(ty::UintTy::U8) => Ok(TypeId::I8),
        ty::TyKind::Uint(ty::UintTy::U16) => Ok(TypeId::I16),
        ty::TyKind::Uint(ty::UintTy::U32) => Ok(TypeId::I32),
        ty::TyKind::Uint(ty::UintTy::U64) => Ok(TypeId::I64),
        ty::TyKind::Uint(ty::UintTy::Usize) => Ok(int_ty_for_width(
            tcx.data_layout.pointer_size().bits() as u16,
        )),
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
                // v27 M6-144：整数/双精度 SIMD 用**固定的跨函数 TypeId**（forge-ir
                // 预注册，所有函数同一 id），才能参与形参/返回/调用 ABI。
                (TypeId::F64, 1) => Ok(TypeId::V64_F64),
                (TypeId::F64, 2) => Ok(TypeId::V128_F64),
                (TypeId::F64, 4) => Ok(TypeId::V256_F64),
                (TypeId::I8, 8) => Ok(TypeId::V64_I8),
                (TypeId::I8, 16) => Ok(TypeId::V128_I8),
                (TypeId::I16, 4) => Ok(TypeId::V64_I16),
                (TypeId::I16, 8) => Ok(TypeId::V128_I16),
                (TypeId::I32, 2) => Ok(TypeId::V64_I32),
                (TypeId::I32, 4) => Ok(TypeId::V128_I32),
                (TypeId::I32, 8) => Ok(TypeId::V256_I32),
                (TypeId::I64, 1) => Ok(TypeId::V64_I64),
                (TypeId::I64, 2) => Ok(TypeId::V128_I64),
                (TypeId::I64, 4) => Ok(TypeId::V256_I64),
                _ => Err(IrError::Unsupported(format!(
                    "SIMD type <{n} x {elem}> (elem_ty={elem_ty:?}): unsupported shape \
                     (supported: f32×2/4/8, f64×1/2/4, i8×8/16, i16×4/8, i32×2/4/8, i64×1/2/4)"
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
// 宽度泛化单测（v28 M4）
// ============================================================

#[cfg(test)]
mod width_tests {
    use super::*;

    #[test]
    fn int_ty_covers_8_16_32_64() {
        assert_eq!(int_ty_for_width(8), TypeId::I8);
        assert_eq!(int_ty_for_width(16), TypeId::I16);
        assert_eq!(int_ty_for_width(32), TypeId::I32);
        assert_eq!(int_ty_for_width(64), TypeId::I64);
    }

    #[test]
    fn data_layout_pointer_width_and_native_widths() {
        for (bits, bytes) in [(16u16, 2u32), (32, 4), (64, 8)] {
            let dl = data_layout_for(bits, Endianness::Little, Mangling::Elf);
            assert_eq!(dl.pointer_layout.get(&0), Some(&(bytes, bytes)));
            assert!(
                dl.native_integer_widths
                    .iter()
                    .all(|w| *w <= u32::from(bits.max(32))),
                "{bits} 位布局含超宽整数"
            );
            assert!(
                dl.native_integer_widths.contains(&(u32::from(bits))),
                "{bits} 位布局应含本位宽"
            );
        }
    }

    #[test]
    fn data_layout_endian_and_mangling_are_kept() {
        let dl = data_layout_for(32, Endianness::Big, Mangling::WindowsCoff);
        assert_eq!(dl.endianness, Endianness::Big);
        assert_eq!(dl.mangling, Mangling::WindowsCoff);
    }
}
