//! v32 M1b 守卫：直接 IR 的 `i128` 入口参数目前**明确 fail-closed**（不是 panic）。
//!
//! forge-rustc 的降级层会按 `FnAbi` 编组 128 位（指针/向量槽/整数对）；这里守的是
//! **直接 IR 签名**的边界。M1b 支持时把本测试改成真执行断言。
use forge_codegen::CompileSession;
use forge_codegen::x86;
use forge_ir::TypeId;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};

#[test]
fn i128_entry_is_fail_closed() {
    x86::ensure_registered();
    let i128 = TypeId::I128;
    let sig = FunctionSignature::new(&[(i128, "a"), (i128, "b")], &[i128]);
    let mut b = FunctionBuilder::new("add128", TypeContext::new(), sig);
    let (entry, p) = b.create_block_with_params(&[(i128, "a"), (i128, "b")]);
    b.switch_to_block(entry);
    let s = b.iadd(p[0], p[1]);
    b.ret(&[s]);
    let f = b.finish().expect("build");
    let r = CompileSession::new(x86::TargetMachine::new()).compile(&f);
    match r {
        Ok(_) => panic!("i128 entry 目前应 fail-closed（M1b 未完成）"),
        Err(e) => {
            let msg = format!("{e}");
            println!("i128 entry fail-closed: {msg}");
            assert!(
                msg.contains("WIDE_INT_SHAPE"),
                "应为明确的 WIDE_INT_SHAPE，而不是 panic/其他错误: {msg}"
            );
        }
    }
}
