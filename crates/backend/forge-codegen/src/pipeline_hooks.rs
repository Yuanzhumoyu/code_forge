//! 把发行后端的 `TargetMachine` 接到编译管线（v19 V1b 的 runtime 注册表）。
//!
//! 生成物只依赖 `forge_isa_runtime`，因此"谁来做 IR → 机器码"必须由**宿主**登记：
//! 本模块为三个发行后端各注册一个工厂闭包（downcast 到具体类型后造 `FunctionCompiler`）。
//! 入口是 [`ensure_registered`]，在 JIT 编译入口处调用（幂等）。
//!
//! v20 A5-3 起这里还登记**调用点布局钩子**（`pipeline::abi_target::register_isa_call_planner`）：
//! 生成的 lowering 在调用点只有实参形状，靠它拿到**被调方**的落点。

use std::any::Any;
use std::sync::OnceLock;

use forge_ir::IrError;
use forge_isa_runtime::{CompiledFunction, FunctionPipeline};

use crate::pipeline::compiler::FunctionCompiler;

struct Hook<M: forge_isa_runtime::machine::target::TargetMachine + Clone>(FunctionCompiler<M>);

impl<M: forge_isa_runtime::machine::target::TargetMachine + Clone> FunctionPipeline for Hook<M> {
    fn compile_raw(&self, func: &forge_ir::Function) -> Result<CompiledFunction, IrError> {
        self.0.compile_raw(func)
    }
}

macro_rules! register_backend {
    ($factory:ident, $tm:ty, $isa:expr) => {
        fn $factory(tm: &dyn Any) -> Option<Box<dyn FunctionPipeline>> {
            let tm = tm.downcast_ref::<$tm>()?.clone();
            Some(Box::new(Hook(FunctionCompiler::new(tm))))
        }
        // 工厂必须以 `'static` 引用登记：用一个常量提升生命周期。
        const _: () = {
            fn factory_ref() -> &'static forge_isa_runtime::PipelineFactory {
                &|tm: &dyn Any| $factory(tm)
            }
            // 占位（真正登记在 ensure_registered 里用 leak）
            let _ = factory_ref;
        };
    };
}

register_backend!(
    x86_factory,
    crate::arch::x86_v12::TargetMachine,
    "x86_64_v12"
);
register_backend!(
    riscv_factory,
    crate::arch::riscv64_v12::TargetMachine,
    "riscv64_v12"
);
register_backend!(
    arm64_factory,
    crate::arch::arm64_v12::TargetMachine,
    "arm64_v12"
);

/// 进程内注册一次（幂等）：把三个发行后端的管线工厂登记到 runtime 注册表。
///
/// 注册用 `Box::leak` 得到 `&'static`（进程生命周期，量级 = 3 个闭包 + 3 台机器）。
///
/// 同时登记**调用点布局钩子**（v20 A5-3）：生成的 lowering 在调用点只有实参形状，
/// 靠这个钩子拿"被调方"的落点（`machine::call_plan::plan_call`）。未注册 ⇒ 调用点
/// **fail-closed**（不按谱面顺序猜落点）。
pub fn ensure_registered() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let x86: &'static forge_isa_runtime::PipelineFactory =
            Box::leak(Box::new(|tm: &dyn Any| x86_factory(tm)));
        let riscv: &'static forge_isa_runtime::PipelineFactory =
            Box::leak(Box::new(|tm: &dyn Any| riscv_factory(tm)));
        let arm64: &'static forge_isa_runtime::PipelineFactory =
            Box::leak(Box::new(|tm: &dyn Any| arm64_factory(tm)));
        forge_isa_runtime::register_pipeline("x86_64_v12", x86);
        forge_isa_runtime::register_pipeline("riscv64_v12", riscv);
        forge_isa_runtime::register_pipeline("arm64_v12", arm64);

        // 调用点布局钩子：`(约定名, 实参形状, 返回形状) → 被调方 CallLayout`。
        crate::pipeline::abi_target::register_isa_call_planner(
            "x86_64_v12",
            crate::arch::x86_v12::TargetMachine::new(),
        );
        crate::pipeline::abi_target::register_isa_call_planner(
            "riscv64_v12",
            crate::arch::riscv64_v12::TargetMachine::new(),
        );
        crate::pipeline::abi_target::register_isa_call_planner(
            "arm64_v12",
            crate::arch::arm64_v12::TargetMachine::new(),
        );
    });
}
