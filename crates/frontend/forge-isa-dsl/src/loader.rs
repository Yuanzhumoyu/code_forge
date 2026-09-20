//! `loader` — 多文件谱的组合（v18 S7d 起）。
//!
//! 语法（方案 §5.8）：
//!
//! ```toml
//! include = ["common/rv_base.toml", "common/rv_f.toml"]   # 数组：按 include 序追加
//!
//! [[override]]                                             # 显式覆盖（取代"后出现的赢"）
//! key = "meta.endian"
//! value = "little"
//! ```
//!
//! # 语义
//!
//! - **include 序在前、根文件在后**；数组节（`[[instructions]]`、`[[operand_slots]]`…）
//!   因此天然"按 include 序追加"。
//! - **同名标量冲突报错**：同一个键在两个文件里都给值 → 合并文本里是 TOML 重复键
//!   （解析错误带行号，指向后出现的文件），提示用 `[[override]]` 显式覆盖
//!   （取代隐式"后者赢"）。
//! - **表节可重复**：`[meta]` / `[conventions.bitfields]` / `[abi.frame]` 这类**表**在
//!   多个文件里出现时，后出现的那次自动改写成**点分续写**（`meta.version = "1"`），
//!   内容并进同一张表（TOML 本身禁止重复 `[table]` 头，直接拼接会报错）。
//! - **`[[override]]`**：`key` 是点分路径、`value` 是新值；旧值从合并文本里删掉、
//!   新值以点分键写到文末（因此一定生效）。`key` 必须在某个文件里真实出现过
//!   （否则报"拼错了"），覆盖不会静默失效。
//! - `include` / `override` 是**组合键**，不属于 ISA 数据（模型 `deny_unknown_fields`），
//!   合并文本里会被清掉。
//! - include 递归深度上限 8；同一文件被包含两次（含成环）报错——显式优于静默合并两遍。
//!
//! # 诊断的行号怎么保真
//!
//! 合并文本是**逐行**构造的，每一行都记着"(来源文件, 该文件内第几行)"
//! （[`LoadedSpec::map_line`]）。所以校验/生成期诊断仍能渲染成可点击的
//! `路径:行:列: 码: 消息`，行号指向**用户真正写的那一行**——包括表节被改写为点分
//! 续写的情形（改写只改合并文本的写法，不改行映射）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 组装好的谱：合并文本 + 逐行来源映射。
#[derive(Debug, Clone)]
pub struct LoadedSpec {
    /// 合并后的 TOML 文本（可直接交给 `parse_and_validate`）。
    pub text: String,
    /// 与 `text` 的每一行一一对应（第 i 项 = 第 i+1 行的来源）。
    line_map: Vec<(PathBuf, usize)>,
    /// 去重来源（include 序 + 根，根在最后）。
    pub sources: Vec<PathBuf>,
    /// 根文件路径（绝对）。
    pub root: PathBuf,
}

impl LoadedSpec {
    /// 读根文件并递归展开 `include`，然后**按块合并**（见模块文档）。
    pub fn load(root: &Path) -> Result<Self, String> {
        let root_abs = absolutize(root)?;
        let mut seen = BTreeSet::new();
        let files = load_recursive(&root_abs, 0, &mut seen)?;

        // 覆盖键：允许写在任一文件里（通常写根文件）。
        let mut overrides: Vec<Override> = Vec::new();
        for f in &files {
            overrides.extend(parse_overrides(&f.text, &f.path)?);
        }
        let mut matched = vec![false; overrides.len()];

        // 逐文件扫成"块"（根表键 / 表节 / 数组节），再合并。
        let mut root_lines: Vec<SrcLine> = Vec::new();
        let mut blocks: Vec<Block> = Vec::new();
        let mut table_index: BTreeMap<String, usize> = BTreeMap::new();
        for f in &files {
            let scanned = scan_blocks(&f.text, &f.path, &overrides, &mut matched)?;
            for b in scanned {
                match (&b.header, b.is_array) {
                    (None, _) => root_lines.extend(b.lines),
                    (Some(_), true) => blocks.push(b),
                    (Some(h), false) => match table_index.get(h) {
                        Some(i) => blocks[*i].lines.extend(b.lines),
                        None => {
                            table_index.insert(h.clone(), blocks.len());
                            blocks.push(b);
                        }
                    },
                }
            }
        }

        // 覆盖键必须真实存在过。
        for (i, o) in overrides.iter().enumerate() {
            if !matched[i] {
                return Err(format!(
                    "[[override]] key = \"{}\" 在任何文件里都没有对应的 `{} = …` 行——\
                     拼错了？还是要覆盖 include 里的表结构？",
                    o.dotted(),
                    o.leaf
                ));
            }
        }
        // 覆盖值：写回**同一个块**（表内就写进那张表，避免点分键落到别的上下文）。
        for o in &overrides {
            let Some(v) = &o.value else {
                return Err(format!(
                    "[[override]] key = \"{}\" 缺少 `value`（显式覆盖必须给新值）",
                    o.dotted()
                ));
            };
            let line = SrcLine {
                text: format!("{} = {v}\n", o.leaf),
                file: root_abs.clone(),
                line: 1,
            };
            match &o.section {
                None => root_lines.push(line),
                Some(sec) => match table_index.get(sec) {
                    Some(i) => blocks[*i].lines.push(line),
                    None => {
                        return Err(format!(
                            "[[override]] key = \"{}\"：目标表 `[{sec}]` 在合并结果里不存在",
                            o.dotted()
                        ));
                    }
                },
            }
        }

        // 组装文本 + 逐行来源映射。
        let mut out = String::new();
        let mut line_map: Vec<(PathBuf, usize)> = Vec::new();
        for l in &root_lines {
            emit(&mut out, &mut line_map, l);
        }
        for b in &blocks {
            let header = b.header.as_deref().unwrap_or("");
            let head = if b.is_array {
                format!("[[{header}]]\n")
            } else {
                format!("[{header}]\n")
            };
            out.push_str(&head);
            // 头部按该块首行的来源记（诊断落到头行时也有文件归属）。
            // 表头行的来源 = 它自己在文件里的位置（诊断落到头行也能精确指向）。
            let origin = b
                .header_origin
                .clone()
                .or_else(|| b.lines.first().map(|l| (l.file.clone(), l.line)))
                .unwrap_or_else(|| (root_abs.clone(), 1));
            line_map.push(origin);
            for l in &b.lines {
                emit(&mut out, &mut line_map, l);
            }
        }

        Ok(Self {
            text: out,
            line_map,
            sources: {
                let mut s: Vec<PathBuf> = Vec::new();
                for f in &files {
                    if !s.contains(&f.path) {
                        s.push(f.path.clone());
                    }
                }
                s
            },
            root: root_abs,
        })
    }

    /// 从**一段文本**构造（不做 include 展开；内联谱/测试用）。
    ///
    /// 行映射退化为"每一行都来自 `path` 的同一行"——与单文件语义一致。
    pub fn from_text(text: String, path: &Path) -> Self {
        let n = text.lines().count().max(1);
        let line_map = (1..=n).map(|i| (path.to_path_buf(), i)).collect();
        Self {
            text,
            line_map,
            sources: vec![path.to_path_buf()],
            root: path.to_path_buf(),
        }
    }

    /// 合并文本的 1-based 行 → (来源文件, 该文件内的 1-based 行)。
    pub fn map_line(&self, line: usize) -> (PathBuf, usize) {
        match self.line_map.get(line.saturating_sub(1)) {
            Some((p, l)) => (p.clone(), *l),
            None => (self.root.clone(), line),
        }
    }

    /// 是否真的合并了多个文件。
    pub fn is_multi_file(&self) -> bool {
        self.sources.len() > 1
    }
}

/// 合并文本里的一行：内容 + 它来自哪个文件的第几行。
#[derive(Debug, Clone)]
struct SrcLine {
    text: String,
    file: PathBuf,
    line: usize,
}

/// 一个块：`header = None` = 根表键；`is_array` = `[[x]]`（可多次出现、按序追加）。
#[derive(Debug, Clone)]
struct Block {
    header: Option<String>,
    is_array: bool,
    /// 表头那一行自己的来源（`[meta]` 的头行 → "哪个文件第几行"）。
    header_origin: Option<(PathBuf, usize)>,
    lines: Vec<SrcLine>,
}

fn emit(out: &mut String, map: &mut Vec<(PathBuf, usize)>, l: &SrcLine) {
    out.push_str(&l.text);
    for _ in 0..l.text.lines().count().max(1) {
        map.push((l.file.clone(), l.line));
    }
}

/// 把一个文件扫成块序列；同时丢掉组合键与覆盖目标键（后者记 `matched`）。
fn scan_blocks(
    text: &str,
    path: &Path,
    overrides: &[Override],
    matched: &mut [bool],
) -> Result<Vec<Block>, String> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut cur = Block {
        header: None,
        is_array: false,
        header_origin: None,
        lines: Vec::new(),
    };
    let mut skipping_block = false; // [[override]] / [override] 块
    let mut skipping_multiline = false; // 被丢弃的跨行值
    for (i, line) in text.lines().enumerate() {
        let src = i + 1;
        let t = line.trim_start();
        if skipping_block {
            if t.starts_with('[') {
                skipping_block = false;
            } else {
                continue;
            }
        }
        if skipping_multiline {
            if t.contains(']') || t.contains('}') {
                skipping_multiline = false;
            }
            continue;
        }
        if t.starts_with('[') {
            let is_array = t.starts_with("[[");
            let header = t
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .to_string();
            // 组合键（`include` / `override`）整节丢掉。
            if header == "include" || header == "override" {
                skipping_block = true;
                continue;
            }
            blocks.push(std::mem::replace(
                &mut cur,
                Block {
                    header: Some(header),
                    is_array,
                    header_origin: Some((path.to_path_buf(), src)),
                    lines: Vec::new(),
                },
            ));
            continue;
        }
        if let Some((key, rest)) = t.split_once('=') {
            let key = key.trim();
            let value = rest.trim_start();
            let multiline = (value.starts_with('[') && !value.contains(']'))
                || (value.starts_with('{') && !value.contains('}'));
            // 组合键：顶层 include / override（内联形式）。
            if cur.header.is_none() && (key == "include" || key == "override") {
                skipping_multiline = multiline;
                continue;
            }
            // 覆盖目标：旧值丢掉（新值稍后写回同一块）。
            if let Some(oi) = overrides
                .iter()
                .position(|o| o.matches_section(cur.header.as_deref(), key))
            {
                matched[oi] = true;
                skipping_multiline = multiline;
                continue;
            }
            cur.lines.push(SrcLine {
                text: format!("{key} = {value}\n"),
                file: path.to_path_buf(),
                line: src,
            });
            continue;
        }
        // 注释/空行：原样留在当前块（对 TOML 无影响，便于人读）。
        cur.lines.push(SrcLine {
            text: format!("{line}\n"),
            file: path.to_path_buf(),
            line: src,
        });
    }
    blocks.push(cur);
    Ok(blocks)
}

/// 一个源文件（已读入）。
struct FileText {
    path: PathBuf,
    text: String,
}

/// 递归读入：先 include（按序、递归），后自己。
fn load_recursive(
    path: &Path,
    depth: usize,
    seen: &mut BTreeSet<PathBuf>,
) -> Result<Vec<FileText>, String> {
    if depth > 8 {
        return Err(format!(
            "include 嵌套超过 8 层（在 {}）——请检查是否成环",
            path.display()
        ));
    }
    if !seen.insert(path.to_path_buf()) {
        return Err(format!(
            "include 成环或重复包含：{}（同一文件只能被包含一次）",
            path.display()
        ));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读不到谱文件 {}：{e}", path.display()))?;
    let value: toml::Table =
        toml::from_str(&text).map_err(|e| format!("{}：TOML 解析失败：{e}", path.display()))?;
    let mut out = Vec::new();
    if let Some(toml::Value::Array(a)) = value.get("include") {
        let dir = path.parent().unwrap_or(Path::new("."));
        for (i, v) in a.iter().enumerate() {
            let Some(s) = v.as_str() else {
                return Err(format!(
                    "{}：include[{i}] 必须是字符串路径（数组按 include 序追加）",
                    path.display()
                ));
            };
            let abs = absolutize(&dir.join(s))?;
            out.extend(load_recursive(&abs, depth + 1, seen)?);
        }
    }
    out.push(FileText {
        path: path.to_path_buf(),
        text,
    });
    Ok(out)
}

fn absolutize(p: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(p)
        .map(|p| {
            let s = p.to_string_lossy().to_string();
            PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
        })
        .map_err(|e| format!("解析路径 {} 失败：{e}", p.display()))
}

/// 一条 `[[override]]`。
#[derive(Debug, Clone)]
struct Override {
    /// 点分路径（`meta.endian` / `conventions.cond.eq.code`）。
    dotted: String,
    /// 声明时的"节"（去掉叶键的点分前缀；顶层键 = `None`）。
    section: Option<String>,
    /// 叶键名。
    leaf: String,
    /// 新值（渲染成 TOML 片段）。
    value: Option<String>,
}

impl Override {
    fn dotted(&self) -> String {
        self.dotted.clone()
    }

    /// 该 override 是否指向"当前节（`sec`）下的 `key`"（`sec = None` = 顶层键）。
    fn matches_section(&self, sec: Option<&str>, key: &str) -> bool {
        if self.leaf != key {
            return false;
        }
        match (&self.section, sec) {
            (None, None) => true,
            (Some(s), Some(cur)) => s == cur,
            _ => false,
        }
    }
}

fn parse_overrides(text: &str, path: &Path) -> Result<Vec<Override>, String> {
    let value: toml::Table =
        toml::from_str(text).map_err(|e| format!("{}：TOML 解析失败：{e}", path.display()))?;
    let Some(arr) = value.get("override") else {
        return Ok(Vec::new());
    };
    let toml::Value::Array(items) = arr else {
        return Err(format!(
            "{}：`override` 必须是 `[[override]]` 数组（key = \"…\", value = …）",
            path.display()
        ));
    };
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let toml::Value::Table(t) = item else {
            return Err(format!("{}：override[{i}] 必须是表", path.display()));
        };
        let Some(key) = t.get("key").and_then(|v| v.as_str()) else {
            return Err(format!(
                "{}：override[{i}] 缺少 `key`（点分路径，如 \"meta.endian\"）",
                path.display()
            ));
        };
        let (section, leaf) = match key.rsplit_once('.') {
            Some((s, l)) => (Some(s.to_string()), l.to_string()),
            None => (None, key.to_string()),
        };
        if leaf.is_empty() {
            return Err(format!(
                "{}：override[{i}] 的 key '{key}' 叶键为空",
                path.display()
            ));
        }
        out.push(Override {
            dotted: key.to_string(),
            section,
            leaf,
            value: t.get("value").map(|v| v.to_string()),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge_isa_loader_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn single_file_keeps_text_and_lines_verbatim() {
        let d = tmp_dir();
        let body = "[meta]\nname = \"x\"\n";
        let p = write(&d, "single.toml", body);
        let spec = LoadedSpec::load(&p).unwrap();
        assert!(!spec.is_multi_file());
        assert!(
            spec.text.ends_with(body),
            "单文件不应被改写：\n{}",
            spec.text
        );
        let (file, line) = spec.map_line(1);
        assert_eq!(file, absolutize(&p).unwrap());
        assert_eq!(line, 1, "`[meta]` 头行 = 文件第 1 行");
        assert!(!spec.text.contains("include"));
    }

    #[test]
    fn include_goes_first_and_lines_map_to_their_file() {
        let d = tmp_dir();
        let inc = write(&d, "base.toml", "[reg.gpr4]\ncount = 8\n");
        let root = write(
            &d,
            "root.toml",
            "include = [\"base.toml\"]\n[meta]\nname = \"x\"\n",
        );
        let spec = LoadedSpec::load(&root).unwrap();
        assert!(spec.is_multi_file());
        assert_eq!(spec.sources.len(), 2);
        let reg = spec.text.find("[reg.gpr4]").unwrap();
        let meta = spec.text.find("[meta]").unwrap();
        assert!(reg < meta, "include 内容必须在根之前");
        let merged_line = spec.text[..reg].matches('\n').count() + 1;
        let (file, inner) = spec.map_line(merged_line);
        assert_eq!(file, absolutize(&inc).unwrap());
        assert_eq!(inner, 1, "映射回 include 文件第 1 行");
        toml::from_str::<toml::Table>(&spec.text).expect("合并文本合法");
    }

    #[test]
    fn repeated_table_section_is_merged_with_dotted_keys() {
        let d = tmp_dir();
        let _inc = write(
            &d,
            "t_base.toml",
            "[meta]\nname = \"inc\"\nendian = \"big\"\n",
        );
        let root = write(
            &d,
            "t_root.toml",
            "include = [\"t_base.toml\"]\n[meta]\nversion = \"1\"\n",
        );
        let spec = LoadedSpec::load(&root).unwrap();
        let v: toml::Table = toml::from_str(&spec.text).expect("合并文本必须合法");
        let meta = v.get("meta").and_then(|m| m.as_table()).expect("meta");
        assert_eq!(meta.get("name").and_then(|x| x.as_str()), Some("inc"));
        assert_eq!(meta.get("endian").and_then(|x| x.as_str()), Some("big"));
        assert_eq!(meta.get("version").and_then(|x| x.as_str()), Some("1"));
        assert_eq!(spec.text.matches("[meta]").count(), 1, "{}", spec.text);
        assert!(spec.text.contains("version = \"1\""), "{}", spec.text);
    }

    #[test]
    fn override_replaces_value_and_key_is_gone_elsewhere() {
        let d = tmp_dir();
        let _inc = write(
            &d,
            "o_base.toml",
            "[meta]\nname = \"inc\"\nendian = \"big\"\n",
        );
        let root = write(
            &d,
            "o_root.toml",
            "include = [\"o_base.toml\"]\n\n[[override]]\nkey = \"meta.endian\"\nvalue = \"little\"\n\n[meta]\nversion = \"1\"\n",
        );
        let spec = LoadedSpec::load(&root).unwrap();
        assert!(!spec.text.contains("endian = \"big\""), "{}", spec.text);
        assert!(spec.text.contains("endian = \"little\""), "{}", spec.text);
        assert!(!spec.text.contains("[[override]]"), "{}", spec.text);
        let v: toml::Table = toml::from_str(&spec.text).unwrap();
        assert_eq!(
            v.get("meta")
                .and_then(|m| m.get("endian"))
                .and_then(|e| e.as_str()),
            Some("little")
        );
    }

    #[test]
    fn override_can_resolve_a_scalar_conflict() {
        let d = tmp_dir();
        let _inc = write(&d, "c_base.toml", "[meta]\nendian = \"big\"\n");
        let root = write(
            &d,
            "c_root.toml",
            "include = [\"c_base.toml\"]\n[[override]]\nkey = \"meta.endian\"\nvalue = \"little\"\n[meta]\nname = \"x\"\n",
        );
        let spec = LoadedSpec::load(&root).unwrap();
        let v: toml::Table = toml::from_str(&spec.text).unwrap();
        assert_eq!(
            v.get("meta")
                .and_then(|m| m.get("endian"))
                .and_then(|e| e.as_str()),
            Some("little")
        );
    }

    #[test]
    fn scalar_conflict_without_override_fails_at_parse() {
        let d = tmp_dir();
        let _inc = write(&d, "k_base.toml", "[meta]\nendian = \"big\"\n");
        let root = write(
            &d,
            "k_root.toml",
            "include = [\"k_base.toml\"]\n[meta]\nendian = \"little\"\n",
        );
        let spec = LoadedSpec::load(&root).unwrap();
        let err = toml::from_str::<toml::Table>(&spec.text)
            .unwrap_err()
            .to_string();
        assert!(err.contains("endian"), "冲突必须点名键：{err}");
    }

    #[test]
    fn unknown_override_key_is_an_error() {
        let d = tmp_dir();
        let root = write(
            &d,
            "bad_override.toml",
            "[meta]\nname = \"x\"\n[[override]]\nkey = \"meta.nope\"\nvalue = 1\n",
        );
        let err = LoadedSpec::load(&root).unwrap_err();
        assert!(err.contains("meta.nope"), "{err}");
    }

    #[test]
    fn override_without_value_is_an_error() {
        let d = tmp_dir();
        let root = write(
            &d,
            "no_value.toml",
            "[meta]\nname = \"x\"\n[[override]]\nkey = \"meta.name\"\n",
        );
        let err = LoadedSpec::load(&root).unwrap_err();
        assert!(err.contains("value"), "{err}");
    }

    #[test]
    fn circular_and_duplicate_includes_are_reported() {
        let d = tmp_dir();
        let a = write(&d, "a.toml", "include = [\"b.toml\"]\n");
        let _b = write(&d, "b.toml", "include = [\"a.toml\"]\n");
        let err = LoadedSpec::load(&a).unwrap_err();
        assert!(err.contains("成环") || err.contains("重复"), "{err}");

        let _c = write(&d, "c.toml", "[meta]\nname = \"c\"\n");
        let root = write(&d, "dup.toml", "include = [\"c.toml\", \"c.toml\"]\n");
        let err = LoadedSpec::load(&root).unwrap_err();
        assert!(err.contains("重复"), "{err}");
    }

    #[test]
    fn missing_include_names_the_path() {
        let d = tmp_dir();
        let p = write(&d, "missing.toml", "include = [\"nope.toml\"]\n");
        let err = LoadedSpec::load(&p).unwrap_err();
        assert!(err.contains("nope.toml"), "{err}");
    }

    #[test]
    fn array_of_tables_appends_in_include_order() {
        let d = tmp_dir();
        let _inc = write(&d, "a_insts.toml", "[[instructions]]\nname = \"A\"\n");
        let root = write(
            &d,
            "a_root.toml",
            "include = [\"a_insts.toml\"]\n[[instructions]]\nname = \"B\"\n",
        );
        let spec = LoadedSpec::load(&root).unwrap();
        let v: toml::Table = toml::from_str(&spec.text).unwrap();
        let insts = v.get("instructions").and_then(|x| x.as_array()).unwrap();
        let names: Vec<&str> = insts
            .iter()
            .filter_map(|i| i.get("name").and_then(|n| n.as_str()))
            .collect();
        assert_eq!(names, vec!["A", "B"], "include 序在前");
    }
}
