//! 语料**内联期望字节**的抽取：上游 LLVM MC 用例把每条的编码写在注释里
//! （`# CHECK-ASM: encoding: [0x03,0xe0,0x40,0x00]` / `// CHECK: add x0, x1, x2 // encoding: [0x20,0x00,0x01,0x8b]`）。
//!
//! 抽出 `(指令原文, 上游字节)` 配对后，凡是**本汇编器解析得动**的那条，就要求
//! `encode()` 出来的字节与上游**逐字节相等**——这是不依赖任何模拟器、也不依赖
//! 我们自己黄金表的**外部编码对拍**。
//!
//! # 上游的四种写法
//!
//! 1. **同行尾部**：`add x0, x1, x2 // CHECK: add x0, x1, x2 // encoding: [0x…]`
//!    → 用**指令本身**的文本（真实源码拼写）配对；
//! 2. **注释自带宽文本**：`// CHECK: and w0, w0, #0x1 ; encoding: [0x…]`
//!    → 用注释里 `encoding:` 之前的汇编文本配对（LLVM 常把整段 CHECK 放在指令块之前）；
//! 3. **只有字节，且在指令之后**：`ret` + `; CHECK: encoding: [0xc0,0x03,0x5f,0xd6]`
//!    （`arm64-branch-encoding.s`）；
//! 4. **只有字节，且在指令之前**：`# CHECK-ASM: encoding: [0x…]` + `lwu x0, 4(x1)`
//!    （`rv64i-valid.s`、`intel-syntax-encoding.s`）。
//!
//! # 怎么决定 3/4
//!
//! **不猜**：由 `Suite::encoding_sides` **逐段声明**（带起始行；`Before` = 注释在指令前，
//! `After` = 注释在指令后）。同一目录里两种风格混着来（`arm64-branch-encoding.s`
//! 是 After，riscv 与 x86 Intel 用例是 Before），**同一个文件里也能中途换侧**
//! （`intel-syntax-encoding.s` 从第 98 行起是 After）——任何"看第一条推断整篇"的启发式
//! 会在"首条指令没有期望注释"的文件上整体错位一条（实测 `rv32i-valid.s` 的
//! 第 15 行 `.Lpcrel_hi0: auipc …` 就是这种情况），而"整篇一个风格"的写法会在混排文件上
//! 把期望字节配错指令（那条假差异比不配对更难查）。
//!
//! 绑定规则：`Before` ⇒ 注释绑给它下面**尚未配对**的第一条指令；`After` ⇒ 绑给它
//! 上面**尚未配对**的最近一条指令（每条指令只吃一份期望）。
//!
//! 两条保护（否则配对会整体错位，把干净的语料判成红）：
//!
//! - **伪操作/标签行不算指令**（`.section`、`LBB0_3:` 参与配对会让后面全体错位一条）；
//! - **连续两条"只有字节"注释中间没有指令** ⇒ 那是**前缀被拆开的期望**
//!   （`acquire lock add [rax], rax` 的 `[0xf2]` + `[0xf0,0x48,0x01,0x00]`），
//!   一份 case 只能带一串字节，**两条都丢掉**（不猜、不错位），条数记进
//!   [`Extracted::dropped`] 供复核；
//! - 字节里含 `A`/`0b…` 的（重定位展开）解析不出十六进制 ⇒ 照样**占一个配对位**，
//!   只是不产出 case——这样它后面的用例不会因此错位。
//!
//! 解析不动的行**不算失败**（那是解析档的账，见 `classify`）——这里只保证
//! "能解析的就不能编错"。

use super::corpus::{EncodingSide, Suite, comment_body, strip_comment};

/// 一条"上游给了期望字节"的指令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodingCase {
    /// 相对语料根的路径。
    pub file: String,
    /// 1-based 行号（指令那一行）。
    pub line_no: usize,
    /// 指令原文（已剥行内注释）。
    pub text: String,
    /// 上游期望字节。
    pub bytes: Vec<u8>,
}

/// 一条 `encoding:` 注释。
#[derive(Debug)]
struct Group {
    line_no: usize,
    /// `encoding:` 之前自带的汇编文本（写法 2）。
    text: Option<String>,
    /// 期望字节；`None` = 注释存在但字节不可解析（如 `[0xeb,A]` 的重定位形式）。
    bytes: Option<Vec<u8>>,
    /// 同一行左侧就是指令（写法 1）。
    same_line: Option<String>,
}

/// 一份语料的抽取结果。
#[derive(Debug, Default)]
pub struct Extracted {
    /// `(指令, 上游字节)` 配对。
    pub cases: Vec<EncodingCase>,
    /// 被丢掉的期望注释条数（**前缀被拆开**：连续几条只有字节的注释中间没有指令，
    /// 一份 case 带不了两串字节，且无法确定它们是否属于同一指令 ⇒ 整段不猜）。
    pub dropped: usize,
}

/// 注释正文 → `(encoding: 之前的汇编文本, 期望字节)`。
///
/// 文本侧把 FileCheck 前缀（`CHECK:` / `CHECK-ASM:` / `CHECK-NEXT:` / `CHECK-ENCODING:`…）
/// 与尾部残留的注释符去干净；去掉后不像一条指令（空 / 以标点开头）就算没有文本。
fn parse_encoding_comment(body: &str) -> (Option<String>, Option<Vec<u8>>) {
    let Some((before, rest)) = body.split_once("encoding:") else {
        return (None, None);
    };
    let bytes = (|| {
        let open = rest.find('[')?;
        let close = rest[open..].find(']')? + open;
        let mut out = Vec::new();
        for tok in rest[open + 1..close].split(',') {
            let tok = tok.trim();
            let hex = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X"))?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
        }
        (!out.is_empty()).then_some(out)
    })();
    (comment_text(before), bytes)
}

/// 注释里 `encoding:` 之前的那段：去掉 FileCheck 前缀与注释符，拿不到"像指令"的文本就给 None。
fn comment_text(before: &str) -> Option<String> {
    let mut s = before.trim_end();
    // 一行里可能出现多个注释符（`// CHECK: … // encoding:`），尾部残留的先剥掉。
    for _ in 0..2 {
        s = s.trim_end_matches(['/', ';', '#']).trim_end();
    }
    let (head, tail) = match s.split_once(char::is_whitespace) {
        Some((h, t)) => (h, t.trim()),
        None => (s, ""),
    };
    // FileCheck 指令形如 `CHECK:`/`CHECK-ASM-AND-OBJ:`/`CHECK-NEXT:`——**整词 + 冒号**才算前缀。
    let text = if head.ends_with(':') && !head.starts_with('.') {
        tail
    } else {
        s
    };
    let text = text.trim();
    let first = text.chars().next()?;
    if text.is_empty() || !(first.is_alphanumeric() || first == '_' || first == '.') {
        return None;
    }
    Some(text.to_string())
}

/// 抽一份语料文件里的全部 `(指令, 上游字节)` 配对（四种写法与相位判定见模块文档）。
pub fn extract_cases(suite: &Suite, file: &str, src: &str) -> Extracted {
    // ── 第一遍：分出所有 `encoding:` 注释与候选指令行 ──
    let mut codes: Vec<(usize, String)> = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let line_no = i + 1;
        // 代码部分 = 剥掉行内注释、去掉标签前缀（`LBB0_3:` 不是指令）、排除伪操作行。
        // 标签判定与 `corpus::extract` 同口径（`:` 前不含空白才当标签）。
        let raw_code = strip_comment(trimmed, suite.comments).trim();
        let code = match raw_code.split_once(':') {
            Some((name, rest))
                if !name.trim().is_empty() && !name.contains(char::is_whitespace) =>
            {
                rest.trim()
            }
            _ => raw_code,
        }
        .to_string();
        let is_code = !code.is_empty() && !code.starts_with('.');
        // 注释正文 = 整行注释的正文（纯注释行）或行内注释的正文（尾部注释）。
        let body = comment_body(trimmed, suite.comments)
            .map(str::to_string)
            .or_else(|| {
                let head = strip_comment(trimmed, suite.comments);
                (head.len() < trimmed.len()).then(|| trimmed[head.len()..].trim().to_string())
            });
        let has_encoding = body.as_deref().is_some_and(|b| b.contains("encoding:"));
        if let Some(body) = body
            && has_encoding
        {
            let (text, bytes) = parse_encoding_comment(&body);
            groups.push(Group {
                line_no,
                text,
                bytes,
                same_line: is_code.then_some(code.clone()),
            });
        }
        // 带 `encoding:` 注释的**同行指令**只归这条注释，不进候选池（否则同一行会被配两次）。
        if is_code && !has_encoding {
            codes.push((line_no, code));
        }
    }

    // ── 只有字节的注释：先按"中间有没有指令"切成 run ──
    // run 长 ≥ 2 = 前缀被拆开的期望（一份 case 装不下）⇒ 整段丢掉；长 1 才可绑定。
    let bare: Vec<usize> = groups
        .iter()
        .enumerate()
        .filter(|(_, g)| g.same_line.is_none() && g.text.is_none())
        .map(|(i, _)| i)
        .collect();
    let mut runs: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    for &gi in &bare {
        let new_run = match cur.last() {
            Some(&p) => codes
                .iter()
                .any(|(n, _)| *n > groups[p].line_no && *n < groups[gi].line_no),
            None => true,
        };
        if new_run {
            runs.push(std::mem::take(&mut cur));
        }
        cur.push(gi);
    }
    runs.push(cur);

    // ── 注释在哪一侧：**逐段声明**（`Suite::encoding_sides`，带起始行），不猜 ──

    // ── 第二遍：绑定并产出 case ──
    let mut paired = vec![false; codes.len()];
    let mut out: Vec<EncodingCase> = Vec::new();
    let mut dropped = 0usize;
    // ── 长编码拆成**多条** `encoding:` 注释（run 长 ≥ 2）──
    //
    // 上游对长编码会写成一串只有字节的注释（`acquire lock add …` 就是
    // `[0xf2]` + `[0xf0,0x48,0x01,0x00]` 两行）。**按声明的侧把 run 切成同侧的块**，
    // 每块各自处理（同一个 run 里两种排版可以并存——`intel-syntax-encoding.s` 尾部
    // 就是"release lock 的两行在后 + pushf/popf 的两行在前"，中间只有一个空行）：
    //
    // - **注释在指令后**（`After`）：这些注释跟**同一条**指令 ⇒ 按序**拼接**成一份
    //   期望，绑给该块之前最近的一条未配对指令；
    // - **注释在指令前**（`Before`）：这些注释是**逐条**的（`pushf`/`popf` 那种
    //   「两条 CHECK 后面跟同样多条指令」）⇒ 按序 **1:1** 配给后面同样多条未配对指令。
    //
    // 两种都不做跨指令的推断：绑不上（那一侧没有足够多未配对指令）就整块计入
    // `dropped`，绝不猜。
    let mut in_long_run = vec![false; groups.len()];
    for r in runs.iter().filter(|r| r.len() > 1) {
        for &gi in r {
            in_long_run[gi] = true;
        }
        // 按侧切块（保持书写序）。
        let mut chunks: Vec<Vec<usize>> = Vec::new();
        for &gi in r {
            let side = suite.encoding_side(file, groups[gi].line_no);
            match chunks.last_mut() {
                Some(c)
                    if suite.encoding_side(file, groups[*c.last().unwrap()].line_no) == side =>
                {
                    c.push(gi)
                }
                _ => chunks.push(vec![gi]),
            }
        }
        for chunk in &chunks {
            let g0 = &groups[chunk[0]];
            let last_line = groups[*chunk.last().unwrap()].line_no;
            let before = suite.encoding_side(file, g0.line_no) == EncodingSide::Before;
            // **块内相邻**：只认"与本块之间没有别的注释"的那些指令——跨过一个注释块
            // 再往远处配就是猜（`unbindable_split_run_is_dropped_without_shifting` 钉住）。
            let prev_group = groups
                .iter()
                .map(|g| g.line_no)
                .filter(|&n| n < g0.line_no)
                .max()
                .unwrap_or(0);
            let next_group = groups
                .iter()
                .map(|g| g.line_no)
                .filter(|&n| n > last_line)
                .min()
                .unwrap_or(usize::MAX);
            if before {
                // 逐条：块里第 i 条 → 紧随其后第 i 条指令。
                let avail: Vec<usize> = (0..codes.len())
                    .filter(|&k| codes[k].0 > last_line && codes[k].0 < next_group && !paired[k])
                    .collect();
                if avail.len() < chunk.len() {
                    dropped += chunk.len(); // 紧跟的指令不够 ⇒ 不猜
                    continue;
                }
                for (&gi, &k) in chunk.iter().zip(avail.iter()) {
                    paired[k] = true;
                    match &groups[gi].bytes {
                        Some(b) => out.push(EncodingCase {
                            file: file.to_string(),
                            line_no: groups[gi].line_no,
                            text: codes[k].1.clone(),
                            bytes: b.clone(),
                        }),
                        None => dropped += 1,
                    }
                }
                continue;
            }
            let k = (0..codes.len())
                .rev()
                .find(|&k| codes[k].0 > prev_group && codes[k].0 < g0.line_no && !paired[k]);
            let Some(k) = k else {
                dropped += chunk.len(); // 紧邻的前一条指令已被配对 ⇒ 不猜
                continue;
            };
            paired[k] = true;
            let mut bytes: Vec<u8> = Vec::new();
            for &gi in chunk {
                if let Some(b) = &groups[gi].bytes {
                    bytes.extend_from_slice(b);
                }
            }
            if bytes.is_empty() {
                dropped += chunk.len();
                continue;
            }
            out.push(EncodingCase {
                file: file.to_string(),
                line_no: g0.line_no,
                text: codes[k].1.clone(),
                bytes,
            });
        }
    }
    for (gi, g) in groups.iter().enumerate() {
        // 写法 1 / 2：注释自己带得动文本。
        if let Some(t) = g.same_line.clone().or_else(|| g.text.clone()) {
            if let Some(b) = &g.bytes {
                out.push(EncodingCase {
                    file: file.to_string(),
                    line_no: g.line_no,
                    text: t,
                    bytes: b.clone(),
                });
            }
            continue;
        }
        if in_long_run[gi] {
            continue; // 已随 run 拼成一份 case（绑不上时上面已计入 dropped）
        }
        // 写法 3 / 4：只有字节的单条注释，按**该行**声明的侧绑到**尚未配对**的指令上。
        let before = suite.encoding_side(file, g.line_no) == EncodingSide::Before;
        let k = if before {
            codes
                .iter()
                .position(|(n, _)| *n > g.line_no)
                .and_then(|i| (i..codes.len()).find(|&k| !paired[k]))
        } else {
            (0..codes.len())
                .rev()
                .find(|&k| codes[k].0 < g.line_no && !paired[k])
        };
        let Some(k) = k else {
            dropped += 1; // 那一侧已经没有可绑的指令 ⇒ 同样不猜
            continue;
        };
        paired[k] = true;
        if let Some(b) = &g.bytes {
            out.push(EncodingCase {
                file: file.to_string(),
                line_no: codes[k].0,
                text: codes[k].1.clone(),
                bytes: b.clone(),
            });
        }
    }
    out.sort_by_key(|c| c.line_no);
    Extracted {
        cases: out,
        dropped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suite() -> Suite {
        Suite {
            key: "synthetic",
            isa: "x86",
            dir: "x86/synthetic",
            comments: &["//", "#"],
            stmt_sep: None,
            source: "-",
            license: "-",
            encoding_sides: &[],
        }
    }

    /// 与 [`suite`] 同，但把"注释在指令后"逐段声明出来（`arm64-branch-encoding.s` 那种）。
    fn suite_after() -> Suite {
        Suite {
            encoding_sides: &[("after.s", 1, EncodingSide::After)],
            ..suite()
        }
    }

    fn got(src: &str) -> (Vec<(usize, String, Vec<u8>)>, usize) {
        let e = extract_cases(&suite(), "t.s", src);
        let cases = e
            .cases
            .into_iter()
            .map(|c| (c.line_no, c.text, c.bytes))
            .collect();
        (cases, e.dropped)
    }

    /// 四种写法都要抽出配对；伪操作与标签行不占配对位。
    #[test]
    fn four_upstream_styles_are_all_extracted() {
        let src = "\
.section .text
L0:
# CHECK-ASM-AND-OBJ: lwu zero, 4(ra)
# CHECK-ASM: encoding: [0x03,0xe0,0x40,0x00]
lwu x0, 4(x1)
add x0, x1, x2 // CHECK: add x0, x1, x2 // encoding: [0x20,0x00,0x01,0x8b]
// CHECK: sub x0, x1, x2 // encoding: [0x20,0x00,0x21,0xcb]
// CHECK: encoding: [0x00,0x00,0x01,0x8a]
and x0, x1, x2
";
        assert_eq!(
            got(src),
            (
                vec![
                    (5, "lwu x0, 4(x1)".into(), vec![0x03, 0xe0, 0x40, 0x00]),
                    (6, "add x0, x1, x2".into(), vec![0x20, 0x00, 0x01, 0x8b]),
                    // 写法 2 的 case 报**注释那一行**（期望写在那儿）。
                    (7, "sub x0, x1, x2".into(), vec![0x20, 0x00, 0x21, 0xcb]),
                    (9, "and x0, x1, x2".into(), vec![0x00, 0x00, 0x01, 0x8a]),
                ],
                0
            )
        );
    }

    /// **声明为 After** 的文件（注释在指令之后）：`arm64-branch-encoding.s` 那种
    /// `指令 / CHECK: encoding: [..]` 交替，绝不能错位一条。
    #[test]
    fn after_side_pairs_with_the_instruction_above() {
        let src = "\
  ret
// CHECK: encoding: [0xc0,0x03,0x5f,0xd6]
  eret
// CHECK: encoding: [0xe0,0x03,0x9f,0xd6]
  br  x5
// CHECK: encoding: [0xa0,0x00,0x1f,0xd6]
";
        let e = extract_cases(&suite_after(), "t_after.s", src);
        let cases: Vec<_> = e
            .cases
            .into_iter()
            .map(|c| (c.line_no, c.text, c.bytes))
            .collect();
        assert_eq!(
            (cases, e.dropped),
            (
                vec![
                    (1, "ret".into(), vec![0xc0, 0x03, 0x5f, 0xd6]),
                    (3, "eret".into(), vec![0xe0, 0x03, 0x9f, 0xd6]),
                    (5, "br  x5".into(), vec![0xa0, 0x00, 0x1f, 0xd6]),
                ],
                0
            )
        );
    }

    /// **同一文件里混用两种风格**（`intel-syntax-encoding.s` 的尾部两条）：风格逐段声明，
    /// 从 `from_line` 起换侧。整篇一个风格时 `[0x83,0xf8,0x02]`（= `cmp eax, 2`）会被配给
    /// **后面**那条指令，凭空造出一条字节差异。
    #[test]
    fn style_switches_mid_file() {
        let suite = Suite {
            encoding_sides: &[("t.s", 4, EncodingSide::After)],
            ..suite()
        };
        let src = "\
// CHECK: encoding: [0xc3]
\tret
\tnop
\tcmp eax, 2
// CHECK: encoding: [0x83,0xf8,0x02]
\tcmp eax, [eax+2]
// CHECK: encoding: [0x67,0x3b,0x40,0x02]
";
        let e = extract_cases(&suite, "t.s", src);
        let cases: Vec<_> = e
            .cases
            .into_iter()
            .map(|c| (c.line_no, c.text, c.bytes))
            .collect();
        assert_eq!(
            cases,
            vec![
                (2, "ret".into(), vec![0xc3]),
                (4, "cmp eax, 2".into(), vec![0x83, 0xf8, 0x02]),
                (6, "cmp eax, [eax+2]".into(), vec![0x67, 0x3b, 0x40, 0x02]),
            ]
        );
    }

    /// 首条指令**没有**期望注释（`rv32i-valid.s` 的 `.Lpcrel_hi0: auipc …`）也不许让
    /// 整篇错位——这正是"看第一条推断整篇"那种启发式会踩的坑，所以改成**逐文件声明**。
    #[test]
    fn uncovered_first_instruction_does_not_shift_the_whole_file() {
        let src = "\
.Lpcrel_hi0: auipc a0, %pcrel_hi(foo)
// CHECK: encoding: [0x37,0x25,0x00,0x00]
lui a0, 2
// CHECK: encoding: [0xb7,0x0d,0x00,0x87]
lui s11, (0x87000000>>12)
";
        assert_eq!(
            got(src),
            (
                vec![
                    (3, "lui a0, 2".into(), vec![0x37, 0x25, 0x00, 0x00]),
                    (
                        5,
                        "lui s11, (0x87000000>>12)".into(),
                        vec![0xb7, 0x0d, 0x00, 0x87]
                    ),
                ],
                0
            )
        );
    }

    /// **写法 4/长编码拆行**：只有字节的注释连成 run（中间没有指令）时，按声明的侧分两种。
    ///
    /// 夹具照抄 `intel-syntax-encoding.s` 尾部的真实排版：`acquire/release lock` 是
    /// "指令在前 + 两条 CHECK"，`pushf/popf` 是"两条 CHECK + 两条指令"——**同一篇里两种
    /// 并存**，中间只有一个空行，所以必须按侧切块（逐段声明就是这份知识）。
    #[test]
    fn split_runs_bind_by_declared_side() {
        let src = "\
  acquire lock add [rax], rax
// CHECK: encoding: [0xf2]
// CHECK: encoding: [0xf0,0x48,0x01,0x00]
  release lock add [rax], rax
// CHECK: encoding: [0xf3]
// CHECK: encoding: [0xf0,0x48,0x01,0x00]

// CHECK: encoding: [0x9c]
// CHECK: encoding: [0x9d]
pushf
popf
";
        // 逐段声明：54 前的行照抄真实文件（1..9 = "指令在前"，10.. = "注释在前"）。
        let sides: &[(&str, usize, EncodingSide)] = &[
            ("t.s", 1, EncodingSide::After),
            ("t.s", 8, EncodingSide::Before),
        ];
        let s = Suite {
            encoding_sides: sides,
            ..suite()
        };
        let e = extract_cases(&s, "t.s", src);
        let cases: Vec<(usize, String, Vec<u8>)> = e
            .cases
            .into_iter()
            .map(|c| (c.line_no, c.text, c.bytes))
            .collect();
        assert_eq!(e.dropped, 0, "两种排版都该绑上：{cases:?}");
        assert_eq!(
            cases,
            vec![
                // 指令在前、注释在后（After）⇒ 两条注释**拼接**成一份期望。
                (
                    2,
                    "acquire lock add [rax], rax".into(),
                    vec![0xf2, 0xf0, 0x48, 0x01, 0x00]
                ),
                (
                    5,
                    "release lock add [rax], rax".into(),
                    vec![0xf3, 0xf0, 0x48, 0x01, 0x00]
                ),
                // 注释在前（Before）⇒ **逐条**配给后面同样多条指令。
                (8, "pushf".into(), vec![0x9c]),
                (9, "popf".into(), vec![0x9d]),
            ]
        );
    }

    /// 绑不上时**不许错位**：注释在前、但后面指令不够 ⇒ 整块丢掉，后续配对照旧。
    #[test]
    fn unbindable_split_run_is_dropped_without_shifting() {
        let src = "\
// CHECK: encoding: [0x9c]
// CHECK: encoding: [0x9d]
pushf
// CHECK: encoding: [0xc3]
    ret
";
        assert_eq!(
            got(src),
            (vec![(5, "ret".into(), vec![0xc3])], 2),
            "只有 1 条指令、2 条注释 ⇒ 那块丢掉，`ret` 仍应拿到 0xc3（不错位）"
        );
    }

    /// 重定位形式（`[0xeb,A]`）解析不出字节，但**仍占一个配对位**，后面的用例不错位。
    #[test]
    fn relocation_expectations_still_consume_a_slot() {
        let src = "\
// CHECK: encoding: [0xeb,A]
\tjmp\tLBB0_3
// CHECK: encoding: [0x48,0x89,0x44,0x24,0xf0]
\tmov\tQWORD PTR [RSP - 16], RAX
";
        assert_eq!(
            got(src),
            (
                vec![(
                    4,
                    "mov\tQWORD PTR [RSP - 16], RAX".into(),
                    vec![0x48, 0x89, 0x44, 0x24, 0xf0]
                )],
                0
            )
        );
    }

    /// 没有 `encoding:` 注释时一条都不抽（该档靠 `ASM-ENCODING-SKIP` 明说，不猜）。
    #[test]
    fn no_encoding_comments_no_cases() {
        let e = extract_cases(&suite(), "t.s", "mov rax, rbx\n# just a comment\n");
        assert!(e.cases.is_empty() && e.dropped == 0);
    }
}
