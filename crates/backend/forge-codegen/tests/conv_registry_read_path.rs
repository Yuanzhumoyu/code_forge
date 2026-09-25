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

/// **IR 属性 → 引擎视图**（v20 A2b）：`Function::param_attrs`/`ret_attrs` 与签名一起投影成
/// `forge_abi::Signature`——这些属性以前只有文本层认识、没有任何代码读。
///
/// 断言两件事：① 声明属性逐条映射（`byval` 的字节数由那个类型算）；② 类型摊开用
/// **TypeStore 的权威大小**（带填充的结构体不能按成员求和）。
#[test]
fn ir_attributes_project_into_the_engine_view() {
    use forge_codegen::pipeline::sig_view::{decl_attrs, signature_view, ty_view};
    use forge_ir::ir::function::ParamAttributes;
    use forge_ir::ir::types::TypeField;

    let ctx = TypeContext::new();
    // struct { i32; i64 }（带填充：权威大小 16，成员裸和 12）。
    let (i32t, i64t) = (TypeId::I32, TypeId::I64);
    let st = ctx.borrow_mut().struct_named(
        "AttrProbe",
        vec![TypeField::new(i32t), TypeField::new(i64t)],
        false,
    );
    {
        let store = ctx.borrow();
        assert_eq!(store.size_bytes(st), 16, "带填充结构体的权威大小");
        let v = ty_view(&store, st);
        assert_eq!((v.size, v.align), (16, 8));
        assert!(v.is_aggregate());

        // 声明属性映射：byval 取类型大小；align = 0 视为未声明。
        let a = ParamAttributes {
            byval: Some(st),
            inreg: true,
            zeroext: true,
            align: 32,
            ..ParamAttributes::default()
        };
        let d = decl_attrs(&a, &store);
        assert_eq!(d.byval, Some(16), "byval(结构体) → 字节数");
        assert!(d.inreg && d.zeroext);
        assert_eq!(d.declared_align(), Some(32));
        assert!(!d.sret && !d.signext);
    }

    // 端到端：Function 的签名 + param_attrs → 引擎视图。
    let sig = FunctionSignature::new(&[(i32t, "n"), (i64t, "s")], &[i32t]);
    let mut b = FunctionBuilder::new("attr_probe", ctx.clone(), sig);
    let (entry, params) = b.create_block_with_params(&[(i32t, "n"), (i64t, "s")]);
    b.switch_to_block(entry);
    b.ret(&[params[0]]);
    let mut func = b.finish().expect("build");
    func.param_attrs = vec![
        ParamAttributes {
            zeroext: true,
            ..ParamAttributes::default()
        },
        ParamAttributes {
            byval: Some(st),
            align: 16,
            ..ParamAttributes::default()
        },
    ];
    func.ret_attrs = vec![ParamAttributes {
        signext: true,
        ..ParamAttributes::default()
    }];

    let view = signature_view(&func).expect("投影");
    assert_eq!(view.params.len(), 2);
    assert_eq!(view.params[0].0, "n");
    assert!(view.attr(0).zeroext && view.attr(0).byval.is_none());
    assert_eq!(view.attr(1).byval, Some(16), "第二个形参的 byval 字节数");
    assert_eq!(view.attr(1).declared_align(), Some(16));
    assert!(view.ret_attrs.signext, "返回值属性也要投影");
    assert_eq!(view.ret.as_ref().map(|t| t.size), Some(4));
    assert!(view.attr(9).is_empty(), "没有属性的参数取默认值");
}
