//! 目标无关的**类型视图**：引擎只需要"多大、怎么对齐、是什么形状"。
//!
//! 刻意不依赖 `forge_ir::TypeStore`：IR 的类型是 interned 句柄，而引擎要对
//! "任意真实编译器给的布局"（rustc 的 `Layout`、C 的 ABI 布局）都能工作——
//! 调用方把类型摊成本结构即可（rustc 前端直接把 `rustc_abi::Layout` 投影过来）。

/// 元素族（向量/HFA 判定用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Elem {
    Int,
    Float,
}

/// 类型形状（分类规则的判据面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TyKind {
    /// 整数 / 布尔 / 字符等标量。
    Int,
    /// 指针 / 引用 / 函数指针。
    Ptr,
    /// 浮点标量（f32/f64/f80…）。
    Float,
    /// 向量：元素族 + lane 数（如 `<4 x f32>`）。
    Vector { elem: Elem, lanes: u32 },
    /// 聚合：成员逐个列出（HFA/HVA 与"按成员拆寄存器"靠它判定）。
    ///
    /// 成员可递归；只关心"同族同宽成员的个数"的约定（AAPCS64 HFA ≤4、RISC-V HFA ≤2）
    /// 用扁平化的成员列表即可。
    Aggregate { members: Vec<TyView> },
    /// 其它（未建模的标量类型，如定点）：按字节大小当整数处理，但规则可单独识别。
    Other,
}

/// 一个类型的 ABI 视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TyView {
    /// 字节大小。
    pub size: u32,
    /// 字节对齐（ABI 要求的自然/声明对齐）。
    pub align: u32,
    pub kind: TyKind,
}

impl TyView {
    pub fn new(size: u32, align: u32, kind: TyKind) -> Self {
        Self { size, align, kind }
    }

    pub fn int(size: u32, align: u32) -> Self {
        Self::new(size, align, TyKind::Int)
    }

    pub fn ptr(size: u32) -> Self {
        Self::new(size, size, TyKind::Ptr)
    }

    pub fn float(size: u32) -> Self {
        Self::new(size, size, TyKind::Float)
    }

    pub fn vector(elem: Elem, lanes: u32, elem_bytes: u32) -> Self {
        let size = lanes * elem_bytes;
        Self::new(size, size.max(elem_bytes), TyKind::Vector { elem, lanes })
    }

    pub fn agg(align: u32, members: Vec<TyView>) -> Self {
        let size: u32 = members.iter().map(|m| m.size).sum();
        Self::new(size, align.max(1), TyKind::Aggregate { members })
    }

    /// 是否整数族（含指针与其它标量——多数约定把它们走同一个池）。
    pub fn is_int_like(&self) -> bool {
        matches!(self.kind, TyKind::Int | TyKind::Ptr | TyKind::Other)
    }

    pub fn is_float(&self) -> bool {
        matches!(self.kind, TyKind::Float)
    }

    pub fn is_vector(&self) -> bool {
        matches!(self.kind, TyKind::Vector { .. })
    }

    pub fn is_aggregate(&self) -> bool {
        matches!(self.kind, TyKind::Aggregate { .. })
    }

    /// **HFA（同质浮点聚合）**判定：全部成员都是**浮点**标量、且个数在 `max` 之内，
    /// 返回成员数。
    ///
    /// 判据与"为什么只看浮点"：
    ///
    /// - 成员必须同族且**同宽**（`{f32,f64}` 不是 HFA）；
    /// - **整数聚合不是 HFA**（那属于 "fit in N×XLEN 整数槽"，由 `aggregate size_le`
    ///   规则命中）。把 `{i64,i64}` 当 HFA 会让 RISC-V/AAPCS64 把它送进浮点池——
    ///   这就是"少写一条 `kind` 谓词、错值静默流到寄存器里"的典型；
    /// - 成员是向量/嵌套聚合时不成立（AAPCS64 的 HFA 允许嵌套，这里保守判否：
    ///   方向是"少认 HFA"，由 `Unsupported` 兜住而不是错值）。
    pub fn homogeneous_float_agg(&self, max: u32) -> Option<u32> {
        let TyKind::Aggregate { members } = &self.kind else {
            return None;
        };
        if members.is_empty() || members.len() as u32 > max {
            return None;
        }
        let first = &members[0];
        if !first.is_float() {
            return None;
        }
        let ok = members.iter().all(|m| m.is_float() && m.size == first.size);
        ok.then_some(members.len() as u32)
    }
}
