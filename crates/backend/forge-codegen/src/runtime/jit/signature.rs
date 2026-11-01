//! JIT 类型化调用的签名面（v30 M4）。
//!
//! [`JitSignature`] 是**编译期**的签名声明：`JitLoader::get::<F>` 用
//! `F::params()` / `F::ret()` 与产物携带的 [`JitSignatureInfo`] 逐项比对，
//! 不匹配就 fail-closed。它只覆盖"确定能安全表达"的形态：
//!
//! - 调用约定只接受 `extern "C"`（编译产物按平台 C ABI 发射）；
//! - 参数/返回限标量（`u8/i8/u16/i16/u32/i32/u64/i64/f32/f64`）与**瘦指针**
//!   （`*const T`/`*mut T`）；
//! - 变参、`i128`、向量、聚合一律走 `JitLoader::get_unchecked`（显式 unsafe）。
//!
//! **为什么是 `unsafe trait`**：实现者断言"这个 Rust 类型与对应 ABI 形态同构"；
//! 只应由本模块的宏为 `extern "C" fn` 生成实现，宿主不要自行实现。

use forge_ir::TypeId;

/// 可作 JIT 形参的标量/瘦指针。
///
/// # Safety
///
/// 实现者必须保证该 Rust 类型按平台 C ABI 传递时与 [`Self::IR`] 描述的类型同构。
pub unsafe trait JitParam: 'static {
    /// 对应的 IR 类型。
    const IR: TypeId;
}

macro_rules! scalar_params {
    ($($t:ty => $id:ident),* $(,)?) => {
        $(
            unsafe impl JitParam for $t {
                const IR: TypeId = TypeId::$id;
            }
        )*
    };
}

scalar_params! {
    u8 => I8, i8 => I8,
    u16 => I16, i16 => I16,
    u32 => I32, i32 => I32,
    u64 => I64, i64 => I64,
    f32 => F32, f64 => F64,
}

unsafe impl<T: 'static> JitParam for *const T {
    const IR: TypeId = TypeId::PTR;
}

unsafe impl<T: 'static> JitParam for *mut T {
    const IR: TypeId = TypeId::PTR;
}

/// 可作 JIT 返回值的类型（`()` = void）。
///
/// # Safety
///
/// 与 [`JitParam`] 同：必须与 IR 返回类型按平台 C ABI 同构。
pub unsafe trait JitReturn: 'static {
    /// 对应 IR 返回类型；`None` = void。
    fn ir() -> Option<TypeId>;
}

unsafe impl JitReturn for () {
    fn ir() -> Option<TypeId> {
        None
    }
}

macro_rules! scalar_returns {
    ($($t:ty),* $(,)?) => {
        $(
            unsafe impl JitReturn for $t {
                fn ir() -> Option<TypeId> {
                    Some(<$t as JitParam>::IR)
                }
            }
        )*
    };
}

scalar_returns!(u8, i8, u16, i16, u32, i32, u64, i64, f32, f64);

unsafe impl<T: 'static> JitReturn for *const T {
    fn ir() -> Option<TypeId> {
        Some(TypeId::PTR)
    }
}

unsafe impl<T: 'static> JitReturn for *mut T {
    fn ir() -> Option<TypeId> {
        Some(TypeId::PTR)
    }
}

/// `extern "C" fn` 的签名描述（0–6 个标量/瘦指针参数）。
///
/// # Safety
///
/// 实现者必须保证 `params()`/`ret()` 与 `Self` 的 ABI 形态逐项一致。
pub unsafe trait JitSignature: Copy + 'static {
    /// 形参 IR 类型（顺序即 ABI 位置序）。
    fn params() -> Vec<TypeId>;
    /// 返回 IR 类型；`None` = void。
    fn ret() -> Option<TypeId>;
}

unsafe impl<R: JitReturn> JitSignature for extern "C" fn() -> R {
    fn params() -> Vec<TypeId> {
        Vec::new()
    }
    fn ret() -> Option<TypeId> {
        R::ir()
    }
}

macro_rules! fn_signatures {
    ($($p:ident),+) => {
        unsafe impl<$($p: JitParam,)+ R: JitReturn> JitSignature for extern "C" fn($($p),+) -> R {
            fn params() -> Vec<TypeId> {
                vec![$($p::IR),+]
            }
            fn ret() -> Option<TypeId> {
                R::ir()
            }
        }
    };
}

fn_signatures!(A);
fn_signatures!(A, B);
fn_signatures!(A, B, C);
fn_signatures!(A, B, C, D);
fn_signatures!(A, B, C, D, E);
fn_signatures!(A, B, C, D, E, F);

// ============================================================
// v32：目标感知的类型面（`bind` 用）
// ============================================================

use crate::pipeline::signature::{JitSigShape, JitTarget, JitType};
use std::ptr::NonNull;

/// Rust 类型 → 目标无关的 [`JitType`] 描述。
///
/// 与 v30 的 [`JitParam::IR`] 不同：这里**没有** `const`，`usize`/指针等按
/// [`JitTarget`] 解析（产物自己的指针位宽），不读宿主 `cfg`。
///
/// # Safety
/// 实现者必须保证该 Rust 类型按平台 C ABI 传递时与返回的 `JitType` 同构。
pub unsafe trait JitValue: 'static {
    fn jit_type(target: &JitTarget) -> JitType;
}

macro_rules! scalar_values {
    ($($t:ty => $val:expr),* $(,)?) => {
        $(
            unsafe impl JitValue for $t {
                fn jit_type(_target: &JitTarget) -> JitType {
                    $val
                }
            }
        )*
    };
}

scalar_values! {
    u8 => JitType::Int { bits: 8 }, i8 => JitType::Int { bits: 8 },
    u16 => JitType::Int { bits: 16 }, i16 => JitType::Int { bits: 16 },
    u32 => JitType::Int { bits: 32 }, i32 => JitType::Int { bits: 32 },
    u64 => JitType::Int { bits: 64 }, i64 => JitType::Int { bits: 64 },
    u128 => JitType::Int { bits: 128 }, i128 => JitType::Int { bits: 128 },
    f32 => JitType::Float { bits: 32 }, f64 => JitType::Float { bits: 64 },
    char => JitType::Int { bits: 32 },
    bool => JitType::Bool,
    () => JitType::Void,
}

unsafe impl JitValue for usize {
    fn jit_type(target: &JitTarget) -> JitType {
        JitType::Int {
            bits: target.pointer_bits,
        }
    }
}

unsafe impl JitValue for isize {
    fn jit_type(target: &JitTarget) -> JitType {
        JitType::Int {
            bits: target.pointer_bits,
        }
    }
}

macro_rules! pointer_values {
    ($($t:ty),* $(,)?) => {
        $(
            unsafe impl<T: 'static> JitValue for $t {
                fn jit_type(_target: &JitTarget) -> JitType {
                    JitType::Ptr { addr_space: 0 }
                }
            }
        )*
    };
}

pointer_values!(
    *const T,
    *mut T,
    NonNull<T>,
    Option<NonNull<T>>,
    Option<*const T>,
    Option<*mut T>
);

macro_rules! fn_value {
    ($($p:ident),*) => {
        unsafe impl<$($p: JitValue,)* R: JitValue> JitValue for extern "C" fn($($p),*) -> R {
            fn jit_type(target: &JitTarget) -> JitType {
                let mut rets = Vec::new();
                if !matches!(R::jit_type(target), JitType::Void) {
                    rets.push(R::jit_type(target));
                }
                JitType::Function {
                    params: vec![$($p::jit_type(target)),*],
                    rets,
                    variadic: false,
                }
            }
        }
    };
}

fn_value!();
fn_value!(A);
fn_value!(A, B);
fn_value!(A, B, C);
fn_value!(A, B, C, D);
fn_value!(A, B, C, D, E);
fn_value!(A, B, C, D, E, F);
fn_value!(A, B, C, D, E, F, G);
fn_value!(A, B, C, D, E, F, G, H);
fn_value!(A, B, C, D, E, F, G, H, I);
fn_value!(A, B, C, D, E, F, G, H, I, J);
fn_value!(A, B, C, D, E, F, G, H, I, J, K);
fn_value!(A, B, C, D, E, F, G, H, I, J, K, L);
fn_value!(A, B, C, D, E, F, G, H, I, J, K, L, M);
fn_value!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
fn_value!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
fn_value!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);

/// `bind` 用的签名声明（与 v30 的 [`JitSignature`] 分开：后者继续服务旧 `get`）。
///
/// # Safety
/// 实现者必须保证 `source`/`abi` 与 `Self` 的 ABI 形态逐项一致。
pub unsafe trait JitCallSignature: Copy + 'static {
    fn source(target: &JitTarget) -> JitSigShape;
    fn abi(target: &JitTarget) -> JitSigShape;
}

macro_rules! call_signature {
    ($($p:ident),*) => {
        unsafe impl<$($p: JitValue,)* R: JitValue> JitCallSignature for extern "C" fn($($p),*) -> R {
            fn source(target: &JitTarget) -> JitSigShape {
                let mut returns = Vec::new();
                if !matches!(R::jit_type(target), JitType::Void) {
                    returns.push(R::jit_type(target));
                }
                JitSigShape {
                    params: vec![$($p::jit_type(target)),*],
                    returns,
                    variadic: false,
                }
            }

            fn abi(target: &JitTarget) -> JitSigShape {
                let src = Self::source(target);
                JitSigShape {
                    params: src.params.into_iter().map(JitType::to_abi).collect(),
                    returns: src.returns.into_iter().map(JitType::to_abi).collect(),
                    variadic: false,
                }
            }
        }
    };
}

call_signature!();
call_signature!(A);
call_signature!(A, B);
call_signature!(A, B, C);
call_signature!(A, B, C, D);
call_signature!(A, B, C, D, E);
call_signature!(A, B, C, D, E, F);
call_signature!(A, B, C, D, E, F, G);
call_signature!(A, B, C, D, E, F, G, H);
call_signature!(A, B, C, D, E, F, G, H, I);
call_signature!(A, B, C, D, E, F, G, H, I, J);
call_signature!(A, B, C, D, E, F, G, H, I, J, K);
call_signature!(A, B, C, D, E, F, G, H, I, J, K, L);
call_signature!(A, B, C, D, E, F, G, H, I, J, K, L, M);
call_signature!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
call_signature!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
call_signature!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);

// v32 M3：C 变参函数指针（Rust 1.99 稳定的 `...` 类型）。
// 固定前缀参与 source/ABI 校验；`...` 由调用者与 Rust 变参类型保证。

unsafe impl<R: JitValue> JitValue for unsafe extern "C" fn(...) -> R {
    fn jit_type(target: &JitTarget) -> JitType {
        let mut rets = Vec::new();
        if !matches!(R::jit_type(target), JitType::Void) {
            rets.push(R::jit_type(target));
        }
        JitType::Function {
            params: Vec::new(),
            rets,
            variadic: true,
        }
    }
}

macro_rules! variadic_value {
    ($($p:ident),*) => {
        unsafe impl<$($p: JitValue,)* R: JitValue> JitValue
            for unsafe extern "C" fn($($p),*, ...) -> R
        {
            fn jit_type(target: &JitTarget) -> JitType {
                let mut rets = Vec::new();
                if !matches!(R::jit_type(target), JitType::Void) {
                    rets.push(R::jit_type(target));
                }
                JitType::Function {
                    params: vec![$($p::jit_type(target)),*],
                    rets,
                    variadic: true,
                }
            }
        }
    };
}

variadic_value!(A);
variadic_value!(A, B);
variadic_value!(A, B, C);
variadic_value!(A, B, C, D);
variadic_value!(A, B, C, D, E);
variadic_value!(A, B, C, D, E, F);
variadic_value!(A, B, C, D, E, F, G);
variadic_value!(A, B, C, D, E, F, G, H);
variadic_value!(A, B, C, D, E, F, G, H, I);
variadic_value!(A, B, C, D, E, F, G, H, I, J);
variadic_value!(A, B, C, D, E, F, G, H, I, J, K);
variadic_value!(A, B, C, D, E, F, G, H, I, J, K, L);
variadic_value!(A, B, C, D, E, F, G, H, I, J, K, L, M);
variadic_value!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
variadic_value!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);

unsafe impl<R: JitValue> JitCallSignature for unsafe extern "C" fn(...) -> R {
    fn source(target: &JitTarget) -> JitSigShape {
        let mut returns = Vec::new();
        if !matches!(R::jit_type(target), JitType::Void) {
            returns.push(R::jit_type(target));
        }
        JitSigShape {
            params: Vec::new(),
            returns,
            variadic: true,
        }
    }

    fn abi(target: &JitTarget) -> JitSigShape {
        let src = Self::source(target);
        JitSigShape {
            params: src.params.into_iter().map(JitType::to_abi).collect(),
            returns: src.returns.into_iter().map(JitType::to_abi).collect(),
            variadic: true,
        }
    }
}

macro_rules! variadic_call_signature {
    ($($p:ident),*) => {
        unsafe impl<$($p: JitValue,)* R: JitValue> JitCallSignature
            for unsafe extern "C" fn($($p),*, ...) -> R
        {
            fn source(target: &JitTarget) -> JitSigShape {
                let mut returns = Vec::new();
                if !matches!(R::jit_type(target), JitType::Void) {
                    returns.push(R::jit_type(target));
                }
                JitSigShape {
                    params: vec![$($p::jit_type(target)),*],
                    returns,
                    variadic: true,
                }
            }

            fn abi(target: &JitTarget) -> JitSigShape {
                let src = Self::source(target);
                JitSigShape {
                    params: src.params.into_iter().map(JitType::to_abi).collect(),
                    returns: src.returns.into_iter().map(JitType::to_abi).collect(),
                    variadic: true,
                }
            }
        }
    };
}

variadic_call_signature!(A);
variadic_call_signature!(A, B);
variadic_call_signature!(A, B, C);
variadic_call_signature!(A, B, C, D);
variadic_call_signature!(A, B, C, D, E);
variadic_call_signature!(A, B, C, D, E, F);
variadic_call_signature!(A, B, C, D, E, F, G);
variadic_call_signature!(A, B, C, D, E, F, G, H);
variadic_call_signature!(A, B, C, D, E, F, G, H, I);
variadic_call_signature!(A, B, C, D, E, F, G, H, I, J);
variadic_call_signature!(A, B, C, D, E, F, G, H, I, J, K);
variadic_call_signature!(A, B, C, D, E, F, G, H, I, J, K, L);
variadic_call_signature!(A, B, C, D, E, F, G, H, I, J, K, L, M);
variadic_call_signature!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
variadic_call_signature!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);

#[cfg(test)]
mod v32_tests {
    use super::*;

    fn t32() -> JitTarget {
        JitTarget {
            arch: "x86".to_string(),
            pointer_bits: 32,
            int_widths: vec![8, 16, 32],
            endian: forge_ir::Endianness::Little,
        }
    }

    #[test]
    fn usize_follows_target_pointer_width_not_host_cfg() {
        assert_eq!(
            <usize as JitValue>::jit_type(&t32()),
            JitType::Int { bits: 32 }
        );
        assert_eq!(
            <isize as JitValue>::jit_type(&t32()),
            JitType::Int { bits: 32 }
        );
        assert_eq!(
            <*const u8 as JitValue>::jit_type(&t32()),
            JitType::Ptr { addr_space: 0 }
        );
    }

    #[test]
    fn function_pointer_has_a_real_function_type() {
        type Cb = extern "C" fn(i64) -> i64;
        assert_eq!(
            <Cb as JitValue>::jit_type(&t32()),
            JitType::Function {
                params: vec![JitType::Int { bits: 64 }],
                rets: vec![JitType::Int { bits: 64 }],
                variadic: false,
            }
        );
        assert_eq!(
            <Cb as JitValue>::jit_type(&t32()).to_abi(),
            JitType::Ptr { addr_space: 0 }
        );
    }

    #[test]
    fn i128_abi_is_explicitly_unsupported_until_m1b() {
        let abi = <extern "C" fn(i128) -> i128 as JitCallSignature>::abi(&t32());
        assert!(
            abi.params
                .iter()
                .any(|t| matches!(t, JitType::UnsupportedAbi(_))),
            "i128 入口 ABI 在 M1b 前必须显式 Unsupported：{abi:?}"
        );
    }

    #[test]
    fn fieldless_enum_abi_is_its_repr() {
        use crate::pipeline::signature::JitEnumVariant;
        let e = JitType::Enum {
            repr: Box::new(JitType::Int { bits: 8 }),
            variants: vec![
                JitEnumVariant {
                    name: "A".into(),
                    discriminant: 0,
                    fields: Vec::new(),
                },
                JitEnumVariant {
                    name: "B".into(),
                    discriminant: 1,
                    fields: Vec::new(),
                },
            ],
        };
        assert_eq!(e.to_abi(), JitType::Int { bits: 8 });
    }

    #[test]
    fn data_enum_and_union_are_explicitly_unsupported_by_value() {
        use crate::pipeline::signature::JitEnumVariant;
        let data = JitType::Enum {
            repr: Box::new(JitType::Int { bits: 8 }),
            variants: vec![JitEnumVariant {
                name: "A".into(),
                discriminant: 0,
                fields: vec![JitType::Int { bits: 32 }],
            }],
        };
        assert!(matches!(data.to_abi(), JitType::UnsupportedAbi(_)));
        let union = JitType::Union {
            size: 8,
            align: 8,
            fields: vec![JitType::Int { bits: 64 }, JitType::Float { bits: 64 }],
        };
        assert!(matches!(union.to_abi(), JitType::UnsupportedAbi(_)));
    }
}
