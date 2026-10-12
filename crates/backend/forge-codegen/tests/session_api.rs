//! M1：**唯一编译入口** `CompileSession` —— 单函数与整模块两条路径同源。
//!
//! 这条守卫钉住三件事：
//!
//! 1. 整模块编译（[`CompileSession::compile_module`]）按 `FuncRef` 序返回**每个**函数，
//!    小模块（串行）与大模块（≥8 个函数走 scoped 并行）走同一条编译实现；
//! 2. 同一函数的字节在"整模块"与"单函数"两条路径上**逐字节相同**（签名表同源）；
//! 3. `add_function` 单函数便捷入口也认会话上的模块签名表（变参 D6 的读口子）。

use forge_codegen::CompileSession;
use forge_codegen::arch::x86::TargetMachine;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::function::Module;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::{CallConvId, ConvName, FuncRef, ModuleSigs, TypeId};

/// `callee(i64) -> i64`（可变参）+ `caller() -> i64`（用 sysv64 调 `callee(7)`）。
///
/// 被调方是否变参决定调用点要不要按变参规划落点——所以这份模块同时覆盖
/// "签名表起作用"与"签名表不起作用"两种编译结果。
fn two_fn_module(callee_variadic: bool) -> Module {
    let mut m = Module::new();
    let sig_c = FunctionSignature::new(&[(TypeId::I64, "x")], &[TypeId::I64])
        .with_variadic(callee_variadic);
    let mut bc = FunctionBuilder::new("callee", TypeContext::new(), sig_c);
    let (blk, p) = bc.create_block_with_params(&[(TypeId::I64, "x")]);
    bc.switch_to_block(blk);
    bc.ret(&[p[0]]);
    m.add_function(bc.finish().expect("callee"));

    let sig_m = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut bm = FunctionBuilder::new("caller", TypeContext::new(), sig_m);
    bm.create_block_here();
    let v = bm.iconst(7, TypeId::I64);
    let callee = FuncRef::new(0);
    let r = bm.call(callee, &[v], &[TypeId::I64]);
    bm.ret(&[r[0]]);
    m.add_function(bm.finish().expect("caller"));
    m
}

/// ① 整模块编译返回每个函数，且按 `FuncRef` 序（= 模块函数表序）。
#[test]
fn compiled_module_lists_every_function_in_func_ref_order() {
    let m = two_fn_module(false);
    let compiled = CompileSession::for_module(TargetMachine::new(), &m)
        .compile_module(&m)
        .expect("整模块编译");
    assert_eq!(compiled.function_count(), 2);
    let names: Vec<&str> = compiled.iter_functions().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["callee", "caller"], "按 FuncRef 序");
    assert!(compiled.function("caller").is_some());
    assert!(compiled.function("nope").is_none());
}

/// ② 同一函数的字节在"整模块"与"单函数"两条路径上逐字节相同。
///
/// 两条路径都必须装上模块签名表——变参被调方在不在表里会改变调用点的落点，
/// 这正是整模块编译唯一的签名事实来源。
#[test]
fn module_and_single_function_paths_agree_byte_for_byte() {
    for callee_variadic in [false, true] {
        let m = two_fn_module(callee_variadic);
        let session = CompileSession::for_module(TargetMachine::new(), &m);
        let module_bytes = session
            .compile_module(&m)
            .expect("整模块编译")
            .function("caller")
            .expect("caller")
            .code
            .clone();
        let single_bytes = session
            .compile(m.get_function(FuncRef::new(1)))
            .expect("单函数编译")
            .code;
        assert_eq!(
            module_bytes, single_bytes,
            "callee_variadic={callee_variadic}：两条路径必须同源"
        );
    }
}

/// ③ 大模块（≥8 个函数 ⇒ scoped 并行路径）与单函数路径同样同源。
#[test]
fn large_modules_take_the_same_path() {
    let mut m = Module::new();
    for i in 0..9 {
        let sig = FunctionSignature::new(&[(TypeId::I64, "x")], &[TypeId::I64]);
        let name = format!("f{i}");
        let mut b = FunctionBuilder::new(name.as_str(), TypeContext::new(), sig);
        let (blk, p) = b.create_block_with_params(&[(TypeId::I64, "x")]);
        b.switch_to_block(blk);
        let two = b.iconst(2, TypeId::I64);
        let sum = b.iadd(p[0], two);
        b.ret(&[sum]);
        m.add_function(b.finish().expect("f"));
    }
    let session = CompileSession::for_module(TargetMachine::new(), &m);
    let module = session.compile_module(&m).expect("并行路径整模块编译");
    assert_eq!(module.function_count(), 9);
    for i in 0..9u32 {
        let name = format!("f{i}");
        let from_module = &module.function(&name).expect("在模块里").code;
        let alone = &session
            .compile(m.get_function(FuncRef::new(i)))
            .expect("单函数编译")
            .code;
        assert_eq!(from_module, alone, "{name}：并行路径与单函数路径必须同源");
    }
}

/// ④ `add_function` 便捷入口认会话上的模块签名表（变参 D6）。
///
/// 装表与不装表编译同一段调用点代码，字节**必须不同**——差异就是"被调方是变参"
/// 这条运行期信息（sysv64 下调用点要写调用的元信息寄存器 `%al`，浮点实参才按
/// 向量寄存器计数落位）。若 `add_function` 丢掉签名表，这条断言会退化成"两边相同"。
#[test]
fn add_function_honours_the_session_signature_table() {
    let sig_caller = FunctionSignature::new(&[], &[TypeId::I64])
        .with_calling_convention(CallConvId::builtin(ConvName::SysV64));

    let with_table = CompileSession::new(TargetMachine::new())
        .with_module_sigs(ModuleSigs::from_pairs(vec![(true, 1)]))
        .add_function("caller", &sig_caller, caller_with_float_arg)
        .expect("装表编译");
    let without_table = CompileSession::new(TargetMachine::new())
        .add_function("caller", &sig_caller, caller_with_float_arg)
        .expect("不装表编译");
    assert_ne!(
        with_table.code, without_table.code,
        "会话上的签名表必须影响调用点（否则变参落点又按非变参发）"
    );
}

/// `caller()`：用 sysv64 调 `FuncRef(0)`，实参 `(i64, f64)`——浮点实参会用掉一个
/// 向量寄存器，变参调用点因此必须写元信息寄存器（`variadic_meta.rs` 的同款形状）。
fn caller_with_float_arg(b: &mut FunctionBuilder) {
    b.create_block_here();
    let fmt = b.iconst_i64(0);
    let x = b.fconst(1.5f64.to_bits(), TypeId::F64);
    let r = b.call(FuncRef::new(0), &[fmt, x], &[TypeId::I64]);
    b.ret(&[r[0]]);
}
