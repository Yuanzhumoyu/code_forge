//! 三方一致守卫（v18 S7c）：**模型 ↔ JSON Schema ↔ 文档**。
//!
//! ISA-DSL 的 JSON Schema 是**手写发射器**（不引 `schemars`，见方案 §10.4），手写就
//! 会漂移。这条守卫把三方钉在一起：
//!
//! 1. **模型 ↔ schema**：`schema::SECTIONS` 每节的键（required + optional + flatten）
//!    必须与 `src/dsl/model.rs` 里对应结构体的 `pub` 字段**逐键相等**；
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

/// 读仓库文本文件并把行尾归一成 LF。
///
/// **为什么必须归一**：Windows 上 git 按 `core.autocrlf` 把签入文本检出成 CRLF，而
/// 我们的发射器产出 LF——直接逐字比较会在 Windows CI 上假红（2026-09-20 CI #146
/// 实测：`checked_in_schema_file_is_up_to_date` 与 `docs_key_table_matches_schema`
/// 只在 Test (Windows) 失败，Linux/macOS 通过）。
fn read_lf(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.replace("\r\n", "\n"))
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
        "IsaModel",
        "derived_preds",
        "派生谓词表（v18 S3f），解析期派生、不参与序列化",
    ),
    (
        "OperandSlot",
        "table_entries",
        "命名位集合表摊平结果（解析期填、不参与序列化）",
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

/// 从模型源码里抽 `pub struct X { … }` 的字段：`(TOML 键, 是否 #[serde(skip)])`。
///
/// **TOML 键**：`#[serde(rename = "…")]` 优先，否则是字段名（`r#match` → `match`）。
/// 不处理 rename 就等于把"字段名"当成"用户写的键"，`reference`/`ref` 这类改名会被
/// 误判为一致——编辑器却会对每一行 `ref = …` 报未知键（2026-09-21 实测 S7e）。
fn model_structs() -> Vec<(String, Vec<(String, bool)>)> {
    let src =
        std::fs::read_to_string(repo_root().join("crates/frontend/forge-isa-dsl/src/dsl/model.rs"))
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
        let mut pending_rename: Option<String> = None;
        let mut k = j + 1;
        while k < lines.len() && depth > 0 {
            let lt = lines[k].trim();
            if lt.starts_with("#[") {
                if lt.contains("skip") {
                    pending_skip = true;
                }
                if let Some(after) = lt.split("rename = \"").nth(1) {
                    pending_rename = after.split('"').next().map(str::to_string);
                }
            }
            depth += lt.matches('{').count() as i32;
            depth -= lt.matches('}').count() as i32;
            if let Some(f) = lt.strip_prefix("pub ")
                && let Some((fname, _)) = f.split_once(':')
            {
                // raw 标识符（`r#match`）在 TOML 里就是 `match`。
                let fname = fname.trim().trim_start_matches("r#").to_string();
                let key = pending_rename.take().unwrap_or(fname);
                fields.push((key, pending_skip));
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
    let docs = read_lf(&path).expect("读 docs/reference/isa-dsl.md");
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

/// **已删除的键不许回来，且"去处"必须存在**（v20 A5-3 / A6）。
///
/// `docs/reference/isa-dsl.md` 的「`[abi]` — 已删除」章有一张「原键 → 去处」对照表。这条守卫
/// 双向钉它：
///
/// ① **原键不许回到 schema**：schema 键表里不得再出现 `[abi]` 这一节（写它会报未知键）；
/// ② **去处必须真的存在**：对照表第二列里出现的 `[machine].<key>` / `[machine.frame].<key>`
///    都必须在 schema 键表对应的行里列着——否则文档会把读者引到一个不存在的键上
///    （这正是"删键迁移"最容易留下的坑）。
#[test]
fn deleted_abi_keys_stay_deleted_and_their_destinations_exist() {
    let docs = read_lf(&repo_root().join("docs/reference/isa-dsl.md")).expect("读 isa-dsl.md");

    // ① schema 键表里不许有 `[abi]` 节（只看**节**那一列——说明列里出现
    //    "`[abi.frame]` 已删除"这类话是正常的）。
    let table = docs_table_region();
    for line in table.lines().filter(|l| l.starts_with('|')) {
        let cells: Vec<&str> = line.split('|').collect();
        if cells.len() < 3 {
            continue;
        }
        let section = cells[1].trim().trim_matches('`');
        assert!(
            section != "[abi]" && !section.starts_with("[abi."),
            "schema 键表里又出现了 `{section}` 节——v20 A5-3 已把 `[abi]` 整节删除"
        );
    }

    // ② 取「已删除」章的对照表。
    let Some(head) = docs.find("## `[abi]` — **已删除**") else {
        panic!("docs/reference/isa-dsl.md 缺少「`[abi]` — 已删除」章");
    };
    let after = &docs[head..];
    let end = after[3..]
        .find("\n## ")
        .map(|i| i + 3)
        .unwrap_or(after.len());
    let chapter = &after[..end];

    // schema 键表 → 每节的可选键集合。
    let mut section_keys: Vec<(String, Vec<String>)> = Vec::new();
    for line in table.lines().filter(|l| l.starts_with('|')) {
        let cells: Vec<&str> = line.split('|').collect();
        if cells.len() < 4 {
            continue;
        }
        let section = cells[1].trim().trim_matches('`').to_string();
        let keys: Vec<String> = cells[3]
            .split('`')
            .skip(1)
            .step_by(2)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && !s.starts_with('†'))
            .collect();
        section_keys.push((section, keys));
    }
    let has_key = |section: &str, key: &str| -> bool {
        section_keys
            .iter()
            .any(|(s, ks)| s == section && ks.iter().any(|k| k == key))
    };

    // 逐行扫对照表第二列，抓**显式**的 `[machine].x` / `[machine.frame].x` 令牌。
    // （文档里要求这么写：去处的键要写成完整路径，否则"同名键"这种措辞无法机器核对。）
    let mut checked = 0usize;
    for line in chapter.lines().filter(|l| l.starts_with('|')) {
        let cells: Vec<&str> = line.split('|').collect();
        if cells.len() < 4 {
            continue;
        }
        let dest = cells[2];
        for key in dest_keys(dest) {
            let (section, name) = key
                .split_once('.')
                .map(|(s, k)| (format!("[{s}]"), k.to_string()))
                .expect("dest_keys 只产出 `[节].键` 形态");
            assert!(
                has_key(&section, &name),
                "对照表把 `{section}.{name}` 当去处，但 schema 键表里没有这个键（改文档或改 schema）"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "对照表里一个可核对的去处都没解析到（解析到 {checked} 条）——守卫失效了；\
         检查 docs/reference/isa-dsl.md 的去处列是否写成 `[machine].<键>` 形态"
    );
}

/// 从「去处」列里取**显式**的 `` `[machine].key` `` / `` `[machine.frame].key` `` 令牌，
/// 返回 `"machine.key"` / `"machine.frame.key"` 形态。
fn dest_keys(dest: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in dest.split('`').skip(1).step_by(2) {
        let t = part.trim();
        let Some(rest) = t.strip_prefix('[') else {
            continue;
        };
        let Some((section, key)) = rest.split_once("].") else {
            continue;
        };
        if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            out.push(format!("{section}.{key}"));
        }
    }
    out
}

/// 仓库根那份 `isa-dsl.schema.json` 必须与发射器**逐字相同**（编辑器补全的事实源）。
///
/// 修法：`cargo run -p forge-isa -- schema --out isa-dsl.schema.json`。
#[test]
fn checked_in_schema_file_is_up_to_date() {
    let path = repo_root().join("isa-dsl.schema.json");
    let on_disk = read_lf(&path).unwrap_or_else(|| {
        panic!(
            "读不到 {}（跑 `cargo run -p forge-isa -- schema --out isa-dsl.schema.json` 生成）",
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

/// **签入的谱 ↔ schema**：`isa/*.toml` 与测试夹具里出现的**每一个键**都必须被 schema
/// 认识（各节都是 `additionalProperties: false`）。
///
/// 为什么单独要这条：模型 ↔ schema 的对照只看"结构体字段 ↔ 节键集"，看不见
/// "用户实际写的键"——`#[serde(rename = "ref")]` 的字段名是 `reference`，schema 一度写成
/// `reference`，于是编辑器对 `isa/x86.toml` 里 35 处 `ref = …` 全部标红（2026-09-21
/// 实测）。这条守卫直接拿**真实谱**当输入，把这一类"schema 与谱不符"钉死。
#[test]
fn shipped_specs_only_use_schema_keys() {
    let root = repo_root();
    let mut specs: Vec<PathBuf> = Vec::new();
    for e in std::fs::read_dir(root.join("isa")).expect("读 isa/") {
        let p = e.expect("entry").path();
        if p.extension().is_some_and(|x| x == "toml") {
            specs.push(p);
        }
    }
    for e in std::fs::read_dir(root.join("crates/backend/forge-codegen/tests/isa")).expect("读夹具")
    {
        let p = e.expect("entry").path();
        if p.extension().is_some_and(|x| x == "toml") {
            specs.push(p);
        }
    }
    assert!(
        specs.len() >= 9,
        "只找到 {} 份谱，路径疑似不对",
        specs.len()
    );

    let mut bad: Vec<String> = Vec::new();
    for spec in &specs {
        let text = read_lf(spec).expect("读谱");
        let table: toml::Table = toml::from_str(&text).expect("谱必须是合法 TOML");
        check_keys(&table, &[], &mut bad);
        // 多文件谱：被 include 的片段单独也是合法谱吗？不要求（它可能只是半张表），
        // 但它里面的键会随合并结果进入模型——因此把片段也按同一套键集检查一遍
        // （它们同样带 `#:schema`，编辑器也会校验）。
    }
    for spec in &specs {
        let text = read_lf(spec).expect("读谱");
        for inc in include_targets(&text) {
            let p = spec.parent().expect("父目录").join(inc);
            let Ok(t) = std::fs::read_to_string(&p) else {
                continue;
            };
            let table: toml::Table = toml::from_str(&t.replace("\r\n", "\n")).expect("片段 TOML");
            check_keys(&table, &[], &mut bad);
        }
    }

    assert!(
        bad.is_empty(),
        "以下键不在 JSON Schema 里（编辑器会对这些谱报未知键）：\n  {}\n\
         修法：把 TOML 里**实际写的键**加进 `src/schema.rs` 对应节（字段名与 TOML 键不同时\n\
         以 `#[serde(rename = \"…\")]` 为准，见 schema_guard.rs::model_structs）。",
        bad.join("\n  ")
    );
}

/// 谱里的 `include = [...]` 目标（相对该文件）。
fn include_targets(text: &str) -> Vec<String> {
    let Ok(t) = toml::from_str::<toml::Table>(text) else {
        return Vec::new();
    };
    t.get("include")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 递归检查一张表的键是否都在 `SECTIONS` 允许的范围内。
///
/// 规则：`path` 处若正好匹配某个节，则键必须在该节（+`ENC_KEYS` flatten）里，且
/// `additional = false` 的节不允许额外键；否则 `path` 是**中间表**，键必须是某个节
/// 路径的下一段（含 `<name>` 通配）。**只有"下面还有节"的子表才继续下钻**——
/// `fields = { hw = 0 }`、`[[templates]].body`、`when = { eq = [...] }` 这类自由表的
/// 内容在 schema 里本就没有约束（属性只有 description、没有子 schema），编辑器不会报错，
/// 守卫也不该报。
fn check_keys(table: &toml::Table, path: &[String], bad: &mut Vec<String>) {
    let (allowed, additional, section_matched) = allowed_at(path);
    for (key, value) in table {
        if section_matched {
            if !additional && !allowed.contains(key.as_str()) {
                bad.push(format!("{}: {}", render_path(path, key), key));
                continue;
            }
        } else if !is_intermediate_child(path, key) {
            bad.push(format!("{}: {}", render_path(path, key), key));
            continue;
        }
        let child_path: Vec<String> = path.iter().cloned().chain([key.clone()]).collect();
        // ① 下面还有节 → 正常下钻（由节校验内容）。
        //    注意必须**先**判断这个：`[conventions.modrm]` 是真正的节，而指令/form 上的
        //    `modrm = { … }` 才是"内联子表"，两者同名（2026-09-21 实测踩过）。
        if has_section_below(&child_path) {
            match value {
                toml::Value::Table(t) => check_keys(t, &child_path, bad),
                toml::Value::Array(items) => {
                    for it in items {
                        if let Some(t) = it.as_table() {
                            check_keys(t, &child_path, bad);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        // ② 内联子表（`modrm`/`vex`/`evex`）：schema 里是"内联子表"节，键集单独给。
        if let Some(keys) = inline_table_keys(key) {
            if let Some(t) = value.as_table() {
                for k2 in t.keys() {
                    if !keys.contains(&k2.as_str()) {
                        bad.push(format!("{}.{k2}: {k2}", render_path(path, key)));
                    }
                }
            }
            continue;
        }
        // ③ 自由表（`fields = {…}`、`[[templates]].body`、`when = {…}`）：内容不受约束。
    }
}

/// `path` 下面（含自身）是否还有节？`false` = 自由表，内容不受 schema 约束。
fn has_section_below(path: &[String]) -> bool {
    SECTIONS.iter().any(|s| {
        let Some(segs) = section_segments(s.path) else {
            return false;
        };
        segs.len() >= path.len() && segs_match(&segs[..path.len()], path)
    })
}

/// schema 里以"内联子表"节描述的键（它们不是 TOML 路径，单独列出）。
/// 与 `src/schema.rs` 里那两条"内联子表"节保持一致。
fn inline_table_keys(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "modrm" | "modrm_fixed" => Some(&["reg", "rm"]),
        "vex" | "evex" => Some(&["map", "pp", "w", "l", "b", "z", "disp_scale"]),
        _ => None,
    }
}

/// `path` 处的节：`(允许的键, 是否允许额外键, 是否匹配到节)`。
fn allowed_at(path: &[String]) -> (BTreeSet<&'static str>, bool, bool) {
    let mut keys: BTreeSet<&'static str> = BTreeSet::new();
    let mut additional = false;
    let mut matched = false;
    for s in SECTIONS {
        let Some(segs) = section_segments(s.path) else {
            continue; // "内联子表"节：不是 TOML 路径
        };
        if segs.len() != path.len() || !segs_match(&segs, path) {
            continue;
        }
        matched = true;
        additional |= s.additional;
        keys.extend(s.required.iter().copied());
        keys.extend(s.optional.iter().copied());
        keys.extend(s.flatten.iter().copied());
    }
    (keys, additional, matched)
}

/// `path` 是否是某个节路径的前缀，且 `key` 是它的下一段。
fn is_intermediate_child(path: &[String], key: &str) -> bool {
    SECTIONS.iter().any(|s| {
        let Some(segs) = section_segments(s.path) else {
            return false;
        };
        segs.len() > path.len()
            && segs_match(&segs[..path.len()], path)
            && (segs[path.len()].starts_with('<') || segs[path.len()] == key)
    })
}

/// 节路径 → 段（`<name>`/`<block>`/`<key>` 等通配段保留 `<`）。无法解析的（内联子表）返回 `None`。
fn section_segments(path: &str) -> Option<Vec<&str>> {
    if path == "<root>" {
        return Some(Vec::new());
    }
    if path.contains('（') || path.contains(" / ") {
        return None; // `enc / vex / evex / modrm（内联子表）` 之类
    }
    let inner = path
        .trim_start_matches("[[")
        .trim_start_matches('[')
        .trim_end_matches("]]")
        .trim_end_matches(']');
    Some(inner.split('.').collect())
}

fn segs_match(pattern: &[&str], path: &[String]) -> bool {
    pattern.len() == path.len()
        && pattern
            .iter()
            .zip(path.iter())
            .all(|(p, a)| p.starts_with('<') || *p == a.as_str())
}

fn render_path(path: &[String], key: &str) -> String {
    if path.is_empty() {
        format!("<root>.{key}")
    } else {
        format!("{}.{key}", path.join("."))
    }
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

// ──────────────────── 谱必须落在严格 TOML 1.0 子集 ────────────────────

/// **谱必须写在严格 TOML 1.0 子集里**——这里只钉**实际踩到过的那一条规则**：
/// 内联表（`{ … }`）里不许出现裸换行。
///
/// 为什么值得一条守卫：承载谱的 `toml` crate 是 **spec 1.1** 实现
/// （`toml-1.1.x+spec-1.1.0`），1.1 **允许**多行内联表 + 尾逗号，而 1.0 解析器
/// （编辑器的 TOML 语言服务、Python `tomllib`、别的工具）**都不收**。谱里写了这种写法，
/// 本仓库的构建全绿、别人的工具报语法错（2026-10-05 实测：`isa/riscv64.toml` 的
/// `aliases = { … }` 跨了 5 行——只有本仓库能解析；改成 `[reg.gpr8.aliases]` 子表）。
///
/// **范围声明**：这不是完整的 TOML 1.0 校验器（那需要真的 1.0 解析器，本仓库的
/// `toml` 依赖是 1.1）。它只查"内联表内裸换行"这一条——**假红必须为零**：
/// 换行在**数组**里（含内联表里的数组）、字符串里、注释里都合法，扫描器逐一区分。
#[test]
fn shipped_specs_are_strict_toml_1_0() {
    let root = repo_root();
    let mut specs: Vec<PathBuf> = Vec::new();
    for dir in ["isa", "crates/backend/forge-codegen/tests/isa"] {
        for e in std::fs::read_dir(root.join(dir)).expect("读谱目录") {
            let p = e.expect("entry").path();
            if p.extension().is_some_and(|x| x == "toml") {
                specs.push(p);
            }
        }
    }
    assert!(specs.len() >= 9, "只找到 {} 份谱", specs.len());

    let mut bad: Vec<String> = Vec::new();
    for spec in &specs {
        let text = read_lf(spec).expect("读谱");
        for line in inline_table_bare_newlines(&text) {
            bad.push(format!("{}:{line}", spec.display()));
        }
    }
    assert!(
        bad.is_empty(),
        "以下谱用了 TOML 1.1 才允许的**多行内联表**（严格 1.0 解析器会报语法错）：\n  {}\n\
         修法：改成子表（`[reg.gpr8.aliases]` + 每行一个键）或把整张内联表压成一行。",
        bad.join("\n  ")
    );
}

/// 扫描器自测：**假红为零**是这条守卫成立的前提，所以先把口径钉住。
///
/// 用例取自本仓库真实写法（多行数组、内联表里套数组、注释里的花括号）与
/// 2026-10-05 修掉的那段 `aliases` 原文。
#[test]
fn inline_table_scanner_is_not_a_liar() {
    // 合法：单行内联表、多行数组（元素是内联表）、内联表里套多行数组。
    let legal = [
        "a = { x = 1, y = 2 }\n",
        "rows = [\n  { inst = \"A\" },\n  { inst = \"B\" },\n]\n",
        "imm = { pieces = [ { offset = 1, width = 2 },   # 注释里的 { 花括号\n                  { offset = 3, width = 4 } ] }\n",
        "names = [\n  \"X0\",\n  \"X1\",\n]\n",
        "s = \"{ 字符串里的花括号 }\"\n",
        "# 整行注释 { 未闭合的花括号\n",
    ];
    for src in legal {
        assert!(
            inline_table_bare_newlines(src).is_empty(),
            "假红（这段是合法 TOML 1.0）：{src:?} → {:?}",
            inline_table_bare_newlines(src)
        );
    }
    // 非法：多行内联表——2026-10-05 `isa/riscv64.toml` 修掉的那段原文（首行 = 第 1 行）。
    let bad = "aliases = { zero = 0, ra = 1,\n            s0 = 8, fp = 8 }\n";
    assert_eq!(
        inline_table_bare_newlines(bad),
        vec![1],
        "多行内联表没被认出来"
    );
}

/// 内联表里出现**裸换行**的**行号**（1-based，指换行所在行）。
///
/// 判定只认括号栈：换行时栈顶是 `{` ⇒ 违规（栈顶是 `[` ⇒ 合法的多行数组）。
/// 注释、`"…"`/`'…'`、`"""…"""`/`'''…'''` 里的花括号与换行都先剥掉。
fn inline_table_bare_newlines(text: &str) -> Vec<usize> {
    #[derive(Clone, Copy, PartialEq)]
    enum St {
        Code,
        Comment,
        Str(char),
        MlStr(char),
    }
    let cs: Vec<char> = text.chars().collect();
    let mut st = St::Code;
    let mut stack: Vec<char> = Vec::new();
    let mut hits: Vec<usize> = Vec::new();
    let mut i = 0usize;
    while i < cs.len() {
        let c = cs[i];
        match st {
            St::Comment => {
                if c == '\n' {
                    st = St::Code;
                }
                i += 1;
            }
            St::Str(q) => {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == q {
                    st = St::Code;
                }
                i += 1;
            }
            St::MlStr(q) => {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == q && cs.get(i + 1) == Some(&q) && cs.get(i + 2) == Some(&q) {
                    st = St::Code;
                    i += 3;
                    continue;
                }
                i += 1;
            }
            St::Code => match c {
                '#' => {
                    st = St::Comment;
                    i += 1;
                }
                '"' | '\'' => {
                    let q = c;
                    if cs.get(i + 1) == Some(&q) && cs.get(i + 2) == Some(&q) {
                        st = St::MlStr(q);
                        i += 3;
                    } else {
                        st = St::Str(q);
                        i += 1;
                    }
                }
                '[' | '{' => {
                    stack.push(c);
                    i += 1;
                }
                ']' | '}' => {
                    stack.pop();
                    i += 1;
                }
                '\n' => {
                    if stack.last() == Some(&'{') {
                        hits.push(i);
                    }
                    i += 1;
                }
                _ => i += 1,
            },
        }
    }
    hits.iter()
        .map(|&idx| cs[..idx].iter().filter(|c| **c == '\n').count() + 1)
        .collect()
}
