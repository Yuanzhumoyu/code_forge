//! **IR 视图 → 引擎视图**（v20 A2b）：把 `FunctionSignature` 与 `Function` 上的
//! `ParamAttributes`/`ret_attrs` 投影成 `forge_abi::Signature`（类型摊成 [`TyView`]、
//! 属性折成 [`DeclAttrs`]）。
//!
//! ## 为什么需要这一层
//!
//! `ParamAttributes` 早就存在（`byval`/`sret`/`inreg`/`zeroext`/`signext`/`align`…），
//! 文本层也一直能解析/打印它——但**没有任何代码读它**：它既不影响落点也不影响发射，
//! 与旧 `CallConv` 一样是装饰。本模块把它接到引擎的输入面上，于是：
//!
//! ```text
//! Function.param_attrs[i]  ──┐
//! FunctionSignature.params ──┼─► signature_view() ─► forge_abi::Signature
//! TypeStore（大小/对齐）    ──┘                          │
//!                                                       ▼
//!                              plan_fn(target, rules, binding, sig) → AbiPlan
//! ```
//!
//! 两条纪律：
//!
//! 1. **类型只摊开、不猜测**：大小/对齐一律取自 `TypeStore`（聚合体尤其——按成员求和的
//!    裸算会漏掉填充，尺寸必须用 store 的权威值）；摊不开的类型（可扩展向量等）落到
//!    `TyKind::Other`，由规则兜底，不假装它是标量。
//! 2. **属性只映射、不发明**：`byval(TypeId)` 的字节数由那个类型算；`align = 0` = 未声明。

use forge_abi::{DeclAttrs, Elem, Signature, TyKind, TyView};
use forge_ir::TypeId;
use forge_ir::error::IrError;
use forge_ir::ir::function::{Function, ParamAttributes};
use forge_ir::ir::types::{TypeEntry, TypeStore};

/// `ParamAttributes` → 引擎的**声明属性**（`byval` 的字节数由类型算）。
pub fn decl_attrs(a: &ParamAttributes, store: &TypeStore) -> DeclAttrs {
    DeclAttrs {
        byval: a.byval.map(|t| store.size_bytes(t).max(1)),
        sret: a.sret.is_some(),
        inreg: a.inreg,
        zeroext: a.zeroext,
        signext: a.signext,
        align: (a.align > 0).then_some(a.align),
    }
}

/// 一个类型 → 引擎的**类型视图**（`size`/`align` 取自 `TypeStore`）。
pub fn ty_view(store: &TypeStore, ty: TypeId) -> TyView {
    let size = store.size_bytes(ty).max(1);
    let align = store.alignment(ty).max(1);
    let kind = match store.get(ty) {
        TypeEntry::Int { .. } => TyKind::Int,
        TypeEntry::Float { .. } | TypeEntry::BFloat { .. } => TyKind::Float,
        TypeEntry::Pointer { .. } | TypeEntry::Function { .. } => TyKind::Ptr,
        TypeEntry::Vector { elem, len } => element_kind(store, *elem, *len, size),
        // 可扩展向量（SVE/RVV）：精确字节数运行时才知道 ⇒ 交给规则兜底（`other`）。
        TypeEntry::ScalableVector { .. } => TyKind::Other,
        TypeEntry::Array { elem, len } => {
            let members = expand_members(store, *elem, *len);
            TyKind::Aggregate { members }
        }
        TypeEntry::Struct { fields, .. } => TyKind::Aggregate {
            members: fields
                .iter()
                .map(|f| ty_view(store, f.ty))
                .collect::<Vec<_>>(),
        },
        _ => TyKind::Other,
    };
    TyView::new(size, align, kind)
}

/// 向量：元素族 + lane 数（摊不出来时按 `other`）。
///
/// `size` 是 store 给的权威字节数（`<3 x f32>` 这类非 2 幂长度也照实记）。
fn element_kind(store: &TypeStore, elem: TypeId, len: u32, _size: u32) -> TyKind {
    let elem_view = ty_view(store, elem);
    let family = match elem_view.kind {
        TyKind::Float => Elem::Float,
        TyKind::Int => Elem::Int,
        _ => return TyKind::Other,
    };
    TyKind::Vector {
        elem: family,
        lanes: len.max(1),
    }
}

/// 数组 → 聚合成员（**只为 HFA/HVA 判定**；取前 [`MAX_EXPANDED_MEMBERS`] 个成员，
/// 超出就当成"一个不可拆的整块"——那种大小的数组本来就 >16B，走 byval/栈）。
fn expand_members(store: &TypeStore, elem: TypeId, len: u64) -> Vec<TyView> {
    const MAX_EXPANDED_MEMBERS: u64 = 16;
    if len <= MAX_EXPANDED_MEMBERS {
        return (0..len).map(|_| ty_view(store, elem)).collect();
    }
    let elem_size = store.size_bytes(elem).max(1);
    let total = elem_size.saturating_mul(len as u32);
    vec![TyView::int(total, store.alignment(elem).max(1))]
}

/// `Function` 的 ABI 视图：签名 + 声明属性（`param_attrs` 是按参数索引的）。
///
/// 返回的错误只有一种情形：函数引用的签名句柄不在类型表里（IR 被改坏）。
pub fn signature_view(func: &Function) -> Result<Signature, IrError> {
    let store = func.types.borrow();
    let sig = store
        .signature_opt(func.signature)
        .ok_or_else(|| IrError::Unsupported("函数引用的签名不在类型表里（IR 结构损坏）".into()))?;

    let params: Vec<(String, TyView)> = sig
        .params
        .iter()
        .map(|(ty, name)| (name.to_string(), ty_view(&store, *ty)))
        .collect();
    // 声明属性按参数索引对齐（`param_attrs` 可以比参数少）。
    let attrs: Vec<DeclAttrs> = (0..params.len())
        .map(|i| {
            func.param_attrs
                .get(i)
                .map(|a| decl_attrs(a, &store))
                .unwrap_or_default()
        })
        .collect();
    let ret_attrs = func
        .ret_attrs
        .first()
        .map(|a| decl_attrs(a, &store))
        .unwrap_or_default();

    Ok(Signature {
        params,
        attrs,
        ret: sig.returns.first().copied().map(|t| ty_view(&store, t)),
        ret_attrs,
        variadic: sig.variadic,
        fixed_count: sig.params.len(),
    })
}
