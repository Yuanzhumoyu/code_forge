//! 调用约定**读路径**守卫（v20 A2）。
//!
//! 背景（旧设计的头号坑）：`CallConv` 有 16 个变体、`ctx.call_conv` 被赋值一次，
//! 但**全仓没有任何一处读**——IR 里那份约定等于文档。A2 把它变成真实的读路径：
//!
//! ```text
//! FunctionSignature.calling_convention
//!    → conv_registry::resolve()   （宿主注册表；**未注册即 fail-closed**）
//!    → ctx.call_conv_name          （A3 用它查 AbiRules/AbiBinding）
//! ```
//!
//! 本文件钉三件事：
//!
//! 1. 内置五份约定能解析（`c`/`win64`/`sysv64`/`aapcs64`/`lp64d`）；
//! 2. **未注册 ⇒ 编译报错**（`Named`/`Index` 两条路都报，且消息列出已注册的名字）
//!    ——不是"静默用缺省约定"（旧 `CallConv::Default` 就是这么变成死值的）；
//! 3. 注册之后同一条 IR 就能编译 ⇒ **改约定名会改变规划结果**（这条链路是活的）。

mod common;

use common::demo8_v12::TargetMachine;
use forge_codegen::FunctionCompiler;
use forge_codegen::pipeline::conv_registry;
use forge_ir::CallConvId;
use forge_ir::ConvName;
use forge_ir::TypeId;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::function::Function;
use forge_ir::ir::types::{FunctionSignature, TypeContext};

/// `fn f(a: i8) -> i8 { a }`，带指定调用约定。
fn build(cc: CallConvId) -> Function {
    let ctx = TypeContext::new();
    let sig =
        FunctionSignature::new(&[(TypeId::I8, "a")], &[TypeId::I8]).with_calling_convention(cc);
    let mut b = FunctionBuilder::new("conv_probe", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::I8, "a")]);
    b.switch_to_block(entry);
    b.ret(&[params[0]]);
    b.finish().expect("build")
}

#[test]
fn builtin_conventions_resolve_to_registry_names() {
    for n in ConvName::ALL {
        assert_eq!(
            conv_registry::resolve(&CallConvId::builtin(n)).expect("内置约定必须在注册表里"),
            n.as_str()
        );
    }
    // 默认签名 = `c` 约定（LLVM 里"不写关键字"的那份）。
    assert_eq!(CallConvId::default(), CallConvId::builtin(ConvName::C));
}

#[test]
fn unregistered_convention_fails_closed_and_names_what_is_known() {
    let err = conv_registry::resolve(&CallConvId::named("nope")).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("nope"), "{msg}");
    assert!(msg.contains("未在宿主注册表里注册"), "{msg}");
    // 消息要能让人自己修：列出已注册的名字 + 内置来源。
    assert!(msg.contains("win64") && msg.contains("sysv64"), "{msg}");
    assert!(msg.contains("register_rules_toml"), "{msg}");

    // 数值约定走 `cc<N>` 这个键：没注册也报错（不猜编号含义）。
    let err = conv_registry::resolve(&CallConvId::index(9999)).unwrap_err();
    assert!(err.to_string().contains("cc9999"), "{err}");
}

/// **端到端**：未注册的约定名让编译在入口就失败；注册之后同一份 IR 能编译通过。
#[test]
fn compile_reads_the_convention_and_registration_unblocks_it() {
    let tm = TargetMachine::new();
    let compiler = FunctionCompiler::new(tm);

    let cc = CallConvId::named("probe_conv");
    let func = build(cc.clone());
    let err = compiler
        .compile(&func)
        .expect_err("未注册的约定必须 fail-closed");
    assert!(err.to_string().contains("probe_conv"), "{err}");

    // 注册一份最小可用约定（规则 + 绑定）后，同一条 IR 就能过——证明"改约定名 ⇒
    // 规划走另一份数据"这条链路是活的（A3 起同一路径产出 AbiPlan）。
    conv_registry::register_rules_toml(
        r#"
name = "probe_conv"
parent = "c"
"#,
    )
    .expect("注册规则");
    assert!(
        conv_registry::registered_names()
            .iter()
            .any(|n| n == "probe_conv")
    );

    compiler.compile(&func).expect("注册后应能编译");
}
