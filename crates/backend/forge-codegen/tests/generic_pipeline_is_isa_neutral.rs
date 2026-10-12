//! **通用管线不认识具体 ISA 的能力名**（v23 M3 的守卫）。
//!
//! ## 为什么钉这条
//!
//! 管线里曾直接写 `crate::avx512_available()`（"64 字节向量需 AVX-512F"）：与 ISA
//! 无关的代码却认识 x86 的能力名，于是"换一台机器/加一个后端"要改通用管线，而漏改
//! 一处就是**静默**的错判（要么拒绝合法代码，要么放进本机跑不动的 EVEX）。
//!
//! v23 M3 把这条判据换成能力数据：`TargetRegInfo::wide_vector_bytes()` —— ISA 后端回答
//! "这份谱申报的档位 ∩ 本机能执行的上界"（宿主探测留在 `forge_isa_runtime::cpu`）。
//! 本守卫钉住结果：**通用管线里不再出现 `avx*`/`sse*`/`neon`/`sve`**。
//!
//! ## 口径
//!
//! - 只扫 `src/pipeline/**`（**通用**管线）。`src/arch/**` 是 ISA 后端，允许认识自己的
//!   能力名；`src/runtime/jit.rs` 是执行层，本来就该按宿主能力 skip。
//! - 每个文件**在第一个 `#[cfg(test)]` 处截断**——测试里按平台探测/skip 是正当的。
//! - 跳过**注释行**：注释里解释历史（"这里以前问 avx512"）是允许的，能力名出现在
//!   **代码**里才是问题。
//! - 白名单条目必须被命中一次（防白名单腐烂）。

use std::path::{Path, PathBuf};

/// 白名单：(相对 `src/pipeline/` 的路径, 该行 trim 后的内容, 理由)。
const ALLOWED: &[(&str, &str, &str)] = &[];

/// 能力名判据：必须是**标识符级**匹配——`classes` 里的 `sse`、`assess` 里的 `sse`
/// 都不算（早期版本的子串判定在这条上误报过）。
fn hits_capability_name(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    for needle in ["avx", "sse", "neon", "sve"] {
        let mut from = 0;
        while let Some(rel) = lower[from..].find(needle) {
            let at = from + rel;
            let before_ok = at == 0
                || !lower.as_bytes()[at - 1].is_ascii_alphanumeric()
                    && lower.as_bytes()[at - 1] != b'_';
            let end = at + needle.len();
            // `avx` 允许继续接数字/字母（avx2/avx512f 是同一个标识符）；
            // `sse`/`neon`/`sve` 同样按前缀判（sse2/sve256）。
            let after_ok = end >= lower.len()
                || !lower.as_bytes()[end].is_ascii_alphabetic() && lower.as_bytes()[end] != b'_'
                || needle == "avx"
                || needle == "sse"
                || needle == "sve";
            if before_ok && after_ok {
                return true;
            }
            from = end;
        }
    }
    false
}

fn scan_dir(root: &Path, dir: &Path, out: &mut Vec<(String, usize, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            scan_dir(root, &p, out);
            continue;
        }
        if p.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let rel = p
            .strip_prefix(root)
            .unwrap_or(&p)
            .to_string_lossy()
            .replace('\\', "/");
        for (i, line) in text.lines().enumerate() {
            // 第一个 `#[cfg(test)]` 之后不再扫（测试可以按平台探测）。
            if line.trim_start().starts_with("#[cfg(test)]") {
                break;
            }
            let code = line.trim_start();
            if code.starts_with("//") || code.starts_with("*") || code.starts_with("/*") {
                continue;
            }
            if hits_capability_name(line) {
                out.push((rel.clone(), i + 1, line.trim().to_string()));
            }
        }
    }
}

#[test]
fn the_generic_pipeline_never_names_a_specific_isa_capability() {
    let root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pipeline");
    let mut hits = Vec::new();
    scan_dir(&root, &root, &mut hits);
    assert!(hits.len() < 200, "扫描疑似失效（命中过多）：{hits:?}");

    let mut used_allow: Vec<usize> = Vec::new();
    let mut violations: Vec<String> = Vec::new();
    for (file, line, text) in &hits {
        match ALLOWED.iter().position(|(f, t, _)| file == f && text == t) {
            Some(i) => used_allow.push(i),
            None => violations.push(format!("{file}:{line}: {text}")),
        }
    }
    assert!(
        violations.is_empty(),
        "通用管线出现具体 ISA 的能力名（请改为问能力：TargetRegInfo::wide_vector_bytes / \
         TargetMachine::features，宿主探测留在 forge_isa_runtime::cpu）：\n{}",
        violations.join("\n")
    );
    for (i, (f, t, why)) in ALLOWED.iter().enumerate() {
        assert!(
            used_allow.contains(&i),
            "白名单条目已失效（未被命中），请删除：{f} :: `{t}`（理由曾是：{why}）"
        );
    }
}
