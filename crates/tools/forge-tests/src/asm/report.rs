//! 记分板与**计数棘轮**。
//!
//! 门禁语义（[`IsaReport::check_ratchet`]）：
//!
//! - `TailMismatch`（**红桶**：某条候选的首段对得上、整条没对上）计数必须与
//!   `asm/ratchet/<isa>.txt` **逐项相等**——多一个 = 汇编器出现新缺陷（或换了语料），
//!   少一个 = 修好了/漏跑了：**两个方向都红**，强制显式更新棘轮文件；
//! - 其余桶同样入棘轮（"上游比我们全"这件事也要可复核）；
//! - 棘轮文件里还留一小段**红桶样例**（人工核对过的方言缺口），不参与判定。
//!
//! 输出：控制台 `ASM-SUMMARY <isa>: …`（libtest 会吞通过用例的 stdout，故同时写
//! `FORGE_ASM_EVENTS` 指向的文件与 `target/asm-suite/<isa>.json`）。

use std::path::Path;

use super::classify::FileReport;

/// 一套语料（一个 suite）的计数。
#[derive(Debug, Clone, Default)]
pub struct SuiteReport {
    pub key: String,
    pub files: usize,
    pub lines: usize,
    pub parsed: usize,
    pub no_prefix: usize,
    pub tail_mismatch: usize,
    pub corpus_only: usize,
    /// 红桶样例（`file:line\ttext\terror`）。
    pub mismatch_samples: Vec<String>,
    /// **逐文件**计数（按传入序 = 路径序）。棘轮只记汇总（逐文件计数进机读记分板），
    /// 但它是**语料取舍**的唯一事实源：`asm/fetch.mjs` 用「整条接得住、一条都不红」
    /// 从上游全集里挑出 vendored 集，判据就是这张表，不靠手抄清单。
    pub file_rows: Vec<FileReport>,
}

/// 一个 ISA 的全部 suite 计数。
#[derive(Debug, Clone, Default)]
pub struct IsaReport {
    pub isa: String,
    pub suites: Vec<SuiteReport>,
    /// 语料目录存在吗（缺语料 ⇒ 该 ISA 档 skip，不假绿）。
    pub corpus_present: bool,
}

impl IsaReport {
    pub fn from_files(key: &str, files: Vec<FileReport>) -> SuiteReport {
        let mut s = SuiteReport {
            key: key.to_string(),
            files: files.len(),
            ..Default::default()
        };
        for f in &files {
            s.lines += f.lines;
            s.parsed += f.parsed;
            s.no_prefix += f.no_prefix;
            s.tail_mismatch += f.tail_mismatch;
            s.corpus_only += f.corpus_only;
            for g in &f.mismatch_samples {
                if s.mismatch_samples.len() < 12 {
                    s.mismatch_samples.push(g.clone());
                }
            }
        }
        s.file_rows = files;
        s
    }

    /// 一行摘要（与 JIT 矩阵的 `MATRIX-SUMMARY` 同风格）。
    pub fn summary_line(&self) -> String {
        let mut parts = Vec::new();
        for s in &self.suites {
            parts.push(format!(
                "{}[files={} lines={} parsed={} no_prefix={} tail_mismatch={} corpus_only={}]",
                s.key, s.files, s.lines, s.parsed, s.no_prefix, s.tail_mismatch, s.corpus_only
            ));
        }
        format!("ASM-SUMMARY {}: {}", self.isa, parts.join(" "))
    }

    /// 机读 JSON（自带极小转义；测试里不引 JSON 库）。
    pub fn to_json(&self) -> String {
        let mut out = String::from("{\n");
        out.push_str(&format!(
            "  \"isa\": \"{}\",\n  \"suites\": [\n",
            esc(&self.isa)
        ));
        for (i, s) in self.suites.iter().enumerate() {
            out.push_str(&format!(
                "    {{\"key\": \"{}\", \"files\": {}, \"lines\": {}, \"parsed\": {}, \
                 \"no_prefix\": {}, \"tail_mismatch\": {}, \"corpus_only\": {},\n     \
                 \"file_rows\": [\n",
                esc(&s.key),
                s.files,
                s.lines,
                s.parsed,
                s.no_prefix,
                s.tail_mismatch,
                s.corpus_only
            ));
            for (j, f) in s.file_rows.iter().enumerate() {
                out.push_str(&format!(
                    "      {{\"file\": \"{}\", \"lines\": {}, \"parsed\": {}, \"no_prefix\": {}, \
                     \"tail_mismatch\": {}, \"corpus_only\": {}}}",
                    esc(&f.file),
                    f.lines,
                    f.parsed,
                    f.no_prefix,
                    f.tail_mismatch,
                    f.corpus_only
                ));
                out.push_str(if j + 1 == s.file_rows.len() {
                    "\n"
                } else {
                    ",\n"
                });
            }
            out.push_str("     ]}");
            out.push_str(if i + 1 == self.suites.len() {
                "\n"
            } else {
                ",\n"
            });
        }
        out.push_str("  ]\n}\n");
        out
    }

    /// 棘轮文件内容（重新生成后直接覆盖 `asm/ratchet/<isa>.txt`）。
    pub fn to_ratchet(&self) -> String {
        let mut out = String::new();
        out.push_str("# 计数棘轮（由 tests/asm_parse.rs 输出；语料或谱一变就要更新本文件）\n");
        out.push_str(
            "# 判定：tail_mismatch（有候选首段对得上、整条没对上）多一个 = 新缺陷/换了语料；\n\
             #       少一个 = 修好了/漏跑。两个方向都红。\n",
        );
        for s in &self.suites {
            out.push_str(&format!("[suite {}]\n", s.key));
            for (k, v) in [
                ("files", s.files),
                ("lines", s.lines),
                ("parsed", s.parsed),
                ("no_prefix", s.no_prefix),
                ("tail_mismatch", s.tail_mismatch),
                ("corpus_only", s.corpus_only),
            ] {
                out.push_str(&format!("{k} = {v}\n"));
            }
            for g in &s.mismatch_samples {
                out.push_str(&format!("# mismatch: {}\n", g.replace('\n', " ")));
            }
        }
        out
    }

    /// 与棘轮文件比对：逐套逐项相等。
    pub fn check_ratchet(&self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            format!(
                "读棘轮文件 {} 失败：{e}——首次使用请把下面这段写进去：\n{}",
                path.display(),
                self.to_ratchet()
            )
        })?;
        let want = parse_ratchet(&text);
        let got = parse_ratchet(&self.to_ratchet());
        if want == got {
            return Ok(());
        }
        let mut diffs = Vec::new();
        for (k, v) in &got {
            match want.get(k) {
                Some(w) if w == v => {}
                Some(w) => diffs.push(format!("{k}: 棘轮 {w} → 实测 {v}")),
                None => diffs.push(format!("{k}: 棘轮缺项 → 实测 {v}")),
            }
        }
        for (k, w) in &want {
            if !got.contains_key(k) {
                diffs.push(format!("{k}: 棘轮 {w} → 实测缺项"));
            }
        }
        Err(format!(
            "ASM-RATCHET-MISMATCH {}：\n  {}\n若确为预期变化，用下面这段覆盖该文件：\n{}",
            path.display(),
            diffs.join("\n  "),
            self.to_ratchet()
        ))
    }
}

/// 解析棘轮文件成 `suite/key -> 计数`（`#` 行忽略）。
fn parse_ratchet(text: &str) -> std::collections::BTreeMap<String, usize> {
    let mut out = std::collections::BTreeMap::new();
    let mut suite = String::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some(name) = l.strip_prefix("[suite ").and_then(|s| s.strip_suffix(']')) {
            suite = name.trim().to_string();
            continue;
        }
        if let Some((k, v)) = l.split_once('=')
            && let Ok(n) = v.trim().parse::<usize>()
        {
            out.insert(format!("{suite}.{}", k.trim()), n);
        }
    }
    out
}

// ─────────────────────── 编码对拍档的记分与棘轮 ───────────────────────

/// 一个 ISA 的编码对拍计数 + **已知差异清单**（`asm/ratchet/encoding.txt`）。
///
/// 外部字节 oracle 必然会出现"两边都合法、但我们没选上游那种写法"的差异
/// （实测：x86 `xor rax, 12` 我们出 imm32 形式，LLVM 出符号扩展 imm8 形式），
/// 也可能出现"上游用了我们解不出来的编码"。这类差异**逐条记在棘轮里**、必须
/// 与实测**逐项相等**：新增一条 ⇒ 红（新缺陷/新缺口），少一条 ⇒ 也红（修好了，
/// 显式更新棘轮并说明）。这样红桶不靠"永久红的门禁"或"静默放过"来维持。
#[derive(Debug, Clone, Default)]
pub struct EncodingReport {
    pub isa: String,
    pub cases: usize,
    pub checked: usize,
    pub unparsed: usize,
    /// 前缀被拆开的期望注释（一份 case 装不下，不猜）。
    pub dropped: usize,
    /// 等价编码（解码后反汇编文本相同）——记账，不算缺陷。
    pub variants: usize,
    /// **已知差异**（每条一句话，含文件:行、两边字节与判定原因）。
    pub known: Vec<String>,
    /// 逐文件计数（`target/asm-suite/encoding-files.json`）。与解析档的 `file_rows` 同用：
    /// `asm/fetch.mjs` 的语料取舍 = 「解析全绿 **且** 编码无差异」，判据都从这里读。
    pub file_rows: Vec<EncodingFileRow>,
}

/// 一份语料文件的编码对拍计数。
#[derive(Debug, Clone, Default)]
pub struct EncodingFileRow {
    pub file: String,
    pub cases: usize,
    pub checked: usize,
    /// 上游给了期望字节、但本谱**解析不动**的条数（记账：解析档的棘轮管这一段）。
    pub unparsed: usize,
    /// 等价编码（解码后反汇编文本相同）——不算差异。
    pub variants: usize,
    /// **已知差异**条数（> 0 ⇒ 该文件不是"逐字节可复现"的语料，取舍时会淘汰）。
    pub known: usize,
}

impl EncodingReport {
    /// 一行摘要（与 `ASM-SUMMARY` 同风格）。
    pub fn summary_line(&self) -> String {
        format!(
            "ASM-ENCODING-SUMMARY {}: cases={} checked={} unparsed={} dropped={} variants={} known={}",
            self.isa,
            self.cases,
            self.checked,
            self.unparsed,
            self.dropped,
            self.variants,
            self.known.len()
        )
    }
}

/// 逐文件编码对拍记分板（`target/asm-suite/encoding-files.json`；键 = `<isa>/<文件名>`）。
pub fn encoding_files_json(reports: &[EncodingReport]) -> String {
    let mut out = String::from("{\n");
    let mut rows: Vec<(&str, &EncodingFileRow)> = Vec::new();
    for r in reports {
        for f in &r.file_rows {
            rows.push((&r.isa, f));
        }
    }
    for (i, (isa, f)) in rows.iter().enumerate() {
        let name = f.file.rsplit('/').next().unwrap_or(&f.file);
        out.push_str(&format!(
            "  \"{}/{}\": {{\"cases\": {}, \"checked\": {}, \"unparsed\": {}, \
             \"variants\": {}, \"known\": {}}}",
            esc(isa),
            esc(name),
            f.cases,
            f.checked,
            f.unparsed,
            f.variants,
            f.known
        ));
        out.push_str(if i + 1 == rows.len() { "\n" } else { ",\n" });
    }
    out.push_str("}\n");
    out
}

/// 三架构的编码对拍棘轮文本（重新刷 = 覆盖 `asm/ratchet/encoding.txt`）。
pub fn encoding_ratchet(reports: &[EncodingReport]) -> String {
    let mut out = String::new();
    out.push_str("# 编码对拍棘轮（由 tests/asm_encoding.rs 输出；语料或谱一变就要更新本文件）\n");
    out.push_str(
        "# 判定：计数或下面的 known 清单与实测不一致就红（两个方向）——\n\
         #       known 多一条 = 新出现的字节差异（缺陷或新缺口）；少一条 = 修好了。\n",
    );
    for r in reports {
        out.push_str(&format!("[isa {}]\n", r.isa));
        for (k, v) in [
            ("cases", r.cases),
            ("checked", r.checked),
            ("unparsed", r.unparsed),
            ("dropped", r.dropped),
            ("variants", r.variants),
            ("known", r.known.len()),
        ] {
            out.push_str(&format!("{k} = {v}\n"));
        }
        for k in &r.known {
            out.push_str(&format!("# known: {}\n", k.replace('\n', " ")));
        }
    }
    out
}

/// 与棘轮文件比对（计数逐项相等 + `known` 清单逐条相等）。
pub fn check_encoding_ratchet(reports: &[EncodingReport], path: &Path) -> Result<(), String> {
    let want_text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "读棘轮文件 {} 失败：{e}——首次使用请把下面这段写进去：\n{}",
            path.display(),
            encoding_ratchet(reports)
        )
    })?;
    let got_text = encoding_ratchet(reports);
    let want = parse_encoding_ratchet(&want_text);
    let got = parse_encoding_ratchet(&got_text);
    let mut diffs = Vec::new();
    for (k, v) in &got.0 {
        match want.0.get(k) {
            Some(w) if w == v => {}
            Some(w) => diffs.push(format!("{k}: 棘轮 {w} → 实测 {v}")),
            None => diffs.push(format!("{k}: 棘轮缺项 → 实测 {v}")),
        }
    }
    for (k, w) in &want.0 {
        if !got.0.contains_key(k) {
            diffs.push(format!("{k}: 棘轮 {w} → 实测缺项"));
        }
    }
    for k in &got.1 {
        if !want.1.contains(k) {
            diffs.push(format!("known 新增：{k}"));
        }
    }
    for k in &want.1 {
        if !got.1.contains(k) {
            diffs.push(format!("known 已消失（修好了？）：{k}"));
        }
    }
    if diffs.is_empty() {
        return Ok(());
    }
    Err(format!(
        "ASM-ENCODING-RATCHET-MISMATCH {}：\n  {}\n若确为预期变化，用下面这段覆盖该文件：\n{}",
        path.display(),
        diffs.join("\n  "),
        got_text
    ))
}

/// 解析编码对拍棘轮 → （`isa/key -> 计数`, known 清单）。
fn parse_encoding_ratchet(text: &str) -> (std::collections::BTreeMap<String, usize>, Vec<String>) {
    let mut counts = std::collections::BTreeMap::new();
    let mut known = Vec::new();
    let mut isa = String::new();
    for line in text.lines() {
        let l = line.trim();
        if let Some(k) = l.strip_prefix("# known: ") {
            known.push(k.trim().to_string());
            continue;
        }
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some(name) = l.strip_prefix("[isa ").and_then(|s| s.strip_suffix(']')) {
            isa = name.trim().to_string();
            continue;
        }
        if let Some((k, v)) = l.split_once('=')
            && let Ok(n) = v.trim().parse::<usize>()
        {
            counts.insert(format!("{isa}.{}", k.trim()), n);
        }
    }
    known.sort();
    (counts, known)
}

/// 追加事件（`FORGE_ASM_EVENTS=<路径>`；与 JIT 矩阵的 `FORGE_JIT_EVENTS` 同款理由：
/// libtest 吞掉**通过**用例的 stdout，CI 上就看不见计数与跳过原因）。
pub fn emit_event(kind: &str, msg: &str) {
    let Some(path) = std::env::var_os("FORGE_ASM_EVENTS") else {
        return;
    };
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{kind} {msg}");
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
