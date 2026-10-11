//! codegen 负向测试：无机器指令映射的 IR 指令必须被编译期拒绝（报错而非 panic/静默错码）。

use forge_codegen::arch::x86::{TargetMachine, ensure_registered};
use forge_codegen::pipeline::compiler::FunctionCompiler;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
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
        forge_ir::FuncRef::new(0),
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
fn agg_param_over_16_bytes_routes_by_convention() {
    // 32 字节聚合按值形参：**落点由约定决定**（A1′ 起不再按本地阈值一律拒绝）
    // - win64（这台机器上 `c` 的别名）⇒ `aggregate size_gt 8 ⇒ indirect`，即
    //   "调用方副本 + 传指针" ⇒ 现在能编译；
    // - sysv64 ⇒ `aggregate size_gt 16 ⇒ stack`（整块进传出参数区），发射侧尚未接
    //   ⇒ 必须 fail-closed。
    use forge_ir::ir::types::{CallConvId, ConvName};
    use forge_ir::text::parser::parse_module;
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
    // win64：被调方（形参转 `ptr` + 按地址读）能编译
    let callee = module.iter_functions().next().unwrap().clone();
    let ok = FunctionCompiler::new(TargetMachine::new()).compile_raw(&callee);
    assert!(
        ok.is_ok(),
        "win64 上 32B 聚合按值形参应走「副本 + 指针」，got: {:?}",
        ok.map(|c| c.code.len())
    );
    // win64：调用方（副本 + 逐段 store + 传指针）也能编译
    let caller = module.iter_functions().nth(1).unwrap().clone();
    let ok_caller = FunctionCompiler::new(TargetMachine::new()).compile_raw(&caller);
    assert!(
        ok_caller.is_ok(),
        "win64 上 32B 聚合按值实参应走「调用方副本 + 指针」，got: {:?}",
        ok_caller.map(|c| c.code.len())
    );
    // sysv64：**被调方**已接（按值整块进传出区 ⇒ 形参保留、值从 `[fp + offset]` 读），
    // 所以被调方现在**能**编译；**调用方**仍 fail-closed（发射侧形状透传待做）。
    // 注意口径：管线读的是 **`Function.calling_convention`** 字段（与 `abi_setup::setup_conv`
    // 同源），签名里的约定只是它的来源——两处一起改，避免测试自己骗自己。
    let mut sysv_callee = callee.clone();
    sysv_callee.calling_convention = CallConvId::Builtin(ConvName::SysV64);
    let mut sig = sysv_callee
        .types
        .borrow()
        .get_signature(sysv_callee.signature)
        .clone();
    sig.calling_convention = CallConvId::Builtin(ConvName::SysV64);
    sysv_callee.signature = sysv_callee.types.borrow_mut().register_signature(sig);
    let ok_sysv = FunctionCompiler::new(TargetMachine::new()).compile_raw(&sysv_callee);
    assert!(
        ok_sysv.is_ok(),
        "sysv64 上 32B 聚合的**被调方**应能按 [fp+offset] 读，got: {:?}",
        ok_sysv.map(|c| c.code.len())
    );
    let mut sysv_caller = caller.clone();
    sysv_caller.calling_convention = CallConvId::Builtin(ConvName::SysV64);
    let mut csig = sysv_caller
        .types
        .borrow()
        .get_signature(sysv_caller.signature)
        .clone();
    csig.calling_convention = CallConvId::Builtin(ConvName::SysV64);
    sysv_caller.signature = sysv_caller.types.borrow_mut().register_signature(csig);
    let ok_sysv_caller = FunctionCompiler::new(TargetMachine::new()).compile_raw(&sysv_caller);
    assert!(
        ok_sysv_caller.is_ok(),
        "sysv64 的**调用方**一侧已接（本地副本 + `byval(聚合类型)` 属性 + 逐段拷进传出区），\
         应能编译，got: {:?}",
        ok_sysv_caller.map(|c| c.code.len())
    );
}
