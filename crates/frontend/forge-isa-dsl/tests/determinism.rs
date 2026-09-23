//! 生成物**确定性**守卫（v19 V6a）：同一份谱重复生成必须逐字节相同。
//!
//! 为什么值得钉：生成器内部有大量"按名字查表"的路径（位域、模板展开、`vary` 行、寄存器组…）。
//! 一旦哪张表从 `BTreeMap` 换成 `HashMap` 的迭代序、或引入时间戳/指针地址，生成物就会在两次
//! 构建之间抖动——后果是缓存失效（每轮重编）、"我这边能编过、CI 编不过"这类难查事故，
//! 而 `cargo test` 却可能全绿（每次跑的是当次生成的那份）。
//!
//! 判据两条（都不依赖任何 ISA 细节）：
//!
//! 1. **内容**：同参数展开两次，token 文本逐字节相等；
//! 2. **文件名**：`generated_file_name` 只由参数决定（同参数恒定、`spec_tests` 变体不同名，
//!    否则同一份谱的两个变体会互相覆盖——S10d 的不变量）。
//!
//! 覆盖面：三份发行谱 + 全部**可独立展开**的测试夹具（跳过只作 `include` 分片的那些）。

use std::path::{Path, PathBuf};

use forge_isa_dsl::gen_file::generated_file_name;
use forge_isa_dsl::{ExpandOptions, Parts, expand_file};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// 全开部件 + 关掉自测（自测只是多出一大段，与确定性无关，关掉让这个守卫快）。
fn opts(spec_tests: bool) -> ExpandOptions {
    ExpandOptions {
        spec_tests,
        name: None,
        parts: Parts::all(),
    }
}

/// 展开两次 ⇒ token 文本必须逐字节相同；文件名同参数恒定、不同参数不同名。
fn assert_stable(path: &str) {
    let a = expand_file(path, &opts(false)).unwrap_or_else(|e| panic!("展开 {path} 失败：{e}"));
    let b = expand_file(path, &opts(false)).unwrap_or_else(|e| panic!("再展开 {path} 失败：{e}"));
    assert_eq!(a.to_string(), b.to_string(), "{path} 两次展开的生成物不同");
    assert_eq!(
        generated_file_name(path, &opts(false)),
        generated_file_name(path, &opts(false)),
        "{path} 的文件名不稳定"
    );
    assert_ne!(
        generated_file_name(path, &opts(false)),
        generated_file_name(path, &opts(true)),
        "{path}：spec_tests 变体应落到不同文件名（否则互相覆盖）"
    );
}

#[test]
fn shipped_specs_generate_byte_identical_output() {
    for isa in ["x86_v12.toml", "arm64_v12.toml", "riscv64_v12.toml"] {
        assert_stable(&root().join("isa").join(isa).to_string_lossy());
    }
}

#[test]
fn fixture_specs_generate_byte_identical_output() {
    // 只列**可独立展开**的根谱；`include_*_base/frag` 那类分片单独展开本来就不合法。
    let fixtures = [
        "demo_v12.toml",
        "demo8_v12.toml",
        "demo_mixed16_32_v12.toml",
        "demo_inst8_v12.toml",
        "demo_inst12_v12.toml",
        "demo_inst100_v12.toml",
        "include_root_v12.toml",
    ];
    let dir = root().join("crates/backend/forge-codegen/tests/isa");
    for f in fixtures {
        let p = dir.join(f);
        assert!(p.is_file(), "夹具缺失：{}", p.display());
        assert_stable(&p.to_string_lossy());
    }
}

/// 同一份谱**不同变体**（部件不同）也必须各自稳定，且彼此不同名。
#[test]
fn restricted_parts_are_stable_too() {
    let spec = root()
        .join("crates/backend/forge-codegen/tests/isa/include_root_v12.toml")
        .to_string_lossy()
        .to_string();
    let enc_only = ExpandOptions {
        spec_tests: false,
        name: Some("stab_enc_only".into()),
        parts: Parts::from_names(&["encode".to_string()]).expect("部件名"),
    };
    let a = expand_file(&spec, &enc_only).expect("展开 enc-only");
    let b = expand_file(&spec, &enc_only).expect("再展开 enc-only");
    assert_eq!(a.to_string(), b.to_string(), "enc-only 变体不稳定");
    assert_ne!(
        generated_file_name(&spec, &enc_only),
        generated_file_name(&spec, &opts(false)),
        "变体必须与全开件不同名"
    );
}
