//! 开放集合**边界**守卫（v3 方案 S5 第 2 项）。
//!
//! # 边界三分
//!
//! - **(a) 闭合集合**：由单一事实源闭死，IR 侧不应出现第二份定义——opcode
//!   （`ops.toml` 生成）、`Icmp/Fcmp` 条件码、指令类别/效果、终结符种类、
//!   `MetadataKind` 的 well-known 项。
//! - **(b) 目标/ISA 数据**：开放，但值由 ISA/目标数据声明——寄存器类与宽度、
//!   栈槽与对齐、指令字宽、pattern 名与 isel 标签、`target triple` 各段。
//! - **(c) 用户程序数据**：开放——函数/块/值/全局/结构体/`section` 名、
//!   metadata 自定义 kind、`Immediate::String`、`source_filename`、`module asm`。
//!
//! # 规则（本文件钉住的正是这条）
//!
//! **不得从 (b)/(c) 的字符串反推 (a) 或任何数值**——那是宿主白名单，表外取值
//! 会得到静默错误答案（历史实例：`TargetTriple::is_32bit/is_64bit/os_name`
//! 三个架构名查表，表外架构两个都返回 `false`；`is_64bit` 曾与 ISA 声明的
//! `addr_width` 各说各话）。数值一律来自 ISA 数据：指针宽度 = `DataLayout` 的
//! `p:<size>:<abi>`（ISA `[meta] addr_width` 派生）。
//!
//! 另：**(b)/(c) 的字符串数据一律用 `ImmStr`**（SSO 内联 + `Arc<str>` 共享，
//! `Clone` O(1)、跨函数/模块共享不深拷贝），IR 公开面不出现裸 `String` 字段。
//!
//! 守卫分两层：行为断言（未知架构名原样往返、宽度只跟布局字符串走）+ 源码断言
//! （`src/` 里不得重现代码表、公开面不得出现 `String` 字段）。

use forge_ir::data_layout::{DataLayout, TargetTriple};
use forge_ir::text::parser::parse_module;

/// 读 `src/**/*.rs`，返回 (相对路径, 去掉 `#[cfg(test)]` 尾部的主体)。
fn src_bodies() -> Vec<(String, String)> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let Ok(text) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let rel = p
                    .strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                let body = text
                    .split("#[cfg(test)]")
                    .next()
                    .unwrap_or(&text)
                    .to_string();
                out.push((rel, body));
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out
}

/// 行为断言 ①：三元组只作数据——未知架构/厂商/OS 原样保留并往返。
#[test]
fn target_names_are_open_data_roundtripping_unknown_arches() {
    let t = TargetTriple::parse("loongarch64-acme-none-elf");
    assert_eq!(t.arch.as_str(), "loongarch64");
    assert_eq!(t.vendor.as_str(), "acme");
    assert_eq!(t.os.as_str(), "none");
    assert_eq!(t.environment.as_str(), "elf");
    assert_eq!(t.to_string(), "loongarch64-acme-none-elf");

    // 文本层同样不改写：parse → display → parse 稳定
    let src = "target triple = \"loongarch64-acme-none-elf\"\nsource_filename = \"odd name.c\"\nmodule asm \"nop\"\n\
               define i32 @f() {\n  %e:\n    ret i32 0\n}\n";
    let m = parse_module(src).expect("parse");
    let text = format!("{}", m);
    assert!(
        text.contains("target triple = \"loongarch64-acme-none-elf\""),
        "未知架构名应原样输出：\n{text}"
    );
    assert!(
        text.contains("source_filename = \"odd name.c\""),
        "source_filename 应原样输出：\n{text}"
    );
    assert!(
        text.contains("module asm \"nop\""),
        "module asm 应原样输出：\n{text}"
    );
    let m2 = parse_module(&text).expect("reparse");
    assert_eq!(m2.target_triple, m.target_triple, "三元组往返稳定");
}

/// 行为断言 ②：指针宽度只跟 `DataLayout` 走，与三元组/架构名无关。
///
/// 这条守卫针对的是"按架构名猜宽度"的回潮：`TargetTriple` 现在**没有任何**
/// 宽度查询（类型级事实），宽度唯一来源是 ISA 数据派生的布局字符串。
#[test]
fn pointer_width_comes_from_layout_not_arch_name() {
    let dl32 = DataLayout::parse("e-m:e-p:32:32-i64:64-n8:16:32").expect("parse p:32");
    assert_eq!(dl32.pointer_size(0), 4);
    assert_eq!(dl32.pointer_align(0), 4);

    let dl64 = DataLayout::parse("e-m:e-p:64:64-i64:64-n8:16:32:64").expect("parse p:64");
    assert_eq!(dl64.pointer_size(0), 8);

    // 同一布局字符串挂在**任意** triple 下宽度不变（未知架构也一样）
    for name in ["x86_64-unknown-linux-gnu", "loongarch64-acme-none-elf"] {
        let mut m = forge_ir::Module::new();
        m.set_target_triple(name);
        m.set_data_layout(dl32.clone());
        assert_eq!(
            m.data_layout().pointer_size(0),
            4,
            "{name} 的指针宽度必须来自布局数据"
        );
        assert_eq!(
            m.target_triple.as_ref().map(|t| t.arch.as_str()),
            Some(name.split('-').next().unwrap())
        );
    }
}

/// 源码断言 ①：`src/` 不得再现"按架构名/OS 名查表"的宿主代码表。
#[test]
fn no_host_arch_or_os_tables_in_src() {
    // 架构名以**带引号的字面量**出现在生产代码里，就是查表回潮的信号
    // （文档注释与 `#[cfg(test)]` 尾部已排除）。
    const ARCH_LITERALS: &[&str] = &[
        "\"x86_64\"",
        "\"aarch64\"",
        "\"arm64\"",
        "\"riscv64\"",
        "\"riscv32\"",
        "\"wasm32\"",
        "\"i386\"",
        "\"i686\"",
        "\"armv7\"",
        "\"thumbv7\"",
        "\"powerpc64\"",
        "\"mips64\"",
        "\"sparc64\"",
        "\"s390x\"",
    ];
    const OS_NAME_QUERIES: &[&str] = &["fn is_32bit", "fn is_64bit", "fn os_name"];

    let mut violations = Vec::new();
    for (file, body) in src_bodies() {
        for (i, line) in body.lines().enumerate() {
            let t = line.trim();
            if t.starts_with("//") {
                continue;
            }
            if let Some(hit) = ARCH_LITERALS.iter().find(|l| t.contains(**l)) {
                violations.push(format!("{file}:{}: 架构名字面量 {hit} → {t}", i + 1));
            }
            if let Some(hit) = OS_NAME_QUERIES.iter().find(|q| t.contains(**q)) {
                violations.push(format!("{file}:{}: 目标查询 {hit} → {t}", i + 1));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "不得把目标名当查表键（宽度/OS 等属性请从 ISA 数据取：DataLayout / \
         `[meta] addr_width` / `[abi]`）：\n{}",
        violations.join("\n")
    );
}

/// 源码断言 ②：IR **公开面**不得有裸 `String` 字段（开放字符串数据统一 `ImmStr`）。
///
/// 覆盖边界：只看 `pub` 字段声明（`src/text/parser/**` 是解析期 AST，按设计持有
/// 拥有的 `String`，不在此列）；诊断消息里的 `String` 是瞬态数据、字段非 `pub`，
/// 也不在此列。`StringPool` 自身是 interner 实现，不是字段。
#[test]
fn no_plain_string_fields_on_public_ir_surface() {
    const ALLOWED: &[(&str, &str, &str)] = &[];

    let mut hits: Vec<(String, usize, String)> = Vec::new();
    for (file, body) in src_bodies() {
        if file.starts_with("text/parser/") {
            continue; // 解析期 AST：拥有的 String 是设计选择
        }
        for (i, line) in body.lines().enumerate() {
            let t = line.trim();
            if t.starts_with("//") || t.contains("fn ") || !t.contains("pub ") {
                continue;
            }
            let is_string_field = [
                ": String,",
                ": Option<String>,",
                ": Vec<String>,",
                ": Box<str>,",
                ": Arc<str>,",
            ]
            .iter()
            .any(|p| t.contains(p))
                || t.ends_with(": String")
                || t.ends_with(": Option<String>")
                || t.ends_with(": Vec<String>");
            if is_string_field {
                hits.push((file.clone(), i + 1, t.to_string()));
            }
        }
    }

    let mut used = Vec::new();
    let mut violations = Vec::new();
    for (file, line, text) in &hits {
        match ALLOWED.iter().position(|(f, t, _)| f == file && t == text) {
            Some(i) => used.push(i),
            None => violations.push(format!("{file}:{line}: {text}")),
        }
    }
    assert!(
        violations.is_empty(),
        "IR 公开面的开放字符串数据请用 ImmStr（SSO + Arc 共享，Clone O(1)）：\n{}",
        violations.join("\n")
    );
    for (i, (f, t, why)) in ALLOWED.iter().enumerate() {
        assert!(
            used.contains(&i),
            "白名单条目已失效（未被命中），请删除：{f} :: `{t}`（理由曾是：{why}）"
        );
    }
}
