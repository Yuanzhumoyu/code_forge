//! **IR → 类型视图**：唯一的那份投影（v23 M2 合并）。
//!
//! ## 为什么只有这一份
//!
//! 判定"这个聚合是不是 HFA"靠**成员**（AAPCS64 的 `{f64,f64,f64}` 走浮点池），而调用点
//! （生成物按 IR 类型现摊）与被调方（函数签名摊）必须给出**同一个**形状——两份投影
//! 只要退化策略有一点不同（长数组折成几个成员、深度上限），HFA 就会一边认一边不认，
//! 实参搬错寄存器且**静默**。此前正是两份：运行时的 `ArgShape::from_ir_type`
//! 与代码生成的 `sig_view::ty_view`（后者还没有深度上限，递归类型会栈溢出）。
//!
//! 现在只有本模块一份；`forge-abi` 的引擎与生成物都从它取形状。
//!
//! ## 两条纪律
//!
//! 1. **大小/对齐一律取 `TypeStore` 的权威值**（`size_bytes`/`alignment`）——按成员求和
//!    会漏掉填充；向量的 size 也不按 `lanes × elem` 重算（非 2 幂长度、SVE 这类会错）。
//! 2. **摊不开就退化成"没有成员"**（深度 > [`MAX_DEPTH`] 或成员数 > [`MAX_MEMBERS`]）：
//!    那种大小的聚合必然走栈/间接，由规则兜底；**不假装**它是标量。

use crate::ty::{Elem, TyKind, TyView};
use forge_ir::TypeId;
use forge_ir::ir::types::{TypeEntry, TypeStore};

/// 成员摊开的深度上限（防御递归类型：结构体可以自引用）。
pub const MAX_DEPTH: u32 = 4;
/// 数组展开的成员数上限（超出的数组按"没有成员"处理）。
pub const MAX_MEMBERS: u64 = 16;

impl TyView {
    /// **IR 类型 → 类型视图**（调用方与被调方共用的唯一投影）。
    ///
    /// 递归摊开成员与 lane：HFA/HVA 判定、按成员拆寄存器都要靠它。
    pub fn from_ir_type(store: &TypeStore, ty: TypeId) -> Self {
        Self::from_ir_type_at(store, ty, 0)
    }

    fn from_ir_type_at(store: &TypeStore, ty: TypeId, depth: u32) -> Self {
        let size = store.size_bytes(ty).max(1);
        let align = store.alignment(ty).max(1);
        let member = |t: TypeId| Self::from_ir_type_at(store, t, depth + 1);
        let agg = |members: Vec<TyView>| TyView::new(size, align, TyKind::Aggregate { members });
        match store.get(ty) {
            TypeEntry::Float { .. } => TyView::float(size),
            TypeEntry::Pointer { .. } | TypeEntry::Function { .. } => TyView::ptr(size),
            TypeEntry::Int { .. } => TyView::int(size, align),
            // 向量的 size/align 用 store 的权威值（不按 lanes × elem 重算）。
            TypeEntry::Vector { elem, len } => TyView::new(
                size,
                align,
                TyKind::Vector {
                    elem: if store.is_float(*elem) {
                        Elem::Float
                    } else {
                        Elem::Int
                    },
                    lanes: (*len).max(1),
                },
            ),
            TypeEntry::Array { elem, len } => {
                if *len <= MAX_MEMBERS && depth < MAX_DEPTH {
                    agg((0..*len).map(|_| member(*elem)).collect())
                } else {
                    agg(Vec::new())
                }
            }
            TypeEntry::Struct { fields, .. } => {
                if depth < MAX_DEPTH {
                    agg(fields.iter().map(|f| member(f.ty)).collect())
                } else {
                    agg(Vec::new())
                }
            }
            // 可扩展向量（SVE/RVV）与其它：精确形状运行时才知道 ⇒ 交给规则兜底（`Other`）。
            _ => TyView::new(size, align, TyKind::Other),
        }
    }
}
