//! codegen 负向测试：无机器指令映射的 IR 指令必须被编译期拒绝（报错而非 panic/静默错码）。

use forge_codegen::arch::x86_v12::{TargetMachine, ensure_registered};
use forge_codegen::pipeline::compiler::FunctionCompiler;
use forge_ir::builder::FunctionBuilder;
use forge_ir::types::{FunctionSignature, TypeContext};
use forge_ir::{InstFlags, Opcode};

#[test]
fn addrspacecast_compile_rejected() {
    // addrspacecast 在 x86 无机器指令映射（DSL lowering 无规则）：
    // compile 必须返回错误，而不是 panic / 静默生成错误代码。
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
    let mut b = FunctionBuilder::new("f", ctx.clone(), sig);
    let (entry, _) = b.create_entry_block();
    b.switch_to_block(entry);
    let p = b.alloca(ctx.i32_ty(), 1);
    let _ = b.emit1(
        Opcode::AddrSpaceCast,
        vec![p],
        vec![],
        ctx.ptr_ty(),
        InstFlags::NONE,
    );
    let v = b.iconst(1, ctx.i32_ty());
    b.ret(&[v]);
    let func = b.finish().expect("build");

    let compiler = FunctionCompiler::new(TargetMachine::new());
    let result = compiler.compile_raw(&func);
    assert!(
        result.is_err(),
        "addrspacecast 无机器指令，compile 应报错，got: {:?}",
        result.map(|c| c.code.len())
    );
}

#[test]
fn invoke_landingpad_compile_rejected() {
    // 3.2 P1.1：含 invoke/landingpad/resume 的异常函数在编译期被拒绝
    //（文本层仅解析/展示，无机器指令映射）。
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[]);
    let mut b = FunctionBuilder::new("f", ctx.clone(), sig);
    let (entry, _) = b.create_entry_block();
    b.switch_to_block(entry);
    let ok = b.create_block();
    let pad = b.create_block();
    let l = b.landingpad(ctx.ptr_ty());
    b.resume(l);
    b.switch_to_block(ok);
    b.ret(&[]);
    b.switch_to_block(pad);
    let l2 = b.landingpad(ctx.ptr_ty());
    b.resume(l2);
    b.switch_to_block(entry);
    b.invoke(
        forge_ir::FuncRef(0),
        &[],
        forge_ir::TypeId::VOID,
        ok,
        &[],
        pad,
        &[],
    );
    let func = b.finish().expect("build");

    let compiler = FunctionCompiler::new(TargetMachine::new());
    let result = compiler.compile_raw(&func);
    assert!(
        result.is_err(),
        "含 invoke/landingpad/resume 的异常函数应编译期拒绝，got: {:?}",
        result.map(|c| c.code.len())
    );
}

#[test]
fn agg_param_over_16_bytes_rejected() {
    // >16 字节聚合常量参数：SysV 栈传递未实现 → Unsupported。
    use forge_ir::ir_parser::parse_module;
    ensure_registered();
    let src = r#"
define i32 @sum({i64, i64, i64, i64} %p) {
  %entry:
    ret i32 0
}
define i32 @main() {
  %entry:
    %r = call i32 @sum({i64, i64, i64, i64} {i64 1, i64 2, i64 3, i64 4})
    ret i32 %r
}
"#;
    let module = parse_module(src).expect("parse");
    let func = module.iter_functions().next().unwrap().clone();
    let result = FunctionCompiler::new(TargetMachine::new()).compile_raw(&func);
    assert!(
        matches!(&result, Err(e) if format!("{e}").contains("Unsupported")),
        ">16 字节聚合传参应 Unsupported，got: {:?}",
        result.map(|c| c.code.len())
    );
}
