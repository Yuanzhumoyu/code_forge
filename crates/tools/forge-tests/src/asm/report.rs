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
                 \"no_prefix\": {}, \"tail_mismatch\": {}, \"corpus_only\": {}}}",
                esc(&s.key),
                s.files,
                s.lines,
                s.parsed,
                s.no_prefix,
                s.tail_mismatch,
                s.corpus_only
            ));
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
