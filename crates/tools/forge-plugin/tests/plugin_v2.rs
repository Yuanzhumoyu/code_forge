//! v28 M3：真实 dylib 插件的加载 + 编译 + 执行（宿主零 ISA 代码）。
//!
//! 夹具 `tests/fixtures/demo_plugin` 只做两件事：`isa_from_file!` + 一行
//! `export_isa_plugin!`。本测试构建它、用 `PluginLoader` 加载，再通过
//! `Registry::lookup("demoplugin")` 编一个函数并真执行。
//!
//! 构建走独立 `target/plugin-fixture`（避免与外层 cargo 抢同一 target 锁）；
//! 首次约 1–2 分钟，之后增量。`--offline` 依赖夹具自带的 `Cargo.lock`。

use std::path::{Path, PathBuf};
use std::process::Command;

use forge_codegen::ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};
use forge_codegen::target_registry;
use forge_isa_runtime::mem::ExecutableMemory;
use forge_plugin::PluginLoader;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// 构建夹具并返回 dylib 路径；已存在则直接复用。
fn fixture_dylib() -> PathBuf {
    let fixture = repo_root().join("crates/tools/forge-plugin/tests/fixtures/demo_plugin");
    let target_dir = repo_root().join("target/plugin-fixture");
    let dll = target_dir.join("debug").join(format!(
        "forge_plugin_fixture{}",
        std::env::consts::DLL_SUFFIX
    ));
    // 每次都跑 cargo（增量；源文件变了也能刷新），避免复用旧 dylib 造成假绿。
    let out = Command::new("cargo")
        .args([
            "build",
            "--offline",
            "--manifest-path",
            fixture.join("Cargo.toml").to_str().unwrap(),
            "--target-dir",
            target_dir.to_str().unwrap(),
            "-j",
            "2",
        ])
        .env("CARGO_INCREMENTAL", "0")
        .output()
        .expect("无法启动 cargo 构建插件夹具");
    assert!(
        out.status.success(),
        "插件夹具构建失败：\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dll.exists(), "构建成功但找不到 dylib：{}", dll.display());
    dll
}

#[test]
fn v2_plugin_loads_compiles_and_runs() {
    let dll = fixture_dylib();
    let mut loader = PluginLoader::new();
    loader
        .load_plugin(&dll)
        .unwrap_or_else(|e| panic!("加载插件 {} 失败：{e}", dll.display()));

    assert_eq!(
        target_registry::isa_for_target("demoplugin64-unknown-none"),
        Some("demoplugin".to_string()),
        "插件目标前缀必须进宿主 target_registry"
    );

    let backend = forge_isa_runtime::Registry::global()
        .lookup("demoplugin")
        .expect("宿主 Registry 里应出现插件后端");

    let sig = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut b = FunctionBuilder::new("plugin_answer", TypeContext::new(), sig);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let v = b.const_int(42i64, TypeId::I64);
    b.ret(&[v]);
    let func = b.finish().expect("IR 构建");

    let cf = backend.compile(&func).expect("插件后端必须能编译这个函数");
    assert!(!cf.code.is_empty(), "插件编译产物不能为空");
    let mem = ExecutableMemory::new(&cf.code).expect("可执行内存");
    let f: extern "C" fn() -> i64 = unsafe { mem.get_fn(0).expect("入口") };
    assert_eq!(f(), 42, "插件编出的函数必须真执行出 42");
}
