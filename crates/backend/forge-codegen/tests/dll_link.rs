//! v28 M5：对象层最小 DLL 链接/导出检查（Windows x64）。
//!
//! `forge-rustc` 的 `crate-type=cdylib` 仍 fail-closed（需要完整 PIC/GOT/PLT 与
//! 动态段建模）；这里验证**对象层**能产出可被 `lld-link /DLL` 链接、且导出符号
//! 可见的最小产物，作为动态产物的下限证据。超出该下限的形态继续明确拒绝。

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use forge_codegen::CompileSession;
use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

#[test]
fn object_links_into_dll_and_exports_symbol() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("target/tmp/dll_link");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let obj = dir.join("dll_link.obj");
    let dll = dir.join("dll_link.dll");

    // 1) 编一个不访问数据的最小函数。
    let sig = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut b = FunctionBuilder::new("f", TypeContext::new(), sig);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let v = b.const_int(42i64, TypeId::I64);
    b.ret(&[v]);
    let func = b.finish().expect("IR build");
    let cf = CompileSession::new(forge_codegen::x86::TargetMachine::new())
        .compile(&func)
        .expect("x86 compile");
    assert!(!cf.code.is_empty());

    // 2) 写 COFF 对象。
    let config =
        forge_object::TargetConfig::from_triple("x86_64-pc-windows-msvc").expect("win triple");
    let mut writer = forge_object::ObjectWriter::new(&config).expect("writer");
    writer.add_function("f", &cf).expect("add function");
    std::fs::write(&obj, writer.write_to_vec().expect("object bytes")).expect("写对象");

    // 3) 链成 DLL，强制导出 f。
    let out = Command::new("lld-link")
        .args([
            "/DLL",
            "/NOENTRY",
            "/EXPORT:f",
            "/NOLOGO",
            &format!("/OUT:{}", dll.display()),
            obj.to_str().unwrap(),
        ])
        .output()
        .expect("需要 lld-link（LLVM 工具链）");
    assert!(
        out.status.success(),
        "lld-link 失败：\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dll.exists(), "DLL 未生成");

    // 4) 导出表必须含 f。
    let out = Command::new("llvm-readobj")
        .args(["--coff-exports", dll.to_str().unwrap()])
        .output()
        .expect("需要 llvm-readobj（LLVM 工具链）");
    assert!(out.status.success(), "llvm-readobj 失败");
    let exports = String::from_utf8_lossy(&out.stdout);
    assert!(
        exports.contains("Name: f"),
        "DLL 导出表里没有 f：\n{exports}"
    );
}
