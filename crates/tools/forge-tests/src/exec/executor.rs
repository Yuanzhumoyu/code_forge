//! Executor trait — 统一的 JIT 产物执行抽象。
//!
//! - 本机（x86_64）：`ExecutableMemory` + extern "C" 调用（真实执行）
//! - riscv64（QEMU system-mode + semihosting）：见 `super::qemu`
//! - aarch64（QEMU system-mode + semihosting）：见 `super::qemu_aarch64`
//!
//! 返回值寄存器映射与 `[abi.ret_regs]` 一致：x86 RAX、riscv a0、arm64 x0。

use code_forge::backend::CompiledFunction;

/// 执行器的架构信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecArch {
    X86_64,
    Riscv64,
    AArch64,
}

impl ExecArch {
    pub fn name(&self) -> &'static str {
        match self {
            ExecArch::X86_64 => "x86_64",
            ExecArch::Riscv64 => "riscv64",
            ExecArch::AArch64 => "aarch64",
        }
    }
}

/// 统一执行抽象：把编译产物执行起来并读返回值。
pub trait Executor {
    /// 本执行器支持的架构。
    fn arch(&self) -> ExecArch;

    /// 执行编译产物（无参 → i64 返回值）。
    fn exec(&self, compiled: &CompiledFunction, args: &[u64]) -> u64;

    /// 执行多函数模块（跨函数 Call/递归路径）。默认不支持（x86 走 JIT
    /// 内存执行；riscv QEMU 覆盖）。`funcs` 为 FuncRef 序；`globals` 为
    /// (名字, init 字节) 序（QEMU 裸机打包布局数据段；x86 JIT 自管）。
    fn exec_module(
        &self,
        _funcs: &[(String, CompiledFunction)],
        _globals: &[(String, Vec<u8>)],
        _main: &str,
        _args: &[u64],
    ) -> u64 {
        panic!(
            "exec_module not supported by {} (use JitCompiler for native)",
            self.arch().name()
        )
    }
}

/// 本机执行器（x86_64：ExecutableMemory + extern "C" 调用）。
pub struct NativeExecutor;

impl Executor for NativeExecutor {
    fn arch(&self) -> ExecArch {
        ExecArch::X86_64
    }

    fn exec(&self, compiled: &CompiledFunction, args: &[u64]) -> u64 {
        // P1-15：native x86_64 执行仅在 x86_64 宿主可用（ExecutableMemory +
        // extern "C" 调用 x86 机器码）——macOS arm64 CI runner 上会 SIGILL。
        // 非 x86_64 时显式 panic（调用方应改用 QEMU/CompileOnly）。
        #[cfg(not(target_arch = "x86_64"))]
        {
            panic!(
                "NativeExecutor 仅在 x86_64 宿主可用（当前 {}）——请用 QEMU 或 CompileOnly",
                std::env::consts::ARCH
            );
        }
        #[cfg(target_arch = "x86_64")]
        {
            assert!(
                args.len() <= 4,
                "native x64 executor supports at most 4 integer args"
            );
            let mem = code_forge::mem::ExecutableMemory::new(&compiled.code)
                .expect("ExecutableMemory::new");
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
                    let f: extern "C" fn(i64, i64, i64, i64) -> i64 =
                        unsafe { mem.get_fn(0).unwrap() };
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
}

/// QEMU riscv64 执行器（system-mode + sifive_test；见 `super::qemu`）。
/// 本机无 QEMU 时 `exec` panic（调用方应先用 `qemu_riscv64_path()` 探测）。
pub struct QemuRiscv64Executor;

/// sifive_test 退出码 = `(value >> 16) & 0xFFFF`（**16 位**，实测 QEMU
/// 11.0.92：a0=-1 → 65535、a0=-42 → 65494；Windows 保留 16 位退出码）。
/// 返回 i64 前按 **有符号 16 位**符号扩展（0xFFD6 → -42），与用例期望对齐。
fn sign_extend_exit(raw: u64) -> u64 {
    (raw as u16 as i16 as i64) as u64
}

impl Executor for QemuRiscv64Executor {
    fn arch(&self) -> ExecArch {
        ExecArch::Riscv64
    }

    fn exec(&self, compiled: &CompiledFunction, args: &[u64]) -> u64 {
        let raw = super::qemu::exec_riscv64(compiled, args)
            .unwrap_or_else(|e| panic!("QemuRiscv64Executor: {e}"));
        sign_extend_exit(raw)
    }

    fn exec_module(
        &self,
        funcs: &[(String, CompiledFunction)],
        globals: &[(String, Vec<u8>)],
        main: &str,
        args: &[u64],
    ) -> u64 {
        let raw = super::qemu::exec_riscv64_module(funcs, globals, main, args)
            .unwrap_or_else(|e| panic!("QemuRiscv64Executor::exec_module: {e}"));
        sign_extend_exit(raw)
    }
}

/// QEMU aarch64 执行器（system-mode + semihosting；见 `super::qemu_aarch64`）。
/// 本机无 QEMU 时 `exec` panic（调用方应先用 `qemu_aarch64_path()` 探测）。
pub struct QemuAarch64Executor;

/// aarch64 semihosting 退出码以字节返回（OS 退出码 0-255）：按**有符号
/// 8 位**符号扩展（255 → -1），与用例期望对齐。
fn sign_extend_exit8(raw: u64) -> u64 {
    (raw as u8 as i8 as i64) as u64
}

impl Executor for QemuAarch64Executor {
    fn arch(&self) -> ExecArch {
        ExecArch::AArch64
    }

    fn exec(&self, compiled: &CompiledFunction, args: &[u64]) -> u64 {
        let raw = super::qemu_aarch64::exec_aarch64(compiled, args)
            .unwrap_or_else(|e| panic!("QemuAarch64Executor: {e}"));
        sign_extend_exit8(raw)
    }

    fn exec_module(
        &self,
        funcs: &[(String, CompiledFunction)],
        globals: &[(String, Vec<u8>)],
        main: &str,
        args: &[u64],
    ) -> u64 {
        let _ = funcs;
        let _ = globals;
        let _ = main;
        let _ = args;
        // 多函数模块路径（跨函数 call 偏移 patch）为后续迭代；当前单函数入口
        panic!("QemuAarch64Executor::exec_module 尚未实现（多函数 call 偏移 patch 后置）")
    }
}

/// 便捷入口：按架构选择执行器。
pub fn run(arch: ExecArch, compiled: &CompiledFunction, args: &[u64]) -> u64 {
    match arch {
        ExecArch::X86_64 => NativeExecutor.exec(compiled, args),
        ExecArch::Riscv64 => QemuRiscv64Executor.exec(compiled, args),
        ExecArch::AArch64 => QemuAarch64Executor.exec(compiled, args),
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
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
        let func = b.finish().expect("build");
        FunctionCompiler::new(code_forge::backend::x86_v12::TargetMachine::new())
            .compile_raw(&func)
            .expect("compile add")
    }

    #[test]
    #[cfg(all(target_arch = "x86_64", windows))]
    fn native_exec_add() {
        let compiled = compile_add();
        let r = NativeExecutor.exec(&compiled, &[20, 22]);
        assert_eq!(r, 42);
    }
}
