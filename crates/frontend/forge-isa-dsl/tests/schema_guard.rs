//! 三方一致守卫（v18 S7c）：**模型 ↔ JSON Schema ↔ 文档**。
//!
//! ISA-DSL 的 JSON Schema 是**手写发射器**（不引 `schemars`，见方案 §10.4），手写就
//! 会漂移。这条守卫把三方钉在一起：
//!
//! 1. **模型 ↔ schema**：`schema::SECTIONS` 每节的键（required + optional + flatten）
//!    必须与 `src/v12/model.rs` 里对应结构体的 `pub` 字段**逐键相等**；
//!    - `#[serde(skip)]` 的内部字段不入 schema，但必须在 `INTERNAL_FIELDS` 里显式登记；
//!    - `#[serde(flatten)]` 的字段本身不是键（它展开成目标结构体的键空间），
//!      必须在 `FLATTEN_FIELDS` 里显式登记。
//! 2. **schema ↔ 文档**：`docs/reference/isa-dsl.md` 里被
//!    `<!-- BEGIN/END: schema-keys -->` 圈住的那张**键速查表**必须与
//!    `schema::markdown_table()` **逐字相同**——改 schema 就得同步那段文档（反之亦然）。
//!
//! 三处任一漂移都会红：模型加了字段忘了写 schema、schema 写了模型不认的键、
//! 文档的键表没跟上。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use forge_isa_dsl::schema::{SECTIONS, markdown_table};

/// 仓库根：`crates/frontend/forge-isa-dsl` 上溯三级。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// 模型的内部字段（`#[serde(skip)]`，**故意**不入 schema）。
/// 每条都要有理由：新增内部字段时必须来这里登记（否则守卫报"模型有、schema 无"）。
const INTERNAL_FIELDS: &[(&str, &str, &str)] = &[
    (
        "Instruction",
        "from_template",
        "展开来源（v18 S2），解析期填、不参与序列化",
    ),
    (
        "V12Model",
        "derived_preds",
        "派生谓词表（v18 S3f），解析期派生、不参与序列化",
    ),
];

/// serde `#[serde(flatten)]` 字段 → 它展开出的键空间（`""` = 自由表，额外键由节声明）。
const FLATTEN_FIELDS: &[(&str, &str, &str)] = &[
    ("Form", "keys", "EncKeys"),
    ("Instruction", "enc", "EncKeys"),
    ("TemplateRow", "fields", ""),
];

/// 文档里键速查表的起止标记。
const TABLE_BEGIN: &str = "<!-- BEGIN: schema-keys";
const TABLE_END: &str = "<!-- END: schema-keys -->";

/// 从模型源码里抽 `pub struct X { … }` 的字段：`(pub 字段, 是否 #[serde(skip)])`。
fn model_structs() -> Vec<(String, Vec<(String, bool)>)> {
    let src =
        std::fs::read_to_string(repo_root().join("crates/frontend/forge-isa-dsl/src/v12/model.rs"))
            .expect("读 model.rs");
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let t = lines[i].trim();
        let Some(rest) = t.strip_prefix("pub struct ") else {
            i += 1;
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        // 找 '{'
        let mut j = i;
        while j < lines.len() && !lines[j].contains('{') {
            j += 1;
        }
        let mut fields = Vec::new();
        let mut depth = 1i32;
        let mut pending_skip = false;
        let mut k = j + 1;
        while k < lines.len() && depth > 0 {
            let lt = lines[k].trim();
            if lt.starts_with("#[") && lt.contains("skip") {
                pending_skip = true;
            }
            depth += lt.matches('{').count() as i32;
            depth -= lt.matches('}').count() as i32;
            if let Some(f) = lt.strip_prefix("pub ")
                && let Some((fname, _)) = f.split_once(':')
            {
                // raw 标识符（`r#match`）在 TOML 里就是 `match`。
                let fname = fname.trim().trim_start_matches("r#").to_string();
                fields.push((fname, pending_skip));
                pending_skip = false;
            }
            k += 1;
        }
        out.push((name, fields));
        i = k;
    }
    out
}

/// **模型 ↔ schema**：逐节键集合相等（`#[serde(skip)]` 内部字段、`flatten` 字段除外）。
#[test]
fn schema_matches_model_structs() {
    let model = model_structs();
    let mut checked = 0usize;
    for s in SECTIONS {
        if s.model.is_empty() {
            continue; // 自由形态（[types] 等）
        }
        let Some((_, fields)) = model.iter().find(|(n, _)| n == s.model) else {
            panic!(
                "schema 的 `{}` 指向不存在的模型结构体 `{}`",
                s.path, s.model
            );
        };
        let schema_keys: BTreeSet<&str> = s
            .required
            .iter()
            .chain(s.optional.iter())
            .chain(s.flatten.iter())
            .copied()
            .collect();
        let mut model_keys: BTreeSet<&str> = BTreeSet::new();
        for (name, skip) in fields {
            if *skip {
                assert!(
                    INTERNAL_FIELDS
                        .iter()
                        .any(|(st, f, _)| *st == s.model && f == name),
                    "模型 `{}::{name}` 标了 #[serde(skip)] 却没在 INTERNAL_FIELDS 登记",
                    s.model
                );
                continue;
            }
            if FLATTEN_FIELDS
                .iter()
                .any(|(st, f, _)| *st == s.model && f == name)
            {
                continue;
            }
            model_keys.insert(name.as_str());
        }
        // flatten 到具体结构体的：把该结构体的键并入模型键集（TOML 里同级）。
        for (st, _, target) in FLATTEN_FIELDS {
            if *st != s.model || target.is_empty() {
                continue;
            }
            let Some((_, tf)) = model.iter().find(|(n, _)| n == target) else {
                panic!("FLATTEN_FIELDS 指向不存在的结构体 {target}");
            };
            for (n, skip) in tf {
                if !*skip {
                    model_keys.insert(n.as_str());
                }
            }
        }
        assert_eq!(
            schema_keys,
            model_keys,
            "`{}` 与模型 `{}` 的键不一致\n  schema 有模型没有：{:?}\n  模型有 schema 没有：{:?}",
            s.path,
            s.model,
            schema_keys.difference(&model_keys).collect::<Vec<_>>(),
            model_keys.difference(&schema_keys).collect::<Vec<_>>()
        );
        checked += 1;
    }
    assert!(
        checked >= 20,
        "只对照了 {checked} 节，schema 表疑似漏了大半"
    );
}

/// 内部字段表不得过时：登记的字段必须真的存在于该结构体且标着 skip。
#[test]
fn internal_fields_are_real_and_skipped() {
    let model = model_structs();
    for (st, f, why) in INTERNAL_FIELDS {
        let Some((_, fields)) = model.iter().find(|(n, _)| n == st) else {
            panic!("INTERNAL_FIELDS 指向不存在的结构体 {st}（{why}）");
        };
        let Some((_, skip)) = fields.iter().find(|(n, _)| n == f) else {
            panic!("INTERNAL_FIELDS 的 {st}::{f} 不存在（{why}）");
        };
        assert!(
            skip,
            "INTERNAL_FIELDS 的 {st}::{f} 并未标 #[serde(skip)]（{why}）"
        );
    }
}

/// 文档的键速查表区段（含标记之间的内容）。
fn docs_table_region() -> String {
    let path = repo_root().join("docs/reference/isa-dsl.md");
    let docs = std::fs::read_to_string(&path).expect("读 docs/reference/isa-dsl.md");
    let Some(start) = docs.find(TABLE_BEGIN) else {
        panic!("docs/reference/isa-dsl.md 缺少键速查表起始标记 `{TABLE_BEGIN}`");
    };
    let after = &docs[start..];
    let Some(head_end) = after.find('\n') else {
        panic!("起始标记行不完整");
    };
    let rest = &after[head_end + 1..];
    let Some(end) = rest.find(TABLE_END) else {
        panic!("docs/reference/isa-dsl.md 缺少键速查表结束标记 `{TABLE_END}`");
    };
    rest[..end].trim_matches('\n').to_string()
}

/// **schema ↔ 文档**：键速查表必须与 `schema::markdown_table()` 逐字相同。
#[test]
fn docs_key_table_matches_schema() {
    let want = markdown_table().trim_matches('\n').to_string();
    let got = docs_table_region();
    if got != want {
        let want_lines: Vec<&str> = want.lines().collect();
        let got_lines: Vec<&str> = got.lines().collect();
        let first_diff = want_lines
            .iter()
            .zip(got_lines.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(want_lines.len().min(got_lines.len()));
        panic!(
            "docs/reference/isa-dsl.md 的键速查表与 schema 不一致（首个差异在第 {} 行）\n\
             期望：{}\n实际：{}\n\n\
             修法：把 `schema::markdown_table()` 的输出粘进该标记区段（跑\n\
             `cargo test -p forge-isa-dsl --test schema_guard -- --nocapture regenerate` 可取）。",
            first_diff + 1,
            want_lines.get(first_diff).copied().unwrap_or("<缺行>"),
            got_lines.get(first_diff).copied().unwrap_or("<缺行>"),
        );
    }
}

/// 打印可粘贴的键速查表（`--nocapture`）。
#[test]
fn print_schema_key_table() {
    eprintln!("SCHEMA-TABLE-BEGIN");
    eprint!("{}", markdown_table());
    eprintln!("SCHEMA-TABLE-END");
}

/// 仓库根那份 `isa-dsl.schema.json` 必须与发射器**逐字相同**（编辑器补全的事实源）。
///
/// 修法：`cargo run -p forge-isa -- schema --out isa-dsl.schema.json`。
#[test]
fn checked_in_schema_file_is_up_to_date() {
    let path = repo_root().join("isa-dsl.schema.json");
    let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "读不到 {}：{e}（跑 `cargo run -p forge-isa -- schema --out isa-dsl.schema.json` 生成）",
            path.display()
        )
    });
    let want = forge_isa_dsl::schema::schema_json();
    assert_eq!(
        on_disk, want,
        "isa-dsl.schema.json 与 schema_json() 不一致——重新生成：\n\
         cargo run -p forge-isa -- schema --out isa-dsl.schema.json"
    );
    // 顺手校验它是**合法 JSON**（自带极小校验器——测试里不引 JSON 库，§10.4）。
    check_json(&on_disk);
}

/// 极小 JSON 校验器（结构 + 字符串转义 + 字面量）：足以证明发射器不会产出
/// 括号不配平/字符串未转义这类结构性错误。
fn check_json(s: &str) {
    let b: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    skip_ws(&b, &mut i);
    value(&b, &mut i, 0);
    skip_ws(&b, &mut i);
    assert_eq!(i, b.len(), "JSON 尾部有多余内容 @{i}");
}

fn skip_ws(b: &[char], i: &mut usize) {
    while *i < b.len() && matches!(b[*i], ' ' | '\t' | '\n' | '\r') {
        *i += 1;
    }
}

fn value(b: &[char], i: &mut usize, depth: usize) {
    assert!(depth < 64, "JSON 嵌套过深");
    skip_ws(b, i);
    let Some(&c) = b.get(*i) else {
        panic!("JSON 提前结束");
    };
    match c {
        '{' => object(b, i, depth),
        '[' => array(b, i, depth),
        '"' => {
            string(b, i);
        }
        't' | 'f' | 'n' => {
            for lit in ["true", "false", "null"] {
                let chars: Vec<char> = lit.chars().collect();
                if b[*i..].starts_with(&chars[..]) {
                    *i += chars.len();
                    return;
                }
            }
            panic!("非法字面量 @{}", *i);
        }
        c if c == '-' || c.is_ascii_digit() => {
            let start = *i;
            if b[*i] == '-' {
                *i += 1;
            }
            while *i < b.len()
                && (b[*i].is_ascii_digit() || matches!(b[*i], '.' | 'e' | 'E' | '+' | '-'))
            {
                *i += 1;
            }
            assert!(*i > start, "非法数字 @{start}");
        }
        other => panic!("非法 JSON token `{other}` @{}", *i),
    }
}

fn string(b: &[char], i: &mut usize) {
    assert_eq!(b[*i], '"');
    *i += 1;
    while *i < b.len() {
        match b[*i] {
            '"' => {
                *i += 1;
                return;
            }
            '\\' => {
                *i += 1;
                assert!(*i < b.len(), "转义序列未结束");
                let e = b[*i];
                assert!(
                    matches!(e, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u'),
                    "非法转义 \\{e}"
                );
                if e == 'u' {
                    for _ in 0..4 {
                        *i += 1;
                        assert!(
                            *i < b.len() && b[*i].is_ascii_hexdigit(),
                            "\\u 需要 4 位十六进制"
                        );
                    }
                }
                *i += 1;
            }
            c if (c as u32) < 0x20 => panic!("字符串里有裸控制字符 U+{:04x}", c as u32),
            _ => *i += 1,
        }
    }
    panic!("字符串未闭合");
}

fn object(b: &[char], i: &mut usize, depth: usize) {
    assert_eq!(b[*i], '{');
    *i += 1;
    skip_ws(b, i);
    if b.get(*i) == Some(&'}') {
        *i += 1;
        return;
    }
    loop {
        skip_ws(b, i);
        string(b, i);
        skip_ws(b, i);
        assert_eq!(b.get(*i), Some(&':'), "对象键后缺 ':'");
        *i += 1;
        value(b, i, depth + 1);
        skip_ws(b, i);
        match b.get(*i) {
            Some(',') => *i += 1,
            Some('}') => {
                *i += 1;
                return;
            }
            other => panic!("对象里非法分隔 {other:?}"),
        }
    }
}

fn array(b: &[char], i: &mut usize, depth: usize) {
    assert_eq!(b[*i], '[');
    *i += 1;
    skip_ws(b, i);
    if b.get(*i) == Some(&']') {
        *i += 1;
        return;
    }
    loop {
        value(b, i, depth + 1);
        skip_ws(b, i);
        match b.get(*i) {
            Some(',') => *i += 1,
            Some(']') => {
                *i += 1;
                return;
            }
            other => panic!("数组里非法分隔 {other:?}"),
        }
    }
}
