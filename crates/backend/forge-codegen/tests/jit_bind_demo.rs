//! v32 M1 效果演示：目标感知的 `bind`（ABI 校验）+ `JitFn::call`。
//!
//! 运行：`cargo test -p forge-codegen --all-features --test jit_bind_demo -- --nocapture`
//! 预期输出（x86_64 宿主）：
//! ```text
//! add(2,3)=5
//! not(true)=false
//! variadic_sum(1,2,3)=7
//! variadic_counted(3,10,20,30)=60
//! slice_sum(10,20,30)=60
//! slice_fill(4)=[7, 7, 7, 7]
//! ```

#![cfg(all(feature = "jit", target_arch = "x86_64"))]

use forge_codegen::jit::JitLoader;
use forge_codegen::x86;
use forge_ir::IntCC;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::TypeContext;
use forge_ir::{FunctionSignature, Module, Opcode, TypeId};

#[test]
fn bind_scalar_and_bool() {
    x86::ensure_registered();
    let mut jit = JitLoader::new(x86::TargetMachine::new());

    // 标量：add(i64, i64) -> i64
    let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
    jit.add_function("add", &sig, |b| {
        let (entry, p) = b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        b.switch_to_block(entry);
        let sum = b.iadd(p[0], p[1]);
        b.ret(&[sum]);
    })
    .expect("add");

    let add = jit
        .bind::<extern "C" fn(i64, i64) -> i64>("add")
        .expect("bind add");
    assert!(add.is_host_compatible(), "x86_64 宿主应可安全调用");
    println!("add(2,3)={}", add.call(2, 3));
    assert_eq!(add.call(2, 3), 5);

    // bool：not(i1) -> i1；ABI 槽是 8 位整数（C99 `_Bool`）
    let sig = FunctionSignature::new(&[(TypeId::BOOL, "a")], &[TypeId::BOOL]);
    jit.add_function("not", &sig, |b| {
        let (entry, p) = b.create_block_with_params(&[(TypeId::BOOL, "a")]);
        b.switch_to_block(entry);
        let r = b.bnot(p[0]);
        b.ret(&[r]);
    })
    .expect("not");

    let not = jit
        .bind::<extern "C" fn(bool) -> bool>("not")
        .expect("bind not");
    println!("not(true)={}", not.call(true));
    assert!(!not.call(true));

    // source 模式：`u8` 与 IR 的 `i1` 语义不同，必须报错而不是静默放宽。
    let err = jit
        .bind_source::<extern "C" fn(u8) -> bool>("not")
        .expect_err("u8 不应匹配 i1 源码签名");
    assert!(matches!(
        err,
        forge_codegen::jit::JitBindError::ParamType { index: 0, .. }
    ));

    // 源码模式正向：bool 的 IR 源签名可校验。
    jit.bind_source::<extern "C" fn(bool) -> bool>("not")
        .expect("bool 源码签名应匹配 i1");

    // 变参：固定前缀类型化校验，`...` 由 Rust 变参指针调用（M3 切片）。
    let mut variadic_sig = FunctionSignature::new(&[(TypeId::I64, "x")], &[TypeId::I64]);
    variadic_sig.variadic = true;
    jit.add_function("vsum", &variadic_sig, |b| {
        let (entry, _p) = b.create_block_with_params(&[(TypeId::I64, "x")]);
        b.switch_to_block(entry);
        let seven = b.const_int(7i64, TypeId::I64);
        b.ret(&[seven]);
    })
    .expect("vsum");

    // 非变参 bind 必须拒绝；变参 bind 校验固定前缀后返回 raw 指针。
    assert!(jit.bind::<extern "C" fn(i64) -> i64>("vsum").is_err());
    let vsum = unsafe { jit.bind_variadic::<unsafe extern "C" fn(i64, ...) -> i64>("vsum") }
        .expect("bind_variadic vsum");
    let r = unsafe { (vsum.raw())(1i64, 2i64, 3i64) };
    assert_eq!(r, 7, "变参函数固定前缀 + 额外实参应正常调用");
    println!("variadic_sum(1,2,3)={r}");

    // 让编译器知道 `Opcode` 被引用（早期探针保留的导入）。
    let _ = Opcode::Nop;
}

/// 截止契约：C 变参**没有自动计数**；由固定参数 `n` + 循环决定读多少个。
/// 这条走模块路径（callee 的变参 ABI 表完整），host 调非变参 `main`。
#[test]
fn variadic_counted_loop() {
    x86::ensure_registered();
    let mut jit = JitLoader::new(x86::TargetMachine::new());

    let sig_c = FunctionSignature::new(&[(TypeId::I64, "fmt"), (TypeId::I64, "n")], &[TypeId::I64])
        .with_variadic(true);
    let mut c = FunctionBuilder::new("callee", TypeContext::new(), sig_c);
    let (entry, ep) = c.create_block_with_params(&[(TypeId::I64, "fmt"), (TypeId::I64, "n")]);
    c.switch_to_block(entry);
    let ap = c.va_start();
    let (loop_b, lp) = c.create_block_with_params(&[(TypeId::I64, "i"), (TypeId::I64, "acc")]);
    let (body, _bp) = c.create_block_with_params(&[]);
    let (exit, xp) = c.create_block_with_params(&[(TypeId::I64, "acc")]);
    c.switch_to_block(entry);
    let zero = c.const_int(0i64, TypeId::I64);
    c.jump(loop_b, &[zero, zero]);
    c.switch_to_block(loop_b);
    let cond = c.icmp(IntCC::Slt, lp[0], ep[1]);
    c.branch(cond, body, &[], exit, &[lp[1]]);
    c.switch_to_block(body);
    let v = c.va_arg(ap, TypeId::I64);
    let sum = c.iadd(lp[1], v);
    let one = c.const_int(1i64, TypeId::I64);
    let i2 = c.iadd(lp[0], one);
    c.jump(loop_b, &[i2, sum]);
    c.switch_to_block(exit);
    c.ret(&[xp[0]]);

    let mut module = Module::new();
    let cref = module.add_function(c.finish().expect("callee"));

    let sig_m = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut m = FunctionBuilder::new("main", TypeContext::new(), sig_m);
    let (me, _) = m.create_block_with_params(&[]);
    m.switch_to_block(me);
    let fmt = m.const_int(0i64, TypeId::I64);
    let n = m.const_int(3i64, TypeId::I64);
    let a = m.const_int(10i64, TypeId::I64);
    let b = m.const_int(20i64, TypeId::I64);
    let d = m.const_int(30i64, TypeId::I64);
    let got = m.call(cref, &[fmt, n, a, b, d], &[TypeId::I64])[0];
    m.ret(&[got]);
    module.add_function(m.finish().expect("main"));

    jit.compile_module(&module).expect("compile module");
    let f: extern "C" fn() -> i64 = jit.get("main").expect("get main");
    assert_eq!(f(), 60, "n=3 决定读取 10+20+30，而不是编译期固定个数");
    println!("variadic_counted(3,10,20,30)=60");
}

/// 安全版“变参”：`(ptr, len)` ABI，Rust 侧传 `&[T]`，长度显式且运行时可变。
#[test]
fn slice_ptr_len_safe() {
    x86::ensure_registered();
    let mut jit = JitLoader::new(x86::TargetMachine::new());

    let sig = FunctionSignature::new(&[(TypeId::PTR, "p"), (TypeId::I64, "n")], &[TypeId::I64]);
    let mut f = FunctionBuilder::new("sum_slice", TypeContext::new(), sig);
    let (entry, ep) = f.create_block_with_params(&[(TypeId::PTR, "p"), (TypeId::I64, "n")]);
    f.switch_to_block(entry);
    let (loop_b, lp) = f.create_block_with_params(&[(TypeId::I64, "i"), (TypeId::I64, "acc")]);
    let (body, _bp) = f.create_block_with_params(&[]);
    let (exit, xp) = f.create_block_with_params(&[(TypeId::I64, "acc")]);
    f.switch_to_block(entry);
    let zero = f.const_int(0i64, TypeId::I64);
    f.jump(loop_b, &[zero, zero]);
    f.switch_to_block(loop_b);
    let cond = f.icmp(IntCC::Slt, lp[0], ep[1]);
    f.branch(cond, body, &[], exit, &[lp[1]]);
    f.switch_to_block(body);
    let addr = f.gep(ep[0], &[lp[0]], TypeId::I64);
    let v = f.load(addr, TypeId::I64);
    let sum = f.iadd(lp[1], v);
    let one = f.const_int(1i64, TypeId::I64);
    let i2 = f.iadd(lp[0], one);
    f.jump(loop_b, &[i2, sum]);
    f.switch_to_block(exit);
    f.ret(&[xp[0]]);

    let mut module = Module::new();
    module.add_function(f.finish().expect("sum_slice"));
    jit.compile_module(&module).expect("compile module");

    let s = jit
        .bind_slice::<i64, i64>("sum_slice")
        .expect("bind_slice sum_slice");
    let got = s.call(&[10, 20, 30]);
    assert_eq!(got, 60, "ptr+len 安全切片应求和 10+20+30");
    println!("slice_sum(10,20,30)={got}");
}

/// 安全可变切片：`(ptr, len)` ABI，原地写回。
#[test]
fn slice_mut_ptr_len_safe() {
    x86::ensure_registered();
    let mut jit = JitLoader::new(x86::TargetMachine::new());

    let sig = FunctionSignature::new(&[(TypeId::PTR, "p"), (TypeId::I64, "n")], &[TypeId::I64]);
    let mut f = FunctionBuilder::new("fill_slice", TypeContext::new(), sig);
    let (entry, ep) = f.create_block_with_params(&[(TypeId::PTR, "p"), (TypeId::I64, "n")]);
    f.switch_to_block(entry);
    let (loop_b, lp) = f.create_block_with_params(&[(TypeId::I64, "i")]);
    let (body, _bp) = f.create_block_with_params(&[]);
    let (exit, _xp) = f.create_block_with_params(&[]);
    f.switch_to_block(entry);
    let zero = f.const_int(0i64, TypeId::I64);
    f.jump(loop_b, &[zero]);
    f.switch_to_block(loop_b);
    let cond = f.icmp(IntCC::Slt, lp[0], ep[1]);
    f.branch(cond, body, &[], exit, &[]);
    f.switch_to_block(body);
    let addr = f.gep(ep[0], &[lp[0]], TypeId::I64);
    let seven = f.const_int(7i64, TypeId::I64);
    f.store(seven, addr);
    let one = f.const_int(1i64, TypeId::I64);
    let i2 = f.iadd(lp[0], one);
    f.jump(loop_b, &[i2]);
    f.switch_to_block(exit);
    f.ret(&[ep[1]]);

    let mut module = Module::new();
    module.add_function(f.finish().expect("fill_slice"));
    jit.compile_module(&module).expect("compile module");

    let s = jit
        .bind_slice_mut::<i64, i64>("fill_slice")
        .expect("bind_slice_mut fill_slice");
    let mut buf = [0i64; 4];
    let n = s.call(&mut buf);
    assert_eq!(n, 4);
    assert_eq!(buf, [7i64; 4], "原地写回应全部为 7");
    println!("slice_fill(4)={buf:?}");
}
