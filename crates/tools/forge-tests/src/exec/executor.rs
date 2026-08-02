//! Executor trait — 统一的 JIT 产物执行抽象。
//!
//! - 本机（x86_64）：`ExecutableMemory` + extern "C" 调用（真实执行）
//! - unicorn（aarch64/riscv64，exec-unicorn feature）：模拟执行
//!
//! 返回值寄存器映射与 `[abi.ret_regs]` 一致：x86 RAX / aarch64 X0 / riscv64 X10。

use code_forge::backend::CompiledFunction;

/// 执行器的架构信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecArch {
    X86_64,
    Aarch64,
    Riscv64,
}

impl ExecArch {
    pub fn name(&self) -> &'static str {
        match self {
            ExecArch::X86_64 => "x86_64",
            ExecArch::Aarch64 => "aarch64",
            ExecArch::Riscv64 => "riscv64",
        }
    }
}

/// 统一执行抽象：把编译产物执行起来并读返回值。
pub trait Executor {
    /// 本执行器支持的架构。
    fn arch(&self) -> ExecArch;

    /// 执行编译产物（无参 → i64 返回值）。
    fn exec(&self, compiled: &CompiledFunction, args: &[u64]) -> u64;
}

/// 本机执行器（x86_64：ExecutableMemory + extern "C" 调用）。
pub struct NativeExecutor;

impl Executor for NativeExecutor {
    fn arch(&self) -> ExecArch {
        ExecArch::X86_64
    }

    fn exec(&self, compiled: &CompiledFunction, args: &[u64]) -> u64 {
        assert!(
            args.len() <= 4,
            "native x64 executor supports at most 4 integer args"
        );
        let mem =
            code_forge::mem::ExecutableMemory::new(&compiled.code).expect("ExecutableMemory::new");
        match args.len() {
            0 => {
                let f: extern "C" fn() -> i64 = unsafe { mem.get_fn(0).unwrap() };
                f() as u64
            }
            1 => {
                let f: extern "C" fn(i64) -> i64 = unsafe { mem.get_fn(0).unwrap() };
                f(args[0] as i64) as u64
            }
            2 => {
                let f: extern "C" fn(i64, i64) -> i64 = unsafe { mem.get_fn(0).unwrap() };
                f(args[0] as i64, args[1] as i64) as u64
            }
            _ => {
                let f: extern "C" fn(i64, i64, i64, i64) -> i64 = unsafe { mem.get_fn(0).unwrap() };
                f(
                    args[0] as i64,
                    args[1] as i64,
                    args[2] as i64,
                    args[3] as i64,
                ) as u64
            }
        }
    }
}

/// unicorn 执行器（aarch64/riscv64 模拟；x86_64 也可经 unicorn 执行）。
#[cfg(feature = "exec-unicorn")]
pub struct UnicornExecutor {
    arch: ExecArch,
}

#[cfg(feature = "exec-unicorn")]
impl UnicornExecutor {
    pub fn new(arch: ExecArch) -> Self {
        Self { arch }
    }
}

#[cfg(feature = "exec-unicorn")]
impl Executor for UnicornExecutor {
    fn arch(&self) -> ExecArch {
        self.arch
    }

    fn exec(&self, compiled: &CompiledFunction, args: &[u64]) -> u64 {
        crate::exec::unicorn::exec_code(self.arch.name(), &compiled.code, args)
    }
}

/// 便捷入口：按架构选择执行器并执行。
pub fn run(arch: ExecArch, compiled: &CompiledFunction, args: &[u64]) -> u64 {
    match arch {
        ExecArch::X86_64 => NativeExecutor.exec(compiled, args),
        #[cfg(feature = "exec-unicorn")]
        ExecArch::Aarch64 | ExecArch::Riscv64 => UnicornExecutor::new(arch).exec(compiled, args),
        #[cfg(not(feature = "exec-unicorn"))]
        other => panic!(
            "exec: arch {:?} requires the `exec-unicorn` feature (unicorn engine)",
            other
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use code_forge::backend::FunctionCompiler;
    use code_forge::prelude::*;

    fn compile_add() -> CompiledFunction {
        let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("add", TypeContext::new(), sig);
        let (_block, params) = b.create_block_here_with([(TypeId::I64, "a"), (TypeId::I64, "b")]);
        let s = b.iadd(params[0], params[1]);
        b.ret(&[s]);
        let func = b.finish();
        FunctionCompiler::new(code_forge::backend::x86_64::TargetMachine::new())
            .compile_raw(&func)
            .expect("compile add")
    }

    #[test]
    fn native_exec_add() {
        let compiled = compile_add();
        let r = NativeExecutor.exec(&compiled, &[20, 22]);
        assert_eq!(r, 42);
    }
}
