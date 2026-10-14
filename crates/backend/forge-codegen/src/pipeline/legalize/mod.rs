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

use crate::IrError;
use crate::machine::target::TargetMachine;
use crate::pipeline::agg_expand::{AggSlots, ChangeSet, IrSize};
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
    pub agg_slots: AggSlots,
    /// sret 形参（宽聚合返回时由 `AggCall` 立起）。
    pub sret_ptr: Option<Value>,
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
    /// 聚合展开族（Stage 0.5）的**唯一**顺序定义。
    ///
    /// 次序不是随便的：① 聚合 store 先展开（它引入打包常量，且必须在 `CompileState`
    /// 克隆常量池**之前**）；② sret 形参先立起来（后面几个要写进那块缓冲）；
    /// ③ call 结果拆段**先于**实参拆段（否则 `%q = call {i64,i64}` 传给后续 call 时，
    /// 实参已是段值、加载追踪会误判）。
    pub fn with_aggregates() -> Self {
        Self {
            passes: vec![
                // 变参先展开：它往常量池插初值常量、物化 `va_list` 对象，后面几个 pass
                //（以及 `CompileState::new` 的常量池克隆）都必须在它之后。
                Box::new(VaPass),
                Box::new(AggStorePass),
                Box::new(AggCallPass),
                Box::new(AggParamPass),
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
        if crate::pipeline::trace_enabled("FORGE_TRACE_AGG") {
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
            if crate::pipeline::trace_enabled("FORGE_TRACE_AGG") {
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
        let (n_objs, n_args) = crate::pipeline::va_expand::expand_va(func, &va, &slots)?;
        if crate::pipeline::trace_enabled("FORGE_TRACE_LOWER")
            || crate::pipeline::trace_enabled("FORGE_TRACE_AGG")
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
        agg_call::expand_agg_call_args(func, ctx.machine, &ctx.conv)?;
        agg_call::expand_large_agg_ret(func, &mut ctx.agg_slots, ctx.sret_ptr)?;
        Ok(IrSize::of(func).delta_to(before, !had_sret && ctx.sret_ptr.is_some()))
    }
}

/// 形参侧：宽聚合形参接收 + 大聚合值展开。
struct AggParamPass;
impl<M: TargetMachine> LegalizePass<M> for AggParamPass {
    fn name(&self) -> &'static str {
        "agg-param"
    }
    fn run(&self, func: &mut Function, ctx: &mut LegalizeCtx<'_, M>) -> Result<ChangeSet, IrError> {
        let before = IrSize::of(func);
        agg_param::expand_large_agg_params(func, &mut ctx.agg_slots, ctx.machine, &ctx.conv)?;
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

    /// pass 清单就是执行序，且**五个族一个不少**（Va 由调用点按 va 上下文跑，不在这里）。
    #[test]
    fn driver_has_the_documented_pass_order() {
        let d = LegalizeDriver::<crate::arch::x86::TargetMachine>::with_aggregates();
        assert_eq!(
            d.pass_names(),
            vec!["va", "agg-store", "agg-call", "agg-param", "gep"],
            "定序是契约：va → store → call（sret/结果/实参/返回）→ param → gep"
        );
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
