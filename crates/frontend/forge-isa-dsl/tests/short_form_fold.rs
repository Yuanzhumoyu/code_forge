//! S8c 结构不变量（v18 S8）：生成物里的长路径/长调用只走**短名**。
//!
//! 生成器在最后一道 token 后处理里做纯等价折叠（`fold_short_forms`）：
//! `Reg :: from_index (…)` → `__ph (…)`、`< Reg as forge_ir :: PhysReg > :: to_index (…)`
//! → `__ti (…)`、`forge_ir :: RegClass` → `__RC`。这三处是生成物里出现最多的长文本
//! （x86 各 1,700 / 1,400 处），折叠后 `all` −5.5%、生成期自测那部分 −10%（x86）。
//!
//! 这里钉住：① 长形**一处不剩**（谁在生成器里新写一处就会被抓）；② 短名定义只发射一次；
//! ③ 短名定义里的路径也走固定根改写（v19 V1b 起一律指 `forge_isa_runtime`，否则
//! 会指向不存在的 `forge_ir`）。

use forge_isa_dsl::{ExpandOptions, Parts, expand_file};

const SPECS: [(&str, &str); 3] = [
    ("x86_v12", "isa/x86_v12.toml"),
    ("riscv64_v12", "isa/riscv64_v12.toml"),
    ("arm64_v12", "isa/arm64_v12.toml"),
];

fn text_of(path: &str, _krate: Option<&str>) -> String {
    let opts = ExpandOptions {
        spec_tests: false,
        name: None,
        parts: Parts::all(),
        params: Default::default(),
    };
    expand_file(path, &opts)
        .unwrap_or_else(|e| panic!("展开 {path} 失败：{e}"))
        .to_string()
}

/// 去掉短名定义那几行（它们按设计写全路径、发射在生成主体之前），只留生成主体。
fn body_after_helpers(text: &str) -> &str {
    match text.split_once("fn __ti (r : Reg) -> u32") {
        Some((_, after)) => after,
        None => text,
    }
}

#[test]
fn long_forms_are_folded_away() {
    for (name, path) in SPECS {
        let text = text_of(path, None);
        let body = body_after_helpers(&text);
        for long in [
            "Reg :: from_index (",
            "forge_ir :: RegClass",
            "< Reg as forge_ir :: PhysReg > :: to_index (",
        ] {
            let n = body.matches(long).count();
            assert_eq!(
                n, 0,
                "{name}: 折叠后生成主体里仍残留 {n} 处 `{long}`——生成器里新写的这一处没走短名，\
                 生成物会白长回去"
            );
        }
        assert!(
            text.contains("__ph ("),
            "{name}: 整份生成物里没有一处 `__ph (`，折叠大概没生效"
        );
        // 短名定义各一次（发射在模块开头）
        for item in ["type __RC = ", "fn __ph (", "fn __ti ("] {
            assert_eq!(
                text.matches(item).count(),
                1,
                "{name}: `{item} …` 应只发射一次"
            );
        }
    }
}

#[test]
fn short_forms_are_rewritten_to_runtime_root() {
    // 跨 crate 生成（tests/ 夹具的形态）：短名定义里的 `forge_ir::` 也必须改写成
    // `forge_isa_runtime::ir::`，否则生成物里根本没有 `forge_ir` 这个名字。
    // 注意文档注释里的 `forge_ir::RegClass` 是字符串（不带 token 空格），不算。
    let path = "crates/backend/forge-codegen/tests/isa/demo_v12.toml";
    let text = text_of(path, Some("forge_codegen"));
    assert_eq!(
        text.matches("forge_ir :: ").count(),
        0,
        "生成物里仍有 token 形态的 `forge_ir ::`（含短名定义）——\
         路径改写没覆盖到"
    );
    assert!(
        text.contains("forge_isa_runtime :: ir :: RegClass"),
        "短名定义 `type __RC = …RegClass;` 没有跟着改写到宿主 crate"
    );
    assert!(text.contains("__ph ("), "夹具生成物里也没有短名调用");
}
