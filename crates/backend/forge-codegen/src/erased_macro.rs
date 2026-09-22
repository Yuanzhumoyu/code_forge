//! `impl_erased_target_machine!` —— 把具体 TargetMachine 接到**宿主的编译管线**上。
//!
//! 它天生依赖宿主的管线类型（`FunctionCompiler`），因此定义在宿主 crate 而不是
//! `forge-isa-runtime`：runtime 只提供 [`ErasedTargetMachine`] trait。V1b 会把本宏
//! 改成 runtime 的带参版本（`pipeline = <路径>`），使第三方宿主也能启用 `tm`。

/// 为具体 TargetMachine 类型实现 ErasedTargetMachine。
///
/// 要求目标类型实现 `Clone`（所有字段为 `Arc`，因此廉价）。
///
/// # Example
/// ```ignore
/// impl_erased_target_machine!(X86TargetMachine);
/// ```
#[macro_export]
macro_rules! impl_erased_target_machine {
    ($tm:ty) => {
        impl $crate::machine::target::ErasedTargetMachine for $tm {
            fn erased_name(&self) -> &str {
                $crate::machine::target::TargetMachine::isa_info(self).name()
            }

            fn compile(
                &self,
                func: &$crate::ir::Function,
            ) -> Result<$crate::CompiledFunction, $crate::ir::IrError> {
                let compiler = $crate::pipeline::compiler::FunctionCompiler::new(self.clone());
                compiler.compile_raw(func)
            }
        }
    };
}
