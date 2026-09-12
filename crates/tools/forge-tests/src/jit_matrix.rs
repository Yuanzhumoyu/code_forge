//! 架构无关 JIT 集成测试矩阵（IR 级，防重复编写）。
//!
//! 每个用例 = (名字, 所需 IR op 集合, 执行函数)。执行函数用注入的
//! `TargetMachine` 编译 + JIT 运行并返回结果；能力集（`Capabilities`）决定
//! 哪些用例可跑——所需 op 未实现 → `Skip`（不失败），实现后自动转绿。
//! ISA 差异只存在于薄 runner（`isa/<name>/` 绑定机器 + 能力集），用例本身
//! 零 ISA 引用（grep 保证）。
//!
//! 用例移植自旧 `forge-tests/src/isa/x86_64/jit.rs`（v11 时代随语法层删除；
//! 断言语义保持，作为 v12 回归网）。

use code_forge::backend::TargetMachine;
use code_forge::backend::{CompiledFunction, FunctionCompiler};
use code_forge::mem::ExecutableMemory;
use code_forge::prelude::*;

/// 无参构建：`fn(&mut FunctionBuilder) -> Value`（返回单个值）。
pub type BuildFn = fn(&mut FunctionBuilder) -> Value;
/// 无参构建：`fn(&mut FunctionBuilder)`（无返回值，如块构建）。
pub type BuildBlockFn = fn(&mut FunctionBuilder);
/// 带参构建：`fn(&mut FunctionBuilder, &[Value]) -> Value`。
pub type BuildArgsFn = fn(&mut FunctionBuilder, &[Value]) -> Value;
/// 模块构建：`fn(&mut Module) -> FuncRef`（返回 main 函数引用；约定 main
/// 名为 "main"，runner 经 JitCompiler::compile_module 编译后取之执行）。
pub type BuildModuleFn = fn(&mut Module) -> FuncRef;

/// 用例种类（决定如何执行与断言）。
pub enum CaseKind {
    /// 返回 i32，断言等于期望值。
    I32(BuildFn, i32),
    /// 返回 i64，断言等于期望值。
    I64(BuildFn, i64),
    /// 返回 f64，近似断言（1e-6）。
    F64(BuildFn, f64),
    /// 返回 i1/bool，断言等于期望值。
    Bool(BuildFn, bool),
    /// 无返回值块，返回 i32，断言等于期望值。
    Block(BuildBlockFn, i32),
    /// 带 ≤4 个 i64 参数，返回 i64，断言等于期望值。
    Args {
        params: &'static [(TypeId, &'static str)],
        args: &'static [u64],
        build: BuildArgsFn,
        expected: i64,
    },
    /// 带 ≤4 个 f64 参数（XMM 传参），返回 i64，断言等于期望值。
    F64Args {
        params: &'static [(TypeId, &'static str)],
        args: &'static [u64], // f64 位模式
        build: BuildArgsFn,
        expected: i64,
    },
    /// 编译级验证（不执行；如浮点参数收参的编译）。
    CompileOnly(BuildFn),
    /// 多函数模块（跨函数 Call）：构建 Module，JIT 编译后执行 main。
    Module(BuildModuleFn, i64),
}

/// 矩阵用例。
pub struct Case {
    pub name: &'static str,
    /// 所需 IR op 名（能力集门控；空 = 无 op 依赖）。
    pub ops: &'static [&'static str],
    pub kind: CaseKind,
}

/// ISA 能力集：支持哪些 IR op（+ AVX 检测由 runner 提供）。
#[derive(Default, Clone)]
pub struct Capabilities {
    pub ops: std::collections::HashSet<&'static str>,
}

impl Capabilities {
    pub fn new(supported: &[&'static str]) -> Self {
        Self {
            ops: supported.iter().copied().collect(),
        }
    }
    /// 用例所需的 op 是否全部支持。
    pub fn supports(&self, ops: &[&'static str]) -> bool {
        ops.iter().all(|o| self.ops.contains(o))
    }
}

/// 运行器：机器工厂 + 能力集 + 可选执行器。
/// `exec` 为 None → 本机 ExecutableMemory（x86_64 快路径）；
/// Some(executor) → 按架构执行（riscv64 走 QEMU）。
pub struct Runner<M: TargetMachine> {
    pub machine: fn() -> M,
    pub caps: Capabilities,
    pub exec: Option<Box<dyn crate::exec::executor::Executor>>,
}

/// 用例结果。
pub enum Outcome {
    Pass,
    /// 所需 op 未实现（能力集外）——不算失败。
    Skip(&'static str),
    /// 执行失败或断言不符。
    Fail(String),
}

/// 把矩阵结果落成 `FORGE_JIT_EVENTS` 事件（`code_forge::backend::jit_event`）：
/// 摘要一行 + 每条 Skip/Fail 一行（含原因）。libtest 会吞掉**通过**测试的输出，
/// 于是"跳过了哪些用例、为什么跳过"在 CI 日志里完全不可见（与 AVX-512 门控
/// 用例同类问题）——事件是唯一可核对的通道。
pub fn emit_events(isa: &str, results: &[(String, Outcome)]) {
    let (mut pass, mut skip) = (0usize, 0usize);
    let mut fails: Vec<String> = Vec::new();
    for (name, outcome) in results {
        match outcome {
            Outcome::Pass => pass += 1,
            Outcome::Skip(why) => {
                skip += 1;
                code_forge::backend::jit_event("MATRIX-SKIP", &format!("{isa} {name} ({why})"));
            }
            Outcome::Fail(e) => fails.push(format!("{name}: {e}")),
        }
    }
    for f in &fails {
        code_forge::backend::jit_event("MATRIX-FAIL", &format!("{isa} {f}"));
    }
    code_forge::backend::jit_event(
        "MATRIX-SUMMARY",
        &format!("{isa} pass={pass} skip={skip} fail={}", fails.len()),
    );
}

/// 执行单个用例（Result 内部实现；`?` 传播执行/断言错误）。
fn run_case_impl<M: TargetMachine + Clone>(r: &Runner<M>, c: &Case) -> Result<i64, String> {
    match &c.kind {
        CaseKind::I32(build, expected) => {
            let v = run_i32(r, c.name, *build)?;
            if v != *expected {
                return Err(format!("{}: got {v}, expected {expected}", c.name));
            }
            Ok(v as i64)
        }
        CaseKind::I64(build, expected) => {
            let v = run_i64(r, c.name, *build)?;
            if v != *expected {
                return Err(format!("{}: got {v}, expected {expected}", c.name));
            }
            Ok(v)
        }
        CaseKind::F64(build, expected) => {
            let v = run_f64(r, c.name, *build)?;
            if (v - expected).abs() > 1e-6 {
                return Err(format!("{}: got {v}, expected {expected}", c.name));
            }
            Ok(v as i64)
        }
        CaseKind::Bool(build, expected) => {
            let v = run_bool(r, c.name, *build)?;
            if v != *expected {
                return Err(format!("{}: got {v}, expected {expected}", c.name));
            }
            Ok(v as i64)
        }
        CaseKind::Block(build, expected) => {
            let v = run_block(r, c.name, *build)?;
            if v != *expected {
                return Err(format!("{}: got {v}, expected {expected}", c.name));
            }
            Ok(v as i64)
        }
        CaseKind::Args {
            params,
            args,
            build,
            expected,
        } => {
            let v = run_args_i64(r, c.name, params, args, *build)?;
            if v != *expected {
                return Err(format!("{}: got {v}, expected {expected}", c.name));
            }
            Ok(v)
        }
        CaseKind::F64Args {
            params,
            args,
            build,
            expected,
        } => {
            let v = run_args_f64(r, c.name, params, args, *build)?;
            if v != *expected {
                return Err(format!("{}: got {v}, expected {expected}", c.name));
            }
            Ok(v)
        }
        CaseKind::CompileOnly(build) => {
            compile_only(r, c.name, *build)?;
            Ok(0i64)
        }
        CaseKind::Module(build, expected) => {
            let v = run_module(r, c.name, *build)?;
            if v != *expected {
                return Err(format!("{}: got {v}, expected {expected}", c.name));
            }
            Ok(v)
        }
    }
}

/// 执行单个用例（能力集门控 → Skip）。
pub fn run_case<M: TargetMachine + Clone>(r: &Runner<M>, c: &Case) -> Outcome {
    if !r.caps.supports(c.ops) {
        return Outcome::Skip("capability");
    }
    match run_case_impl(r, c) {
        Ok(_) => Outcome::Pass,
        Err(e) => Outcome::Fail(e),
    }
}

/// 期望值是否可被 QEMU 退出码表示。sifive_test 退出码 **16 位有符号**
///（±32767 内；Windows 保留 16 位退出码，负值经符号扩展还原）。大值
/// 用例（i32::MAX/MIN、0x1234 等）超 16 位 → runner 过滤为 Skip
/// （CompileOnly 仍由 forge-codegen 的 tm_tests 覆盖）。
pub fn expected_fits_u8(c: &Case) -> bool {
    let v = match &c.kind {
        CaseKind::I32(_, e) => *e as i64,
        CaseKind::I64(_, e) => *e,
        CaseKind::Bool(_, _) => return true,
        CaseKind::Block(_, e) => *e as i64,
        CaseKind::Args { expected, .. } => *expected,
        CaseKind::F64Args { expected, .. } => *expected,
        CaseKind::F64(_, _) => return false, // riscv 无浮点
        CaseKind::CompileOnly(_) => return true,
        CaseKind::Module(_, e) => *e,
    };
    (-32767..=32767).contains(&v)
}

/// 运行全部用例，可附加**值域过滤**（过滤为 Skip("value-range")）。
pub fn run_all_filtered<M: TargetMachine + Clone>(
    r: &Runner<M>,
    filter: impl Fn(&Case) -> bool,
) -> Vec<(String, Outcome)> {
    CASES
        .iter()
        .map(|c| {
            let outcome = if !filter(c) {
                Outcome::Skip("value-range")
            } else {
                run_case(r, c)
            };
            (c.name.to_string(), outcome)
        })
        .collect()
}

// ───────────────────────── 执行原语（机器注入）─────────────────────────

fn compile_raw<M: TargetMachine>(
    r: &Runner<M>,
    name: &str,
    func: &Function,
) -> Result<CompiledFunction, String> {
    FunctionCompiler::new((r.machine)())
        .compile_raw(func)
        .map_err(|e| format!("{name}: compile: {e:?}"))
}

/// 统一执行：有注入执行器 → 走它；否则本机 ExecutableMemory。
fn exec_code<M: TargetMachine>(
    r: &Runner<M>,
    name: &str,
    compiled: &CompiledFunction,
    args: &[u64],
) -> Result<u64, String> {
    if let Some(ex) = &r.exec {
        return Ok(ex.exec(compiled, args));
    }
    let mem = ExecutableMemory::new(&compiled.code).map_err(|e| format!("{name}: alloc: {e}"))?;
    unsafe {
        let f: extern "C" fn() -> i64 = mem.get_fn(0).unwrap();
        let _ = args;
        Ok(f() as u64)
    }
}

fn run_i32<M: TargetMachine>(r: &Runner<M>, name: &str, build: BuildFn) -> Result<i32, String> {
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    b.create_block_here();
    let v = build(&mut b);
    b.ret(&[v]);
    let func = b.finish().expect("build");
    let compiled = compile_raw(r, name, &func)?;
    if r.exec.is_some() {
        // 注入执行器（riscv QEMU）：返回值低 32 位
        return Ok(exec_code(r, name, &compiled, &[])? as u32 as i32);
    }
    let mem = ExecutableMemory::new(&compiled.code).map_err(|e| format!("{name}: alloc: {e}"))?;
    let f: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
    Ok(f())
}

fn run_i64<M: TargetMachine>(r: &Runner<M>, name: &str, build: BuildFn) -> Result<i64, String> {
    let sig = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    b.create_block_here();
    let v = build(&mut b);
    b.ret(&[v]);
    let func = b.finish().expect("build");
    let compiled = compile_raw(r, name, &func)?;
    if r.exec.is_some() {
        return Ok(exec_code(r, name, &compiled, &[])? as i64);
    }
    let mem = ExecutableMemory::new(&compiled.code).map_err(|e| format!("{name}: alloc: {e}"))?;
    let f: extern "C" fn() -> i64 = unsafe { mem.get_fn(0).unwrap() };
    Ok(f())
}

fn run_f64<M: TargetMachine>(r: &Runner<M>, name: &str, build: BuildFn) -> Result<f64, String> {
    let sig = FunctionSignature::new(&[], &[TypeId::F64]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    b.create_block_here();
    let v = build(&mut b);
    b.ret(&[v]);
    let func = b.finish().expect("build");
    let compiled = compile_raw(r, name, &func)?;
    if r.exec.is_some() {
        // QEMU 整数退出码无法表达 f64 —— 该路径不应在 riscv 使用（能力集门控）
        return Err(format!(
            "{name}: f64 exec not supported by injected executor"
        ));
    }
    let mem = ExecutableMemory::new(&compiled.code).map_err(|e| format!("{name}: alloc: {e}"))?;
    let f: extern "C" fn() -> f64 = unsafe { mem.get_fn(0).unwrap() };
    Ok(f())
}

fn run_bool<M: TargetMachine>(r: &Runner<M>, name: &str, build: BuildFn) -> Result<bool, String> {
    let sig = FunctionSignature::new(&[], &[TypeId::I8]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    b.create_block_here();
    let v = build(&mut b);
    b.ret(&[v]);
    let func = b.finish().expect("build");
    let compiled = compile_raw(r, name, &func)?;
    if r.exec.is_some() {
        return Ok(exec_code(r, name, &compiled, &[])? != 0);
    }
    let mem = ExecutableMemory::new(&compiled.code).map_err(|e| format!("{name}: alloc: {e}"))?;
    let f: extern "C" fn() -> u8 = unsafe { mem.get_fn(0).unwrap() };
    Ok(f() != 0)
}

fn run_block<M: TargetMachine>(
    r: &Runner<M>,
    name: &str,
    build: BuildBlockFn,
) -> Result<i32, String> {
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    build(&mut b);
    let func = b.finish().expect("build");
    let compiled = compile_raw(r, name, &func)?;
    if r.exec.is_some() {
        return Ok(exec_code(r, name, &compiled, &[])? as u32 as i32);
    }
    let mem = ExecutableMemory::new(&compiled.code).map_err(|e| format!("{name}: alloc: {e}"))?;
    let f: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
    Ok(f())
}

fn run_args_i64<M: TargetMachine>(
    r: &Runner<M>,
    name: &str,
    params: &[(TypeId, &'static str)],
    args: &[u64],
    build: BuildArgsFn,
) -> Result<i64, String> {
    assert_eq!(params.len(), args.len(), "{name}: params/args mismatch");
    let sig = FunctionSignature::new(params, &[TypeId::I64]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    let (block, p) = b.create_block_with_params(params);
    b.switch_to_block(block);
    let v = build(&mut b, &p);
    b.ret(&[v]);
    let func = b.finish().expect("build");
    let compiled = compile_raw(r, name, &func)?;
    if r.exec.is_some() {
        let a: Vec<u64> = args.to_vec();
        return Ok(exec_code(r, name, &compiled, &a)? as i64);
    }
    let mem = ExecutableMemory::new(&compiled.code).map_err(|e| format!("{name}: alloc: {e}"))?;
    let a: Vec<i64> = args.iter().map(|&x| x as i64).collect();
    unsafe {
        match a.len() {
            1 => {
                let f: extern "C" fn(i64) -> i64 = mem.get_fn(0).unwrap();
                Ok(f(a[0]))
            }
            2 => {
                let f: extern "C" fn(i64, i64) -> i64 = mem.get_fn(0).unwrap();
                Ok(f(a[0], a[1]))
            }
            3 => {
                let f: extern "C" fn(i64, i64, i64) -> i64 = mem.get_fn(0).unwrap();
                Ok(f(a[0], a[1], a[2]))
            }
            4 => {
                let f: extern "C" fn(i64, i64, i64, i64) -> i64 = mem.get_fn(0).unwrap();
                Ok(f(a[0], a[1], a[2], a[3]))
            }
            n => Err(format!("{name}: unsupported arg count {n}")),
        }
    }
}

/// 带 ≤4 个 f64 参数（XMM0-3 传参），返回 i64。args 为 f64 位模式。
fn run_args_f64<M: TargetMachine>(
    r: &Runner<M>,
    name: &str,
    params: &[(TypeId, &'static str)],
    args: &[u64],
    build: BuildArgsFn,
) -> Result<i64, String> {
    assert_eq!(params.len(), args.len(), "{name}: params/args mismatch");
    let sig = FunctionSignature::new(params, &[TypeId::I64]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    let (block, p) = b.create_block_with_params(params);
    b.switch_to_block(block);
    let v = build(&mut b, &p);
    b.ret(&[v]);
    let func = b.finish().expect("build");
    let compiled = compile_raw(r, name, &func)?;
    if r.exec.is_some() {
        return Err(format!(
            "{name}: f64 args exec not supported by injected executor"
        ));
    }
    let mem = ExecutableMemory::new(&compiled.code).map_err(|e| format!("{name}: alloc: {e}"))?;
    let a: Vec<f64> = args.iter().map(|&x| f64::from_bits(x)).collect();
    unsafe {
        match a.len() {
            1 => {
                let f: extern "C" fn(f64) -> i64 = mem.get_fn(0).unwrap();
                Ok(f(a[0]))
            }
            2 => {
                let f: extern "C" fn(f64, f64) -> i64 = mem.get_fn(0).unwrap();
                Ok(f(a[0], a[1]))
            }
            3 => {
                let f: extern "C" fn(f64, f64, f64) -> i64 = mem.get_fn(0).unwrap();
                Ok(f(a[0], a[1], a[2]))
            }
            4 => {
                let f: extern "C" fn(f64, f64, f64, f64) -> i64 = mem.get_fn(0).unwrap();
                Ok(f(a[0], a[1], a[2], a[3]))
            }
            n => Err(format!("{name}: unsupported arg count {n}")),
        }
    }
}

fn compile_only<M: TargetMachine>(r: &Runner<M>, name: &str, build: BuildFn) -> Result<(), String> {
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    b.create_block_here();
    let v = build(&mut b);
    b.ret(&[v]);
    let func = b.finish().expect("build");
    compile_raw(r, name, &func)?;
    Ok(())
}

/// 模块编译 + 执行 main（跨函数 Call 路径）。
/// 注入执行器（riscv QEMU）→ exec_module（多函数打包）；否则本机 JIT。
fn run_module<M: TargetMachine + Clone>(
    r: &Runner<M>,
    name: &str,
    build: BuildModuleFn,
) -> Result<i64, String> {
    let mut module = Module::new();
    let main_ref = build(&mut module);
    // 注入执行器（riscv QEMU）：编译所有函数 → exec_module
    if let Some(ex) = &r.exec {
        let funcs: Vec<(String, CompiledFunction)> = (0..module.function_count())
            .map(|i| {
                let fr = FuncRef(i as u32);
                let func = module.get_function(fr);
                let compiled = FunctionCompiler::new((r.machine)())
                    .compile_raw(func)
                    .map_err(|e| format!("{name}: compile fn{}: {e:?}", i))?;
                Ok((func.name.as_str().to_string(), compiled))
            })
            .collect::<Result<_, String>>()?;
        // globals：名字 + init 字节（QEMU 裸机打包布局数据段）
        let globals: Vec<(String, Vec<u8>)> = module
            .iter_globals()
            .map(|(_, gv)| {
                let init = gv
                    .init
                    .clone()
                    .unwrap_or_else(|| vec![0u8; module.types.borrow().size_bytes(gv.ty) as usize]);
                (gv.name.as_str().to_string(), init)
            })
            .collect();
        let main_name = module.get_function(main_ref).name.as_str().to_string();
        return Ok(ex.exec_module(&funcs, &globals, &main_name, &[]) as i64);
    }
    let mut jit = code_forge::jit::JitCompiler::new((r.machine)());
    jit.compile_module(&module)
        .map_err(|e| format!("{name}: module compile: {e:?}"))?;
    let main_name = module.get_function(main_ref).name.clone();
    let f: extern "C" fn() -> i64 = jit
        .get_fn(main_name.as_str())
        .map_err(|e| format!("{name}: get_fn: {e:?}"))?;
    Ok(f())
}

// ───────────────────────── 用例注册表 ─────────────────────────

/// 外部辅助函数（CallIndirect 用例的目标；函数指针经 static 无捕获引用）。
extern "C" fn ext_add42(x: i64) -> i64 {
    x + 42
}
static EXT_ADD42: extern "C" fn(i64) -> i64 = ext_add42;

/// 全部用例（架构无关）。所需 op 见 `ops`——能力集未覆盖时自动 Skip。
pub const CASES: &[Case] = &[
    // ── 常量与算术（Iadd/Isub/Imul/Iconst）──
    Case {
        name: "const_42",
        ops: &["Iconst"],
        kind: CaseKind::I32(|b| b.iconst_i32(42), 42),
    },
    Case {
        name: "add",
        ops: &["Iadd"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(20);
                let c = b.iconst_i32(22);
                b.iadd(a, c)
            },
            42,
        ),
    },
    Case {
        name: "sub",
        ops: &["Isub"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(84);
                let c = b.iconst_i32(42);
                b.isub(a, c)
            },
            42,
        ),
    },
    Case {
        name: "mul",
        ops: &["Imul"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(6);
                let c = b.iconst_i32(7);
                b.imul(a, c)
            },
            42,
        ),
    },
    Case {
        name: "mul_add",
        ops: &["Imul", "Iadd"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(5);
                let c = b.iconst_i32(8);
                let m = b.imul(a, c);
                let d = b.iconst_i32(2);
                b.iadd(m, d)
            },
            42,
        ),
    },
    Case {
        name: "shift_add",
        ops: &["Ishl", "Iadd"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(20);
                let s = b.iconst_i32(1);
                let sh = b.ishl(a, s);
                let c = b.iconst_i32(2);
                b.iadd(sh, c)
            },
            42,
        ),
    },
    Case {
        name: "multi_ops",
        ops: &["Iadd", "Imul", "Isub"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(6);
                let c = b.iconst_i32(7);
                let m = b.imul(a, c);
                let d = b.iconst_i32(10);
                b.isub(m, d)
            },
            32,
        ),
    },
    Case {
        name: "chained_arithmetic",
        ops: &["Iadd", "Imul"],
        kind: CaseKind::I32(
            |b| {
                let x = b.iconst_i32(2);
                let y = b.iconst_i32(3);
                let z = b.iconst_i32(4);
                let s = b.iadd(x, y);
                b.imul(s, z)
            },
            20,
        ),
    },
    // ── 位运算（Band/Bor/Bxor/Bnot）──
    Case {
        name: "and",
        ops: &["Band"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(0xFF);
                let c = b.iconst_i32(42);
                b.band(a, c)
            },
            42,
        ),
    },
    Case {
        name: "or",
        ops: &["Bor"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(40);
                let c = b.iconst_i32(2);
                b.bor(a, c)
            },
            42,
        ),
    },
    Case {
        name: "xor",
        ops: &["Bxor"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(63);
                let c = b.iconst_i32(21);
                b.bxor(a, c)
            },
            42,
        ),
    },
    Case {
        name: "not",
        ops: &["Bnot"],
        kind: CaseKind::I64(
            |b| {
                let a = b.iconst_i64(!42i64);
                b.bnot(a)
            },
            42,
        ),
    },
    Case {
        name: "bitwise_chain",
        ops: &["Band", "Bor"],
        kind: CaseKind::I32(
            |b| {
                let x = b.iconst_i32(0x0F);
                let y = b.iconst_i32(0xF0);
                let a = b.bor(x, y);
                let m = b.iconst_i32(0xFF);
                b.band(a, m)
            },
            0xFF,
        ),
    },
    // ── 移位（Ishl/Ushr/Sshr）──
    Case {
        name: "shl",
        ops: &["Ishl"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(21);
                let s = b.iconst_i32(1);
                b.ishl(a, s)
            },
            42,
        ),
    },
    Case {
        name: "shr",
        ops: &["Ushr"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(168);
                let s = b.iconst_i32(2);
                b.ushr(a, s)
            },
            42,
        ),
    },
    Case {
        name: "sshr",
        ops: &["Sshr"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(-84);
                let s = b.iconst_i32(1);
                b.sshr(a, s)
            },
            -42,
        ),
    },
    Case {
        name: "shift_chain",
        ops: &["Ishl", "Ushr"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(84);
                let s1 = b.iconst_i32(1);
                let sh = b.ishl(a, s1);
                let s2 = b.iconst_i32(1);
                b.ushr(sh, s2)
            },
            84,
        ),
    },
    // ── 除法/取模（Sdiv/Srem/Udiv/Urem）──
    Case {
        name: "sdiv",
        ops: &["Sdiv"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(84);
                let c = b.iconst_i32(2);
                b.sdiv(a, c)
            },
            42,
        ),
    },
    Case {
        name: "sdiv_10_3",
        ops: &["Sdiv"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(10);
                let c = b.iconst_i32(3);
                b.sdiv(a, c)
            },
            3,
        ),
    },
    Case {
        name: "sdiv_var_module",
        ops: &["Sdiv", "Store", "Load", "StackAddr", "Module"],
        kind: CaseKind::Module(
            |m| {
                // 复刻 mini_c 变量除法：x=100; y=7; return x/y;
                // mini_c 的 alloc_slot 每次新建地址，但 load 复用 store 的地址
                // Value（SSA 共享——store 不消费地址 value，load 复用）。
                let sig = FunctionSignature::new(&[], &[TypeId::I32]);
                let mut b = FunctionBuilder::new("main", TypeContext::new(), sig);
                b.create_block_here();
                let base1 = b.stack_addr(0);
                let off1 = b.iconst_i32(-4);
                let p1 = b.iadd(base1, off1);
                let x = b.iconst_i32(100);
                b.store(x, p1);
                let base2 = b.stack_addr(0);
                let off2 = b.iconst_i32(-8);
                let p2 = b.iadd(base2, off2);
                let y = b.iconst_i32(7);
                b.store(y, p2);
                let lx = b.load(p1, TypeId::I32);
                let ly = b.load(p2, TypeId::I32);
                let r = b.sdiv(lx, ly);
                b.ret(&[r]);
                m.add_function(b.finish().expect("main"))
            },
            14,
        ),
    },
    Case {
        name: "udiv",
        ops: &["Udiv"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(126);
                let c = b.iconst_i32(3);
                b.udiv(a, c)
            },
            42,
        ),
    },
    Case {
        name: "srem",
        ops: &["Srem"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(100);
                let c = b.iconst_i32(7);
                b.srem(a, c)
            },
            2,
        ),
    },
    Case {
        name: "urem",
        ops: &["Urem"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(100);
                let c = b.iconst_i32(7);
                b.urem(a, c)
            },
            2,
        ),
    },
    // ── 比较（Icmp + 条件码；bool 经 icmp）──
    Case {
        name: "icmp_eq",
        ops: &["Icmp"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(42);
                let c = b.iconst_i32(42);
                b.icmp(IntCC::Equal, a, c)
            },
            true,
        ),
    },
    Case {
        name: "icmp_ne",
        ops: &["Icmp"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(42);
                let c = b.iconst_i32(43);
                b.icmp(IntCC::NotEqual, a, c)
            },
            true,
        ),
    },
    Case {
        name: "icmp_sgt",
        ops: &["Icmp"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(50);
                let c = b.iconst_i32(42);
                b.icmp(IntCC::SignedGreaterThan, a, c)
            },
            true,
        ),
    },
    Case {
        name: "icmp_slt",
        ops: &["Icmp"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(42);
                let c = b.iconst_i32(50);
                b.icmp(IntCC::SignedLessThan, a, c)
            },
            true,
        ),
    },
    Case {
        name: "icmp_sle",
        ops: &["Icmp"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(42);
                let c = b.iconst_i32(42);
                b.icmp(IntCC::SignedLessThanOrEqual, a, c)
            },
            true,
        ),
    },
    Case {
        name: "icmp_ult",
        ops: &["Icmp"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(1);
                let c = b.iconst_i32(2);
                b.icmp(IntCC::UnsignedLessThan, a, c)
            },
            true,
        ),
    },
    Case {
        name: "icmp_ugt",
        ops: &["Icmp"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(2);
                let c = b.iconst_i32(1);
                b.icmp(IntCC::UnsignedGreaterThan, a, c)
            },
            true,
        ),
    },
    Case {
        name: "icmp_uge",
        ops: &["Icmp"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(42);
                let c = b.iconst_i32(42);
                b.icmp(IntCC::UnsignedGreaterThanOrEqual, a, c)
            },
            true,
        ),
    },
    Case {
        name: "icmp_ule",
        ops: &["Icmp"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(42);
                let c = b.iconst_i32(42);
                b.icmp(IntCC::UnsignedLessThanOrEqual, a, c)
            },
            true,
        ),
    },
    // ── 符号扩展（Sextend）──
    Case {
        name: "sextend_negative",
        ops: &["Sextend"],
        kind: CaseKind::I64(
            |b| {
                let a = b.iconst_i32(-42);
                b.sextend(a, TypeId::I64)
            },
            -42,
        ),
    },
    Case {
        name: "sextend_minus_one",
        ops: &["Sextend"],
        kind: CaseKind::I64(
            |b| {
                let a = b.iconst_i32(-1);
                b.sextend(a, TypeId::I64)
            },
            -1,
        ),
    },
    Case {
        name: "sextend_positive",
        ops: &["Sextend"],
        kind: CaseKind::I64(
            |b| {
                let a = b.iconst_i32(42);
                b.sextend(a, TypeId::I64)
            },
            42,
        ),
    },
    // ── 内存（StackAddr/Load/Store）──
    Case {
        name: "stack_load_store",
        ops: &["StackAddr", "Load", "Store"],
        kind: CaseKind::I32(
            |b| {
                let addr = b.stack_addr(0);
                let v = b.iconst_i32(42);
                b.store(v, addr);
                b.load(addr, TypeId::I32)
            },
            42,
        ),
    },
    Case {
        name: "cross_block_store_load",
        ops: &["StackAddr", "Load", "Store"],
        kind: CaseKind::I32(
            |b| {
                let addr = b.stack_addr(0);
                let v = b.iconst_i32(7);
                b.store(v, addr);
                b.load(addr, TypeId::I32)
            },
            7,
        ),
    },
    // 帧槽地址算术（mini_c alloc_slot 形态）：`iadd(stack_addr(0), iconst_i32(-N))`
    // ——指针（PTR）加窄常量（I32），结果按 `TypeId::upcast` 恒为 PTR。后端必须
    // 按**结果**宽度编码加法：取窄操作数宽度会发 32 位 ADD 清零地址高半，栈位于
    // 4 GiB 之上时 store/load 立即 SEGV（x86 ADD_RM_R 的 opsize 曾取 modrm.reg =
    // 源槽，实证 mini_c 全量崩溃）。
    Case {
        name: "stack_addr_iadd_offset_store_load",
        ops: &["StackAddr", "Load", "Store", "Iadd"],
        kind: CaseKind::I32(
            |b| {
                let base = b.stack_addr(0);
                let off = b.iconst_i32(-4);
                let slot = b.iadd(base, off);
                let v = b.iconst_i32(42);
                b.store(v, slot);
                b.load(slot, TypeId::I32)
            },
            42,
        ),
    },
    // ── 控制流（Branch/Jump/Block 参数）──
    Case {
        name: "conditional_branch",
        ops: &["Icmp"],
        kind: CaseKind::Block(
            |b| {
                b.create_block_here();
                let c = b.iconst_bool(true);
                let then = b.create_block();
                let els = b.create_block();
                b.branch(c, then, &[], els, &[]);
                b.switch_to_block(then);
                let v = b.iconst_i32(42);
                b.ret(&[v]);
                b.switch_to_block(els);
                let v2 = b.iconst_i32(0);
                b.ret(&[v2]);
            },
            42,
        ),
    },
    Case {
        name: "conditional_branch_else",
        ops: &["Icmp"],
        kind: CaseKind::Block(
            |b| {
                b.create_block_here();
                let c = b.iconst_bool(false);
                let then = b.create_block();
                let els = b.create_block();
                b.branch(c, then, &[], els, &[]);
                b.switch_to_block(then);
                let v = b.iconst_i32(0);
                b.ret(&[v]);
                b.switch_to_block(els);
                let v2 = b.iconst_i32(42);
                b.ret(&[v2]);
            },
            42,
        ),
    },
    Case {
        name: "simple_loop",
        ops: &["Iadd", "Icmp"],
        kind: CaseKind::Block(
            |b| {
                // 1+2+…+10 = 55；block 参数承载循环变量（phi 语义）
                let entry = b.create_block();
                let (loop_h, lp) =
                    b.create_block_with_params(&[(TypeId::I32, "i"), (TypeId::I32, "acc")]);
                let exit = b.create_block();
                b.switch_to_block(entry);
                let i0 = b.iconst_i32(0);
                let a0 = b.iconst_i32(0);
                b.jump(loop_h, &[i0, a0]);
                b.switch_to_block(loop_h);
                let one = b.iconst_i32(1);
                let new_i = b.iadd(lp[0], one);
                let new_acc = b.iadd(lp[1], new_i);
                let ten = b.iconst_i32(10);
                let done = b.icmp(IntCC::SignedGreaterThanOrEqual, new_i, ten);
                b.branch(done, exit, &[], loop_h, &[new_i, new_acc]);
                b.switch_to_block(exit);
                b.ret(&[new_acc]);
            },
            55,
        ),
    },
    Case {
        name: "if_else_chain",
        ops: &["Icmp", "Iadd"],
        kind: CaseKind::Block(
            |b| {
                let entry = b.create_block();
                let then = b.create_block();
                let els = b.create_block();
                let (end, phi) = b.create_block_with_params(&[(TypeId::I32, "phi")]);
                b.switch_to_block(entry);
                let c = b.iconst_bool(true);
                b.branch(c, then, &[], els, &[]);
                b.switch_to_block(then);
                let v1 = b.iconst_i32(40);
                b.jump(end, &[v1]);
                b.switch_to_block(els);
                let v2 = b.iconst_i32(2);
                b.jump(end, &[v2]);
                b.switch_to_block(end);
                let z = b.iconst_i32(0);
                let r = b.iadd(phi[0], z);
                b.ret(&[r]);
            },
            40,
        ),
    },
    // ── P0-9：多 pred 块参数传**动态值**（两 pred 传不同函数参数，
    // 寄存器压力高于常量场景；验证 phi 传参在 regalloc 后仍正确）──
    Case {
        name: "multi_pred_dynamic_phi",
        ops: &["Iadd", "Icmp"],
        kind: CaseKind::Block(
            |b| {
                let entry = b.create_block();
                let then = b.create_block();
                let els = b.create_block();
                let (end, phi) = b.create_block_with_params(&[(TypeId::I64, "phi")]);
                b.switch_to_block(entry);
                // 用动态值做条件（10 > 20 恒假 → 走 else 传 20）
                let ten = b.iconst_i64(10);
                let twenty = b.iconst_i64(20);
                let zero = b.iconst_i64(0);
                let c = b.icmp(IntCC::SignedGreaterThan, ten, twenty);
                b.branch(c, then, &[], els, &[]);
                b.switch_to_block(then);
                b.jump(end, &[ten]);
                b.switch_to_block(els);
                b.jump(end, &[twenty]);
                b.switch_to_block(end);
                let r = b.iadd(phi[0], zero);
                b.ret(&[r]);
            },
            20,
        ),
    },
    // ── 参数（≤4 i64）──
    Case {
        name: "param_one_identity",
        ops: &["Iconst"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[42],
            build: |_b, p| p[0],
            expected: 42,
        },
    },
    Case {
        name: "param_one_add_const",
        ops: &["Iadd"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[40],
            build: |b, p| {
                let c = b.iconst_i64(2);
                b.iadd(p[0], c)
            },
            expected: 42,
        },
    },
    Case {
        name: "param_two_add",
        ops: &["Iadd"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a"), (TypeId::I64, "b")],
            args: &[20, 22],
            build: |b, p| b.iadd(p[0], p[1]),
            expected: 42,
        },
    },
    Case {
        name: "param_two_mul",
        ops: &["Imul"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a"), (TypeId::I64, "b")],
            args: &[6, 7],
            build: |b, p| b.imul(p[0], p[1]),
            expected: 42,
        },
    },
    Case {
        name: "param_two_sub",
        ops: &["Isub"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a"), (TypeId::I64, "b")],
            args: &[84, 42],
            build: |b, p| b.isub(p[0], p[1]),
            expected: 42,
        },
    },
    Case {
        name: "param_two_bitwise",
        ops: &["Bxor"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a"), (TypeId::I64, "b")],
            args: &[63, 21],
            build: |b, p| b.bxor(p[0], p[1]),
            expected: 42,
        },
    },
    Case {
        name: "param_two_div",
        ops: &["Sdiv"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a"), (TypeId::I64, "b")],
            args: &[84, 2],
            build: |b, p| b.sdiv(p[0], p[1]),
            expected: 42,
        },
    },
    Case {
        name: "param_three_add",
        ops: &["Iadd"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a"), (TypeId::I64, "b"), (TypeId::I64, "c")],
            args: &[10, 12, 20],
            build: |b, p| {
                let s = b.iadd(p[0], p[1]);
                b.iadd(s, p[2])
            },
            expected: 42,
        },
    },
    Case {
        name: "param_four_add",
        ops: &["Iadd"],
        kind: CaseKind::Args {
            params: &[
                (TypeId::I64, "a"),
                (TypeId::I64, "b"),
                (TypeId::I64, "c"),
                (TypeId::I64, "d"),
            ],
            args: &[1, 2, 3, 36],
            build: |b, p| {
                let s1 = b.iadd(p[0], p[1]);
                let s2 = b.iadd(s1, p[2]);
                b.iadd(s2, p[3])
            },
            expected: 42,
        },
    },
    // ── 混宽整数二元运算（`TypeId::upcast`：I64 ⊕ I32 → I64）──
    // 机器指令只有一个操作宽度，必须取**结果**宽度（= upcast，两操作数中较宽者）。
    // 取窄操作数宽度会发 32 位运算清零结果高半——实参高半非 0（0x2_0000_0000）
    // 即暴露，与栈地址是否在 4 GiB 之上无关（确定性守卫）。
    // 窄常量在 64 位寄存器中按类型符号扩展（Iconst 语义），故 -4/-1 参与 64 位运算取值正确。
    Case {
        name: "iadd_i64_i32_mixed_width",
        ops: &["Iadd"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[0x2_0000_0000],
            build: |b, p| {
                let off = b.iconst_i32(-4);
                b.iadd(p[0], off)
            },
            expected: 0x1_FFFF_FFFC,
        },
    },
    Case {
        name: "isub_i64_i32_mixed_width",
        ops: &["Isub"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[0x2_0000_0000],
            build: |b, p| {
                let off = b.iconst_i32(4);
                b.isub(p[0], off)
            },
            expected: 0x1_FFFF_FFFC,
        },
    },
    Case {
        name: "bor_i64_i32_mixed_width",
        ops: &["Bor"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[0x2_0000_0000],
            build: |b, p| {
                let m = b.iconst_i32(42);
                b.bor(p[0], m)
            },
            expected: 0x2_0000_002A,
        },
    },
    Case {
        name: "bxor_i64_i32_mixed_width",
        ops: &["Bxor"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[0x2_0000_0000],
            build: |b, p| {
                let m = b.iconst_i32(42);
                b.bxor(p[0], m)
            },
            expected: 0x2_0000_002A,
        },
    },
    Case {
        name: "band_i64_i32_mixed_width",
        ops: &["Band"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[0x2_0000_00FF],
            build: |b, p| {
                let m = b.iconst_i32(-1); // 全 1 掩码：符号扩展后 64 位恒等
                b.band(p[0], m)
            },
            expected: 0x2_0000_00FF,
        },
    },
    // 混宽比较：`icmp` 结果是 BOOL，不带操作数宽度——比较宽度必须取两操作数的
    // 宽者。按较窄者编码只比低半：高半非 0、低半为 0 的指针（0x2_0000_0000）
    // 会与 `iconst_i32(0)` 判等为真，即空指针检查失效。
    Case {
        name: "icmp_eq_i64_i32_mixed_width",
        ops: &["Icmp"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[0x2_0000_0000],
            build: |b, p| {
                let z = b.iconst_i32(0);
                let c = b.icmp(IntCC::Equal, p[0], z);
                b.sextend(c, TypeId::I64)
            },
            expected: 0,
        },
    },
    Case {
        name: "icmp_ne_i64_i32_mixed_width",
        ops: &["Icmp"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[0x2_0000_0000],
            build: |b, p| {
                let z = b.iconst_i32(0);
                let c = b.icmp(IntCC::NotEqual, p[0], z);
                b.sextend(c, TypeId::I64)
            },
            expected: 1,
        },
    },
    Case {
        name: "icmp_slt_i64_i32_mixed_width",
        ops: &["Icmp"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[0x2_0000_0000],
            build: |b, p| {
                let one = b.iconst_i32(1);
                // 0x2_0000_0000 < 1 为假；32 位比较只看低半（0 < 1）会误判为真
                let c = b.icmp(IntCC::SignedLessThan, p[0], one);
                b.sextend(c, TypeId::I64)
            },
            expected: 0,
        },
    },
    // ── 返回值形状 ──
    Case {
        name: "return_negative",
        ops: &["Iconst"],
        kind: CaseKind::I32(|b| b.iconst_i32(-42), -42),
    },
    Case {
        name: "return_zero",
        ops: &["Iconst"],
        kind: CaseKind::I32(|b| b.iconst_i32(0), 0),
    },
    Case {
        name: "return_minus_one",
        ops: &["Iconst"],
        kind: CaseKind::I32(|b| b.iconst_i32(-1), -1),
    },
    Case {
        name: "return_large_positive",
        ops: &["Iconst"],
        kind: CaseKind::I64(|b| b.iconst_i64(3_000_000_000), 3_000_000_000),
    },
    Case {
        name: "return_max_i32",
        ops: &["Iconst"],
        kind: CaseKind::I32(|b| b.iconst_i32(i32::MAX), i32::MAX),
    },
    Case {
        name: "return_min_i32",
        ops: &["Iconst"],
        kind: CaseKind::I32(|b| b.iconst_i32(i32::MIN), i32::MIN),
    },
    // ── 布尔逻辑（经 icmp + band/bor/bnot）──
    Case {
        name: "bool_and",
        ops: &["Icmp", "Band"],
        kind: CaseKind::Bool(
            |b| {
                let t = b.iconst_bool(true);
                let f = b.iconst_bool(false);
                let r = b.band(t, f);
                let one = b.iconst_bool(true);
                b.icmp(IntCC::NotEqual, r, one)
            },
            true,
        ),
    },
    // ── spill 高压（12 常量求和）──
    Case {
        name: "spill_high_pressure",
        ops: &["Iadd", "Iconst"],
        kind: CaseKind::I32(
            |b| {
                let mut acc = b.iconst_i32(0);
                for i in 1..=12i32 {
                    let v = b.iconst_i32(i);
                    acc = b.iadd(acc, v);
                }
                acc
            },
            78,
        ),
    },
    // ── 多返回值（本机 ABI：首值 RAX；当前 runner 只取首值）──
    Case {
        name: "multi_return_first",
        ops: &["Iconst"],
        kind: CaseKind::I32(|b| b.iconst_i32(42), 42),
    },
    // ══════════════════ 迭代 6 续：整数全量（when 谓词接线）══════════════════
    // ── Select(cond, a, b) ──
    Case {
        name: "select_true",
        ops: &["Select", "Iconst"],
        kind: CaseKind::I32(
            |b| {
                let c = b.iconst_bool(true);
                let t = b.iconst_i32(42);
                let f = b.iconst_i32(0);
                b.select(c, t, f)
            },
            42,
        ),
    },
    Case {
        name: "select_false",
        ops: &["Select", "Iconst"],
        kind: CaseKind::I32(
            |b| {
                let c = b.iconst_bool(false);
                let t = b.iconst_i32(42);
                let f = b.iconst_i32(7);
                b.select(c, t, f)
            },
            7,
        ),
    },
    Case {
        name: "select_icmp_cond",
        ops: &["Select", "Icmp"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a"), (TypeId::I64, "b"), (TypeId::I64, "c")],
            args: &[1, 42, 0],
            build: |b, p| {
                let zero = b.iconst_i64(0);
                let c = b.icmp(IntCC::NotEqual, p[0], zero);
                b.select(c, p[1], p[2])
            },
            expected: 42,
        },
    },
    // ── Uextend（when 分派：le 8 / le 16 / default）+ Ireduce ──
    Case {
        name: "uextend_i8_to_i64",
        ops: &["Uextend", "Ireduce"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[0xFFFFFFFFFFFFFF80],
            build: |b, p| {
                let v8 = b.ireduce(p[0], TypeId::I8);
                b.uextend(v8, TypeId::I64)
            },
            expected: 0x80,
        },
    },
    Case {
        name: "uextend_i16_to_i64",
        ops: &["Uextend", "Ireduce"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[0xFFFFFFFFFFFF1234],
            build: |b, p| {
                let v16 = b.ireduce(p[0], TypeId::I16);
                b.uextend(v16, TypeId::I64)
            },
            expected: 0x1234,
        },
    },
    Case {
        name: "ireduce_trunc_i64_to_i32",
        ops: &["Uextend", "Ireduce"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a")],
            args: &[0xFFFFFFFF00001234],
            build: |b, p| {
                let v32 = b.ireduce(p[0], TypeId::I32);
                b.uextend(v32, TypeId::I64)
            },
            expected: 0x1234,
        },
    },
    // ── 旋转（ROL/ROR，CL 隐式计数）──
    Case {
        name: "rotl_i32",
        ops: &["Rotl"],
        kind: CaseKind::I32(
            |b| {
                let v = b.iconst_i32(0x12345678);
                let n = b.iconst_i32(8);
                b.rotl(v, n)
            },
            0x34567812,
        ),
    },
    Case {
        name: "rotr_i32",
        ops: &["Rotr"],
        kind: CaseKind::I32(
            |b| {
                let v = b.iconst_i32(0x12345678);
                let n = b.iconst_i32(8);
                b.rotr(v, n)
            },
            0x78123456,
        ),
    },
    Case {
        name: "rotl_i64",
        ops: &["Rotl"],
        kind: CaseKind::I64(
            |b| {
                let v = b.iconst_i64(0x1122334455667788);
                let n = b.iconst_i64(32);
                b.rotl(v, n)
            },
            0x5566778811223344,
        ),
    },
    // ── 极值（cmp + cmov）──
    Case {
        name: "smin",
        ops: &["Smin"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(-5);
                let c = b.iconst_i32(3);
                b.smin(a, c)
            },
            -5,
        ),
    },
    Case {
        name: "smax",
        ops: &["Smax"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(-5);
                let c = b.iconst_i32(3);
                b.smax(a, c)
            },
            3,
        ),
    },
    Case {
        name: "umin",
        ops: &["Umin"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(-1); // 0xFFFFFFFF（无符号大）
                let c = b.iconst_i32(5);
                b.umin(a, c)
            },
            5,
        ),
    },
    Case {
        name: "umax",
        ops: &["Umax"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(-1);
                let c = b.iconst_i32(5);
                b.umax(a, c)
            },
            -1,
        ),
    },
    Case {
        name: "smin_args",
        ops: &["Smin"],
        kind: CaseKind::Args {
            params: &[(TypeId::I64, "a"), (TypeId::I64, "b")],
            args: &[0xFFFFFFFFFFFFFFD6, 42], // -42, 42
            build: |b, p| b.smin(p[0], p[1]),
            expected: -42,
        },
    },
    // ── Abs（neg + cmovl）──
    Case {
        name: "abs_negative",
        ops: &["Abs"],
        kind: CaseKind::I32(
            |b| {
                let v = b.iconst_i32(-42);
                b.abs(v)
            },
            42,
        ),
    },
    Case {
        name: "abs_positive",
        ops: &["Abs"],
        kind: CaseKind::I32(
            |b| {
                let v = b.iconst_i32(42);
                b.abs(v)
            },
            42,
        ),
    },
    Case {
        name: "abs_min_int",
        ops: &["Abs"],
        kind: CaseKind::I32(
            |b| {
                let v = b.iconst_i32(i32::MIN);
                b.abs(v)
            },
            i32::MIN,
        ),
    },
    // ── Clz/Ctz/Popcnt/Bswap ──
    Case {
        name: "clz_one",
        ops: &["Clz"],
        kind: CaseKind::I64(
            |b| {
                let v = b.iconst_i64(1);
                b.clz(v)
            },
            63,
        ),
    },
    Case {
        name: "clz_msb",
        ops: &["Clz"],
        kind: CaseKind::I64(
            |b| {
                let v = b.iconst_i64(i64::MIN);
                b.clz(v)
            },
            0,
        ),
    },
    Case {
        name: "ctz_msb",
        ops: &["Ctz"],
        kind: CaseKind::I64(
            |b| {
                let v = b.iconst_i64(i64::MIN);
                b.ctz(v)
            },
            63,
        ),
    },
    Case {
        name: "ctz_one",
        ops: &["Ctz"],
        kind: CaseKind::I64(
            |b| {
                let v = b.iconst_i64(1);
                b.ctz(v)
            },
            0,
        ),
    },
    Case {
        name: "popcnt_byte_pattern",
        ops: &["Popcnt"],
        kind: CaseKind::I64(
            |b| {
                let v = b.iconst_i64(0x0F0F0F0F0F0F0F0F);
                b.popcnt(v)
            },
            32,
        ),
    },
    Case {
        name: "bswap_i64",
        ops: &["Bswap"],
        kind: CaseKind::I64(
            |b| {
                let v = b.iconst_i64(0x0000000011223344);
                b.bswap(v)
            },
            0x4433221100000000,
        ),
    },
    // ── 空指针判断 / Freeze ──
    Case {
        name: "is_null_stack",
        ops: &["IsNull", "StackAddr"],
        kind: CaseKind::Bool(
            |b| {
                let addr = b.stack_addr(0);
                b.is_null(addr)
            },
            false,
        ),
    },
    Case {
        name: "is_not_null_stack",
        ops: &["IsNotNull", "StackAddr"],
        kind: CaseKind::Bool(
            |b| {
                let addr = b.stack_addr(0);
                b.is_not_null(addr)
            },
            true,
        ),
    },
    Case {
        name: "freeze",
        ops: &["Freeze"],
        kind: CaseKind::I32(
            |b| {
                let v = b.iconst_i32(42);
                b.freeze(v)
            },
            42,
        ),
    },
    // ── Trap（UD2；编译级验证，不执行）──
    Case {
        name: "trap_compile",
        ops: &["Trap"],
        kind: CaseKind::CompileOnly(|b| {
            // compile_only 已 create_block_here（勿重复建块——空块无终结符）
            let v = b.iconst_i32(0);
            b.trap();
            b.unreachable();
            v
        }),
    },
    // ── Fence（MFENCE；无数据依赖，结果=常量）──
    Case {
        name: "fence_seqcst",
        ops: &["Fence"],
        kind: CaseKind::Block(
            |b| {
                b.create_block_here();
                b.fence(Ordering::SequentiallyConsistent);
                let v = b.iconst_i32(42);
                b.ret(&[v]);
            },
            42,
        ),
    },
    // ── Alloca：栈槽地址 + 存取 ──
    Case {
        name: "alloca_store_load",
        ops: &["Alloca", "Store", "Load", "Iconst"],
        kind: CaseKind::I32(
            |b| {
                let ptr = b.alloca(TypeId::I32, 1);
                let v = b.iconst_i32(42);
                b.store(v, ptr);
                b.load(ptr, TypeId::I32)
            },
            42,
        ),
    },
    // ── 溢出 6 条（{out2} 多结果接线；flag 用例返回 BOOL）──
    Case {
        name: "sadd_overflow_ok_value",
        ops: &["SaddOverflow"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(30);
                let c = b.iconst_i32(12);
                let (v, _f) = b.sadd_overflow(a, c);
                v
            },
            42,
        ),
    },
    Case {
        name: "sadd_overflow_flag",
        ops: &["SaddOverflow"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(i32::MAX);
                let c = b.iconst_i32(1);
                let (_v, f) = b.sadd_overflow(a, c);
                f
            },
            true,
        ),
    },
    Case {
        name: "uadd_overflow_ok_value",
        ops: &["UaddOverflow"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(20);
                let c = b.iconst_i32(22);
                let (v, _f) = b.uadd_overflow(a, c);
                v
            },
            42,
        ),
    },
    Case {
        name: "uadd_overflow_flag",
        ops: &["UaddOverflow"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(-1);
                let c = b.iconst_i32(1);
                let (_v, f) = b.uadd_overflow(a, c);
                f
            },
            true,
        ),
    },
    Case {
        name: "ssub_overflow_ok_value",
        ops: &["SsubOverflow"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(-42);
                let c = b.iconst_i32(0);
                let (v, _f) = b.ssub_overflow(a, c);
                v
            },
            -42,
        ),
    },
    Case {
        name: "ssub_overflow_flag",
        ops: &["SsubOverflow"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(i32::MIN);
                let c = b.iconst_i32(1);
                let (_v, f) = b.ssub_overflow(a, c);
                f
            },
            true,
        ),
    },
    Case {
        name: "usub_overflow_flag",
        ops: &["UsubOverflow"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(0);
                let c = b.iconst_i32(1);
                let (_v, f) = b.usub_overflow(a, c);
                f
            },
            true,
        ),
    },
    Case {
        name: "smul_overflow_ok_value",
        ops: &["SmulOverflow"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(6);
                let c = b.iconst_i32(7);
                let (v, _f) = b.smul_overflow(a, c);
                v
            },
            42,
        ),
    },
    Case {
        name: "smul_overflow_flag",
        ops: &["SmulOverflow"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(i32::MAX);
                let c = b.iconst_i32(2);
                let (_v, f) = b.smul_overflow(a, c);
                f
            },
            true,
        ),
    },
    Case {
        name: "umul_overflow_ok_value",
        ops: &["UmulOverflow"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(6);
                let c = b.iconst_i32(7);
                let (v, _f) = b.umul_overflow(a, c);
                v
            },
            42,
        ),
    },
    Case {
        name: "umul_overflow_flag",
        ops: &["UmulOverflow"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.iconst_i32(-1);
                let c = b.iconst_i32(2);
                let (_v, f) = b.umul_overflow(a, c);
                f
            },
            true,
        ),
    },
    // ── 饱和（SaddSat i32/i64、UaddSat、UsubSat）──
    Case {
        name: "sadd_sat_i32_max",
        ops: &["SaddSat"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(i32::MAX);
                let c = b.iconst_i32(1);
                b.sadd_sat(a, c)
            },
            i32::MAX,
        ),
    },
    Case {
        name: "sadd_sat_i32_min",
        ops: &["SaddSat"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(i32::MIN);
                let c = b.iconst_i32(-1);
                b.sadd_sat(a, c)
            },
            i32::MIN,
        ),
    },
    Case {
        name: "sadd_sat_i32_ok",
        ops: &["SaddSat"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(20);
                let c = b.iconst_i32(22);
                b.sadd_sat(a, c)
            },
            42,
        ),
    },
    Case {
        name: "sadd_sat_i64_max",
        ops: &["SaddSat"],
        kind: CaseKind::I64(
            |b| {
                let a = b.iconst_i64(i64::MAX);
                let c = b.iconst_i64(1);
                b.sadd_sat(a, c)
            },
            i64::MAX,
        ),
    },
    Case {
        name: "uadd_sat_max",
        ops: &["UaddSat"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(-1);
                let c = b.iconst_i32(1);
                b.uadd_sat(a, c)
            },
            -1,
        ),
    },
    Case {
        name: "uadd_sat_ok",
        ops: &["UaddSat"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(20);
                let c = b.iconst_i32(22);
                b.uadd_sat(a, c)
            },
            42,
        ),
    },
    Case {
        name: "usub_sat_zero",
        ops: &["UsubSat"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(0);
                let c = b.iconst_i32(1);
                b.usub_sat(a, c)
            },
            0,
        ),
    },
    Case {
        name: "usub_sat_ok",
        ops: &["UsubSat"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(50);
                let c = b.iconst_i32(8);
                b.usub_sat(a, c)
            },
            42,
        ),
    },
    // ── Bitreverse（Hacker's Delight 64 位）──
    Case {
        name: "bitreverse_one",
        ops: &["Bitreverse"],
        kind: CaseKind::I64(
            |b| {
                let v = b.iconst_i64(1);
                b.bitreverse(v)
            },
            i64::MIN,
        ),
    },
    Case {
        name: "bitreverse_msb",
        ops: &["Bitreverse"],
        kind: CaseKind::I64(
            |b| {
                let v = b.iconst_i64(i64::MIN);
                b.bitreverse(v)
            },
            1,
        ),
    },
    // ══════════════════ 迭代 7（Phase 4）：标量浮点 ══════════════════
    // ── 算术（F64；Fconst 位模式常量）──
    Case {
        name: "fconst_only",
        ops: &["Fconst"],
        kind: CaseKind::F64(|b| b.fconst(1.5f64.to_bits(), TypeId::F64), 1.5),
    },
    Case {
        name: "fadd_const",
        ops: &["Fadd", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst(1.5f64.to_bits(), TypeId::F64);
                let c = b.fconst(2.25f64.to_bits(), TypeId::F64);
                b.fadd(a, c)
            },
            3.75,
        ),
    },
    Case {
        name: "fsub_const",
        ops: &["Fsub", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst(5.0f64.to_bits(), TypeId::F64);
                let c = b.fconst(2.5f64.to_bits(), TypeId::F64);
                b.fsub(a, c)
            },
            2.5,
        ),
    },
    Case {
        name: "fmul_const",
        ops: &["Fmul", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst(3.0f64.to_bits(), TypeId::F64);
                let c = b.fconst(2.5f64.to_bits(), TypeId::F64);
                b.fmul(a, c)
            },
            7.5,
        ),
    },
    Case {
        name: "fdiv_const",
        ops: &["Fdiv", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst(7.5f64.to_bits(), TypeId::F64);
                let c = b.fconst(2.5f64.to_bits(), TypeId::F64);
                b.fdiv(a, c)
            },
            3.0,
        ),
    },
    Case {
        name: "fadd_f32",
        ops: &["Fadd", "Fpext", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst(1.5f32.to_bits() as u64, TypeId::F32);
                let c = b.fconst(2.25f32.to_bits() as u64, TypeId::F32);
                let s = b.fadd(a, c);
                b.fpext(s, TypeId::F64)
            },
            3.75,
        ),
    },
    // ── Fneg / Fabs / Fsqrt ──
    Case {
        name: "fneg_const",
        ops: &["Fneg", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let v = b.fconst(2.5f64.to_bits(), TypeId::F64);
                b.fneg(v)
            },
            -2.5,
        ),
    },
    Case {
        name: "fabs_negative",
        ops: &["Fabs", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let v = b.fconst((-3.5f64).to_bits(), TypeId::F64);
                b.fabs(v)
            },
            3.5,
        ),
    },
    Case {
        name: "fsqrt_const",
        ops: &["Fsqrt", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let v = b.fconst(16.0f64.to_bits(), TypeId::F64);
                b.fsqrt(v)
            },
            4.0,
        ),
    },
    // ── Fmin / Fmax ──
    Case {
        name: "fmin_const",
        ops: &["Fmin", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst(1.5f64.to_bits(), TypeId::F64);
                let c = b.fconst(2.5f64.to_bits(), TypeId::F64);
                b.fmin(a, c)
            },
            1.5,
        ),
    },
    Case {
        name: "fmax_const",
        ops: &["Fmax", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst(1.5f64.to_bits(), TypeId::F64);
                let c = b.fconst(2.5f64.to_bits(), TypeId::F64);
                b.fmax(a, c)
            },
            2.5,
        ),
    },
    // ── 舍入 ──
    Case {
        name: "ffloor_const",
        ops: &["Ffloor", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let v = b.fconst(2.7f64.to_bits(), TypeId::F64);
                b.ffloor(v)
            },
            2.0,
        ),
    },
    Case {
        name: "fceil_const",
        ops: &["Fceil", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let v = b.fconst(2.3f64.to_bits(), TypeId::F64);
                b.fceil(v)
            },
            3.0,
        ),
    },
    Case {
        name: "ftrunc_const",
        ops: &["Ftrunc", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let v = b.fconst(2.7f64.to_bits(), TypeId::F64);
                b.ftrunc(v)
            },
            2.0,
        ),
    },
    Case {
        name: "ftrunc_negative",
        ops: &["Ftrunc", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let v = b.fconst((-2.7f64).to_bits(), TypeId::F64);
                b.ftrunc(v)
            },
            -2.0,
        ),
    },
    Case {
        name: "fround_half_even",
        ops: &["Fround", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let v = b.fconst(2.5f64.to_bits(), TypeId::F64);
                b.fround(v)
            },
            2.0,
        ),
    },
    // ── Fpext / Fptrunc ──
    Case {
        name: "fpext_f32_to_f64",
        ops: &["Fpext", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let v = b.fconst(2.5f32.to_bits() as u64, TypeId::F32);
                b.fpext(v, TypeId::F64)
            },
            2.5,
        ),
    },
    Case {
        name: "fptrunc_f64_to_f32",
        ops: &["Fptrunc", "Fpext", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let v = b.fconst(2.5f64.to_bits(), TypeId::F64);
                let s = b.fptrunc(v, TypeId::F32);
                b.fpext(s, TypeId::F64)
            },
            2.5,
        ),
    },
    // ── Fcmp（返回 BOOL）──
    Case {
        name: "fcmp_eq",
        ops: &["Fcmp", "Fconst"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.fconst(1.5f64.to_bits(), TypeId::F64);
                let c = b.fconst(1.5f64.to_bits(), TypeId::F64);
                b.fcmp(FloatCC::Equal, a, c)
            },
            true,
        ),
    },
    Case {
        name: "fcmp_lt",
        ops: &["Fcmp", "Fconst"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.fconst(1.5f64.to_bits(), TypeId::F64);
                let c = b.fconst(2.5f64.to_bits(), TypeId::F64);
                b.fcmp(FloatCC::LessThan, a, c)
            },
            true,
        ),
    },
    Case {
        name: "fcmp_gt_false",
        ops: &["Fcmp", "Fconst"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.fconst(1.5f64.to_bits(), TypeId::F64);
                let c = b.fconst(2.5f64.to_bits(), TypeId::F64);
                b.fcmp(FloatCC::GreaterThan, a, c)
            },
            false,
        ),
    },
    Case {
        name: "fcmp_ne",
        ops: &["Fcmp", "Fconst"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.fconst(1.5f64.to_bits(), TypeId::F64);
                let c = b.fconst(2.5f64.to_bits(), TypeId::F64);
                b.fcmp(FloatCC::NotEqual, a, c)
            },
            true,
        ),
    },
    Case {
        name: "fcmp_ule",
        ops: &["Fcmp", "Fconst"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.fconst(1.5f64.to_bits(), TypeId::F64);
                let c = b.fconst(2.5f64.to_bits(), TypeId::F64);
                b.fcmp(FloatCC::LessThanOrEqual, a, c)
            },
            true,
        ),
    },
    Case {
        name: "fcmp_oge_equal",
        ops: &["Fcmp", "Fconst"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.fconst(2.5f64.to_bits(), TypeId::F64);
                let c = b.fconst(2.5f64.to_bits(), TypeId::F64);
                b.fcmp(FloatCC::GreaterThanOrEqual, a, c)
            },
            true,
        ),
    },
    Case {
        name: "fcmp_ordered_nan",
        ops: &["Fcmp", "Fconst"],
        kind: CaseKind::Bool(
            |b| {
                let nan = b.fconst(f64::NAN.to_bits(), TypeId::F64);
                let one = b.fconst(1.0f64.to_bits(), TypeId::F64);
                b.fcmp(FloatCC::Ordered, nan, one)
            },
            false,
        ),
    },
    Case {
        name: "fcmp_unordered_nan",
        ops: &["Fcmp", "Fconst"],
        kind: CaseKind::Bool(
            |b| {
                let nan = b.fconst(f64::NAN.to_bits(), TypeId::F64);
                let one = b.fconst(1.0f64.to_bits(), TypeId::F64);
                b.fcmp(FloatCC::Unordered, nan, one)
            },
            true,
        ),
    },
    Case {
        name: "fcmp_f32_lt",
        ops: &["Fcmp", "Fconst"],
        kind: CaseKind::Bool(
            |b| {
                let a = b.fconst(1.5f32.to_bits() as u64, TypeId::F32);
                let c = b.fconst(2.5f32.to_bits() as u64, TypeId::F32);
                b.fcmp(FloatCC::LessThan, a, c)
            },
            true,
        ),
    },
    // ── Fload / Fstore（alloca 槽 + 浮点内存标量）──
    Case {
        name: "fload_fstore",
        ops: &["Fload", "Fstore", "Alloca", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let ptr = b.alloca(TypeId::F64, 1);
                let v = b.fconst(3.25f64.to_bits(), TypeId::F64);
                b.fstore(v, ptr);
                b.fload(ptr, TypeId::F64)
            },
            3.25,
        ),
    },
    Case {
        name: "fload_fstore_f32",
        ops: &["Fload", "Fstore", "Alloca", "Fpext", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let ptr = b.alloca(TypeId::F32, 1);
                let v = b.fconst(3.25f32.to_bits() as u64, TypeId::F32);
                b.fstore(v, ptr);
                let s = b.fload(ptr, TypeId::F32);
                b.fpext(s, TypeId::F64)
            },
            3.25,
        ),
    },
    // ── 转换 ──
    Case {
        name: "sitofp_i32_f64",
        ops: &["Sitofp"],
        kind: CaseKind::F64(
            |b| {
                let v = b.iconst_i32(42);
                b.sitofp(v, TypeId::F64)
            },
            42.0,
        ),
    },
    Case {
        name: "sitofp_i64_f64_neg",
        ops: &["Sitofp"],
        kind: CaseKind::F64(
            |b| {
                let v = b.iconst_i64(-42);
                b.sitofp(v, TypeId::F64)
            },
            -42.0,
        ),
    },
    Case {
        name: "sitofp_i32_f32",
        ops: &["Sitofp", "Fpext"],
        kind: CaseKind::F64(
            |b| {
                let v = b.iconst_i32(42);
                let s = b.sitofp(v, TypeId::F32);
                b.fpext(s, TypeId::F64)
            },
            42.0,
        ),
    },
    Case {
        name: "fptosi_f64_i32",
        ops: &["Fptosi", "Fconst"],
        kind: CaseKind::I32(
            |b| {
                let v = b.fconst(42.7f64.to_bits(), TypeId::F64);
                b.fptosi(v, TypeId::I32)
            },
            42,
        ),
    },
    Case {
        name: "fptosi_f64_i64_neg",
        ops: &["Fptosi", "Fconst"],
        kind: CaseKind::I64(
            |b| {
                let v = b.fconst((-42.7f64).to_bits(), TypeId::F64);
                b.fptosi(v, TypeId::I64)
            },
            -42,
        ),
    },
    Case {
        name: "fptoui_f64_u32",
        ops: &["Fptoui", "Fconst"],
        kind: CaseKind::I32(
            |b| {
                let v = b.fconst(42.7f64.to_bits(), TypeId::F64);
                b.fptoui(v, TypeId::I32)
            },
            42,
        ),
    },
    Case {
        name: "uitofp_u32_f64",
        ops: &["Uitofp"],
        kind: CaseKind::F64(
            |b| {
                let v = b.iconst_i32(42);
                b.uitofp(v, TypeId::F64)
            },
            42.0,
        ),
    },
    // ── Fma / Fcopysign ──
    Case {
        name: "fma_const",
        ops: &["Fma", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst(2.0f64.to_bits(), TypeId::F64);
                let c = b.fconst(3.0f64.to_bits(), TypeId::F64);
                let d = b.fconst(1.0f64.to_bits(), TypeId::F64);
                b.fma(a, c, d)
            },
            7.0,
        ),
    },
    // S6 树型模式锚：Fadd(Fmul(a,b), c) → mul+add 融合（Fmul 单 use、同块）。
    // 验证 [[pattern]] 预扫命中 + consumed 跳过 + lower_pattern 分发。
    Case {
        name: "fma_fadd_fmul_pattern_f64",
        ops: &["Fadd", "Fmul", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst(2.0f64.to_bits(), TypeId::F64);
                let c = b.fconst(3.0f64.to_bits(), TypeId::F64);
                let d = b.fconst(4.0f64.to_bits(), TypeId::F64);
                let t = b.fmul(a, c);
                b.fadd(t, d)
            },
            10.0,
        ),
    },
    Case {
        name: "fma_fadd_fmul_pattern_f32",
        ops: &["Fadd", "Fmul", "Fpext", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst(2.0f32.to_bits() as u64, TypeId::F32);
                let c = b.fconst(3.0f32.to_bits() as u64, TypeId::F32);
                let d = b.fconst(4.0f32.to_bits() as u64, TypeId::F32);
                let t = b.fmul(a, c);
                let s = b.fadd(t, d);
                b.fpext(s, TypeId::F64)
            },
            10.0,
        ),
    },
    Case {
        name: "fcopysign_neg_to_pos",
        ops: &["Fcopysign", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst((-3.5f64).to_bits(), TypeId::F64);
                let c = b.fconst(2.0f64.to_bits(), TypeId::F64);
                b.fcopysign(a, c)
            },
            3.5,
        ),
    },
    Case {
        name: "fcopysign_pos_to_neg",
        ops: &["Fcopysign", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst(3.5f64.to_bits(), TypeId::F64);
                let c = b.fconst((-2.0f64).to_bits(), TypeId::F64);
                b.fcopysign(a, c)
            },
            -3.5,
        ),
    },
    // ── 浮点参数（XMM0-3 收参 + 浮点运算 + 转整数返回）──
    Case {
        name: "float_args_two",
        ops: &["Fadd", "Fptosi"],
        kind: CaseKind::F64Args {
            params: &[(TypeId::F64, "a"), (TypeId::F64, "b")],
            args: &[1.5f64.to_bits(), 2.25f64.to_bits()],
            build: |b, p| {
                let s = b.fadd(p[0], p[1]);
                b.fptosi(s, TypeId::I64)
            },
            expected: 3,
        },
    },
    Case {
        name: "float_args_four_xmm3",
        ops: &["Fadd", "Fptosi"],
        kind: CaseKind::F64Args {
            params: &[
                (TypeId::F64, "a"),
                (TypeId::F64, "b"),
                (TypeId::F64, "c"),
                (TypeId::F64, "d"),
            ],
            args: &[
                1.0f64.to_bits(),
                2.0f64.to_bits(),
                3.0f64.to_bits(),
                4.0f64.to_bits(),
            ],
            build: |b, p| {
                let s1 = b.fadd(p[0], p[1]);
                let s2 = b.fadd(p[2], p[3]);
                let s = b.fadd(s1, s2);
                b.fptosi(s, TypeId::I64)
            },
            expected: 10,
        },
    },
    // ══════════════════ 迭代 8（Phase 5）：跨函数 Call ══════════════════
    Case {
        name: "call_cross_function",
        ops: &["Call", "Iadd"],
        kind: CaseKind::Module(
            |m| {
                // callee: (i64, i64) -> i64 { a + b }
                let sig_c = FunctionSignature::new(
                    &[(TypeId::I64, "a"), (TypeId::I64, "b")],
                    &[TypeId::I64],
                );
                let mut bc = FunctionBuilder::new("callee", TypeContext::new(), sig_c);
                let (blk, p) =
                    bc.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
                bc.switch_to_block(blk);
                let s = bc.iadd(p[0], p[1]);
                bc.ret(&[s]);
                let callee_ref = m.add_function(bc.finish().expect("callee"));
                // main: () -> i64 { callee(20, 22) }
                let sig_m = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut bm = FunctionBuilder::new("main", TypeContext::new(), sig_m);
                bm.create_block_here();
                let a = bm.iconst_i64(20);
                let b2 = bm.iconst_i64(22);
                let r = bm.call(callee_ref, &[a, b2], &[TypeId::I64]);
                bm.ret(&r);
                m.add_function(bm.finish().expect("main"))
            },
            42,
        ),
    },
    Case {
        name: "call_recursive_fib",
        ops: &["Call", "Iadd", "Icmp", "Module"],
        kind: CaseKind::Module(
            |m| {
                // 递归 fib：先 add 占位拿 FuncRef，再 replace 真实体（自调用）。
                // fib: (i32 n) -> i32 { if n<=1 { n } else { fib(n-1)+fib(n-2) } }
                let sig_f = FunctionSignature::new(&[(TypeId::I32, "n")], &[TypeId::I32]);
                let mut placeholder =
                    FunctionBuilder::new("fib", TypeContext::new(), sig_f.clone());
                placeholder.create_block_here();
                let z0 = placeholder.iconst_i32(0);
                placeholder.ret(&[z0]);
                let fib_ref = m.add_function(placeholder.finish().expect("placeholder"));

                let mut bf = FunctionBuilder::new("fib", TypeContext::new(), sig_f);
                let (blk, p) = bf.create_block_with_params(&[(TypeId::I32, "n")]);
                bf.switch_to_block(blk);
                let one = bf.iconst_i32(1);
                let le = bf.icmp(IntCC::SignedLessThanOrEqual, p[0], one);
                let then_blk = bf.create_block();
                let else_blk = bf.create_block();
                bf.branch(le, then_blk, &[], else_blk, &[]);
                // then: return n
                bf.switch_to_block(then_blk);
                bf.ret(&[p[0]]);
                // else: fib(n-1) + fib(n-2)
                bf.switch_to_block(else_blk);
                let one2 = bf.iconst_i32(1);
                let nm1 = bf.isub(p[0], one2);
                let r1 = bf.call(fib_ref, &[nm1], &[TypeId::I32]);
                let two = bf.iconst_i32(2);
                let nm2 = bf.isub(p[0], two);
                let r2 = bf.call(fib_ref, &[nm2], &[TypeId::I32]);
                let sum = bf.iadd(r1[0], r2[0]);
                bf.ret(&[sum]);
                m.replace_function(fib_ref, bf.finish().expect("fib"));

                // main: () -> i32 { fib(5) }
                let sig_m = FunctionSignature::new(&[], &[TypeId::I32]);
                let mut bm = FunctionBuilder::new("main", TypeContext::new(), sig_m);
                bm.create_block_here();
                let five = bm.iconst_i32(5);
                let r = bm.call(fib_ref, &[five], &[TypeId::I32]);
                bm.ret(&r);
                m.add_function(bm.finish().expect("main"))
            },
            5,
        ),
    },
    Case {
        name: "call_recursive_fib_slot",
        ops: &[
            "Call",
            "Iadd",
            "Icmp",
            "Store",
            "Load",
            "StackAddr",
            "Module",
        ],
        kind: CaseKind::Module(
            |m| {
                // 模拟 mini_c 的 fib：参数经栈槽 + 双递归。
                // fib: (i32 n) -> i32 { if n<=1 { n } else { fib(n-1)+fib(n-2) } }
                let sig_f = FunctionSignature::new(&[(TypeId::I32, "n")], &[TypeId::I32]);
                let mut placeholder =
                    FunctionBuilder::new("fib", TypeContext::new(), sig_f.clone());
                placeholder.create_block_here();
                let z0 = placeholder.iconst_i32(0);
                placeholder.ret(&[z0]);
                let fib_ref = m.add_function(placeholder.finish().expect("placeholder"));

                let mut bf = FunctionBuilder::new("fib", TypeContext::new(), sig_f);
                let (blk, p) = bf.create_block_with_params(&[(TypeId::I32, "n")]);
                bf.switch_to_block(blk);
                // n 存栈槽
                let base = bf.stack_addr(0);
                let off = bf.iconst_i32(-4);
                let slot = bf.iadd(base, off);
                bf.store(p[0], slot);
                // if n <= 1
                let one = bf.iconst_i32(1);
                let le = bf.icmp(IntCC::SignedLessThanOrEqual, p[0], one);
                let then_blk = bf.create_block();
                let else_blk = bf.create_block();
                bf.branch(le, then_blk, &[], else_blk, &[]);
                bf.switch_to_block(then_blk);
                bf.ret(&[p[0]]);
                bf.switch_to_block(else_blk);
                // n-1
                let n = bf.load(slot, TypeId::I32);
                let one2 = bf.iconst_i32(1);
                let nm1 = bf.isub(n, one2);
                let r1 = bf.call(fib_ref, &[nm1], &[TypeId::I32]);
                // n-2
                let n2 = bf.load(slot, TypeId::I32);
                let two = bf.iconst_i32(2);
                let nm2 = bf.isub(n2, two);
                let r2 = bf.call(fib_ref, &[nm2], &[TypeId::I32]);
                let sum = bf.iadd(r1[0], r2[0]);
                bf.ret(&[sum]);
                m.replace_function(fib_ref, bf.finish().expect("fib"));

                // main: () -> i32 { fib(3) }
                let sig_m = FunctionSignature::new(&[], &[TypeId::I32]);
                let mut bm = FunctionBuilder::new("main", TypeContext::new(), sig_m);
                bm.create_block_here();
                let three = bm.iconst_i32(3);
                let r = bm.call(fib_ref, &[three], &[TypeId::I32]);
                bm.ret(&r);
                m.add_function(bm.finish().expect("main"))
            },
            2,
        ),
    },
    Case {
        name: "call_chain_three_functions",
        ops: &["Call", "Iadd", "Imul"],
        kind: CaseKind::Module(
            |m| {
                // add2: i64 -> i64 { x + 2 }
                let sig_a = FunctionSignature::new(&[(TypeId::I64, "x")], &[TypeId::I64]);
                let mut ba = FunctionBuilder::new("add2", TypeContext::new(), sig_a);
                let (blk, p) = ba.create_block_with_params(&[(TypeId::I64, "x")]);
                ba.switch_to_block(blk);
                let two = ba.iconst_i64(2);
                let s = ba.iadd(p[0], two);
                ba.ret(&[s]);
                let add2_ref = m.add_function(ba.finish().expect("add2"));
                // mul3: i64 -> i64 { x * 3 }
                let sig_m = FunctionSignature::new(&[(TypeId::I64, "x")], &[TypeId::I64]);
                let mut bm = FunctionBuilder::new("mul3", TypeContext::new(), sig_m);
                let (blk2, p2) = bm.create_block_with_params(&[(TypeId::I64, "x")]);
                bm.switch_to_block(blk2);
                let three = bm.iconst_i64(3);
                let s2 = bm.imul(p2[0], three);
                bm.ret(&[s2]);
                let mul3_ref = m.add_function(bm.finish().expect("mul3"));
                // main: () -> i64 { add2(mul3(20)) } = 62
                let sig_main = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut bmain = FunctionBuilder::new("main", TypeContext::new(), sig_main);
                bmain.create_block_here();
                let twenty = bmain.iconst_i64(20);
                let r1 = bmain.call(mul3_ref, &[twenty], &[TypeId::I64]);
                let r2 = bmain.call(add2_ref, &r1, &[TypeId::I64]);
                bmain.ret(&r2);
                m.add_function(bmain.finish().expect("main"))
            },
            62,
        ),
    },
    Case {
        name: "call_float_roundtrip",
        ops: &["Call", "Fadd", "Fptosi", "Fconst"],
        kind: CaseKind::Module(
            |m| {
                // callee: (f64, f64) -> f64 { a + b }
                let sig_c = FunctionSignature::new(
                    &[(TypeId::F64, "a"), (TypeId::F64, "b")],
                    &[TypeId::F64],
                );
                let mut bc = FunctionBuilder::new("fadd_callee", TypeContext::new(), sig_c);
                let (blk, p) =
                    bc.create_block_with_params(&[(TypeId::F64, "a"), (TypeId::F64, "b")]);
                bc.switch_to_block(blk);
                let s = bc.fadd(p[0], p[1]);
                bc.ret(&[s]);
                let callee_ref = m.add_function(bc.finish().expect("callee"));
                // main: () -> i64 { fptosi(callee(1.5, 2.25), I64) } = 3
                let sig_main = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut bmain = FunctionBuilder::new("main", TypeContext::new(), sig_main);
                bmain.create_block_here();
                let a = bmain.fconst(1.5f64.to_bits(), TypeId::F64);
                let b2 = bmain.fconst(2.25f64.to_bits(), TypeId::F64);
                let r = bmain.call(callee_ref, &[a, b2], &[TypeId::F64]);
                let i = bmain.fptosi(r[0], TypeId::I64);
                bmain.ret(&[i]);
                m.add_function(bmain.finish().expect("main"))
            },
            3,
        ),
    },
    Case {
        name: "call_indirect_external",
        ops: &["CallIndirect"],
        kind: CaseKind::Module(
            |m| {
                // main: () -> i64 { call_indirect(ext_add42, 20) } = 62
                let addr = EXT_ADD42 as *const () as i64;
                let sig_main = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut bmain = FunctionBuilder::new("main", TypeContext::new(), sig_main);
                bmain.create_block_here();
                let ptr = bmain.iconst_i64(addr);
                let arg = bmain.iconst_i64(20);
                let r = bmain.call_indirect(ptr, &[arg], &[TypeId::I64]);
                bmain.ret(&r);
                m.add_function(bmain.finish().expect("main"))
            },
            62,
        ),
    },
    // ══════════════════ 迭代 9（Phase 6）：向量 V64/V128/V256 ══════════════════
    // 用例返回标量：向量运算 → vextract lane → fpext/fptosi。
    Case {
        name: "vadd_f32_lane0",
        ops: &["Vadd", "Vconst", "Vextract", "Fpext"],
        kind: CaseKind::F64(
            |b| {
                let a = b.vconst(vec![1.0f32, 2.0, 3.0, 4.0]);
                let c = b.vconst(vec![10.0f32, 20.0, 30.0, 40.0]);
                let s = b.vadd(a, c);
                let idx = b.iconst_i32(0);
                let lane = b.vextract(s, idx);
                b.fpext(lane, TypeId::F64)
            },
            11.0,
        ),
    },
    Case {
        name: "vsub_f32_lane2",
        ops: &["Vsub", "Vconst", "Vextract", "Fpext"],
        kind: CaseKind::F64(
            |b| {
                let a = b.vconst(vec![1.0f32, 2.0, 3.0, 4.0]);
                let c = b.vconst(vec![10.0f32, 20.0, 30.0, 40.0]);
                let s = b.vsub(a, c);
                let idx = b.iconst_i32(2);
                let lane = b.vextract(s, idx);
                b.fpext(lane, TypeId::F64)
            },
            -27.0,
        ),
    },
    Case {
        name: "vmul_f32_lane1",
        ops: &["Vmul", "Vconst", "Vextract", "Fpext"],
        kind: CaseKind::F64(
            |b| {
                let a = b.vconst(vec![1.0f32, 2.0, 3.0, 4.0]);
                let c = b.vconst(vec![10.0f32, 20.0, 30.0, 40.0]);
                let s = b.vmul(a, c);
                let idx = b.iconst_i32(1);
                let lane = b.vextract(s, idx);
                b.fpext(lane, TypeId::F64)
            },
            40.0,
        ),
    },
    Case {
        name: "vdiv_f32_lane3",
        ops: &["Vdiv", "Vconst", "Vextract", "Fpext"],
        kind: CaseKind::F64(
            |b| {
                let a = b.vconst(vec![1.0f32, 2.0, 3.0, 4.0]);
                let c = b.vconst(vec![10.0f32, 20.0, 30.0, 40.0]);
                let s = b.vdiv(a, c);
                let idx = b.iconst_i32(3);
                let lane = b.vextract(s, idx);
                b.fpext(lane, TypeId::F64)
            },
            0.1,
        ),
    },
    Case {
        name: "vadd_i32_lane0",
        ops: &["Vadd", "Vconst", "Vextract"],
        kind: CaseKind::I64(
            |b| {
                let a = b.vconst(vec![1i32, 2, 3, 4]);
                let c = b.vconst(vec![10i32, 20, 30, 40]);
                let s = b.vadd(a, c);
                let idx = b.iconst_i32(0);
                let lane = b.vextract(s, idx);
                b.uextend(lane, TypeId::I64)
            },
            11,
        ),
    },
    Case {
        name: "vneg_f32_lane0",
        ops: &["Vneg", "Vconst", "Vextract", "Fpext"],
        kind: CaseKind::F64(
            |b| {
                let a = b.vconst(vec![1.0f32, 2.0, 3.0, 4.0]);
                let s = b.vneg(a);
                let idx = b.iconst_i32(0);
                let lane = b.vextract(s, idx);
                b.fpext(lane, TypeId::F64)
            },
            -1.0,
        ),
    },
    Case {
        name: "vabs_f32_lane0",
        ops: &["Vabs", "Vconst", "Vextract", "Fpext"],
        kind: CaseKind::F64(
            |b| {
                let a = b.vconst(vec![-1.5f32, 2.0, 3.0, 4.0]);
                let s = b.vabs(a);
                let idx = b.iconst_i32(0);
                let lane = b.vextract(s, idx);
                b.fpext(lane, TypeId::F64)
            },
            1.5,
        ),
    },
    Case {
        name: "vbroadcast_f32_lane2",
        ops: &["Vbroadcast", "Vextract", "Fpext", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let vt = b.type_ctx().vector_ty(TypeId::F32, 4);
                let e = b.fconst(5.0f32.to_bits() as u64, TypeId::F32);
                let v = b.vbroadcast(e, vt);
                let idx = b.iconst_i32(2);
                let lane = b.vextract(v, idx);
                b.fpext(lane, TypeId::F64)
            },
            5.0,
        ),
    },
    Case {
        name: "vextract_i32_lane1",
        ops: &["Vconst", "Vextract"],
        kind: CaseKind::I64(
            |b| {
                let a = b.vconst(vec![1i32, 2, 3, 4]);
                let idx = b.iconst_i32(1);
                let lane = b.vextract(a, idx);
                b.uextend(lane, TypeId::I64)
            },
            2,
        ),
    },
    Case {
        name: "vinsert_f32_lane0",
        ops: &["Vinsert", "Vconst", "Vextract", "Fpext", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.vconst(vec![1.0f32, 2.0, 3.0, 4.0]);
                let e = b.fconst(99.0f32.to_bits() as u64, TypeId::F32);
                let idx = b.iconst_i32(0);
                let v = b.vinsert(a, e, idx);
                let lidx = b.iconst_i32(0);
                let lane = b.vextract(v, lidx);
                b.fpext(lane, TypeId::F64)
            },
            99.0,
        ),
    },
    Case {
        name: "vbitcast_f32_to_i32",
        ops: &["Vbitcast", "Vconst", "Vextract"],
        kind: CaseKind::I64(
            |b| {
                let a = b.vconst(vec![1.0f32, 2.0, 3.0, 4.0]);
                let vt = b.type_ctx().vector_ty(TypeId::I32, 4);
                let v = b.vbitcast(a, vt);
                let idx = b.iconst_i32(0);
                let lane = b.vextract(v, idx);
                b.uextend(lane, TypeId::I64)
            },
            0x3F800000,
        ),
    },
    Case {
        name: "shuffle_f32_lane2",
        ops: &["ShuffleVector", "Vconst", "Vextract", "Fpext"],
        kind: CaseKind::F64(
            |b| {
                // SHUFPS 固定源：lane0/1 ← a、lane2/3 ← b。mask [0,1,4,5]
                // → [a0, a1, b0, b1] = [1, 2, 5, 6]
                let a = b.vconst(vec![1.0f32, 2.0, 3.0, 4.0]);
                let c = b.vconst(vec![5.0f32, 6.0, 7.0, 8.0]);
                let s = b.shuffle_vector(a, c, &[0, 1, 4, 5]);
                let idx = b.iconst_i32(2);
                let lane = b.vextract(s, idx);
                b.fpext(lane, TypeId::F64)
            },
            5.0,
        ),
    },
    Case {
        name: "vadd_f32_v256",
        ops: &["Vadd", "Vconst", "Vextract", "Fpext", "Vsplit", "AVX"],
        kind: CaseKind::F64(
            |b| {
                // 高半 lane0 = 5 + 50 = 55
                let a = b.vconst(vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
                let c = b.vconst(vec![10.0f32, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0]);
                let s = b.vadd(a, c);
                let hi = b.vsplit(s, 1);
                let idx = b.iconst_i32(0);
                let lane = b.vextract(hi, idx);
                b.fpext(lane, TypeId::F64)
            },
            55.0,
        ),
    },
    // ══════════════════ 迭代 10（Phase 7 收尾）：缺口补齐 ══════════════════
    Case {
        name: "ssub_sat_i32_ok",
        ops: &["SsubSat"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(20);
                let c = b.iconst_i32(22);
                b.ssub_sat(a, c)
            },
            -2,
        ),
    },
    Case {
        name: "ssub_sat_i32_underflow",
        ops: &["SsubSat"],
        kind: CaseKind::I32(
            |b| {
                let a = b.iconst_i32(i32::MIN);
                let c = b.iconst_i32(1);
                b.ssub_sat(a, c)
            },
            i32::MIN,
        ),
    },
    Case {
        name: "ssub_sat_i64_overflow",
        ops: &["SsubSat"],
        kind: CaseKind::I64(
            |b| {
                let a = b.iconst_i64(i64::MAX);
                let c = b.iconst_i64(-1);
                b.ssub_sat(a, c)
            },
            i64::MAX,
        ),
    },
    Case {
        name: "ptrtoint_roundtrip",
        ops: &["Ptrtoint", "Inttoptr"],
        kind: CaseKind::I64(
            |b| {
                let v = b.iconst_i64(0x12345678);
                let p = b.inttoptr(v, TypeId::PTR);
                b.ptrtoint(p, TypeId::I64)
            },
            0x12345678,
        ),
    },
    Case {
        name: "undef_zero",
        ops: &["Undef"],
        kind: CaseKind::I64(|b| b.undef(TypeId::I64), 0),
    },
    Case {
        name: "poison_zero",
        ops: &["Poison"],
        kind: CaseKind::I32(|b| b.poison(TypeId::I32), 0),
    },
    Case {
        name: "globaladdr_load",
        ops: &["GlobalAddr", "Load"],
        kind: CaseKind::Module(
            |m| {
                // 全局 i64 = 42；main 经 global_addr + load 读取并返回
                let g = m
                    .add_global(
                        GlobalVariable::mutable("g_answer", TypeId::I64)
                            .with_init(42i64.to_le_bytes().to_vec()),
                    )
                    .expect("global");
                let sig = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut b = FunctionBuilder::new("main", TypeContext::new(), sig);
                b.create_block_here();
                let addr = b.global_addr(g);
                let v = b.load(addr, TypeId::I64);
                b.ret(&[v]);
                m.add_function(b.finish().expect("main"))
            },
            42,
        ),
    },
    Case {
        name: "globaladdr_store_load",
        ops: &["GlobalAddr", "Store", "Load"],
        kind: CaseKind::Module(
            |m| {
                // 全局 i64 初始 7；main 写入 100 后读回
                let g = m
                    .add_global(
                        GlobalVariable::mutable("g_counter", TypeId::I64)
                            .with_init(7i64.to_le_bytes().to_vec()),
                    )
                    .expect("global");
                let sig = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut b = FunctionBuilder::new("main", TypeContext::new(), sig);
                b.create_block_here();
                let addr = b.global_addr(g);
                let hundred = b.iconst_i64(100);
                b.store(hundred, addr);
                let v = b.load(addr, TypeId::I64);
                b.ret(&[v]);
                m.add_function(b.finish().expect("main"))
            },
            100,
        ),
    },
    Case {
        name: "frem_f64",
        ops: &["Frem", "Fconst"],
        kind: CaseKind::F64(
            |b| {
                let a = b.fconst(10.5f64.to_bits(), TypeId::F64);
                let d = b.fconst(3.0f64.to_bits(), TypeId::F64);
                b.frem(a, d)
            },
            1.5,
        ),
    },
    Case {
        name: "atomic_rmw_add",
        ops: &["AtomicRmw", "GlobalAddr", "Load"],
        kind: CaseKind::Module(
            |m| {
                let g = m
                    .add_global(
                        GlobalVariable::mutable("g_atomic_add", TypeId::I64)
                            .with_init(10i64.to_le_bytes().to_vec()),
                    )
                    .expect("global");
                let sig = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut b = FunctionBuilder::new("main", TypeContext::new(), sig);
                b.create_block_here();
                let addr = b.global_addr(g);
                let five = b.iconst_i64(5);
                b.atomic_rmw(
                    AtomicRmwOp::Add,
                    addr,
                    five,
                    Ordering::SequentiallyConsistent,
                );
                let v = b.load(addr, TypeId::I64);
                b.ret(&[v]);
                m.add_function(b.finish().expect("main"))
            },
            15,
        ),
    },
    Case {
        name: "atomic_rmw_sub",
        ops: &["AtomicRmw", "GlobalAddr", "Load"],
        kind: CaseKind::Module(
            |m| {
                let g = m
                    .add_global(
                        GlobalVariable::mutable("g_atomic_sub", TypeId::I64)
                            .with_init(20i64.to_le_bytes().to_vec()),
                    )
                    .expect("global");
                let sig = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut b = FunctionBuilder::new("main", TypeContext::new(), sig);
                b.create_block_here();
                let addr = b.global_addr(g);
                let six = b.iconst_i64(6);
                b.atomic_rmw(
                    AtomicRmwOp::Sub,
                    addr,
                    six,
                    Ordering::SequentiallyConsistent,
                );
                let v = b.load(addr, TypeId::I64);
                b.ret(&[v]);
                m.add_function(b.finish().expect("main"))
            },
            14,
        ),
    },
    Case {
        name: "atomic_rmw_xchg",
        ops: &["AtomicRmw", "GlobalAddr", "Load"],
        kind: CaseKind::Module(
            |m| {
                let g = m
                    .add_global(
                        GlobalVariable::mutable("g_atomic_xchg", TypeId::I64)
                            .with_init(7i64.to_le_bytes().to_vec()),
                    )
                    .expect("global");
                let sig = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut b = FunctionBuilder::new("main", TypeContext::new(), sig);
                b.create_block_here();
                let addr = b.global_addr(g);
                let hundred = b.iconst_i64(100);
                b.atomic_rmw(
                    AtomicRmwOp::Xchg,
                    addr,
                    hundred,
                    Ordering::SequentiallyConsistent,
                );
                let v = b.load(addr, TypeId::I64);
                b.ret(&[v]);
                m.add_function(b.finish().expect("main"))
            },
            100,
        ),
    },
    Case {
        name: "cmpxchg_success",
        ops: &["Cmpxchg", "GlobalAddr", "Load"],
        kind: CaseKind::Module(
            |m| {
                let g = m
                    .add_global(
                        GlobalVariable::mutable("g_cmpxchg", TypeId::I64)
                            .with_init(10i64.to_le_bytes().to_vec()),
                    )
                    .expect("global");
                let sig = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut b = FunctionBuilder::new("main", TypeContext::new(), sig);
                b.create_block_here();
                let addr = b.global_addr(g);
                let cmp = b.iconst_i64(10);
                let new = b.iconst_i64(99);
                b.cmpxchg(
                    addr,
                    cmp,
                    new,
                    Ordering::SequentiallyConsistent,
                    Ordering::SequentiallyConsistent,
                    false,
                );
                let v = b.load(addr, TypeId::I64);
                b.ret(&[v]);
                m.add_function(b.finish().expect("main"))
            },
            99,
        ),
    },
    Case {
        name: "cmpxchg_fail_keeps_old",
        ops: &["Cmpxchg", "GlobalAddr", "Load"],
        kind: CaseKind::Module(
            |m| {
                let g = m
                    .add_global(
                        GlobalVariable::mutable("g_cmpxchg2", TypeId::I64)
                            .with_init(10i64.to_le_bytes().to_vec()),
                    )
                    .expect("global");
                let sig = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut b = FunctionBuilder::new("main", TypeContext::new(), sig);
                b.create_block_here();
                let addr = b.global_addr(g);
                let cmp = b.iconst_i64(123); // 不匹配 → 内存保持
                let new = b.iconst_i64(99);
                b.cmpxchg(
                    addr,
                    cmp,
                    new,
                    Ordering::SequentiallyConsistent,
                    Ordering::SequentiallyConsistent,
                    false,
                );
                let v = b.load(addr, TypeId::I64);
                b.ret(&[v]);
                m.add_function(b.finish().expect("main"))
            },
            10,
        ),
    },
    Case {
        name: "gep_array_index",
        ops: &["GetElementPtr", "GlobalAddr", "Load"],
        kind: CaseKind::Module(
            |m| {
                // 全局 i64 数组 [10,20,30,40]；gep i64, ptr, 2 → load = 30
                let mut bytes = Vec::new();
                for v in [10i64, 20, 30, 40] {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
                let g = m
                    .add_global(GlobalVariable::mutable("g_arr", TypeId::I64).with_init(bytes))
                    .expect("global");
                let sig = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut b = FunctionBuilder::new("main", TypeContext::new(), sig);
                b.create_block_here();
                let base = b.global_addr(g);
                let idx = b.iconst_i64(2);
                let elem = b.gep(base, &[idx], TypeId::I64);
                let v = b.load(elem, TypeId::I64);
                b.ret(&[v]);
                m.add_function(b.finish().expect("main"))
            },
            30,
        ),
    },
    Case {
        name: "gep_dynamic_index",
        ops: &["GetElementPtr", "GlobalAddr", "Load", "Iadd"],
        kind: CaseKind::Module(
            |m| {
                // 动态索引：gep i64, ptr, (1+1) → load = 30
                let mut bytes = Vec::new();
                for v in [10i64, 20, 30, 40] {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
                let g = m
                    .add_global(GlobalVariable::mutable("g_arr2", TypeId::I64).with_init(bytes))
                    .expect("global");
                let sig = FunctionSignature::new(&[], &[TypeId::I64]);
                let mut b = FunctionBuilder::new("main", TypeContext::new(), sig);
                b.create_block_here();
                let base = b.global_addr(g);
                let one = b.iconst_i64(1);
                let two = b.iconst_i64(1);
                let idx = b.iadd(one, two);
                let elem = b.gep(base, &[idx], TypeId::I64);
                let v = b.load(elem, TypeId::I64);
                b.ret(&[v]);
                m.add_function(b.finish().expect("main"))
            },
            30,
        ),
    },
    Case {
        name: "nop_in_block",
        ops: &["Nop"],
        kind: CaseKind::Block(
            |b| {
                b.create_block_here();
                b.nop();
                let v = b.iconst_i32(42);
                b.ret(&[v]);
            },
            42,
        ),
    },
];

/// 运行全部用例，返回 (名字, 结果)。
pub fn run_all<M: TargetMachine + Clone>(r: &Runner<M>) -> Vec<(String, Outcome)> {
    CASES
        .iter()
        .map(|c| (c.name.to_string(), run_case(r, c)))
        .collect()
}
