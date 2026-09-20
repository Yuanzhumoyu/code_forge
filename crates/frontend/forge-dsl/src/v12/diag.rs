//! 诊断定位与**多错误收集**（v18 S1）。
//!
//! # 为什么要重写
//!
//! 旧实现（v12–v17）有两个硬伤：
//!
//! 1. **fail-fast**：`validate()` 顺序调用 14 个校验器、每个返回 `Result<(), String>`，
//!    一次只报第一条——改一份 3,000 行的 ISA 谱要来回十几轮；
//! 2. **启发式定位**：靠 `source.find("name = \"X\"")` 回找行号，同名或同名前缀会指错，
//!    抽不出名字就退化成 `1:1`（实测：`[stack].align` 的错误只能指到文件第一行）。
//!
//! 现在的做法：**一次预扫建立声明索引**（`DeclIndex`：节 + 名字 → 精确行列），
//! 校验器把消息压进 `Diags`（带上节级错误码），最后由 [`Diags::render`] 一次渲染成
//! 多行 `路径:行:列: 码: 消息`。索引里同一名字有多处声明时，附注"另一处声明在 行:列"
//! ——重复声明这类错误因此能一次指出两处。
//!
//! 消息格式约定（沿用 v12 的既有约定，校验器只需照旧写前缀）：
//! `[[instructions.NAME]]: …` / `[[forms.NAME]]: …` / `[[operand_slots]] #i ('NAME'): …` /
//! `[[lowering.OP]]: …` / `[reg.NAME]: …` / `[stack].key …` / `[[pattern]] #i: …`。

use std::collections::BTreeMap;

/// 单次编译最多报多少条（超出只计数，避免一屏几千条）。
pub(crate) const MAX_DIAGS: usize = 32;

/// 一条诊断：节级错误码 + (行, 列) + 消息（消息里已含声明路径前缀）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub code: &'static str,
    pub line: usize,
    pub col: usize,
    pub msg: String,
    /// 附注（如"另一处声明在 12:1"）。
    pub notes: Vec<String>,
}

/// 收集器：按声明顺序累积诊断，超出上限后只计数。
#[derive(Debug, Default)]
pub struct Diags {
    items: Vec<Diag>,
    dropped: usize,
}

impl Diags {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, code: &'static str, line: usize, col: usize, msg: impl Into<String>) {
        if self.items.len() >= MAX_DIAGS {
            self.dropped += 1;
            return;
        }
        self.items.push(Diag {
            code,
            line,
            col,
            msg: msg.into(),
            notes: Vec::new(),
        });
    }

    /// 用声明索引把一条"裸消息"（含前缀约定）变成带精确位置的诊断。
    pub fn push_anchored(&mut self, idx: &DeclIndex, msg: &str) {
        self.push_resolved(idx.anchor(msg), msg);
    }

    /// 重复声明专用：诊断落在**最后一处**声明上，并附注首次声明位置。
    pub fn push_duplicate(&mut self, idx: &DeclIndex, msg: &str) {
        let mut a = idx.anchor(msg);
        if let Some(last) = a.other_decls.pop() {
            let firsts = std::mem::replace(&mut a.other_decls, vec![(a.line, a.col)]);
            let _ = firsts;
            a.line = last.0;
            a.col = last.1;
        }
        self.push_resolved(a, msg);
    }

    fn push_resolved(&mut self, anchor: Anchor, msg: &str) {
        let mut diag = Diag {
            code: anchor.code,
            line: anchor.line,
            col: anchor.col,
            msg: msg.to_string(),
            notes: Vec::new(),
        };
        for (l, c) in anchor.other_decls {
            diag.notes.push(format!("同名声明也出现在 {l}:{c}"));
        }
        if self.items.len() >= MAX_DIAGS {
            self.dropped += 1;
        } else {
            self.items.push(diag);
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 逐条遍历（单测/驱动用）。
    pub fn iter(&self) -> impl Iterator<Item = &Diag> {
        self.items.iter()
    }

    /// 被上限丢弃的条数（渲染时给"另有 N 条"）。
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// 取出全部诊断（交给 `V12Error::Validation`）。
    pub fn into_items(self) -> Vec<Diag> {
        self.items
    }

    /// 渲染为可点击的多行文本。`path = None` 时不加路径前缀（单测/Display 用）。
    pub fn render_with(&self, path: Option<&std::path::Path>) -> String {
        render(&self.items, self.dropped, path)
    }

    /// 渲染为可点击的多行文本（带路径；`isa_from_file!` 用）。
    pub fn render(&self, path: &std::path::Path) -> String {
        self.render_with(Some(path))
    }
}

/// 渲染诊断集合（`path = None` → 无路径前缀；`dropped > 0` → 追加计数尾巴）。
pub(crate) fn render(items: &[Diag], dropped: usize, path: Option<&std::path::Path>) -> String {
    let mut out = String::new();
    for d in items {
        match path {
            Some(p) => out.push_str(&format!(
                "{}:{}:{}: {}: {}\n",
                p.display(),
                d.line,
                d.col,
                d.code,
                d.msg
            )),
            None => out.push_str(&format!("{}:{}: {}: {}\n", d.line, d.col, d.code, d.msg)),
        }
        for n in &d.notes {
            out.push_str(&format!("  = 注：{n}\n"));
        }
    }
    if dropped > 0 {
        out.push_str(&format!(
            "（另有 {dropped} 条错误未列出；上限 {MAX_DIAGS} 条）\n"
        ));
    }
    out
}

/// 声明在源里的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

/// 锚定结果：位置 + 节级错误码 + 同名其它声明。
#[derive(Debug, Clone)]
pub struct Anchor {
    pub code: &'static str,
    pub line: usize,
    pub col: usize,
    /// 除首个之外的其它同名声明（重复声明诊断用）。
    pub other_decls: Vec<(usize, usize)>,
}

/// 节名 → 错误码（粗粒度、稳定；细粒度信息在消息里）。
pub(crate) fn section_code(kind: &str) -> &'static str {
    match kind {
        "meta" => "DSL-META",
        "encoding" => "DSL-ENCODING",
        "reg" => "DSL-REG",
        "stack" => "DSL-STACK",
        "types" => "DSL-TYPES",
        "conventions" => "DSL-CONV",
        "operand_slots" => "DSL-SLOT",
        "forms" => "DSL-FORM",
        "instructions" => "DSL-INST",
        "templates" => "DSL-TEMPLATE",
        "reloc" => "DSL-RELOC",
        "derive" => "DSL-DERIVE",
        "pseudo" => "DSL-PSEUDO",
        "lowering" => "DSL-LOWER",
        "pattern" => "DSL-PATTERN",
        "abi" => "DSL-ABI",
        "emit" => "DSL-EMIT",
        "spill" => "DSL-SPILL",
        "include" => "DSL-INCLUDE",
        _ => "DSL-OTHER",
    }
}

/// 声明索引：一次预扫，把"节 + 名字 → 位置"记下来。
///
/// 键空间（与校验器的消息前缀约定一一对应）：
///
/// | 源写法 | 键 |
/// | --- | --- |
/// | `[[instructions]]` + `name = "X"` | `("instructions", "X")` |
/// | `[[forms]]` / `[[operand_slots]]` / `[[templates]]` / `[[templates.rows]]` + `name`/`inst` | 同名节 |
/// | `[[lowering]]` + `op = "X"` | `("lowering", "X")` |
/// | `[reg.gpr8]` | `("reg", "gpr8")` |
/// | `[spill.GPR]` | `("spill", "GPR")` |
/// | `[meta]`/`[stack]`/`[abi]`/`[emit]`/`[types]` | `(kind, "")` |
/// | `[[pattern]]`（无名字） | `("pattern", "")`（记录该节全部出现位置） |
#[derive(Debug, Default)]
pub struct DeclIndex {
    /// (kind, name) → 声明列表（>1 = 重复声明）。
    decls: BTreeMap<(String, String), Vec<Decl>>,
    /// kind → 该节全部声明（无名字节与兜底定位用）。
    sections: BTreeMap<String, Vec<Decl>>,
    /// 源文本按行缓存（块内"精准定位到具体键"用）。
    lines: Vec<String>,
}

/// 一处声明：起始位置 + 块的行范围（含节头/`[[…]]` 头到下一个头之前）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Decl {
    pos: Pos,
    /// 块首行（下界）与块尾行（上界，1-based，闭区间）。
    block: (usize, usize),
}

impl DeclIndex {
    /// 扫描源文本建立索引（纯文本扫描，不依赖 TOML 解析成功）。
    pub fn build(source: &str) -> Self {
        // 先收集块（头行 + 名字行），再回填块尾：第 i 个块的尾 = 第 i+1 个块首 - 1。
        struct Raw {
            kind: String,
            name: Option<String>,
            start: usize,
        }
        let mut raw: Vec<Raw> = Vec::new();
        let mut kind: Option<String> = None;
        let lines: Vec<String> = source.lines().map(|s| s.to_string()).collect();
        let last_line = lines.len().max(1);
        for (i, line) in lines.iter().enumerate() {
            let line_no = i + 1;
            let t = line.trim_start();
            if t.starts_with('[') {
                let Some(end) = t.find(']') else { continue };
                let header = t[..=end]
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .trim();
                let (base, name) = match header.split_once('.') {
                    Some((b, n)) => (b.to_string(), Some(n.to_string())),
                    None => (header.to_string(), None),
                };
                raw.push(Raw {
                    kind: base.clone(),
                    name,
                    start: line_no,
                });
                kind = Some(base);
                continue;
            }
            let Some(k) = &kind else { continue };
            for key in ["name", "op"] {
                let needle = format!("{key} = \"");
                if let Some(rest) = t.strip_prefix(&needle)
                    && let Some(end) = rest.find('"')
                {
                    raw.push(Raw {
                        kind: k.clone(),
                        name: Some(rest[..end].to_string()),
                        start: line_no,
                    });
                    break;
                }
            }
        }
        let mut idx = Self {
            lines,
            ..Self::default()
        };
        for (i, r) in raw.iter().enumerate() {
            let end = raw
                .get(i + 1)
                .map(|n| n.start.saturating_sub(1))
                .unwrap_or(last_line);
            let col = idx
                .lines
                .get(r.start - 1)
                .map(|l| l.len() - l.trim_start().len() + 1)
                .unwrap_or(1);
            let decl = Decl {
                pos: Pos { line: r.start, col },
                block: (r.start, end),
            };
            // 节级表（按 kind 记全部出现）
            idx.sections.entry(r.kind.clone()).or_default().push(decl);
            match &r.name {
                Some(n) => {
                    idx.decls
                        .entry((r.kind.clone(), n.clone()))
                        .or_default()
                        .push(decl);
                }
                None => {
                    idx.decls
                        .entry((r.kind.clone(), String::new()))
                        .or_default()
                        .push(decl);
                }
            }
        }
        idx
    }

    fn lookup(&self, kind: &str, name: &str) -> Option<&Vec<Decl>> {
        self.decls.get(&(kind.to_string(), name.to_string()))
    }

    fn section_first(&self, kind: &str) -> Option<Decl> {
        self.sections.get(kind).and_then(|v| v.first().copied())
    }

    /// 在块内把位置精准到"出错的那一行"。
    ///
    /// 做法：从消息里抽出**被引号/花括号括起来的值**（校验器统一这么点名出错的值），
    /// 在块的行范围内找第一处出现该值的行。范围被限制在**同一处声明**内，
    /// 因此不会像旧实现那样跨文件误指（旧实现全局 `source.find`）。
    fn pinpoint(&self, decl: Decl, msg: &str) -> Pos {
        let mut p = decl.pos;
        for token in quoted_values(msg) {
            for line_no in decl.block.0..=decl.block.1 {
                let Some(text) = self.lines.get(line_no - 1) else {
                    continue;
                };
                if text.contains(&token) {
                    let col = text.len() - text.trim_start().len() + 1;
                    return Pos { line: line_no, col };
                }
            }
        }
        p.col = p.col.max(1);
        p
    }

    /// 从消息前缀约定解析出 (节, 名字)；解析不出名字时用节头位置兜底，
    /// 再在块内精准到出错的那一行（见 [`DeclIndex::pinpoint`]）。
    pub fn anchor(&self, msg: &str) -> Anchor {
        let (kind, name) = parse_prefix(msg);
        let code = section_code(kind.as_deref().unwrap_or(""));
        let (decl, other) = match (&kind, &name) {
            (Some(k), Some(n)) => match self.lookup(k, n) {
                Some(decls) => {
                    let others = decls[1..]
                        .iter()
                        .map(|d| (d.pos.line, d.pos.col))
                        .collect::<Vec<_>>();
                    (decls[0], others)
                }
                // 名字找不到（未声明的引用）⇒ 落在该节首处，仍比 1:1 有用
                None => match self.section_first(k) {
                    Some(d) => (d, Vec::new()),
                    None => (
                        Decl {
                            pos: Pos { line: 1, col: 1 },
                            block: (1, self.lines.len().max(1)),
                        },
                        Vec::new(),
                    ),
                },
            },
            (Some(k), None) => match self.section_first(k) {
                Some(d) => (d, Vec::new()),
                None => (
                    Decl {
                        pos: Pos { line: 1, col: 1 },
                        block: (1, self.lines.len().max(1)),
                    },
                    Vec::new(),
                ),
            },
            _ => (
                Decl {
                    pos: Pos { line: 1, col: 1 },
                    block: (1, self.lines.len().max(1)),
                },
                Vec::new(),
            ),
        };
        let pos = self.pinpoint(decl, msg);
        Anchor {
            code,
            line: pos.line,
            col: pos.col,
            other_decls: other,
        }
    }
}

/// 抽出消息里"被引号/花括号括起来的值"——校验器点名出错值时统一这么写。
///
/// 支持 `'x'`、`"x"`、`` `x` ``、`{x}`、`@x`（伪指令）。返回按长度降序（先试更具体的）。
fn quoted_values(msg: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = msg.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        let close = match c {
            '\'' => Some('\''),
            '"' => Some('"'),
            '`' => Some('`'),
            '{' => Some('}'),
            _ => None,
        };
        if let Some(close) = close
            && let Some(rel) = msg[i + 1..].find(close)
        {
            let val = &msg[i + 1..i + 1 + rel];
            if !val.is_empty() {
                out.push(val.to_string());
            }
            i = i + 1 + rel + 1;
            continue;
        }
        i += 1;
    }
    // `@伪指令`（无引号）也可能是出错的值
    for tok in msg.split_whitespace() {
        if let Some(rest) = tok.strip_prefix('@') {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.push(format!("@{name}"));
            }
        }
    }
    out.sort_by_key(|v| std::cmp::Reverse(v.len()));
    out.dedup();
    out
}

/// 解析消息前缀 → (节, 可选名字)。
fn parse_prefix(msg: &str) -> (Option<String>, Option<String>) {
    // `[[operand_slots]] #3 ('gprx'): …`
    if let Some(rest) = msg.strip_prefix("[[operand_slots]]")
        && let Some(open) = rest.find("('")
        && let Some(close) = rest[open + 2..].find("')")
    {
        return (
            Some("operand_slots".into()),
            Some(rest[open + 2..open + 2 + close].to_string()),
        );
    }
    let (body, bracket_len) = if let Some(b) = msg.strip_prefix("[[") {
        (b, 2)
    } else if let Some(b) = msg.strip_prefix('[') {
        (b, 1)
    } else {
        return (None, None);
    };
    let _ = bracket_len;
    let Some(end) = body.find(']') else {
        return (None, None);
    };
    let head = &body[..end];
    match head.split_once('.') {
        // `[reg.gpr8]` / `[[instructions.ADD]]` / `[[spill.GPR]]`
        Some((k, n)) => (Some(k.to_string()), Some(n.to_string())),
        // `[[instructions]] #3:` / `[meta]` / `[stack]` —— 无名节
        None => (Some(head.to_string()), None),
    }
}

/// 字节偏移 → 1-based (行, 列)（TOML 解析错误用）。
pub(crate) fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let off = offset.min(source.len());
    let before = &source[..off];
    let line = before.matches('\n').count() + 1;
    let col = before
        .rsplit('\n')
        .next()
        .map(|l| l.chars().count())
        .unwrap_or(0)
        + 1;
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"[meta]
name = "x"

[stack]
align = 16

[reg.gpr8]
names = ["A0", "A1"]

[[forms]]
name = "RR"

[[instructions]]
name = "ADD"
form = "RR"

[[instructions]]
name = "ADD"

[[lowering]]
op = "Iadd"
insts = ["ADD {out}, {0}, {1}"]
"#;

    #[test]
    fn index_finds_declarations_exactly() {
        let idx = DeclIndex::build(DOC);
        let a = idx.anchor("[[instructions.ADD]]: bad");
        assert_eq!((a.line, a.col), (14, 1), "ADD 首次声明在第 14 行");
        assert_eq!(a.code, "DSL-INST");
        assert_eq!(
            a.other_decls,
            vec![(18, 1)],
            "重复声明应给出第二处位置：{a:?}"
        );
        let f = idx.anchor("[[forms.RR]]: bad");
        assert_eq!((f.line, f.col), (11, 1));
        let l = idx.anchor("[[lowering.Iadd]]: bad");
        assert_eq!((l.line, l.col), (21, 1));
        let r = idx.anchor("[reg.gpr8]: bad");
        assert_eq!((r.line, r.col), (7, 1));
        let s = idx.anchor("[stack].align must be > 0");
        assert_eq!((s.line, s.col), (4, 1), "无名节落到节头而不是 1:1");
    }

    #[test]
    fn unknown_name_falls_back_to_section() {
        let idx = DeclIndex::build(DOC);
        let a = idx.anchor("[[instructions.NOPE]]: bad");
        assert_eq!(
            (a.line, a.col),
            (13, 1),
            "未声明的名字落回 [[instructions]] 首处"
        );
        let z = idx.anchor("完全没有前缀的消息");
        assert_eq!((z.line, z.col), (1, 1));
    }

    #[test]
    fn encoding_section_gets_its_own_code() {
        let doc = "[meta]\nname = \"x\"\n[encoding]\nkind = \"fixed\"\nwidths = [16]\n";
        let idx = DeclIndex::build(doc);
        let a = idx.anchor("[encoding].widths 只适用于 kind = \"mixed\"");
        assert_eq!(a.code, "DSL-ENCODING");
        assert_eq!((a.line, a.col), (3, 1), "无名节落到 [encoding] 节头");
    }

    #[test]
    fn diags_render_multi_line_and_cap() {
        let idx = DeclIndex::build(DOC);
        let mut d = Diags::new();
        d.push_anchored(&idx, "[[instructions.ADD]]: 第一条");
        d.push_anchored(&idx, "[[lowering.Iadd]]: 第二条");
        let text = d.render(std::path::Path::new("/tmp/x.toml"));
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].starts_with("/tmp/x.toml:14:1: DSL-INST: [[instructions.ADD]]: 第一条"),
            "{text}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("注：同名声明也出现在 18:1")),
            "{text}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("DSL-LOWER: [[lowering.Iadd]]: 第二条")),
            "{text}"
        );
        for i in 0..(MAX_DIAGS + 5) {
            d.push("DSL-OTHER", 1, 1, format!("x{i}"));
        }
        let text = d.render(std::path::Path::new("p"));
        // 共 2 + (MAX_DIAGS + 5) 次 push ⇒ 保留 MAX_DIAGS 条，其余计入 dropped。
        assert_eq!(d.len(), MAX_DIAGS);
        assert_eq!(d.dropped(), 2 + 5);
        assert!(text.contains("另有 7 条错误未列出"), "{text}");
    }
}
