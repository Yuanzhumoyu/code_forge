//! 合法化**分类**的唯一入口（v25 M1）。
//!
//! ## 为什么要有这一层
//!
//! v24 的两相化只做完了**改写**那一半：计划相产出的 `AbiEdges` 无人消费，而落点决策仍散在
//! 5 个调用点（`agg_param` / `agg_call` ×3 / `plan`），每处各自跑一遍引擎规划
//! （`plan_for_shapes` + 注册表读锁），并用本仓手抄的落点枚举表达（它只是 `Placement`
//! 的三值投影）。
//!
//! 本文件是**唯一**允许问引擎的地方（守卫 `classification_has_one_entry` 钉住）：
//!
//! * **规范类型**：返回 `forge-abi-model` 的 `ArgLoc` / `RetLoc`——`Placement` 已表达
//!   "寄存器 / 按值整块栈 / 间接（含调用方副本）"，并自带消费者访问器
//!   （`as_reg` / `indirect_ptr` / `stack_offset` / `is_stack_byval` / `reg_refs`）
//!   ⇒ 本仓不再手抄一份落点枚举（外部参考：ClangIR 的"ABI 分类单一来源 + rewriter 只消费分类"）。
//! * **去重**：同一类型在一个编译单元里只问一次（旧实现每个使用点各问一次，且每次都重跑
//!   引擎规划 + 读锁）。
//! * **形状必须摊开成员**：一律走 `TyView::from_ir_type`（C1 修复的不变量：早期伪造
//!   `Aggregate { members: [] }`，AAPCS64/LP64D 的 HFA 被当成普通聚合 ⇒ 调用方按"本地副本 +
//!   指针"改写、被调方按浮点寄存器收参 ⇒ 静默错值）。
//!
//! **请求形状与旧探针一致**（单类型请求），因此**决策内容不变**——v25 M1 只改"决策从哪来、
//! 问几次"。改用"整签名一次问全"（更贴合真实落点，尤其是前面已有栈实参时的
//! `StackByVal.offset`）是**下一步**的候选，届时以全量门禁 + 对拍测试为证据单独评估。

use crate::IrError;
use forge_abi::{ArgLoc, Placement, RetLoc};
use forge_ir::TypeId;
use forge_isa_runtime::ir::ir::types::TypeStore;
use forge_isa_runtime::machine::abi_model::TyView;
use forge_isa_runtime::machine::call_plan::CallRequest;
use forge_isa_runtime::machine::target::TargetMachine;
use std::collections::HashMap;

/// **分类器**：一次编译里问引擎的唯一通道（按类型去重）。
pub(crate) struct Classifier<'a, M: TargetMachine> {
    machine: &'a M,
    conv: &'a str,
    store: &'a TypeStore,
    /// 实参落点缓存（同一类型只问一次）。
    args: HashMap<TypeId, ArgLoc>,
    /// 返回落点缓存（同上）。
    rets: HashMap<TypeId, RetLoc>,
    /// callee-saved 区平移（与 `LowerCtx::stack_slot_shift` 同源；首次问路时算好）。
    shift: Option<i32>,
    /// 引擎规划次数（守卫 `classification_is_deduplicated` 与诊断用）。
    plans: usize,
}

impl<'a, M: TargetMachine> Classifier<'a, M> {
    pub fn new(machine: &'a M, conv: &'a str, store: &'a TypeStore) -> Self {
        Self {
            machine,
            conv,
            store,
            args: HashMap::new(),
            rets: HashMap::new(),
            shift: None,
            plans: 0,
        }
    }

    /// 聚合**实参**落点（同一类型只问一次）。返回的是 `forge-abi-model` 的规范落点。
    ///
    /// **返回拥有权**而不是 `&ArgLoc`：调用点常在同一个 `match` 里还要问 `shift()`，
    /// 而 `&mut self` 借出的引用会把整个分类器借走（实测：`E0502`）。克隆代价可忽略
    /// （每个聚合形参/实参一次），换来调用点无需关心借用次序。
    pub fn arg_place(&mut self, ty: TypeId) -> Result<Placement, IrError> {
        if !self.args.contains_key(&ty) {
            let req = CallRequest::new(self.conv.to_string())
                .args([TyView::from_ir_type(self.store, ty)]);
            let plan = self.plan(req).map_err(|e| {
                crate::IrError::capability(
                    "AGG_PLACE_PROBE",
                    format!("聚合落点查询：约定 `{}` 规划失败（{e}）", self.conv),
                )
            })?;
            let loc = plan.args.first().cloned().ok_or_else(|| {
                IrError::Internal(format!(
                    "聚合落点查询：约定 `{}` 规划结果里没有该实参（计划与请求不一致）",
                    self.conv
                ))
            })?;
            self.remember_shift(&plan);
            self.args.insert(ty, loc);
        }
        Ok(self.args.get(&ty).expect("上面刚插入").place.clone())
    }

    /// 聚合**返回**落点（同一类型只问一次）。`Indirect` 即 sret（见 [`Self::ret_is_sret`]）。
    pub fn ret_place(&mut self, ty: TypeId) -> Result<RetLoc, IrError> {
        if !self.rets.contains_key(&ty) {
            let req = CallRequest::new(self.conv.to_string())
                .rets([TyView::from_ir_type(self.store, ty)]);
            let plan = self.plan(req).map_err(|e| {
                crate::IrError::capability(
                    "AGG_RET_PLACE_PROBE",
                    format!("聚合返回落点查询：约定 `{}` 规划失败（{e}）", self.conv),
                )
            })?;
            self.remember_shift(&plan);
            let ret = plan.ret.clone();
            self.rets.insert(ty, ret);
        }
        Ok(self.rets.get(&ty).expect("上面刚插入").clone())
    }

    /// callee-saved 区平移：`[[lower.StackAddr]]` 的 lea 位移 = 立即数 − 平移，
    /// 所以被调方要落在 `[fp + ABI 偏移]` 上就得把平移加回立即数。
    /// 尚未问过任何落点（没有 plan）时返回 0——与旧探针"没有 plan 就没有平移"一致。
    pub fn shift(&self) -> i32 {
        self.shift.unwrap_or(0)
    }

    /// 引擎规划次数（守卫与诊断用）。
    pub fn plans(&self) -> usize {
        self.plans
    }

    /// 跑一次引擎规划（**不带**缺口 ID）：调用点各自用**字面量** ID 构造缺口错误——
    /// 台账守卫靠"字面量写在构造点"来识别失败点，把 ID 变成变量会让它看不见，
    /// 台账行随即被判成**死行**（实测撞到过）。
    fn plan(&mut self, req: CallRequest) -> Result<forge_abi::AbiPlan, forge_abi::AbiError> {
        self.plans += 1;
        let reg = crate::pipeline::conv_registry::registry()
            .read()
            .expect("约定注册表被投毒");
        crate::pipeline::abi_target::plan_for_shapes(self.machine, &reg, &req)
    }

    fn remember_shift(&mut self, plan: &forge_abi::AbiPlan) {
        if self.shift.is_none() {
            self.shift = Some(
                crate::pipeline::frame_layout::frame_layout_info(self.machine, Some(plan))
                    .stack_slot_shift,
            );
        }
    }
}

/// "这份约定把这个大小的聚合判成了**标量**栈落点" ⇒ 约定数据有问题（不是能力缺口）。
///
/// 单独成函数是为了让调用点写清口径：`Placement::Stack` 的语义是"一格一个值"，
/// 聚合走它意味着规则数据自相矛盾（v24 M3 的边界分类：这属于**数据错误**，不是缺口）。
pub(crate) fn scalar_stack_is_a_data_error(conv: &str, size: u32) -> IrError {
    IrError::Semantic(format!(
        "聚合落点查询：约定 `{conv}` 把 {size} 字节聚合判成标量栈落点（约定数据有问题）"
    ))
}

/// 落点是否"按引用传指针"（`Indirect`：指针指向调用方副本）。
pub(crate) fn is_by_ref(place: &Placement) -> bool {
    matches!(place, Placement::Indirect { .. })
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_ir::ir::types::TypeContext;

    fn store_with_agg() -> (TypeContext, TypeId) {
        let ctx = TypeContext::new();
        let agg = ctx
            .borrow_mut()
            .struct_anon(vec![TypeId::I64, TypeId::I64, TypeId::I64], false);
        (ctx, agg)
    }

    /// **去重**：同一类型问两次，引擎只被调用一次（旧实现每个使用点各问一次）。
    #[test]
    fn classification_is_deduplicated() {
        let (ctx, agg) = store_with_agg();
        let store = ctx.borrow();
        let m = crate::arch::x86::TargetMachine::new();
        let mut clf = Classifier::new(&m, "win64", &store);
        let a = clf.arg_place(agg).expect("分类");
        let b = clf.arg_place(agg).expect("分类");
        assert_eq!(clf.plans(), 1, "同一类型只应问一次引擎");
        assert_eq!(a, b, "两次查询必须一致");
        // 24B 聚合在 win64 下走 by-ref（指针指向调用方副本）。
        assert!(is_by_ref(&a), "win64 的 24B 聚合实参应走 ByRef：{a:?}");
    }

    /// 返回落点与 `RetLoc` 对齐：win64 的 24B 聚合返回走 sret。
    #[test]
    fn ret_classification_matches_abi_model() {
        let (ctx, agg) = store_with_agg();
        let store = ctx.borrow();
        let m = crate::arch::x86::TargetMachine::new();
        let mut clf = Classifier::new(&m, "win64", &store);
        assert!(
            matches!(clf.ret_place(agg).expect("分类"), RetLoc::Indirect { .. }),
            "win64 的 24B 返回应是 sret（Indirect）"
        );
        assert_eq!(clf.plans(), 1, "返回分类也应去重");
    }
}
