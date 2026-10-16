//! 合法化 pass：**调用侧**聚合展开（sret 形参、call 结果拆段）。
//!
//! 从 `compiler.rs` 迁出（v23 M6 批次 2）——pass 体不再住在调用点旁边，
//! 由 `legalize::LegalizeDriver` 定序调用。
use crate::IrError;
use crate::pipeline::legalize::common::append_param;
use forge_ir::*;

/// （`plan.ret = Indirect`，即 sret）时，给函数**追加**一个带 `sret` 属性的 `ptr` 形参，
/// 并把签名的返回清空——值写进缓冲由 [`expand_large_agg_ret`] 完成。
///
/// **必须先于其它聚合展开**：那些展开（形参/实参/call 结果）会把返回值重写或登记进
/// `agg_slots`，早一步把 sret 形参立起来，后续它们才知道"返回值往哪儿写"。
pub(crate) fn expand_sret_return(
    func: &mut Function,
    edges: &crate::pipeline::legalize::plan::AbiEdges,
) -> Result<Option<Value>, IrError> {
    // **只读计划**（v25 M1）：宽聚合返回是不是 sret，由计划相给出的 `RetLoc` 说
    //（`Indirect` = 调用方出缓冲）。本 pass 不再问引擎。
    if !matches!(edges.ret, Some(forge_abi::RetLoc::Indirect { .. })) {
        return Ok(None);
    }
    // **不写 `sret` 属性**（与 A1′ 的 by-ref 形参同一条规律）：引擎对带 sret/byval 的落点
    // 给 `Indirect`，而生成物 `move_args` 的 `Indirect` 收参是"从指针 **load** 出值"
    // （值为寄存器语义）——那样形参拿到的是**缓冲里的内容**而不是缓冲地址。
    // 按约定，sret 缓冲指针本来就落在**首个整数槽**（win64 = RCX、sysv64 = RDI），
    // 与"一个普通的第 0 个指针实参"完全同形 ⇒ 直接用普通 `ptr` 形参，调用方把它当实参 0 传。
    let p = append_param(func, TypeId::PTR, "<sret>", Default::default())?;
    // 签名返回清空：缓冲由 sret 形参承载（引擎把它收进约定的隐藏返回槽）
    let old_sig = func.types.borrow().get_signature(func.signature).clone();
    let new_sig = FunctionSignature {
        params: old_sig.params.clone(),
        returns: Vec::new(),
        calling_convention: old_sig.calling_convention,
        variadic: old_sig.variadic,
    };
    func.signature = func.types.borrow_mut().register_signature(new_sig);
    Ok(Some(p))
}

mod args;
mod results;

pub(crate) use args::expand_agg_call_args;
pub(crate) use results::{expand_large_agg_call_results, expand_large_agg_ret};
