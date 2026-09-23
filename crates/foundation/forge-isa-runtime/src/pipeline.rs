//! 宿主编译管线的**注册表**（v19 V1b）。
//!
//! 生成物（`isa_from_file!`）只依赖本 crate，而"IR 函数 → 机器码"的编译管线是**宿主**
//! 的东西（JIT/寄存器分配/发射）。两者靠本表解耦：宿主在自己那侧为每个 ISA 注册一个
//! 工厂闭包，生成物里 `impl_erased_target_machine!` 的 `compile` 按 ISA 名查表。
//!
//! - 未注册 ⇒ 返回 [`forge_ir::IrError::Unsupported`]（fail-closed，不 panic）；
//! - 工厂签名是「任意机器引用 → 可选管线对象」，因此运行面**不需要知道**具体机器类型
//!   （宿主在闭包里 downcast）。
//! - 注册是进程级一次性的（`OnceLock` + 小表）；重复注册同名 ISA 会覆盖并告警。

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::CompiledFunction;
use forge_ir::IrError;

/// 一条"IR 函数 → 编译产物"的管线（宿主实现）。
pub trait FunctionPipeline: Send + Sync {
    /// 编译一个 IR 函数；语义与旧的 `FunctionCompiler::compile_raw` 相同。
    fn compile_raw(&self, func: &forge_ir::Function) -> Result<CompiledFunction, IrError>;
}

/// 工厂：把类型擦除的机器引用变成管线对象；`None` = 这个工厂不认这台机器。
pub type PipelineFactory =
    dyn Fn(&dyn Any) -> Option<Box<dyn FunctionPipeline>> + Send + Sync + 'static;

static FACTORIES: OnceLock<Mutex<BTreeMap<String, &'static PipelineFactory>>> = OnceLock::new();

fn factories() -> &'static Mutex<BTreeMap<String, &'static PipelineFactory>> {
    FACTORIES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// 为某个 ISA 名（`IsaInfo::name()`）注册管线工厂。宿主在启动/首次使用前调用一次。
pub fn register_pipeline(isa: &str, factory: &'static PipelineFactory) {
    let mut map = factories().lock().expect("pipeline registry poisoned");
    if map.insert(isa.to_string(), factory).is_some() {
        // 覆盖是允许的（测试会反复注册），但要说清楚。
        eprintln!("forge-isa-runtime: ISA `{isa}` 的管线工厂被重复注册（后者生效）");
    }
}

/// 查询某 ISA 是否已注册管线。
pub fn has_pipeline(isa: &str) -> bool {
    factories()
        .lock()
        .map(|m| m.contains_key(isa))
        .unwrap_or(false)
}

/// 按 ISA 名编译一个 IR 函数（`ErasedTargetMachine::compile` 的实现体）。
///
/// 未注册该 ISA 的管线 ⇒ [`IrError::Unsupported`]，错误消息点名 ISA 与"需要宿主注册"。
pub fn compile_via_pipeline(
    isa: &str,
    tm: &dyn Any,
    func: &forge_ir::Function,
) -> Result<CompiledFunction, IrError> {
    let factory = factories().lock().ok().and_then(|m| m.get(isa).copied());
    let Some(factory) = factory else {
        return Err(IrError::Unsupported(format!(
            "ISA `{isa}` 未注册编译管线：宿主需调用 forge_isa_runtime::register_pipeline(\"{isa}\", <工厂>)"
        )));
    };
    let Some(pipeline) = factory(tm) else {
        return Err(IrError::Unsupported(format!(
            "ISA `{isa}` 的管线工厂不认这台机器类型（注册的工厂与 TargetMachine 不匹配）"
        )));
    };
    pipeline.compile_raw(func)
}
