//! 统一测试 harness — build → compile → exec 一条龙。
//!
//! 取代各 ISA 模块中重复的 `run()` / `run_f64()` / `run_test_i32()` helper，
//! 供 `exec!` / `exec_f64!` / `exec_args!` 宏与各 ISA 测试模块复用。
//!
//! 本机（x86_64）执行路径：`FunctionCompiler` → `ExecutableMemory` →
//! extern "C" 调用；带参数路径经 `executor::NativeExecutor`（≤4 整型参数）。

use crate::exec::executor::Executor;
use code_forge::backend::{CompiledFunction, FunctionCompiler};
use code_forge::mem::ExecutableMemory;
use code_forge::prelude::*;

/// 构建无参函数（body 返回单个值）。
pub fn build_func(name: &str, ret: TypeId, build: fn(&mut FunctionBuilder) -> Value) -> Function {
    let sig = FunctionSignature::new(&[], &[ret]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    b.create_block_here();
    let v = build(&mut b);
    b.ret(&[v]);
    b.finish().expect("build")
}

/// 构建带参函数（build 接收参数 Value 切片，返回单个值）。
pub fn build_func_args(
    name: &str,
    params: &[(TypeId, &'static str)],
    ret: TypeId,
    build: fn(&mut FunctionBuilder, &[Value]) -> Value,
) -> Function {
    let sig = FunctionSignature::new(params, &[ret]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    let (block, p) = b.create_block_with_params(params);
    b.switch_to_block(block);
    let v = build(&mut b, &p);
    b.ret(&[v]);
    b.finish().expect("build")
}

/// 用 x86_64 后端编译。
pub fn compile_x86_64(name: &str, func: &Function) -> CompiledFunction {
    code_forge::backend::x86_64::ensure_registered();
    FunctionCompiler::new(code_forge::backend::x86_64::TargetMachine::new())
        .compile_raw(func)
        .unwrap_or_else(|e| panic!("{name}: compile: {e:?}"))
}

/// 用任意 TargetMachine 编译（compile-only 场景；执行交由各架构执行器）。
pub fn compile<M: code_forge::backend::TargetMachine>(
    name: &str,
    machine: impl Fn() -> M,
    func: &Function,
) -> CompiledFunction {
    FunctionCompiler::new(machine())
        .compile_raw(func)
        .unwrap_or_else(|e| panic!("{name}: compile: {e:?}"))
}

/// compile-only 断言：构建无参函数并编译成功，产物非空。
pub fn compile_ok<M: code_forge::backend::TargetMachine>(
    name: &str,
    machine: impl Fn() -> M,
    ret: TypeId,
    build: fn(&mut FunctionBuilder) -> Value,
) {
    let func = build_func(name, ret, build);
    let compiled = compile(name, machine, &func);
    assert!(!compiled.code.is_empty(), "{name}: empty code");
}

/// 本机执行：无参返回 i32。
pub fn run_i32(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> i32 {
    let func = build_func(name, TypeId::I32, build);
    let compiled = compile_x86_64(name, &func);
    assert!(!compiled.code.is_empty(), "{name}: empty code");
    let mem = ExecutableMemory::new(&compiled.code).expect("ExecutableMemory::new");
    let f: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

/// 本机执行：无参返回 i64。
pub fn run_i64(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> i64 {
    let func = build_func(name, TypeId::I64, build);
    let compiled = compile_x86_64(name, &func);
    assert!(!compiled.code.is_empty(), "{name}: empty code");
    let mem = ExecutableMemory::new(&compiled.code).expect("ExecutableMemory::new");
    let f: extern "C" fn() -> i64 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

/// 本机执行：无参返回 f64。
pub fn run_f64(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> f64 {
    let func = build_func(name, TypeId::F64, build);
    let compiled = compile_x86_64(name, &func);
    assert!(!compiled.code.is_empty(), "{name}: empty code");
    let mem = ExecutableMemory::new(&compiled.code).expect("ExecutableMemory::new");
    let f: extern "C" fn() -> f64 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

/// 本机执行：无参返回 f32（第四十五轮——jit.rs 转发用）。
pub fn run_f32(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> f32 {
    let func = build_func(name, TypeId::F32, build);
    let compiled = compile_x86_64(name, &func);
    assert!(!compiled.code.is_empty(), "{name}: empty code");
    let mem = ExecutableMemory::new(&compiled.code).expect("ExecutableMemory::new");
    let f: extern "C" fn() -> f32 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

/// 本机执行：无参返回 i1（转 bool——jit.rs 转发用）。
pub fn run_bool(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> bool {
    let func = build_func(name, TypeId::I8, build);
    let compiled = compile_x86_64(name, &func);
    assert!(!compiled.code.is_empty(), "{name}: empty code");
    let mem = ExecutableMemory::new(&compiled.code).expect("ExecutableMemory::new");
    let f: extern "C" fn() -> u8 = unsafe { mem.get_fn(0).unwrap() };
    f() != 0
}

/// 本机执行：无参无返回值（构建块无 ret——jit.rs 转发用）。
pub fn run_block(name: &str, build: fn(&mut FunctionBuilder)) -> i32 {
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    build(&mut b);
    let func = b.finish().expect("build");
    let compiled = compile_x86_64(name, &func);
    assert!(!compiled.code.is_empty(), "{name}: empty code");
    let mem = ExecutableMemory::new(&compiled.code).expect("ExecutableMemory::new");
    let f: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

/// 本机执行：带整型参数返回 i64（≤4 参数，经 NativeExecutor）。
pub fn run_args_i64(
    name: &str,
    params: &[(TypeId, &'static str)],
    args: &[u64],
    build: fn(&mut FunctionBuilder, &[Value]) -> Value,
) -> i64 {
    assert_eq!(
        params.len(),
        args.len(),
        "{name}: params/args count mismatch ({} vs {})",
        params.len(),
        args.len()
    );
    let func = build_func_args(name, params, TypeId::I64, build);
    let compiled = compile_x86_64(name, &func);
    crate::exec::executor::NativeExecutor.exec(&compiled, args) as i64
}
