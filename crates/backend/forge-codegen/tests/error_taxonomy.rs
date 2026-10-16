//! **错误分类学守卫**（v25 M2）。
//!
//! ## 病根
//!
//! `IrError::Unsupported` 曾一名三用：**能力缺口**（这台机器/这份约定/这份谱做不了）、
//! **畸形 IR / 内部不变量**（输入坏或代码 bug）、**使用错误**（约定没注册、装载 ISA 不匹配）。
//! 三者对用户的意义完全不同（换约定 / 报 bug / 改调用），混在一个变体里 ⇒ 台查看不见全貌、
//! CI 没法按类分流。而 `IrError::Unimplemented` 与 `Unsupported` 语义重叠（"没实现"就是
//! "做不了"），是纯粹的第二套说法。
//!
//! ## 判据
//!
//! 1. **后端与运行时的 `Unsupported` 一律是能力缺口**：`crates/backend/**` 与
//!    `crates/foundation/forge-isa-runtime/src/**` 里**不允许**出现裸的
//!    `IrError::Unsupported(`／`Unsupported(` 构造——只有 `IrError::capability(ID, …)` 一种出口
//!    （它自己会拼 `[cap:ID]`）。白名单条目必须**被命中**（仓库纪律），且写明理由。
//! 2. **第二套说法已删除**：全仓不再有 `Unimplemented` 变体（自跳过本文件——它必须写出这个词）。
//!
//! 畸形 IR ⇒ `IrError::Internal`；数据/使用错误 ⇒ `IrError::Semantic`；本守卫**不**替它们
//! 判断具体是哪种，只把"缺口必须有身份"这条钉死。

use std::collections::BTreeSet;

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// 允许出现裸 `Unsupported(` 的文件（相对仓库根）——每条都要有理由，且必须被命中。
const ALLOWED: [(&str, &str); 0] = [];

/// **后端的 `Unsupported` 一律是能力缺口**（源码级，注释行不算）。
#[test]
fn unsupported_is_always_a_capability() {
    let root = repo_root();
    let mut files = Vec::new();
    rs_files(&root.join("crates/backend"), &mut files);
    rs_files(
        &root.join("crates/foundation/forge-isa-runtime/src"),
        &mut files,
    );

    let mut hits: Vec<String> = Vec::new();
    let mut allowed_hit: BTreeSet<String> = BTreeSet::new();
    for p in files {
        let rel = p
            .strip_prefix(&root)
            .unwrap_or(&p)
            .to_string_lossy()
            .replace('\\', "/");
        // 只扫**生产代码**（`src/`）：`tests/` 里允许用裸 `Unsupported` 构造负例
        // （例如台账守卫的"普通失败不算缺口"那条单测）。
        if !rel.contains("/src/") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue; // 注释里提到它不算构造点
            }
            if !line.contains("Unsupported(") {
                continue;
            }
            if ALLOWED.iter().any(|(f, _)| *f == rel) {
                allowed_hit.insert(rel.clone());
                continue;
            }
            hits.push(format!("{rel}:{}: {}", i + 1, line.trim()));
        }
    }
    assert!(
        hits.is_empty(),
        "后端的 `Unsupported(` 必须是能力缺口（用 `IrError::capability(ID, …)`）；\n\
         畸形 IR ⇒ `Internal`，数据/使用错误 ⇒ `Semantic`：\n{}",
        hits.join("\n")
    );
    // 白名单条目必须被命中：死条目说明规则已不适用（仓库纪律）。
    let want: BTreeSet<String> = ALLOWED.iter().map(|(f, _)| (*f).to_string()).collect();
    assert_eq!(allowed_hit, want, "白名单条目必须被命中：{want:?}");
}

/// **`Unimplemented` 变体已删除**：全仓零出现（自跳过本文件）。
#[test]
fn second_variant_for_not_supported_is_gone() {
    let root = repo_root();
    let mut files = Vec::new();
    rs_files(&root.join("crates"), &mut files);
    let mut hits: Vec<String> = Vec::new();
    for p in files {
        if p.file_name().is_some_and(|n| n == "error_taxonomy.rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if line.contains("Unimplemented") {
                hits.push(format!(
                    "{}:{}: {}",
                    p.strip_prefix(&root).unwrap_or(&p).display(),
                    i + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "与 `Unsupported` 重叠的第二套说法回潮了：\n{}",
        hits.join("\n")
    );
}
