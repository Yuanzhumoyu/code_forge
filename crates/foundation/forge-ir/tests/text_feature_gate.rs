//! `features = ["text"]` 门控守卫（v3 方案 S7）。
//!
//! 文本层（`text::parser` = logos 词法 + lalrpop 语法 + 语义构建；`text::display` =
//! LLVM 文本打印）由 cargo feature `text`（默认开启）门控：关闭后 forge-ir 只剩内存
//! IR，不再依赖 `logos`/`lalrpop-util`，`build.rs` 也不再生成 LALRPOP 表。
//!
//! **为什么需要守卫**：这个边界光靠"能编译"验证不了——只要有一个**核心**文件
//! 引用文本层类型（改前确实是：`GlobalVariable::init_expr` 存解析层 `ConstExpr`），
//! 关闭 feature 就编不过；而没人天天跑 `--no-default-features`，这种耦合会悄悄
//! 长回来。所以这里做**源码级**断言（与 `dfg_privatization.rs` 同一风格）：
//!
//! 1. 核心文件（`src/**` 去掉 `lib.rs` 与文本层目录 `src/text/`）不得出现
//!    `text::parser`/`text::display`/`crate::text::` 字样（**含注释**——注释里提它就
//!    意味着耦合意图）；
//! 2. `lib.rs` 里两个模块声明必须**恰好一次**且紧跟 `#[cfg(feature = "text")]`；
//! 3. `Cargo.toml` 里 `text` 的依赖必须是 `dep:` 形式且两个运行时依赖 `optional`；
//! 4. `build.rs` 里 LALRPOP 生成被 `CARGO_FEATURE_TEXT` 包住，而指令元数据
//!    （`Opcode` 表，核心也读）**不得**被门控。
//!
//! 行为侧证据不在测试里（测试进程内再跑 cargo 会撞锁）：门禁命令是
//! `cargo check -p forge-ir --no-default-features --lib`（`--lib` 是必须的——
//! `tests/*.rs` 大量使用文本层，它们本来就需要 `text`），已进 CI。
//! 本文件自身不引用文本层，因此带不带 `text` 都能编译。

use std::path::{Path, PathBuf};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(manifest_dir().join(rel))
        .unwrap_or_else(|e| panic!("读不到 {rel}：{e}"))
}

/// 收集 `src/` 下的 `.rs`（跳过文本层本体 `src/text/`），返回相对 `src/` 的路径。
fn collect_core_sources(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("读不到 {}：{e}", dir.display()));
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "text") {
                continue; // 文本层本体（`src/text/`：parser + display）
            }
            collect_core_sources(&p, root, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p.strip_prefix(root).unwrap_or(&p).to_path_buf());
        }
    }
}

/// **守卫 1**：核心文件不得引用文本层。
#[test]
fn core_sources_do_not_reference_the_text_layer() {
    let src = manifest_dir().join("src");
    let mut files = Vec::new();
    collect_core_sources(&src, &src, &mut files);
    assert!(
        files.len() > 20,
        "核心源文件数异常（{} 个）——遍历逻辑可能失效",
        files.len()
    );

    let mut hits = Vec::new();
    for rel in &files {
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        // `lib.rs` 持有模块声明（守卫 2 单独钉）；文本层本体已在遍历时跳过。
        if rel_str == "lib.rs" {
            continue;
        }
        let text = read(&format!("src/{rel_str}"));
        for (i, line) in text.lines().enumerate() {
            for needle in ["text::parser", "text::display", "crate::text::"] {
                if line.contains(needle) {
                    hits.push(format!("src/{rel_str}:{}: {}", i + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "核心文件引用了文本层——关闭 `features=[\"text\"]` 就编不过（核心只许经不透明文本载荷\
         与文本层交互，例如 `GlobalVariable::init_expr_text`）:\n{}",
        hits.join("\n")
    );
}

/// **守卫 2**：`lib.rs` 的 `pub mod text;` 恰好一次，且紧跟 `#[cfg(feature = "text")]`。
///
/// 目录归类后 crate 里**唯一**的门控点就是它（`src/text/{parser,display}` 整块被门控）。
#[test]
fn text_modules_are_feature_gated() {
    let lib = read("src/lib.rs");
    let lines: Vec<&str> = lib.lines().collect();
    let decl = "pub mod text;";
    assert_eq!(
        lib.matches(decl).count(),
        1,
        "`{decl}` 应恰好声明一次（重复声明意味着可能有一处没门控）"
    );
    let idx = lines
        .iter()
        .position(|l| l.trim() == decl)
        .unwrap_or_else(|| panic!("lib.rs 里找不到 `{decl}`"));
    // 往上找最近的非空、非注释行——它必须就是 cfg 属性（文档注释允许夹在中间）。
    let prev = lines[..idx]
        .iter()
        .rev()
        .map(|l| l.trim())
        .find(|t| !t.is_empty() && !t.starts_with("//"))
        .unwrap_or_else(|| panic!("`{decl}` 之前没有代码行"));
    assert_eq!(
        prev, "#[cfg(feature = \"text\")]",
        "`{decl}` 必须由 `#[cfg(feature = \"text\")]` 门控（实测上一行：`{prev}`）"
    );
    // 文本层是**目录**：`src/text/{parser,display}` 整块在门控之内 ⇒ 核心目录里不许再有
    // 独立的 `display.rs`/`ir_parser.rs`（防止有人把文件搬回来绕过门控）。
    for stale in ["src/display.rs", "src/ir_parser.rs"] {
        assert!(
            !manifest_dir().join(stale).exists(),
            "`{stale}` 不该存在——文本层统一在 `src/text/`（目录归类 + 门控唯一入口）"
        );
    }
}

/// **守卫 3**：manifest 声明了 `text`（默认开启），两个运行时依赖可选。
#[test]
fn manifest_declares_optional_text_feature() {
    let cargo = read("Cargo.toml");
    for want in [
        "default = [\"text\"]",
        "text = [\"dep:logos\", \"dep:lalrpop-util\"]",
    ] {
        assert!(cargo.contains(want), "Cargo.toml 缺 `{want}`");
    }
    // `optional = true`（否则 `dep:` 语法直接构建期报错）——两个运行时依赖各一次。
    assert_eq!(
        cargo.matches("optional = true").count(),
        2,
        "`logos` 与 `lalrpop-util` 都必须 `optional = true`"
    );
    assert!(
        !cargo.contains("\nlogos = \"") && !cargo.contains("\nlalrpop-util = \""),
        "`logos`/`lalrpop-util` 不得是必选依赖（否则 `--no-default-features` 仍会拉进来）"
    );
}

/// **守卫 4**：`build.rs` 里 LALRPOP 生成被 `CARGO_FEATURE_TEXT` 包住；
/// 指令元数据生成（`Opcode` 表——核心也读）**不得**被门控。
#[test]
fn build_script_gates_lalrpop_only() {
    let build = read("build.rs");
    let lines: Vec<&str> = build.lines().collect();
    let gate = lines
        .iter()
        .position(|l| l.contains("CARGO_FEATURE_TEXT"))
        .expect("build.rs 必须读 CARGO_FEATURE_TEXT（`lalrpop` 是 build-dependency、不可选）");
    let body = lines[gate + 1..]
        .iter()
        .take_while(|l| l.trim() != "}")
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains("lalrpop::process_root()"),
        "LALRPOP 生成应在 `CARGO_FEATURE_TEXT` 块内（实测块体：{body}）"
    );
    assert!(
        !body.contains("generate_opcode_table"),
        "指令元数据生成不得被门控——核心（`Opcode` 枚举/派生表/名字查找）也读它"
    );
    assert_eq!(
        build.matches("generate_opcode_table();").count(),
        1,
        "元数据生成应恰好调用一次"
    );
}
