//! 语料扫描与**行抽取**：把上游文件切成"能喂给汇编器的那些行"。
//!
//! 上游语料写给人看（注释、伪指令、宏、预处理、一行多语句），汇编器只吃指令行。
//! 这里按 **suite 档案**（注释前缀 / 语句分隔符）把文件归一化成
//! [`Line`]（去掉注释、拆开一行多语句、剥掉标签前缀）；**不改语料、不改谱**。

use std::path::{Path, PathBuf};

/// `encoding:` 注释相对**指令**的位置（编码对拍档用）。
///
/// 上游两种风格都有，且**同一目录里混着来**，所以**逐文件声明、不猜**：
///
/// - [`EncodingSide::Before`]：注释在指令**前**（riscv `rv64i-valid.s`、x86 Intel 用例）；
/// - [`EncodingSide::After`]：注释在指令**后**（`arm64-branch-encoding.s`）。
///
/// 声明与文件不符时，编码对拍档会**大声报错**（字节对不上），不会静默放过——照报错
/// 补一条 [`Suite::encoding_sides`] 即可。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingSide {
    Before,
    After,
}

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
    /// `encoding:` 注释的位置：`(文件名后缀, 起始行, 位置)`，按**声明序**取最后一个
    /// `后缀命中 && 起始行 <= 行号` 的声明（1-based；`1` = 全程）。
    ///
    /// 为什么带行号：**同一文件里两种风格可以混用**（实测
    /// `x86/llvm-mc/intel-syntax-encoding.s` 前 94 行是"注释在指令前"，尾两条
    /// `cmp eax, FOO` / `cmp eax, FOO[eax]` 却是"注释在指令后"——按整篇一个风格配
    /// 会把 `[0x83,0xf8,0x02]` 配给后一条，得到一条**假**的字节差异）。所以风格是
    /// **逐段声明**的，不是整篇猜的。
    pub encoding_sides: &'static [(&'static str, usize, EncodingSide)],
}

impl Suite {
    /// 这份语料文件**第 `line_no` 行**处的 `encoding:` 注释在哪一侧。
    pub fn encoding_side(&self, file: &str, line_no: usize) -> EncodingSide {
        self.encoding_sides
            .iter()
            .rev()
            .find(|(pat, from, _)| file.ends_with(pat) && *from <= line_no)
            .map(|(_, _, side)| *side)
            .unwrap_or(EncodingSide::Before)
    }
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
        encoding_sides: &[],
    },
    Suite {
        key: "llvm-mc",
        isa: "x86",
        dir: "x86/llvm-mc",
        comments: &["//", "#"],
        stmt_sep: None,
        source: "llvm/llvm-project@llvmorg-19.1.0 llvm/test/MC/X86/（只取 Intel 语法的文件）",
        license: "Apache-2.0 WITH LLVM-exception",
        // 整篇是"注释在指令前"，**但从第 98 行起换成"注释在指令后"**（尾两条
        // `cmp eax, FOO` / `cmp eax, FOO[eax]`；上游字节 `[0x83,0xf8,0x02]` =
        // `cmp eax, 2` 只可能是前一条的）。逐段声明，别整篇猜。
        // 这一篇**内部风格不统一**（逐段声明就是为了这个）：
        //   1..53   注释在指令**前**（缺省，无需声明）
        //   54..60  锁前缀那两条：指令在前、两条 `CHECK: encoding:` 在后（长编码拆两行）
        //   61..97  回到"注释在前"
        //   98..    尾部两条 `cmp eax, FOO`：注释在指令后
        encoding_sides: &[
            ("intel-syntax-encoding.s", 54, EncodingSide::After),
            ("intel-syntax-encoding.s", 61, EncodingSide::Before),
            ("intel-syntax-encoding.s", 98, EncodingSide::After),
        ],
    },
    Suite {
        key: "llvm-mc",
        isa: "riscv64",
        dir: "riscv64/llvm-mc",
        comments: &["#", "//"],
        stmt_sep: None,
        source: "llvm/llvm-project@llvmorg-19.1.0 llvm/test/MC/RISCV/",
        license: "Apache-2.0 WITH LLVM-exception",
        // `rv*i-valid.s` / `rv*m-valid.s` 全是 `# CHECK-ASM: encoding: [..]` + 指令行。
        encoding_sides: &[],
    },
    Suite {
        key: "llvm-mc",
        isa: "aarch64",
        dir: "aarch64/llvm-mc",
        // **没有 `#`**：AArch64 的 `#` 是**立即数前缀**（`tbz x1, #3, foo`、`b #28`、`svc #0`），
        // 当注释符会把带立即数的行**从中间截断**（`tbz x1, #3, foo` → `tbz x1,`），
        // 于是它们被误判成"本 ISA 没有这条指令"。实测这批文件里**没有以 `#` 开头的注释行**
        // （LLVM 的 AArch64 用例用 `;` 与 `//`）。
        comments: &[";", "//"],
        stmt_sep: Some("%%"),
        source: "llvm/llvm-project@llvmorg-19.1.0 llvm/test/MC/AArch64/",
        license: "Apache-2.0 WITH LLVM-exception",
        // 这一套两种风格都有，**逐段**写清楚：
        // - `arm64-branch-encoding.s`：`ret` 换行后跟 `; CHECK: encoding: [..]` ⇒ After；
        // - `arm64-logical-encoding.s`：注释自带汇编文本（`; CHECK: and w0, … ; encoding:`）
        //   且成块放在指令前 ⇒ 位置无所谓（走"注释自带文本"那条路），这里仍标 Before；
        // - `arm64-separator.s`：没有 `encoding:`。
        encoding_sides: &[("arm64-branch-encoding.s", 1, EncodingSide::After)],
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
    /// 标签定义（`name:`）——**整份文件**都当上下文喂进去：汇编器两遍布局，前向标签
    /// 引用是正常写法。
    pub labels: String,
    /// 符号常量定义（`.equ`/`.set`）——**带行号**存：它们是**顺序语义**（前向引用是错，
    /// 同名后定义覆盖前定义，实测 `rv32i-valid.s` 里 `CONST` 先 30 后 16），所以喂的时候
    /// 只取定义在本行**之前**的那些。不喂会有两类假象：`lui a0, CONST` 被算成"方言缺口"，
    /// 而整文件一起喂又会把 `CONST` 全解成 16 ⇒ 编码对拍假红。
    pub symbols: Vec<(usize, String)>,
    /// 被丢掉的行数（注释/伪指令/宏/预处理/空行）。
    pub skipped: usize,
}

/// 定义**符号常量**的伪指令（`.equ`/`.set`——汇编器里是同一个东西的两种拼法，
/// 见 `dsl/codegen/machine.rs` 的伪指令分派）。
///
/// 这些行**不是**指令（照旧计入 [`Extracted::skipped`]），但它们是后续行的前提。
const SYMBOL_DIRECTIVES: &[&str] = &["equ", "set"];

/// 这一行是符号常量定义吗（`.equ NAME, expr` / `.set NAME, expr`）。
fn is_symbol_def(t: &str) -> bool {
    let Some(rest) = t.strip_prefix('.') else {
        return false;
    };
    SYMBOL_DIRECTIVES.contains(&rest.split_whitespace().next().unwrap_or(""))
}

/// 把 `line_no` 行的**上下文**（标签 + 该行之前的符号常量定义）与文本拼成一段源码。
///
/// 上下文是**语料抽取**的概念，不是汇编器语法：上游文件里符号在别处定义，
/// 单行喂进去必然解不出来。喂进去之后"解析不动"才说明汇编器真缺东西。
pub fn with_prelude(ex: &Extracted, line_no: usize, text: &str) -> String {
    if ex.labels.is_empty() && ex.symbols.is_empty() {
        return text.to_string();
    }
    let mut s = String::with_capacity(ex.labels.len() + text.len() + 32);
    s.push_str(&ex.labels);
    for (no, def) in &ex.symbols {
        if *no < line_no {
            s.push_str(def);
            s.push('\n');
        }
    }
    s.push_str(text);
    s
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
                // 符号常量定义：行仍不是候选指令，但后续行解析时必须在场。
                if is_symbol_def(t) {
                    out.symbols.push((line_no, t.to_string()));
                }
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

/// 语料是否**在盘上**。
///
/// B1 起语料**不再是 git 跟踪件**（`asm/parse/**` 进 `.gitignore`），而是本地缓存：
/// 上游语料体积大、且"留哪些文件"是**算出来的**（`fetch.mjs` 取舍），把它签进仓库
/// 只会带来 600 文件级别的 churn 与"裁回没跑成"这类错值。
///
/// 判据 = `asm/parse` 下至少有一个 `.s`。**缺失时三档套件显式跳过**（打
/// `ASM-CORPUS-MISSING`），既不在干净克隆上假红，也不假装跑过——棘轮与 PROVENANCE 的
/// 比对必须建立在真语料之上。
pub fn corpus_present() -> bool {
    fn any_s(dir: &Path) -> bool {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return false;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if any_s(&p) {
                    return true;
                }
            } else if p.extension().is_some_and(|x| x == "s") {
                return true;
            }
        }
        false
    }
    any_s(&corpus_root().join("parse"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn suite() -> Suite {
        Suite {
            key: "synthetic",
            isa: "x86",
            dir: "x86/synthetic",
            comments: &["#", "//"],
            stmt_sep: None,
            source: "-",
            license: "-",
            encoding_sides: &[],
        }
    }

    /// 符号常量定义**不是**候选指令（照旧计入 `skipped`），但进 `symbols` 且带行号。
    /// `.equ` 与 `.set` 都要认（上游两种拼法都有）。
    #[test]
    fn symbol_definitions_are_collected_not_classified() {
        let ex = extract(
            &suite(),
            "t.s",
            ".equ A, 30\nmov rax, [A]\n.set B, 2\ncmp eax, B\n",
        );
        assert_eq!(
            ex.symbols,
            vec![(1, ".equ A, 30".into()), (3, ".set B, 2".into())]
        );
        assert_eq!(ex.skipped, 2, "两条符号定义行都不算候选指令");
        assert_eq!(ex.lines.len(), 2);
        assert_eq!(ex.lines[0].line_no, 2);
    }

    /// 上下文只喂**本行之前**的符号定义（顺序语义），标签则整篇都喂（两遍布局）。
    #[test]
    fn prelude_is_positional_for_symbols_and_global_for_labels() {
        let ex = extract(
            &suite(),
            "t.s",
            ".set C, 30\nlui a0, C\n.set C, 16\nlui a0, C\nL0:\n",
        );
        let first = with_prelude(&ex, ex.lines[0].line_no, &ex.lines[0].text);
        assert_eq!(first, "L0:\n.set C, 30\nlui a0, C");
        let second = with_prelude(&ex, ex.lines[1].line_no, &ex.lines[1].text);
        assert_eq!(second, "L0:\n.set C, 30\n.set C, 16\nlui a0, C");
    }

    /// 没有定义时 `with_prelude` 原样返回（不给每一行都白拼一次）。
    #[test]
    fn prelude_is_identity_without_context() {
        let ex = extract(&suite(), "t.s", "mov rax, rbx\n");
        assert_eq!(with_prelude(&ex, 1, "mov rax, rbx"), "mov rax, rbx");
    }
}
