//! 合法化**计划相**（v24 M1）：**只读地**算出"每条 ABI 边界上的聚合该怎么办"。
//!
//! ## 为什么要有这一相
//!
//! v23 的合法化是"每个 pass 就地改 IR 并立即重写使用处"，于是"谁先跑"成了隐式契约：
//! v23 收尾阶段由这套机制直接产出的缺陷有四个（use-before-def、`return` 跳出循环漏重写
//! 后续使用点、实参来源覆盖不全、pass 次序两向约束逼出 pass 拆分）。根因是**决策与改写
//! 缠在一起**：决策要读 IR、改写要写 IR，两者在同一遍里交替，谁先谁后就变成了语义。
//!
//! 本文件把**决策**单独摘出来：读 IR（含向 `forge-abi` 的落点探针问路），产出一份
//! [`AbiEdges`] 计划，**一个字节的 IR 都不改**。物化（Phase B）拿到计划后**单遍**改写。
//!
//! ## 契约
//!
//! - **只读**：`plan_abi_edges` 不调用任何 `make_inst` / `make_value` / `register_signature`
//!   / `tombstone_inst`；守卫 `plan_is_read_only` 用 IR 规模快照钉住这一点。
//! - **决策来源唯一**：落点一律问 `forge-abi`（[`agg_arg_place`] / [`agg_ret_is_sret`] /
//!   变参形状的 `arg_rules`）——管线里**不出现**约定名分支（通用性）。
//! - **写一次**：[`Places`] 的每条键只允许写一次（第二次写是计划本身的 bug，
//!   守卫 `place_is_written_once` 钉住）。

use crate::IrError;
use crate::pipeline::legalize::common::{AggArgLoc, agg_arg_place, agg_ret_is_sret};
use forge_ir::*;
use forge_isa_runtime::machine::call_layout::VaInfo;
use forge_isa_runtime::machine::target::TargetMachine;

/// 形参的落点计划（决定签名怎么重建、值住哪）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamPlan {
    /// 原样保留（非聚合；或标量语义、无需 ABI 展开）。
    Keep,
    /// **拆成 `n` 个 `i64` 形参**（寄存器档：`<<= 约定阈值` 的聚合）。
    AsSegs(u8),
    /// **改成单个 `ptr` 形参**（by-ref：指针指向调用方的副本，win64 `>8B` 就是这条）。
    AsPtr,
    /// 签名不动，值**住在 `[fp + offset]`**（按值整块进栈区，sysv64 `>16B` 那条）。
    AsStackMem { offset: i32, size: u32 },
}

/// 返回的落点计划。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RetPlan {
    /// 原样返回（标量 / 小聚合寄存器档）。
    #[default]
    Keep,
    /// **sret**：调用方给缓冲指针，被调方按段写进去。
    Sret,
    /// 拆成 `n` 个返回值。
    AsSegs(u8),
}

/// 一条 call **实参**的落点计划。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArgPlan {
    /// 非聚合 / 无需展开。
    Keep,
    /// 拆成 `n` 个 `i64` 实参（寄存器档）。
    Segs(u8),
    /// 调用方自建副本、传**指针**（by-ref）。
    ByRef { size: u32 },
    /// 调用方自建副本、整块写进传出参数区（sysv64 `>16B`）。
    StackByVal { offset: i32, size: u32 },
}

/// 一条 call **结果**的落点计划。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallResultPlan {
    /// 非聚合 / 无需展开。
    Keep,
    /// 拆成 `n` 个 `i64` 结果（寄存器档）。
    Segs(u8),
    /// 结果写进调用方缓冲（sret），结果值本身作废。
    SretMem,
}

/// 一条 `va_arg`（聚合结果）的取参计划。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VaArgPlan {
    /// 非聚合 / 结果不是聚合。
    Keep,
    /// 变参区那个槽里放的是**指向聚合的指针**（win64 `>8B`）。
    Pointer { size: u32 },
    /// 变参区里**直接是聚合字节**（sysv64 `>16B` 的 memory class、或 ≤阈值的内联档）。
    Inline { size: u32 },
}

/// 一条 call 实参的完整记录（物化时按 `inst` 定位、按 `op_idx` 替换）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct CallArgEdge {
    pub inst: Inst,
    pub op_idx: usize,
    pub val: Value,
    pub plan: ArgPlan,
}

/// 一条 call 结果的完整记录。
#[derive(Debug, Clone, Copy)]
pub(crate) struct CallResultEdge {
    pub inst: Inst,
    pub val: Value,
    pub plan: CallResultPlan,
}

/// 一条 `va_arg` 的完整记录。
#[derive(Debug, Clone, Copy)]
pub(crate) struct VaArgEdge {
    pub inst: Inst,
    pub val: Value,
    pub plan: VaArgPlan,
}

/// **整个函数的 ABI 边界计划**（Phase A 的产物；Phase B 只读它）。
#[derive(Debug, Default, Clone)]
pub(crate) struct AbiEdges {
    pub params: Vec<ParamPlan>,
    pub ret: RetPlan,
    pub call_args: Vec<CallArgEdge>,
    pub call_results: Vec<CallResultEdge>,
    pub va_args: Vec<VaArgEdge>,
}

impl AbiEdges {
    /// 计划里一共有多少个"要动"的边界（`Keep` 不算）——诊断与"是否需要物化"判据用。
    pub fn actionable(&self) -> usize {
        let p = self
            .params
            .iter()
            .filter(|x| !matches!(x, ParamPlan::Keep))
            .count();
        let r = usize::from(!matches!(self.ret, RetPlan::Keep));
        let a = self
            .call_args
            .iter()
            .filter(|x| !matches!(x.plan, ArgPlan::Keep))
            .count();
        let c = self
            .call_results
            .iter()
            .filter(|x| !matches!(x.plan, CallResultPlan::Keep))
            .count();
        let v = self
            .va_args
            .iter()
            .filter(|x| !matches!(x.plan, VaArgPlan::Keep))
            .count();
        p + r + a + c + v
    }

    /// 诊断用的一行摘要（`FORGE_TRACE_AGG`）：**逐条点名**要动的边界（哪条指令、哪个值、
    /// 落成什么）——调试"这条实参怎么传的"时，这一行比一串计数有用得多。计数字段仍然保留，
    /// 因为它给出"这个函数到底有多少条边界"的规模感。
    pub fn summary(&self) -> String {
        let mut parts = vec![format!(
            "params={:?} ret={:?} call_args={} call_rets={} va_args={}",
            self.params,
            self.ret,
            self.call_args.len(),
            self.call_results.len(),
            self.va_args.len()
        )];
        for e in self
            .call_args
            .iter()
            .filter(|e| !matches!(e.plan, ArgPlan::Keep))
        {
            parts.push(format!(
                "arg {}#{} {:?} => {:?}",
                e.inst, e.op_idx, e.val, e.plan
            ));
        }
        for e in self
            .call_results
            .iter()
            .filter(|e| !matches!(e.plan, CallResultPlan::Keep))
        {
            parts.push(format!("ret {:?} {:?} => {:?}", e.inst, e.val, e.plan));
        }
        for e in self
            .va_args
            .iter()
            .filter(|e| !matches!(e.plan, VaArgPlan::Keep))
        {
            parts.push(format!("va {:?} {:?} => {:?}", e.inst, e.val, e.plan));
        }
        parts.join("；")
    }
}

/// **计划相主入口**：只读地算出 [`AbiEdges`]。
///
/// 输入：函数 + 机器视图 + 已解析的约定名 + （可选）变参形状信息。
/// 输出：计划；**函数与它的类型表在整个过程中只被读**。
///
/// 落点决策一律来自 `forge-abi` 的探针（[`agg_arg_place`] / [`agg_ret_is_sret`]），
/// 需要"按值整块"还是"按引用"由**数据**说了算——这里没有任何约定名分支。
pub(crate) fn plan_abi_edges(
    func: &Function,
    machine: &impl TargetMachine,
    conv: &str,
    va: Option<&VaInfo>,
) -> Result<AbiEdges, IrError> {
    // 读路径纪律：入口取一次读锁，内部把 `&TypeStore` 传下去（不再逐次 borrow）。
    let types = func.types.clone();
    let store = types.borrow();
    let mut edges = AbiEdges::default();

    // ── 形参 ──────────────────────────────────────────────────────────
    for t in func.param_types() {
        if !store.is_aggregate(t) {
            edges.params.push(ParamPlan::Keep);
            continue;
        }
        let size = store.size_bytes(t);
        if size <= 8 {
            // 小聚合：约定可能仍走寄存器档（C ABI 强制），保持原样交给物化按段处理。
            edges.params.push(ParamPlan::Keep);
            continue;
        }
        let (loc, _shift) = agg_arg_place(machine, conv, &store, t)?;
        edges.params.push(match loc {
            AggArgLoc::ByRef => ParamPlan::AsPtr,
            AggArgLoc::StackByVal { offset, size } => ParamPlan::AsStackMem {
                offset,
                size: u32::from(size),
            },
            AggArgLoc::Registers => ParamPlan::AsSegs(size.div_ceil(8) as u8),
        });
    }

    // ── 返回 ─────────────────────────────────────────────────────────
    for t in func.return_types() {
        if !store.is_aggregate(t) {
            continue;
        }
        let size = store.size_bytes(t);
        if size <= 8 {
            continue;
        }
        edges.ret = if agg_ret_is_sret(machine, conv, &store, t)? {
            RetPlan::Sret
        } else {
            RetPlan::AsSegs(size.div_ceil(8) as u8)
        };
    }

    // ── call 实参 / 结果、`va_arg` ────────────────────────────────────
    for (_, bd) in func.dfg.blocks() {
        for &ii in &bd.inst_order {
            let inst = func.dfg.inst_data(ii);
            match inst.opcode {
                Opcode::Call | Opcode::CallIndirect => {
                    let skip = usize::from(inst.opcode == Opcode::CallIndirect);
                    for (op_idx, &v) in inst.operands.iter().enumerate().skip(skip) {
                        let Some(ty) = func.dfg.value_type(v) else {
                            continue;
                        };
                        // **完整描述**：不需要动的实参也进计划（记 `Keep`），这样"计划 = 这个
                        // 调用点全部 ABI 边界"是可枚举的，而不是"只有异常的那几条"。
                        let plan = if !store.is_aggregate(ty) || store.size_bytes(ty) <= 8 {
                            ArgPlan::Keep
                        } else {
                            let size = store.size_bytes(ty);
                            let (loc, _shift) = agg_arg_place(machine, conv, &store, ty)?;
                            match loc {
                                AggArgLoc::ByRef => ArgPlan::ByRef { size },
                                AggArgLoc::StackByVal { offset, size } => ArgPlan::StackByVal {
                                    offset,
                                    size: u32::from(size),
                                },
                                AggArgLoc::Registers => ArgPlan::Segs(size.div_ceil(8) as u8),
                            }
                        };
                        edges.call_args.push(CallArgEdge {
                            inst: ii,
                            op_idx,
                            val: v,
                            plan,
                        });
                    }
                    // call 结果（完整描述：不需要动的也记 `Keep`）
                    if let Some(rt) = inst.results.first().and_then(|v| func.dfg.value_type(*v)) {
                        let plan = if !store.is_aggregate(rt) || store.size_bytes(rt) <= 8 {
                            CallResultPlan::Keep
                        } else {
                            let size = store.size_bytes(rt);
                            if agg_ret_is_sret(machine, conv, &store, rt)? {
                                CallResultPlan::SretMem
                            } else {
                                CallResultPlan::Segs(size.div_ceil(8) as u8)
                            }
                        };
                        edges.call_results.push(CallResultEdge {
                            inst: ii,
                            val: inst.results[0],
                            plan,
                        });
                    }
                }
                Opcode::VaArg => {
                    let Some(res) = inst.results.first().copied() else {
                        continue;
                    };
                    let Some(ty) = func.dfg.value_type(res) else {
                        continue;
                    };
                    if !store.is_aggregate(ty) {
                        // 完整描述：标量取参也进计划。
                        edges.va_args.push(VaArgEdge {
                            inst: ii,
                            val: res,
                            plan: VaArgPlan::Keep,
                        });
                        continue;
                    }
                    let size = store.size_bytes(ty);
                    let rule = va.and_then(|v| v.arg_rules.aggregate);
                    let plan = match rule {
                        // 约定声明了聚合取参规则：按"区域里是指针还是内联字节"分档
                        // （阈值语义：`indirect` 只对**大于**阈值的尺寸成立）。
                        Some(a)
                            if a.indirect
                                && (a.indirect_size_gt == 0 || size > a.indirect_size_gt) =>
                        {
                            VaArgPlan::Pointer { size }
                        }
                        Some(_) => VaArgPlan::Inline { size },
                        // 没声明 ⇒ 保持 `Keep`，物化相会 fail-closed（点名缺哪条规则）。
                        None => VaArgPlan::Keep,
                    };
                    edges.va_args.push(VaArgEdge {
                        inst: ii,
                        val: res,
                        plan,
                    });
                }
                _ => {}
            }
        }
    }

    Ok(edges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::agg_expand::{IrSize, Place, Places};

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
        let edges = plan_abi_edges(&f, &m, "win64", None).expect("plan");
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
        assert_eq!(
            edges.params[0],
            ParamPlan::AsPtr,
            "win64 的 24B 形参应走 by-ref"
        );
        assert_eq!(edges.ret, RetPlan::Sret, "win64 的 24B 返回应走 sret");
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
