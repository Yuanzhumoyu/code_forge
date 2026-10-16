//! **合法化单一驱动**（v23 M6）。
//!
//! 以前 Stage 0.5 是"在 `compiler.rs` 里按硬编码次序叫 8 个 `expand_*` 函数"：次序、依赖
//! （sret 要先立、call 结果拆段要先于参数拆段）与"改了什么"都散在调用点上，加一个 pass
//! 就得回去读那一段、改那段。这里把三件事收成一处：
//!
//! - **定序**：`LegalizeDriver::with_aggregates` 按依赖序持有 pass；
//! - **变化报告**：每个 pass 返回 [`ChangeSet`]（IR 规模差 + sret 标记），驱动累加；
//!   `FORGE_TRACE_AGG` 打印的就是它；
//! - **幂等**：把驱动跑第二遍，合法化过的 IR 不应再产生改动 ⇒ `ChangeSet::total() == 0`
//!   （守卫见 `tests`；这也是"pass 是否无意中重复改写"的探针）。
//!
//! 8 个 `expand_*` 收成 5 个 pass：`AggStore`、`AggCall`（sret + 结果 + 实参 + 返回）、
//! `AggParam`（形参 + 大聚合值展开）、`Gep`、`Va`（`va_expand::expand_va` 由外部按
//! va 上下文调用——它需要 `VaSlots` 与 va 描述，形状与其余四个不同，留给调用点）。

pub(crate) mod agg_call;
pub(crate) mod agg_param;
pub(crate) mod agg_store;
pub(crate) mod common;
pub(crate) mod gep;
pub(crate) mod materialize;
pub(crate) mod plan;

use crate::IrError;
use crate::machine::target::TargetMachine;
use crate::pipeline::agg_expand::{ChangeSet, IrSize, Places};
use crate::pipeline::legalize::plan::AbiEdges;
use forge_ir::{Function, Value};

/// pass 之间的**共享可变上下文**：机器视图、约定名、聚合槽表、sret 指针。
///
/// 这几个量在 pass 之间有先后依赖（`sret_ptr` 由 `AggCall` 立起来、`agg_slots` 由多个
/// pass 陆续填充），以前是调用点上的局部变量、靠调用次序隐式传递；现在显式放进 ctx。
pub(crate) struct LegalizeCtx<'a, M: TargetMachine> {
    pub machine: &'a M,
    /// 已解析的调用约定名（`conv_registry::resolve` 的产物）。
    pub conv: String,
    /// 聚合槽表（值 → (缓冲, 类型)）。
    pub agg_slots: Places,
    /// sret 形参（宽聚合返回时由 `AggCall` 立起）。
    pub sret_ptr: Option<Value>,
    /// **ABI 边界计划**（v24 M1 Phase A 的产物）：由 `PlanPass` 只读地算出，
    /// 供 `MaterializePass` 消费。`None` = 还没跑计划相。
    pub edges: Option<AbiEdges>,
    /// **重映射后仍代表聚合的值 → 那个聚合的类型**（v24 M1 的事实交接）。
    ///
    /// 为什么需要：by-ref 聚合形参会**重建签名**——旧形参值被全局替换成一个 `ptr`
    /// （`replace_all_uses`），此后"这个 `ptr` 代表 T 型聚合"这条事实**不在 IR 类型里**了
    /// （`value_type` 只剩 `PTR`），而物化相要按 T 算字段偏移。生产者（`agg-param`）知道这件事，
    /// 在这里显式交给消费者——**不是**第二份类型表，而是重映射**销毁**的那一点信息。
    pub shadow_agg: forge_ir::entity::map::SecondaryMap<Value, forge_ir::TypeId>,
    /// **变参展开的输入**：`(va 形状信息, 预先排布的 ABI 槽)`。
    ///
    /// 由编译入口排好（槽深度依赖前端局部槽，只有 lowering 前那次预扫算得出），
    /// 由 `Va` pass 消费——两边是同一份数据，不再各拿一半。
    pub va: Option<(
        forge_isa_runtime::machine::call_layout::VaInfo,
        crate::pipeline::va_expand::AbiSlots,
    )>,
}

impl<'a, M: TargetMachine> LegalizeCtx<'a, M> {
    pub fn new(machine: &'a M, conv: String) -> Self {
        Self {
            machine,
            conv,
            agg_slots: Default::default(),
            sret_ptr: None,
            edges: None,
            shadow_agg: Default::default(),
            va: None,
        }
    }
}

/// 一个合法化 pass：**只看 `func` 与 `ctx`**，返回自己改动了什么。
pub(crate) trait LegalizePass<M: TargetMachine> {
    /// pass 名（诊断/守卫用）。
    fn name(&self) -> &'static str;
    fn run(&self, func: &mut Function, ctx: &mut LegalizeCtx<'_, M>) -> Result<ChangeSet, IrError>;
}

/// 驱动：定序 + 定点 + 变化报告。
pub(crate) struct LegalizeDriver<M: TargetMachine> {
    passes: Vec<Box<dyn LegalizePass<M>>>,
}

impl<M: TargetMachine> LegalizeDriver<M> {
    /// **分批**：聚合族（**不含 Va**）——供"先跑聚合、再排 ABI 槽、最后跑 Va"的入口用。
    ///
    /// 为什么必须能分批（v23 收尾 M7 实测的**帧槽重叠**）：`plan_abi_slots` 要把变参 ABI 槽
    /// （保存区 + `va_list` 对象）排在前端局部槽**之下**，它靠扫一遍 IR 算深度；而聚合族 pass
    /// **会新增 `Alloca`**（聚合槽）。若 ABI 槽在聚合族之前排定，lowering 看到的局部区就比排布时
    /// 假设的更大 ⇒ **新槽可能落进 ABI 槽区**（被游标推进/序言保存覆盖）⇒ 症状是"字段各自读对、
    /// 一起用就错、每次运行还不同"。分批后 ABI 槽看到的是**合法化之后**的 IR，两处深度一致。
    pub fn with_aggregates_except_va() -> Self {
        Self {
            passes: vec![
                // **计划相先行**（v24 M1）：只读地把 ABI 边界计划算出来，后面所有 pass 只消费它。
                Box::new(PlanPass),
                Box::new(AggParamPass),
                Box::new(AggStorePass),
                Box::new(AggCallPass),
                Box::new(AggLeftoverPass),
            ],
        }
    }

    /// **收尾批**：物化相 + GEP。放在**最后**跑的理由：`Va` 也会登记位置（它把聚合实参拷进
    /// 自己的帧槽），所以物化必须等 va 跑完；GEP 与聚合无关，顺手一起收尾。
    pub fn finalize() -> Self {
        Self {
            passes: vec![Box::new(MaterializePass), Box::new(GepPass)],
        }
    }

    /// **分批**：只有 `Va`（要在 ABI 槽排定之后跑）。
    pub fn va_only() -> Self {
        Self {
            passes: vec![Box::new(VaPass)],
        }
    }

    /// 聚合展开族（Stage 0.5）的**唯一**顺序定义。
    ///
    /// **入口现在按分批形态跑**（聚合族 → 排 ABI 槽 → va，见
    /// [`Self::with_aggregates_except_va`] / [`Self::va_only`]，原因是 ABI 槽必须看到聚合族
    /// 新建的 `Alloca`）；这一份"合并次序"是**契约与守卫**用的（`driver_has_the_documented_pass_order`
    /// 断言分批两份拼起来与它一致）。
    #[cfg_attr(not(test), allow(dead_code))]
    /// 次序不是随便的，两条方向相反的约束把它钉死：
    ///
    /// ① 变参先展开（它往常量池插初值常量、物化 `va_list` 对象，后面几个 pass 以及
    ///    `CompileState::new` 的常量池克隆都必须在它之后）；
    /// ② **形参先于实参/存储**：聚合形参的槽由 `agg-param` 建立并登记，"把聚合形参转发给
    ///    另一次调用""把聚合形参存进内存"都要读那个槽。此前次序是 `store → call → param`，
    ///    于是 by-ref 聚合形参转发时实参拆段看不到地址，直接 fail-closed
    ///    （`聚合实参拆段：值来源不支持（def Param(…))`）；
    /// ③ **大聚合 load 值传播（`agg-leftover`）排在实参之后**：它的使用处分派要靠"前面 pass
    ///    已经收拾干净"来判断没剩下东西，早了会对着未物化的聚合实参报错 ⇒ 因此把原来的
    ///    `agg-param` 拆成"形参（早）"与"load 传播（晚）"两个 pass（本批实测撞到才拆的）；
    /// ④ 聚合 store 展开先于 call（它引入打包常量，且必须在常量池克隆之前）；
    /// ⑤ call 结果拆段**先于**实参拆段（否则 `%q = call {i64,i64}` 传给后续 call 时，
    ///    实参已是段值、加载追踪会误判）——这一条在 `agg-call` **pass 内部**，与 pass 间次序无关。
    pub fn with_aggregates() -> Self {
        Self {
            passes: vec![
                // 计划相（只读）：算出 ABI 边界计划。
                Box::new(PlanPass),
                // 变参先展开：它往常量池插初值常量、物化 `va_list` 对象，后面几个 pass
                //（以及 `CompileState::new` 的常量池克隆）都必须在它之后。
                Box::new(VaPass),
                // 形参先于存储/实参：它们的地址来自这里（见上方 ②）。
                Box::new(AggParamPass),
                Box::new(AggStorePass),
                Box::new(AggCallPass),
                // load 值传播（见上方 ③）。
                Box::new(AggLeftoverPass),
                // 最后一步改写：物化相（v24 M1）+ GEP。
                Box::new(MaterializePass),
                Box::new(GepPass),
            ],
        }
    }

    /// pass 名（按执行序）——守卫与诊断读它，免得再抄一份清单。
    pub fn pass_names(&self) -> Vec<&'static str> {
        self.passes.iter().map(|p| p.name()).collect()
    }

    /// 按序跑一遍，返回**累计**改动量。
    pub fn run(
        &self,
        func: &mut Function,
        ctx: &mut LegalizeCtx<'_, M>,
    ) -> Result<ChangeSet, IrError> {
        if crate::pipeline::trace::knob("FORGE_TRACE_AGG") {
            eprintln!(
                "[forge] 合法化驱动：{} 个 pass，次序 {:?}",
                self.passes.len(),
                self.pass_names()
            );
        }
        let mut total = ChangeSet::default();
        for pass in &self.passes {
            let c = pass.run(func, ctx)?;
            total.new_insts += c.new_insts;
            total.new_values += c.new_values;
            total.new_consts += c.new_consts;
            total.sret |= c.sret;
            if crate::pipeline::trace::knob("FORGE_TRACE_AGG") {
                eprintln!(
                    "[forge] 合法化 pass {}：+{} 指令 / +{} 值 / +{} 常量 / sret={}",
                    pass.name(),
                    c.new_insts,
                    c.new_values,
                    c.new_consts,
                    c.sret
                );
            }
        }
        Ok(total)
    }
}

/// 量一次 IR 规模 → 跑 `f` → 报差值（每个 pass 的 `ChangeSet` 都这样来，口径单一）。
fn sized<F>(func: &mut Function, sret_before: Option<Value>, f: F) -> Result<ChangeSet, IrError>
where
    F: FnOnce(&mut Function) -> Result<(), IrError>,
{
    let before = IrSize::of(func);
    f(func)?;
    let sret = sret_before.is_none();
    Ok(IrSize::of(func).delta_to(before, sret))
}

/// **变参展开**（`VaStart`/`VaArg` → 显式 IR）：按 plan 的形状数据跑同一套算法。
///
/// v23 M6：它原来单独站在编译入口里（与聚合展开并列、各自 `if` 一次），现在也是驱动里的
/// 一个 pass——次序、报告口径与其余 pass 一致。输入 `(va, slots)` 由入口排好放进 ctx。
struct VaPass;
impl<M: TargetMachine> LegalizePass<M> for VaPass {
    fn name(&self) -> &'static str {
        "va"
    }
    fn run(&self, func: &mut Function, ctx: &mut LegalizeCtx<'_, M>) -> Result<ChangeSet, IrError> {
        let Some((va, slots)) = ctx.va.take() else {
            return Ok(ChangeSet::default());
        };
        let before = IrSize::of(func);
        let (n_objs, n_args) =
            crate::pipeline::va_expand::expand_va(func, &va, &slots, &mut ctx.agg_slots)?;
        if crate::pipeline::trace::knob("FORGE_TRACE_LOWER")
            || crate::pipeline::trace::knob("FORGE_TRACE_AGG")
        {
            eprintln!(
                "[forge] 变参展开：物化 {n_objs} 份对象、取值 {n_args} 条\
                 （形状 {}、保存区深度 {}、对象深度 {}）",
                va.shape.as_deref().unwrap_or("<explicit>"),
                slots.save_depth,
                slots.obj_depth
            );
        }
        Ok(IrSize::of(func).delta_to(before, false))
    }
}

/// **计划相**（v24 M1）：只读地算出 ABI 边界计划（`ctx.edges`），**不改 IR**。
///
/// 为什么单独一个 pass：v23 的合法化把"决策"和"就地改写"缠在同一遍里，于是 pass 次序
/// 变成了语义（四个缺陷的共同形状）。计划相只读 ⇒ 它排第几都不影响结果；
/// 物化相（后续批次）拿到计划后单遍改写。
///
/// 判据（`plan.rs` 的单测）：IR 规模在计划相前后**逐项相等**；位置表写一次。
struct PlanPass;
impl<M: TargetMachine> LegalizePass<M> for PlanPass {
    fn name(&self) -> &'static str {
        "plan"
    }
    fn run(&self, func: &mut Function, ctx: &mut LegalizeCtx<'_, M>) -> Result<ChangeSet, IrError> {
        let before = IrSize::of(func);
        let va = ctx.va.as_ref().map(|(v, _)| v);
        let edges = plan::plan_abi_edges(func, ctx.machine, &ctx.conv, va)?;
        if crate::pipeline::trace::knob("FORGE_TRACE_AGG") {
            eprintln!(
                "[forge] ABI 计划 {}：{} 条可动边界（{}）",
                func.name,
                edges.actionable(),
                edges.summary()
            );
        }
        ctx.edges = Some(edges);
        // 计划相**声明零改动**：`delta_to` 会算出 0（只读是契约，不是巧合）。
        Ok(IrSize::of(func).delta_to(before, false))
    }
}

/// **物化相**（v24 M1 Phase B）：按指令序单遍把 `Places` 里的内存档值展开成显式 IR。
///
/// 排在所有"会登记位置"的 pass **之后**（形参/存储/调用/load 传播），`gep` 之前。
/// 它自己构造块内指令序并**一次写回** ⇒ 生成指令天生排在消费者之前（不需要事后搬运）。
struct MaterializePass;
impl<M: TargetMachine> LegalizePass<M> for MaterializePass {
    fn name(&self) -> &'static str {
        "materialize"
    }
    fn run(&self, func: &mut Function, ctx: &mut LegalizeCtx<'_, M>) -> Result<ChangeSet, IrError> {
        let cs = materialize::materialize_memory_uses(func, &mut ctx.agg_slots, &ctx.shadow_agg)?;
        Ok(cs)
    }
}

/// 聚合字面量 store → 打包 i64 常量 store。
struct AggStorePass;
impl<M: TargetMachine> LegalizePass<M> for AggStorePass {
    fn name(&self) -> &'static str {
        "agg-store"
    }
    fn run(
        &self,
        func: &mut Function,
        _ctx: &mut LegalizeCtx<'_, M>,
    ) -> Result<ChangeSet, IrError> {
        sized(func, Some(Value::new(0)), |f| {
            agg_store::expand_agg_stores(f)
        })
    }
}

/// 调用相关：sret 形参、call 结果拆段、by-ref 实参、宽聚合返回。
struct AggCallPass;
impl<M: TargetMachine> LegalizePass<M> for AggCallPass {
    fn name(&self) -> &'static str {
        "agg-call"
    }
    fn run(&self, func: &mut Function, ctx: &mut LegalizeCtx<'_, M>) -> Result<ChangeSet, IrError> {
        let before = IrSize::of(func);
        let had_sret = ctx.sret_ptr.is_some();
        let sret = agg_call::expand_sret_return(func, ctx.machine, &ctx.conv)?;
        if sret.is_some() {
            ctx.sret_ptr = sret;
        }
        agg_call::expand_large_agg_call_results(func, &mut ctx.agg_slots, ctx.machine, &ctx.conv)?;
        agg_call::expand_agg_call_args(
            func,
            ctx.machine,
            &ctx.conv,
            &mut ctx.agg_slots,
            &ctx.shadow_agg,
        )?;
        agg_call::expand_large_agg_ret(func, &mut ctx.agg_slots, ctx.sret_ptr)?;
        Ok(IrSize::of(func).delta_to(before, !had_sret && ctx.sret_ptr.is_some()))
    }
}

/// 形参侧：宽聚合形参接收（**只管形参**——槽先立起来，后面 store/call 才有地址可用）。
struct AggParamPass;
impl<M: TargetMachine> LegalizePass<M> for AggParamPass {
    fn name(&self) -> &'static str {
        "agg-param"
    }
    fn run(&self, func: &mut Function, ctx: &mut LegalizeCtx<'_, M>) -> Result<ChangeSet, IrError> {
        let before = IrSize::of(func);
        agg_param::expand_large_agg_params(
            func,
            &mut ctx.agg_slots,
            &mut ctx.shadow_agg,
            ctx.machine,
            &ctx.conv,
        )?;
        Ok(IrSize::of(func).delta_to(before, false))
    }
}

/// 大聚合 **load 值传播**（`%v = load T(>8B)` 按使用处就地展开）。
///
/// 必须排在 `agg-call` **之后**：它的使用处分派里，`Call`/`Return` 这些"已经由前面
/// pass 收拾干净"的使用要能被识别成"没剩下"，否则它会对着尚未物化的聚合实参报
/// `call 聚合实参：值既不在 slot 映射里、也没有段值`（本批把形参提到实参之前时实测撞到，
/// 因此**一个 pass 拆成两个**：形参早、load 传播晚）。
struct AggLeftoverPass;
impl<M: TargetMachine> LegalizePass<M> for AggLeftoverPass {
    fn name(&self) -> &'static str {
        "agg-leftover"
    }
    fn run(&self, func: &mut Function, ctx: &mut LegalizeCtx<'_, M>) -> Result<ChangeSet, IrError> {
        let before = IrSize::of(func);
        agg_param::expand_large_aggs(func, &mut ctx.agg_slots)?;
        Ok(IrSize::of(func).delta_to(before, false))
    }
}

/// GEP 常量折叠/形状展开。
struct GepPass;
impl<M: TargetMachine> LegalizePass<M> for GepPass {
    fn name(&self) -> &'static str {
        "gep"
    }
    fn run(
        &self,
        func: &mut Function,
        _ctx: &mut LegalizeCtx<'_, M>,
    ) -> Result<ChangeSet, IrError> {
        sized(func, Some(Value::new(0)), gep::expand_geps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// pass 清单就是执行序，且**各族一个不少**（Va 由调用点按 va 上下文跑，不在这里）。
    #[test]
    fn driver_has_the_documented_pass_order() {
        let d = LegalizeDriver::<crate::arch::x86::TargetMachine>::with_aggregates();
        assert_eq!(
            d.pass_names(),
            vec![
                "plan",
                "va",
                "agg-param",
                "agg-store",
                "agg-call",
                "agg-leftover",
                "materialize",
                "gep"
            ],
            "定序是契约：plan（只读计划）→ va → param（槽先立起来）→ store → call → \
             leftover（load 值传播）→ materialize（单遍物化）→ gep"
        );
        // 分批形态：聚合族（不含 va）与 va 单独一份——入口按"聚合族 → 排 ABI 槽 → va"用，
        // 两边的名字必须与合并清单一致（不然分批就会漏 pass 或次序漂移）。
        let agg_only =
            LegalizeDriver::<crate::arch::x86::TargetMachine>::with_aggregates_except_va();
        assert_eq!(
            agg_only.pass_names(),
            vec!["plan", "agg-param", "agg-store", "agg-call", "agg-leftover"],
            "聚合族（不含 va）的次序是契约：计划相亲居其首"
        );
        let va_only = LegalizeDriver::<crate::arch::x86::TargetMachine>::va_only();
        assert_eq!(va_only.pass_names(), vec!["va"], "va 单独一批");
        let fin = LegalizeDriver::<crate::arch::x86::TargetMachine>::finalize();
        assert_eq!(
            fin.pass_names(),
            vec!["materialize", "gep"],
            "收尾批：物化相（等 va 也登记完位置）+ GEP"
        );
        // 拼起来（入口的真实分批次序：聚合族 → va → 收尾）必须与合并清单同名同量。
        let mut merged = agg_only.pass_names();
        merged.extend_from_slice(&va_only.pass_names());
        merged.extend_from_slice(&fin.pass_names());
        let mut expect = d.pass_names();
        expect.sort_unstable();
        merged.sort_unstable();
        assert_eq!(merged, expect, "分批两份拼起来 = 合并清单");
    }

    /// **合法化幂等**（M6 验收项）：把驱动在同一个函数上跑**第二遍**，合法化过的 IR 不应再
    /// 产生任何改动 ⇒ `ChangeSet::total() == 0`。
    ///
    /// 为什么这条值钱：每个 pass 都是"就地改写 IR"的，改写若不收敛（例如把已展开的值又展开一次、
    /// 或 store 展开后又被当成未展开），症状是**指令数/bootstrap 膨胀**而不是报错——跑第二遍才知道。
    #[test]
    fn legalization_is_idempotent() {
        use forge_ir::ir::builder::FunctionBuilder;
        use forge_ir::ir::types::{FunctionSignature, TypeContext};
        use forge_ir::{Opcode, TypeId};

        // 16 字节聚合形参（win64: >8B ⇒ by-ref）⇒ `AggParam` 必定动手。
        let ctx = TypeContext::new();
        let agg = ctx
            .borrow_mut()
            .struct_anon(vec![TypeId::I64, TypeId::I64], false);
        let sig = FunctionSignature::new(&[(agg, "a")], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("agg_idem", ctx, sig);
        let (e, params) = b.create_block_with_params(&[(agg, "a")]);
        b.switch_to_block(e);
        let f0 = b.extract_value(params[0], 0);
        let f1 = b.extract_value(params[0], 1);
        let sum = b.iadd(f0, f1);
        b.ret(&[sum]);
        let mut func = b.finish().expect("build agg_idem");

        let machine = crate::arch::x86::TargetMachine::new();
        let mut ctx = LegalizeCtx::new(&machine, "win64".to_string());
        let driver = LegalizeDriver::with_aggregates();

        let first = driver
            .run(&mut func, &mut ctx)
            .expect("第一遍合法化必须成功");
        assert!(
            first.total() > 0,
            "16 字节聚合形参必须让某个 pass 动手（否则这条幂等判据是空转）：{first:?}"
        );

        let before: Vec<Opcode> = func.dfg.insts().map(|(_, d)| d.opcode).collect();
        let second = driver
            .run(&mut func, &mut ctx)
            .expect("第二遍合法化必须成功（幂等意味着它什么都改不了）");
        let after: Vec<Opcode> = func.dfg.insts().map(|(_, d)| d.opcode).collect();
        assert_eq!(second.total(), 0, "第二遍不应再有改动（幂等）：{second:?}");
        assert_eq!(before, after, "第二遍不应改变指令序列");
    }
}
