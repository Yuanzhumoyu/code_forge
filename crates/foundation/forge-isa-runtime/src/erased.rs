//! `impl_erased_target_machine!` —— 把具体 `TargetMachine` 接到宿主编译管线（注册表）。
//!
//! 运行面不认识编译管线：宏只按 ISA 名查 [`crate::pipeline`] 的注册表；宿主负责注册。

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
                $crate::pipeline::compile_via_pipeline(self.erased_name(), self, func)
            }
        }
    };
}
