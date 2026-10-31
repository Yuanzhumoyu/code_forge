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
