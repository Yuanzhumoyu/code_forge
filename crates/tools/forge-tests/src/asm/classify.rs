//! 归因分类：把每一行候选指令判进四个桶。
//!
//! **判定不依赖"首词 = 助记符"**——DSL v17 起 `asm` 模板可以操作数前置、首段可以是
//! 多 token 字面，所以"这条指令叫什么"不能靠切词猜（v20 V9 首版的助记符表已废弃）。
//! 现在只有一个事实源：**生成物的线性扫描器**：
//!
//! | 桶 | 判定 | 门禁含义 |
//! | --- | --- | --- |
//! | [`Bucket::Parsed`] | `parse_insts` 成功 | 正常 |
//! | [`Bucket::NoPrefix`] | 失败，且**没有任何候选的首段**能对上（`could_be_instruction` = false） | 记账（上游比我们全 / 语料里有别家写法） |
//! | [`Bucket::TailMismatch`] | 失败，但某条候选的**首段**对得上——首段之后没对上 | **唯一红桶**（真缺陷或需登记的方言缺口） |
//! | [`Bucket::CorpusOnly`] | 失败原因是 `UndefinedLabel`（标签在别处） | 记账（上下文不足，非缺陷） |
//!
//! 两个失败桶的分界就是扫描的第一步：`could_be_instruction` 是**必要条件**探针
//! （`false` ⇒ 一定不是本 ISA 的指令），所以 `TailMismatch` 里的每一条都"本 ISA 有能
//! 对上的开头"，值得逐条人工核对。

use super::corpus::{Extracted, Line, Suite, with_prelude};

/// 解析失败的粗分类：`UndefinedLabel` 是**上下文不足**（标签在别处），不是方言缺口。
#[derive(Debug)]
pub enum ParseErr {
    UndefinedLabel,
    Other(String),
}

/// 一个 ISA 的汇编器适配面（[`super::targets`] 里逐个实现）。
pub trait AsmTarget: Sync {
    fn isa(&self) -> &'static str;
    /// 解析一段汇编源码；返回指令条数。
    fn parse(&self, src: &str) -> Result<usize, ParseErr>;
    /// 线性扫描探针：本 ISA 的候选里有没有哪条的**首段**能匹配这段文本
    /// （生成物的 `could_be_instruction`；`false` ⇒ 一定不是本 ISA 的指令）。
    fn could_be_instruction(&self, src: &str) -> bool;
}

/// 四桶。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Parsed,
    NoPrefix,
    TailMismatch,
    CorpusOnly,
}

/// 一份文件的归因结果。
#[derive(Debug, Default, Clone)]
pub struct FileReport {
    pub file: String,
    pub lines: usize,
    pub parsed: usize,
    pub no_prefix: usize,
    pub tail_mismatch: usize,
    pub corpus_only: usize,
    /// 红桶样例（`行号: 文本 -> 错误`），最多留几条供人工核对。
    pub mismatch_samples: Vec<String>,
}

/// 逐行分类一份语料文件。
pub fn classify_file(t: &dyn AsmTarget, suite: &Suite, file: &str, src: &str) -> FileReport {
    classify_extracted(t, &super::corpus::extract(suite, file, src), file)
}

/// 逐行分类（提取结果已给时用）。
pub fn classify_extracted(t: &dyn AsmTarget, ex: &Extracted, file: &str) -> FileReport {
    let mut r = FileReport {
        file: file.to_string(),
        corpus_only: ex.skipped,
        ..Default::default()
    };
    r.lines = ex.lines.len() + ex.skipped;
    for line in &ex.lines {
        match classify_line(t, ex, line) {
            Bucket::Parsed => r.parsed += 1,
            Bucket::NoPrefix => r.no_prefix += 1,
            Bucket::TailMismatch => {
                r.tail_mismatch += 1;
                if r.mismatch_samples.len() < 8 {
                    let ctx = format!("{}:{}\t{}", line.file, line.line_no, line.text);
                    let err = match t.parse(&with_prelude(ex, line.line_no, &line.text)) {
                        Err(ParseErr::Other(e)) => e,
                        _ => String::new(),
                    };
                    r.mismatch_samples.push(format!("{ctx}\t{err}"));
                }
            }
            Bucket::CorpusOnly => r.corpus_only += 1,
        }
    }
    r
}

/// 单行判定。
pub fn classify_line(t: &dyn AsmTarget, ex: &Extracted, line: &Line) -> Bucket {
    match t.parse(&with_prelude(ex, line.line_no, &line.text)) {
        Ok(_) => Bucket::Parsed,
        // 标签在别的文件/别处 ⇒ 判"上下文不足"（记账，不当缺陷）。
        Err(ParseErr::UndefinedLabel) => Bucket::CorpusOnly,
        // 失败时问扫描器：这段开头本 ISA 有没有候选接得住？
        Err(ParseErr::Other(_)) => {
            if t.could_be_instruction(&line.text) {
                Bucket::TailMismatch
            } else {
                Bucket::NoPrefix
            }
        }
    }
}
