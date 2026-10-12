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
//! 1. **类型只摊开、不猜测**：投影**只有一份**——`TyView::from_ir_type`
//!    （`forge-abi-model`；调用点/被调方/本模块共用），大小/对齐一律取 `TypeStore` 的权威值
//!    （按成员求和会漏掉填充）；摊不开的类型（可扩展向量等）落到 `TyKind::Other`，
//!    由规则兜底，不假装它是标量。
//! 2. **属性只映射、不发明**：`byval(TypeId)` 的字节数由那个类型算；`align = 0` = 未声明。

use forge_abi::{DeclAttrs, Signature, TyView};
use forge_ir::error::IrError;
use forge_ir::ir::function::{Function, ParamAttributes};
use forge_ir::ir::types::TypeStore;

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
        .map(|(ty, name)| (name.to_string(), TyView::from_ir_type(&store, *ty)))
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
        // **全部**返回类型（v20 A6 的多值返回）：IR 的 `returns: Vec<TypeId>` 就是
        // 多值返回的来源（ScalarPair 拆出来的两个标量、`(i64,i64,i64)` 这类），
        // 只取 `first()` 会把"第二个值在哪个寄存器"留给发射侧硬编码（旧实现就是
        // 写死 x86 的类内号 1 = RDX，换台机器就错）。
        rets: sig
            .returns
            .iter()
            .map(|t| TyView::from_ir_type(&store, *t))
            .collect::<Vec<_>>(),
        ret_attrs,
        variadic: sig.variadic,
        fixed_count: sig.params.len(),
    })
}
