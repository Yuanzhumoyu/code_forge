//! [`AsConst`]：把 Rust 标量物化成 IR 常量值。
//!
//! 这是**唯一**的便捷入口——历史上那批按类型平铺的函数（`iconst_i8/i16/i32/i64`、
//! `fconst_f32/f64`、`iconst_i128`、`fconst128`）已全部删除：它们的名字把"位宽"和"构造"
//! 混在一起，而位宽本就该由 Rust 类型决定。
//!
//! 两种等价写法：
//!
//! ```ignore
//! let a = fb.constant(42i32);          // 便捷入口（推荐）
//! let b = 42i32.into_const(&mut fb);   // trait 方法直接调用
//! ```
//!
//! 结果类型由 Rust 类型决定：`bool` → i1；`i8/i16/i32/i64`、`u8/u16/u32/u64` → 同宽；
//! `i128/u128` → `i128`；`f32/f64` → 同宽浮点。`usize/isize` **不提供**（宽度随目标）。

use crate::Value;
use crate::entity::TypeId;
use crate::ir::builder::FunctionBuilder;

/// 把值物化成 IR 常量值（结果的 IR 类型由 `Self` 决定）。
pub trait AsConst: Sized {
    /// 物化：返回该常量在函数里的 SSA 值。
    fn into_const(self, fb: &mut FunctionBuilder) -> Value;
}

macro_rules! as_const_int {
    ($($ty:ty => $bits:literal),* $(,)?) => {
        $(
            impl AsConst for $ty {
                fn into_const(self, fb: &mut FunctionBuilder) -> Value {
                    fb.const_int(i128::from(self), fb.int_type($bits))
                }
            }
        )*
    };
}

as_const_int!(
    i8 => 8,
    i16 => 16,
    i32 => 32,
    i64 => 64,
    u8 => 8,
    u16 => 16,
    u32 => 32,
    u64 => 64,
);

impl AsConst for bool {
    fn into_const(self, fb: &mut FunctionBuilder) -> Value {
        fb.const_bool(self)
    }
}

impl AsConst for i128 {
    fn into_const(self, fb: &mut FunctionBuilder) -> Value {
        fb.const_int(self, TypeId::I128)
    }
}

/// `u128` 以 `as i128` 包装：IR 整数是**无符号位图**，二者位模式相同。
impl AsConst for u128 {
    fn into_const(self, fb: &mut FunctionBuilder) -> Value {
        fb.const_int(self as i128, TypeId::I128)
    }
}

impl AsConst for f32 {
    fn into_const(self, fb: &mut FunctionBuilder) -> Value {
        fb.const_float(self.to_bits(), fb.float_type(32))
    }
}

impl AsConst for f64 {
    fn into_const(self, fb: &mut FunctionBuilder) -> Value {
        fb.const_float(self.to_bits(), fb.float_type(64))
    }
}
