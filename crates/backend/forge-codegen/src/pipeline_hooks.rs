//! 把发行后端的 `TargetMachine` 接到编译管线（v19 V1b 的 runtime 注册表）。
//!
//! 生成物只依赖 `forge_isa_runtime`，因此"谁来做 IR → 机器码"必须由**宿主**登记：
//! 本模块为三个发行后端各注册一个工厂闭包（downcast 到具体类型后造 `CompileSession`）。
//! 入口是 [`ensure_registered`]，在 JIT 编译入口处调用（幂等）。
//!
//! v20 A5-3 起这里还登记**调用点布局钩子**（`pipeline::abi_target::register_isa_call_planner`）：
//! 生成的 lowering 在调用点只有实参形状，靠它拿到**被调方**的落点。

use std::any::Any;
use std::sync::OnceLock;

use forge_ir::IrError;
use forge_isa_runtime::{CompiledFunction, FunctionPipeline};

use crate::pipeline::compiler::CompileSession;

struct Hook<M: forge_isa_runtime::machine::target::TargetMachine + Clone>(CompileSession<M>);

impl<M: forge_isa_runtime::machine::target::TargetMachine + Clone> FunctionPipeline for Hook<M> {
    fn compile(&self, func: &forge_ir::Function) -> Result<CompiledFunction, IrError> {
        self.0.compile(func)
    }
}

macro_rules! register_backend {
    ($factory:ident, $tm:ty, $isa:expr) => {
        fn $factory(tm: &dyn Any) -> Option<Box<dyn FunctionPipeline>> {
            let tm = tm.downcast_ref::<$tm>()?.clone();
            Some(Box::new(Hook(CompileSession::new(tm))))
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

// v28 M2：工厂/管线/调用点钩子全部由 build.rs 从 isa/manifest.toml 生成。
include!(concat!(env!("OUT_DIR"), "/backend_hooks.rs"));

/// 进程内注册一次（幂等）：把 manifest 里每个发行后端的管线工厂登记到 runtime 注册表。
///
/// 注册用 `Box::leak` 得到 `&'static`（进程生命周期，量级 = 闭包 + 机器）。
///
/// 同时登记**调用点布局钩子**（v20 A5-3）：生成的 lowering 在调用点只有实参形状，
/// 靠这个钩子拿"被调方"的落点（`machine::call_plan::plan_call`）。未注册 ⇒ 调用点
/// **fail-closed**（不按谱面顺序猜落点）。
pub fn ensure_registered() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(register_generated_backends);
}

/// 为**外部宿主/动态插件**注册一个已生成的后端（v28 M3）。
///
/// `machine` 是 `TargetMachine::new()`；本函数登记管线工厂 + 调用点布局钩子。
/// 插件宏 `export_isa_plugin!` 与外部宿主都复用这一处，避免手写工厂闭包。
pub fn register_generated_isa<M>(name: &'static str, machine: M)
where
    M: forge_isa_runtime::machine::target::TargetMachine + Clone + 'static,
{
    fn factory<M>(tm: &dyn Any) -> Option<Box<dyn FunctionPipeline>>
    where
        M: forge_isa_runtime::machine::target::TargetMachine + Clone + 'static,
    {
        let tm = tm.downcast_ref::<M>()?.clone();
        Some(Box::new(Hook(CompileSession::new(tm))))
    }
    let f: &'static forge_isa_runtime::PipelineFactory =
        Box::leak(Box::new(|tm: &dyn Any| factory::<M>(tm)));
    forge_isa_runtime::register_pipeline(name, f);
    crate::pipeline::abi_target::register_isa_call_planner(name, machine);
}

/// 插件/外部宿主的**一行导出**（v28 M3）。
///
/// 用法（插件 crate）：
///
/// ```ignore
/// forge_dsl::isa_from_file!("isa/my.toml", name = "myisa");
/// forge_codegen::export_isa_plugin!(myisa, targets = ["myisa64"]);
/// ```
///
/// 展开出 C ABI 的 `__forge_isa_name` / `__forge_isa_targets` / `__forge_isa_compile`；
/// `PluginLoader` 读取前两者、把 `__forge_isa_compile` 包成宿主侧的
/// `ErasedTargetMachine` 注册进 Registry——**宿主零 ISA 代码**。
#[macro_export]
macro_rules! export_isa_plugin {
    ($module:ident, targets = [$($t:expr),* $(,)?]) => {
        /// ISA 名（NUL 结尾）。
        #[unsafe(no_mangle)]
        pub extern "C" fn __forge_isa_name() -> *const core::ffi::c_char {
            concat!(stringify!($module), "\0").as_ptr() as *const core::ffi::c_char
        }

        /// 目标前缀表（分号分隔、NUL 结尾）。
        #[unsafe(no_mangle)]
        pub extern "C" fn __forge_isa_targets() -> *const core::ffi::c_char {
            concat!($($t, ";"),*, "\0").as_ptr() as *const core::ffi::c_char
        }

        /// 插件侧编译入口；宿主只持有函数指针，不依赖插件内部类型。
        #[allow(improper_ctypes_definitions)]
        #[unsafe(no_mangle)]
        pub extern "C" fn __forge_isa_compile(
            func: &$crate::ir::Function,
        ) -> Result<$crate::CompiledFunction, $crate::IrError> {
            $crate::pipeline::compiler::CompileSession::new($module::TargetMachine::new())
                .compile(func)
        }
    };
}
