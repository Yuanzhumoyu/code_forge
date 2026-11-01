//! v32 M1b：sysv64 的 i128 入口是整数对，wide_split 拆成两个 i64 形参。
//! 仅编译验证（宿主是 win64，不能直接以 sysv ABI 调用）。
use forge_codegen::CompileSession;
use forge_codegen::x86;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::{CallConvId, ConvName, TypeId};

#[test]
fn i128_sysv_entry_compiles() {
    x86::ensure_registered();
    let i128 = TypeId::I128;
    let i64 = TypeId::I64;
    let mut sig = FunctionSignature::new(&[(i128, "a"), (i128, "b")], &[i64]);
    sig.calling_convention = CallConvId::Builtin(ConvName::SysV64);
    let mut b = FunctionBuilder::new("add128_sysv", TypeContext::new(), sig);
    let (entry, p) = b.create_block_with_params(&[(i128, "a"), (i128, "b")]);
    b.switch_to_block(entry);
    let s = b.iadd(p[0], p[1]);
    let narrowed = b.ireduce(s, i64);
    b.ret(&[narrowed]);
    let f = b.finish().expect("build");
    match CompileSession::new(x86::TargetMachine::new()).compile(&f) {
        Ok(c) => println!("sysv i128 entry compiled: code len {}", c.code.len()),
        Err(e) => panic!("sysv i128 entry should compile: {e}"),
    }
}
