//! 语料扫描与**行抽取**：把上游文件切成"能喂给汇编器的那些行"。
//!
//! 上游语料写给人看（注释、伪指令、宏、预处理、一行多语句），汇编器只吃指令行。
//! 这里按 **suite 档案**（注释前缀 / 语句分隔符）把文件归一化成
//! [`Line`]（去掉注释、拆开一行多语句、剥掉标签前缀）；**不改语料、不改谱**。

use std::path::{Path, PathBuf};

/// 一套语料的方言档案。语料文件本身保持上游原文，方言知识写在这里（可评审）。
pub struct Suite {
    /// 报告与棘轮里用的键。
    pub key: &'static str,
    pub isa: &'static str,
    /// 相对 `asm/parse/` 的目录。
    pub dir: &'static str,
    /// 注释前缀（**行内**也剥：取最左出现位置截断）。
    pub comments: &'static [&'static str],
    /// 一行多语句的分隔符（arm64 的 `%%`）。
    pub stmt_sep: Option<&'static str>,
    /// 上游来源（URL + 固定 ref），与 `PROVENANCE.md` 一致。
    pub source: &'static str,
    /// 许可（SPDX）。
    pub license: &'static str,
}

/// 三份发行谱当前的解析档语料（vendored 集）。
pub const SUITES: &[Suite] = &[
    Suite {
        key: "gnu-gas-intel",
        isa: "x86",
        dir: "x86/gnu-gas-intel",
        comments: &["#", "//"],
        stmt_sep: None,
        source: "binutils-gdb gas/testsuite/gas/i386/intel.s（Intel 方言；镜像 ahjragaas/binutils-gdb@master）",
        license: "GPL-3.0-or-later",
    },
    Suite {
        key: "llvm-mc",
        isa: "riscv64",
        dir: "riscv64/llvm-mc",
        comments: &["#", "//"],
        stmt_sep: None,
        source: "llvm/llvm-project@llvmorg-19.1.0 llvm/test/MC/RISCV/",
        license: "Apache-2.0 WITH LLVM-exception",
    },
    Suite {
        key: "llvm-mc",
        isa: "aarch64",
        dir: "aarch64/llvm-mc",
        comments: &[";", "//", "#"],
        stmt_sep: Some("%%"),
        source: "llvm/llvm-project@llvmorg-19.1.0 llvm/test/MC/AArch64/",
        license: "Apache-2.0 WITH LLVM-exception",
    },
];

/// 语料根：`<crate>/asm`（`CARGO_MANIFEST_DIR` 由宿主保证存在）。
pub fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("asm")
}

/// 一行"候选指令"（已归一化）。
///
/// **不带"助记符"字段**：模板可以操作数前置、首段可以是多 token 字面，切词猜出来的
/// "助记符"没有意义（见 `classify` 的桶判定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// 相对语料根的路径（报告里点名用）。
    pub file: String,
    /// 1-based 行号（指回上游文件）。
    pub line_no: usize,
    /// 指令文本（无注释、无标签前缀、单条语句）。
    pub text: String,
}

/// 一份语料文件里抽出来的东西。
#[derive(Debug, Default)]
pub struct Extracted {
    /// 候选指令行。
    pub lines: Vec<Line>,
    /// 标签定义（`name:`）——解析某一行时作上下文喂进去，解掉 `UndefinedLabel`。
    pub labels: String,
    /// 被丢掉的行数（注释/伪指令/宏/预处理/空行）。
    pub skipped: usize,
}

/// 剥注释：取最左的注释前缀位置截断（同一行内）。
pub(crate) fn strip_comment<'a>(line: &'a str, comments: &[&str]) -> &'a str {
    let mut cut = line.len();
    for c in comments {
        if let Some(i) = line.find(c) {
            cut = cut.min(i);
        }
    }
    &line[..cut]
}

/// 这一行是（整行）注释吗？是则返回注释正文（去掉前缀）。
pub(crate) fn comment_body<'a>(line: &'a str, comments: &[&str]) -> Option<&'a str> {
    comments
        .iter()
        .find_map(|c| line.strip_prefix(*c).map(str::trim_start))
}

/// 是伪指令/预处理器/宏行吗（`.equ`/`#define`/`.macro`…）。
fn is_directive(t: &str) -> bool {
    t.starts_with('.')
        || t.starts_with("#define")
        || t.starts_with("#include")
        || t.starts_with('#')
}

/// 把一段上游源码切成候选指令行。
pub fn extract(suite: &Suite, file: &str, src: &str) -> Extracted {
    let mut out = Extracted::default();
    for (i, raw) in src.lines().enumerate() {
        let line_no = i + 1;
        let body = strip_comment(raw, suite.comments).trim();
        if body.is_empty() {
            out.skipped += 1;
            continue;
        }
        // 一行多语句（arm64 `%%`）：逐条展开。
        let stmts: Vec<&str> = match suite.stmt_sep {
            Some(sep) => body.split(sep).collect(),
            None => vec![body],
        };
        for stmt in stmts {
            let mut t = stmt.trim();
            if t.is_empty() {
                continue;
            }
            // 标签前缀：`name:` 之后可以跟指令；纯标签行喂进 labels 上下文。
            if let Some(colon) = t.find(':') {
                let (name, rest) = t.split_at(colon);
                let name = name.trim();
                if !name.is_empty() && !name.contains(char::is_whitespace) {
                    out.labels.push_str(name);
                    out.labels.push_str(":\n");
                    t = rest[1..].trim();
                }
            }
            if t.is_empty() {
                out.skipped += 1;
                continue;
            }
            if is_directive(t) {
                out.skipped += 1;
                continue;
            }
            out.lines.push(Line {
                file: file.to_string(),
                line_no,
                text: t.to_string(),
            });
        }
    }
    out
}

/// 读一套语料的全部文件（递归；按路径排序保证 deterministic）。
pub fn read_suite(suite: &Suite) -> Result<Vec<(String, String)>, String> {
    let dir = corpus_root().join("parse").join(suite.dir);
    if !dir.is_dir() {
        return Err(format!("语料目录不存在：{}", dir.display()));
    }
    let mut files = Vec::new();
    collect(&dir, &dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("读目录 {} 失败：{e}", dir.display()))?;
    for e in entries {
        let e = e.map_err(|e| format!("读目录项失败：{e}"))?;
        let p = e.path();
        if p.is_dir() {
            collect(root, &p, out)?;
        } else if is_corpus_file(&p) {
            let rel = p
                .strip_prefix(root)
                .map_err(|e| format!("路径归一失败：{e}"))?
                .to_string_lossy()
                .replace('\\', "/");
            let text =
                std::fs::read_to_string(&p).map_err(|e| format!("读 {} 失败：{e}", p.display()))?;
            out.push((rel, text));
        }
    }
    Ok(())
}

/// 收哪些文件：汇编源码与上游的机读期望（`.s`/`.S`/`.asm`/`cmd`/`.reference`/`.hex`/`.d`/`.txt`）。
fn is_corpus_file(p: &Path) -> bool {
    let Some(ext) = p.extension().and_then(|s| s.to_str()) else {
        // 无扩展名：XED 的 `cmd` / `codes` / `stdout.reference` 之类。
        return matches!(
            p.file_name().and_then(|s| s.to_str()),
            Some("cmd" | "codes" | "retcode.reference" | "stdout.reference" | "stderr.reference")
        );
    };
    matches!(
        ext,
        "s" | "S" | "asm" | "hex" | "d" | "l" | "txt" | "reference" | "excerpt"
    )
}
