//! 教程里的示例谱必须**真的能用**（v18 S7e）。
//!
//! `docs/guides/isa-dsl-tutorial.md` 的 §0 给出一份完整 TOY16 谱，读者会直接复制去跑。文档
//! 与代码一样会腐烂——所以这里把教程里那段 TOML **抽出来实跑**：解析 + 校验 + 展开，并断言
//! 教程承诺的几件事（`ops` 名 = 生成字段名、四条指令都在、`encode` 存在）。
//!
//! 这是"文档即测试"的最小形态：不改文档、不加依赖，只在文档改坏时变红。

use std::path::{Path, PathBuf};

use forge_isa_dsl::{ExpandOptions, Parts, expand_file, validate_source};

/// 仓库根：`crates/frontend/forge-isa-dsl` 上溯三级。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// 取文档里**第一个** ```` ```toml ```` 代码块（教程的 §0 完整谱）。
fn first_toml_block(md: &str) -> String {
    let mut lines = md.lines();
    let mut out: Vec<&str> = Vec::new();
    let mut inside = false;
    for l in &mut lines {
        if !inside {
            if l.trim() == "```toml" {
                inside = true;
            }
            continue;
        }
        if l.trim() == "```" {
            break;
        }
        out.push(l);
    }
    assert!(!out.is_empty(), "教程里没找到 ```toml 代码块");
    out.join("\n") + "\n"
}

#[test]
fn tutorial_spec_validates_and_expands() {
    let md = std::fs::read_to_string(repo_root().join("docs/guides/isa-dsl-tutorial.md"))
        .expect("读教程");
    let spec = first_toml_block(&md);

    // ① 校验通过（教程承诺"复制即可 validate OK"）。
    assert!(
        validate_source(&spec, Path::new("toy16.toml")).is_ok(),
        "教程里的 TOY16 谱校验失败"
    );

    // ② 展开成模块：四条指令 + 字段名来自 ops 的可见证据。
    let opts = ExpandOptions {
        spec_tests: false,
        name: None,
        parts: Parts::all(),
    };
    let tmp =
        std::env::temp_dir().join(format!("forge_tutorial_toy16_{}.toml", std::process::id()));
    std::fs::write(&tmp, &spec).expect("写临时谱");
    let ts = expand_file(tmp.to_str().unwrap(), &opts).expect("展开教程谱");
    let _ = std::fs::remove_file(&tmp);
    let flat: String = ts
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    assert!(flat.contains("pubenumInst"), "应有 Inst 枚举");
    for v in ["Add", "Sub", "Movi", "Brz"] {
        assert!(flat.contains(v), "教程里的指令 {v} 应出现在生成物里");
    }
    // 教程正文承诺：`ops = ["dst:g:out", "src:g"]` ⇒ `Inst::Add { dst, src }`。
    assert!(
        flat.contains("Add{dst:Reg,src:Reg}"),
        "字段名应来自 ops（教程中的说明与实现必须一致）"
    );
    assert!(flat.contains("pubfnencode("), "应生成编码器");
}
