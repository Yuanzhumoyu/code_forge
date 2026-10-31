//! v30 M5：**人体工学验收** —— 宿主用三步拿到可调用函数。
//!
//! 这条用例故意写得像文档：`JitLoader::new` → `add_function` → `get::<F>`。
//! 它跑在 `--all-features`（`jit`）下，是"重构后确实更好用"的最小证据。
#![cfg(all(feature = "jit", target_arch = "x86_64"))]

use forge_codegen::jit::JitLoader;
use forge_codegen::x86;
use forge_ir::{FunctionSignature, TypeId};

#[test]
fn compile_and_call_in_three_steps() {
    x86::ensure_registered();
    let mut jit = JitLoader::new(x86::TargetMachine::new());

    let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
    jit.add_function("add", &sig, |b| {
        let (entry, p) = b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        b.switch_to_block(entry);
        let sum = b.iadd(p[0], p[1]);
        b.ret(&[sum]);
    })
    .expect("编译并装载 add");

    let add: extern "C" fn(i64, i64) -> i64 = jit.get("add").expect("类型化取指针");
    assert_eq!(add(20, 22), 42);
}
