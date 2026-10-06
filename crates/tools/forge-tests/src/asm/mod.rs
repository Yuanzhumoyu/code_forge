//! **真实汇编语料测试基础设施**（v20 V9）。
//!
//! 语料住在 `crates/tools/forge-tests/asm/`，**逐字保留上游原文**：
//!
//! - `asm/parse/<isa>/<suite>/**`：解析档语料（上游 `.s`/`.asm`/`cmd`…）；
//! - `asm/exec/<isa>/*.s` + `*.expect`：执行档小程序（我们自己写的真语法汇编，
//!   按各机 ABI 的叶子函数约定，见 [`exec`]）；
//! - `asm/ratchet/<isa>.txt`：**计数棘轮**（各桶计数 + 人工核对的红桶样例），
//!   语料或谱一变，计数就变 ⇒ 强制显式更新本文件（既防回潮也防"悄悄变绿"）；
//! - `asm/fetch.mjs`：拉取 + **取舍** + 重写 `asm/PROVENANCE.md`。取舍不是手抄清单：
//!   它按两个档写出的**逐文件**记分板（`target/asm-suite/<isa>.json` 的 `file_rows`、
//!   `encoding-files.json`）算「整条解析得住 + 不产生红桶 + 编码逐字节相等」，
//!   谱长本事后重跑即自动补回语料。
//!
//! 四个测试二进制：`tests/asm_parse.rs`（解析）、`tests/asm_encoding.rs`
//! （用语料内联的期望字节对拍）、`tests/asm_exec.rs`（真跑）、
//! `tests/asm_provenance.rs`（`PROVENANCE.md` ↔ 磁盘语料双向一致）。
//!
//! **边界（写在这儿，免得读者以为是漏做）**：
//!
//! - 只接**本汇编器支持的那一档方言**：x86 只接 Intel 语法（AT&T 不在范围内）；
//! - 语料的注释/分隔符与 `[meta].comment_char` 不同 ⇒ 由本模块按 suite 的
//!   [`Suite`] 档案**归一化**（不改谱）；
//! - 需要 C 预处理器的语料（`riscv-tests`/`riscv-arch-test` 的 `.S`）**不进解析档**
//!   ——本汇编器没有预处理器，见 `asm/README.md`。

pub mod classify;
pub mod corpus;
pub mod encoding;
pub mod exec;
pub mod report;
pub mod targets;

pub use classify::{AsmTarget, Bucket, classify_file};
pub use corpus::{Line, SUITES, Suite, corpus_root, read_suite};
pub use encoding::EncodingCase;
pub use report::{IsaReport, SuiteReport};
