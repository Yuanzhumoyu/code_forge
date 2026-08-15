use crate::error::IrError;
use dashu::{
    Integer, Natural, Real,
    base::{Abs, ConversionError, Signed},
};
use thiserror::Error;

/// IEEE 754 浮点格式描述。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FloatFormat {
    pub exp_bits: u32,
    pub sig_bits: u32,
}

impl FloatFormat {
    pub const F16: Self = Self {
        exp_bits: 5,
        sig_bits: 10,
    };
    pub const F32: Self = Self {
        exp_bits: 8,
        sig_bits: 23,
    };
    pub const F64: Self = Self {
        exp_bits: 11,
        sig_bits: 52,
    };
    pub const F128: Self = Self {
        exp_bits: 15,
        sig_bits: 112,
    };

    pub fn total_bits(&self) -> u32 {
        1 + self.exp_bits + self.sig_bits
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Big {
    Signed(Integer),
    Unsigned(Natural),
    Float(Real),
}

/// 放置常量值。
impl Big {
    pub const S_ZERO: Big = Big::Signed(Integer::ZERO);
    pub const U_ZERO: Big = Big::Unsigned(Natural::ZERO);
    pub const F_ZERO: Big = Big::Float(Real::ZERO);

    pub const S_ONE: Big = Big::Signed(Integer::ONE);
    pub const U_ONE: Big = Big::Unsigned(Natural::ONE);
    pub const F_ONE: Big = Big::Float(Real::ONE);

    pub const S_NEG_ONE: Big = Big::Signed(Integer::NEG_ONE);
    pub const F_NEG_ONE: Big = Big::Float(Real::NEG_ONE);

    // pub const PI: Big = Big::Float(Real::from_parts((314159265358979323846).into(),-1));
}

impl Big {
    pub fn from_f64(value: f64) -> Option<Self> {
        // 处理特殊值
        if value.is_nan() {
            return None;
        }
        if value.is_infinite() {
            return Some(Big::Float(if value.is_sign_positive() {
                Real::INFINITY
            } else {
                Real::NEG_INFINITY
            }));
        }
        if value == 0.0 {
            return Some(Big::Float(if value.is_sign_positive() {
                Real::ZERO
            } else {
                -Real::ZERO
            }));
        }

        // 将 f64 分解为有效数和指数，通过 from_parts 构造 Real
        let bits = value.to_bits();
        let raw_exp = ((bits >> 52) & 0x7FF) as i32;
        let raw_sig = bits & ((1u64 << 52) - 1);

        if raw_exp == 0 {
            // 次正规数: value = sig × 2^(-1022-52)
            if raw_sig == 0 {
                return Some(Big::F_ZERO);
            }
            let sig = Integer::from(raw_sig);
            let exp: isize = -1074;
            Some(Big::Float(Real::from_parts(sig, exp)))
        } else {
            // 正规数: value = (1 + sig/2^52) × 2^(exp-1023)
            let sig = Integer::from(raw_sig | (1u64 << 52));
            let exp: isize = (raw_exp as isize) - 1075; // -1023 - 52 = -1075
            let result = if value.is_sign_positive() {
                Real::from_parts(sig, exp)
            } else {
                Real::from_parts(-sig, exp)
            };
            Some(Big::Float(result))
        }
    }
    /// 判断是否为零。
    pub const fn is_zero(&self) -> bool {
        match self {
            Self::Signed(i) => i.is_zero(),
            Self::Unsigned(i) => i.is_zero(),
            Self::Float(f) => f.repr().is_pos_zero() || f.repr().is_neg_zero(),
        }
    }

    /// 返回绝对值。
    pub fn abs(&self) -> Self {
        match self {
            Self::Signed(i) => Self::Signed(i.abs()),
            Self::Unsigned(i) => Self::Unsigned(i.clone()),
            Self::Float(f) => Self::Float(f.clone().abs()),
        }
    }
    pub const fn is_one(&self) -> bool {
        match self {
            Self::Signed(i) => i.is_one(),
            Self::Unsigned(i) => i.is_one(),
            Self::Float(f) => f.repr().is_one(),
        }
    }

    /// 判断是否为负数（Float 变体;整数恒 false）。
    pub fn is_neg(&self) -> bool {
        match self {
            Big::Signed(i) => i.is_negative(),
            Big::Float(f) => f.is_negative(),
            _ => false,
        }
    }
}

macro_rules! macro_impl_big_ops {
    ($($op_type:ident[$op:tt $op_fn:ident]),*) => {
        $(
            impl std::ops::$op_type for Big {
                type Output = Self;

                fn $op_fn(self, rhs: Self) -> Self::Output {
                    match (self, rhs) {
                        (Big::Signed(a), Big::Signed(b)) => Big::Signed(a $op b),
                        (Big::Signed(a), Big::Unsigned(b)) => Big::Signed(a $op b),
                        (Big::Signed(a), Big::Float(b)) => Big::Float(a $op b),
                        (Big::Unsigned(a), Big::Signed(b)) => Big::Signed(a $op b),
                        (Big::Unsigned(a), Big::Unsigned(b)) => Big::Unsigned(a $op b),
                        (Big::Unsigned(a), Big::Float(b)) => Big::Float(a $op b),
                        (Big::Float(a), Big::Signed(b)) => Big::Float(a $op b),
                        (Big::Float(a), Big::Unsigned(b)) => Big::Float(a $op b),
                        (Big::Float(a), Big::Float(b)) => Big::Float(a $op b),
                    }
                }
            }
        )*
    };
}

macro_impl_big_ops!(Add[+ add], Mul[* mul], Div[/ div]);

impl std::ops::Sub for Big {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Big::Unsigned(a), Big::Unsigned(b)) => {
                if a >= b {
                    Big::Unsigned(a - b)
                } else {
                    Big::Signed(Integer::from(a) - Integer::from(b))
                }
            }
            (Big::Signed(a), Big::Signed(b)) => Big::Signed(a - b),
            (Big::Signed(a), Big::Unsigned(b)) => Big::Signed(a - Integer::from(b)),
            (Big::Signed(a), Big::Float(b)) => Big::Float(Real::from(a) - b),
            (Big::Unsigned(a), Big::Signed(b)) => Big::Signed(Integer::from(a) - b),
            (Big::Unsigned(a), Big::Float(b)) => Big::Float(Real::from(a) - b),
            (Big::Float(a), Big::Signed(b)) => Big::Float(a - Real::from(b)),
            (Big::Float(a), Big::Unsigned(b)) => Big::Float(a - Real::from(b)),
            (Big::Float(a), Big::Float(b)) => Big::Float(a - b),
        }
    }
}

impl std::ops::Neg for Big {
    type Output = Self;

    fn neg(self) -> Self::Output {
        match self {
            Big::Signed(ibig) => Big::Signed(-ibig),
            Big::Unsigned(ubig) => Big::Signed(-ubig),
            Big::Float(fbig) => Big::Float(-fbig),
        }
    }
}

macro_rules! macro_number_to_big {
    (Unsigned($($ty:ty),*)) => {
        $(
            impl From<$ty> for Big {
                fn from(value: $ty) -> Self {
                    Big::Unsigned(Natural::from(value))
                }
            }
        )*
    };
    (Signed($($ty:ty),*)) => {
        $(
            impl From<$ty> for Big {
                fn from(value: $ty) -> Self {
                    Big::Signed(Integer::from(<_ as Into<Integer>>::into(value)))
                }
            }
        )*
    };
}

macro_number_to_big!(Signed(bool, i8, i16, i32, i64, i128, isize));
macro_number_to_big!(Unsigned(u8, u16, u32, u64, u128, usize));

impl std::fmt::Display for Big {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Big::Unsigned(value) => write!(f, "{}", value),
            Big::Signed(value) => write!(f, "{}", value),
            Big::Float(value) => write!(f, "{}", value),
        }
    }
}

macro_rules! impl_big_to_number {
    ($($ty:ty),*) => {
        $(
            impl TryFrom<Big> for $ty {
                type Error = BigConversionError;

                fn try_from(value: Big) -> Result<Self, Self::Error> {
                    match value {
                        Big::Unsigned(value) => <$ty as TryFrom<Natural>>::try_from(value),
                        Big::Signed(value) => <$ty as TryFrom<Integer>>::try_from(value),
                        Big::Float(value) => <$ty as TryFrom<Real>>::try_from(value),
                    }.map_err(BigConversionError::from)
                }
            }
        )*
    };
}

impl_big_to_number!(
    i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum BigConversionError {
    /// The number is not in the representation range
    #[error("The number is not in the representation range")]
    OutOfBounds,
    /// The conversion will cause a loss of precision
    #[error("The conversion will cause a loss of precision")]
    LossOfPrecision,
    /// 只支持将Big转换成f32,f64
    #[error("只支持将Big::Float转换成f32,f64")]
    FloatNotSupported,
}

impl From<ConversionError> for BigConversionError {
    fn from(value: ConversionError) -> Self {
        match value {
            ConversionError::OutOfBounds => Self::OutOfBounds,
            ConversionError::LossOfPrecision => Self::LossOfPrecision,
        }
    }
}

// ============================================================
// PartialOrd / Ord
// ============================================================

impl PartialOrd for Big {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Big::Float(a), Big::Float(b)) => a.partial_cmp(b),
            (Big::Float(_), _) | (_, Big::Float(_)) => {
                // Float vs non-float: compare via to_f64
                let a = self.to_f64_lossy();
                let b = other.to_f64_lossy();
                a.partial_cmp(&b)
            }
            _ => Some(self.cmp_total(other)),
        }
    }
}

impl Big {
    fn cmp_total(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Big::Signed(a), Big::Signed(b)) => a.cmp(b),
            (Big::Unsigned(a), Big::Unsigned(b)) => a.cmp(b),
            (Big::Signed(a), Big::Unsigned(b)) => a.cmp(&Integer::from(b.clone())),
            (Big::Unsigned(a), Big::Signed(b)) => Integer::from(a.clone()).cmp(b),
            _ => std::cmp::Ordering::Equal,
        }
    }

    fn to_f64_lossy(&self) -> f64 {
        match self {
            Big::Float(f) => f64::try_from(f.clone()).unwrap_or(0.0),
            Big::Signed(i) => {
                if let Ok(v) = i64::try_from(i) {
                    v as f64
                } else {
                    f64::INFINITY
                }
            }
            Big::Unsigned(u) => {
                if let Ok(v) = u64::try_from(u) {
                    v as f64
                } else {
                    f64::INFINITY
                }
            }
        }
    }
}

// ============================================================
// 运算符: Rem, BitAnd, BitOr, BitXor, Shl, Shr
// ============================================================

impl std::ops::Rem for Big {
    type Output = Self;
    fn rem(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Big::Signed(a), Big::Signed(b)) => Big::Signed(a % b),
            (Big::Signed(a), Big::Unsigned(b)) => Big::Signed(a % Integer::from(b)),
            (Big::Unsigned(a), Big::Signed(b)) => Big::Signed(Integer::from(a) % b),
            (Big::Unsigned(a), Big::Unsigned(b)) => Big::Unsigned(a % b),
            // Float 取模 = fmod（dashu Real 的 `%`）。混合类型经 f64 中间值
            // （有损但语义明确——不再静默返回第一个操作数）。
            (Big::Float(a), Big::Float(b)) => Big::Float(a % b),
            (Big::Float(a), b) => {
                let af = f64::try_from(a.clone()).unwrap_or(0.0);
                Big::from_f64(af % b.to_f64_lossy()).unwrap_or(Big::F_ZERO)
            }
            (a, Big::Float(b)) => {
                let bf = f64::try_from(b.clone()).unwrap_or(0.0);
                Big::from_f64(a.to_f64_lossy() % bf).unwrap_or(Big::F_ZERO)
            }
        }
    }
}

/// 位运算操作数必须为整数——Float 参与位运算是未定义语义，
/// 显式 panic 而非静默当作 0（静默 0 会产出错误的折叠结果）。
fn bitop_integer(b: Big) -> Integer {
    match b {
        Big::Signed(i) => i,
        Big::Unsigned(u) => Integer::from(u),
        Big::Float(_) => panic!("bitwise operation on Float Big is undefined"),
    }
}

impl std::ops::BitAnd for Big {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Big::Unsigned(a), Big::Unsigned(b)) => Big::Unsigned(a & b),
            (a, b) => Big::Signed(bitop_integer(a) & bitop_integer(b)),
        }
    }
}

impl std::ops::BitOr for Big {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Big::Signed(bitop_integer(self) | bitop_integer(rhs))
    }
}

impl std::ops::BitXor for Big {
    type Output = Self;
    fn bitxor(self, rhs: Self) -> Self::Output {
        Big::Signed(bitop_integer(self) ^ bitop_integer(rhs))
    }
}

impl std::ops::Shl<usize> for Big {
    type Output = Self;
    fn shl(self, rhs: usize) -> Self::Output {
        match self {
            Big::Signed(i) => Big::Signed(i << rhs),
            Big::Unsigned(u) => Big::Unsigned(u << rhs),
            Big::Float(_) => self,
        }
    }
}

impl std::ops::Shr<usize> for Big {
    type Output = Self;
    fn shr(self, rhs: usize) -> Self::Output {
        match self {
            Big::Signed(i) => Big::Signed(i >> rhs),
            Big::Unsigned(u) => Big::Unsigned(u >> rhs),
            Big::Float(_) => self,
        }
    }
}

// ============================================================
// 便捷方法
// ============================================================

impl Big {
    // 构造
    pub fn from_i64(value: i64) -> Self {
        Big::Signed(Integer::from(value))
    }
    pub fn from_u64(value: u64) -> Self {
        Big::Unsigned(Natural::from(value))
    }
    pub fn from_i128(value: i128) -> Self {
        Big::Signed(Integer::from(value))
    }

    // 类型查询
    pub fn is_float(&self) -> bool {
        matches!(self, Big::Float(_))
    }
    pub fn try_to_i64(&self) -> Option<i64> {
        match self {
            Big::Signed(i) => i64::try_from(i).ok(),
            Big::Unsigned(u) => i64::try_from(u).ok(),
            _ => None,
        }
    }
    pub fn try_to_u64(&self) -> Option<u64> {
        match self {
            Big::Signed(i) => u64::try_from(i).ok(),
            Big::Unsigned(u) => u64::try_from(u).ok(),
            _ => None,
        }
    }
    pub fn to_f64(&self) -> f64 {
        match self {
            Big::Float(f) => f64::try_from(f.clone()).unwrap_or(f64::NAN),
            _ => self.to_f64_lossy(),
        }
    }
    pub fn trunc_to_u64(&self) -> u64 {
        self.try_to_u64().unwrap_or(0)
    }

    // 位宽截断
    pub fn truncate_to_bits(&self, bits: u32) -> Self {
        if bits == 0 {
            return Big::U_ZERO;
        }
        match self {
            Big::Unsigned(u) => {
                let modulus = Natural::from(1u32) << bits as usize;
                Big::Unsigned(u % modulus)
            }
            Big::Signed(i) => {
                let modulus = Natural::from(1u32) << bits as usize;
                let abs = Natural::try_from(i.abs()).unwrap_or(Natural::ZERO);
                let rem = &abs % &modulus;
                if i.is_negative() && !rem.is_zero() {
                    Big::Unsigned(&modulus - &rem)
                } else {
                    Big::Unsigned(rem)
                }
            }
            Big::Float(_) => Big::U_ZERO,
        }
    }

    pub fn truncate_to_bits_signed(&self, bits: u32) -> Self {
        if bits == 0 {
            return Big::S_ZERO;
        }
        let unsigned = self.truncate_to_bits(bits);
        let sign_bit = Natural::from(1u32) << (bits as usize - 1);
        let has_sign = match &unsigned {
            Big::Unsigned(u) => !(u & &sign_bit).is_zero(),
            _ => false,
        };
        if has_sign {
            let power = Natural::from(1u32) << bits as usize;
            if let Big::Unsigned(u) = unsigned {
                let val = &power - &u;
                Big::Signed(-Integer::from(val))
            } else {
                Big::S_ZERO
            }
        } else {
            match unsigned {
                Big::Unsigned(u) => Big::Signed(Integer::from(u)),
                _ => Big::S_ZERO,
            }
        }
    }

    // 除法变体
    pub fn sdiv(&self, other: &Self) -> Result<Self, IrError> {
        if other.is_zero() {
            return Err(IrError::DivisionByZero);
        }
        let a = self.clone();
        let b = other.clone();
        Ok(match (a, b) {
            (Big::Signed(a), Big::Signed(b)) => Big::Signed(a / b),
            (Big::Signed(a), Big::Unsigned(b)) => Big::Signed(a / Integer::from(b)),
            (Big::Unsigned(a), Big::Signed(b)) => Big::Signed(Integer::from(a) / b),
            (Big::Unsigned(a), Big::Unsigned(b)) => Big::Unsigned(a / b),
            _ => Big::S_ZERO,
        })
    }

    pub fn udiv(&self, other: &Self) -> Result<Self, IrError> {
        if other.is_zero() {
            return Err(IrError::DivisionByZero);
        }
        let a = self.abs_as_unsigned();
        let b = other.abs_as_unsigned();
        Ok(Big::Unsigned(a / b))
    }

    pub fn srem(&self, other: &Self) -> Result<Self, IrError> {
        if other.is_zero() {
            return Err(IrError::DivisionByZero);
        }
        Ok(self.clone() % other.clone())
    }

    pub fn urem(&self, other: &Self) -> Result<Self, IrError> {
        if other.is_zero() {
            return Err(IrError::DivisionByZero);
        }
        let a = self.abs_as_unsigned();
        let b = other.abs_as_unsigned();
        Ok(Big::Unsigned(a % b))
    }

    fn abs_as_unsigned(&self) -> Natural {
        match self {
            Big::Unsigned(u) => u.clone(),
            Big::Signed(i) => Natural::try_from(i.abs()).unwrap_or(Natural::ZERO),
            Big::Float(f) => {
                Natural::try_from(Integer::from(f64::try_from(f.clone()).unwrap_or(0.0) as i64))
                    .unwrap_or(Natural::ZERO)
            }
        }
    }

    // 浮点位模式编解码
    pub fn from_bits(bits: u64, fmt: FloatFormat) -> Self {
        let f = match fmt.total_bits() {
            16 => {
                let sign = bits >> 15;
                let exp = ((bits >> 10) & 0x1F) as i32;
                let sig = bits & 0x3FF;
                if exp == 0 {
                    if sig == 0 {
                        f64::from_bits(sign << 63)
                    } else {
                        let mut s = sig;
                        let mut e = -14i32;
                        while (s & 0x400) == 0 {
                            s <<= 1;
                            e -= 1;
                        }
                        f64::from_bits(
                            (sign << 63) | (((e + 1023) as u64) << 52) | ((s & 0x3FF) << 42),
                        )
                    }
                } else if exp == 0x1F {
                    f64::from_bits((sign << 63) | (0x7FF_u64 << 52) | (sig << 42))
                } else {
                    f64::from_bits((sign << 63) | (((exp - 15 + 1023) as u64) << 52) | (sig << 42))
                }
            }
            32 => f32::from_bits(bits as u32) as f64,
            _ => f64::from_bits(bits),
        };
        Big::from_f64(f).unwrap_or(Big::F_ZERO)
    }

    pub fn to_bits_trunc(&self, fmt: FloatFormat) -> u64 {
        let f = self.to_f64();
        match fmt.total_bits() {
            16 => {
                let bits = f.to_bits();
                let sign = ((bits >> 63) & 1) as u16;
                let exp = ((bits >> 52) & 0x7FF) as i32;
                let sig = bits & ((1u64 << 52) - 1);
                if exp == 0 {
                    return (sign as u64) << 15;
                }
                if exp == 0x7FF {
                    return if sig == 0 {
                        ((sign as u64) << 15) | (0x1F << 10)
                    } else {
                        ((sign as u64) << 15) | (0x1F << 10) | ((sig >> 42) & 0x3FF) | 0x200
                    };
                }
                let f16_exp = exp - 1023 + 15;
                if f16_exp <= 0 {
                    if f16_exp < -10 {
                        return (sign as u64) << 15;
                    }
                    return ((sign as u64) << 15) | ((sig | (1u64 << 52)) >> (1 - f16_exp + 42));
                }
                if f16_exp >= 0x1F {
                    return ((sign as u64) << 15) | (0x1F << 10);
                }
                let f16_sig = (sig >> 42) as u16;
                let round = (sig >> 41) & 1;
                let sticky = (sig & ((1u64 << 41) - 1)) != 0;
                let mut r = f16_sig;
                if round == 1 && (sticky || (f16_sig & 1) != 0) {
                    r += 1;
                }
                ((sign as u64) << 15) | ((f16_exp as u64) << 10) | (r as u64 & 0x3FF)
            }
            32 => (f as f32).to_bits() as u64,
            _ => f.to_bits(),
        }
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // === 整数测试 ===
    #[test]
    fn from_i64_conversions() {
        assert_eq!(Big::from_i64(42).try_to_i64(), Some(42));
    }
    #[test]
    fn add_sub() {
        let a = Big::from_i64(100);
        let b = Big::from_i64(50);
        assert_eq!((a.clone() + b.clone()).try_to_i64(), Some(150));
        assert_eq!((a.clone() - b.clone()).try_to_i64(), Some(50));
    }

    #[test]
    fn mul() {
        assert_eq!(
            (Big::from_i64(12) * Big::from_i64(34)).try_to_i64(),
            Some(408)
        );
    }

    #[test]
    fn div_rem() {
        let a = Big::from_i64(100);
        let b = Big::from_i64(7);
        assert_eq!(a.sdiv(&b).unwrap().try_to_i64(), Some(14));
        assert_eq!(a.srem(&b).unwrap().try_to_i64(), Some(2));
    }

    #[test]
    fn bitwise() {
        let a = Big::from_i64(0b1100);
        let b = Big::from_i64(0b1010);
        assert_eq!((a.clone() & b.clone()).try_to_i64(), Some(0b1000));
        assert_eq!((a | b).try_to_i64(), Some(0b1110));
    }

    #[test]
    fn shifts() {
        assert_eq!((Big::from_i64(3) << 2).try_to_i64(), Some(12));
        assert_eq!((Big::from_i64(-16) >> 2).try_to_i64(), Some(-4));
        assert_eq!((Big::from_u64(16) >> 2).try_to_u64(), Some(4));
    }

    #[test]
    fn truncate_to_bits() {
        assert_eq!(
            Big::from_i64(0x12345678).truncate_to_bits(16).try_to_u64(),
            Some(0x5678)
        );
        assert_eq!(
            Big::from_i64(-1).truncate_to_bits_signed(8).try_to_i64(),
            Some(-1)
        );
        assert_eq!(
            Big::from_i64(0xFF).truncate_to_bits_signed(8).try_to_i64(),
            Some(-1)
        );
    }

    #[test]
    fn i128_conversions() {
        assert!(Big::from_i128(i128::MAX).try_to_i64().is_none()); // too large for i64
        assert_eq!(Big::from_i128(42).try_to_i64(), Some(42));
    }

    #[test]
    fn zero_ops() {
        assert!((Big::S_ZERO * Big::from_i64(5)).is_zero());
        assert_eq!(
            Big::from_i64(5)
                .sdiv(&Big::from_i64(5))
                .unwrap()
                .try_to_i64(),
            Some(1)
        );
    }

    #[test]
    fn display() {
        assert_eq!(format!("{}", Big::from_i64(42)), "42");
        assert_eq!(format!("{}", Big::from_i64(-42)), "-42");
    }

    // === 浮点测试 ===
    #[test]
    fn from_f64_roundtrip() {
        for &v in &[
            0.0f64,
            1.0,
            -1.0,
            std::f64::consts::PI,
            -std::f64::consts::E,
            1.5,
            -0.5,
            42.0,
        ] {
            let bf = Big::from_f64(v).unwrap();
            assert!(
                (bf.to_f64() - v).abs() < 1e-10,
                "roundtrip failed for {}",
                v
            );
        }
    }

    #[test]
    fn float_negate() {
        let a = Big::from_f64(3.0).unwrap();
        assert!((-a.to_f64() + 3.0).abs() < 1e-10);
    }

    #[test]
    fn float_add_sub() {
        let a = Big::from_f64(3.0).unwrap();
        let b = Big::from_f64(2.0).unwrap();
        assert!(((a.clone() + b.clone()).to_f64() - 5.0).abs() < 1e-10);
        assert!(((a - b).to_f64() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn float_mul_div() {
        let a = Big::from_f64(6.0).unwrap();
        let b = Big::from_f64(2.0).unwrap();
        assert!(((a.clone() * b.clone()).to_f64() - 12.0).abs() < 1e-10);
        assert!(((a / b).to_f64() - 3.0).abs() < 1e-10);
    }

    #[test]
    fn float_from_bits_f16() {
        let bf = Big::from_bits(0x3C00, FloatFormat::F16);
        assert!((bf.to_f64() - 1.0).abs() < 0.01);
        assert_eq!(bf.to_bits_trunc(FloatFormat::F16), 0x3C00);
    }

    #[test]
    fn float_from_bits_f32() {
        let bf = Big::from_bits(0x40490FDB, FloatFormat::F32);
        assert_eq!(bf.to_bits_trunc(FloatFormat::F32), 0x40490FDB);
    }

    // === Big enum 特定测试 ===
    #[test]
    fn cross_type_addition() {
        let s = Big::from_i64(10);
        let u = Big::from_u64(5);
        let sum = s + u;
        assert_eq!(sum.try_to_i64(), Some(15));
    }

    #[test]
    fn float_to_int_conversion() {
        let f = Big::from_f64(1.5).unwrap();
        assert!(f.is_float());
    }

    #[test]
    fn wide_unsigned_multiplication() {
        let a = Big::from_u64(u64::MAX);
        let b = Big::from_u64(u64::MAX);
        let prod = a * b;
        assert!(prod.try_to_u64().is_none()); // too large
    }
}
