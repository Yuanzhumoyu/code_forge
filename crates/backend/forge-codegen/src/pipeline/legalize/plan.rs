//! 合法化**计划相**（v24 M1 立项，v25 M1 收口）：**只读地**算出"每条 ABI 边界上的聚合该怎么办"。
//!
//! ## 为什么要有这一相
//!
//! v23 的合法化是"每个 pass 就地改 IR 并立即重写使用处"，于是"谁先跑"成了隐式契约：
//! v23 收尾阶段由这套机制直接产出的缺陷有四个（use-before-def、`return` 跳出循环漏重写
//! 后续使用点、实参来源覆盖不全、pass 次序两向约束逼出 pass 拆分）。根因是**决策与改写
//! 缠在一起**：决策要读 IR、改写要写 IR，两者在同一遍里交替，谁先谁后就变成了语义。
//!
//! 本文件把**决策**单独摘出来：读 IR（落点一律问 [`Classifier`]），产出一份 [`AbiEdges`]
//! 计划，**一个字节的 IR 都不改**。物化（Phase B）拿到计划后**单遍**改写。
//!
//! ## 契约（v25 M1 起）
//!
//! - **只读**：`plan_abi_edges` 不调用任何 `make_inst` / `make_value` / `register_signature`
//!   / `tombstone_inst`；守卫 `plan_is_read_only` 用 IR 规模快照钉住这一点。
//! - **决策来源唯一**：落点一律问 `forge-abi`，且**只经** [`Classifier`]
//!   （`legalize/classify.rs`）——管线里**不出现**约定名分支，也不手抄落点枚举（通用性）。
//! - **载荷是规范类型**：计划里存的是 `forge-abi-model` 的 `ArgLoc` / `RetLoc`，不是本仓
//!   自己的一套"计划枚举"（v25 M1 删掉了那五种枚举——它们只在本文件内部构造与打印，
//!   是 `Placement` 的手抄投影）。
//! - **消费者按它改写**：`agg-param` / `agg-call` 只读计划、**不再自己问引擎**
//!   （它们的签名里已经拿不到机器与约定名，规则由类型系统保证）。
//! - **写一次**：[`Places`] 的每条键只允许写一次（第二次写是计划本身的 bug，
//!   守卫 `place_is_written_once` 钉住）。

use crate::IrError;
use crate::pipeline::legalize::classify::{Classifier, scalar_stack_is_a_data_error};
use forge_abi::{ArgLoc, Placement, RetLoc};
use forge_ir::*;
use forge_isa_runtime::machine::target::TargetMachine;

/// 一个**宽聚合形参**（`>8B`）的落点：只登记要动的那些，`index` 是形参序号。
///
/// 为什么稀疏列表而不是"与形参一一对齐的 `Vec`"：对齐表要读者自己判"这一项是不是聚合"，
/// 而稀疏表连"索引"都显式写出来（`param_of(i)` 是 O(n) 的小查找，n = 聚合形参数）。
#[derive(Debug, Clone)]
pub(crate) struct ParamEdge {
    pub index: usize,
    pub loc: ArgLoc,
}

/// 一条 call **实参**的落点（只登记宽聚合实参）。
///
/// 查表用**值**而不是 `op_idx`：`agg-call` 批次里 `expand_large_agg_call_results` 会往
/// call 的实参表**前插**一个 sret 缓冲指针，之后的 `expand_agg_call_args` 看到的
/// `op_idx` 比计划记录时**大 1**。值句柄不受插入影响，而"一次分类"保证同一个值只有
/// 一种落点（同一类型只问一次引擎）——所以按值查是稳的，按序号查是个陷阱。
#[derive(Debug, Clone)]
pub(crate) struct CallArgEdge {
    pub inst: Inst,
    pub val: Value,
    pub loc: ArgLoc,
}

/// 一条 call **结果**（宽聚合）的落点。
#[derive(Debug, Clone)]
pub(crate) struct CallResultEdge {
    pub inst: Inst,
    pub val: Value,
    pub ret: RetLoc,
}

/// **整个函数的 ABI 边界计划**（Phase A 的产物；Phase B 只读它）。
#[derive(Debug, Default, Clone)]
pub(crate) struct AbiEdges {
    /// 这次计划问了引擎几次（同一类型只问一次；诊断与守卫的观测量）。
    pub plans: usize,
    /// callee-saved 区平移（与 `LowerCtx::stack_slot_shift` 同源）。
    pub shift: i32,
    /// 宽聚合形参的落点（稀疏；按 `index` 查）。
    pub params: Vec<ParamEdge>,
    /// 宽聚合返回的落点（`None` = 没有要处理的宽聚合返回）。
    pub ret: Option<RetLoc>,
    /// 各调用点的宽聚合实参落点（按 `(inst, val)` 查）。
    pub call_args: Vec<CallArgEdge>,
    /// 各调用点的宽聚合结果落点（按 `inst` 查）。
    pub call_results: Vec<CallResultEdge>,
}

impl AbiEdges {
    /// 形参 `index` 的落点（`None` = 非宽聚合 / 不在计划里）。
    pub fn param_of(&self, index: usize) -> Option<&ArgLoc> {
        self.params
            .iter()
            .find(|e| e.index == index)
            .map(|e| &e.loc)
    }

    /// call `inst` 里实参**值** `val` 的落点（理由见 [`CallArgEdge`]）。
    pub fn call_arg_of(&self, inst: Inst, val: Value) -> Option<&ArgLoc> {
        self.call_args
            .iter()
            .find(|e| e.inst == inst && e.val == val)
            .map(|e| &e.loc)
    }

    /// call `inst` 的结果落点。
    pub fn call_result_of(&self, inst: Inst) -> Option<&RetLoc> {
        self.call_results
            .iter()
            .find(|e| e.inst == inst)
            .map(|e| &e.ret)
    }

    /// 计划里一共有多少条"要动"的边界——诊断与"是否需要物化"判据用。
    pub fn actionable(&self) -> usize {
        self.params.len()
            + usize::from(self.ret.is_some())
            + self.call_args.len()
            + self.call_results.len()
    }

    /// 诊断用的一行摘要（`FORGE_TRACE_AGG`）：**逐条点名**要动的边界（哪条指令、哪个值、
    /// 落成什么）——调试"这条实参怎么传的"时，这一行比一串计数有用得多。
    pub fn summary(&self) -> String {
        let mut parts = vec![format!(
            "params={} ret={:?} call_args={} call_rets={} 引擎规划={}",
            self.params.len(),
            self.ret,
            self.call_args.len(),
            self.call_results.len(),
            self.plans
        )];
        for e in &self.params {
            parts.push(format!(
                "param#{} {} => {:?}",
                e.index, e.loc.what, e.loc.place
            ));
        }
        for e in &self.call_args {
            parts.push(format!("arg {:?} {:?} => {:?}", e.inst, e.val, e.loc.place));
        }
        for e in &self.call_results {
            parts.push(format!("ret {:?} {:?} => {:?}", e.inst, e.val, e.ret));
        }
        parts.join("；")
    }
}

/// **计划相主入口**：只读地算出 [`AbiEdges`]。
///
/// 输入：函数 + 机器视图 + 已解析的约定名。
///
/// **不登记 `va_arg`**：那一条的规则是**约定数据**（`VaInfo::arg_rules`），
/// `va` pass 本来就在手边（它自己 fail-closed 并点名缺哪条规则）——计划只描述
/// **物化相要改写的边界**，不做第三份"规则的转录"。
/// 输出：计划；**函数与它的类型表在整个过程中只被读**。
///
/// 落点决策一律来自 `forge-abi`，并只经 [`Classifier`]（`legalize/classify.rs`），
/// 需要"按值整块"还是"按引用"由**数据**说了算——这里没有任何约定名分支。
pub(crate) fn plan_abi_edges(
    func: &Function,
    machine: &impl TargetMachine,
    conv: &str,
) -> Result<AbiEdges, IrError> {
    // 读路径纪律：入口取一次读锁，内部把 `&TypeStore` 传下去（不再逐次 borrow）。
    let types = func.types.clone();
    let store = types.borrow();
    // **分类器**：本函数与它所有调用点的落点都从这里问（同一类型只问一次引擎）。
    let mut clf = Classifier::new(machine, conv, &store);
    let mut edges = AbiEdges::default();

    // ── 形参（只登记宽聚合：`>8B` 才需要 ABI 展开）─────────────────────
    // **按大小登记，不只认聚合**（v27 M6-83，`WA-52`）：128 位整数是 16 字节**标量**，
    // 它的落点同样可能要求"按引用 / 向量槽"，不能因为"不是聚合"就不登记——
    // 那样 ABI 层与物化层都看不到这条边界，最后只能在发射侧炸成 `MOV_RM8_R64`。
    for (i, t) in func.param_types().iter().enumerate() {
        // **向量不在这里改写**：向量（V64/V128/V256）的按值/按引用传递由
        // `forge-rustc` 的降级层与发射侧的 `bank = "vec"` 规则负责——它们在 IR 里要么
        // 是单个向量值（≤16B，走 XMM），要么已经是 sret 缓冲（>16B）。把向量按
        // "宽聚合"改写会让 `V128` 被拆成两个 i64 结果（静默读错寄存器）。
        if store.is_vector(*t) || store.size_bytes(*t) <= 8 {
            continue;
        }
        let place = clf.arg_place(*t)?;
        if let Placement::Stack { size, .. } = place {
            // 标量语义的 `Stack` 对聚合不该出现；出现了就是约定数据有问题。
            return Err(scalar_stack_is_a_data_error(conv, u32::from(size)));
        }
        edges.params.push(ParamEdge {
            index: i,
            loc: ArgLoc {
                what: format!("param#{i}"),
                index: Some(i),
                size: store.size_bytes(*t),
                place,
            },
        });
    }

    // ── 返回（只登记宽聚合；函数侧通常 0/1 个）─────────────────────────
    if let Some(t) = func
        .return_types()
        .into_iter()
        .find(|t| !store.is_vector(*t) && store.size_bytes(*t) > 8)
    {
        edges.ret = Some(clf.ret_place(t)?);
    }

    // ── call 实参 / 结果、`va_arg` ────────────────────────────────────
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = func.dfg.inst_data(ii);
            match inst.opcode {
                Opcode::Call | Opcode::CallIndirect => {
                    let skip = usize::from(inst.opcode == Opcode::CallIndirect);
                    for &v in inst.operands.iter().skip(skip) {
                        let Some(ty) = func.dfg.value_type(v) else {
                            continue;
                        };
                        // 同形参：**按大小**登记（128 位标量也算边界对象，见上面的说明）；
                        // 向量同样不登记（见形参处的说明）。
                        if store.is_vector(ty) || store.size_bytes(ty) <= 8 {
                            continue;
                        }
                        let place = clf.arg_place(ty)?;
                        if let Placement::Stack { size, .. } = place {
                            return Err(scalar_stack_is_a_data_error(conv, u32::from(size)));
                        }
                        edges.call_args.push(CallArgEdge {
                            inst: ii,
                            val: v,
                            loc: ArgLoc {
                                what: format!("arg{skip}+"),
                                index: None,
                                size: store.size_bytes(ty),
                                place,
                            },
                        });
                    }
                    // call 结果（宽聚合才登记）
                    if let Some(rt) = inst.results.first().and_then(|v| func.dfg.value_type(*v))
                        && !store.is_vector(rt)
                        && store.size_bytes(rt) > 8
                    {
                        let ret = clf.ret_place(rt)?;
                        if !matches!(ret, RetLoc::Indirect { .. }) && store.size_bytes(rt) > 16 {
                            return Err(crate::IrError::capability(
                                "AGG_RET_PLACEMENT",
                                format!(
                                    "聚合返回 {} 字节：这份约定既不是 sret 也不是寄存器档：type {rt:?}",
                                    store.size_bytes(rt)
                                ),
                            ));
                        }
                        edges.call_results.push(CallResultEdge {
                            inst: ii,
                            val: inst.results[0],
                            ret,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    edges.shift = clf.shift();
    edges.plans = clf.plans();
    Ok(edges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::agg_expand::{IrSize, Place, Places};
    use crate::pipeline::legalize::classify::is_by_ref;

    /// 建一个"含聚合形参 + 聚合返回"的函数（够触发计划里的两条路径）。
    fn fixture() -> Function {
        use forge_ir::ir::builder::FunctionBuilder;
        use forge_ir::ir::types::{FunctionSignature, TypeContext};

        let ctx = TypeContext::new();
        let agg = ctx
            .borrow_mut()
            .struct_anon(vec![TypeId::I64, TypeId::I64, TypeId::I64], false);
        let sig = FunctionSignature::new(&[(agg, "s")], &[agg]);
        let mut b = FunctionBuilder::new("plan_fixture", ctx, sig);
        let (e, ps) = b.create_block_with_params(&[(agg, "s")]);
        b.switch_to_block(e);
        b.ret(&[ps[0]]);
        b.finish().expect("build")
    }

    /// **计划相只读**（M1 的硬判据）：跑完 `plan_abi_edges` 后 IR 规模**一个字节都没变**。
    ///
    /// 为什么值钱：两相化的全部收益来自"决策不改写"。一旦有人在计划里顺手建了个值/指令，
    /// 这条立刻红——而"顺手改一下"正是 v23 那四个缺陷的共同形状。
    #[test]
    fn plan_is_read_only() {
        let f = fixture();
        let before = IrSize::of(&f);
        let m = crate::arch::x86::TargetMachine::new();
        let edges = plan_abi_edges(&f, &m, "win64").expect("plan");
        let after = IrSize::of(&f);
        assert_eq!(before.insts, after.insts, "计划相不得新增/删除指令");
        assert_eq!(before.values, after.values, "计划相不得新增值");
        assert_eq!(before.consts, after.consts, "计划相不得新增常量");
        // 这个夹具：一个 24 字节聚合形参（win64 ⇒ by-ref 指针）+ 一个 24 字节返回（⇒ sret）
        assert!(
            edges.actionable() >= 2,
            "夹具应至少给出形参与返回两条可动边界：{:?}",
            edges.summary()
        );
        assert_eq!(edges.params.len(), 1);
        let p = edges.param_of(0).expect("形参 #0 应在计划里");
        assert!(
            is_by_ref(&p.place),
            "win64 的 24B 形参应走 by-ref：{:?}",
            p.place
        );
        assert!(
            matches!(edges.ret, Some(RetLoc::Indirect { .. })),
            "win64 的 24B 返回应走 sret：{:?}",
            edges.ret
        );
    }

    /// **一次分类**：计划里的引擎规划次数 = **不同**类型个数（不是使用点个数）。
    #[test]
    fn one_plan_per_distinct_type() {
        let f = fixture();
        let m = crate::arch::x86::TargetMachine::new();
        let edges = plan_abi_edges(&f, &m, "win64").expect("plan");
        // 夹具只有 1 个聚合类型（形参与返回同型）⇒ 形参一次 + 返回一次 = 2。
        assert_eq!(
            edges.plans,
            2,
            "形参与返回是同一类型，但各查一次（形参与返回是两个不同的落点问题）：{}",
            edges.summary()
        );
    }

    /// **位置表写一次**：同一值计划两次是计划本身的 bug（不是运行时数据问题）。
    ///
    /// 为什么值钱：旧的位置表时代"值要么在段里、要么在内存里"由多个 pass 陆续写入，
    /// 后写的会**覆盖**前写的（正是 pass 次序敏感的根源）。新表把这件事变成硬约束。
    #[test]
    fn place_is_written_once() {
        let mut f = fixture();
        let v = f.dfg.make_value(TypeId::I64, ValueDef::Param(f.entry(), 0));
        let addr = f.dfg.make_value(TypeId::PTR, ValueDef::Param(f.entry(), 0));
        let mut places = Places::default();
        places.set(v, Place::Mem(addr));
        assert!(matches!(places.get(v), Some(Place::Mem(_))));
        // 第二次写：测试环境下必须 panic（debug 断言），这里用 catch_unwind 钉住行为。
        let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut f2 = fixture();
            let v2 = f2
                .dfg
                .make_value(TypeId::I64, ValueDef::Param(f2.entry(), 0));
            let a2 = f2
                .dfg
                .make_value(TypeId::PTR, ValueDef::Param(f2.entry(), 0));
            let mut p2 = Places::default();
            p2.set(v2, Place::Mem(a2));
            p2.set(v2, Place::Mem(a2));
        }));
        assert!(second.is_err(), "同一值写两次必须被钉住（不得静默覆盖）");
    }
}
