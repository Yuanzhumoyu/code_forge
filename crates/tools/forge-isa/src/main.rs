//! `forge-isa` — ISA-DSL 工具链 CLI（v18 S7b）。
//!
//! 子命令（全部对**谱文件**操作，不生成代码）：
//!
//! | 命令 | 作用 |
//! | --- | --- |
//! | `validate <file>…` | 解析 + 校验，打印全部诊断（`路径:行:列: 码: 消息`），有错退出 1 |
//! | `insts <file>` | 列出**展开后**的指令与其生效规格（字长/opcode/form/ops/asm/编码键/ref/reloc） |
//! | `explain <file> <inst>` | 单条指令的完整来源：来自哪个模板的哪一行 + 该行的键 + 生效编码键 |
//! | `diff <a> <b>` | 两份谱的**规格 diff**（增/删/改字段），`explain` 的文本即可用来核对迁移等价 |
//! | `test <谱>` | **跑谱里的自测**（v19 V3a）：现搭一个零宿主 crate，`cargo test` 执行生成物里的 `__spec_tests`（谱内 `[[vectors]]` + 每条指令的闭环用例） |
//!
//! 约定：默认人类可读输出，`--json` 走机读（手写发射器，不引 `serde_json`——
//! 与"不新增依赖"的显式假设一致，见 `docs/archive/forge-dsl-v18-plan.md` §10.4）。
//! 退出码：`0` 成功、`1` 诊断/失败、`2` 用法错误。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use forge_isa_dsl::report::{self, DiagLine, Explain, InstRow, SpecDiff};

mod isa_test;

const USAGE: &str = "\
forge-isa — ISA-DSL 工具链（v18 S7b）

用法：
  forge-isa validate <谱.toml>... [--strict-overlap] [--params k=v,...]
                                             解析 + 校验，打印全部诊断（严格档另报 lowering 部分重叠）
  forge-isa insts    <谱.toml> [--json] [--params k=v,...]
                                             列出展开后的指令与生效规格（`--params` = 变体投影）
  forge-isa explain  <谱.toml> <指令名> [--json]
                                             单条指令的来源（模板行 + 生效编码键）
  forge-isa diff     <a.toml> <b.toml> [--json]
                                             两份谱的规格 diff（增/删/改字段）
  forge-isa test     <谱.toml> [--json]      跑谱里的自测（谱内向量 + 每条指令闭环用例）
  forge-isa lint     <谱.toml>... [--json] [--ops <宿主 op 表.toml>] [--refs] [--bits] [--suggest]
                                             静态体检（未用的槽/form/位域、位域重叠…）；
                                             `--ops` 报能力缺口（终结/宿主管线/真缺口三类分开）、
                                             `--refs` 未引用的 ref、`--bits` 未指定位、`--suggest` 可合并的 vary 族
  forge-isa schema   [--out <file>]         打印（或写出）ISA-DSL 的 JSON Schema
  forge-isa fmt      <谱.toml> [--out <file>] 打印（或写出）**合并后**的规范文稿
                                             （include 展开 + override 应用，供人核对）
  forge-isa --help | --version

退出码：0 = 成功；1 = 诊断或失败；2 = 用法错误。";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(usage) => {
            eprintln!("{usage}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let Some(cmd) = args.first() else {
        return Err("缺少子命令".into());
    };
    match cmd.as_str() {
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        "--version" | "-V" => {
            println!("forge-isa {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        "validate" => {
            let (rest, mut opts) = split_params(&args[1..])?;
            opts.strict_overlap = has_flag(&rest, "--strict-overlap");
            let files = paths(&rest, &["--strict-overlap"])?;
            if files.is_empty() {
                return Err("validate 需要一个或多个谱文件".into());
            }
            Ok(cmd_validate(&files, &opts))
        }
        "insts" => {
            let (rest, opts) = split_params(&args[1..])?;
            let json = has_flag(&rest, "--json");
            let files = paths(&rest, &["--json"])?;
            let [file] = files.as_slice() else {
                return Err("insts 需要恰好一个谱文件".into());
            };
            Ok(cmd_insts(file, json, &opts))
        }
        "explain" => {
            let json = has_flag(&args[1..], "--json");
            let rest = paths(&args[1..], &["--json"])?;
            let [file, inst] = rest.as_slice() else {
                return Err("explain 需要 <谱文件> <指令名>".into());
            };
            Ok(cmd_explain(file, &inst.to_string_lossy(), json))
        }
        "diff" => {
            let json = has_flag(&args[1..], "--json");
            let files = paths(&args[1..], &["--json"])?;
            let [a, b] = files.as_slice() else {
                return Err("diff 需要两份谱文件".into());
            };
            Ok(cmd_diff(a, b, json))
        }
        "lint" => {
            // `--ops <宿主 op 表>` / `--refs` / `--bits` / `--suggest` 是 V4c/V4d 的可选输入
            // （判据见 `forge_isa_dsl::lint` 模块头）。
            let (rest, ops_path) = split_flag_value(&args[1..], "--ops")?;
            let refs = has_flag(&rest, "--refs");
            let bits = has_flag(&rest, "--bits");
            let suggest = has_flag(&rest, "--suggest");
            let json = has_flag(&rest, "--json");
            let files = paths(&rest, &["--json", "--refs", "--bits", "--suggest"])?;
            if files.is_empty() {
                return Err("lint 需要一个或多个谱文件".into());
            }
            Ok(cmd_lint(
                &files,
                json,
                refs,
                ops_path.as_deref(),
                bits,
                suggest,
            ))
        }
        "test" => {
            let json = has_flag(&args[1..], "--json");
            let files = paths(&args[1..], &["--json"])?;
            let [file] = files.as_slice() else {
                return Err("test 需要恰好一个谱文件".into());
            };
            Ok(isa_test::run(file, json))
        }
        "fmt" => {
            let out = flag_value(&args[1..], "--out")?;
            let files = paths(&args[1..], &["--out"])?;
            // `--out <file>` 的值会被 paths() 当成位置参数 → 允许 1~2 个并取第一个。
            let Some(file) = files.first() else {
                return Err("fmt 需要一个谱文件".into());
            };
            Ok(cmd_fmt(file, out))
        }
        "schema" => {
            // `--out <file>`：写文件（仓库根的 `isa-dsl.schema.json` 就是这样生成的）；
            // 缺省打印到 stdout。
            let out = flag_value(&args[1..], "--out")?;
            Ok(cmd_schema(out))
        }
        other => Err(format!("未知子命令 `{other}`")),
    }
}

/// 取 `--flag <值>` 的值（缺省 `None`）。
fn flag_value(args: &[String], flag: &str) -> Result<Option<PathBuf>, String> {
    for (i, a) in args.iter().enumerate() {
        if a == flag {
            let Some(v) = args.get(i + 1) else {
                return Err(format!("`{flag}` 需要一个取值"));
            };
            return Ok(Some(PathBuf::from(v)));
        }
    }
    Ok(None)
}

/// 摘掉 `--flag <值>`（连值一起），返回 (剩余参数, 值)（v19 V4c：`lint --ops <表>`）。
///
/// 与 [`flag_value`] 的分工：`flag_value` 只读取、留给 `paths()` 去撞"值被当位置参数"；
/// 多文件子命令（lint）必须把值真的摘出去，否则 op 表路径会被当成谱文件。
fn split_flag_value(args: &[String], flag: &str) -> Result<(Vec<String>, Option<PathBuf>), String> {
    let mut rest = Vec::new();
    let mut value = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            let Some(v) = args.get(i + 1) else {
                return Err(format!("`{flag}` 需要一个取值"));
            };
            value = Some(PathBuf::from(v));
            i += 2;
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    Ok((rest, value))
}

/// `lint`：静态体检（v19 V4a/V4c）——**不执行、不编译**，只报"写了却用不上 / 自相矛盾 /
/// 宿主覆盖不到"的声明。
///
/// 退出码与 `validate` 一致：`0` 干净、`1` 有结论或有诊断、`2` 用法错误。
/// 多文件谱：先按 `validate` 的口径报诊断（诊断带**来源文件**），体检查询跑在**合并后**的
/// 文稿上（行号与 `forge-isa fmt` 的文稿一致——已在消息里写明路径前缀）。
///
/// 四个可选档（V4c/V4d）：`--ops <宿主 op 表>` 走能力缺口检查（三类分开报，另打印覆盖率口径）、
/// `--refs` 报"声明了却没被任何模板行首引用的 `ref`"、`--bits` 报"指令字里没有位域覆盖的位段"、
/// `--suggest` 报"可合并为 `vary` 的族"（后三者默认关——预留名字/保留位/写法取舍都属作者意图）。
fn cmd_lint(
    files: &[PathBuf],
    json: bool,
    refs: bool,
    ops_path: Option<&Path>,
    bits: bool,
    suggest: bool,
) -> ExitCode {
    let mut opts = forge_isa_dsl::lint::LintOpts {
        check_unused_refs: refs,
        check_unassigned_bits: bits,
        suggest_vary: suggest,
        ..Default::default()
    };
    if let Some(p) = ops_path {
        let text = match std::fs::read_to_string(p) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("读宿主 op 表 {} 失败：{e}", p.display());
                return ExitCode::from(1);
            }
        };
        match forge_isa_dsl::lint::host_ops_from_toml(&text) {
            Ok(v) => opts.host_ops = v,
            Err(e) => {
                eprintln!("{}: {e}", p.display());
                return ExitCode::from(1);
            }
        }
    }
    let mut clean = true;
    let mut json_rows: Vec<String> = Vec::new();
    for f in files {
        let spec = match report::load_spec(f) {
            Ok(s) => s,
            Err(d) => {
                print_diags(f, &d);
                clean = false;
                continue;
            }
        };
        // 校验不过就先报诊断（与 validate 同一套渲染），不再谈"干净不干净"。
        let diags = report::validate_loaded(&spec);
        if !diags.is_empty() {
            print_diags(f, &diags);
            clean = false;
            continue;
        }
        let result = forge_isa_dsl::lint::lint_source_opts(&spec.text, &opts).unwrap_or_default();
        let found = &result.findings;
        if found.is_empty() {
            if !json {
                println!("{}: 干净（无 lint 结论）", f.display());
            }
        } else {
            clean = false;
            for d in found {
                if json {
                    json_rows.push(format!(
                        "{{\"spec\":{},\"code\":{},\"line\":{},\"col\":{},\"msg\":{}}}",
                        json_str(&f.display().to_string()),
                        json_str(&d.code),
                        d.line,
                        d.col,
                        json_str(&d.msg)
                    ));
                } else {
                    println!(
                        "{}:{}:{}: {}: {}",
                        f.display(),
                        d.line,
                        d.col,
                        d.code,
                        d.msg
                    );
                }
            }
        }
        // 能力缺口口径（给了 `--ops` 才有）：三类分开打印，便于直接和宿主对账。
        if let Some(cov) = &result.ops_coverage {
            if json {
                json_rows.push(format!(
                    "{{\"spec\":{},\"code\":\"OPS-COVERAGE\",\"covered\":{},\"terminators\":{},\
                     \"host_pipeline\":{},\"gaps\":{}}}",
                    json_str(&f.display().to_string()),
                    cov.covered,
                    cov.terminators,
                    cov.host_pipeline,
                    cov.gaps.len()
                ));
            } else {
                println!(
                    "# {}: 宿主 op 覆盖 = [[lowering]]+[[pattern]] {} / 终结指令 {}（谱里不该有）\
                     / 宿主管线 {}（宿主直查）/ 真缺口 {}",
                    f.display(),
                    cov.covered,
                    cov.terminators,
                    cov.host_pipeline,
                    cov.gaps.len()
                );
            }
        }
    }
    if json {
        println!("{{\"findings\":[{}]}}", json_rows.join(","));
    }
    if clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// 最小 JSON 字符串转义（不引 `serde_json`，见 `docs/archive/forge-dsl-v18-plan.md` §10.4）。
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `fmt`：把 **include 展开 + override 应用**后的规范文稿打出来（人工核对用）。
///
/// 不做"原地重排/美化"——那会丢掉注释与用户排版；它的价值是让多文件谱有一个
/// **确定性、可 diff** 的等价单文件视图（诊断的行号也指向这份文稿）。
fn cmd_fmt(file: &Path, out: Option<PathBuf>) -> ExitCode {
    let spec = match report::load_spec(file) {
        Ok(s) => s,
        Err(d) => {
            print_diags(file, &d);
            return ExitCode::from(1);
        }
    };
    match out {
        Some(path) => match std::fs::write(&path, &spec.text) {
            Ok(()) => {
                println!(
                    "写出 {}（来源 {} 个文件：{}）",
                    path.display(),
                    spec.sources.len(),
                    spec.sources
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("写 {} 失败：{e}", path.display());
                ExitCode::from(1)
            }
        },
        None => {
            print!("{}", spec.text);
            ExitCode::SUCCESS
        }
    }
}

fn cmd_schema(out: Option<PathBuf>) -> ExitCode {
    let schema = forge_isa_dsl::schema::schema_json();
    match out {
        Some(path) => match std::fs::write(&path, schema) {
            Ok(()) => {
                println!("写出 {}", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("写 {} 失败：{e}", path.display());
                ExitCode::from(1)
            }
        },
        None => {
            print!("{schema}");
            ExitCode::SUCCESS
        }
    }
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

/// 取出 `--params a=1,b=2`（可多次），返回 (剩余参数, 档位)（v19 V5 变体投影）。
fn split_params(args: &[String]) -> Result<(Vec<String>, report::RunOpts), String> {
    let mut rest = Vec::new();
    let mut list: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--params" {
            let Some(v) = args.get(i + 1) else {
                return Err("`--params` 需要一个取值（如 `xlen=32`，多个用逗号分隔）".into());
            };
            list.extend(
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            );
            i += 2;
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    let opts = report::RunOpts::with_params(&list)?;
    Ok((rest, opts))
}

/// 位置参数（过滤已知开关）；`-` 开头的未知项按用法错误处理。
fn paths(args: &[String], flags: &[&str]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for a in args {
        if flags.contains(&a.as_str()) {
            continue;
        }
        if a.starts_with('-') {
            return Err(format!("未知开关 `{a}`"));
        }
        out.push(PathBuf::from(a));
    }
    Ok(out)
}

/// 打印诊断（`路径:行:列: 码: 消息` + 附注），返回是否有错。
fn print_diags(path: &Path, diags: &[DiagLine]) -> bool {
    for d in diags {
        // 多文件谱：诊断自带来源文件（include 里的错要指向那个文件）；否则用根文件。
        let shown = d.file.as_deref().unwrap_or("");
        let shown = if shown.is_empty() {
            path.display().to_string()
        } else {
            shown.to_string()
        };
        println!("{}:{}:{}: {}: {}", shown, d.line, d.col, d.code, d.msg);
        for n in &d.notes {
            println!("  = {n}");
        }
    }
    !diags.is_empty()
}

fn cmd_validate(files: &[PathBuf], opts: &report::RunOpts) -> ExitCode {
    let mut bad = false;
    for f in files {
        let (diags, name) = report::validate_file_opts(f, opts);
        if diags.is_empty() {
            println!(
                "{}: OK{}",
                f.display(),
                name.map(|n| format!("（ISA = {n}）")).unwrap_or_default()
            );
        } else {
            bad = true;
            print_diags(f, &diags);
        }
    }
    if bad {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn cmd_insts(file: &Path, json: bool, opts: &report::RunOpts) -> ExitCode {
    let spec = match report::load_spec(file) {
        Ok(s) => s,
        Err(d) => {
            print_diags(file, &d);
            return ExitCode::from(1);
        }
    };
    match report::insts_projected_loaded(&spec, opts) {
        Err(diags) => {
            print_diags(file, &diags);
            ExitCode::from(1)
        }
        Ok((isa, rows, proj)) => {
            if json {
                println!("{}", insts_json(&isa, &rows, &proj));
            } else {
                println!(
                    "# {} （version {}；encoding = {}；{} 条指令 / {} 条模板 / {} 条 lowering）",
                    isa.name,
                    // `[meta].version`（缺省 = `-`）。这里不能叫 "schema"：它是 ISA 自己的
                    // 版本串，DSL 语法版本靠文档/CHANGELOG 追踪，不写进 TOML。
                    isa.version.as_deref().unwrap_or("-"),
                    kind_text(&isa),
                    isa.instructions,
                    isa.templates,
                    isa.lowering_rules
                );
                if let Some(line) = projection_line(&proj) {
                    println!("{line}");
                }
                for r in &rows {
                    println!("{}", inst_line(r));
                }
            }
            ExitCode::SUCCESS
        }
    }
}

/// 变体投影账目（v19 V5）：`--params` 为空时返回 `None`（默认档不打这行）。
///
/// 只读投影的"证据"就是这一行：丢了几条指令（哪些）、逐节丢了什么（含连带丢的
/// lowering）、投影后各剩多少——不给数字的投影等于没说清动了什么。
fn projection_line(p: &report::Projection) -> Option<String> {
    if p.params.is_empty() {
        return None;
    }
    let args: Vec<String> = p.params.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let dropped = if p.dropped_insts.is_empty() {
        "无".to_string()
    } else {
        p.dropped_insts.join(", ")
    };
    let decls: Vec<String> = p
        .dropped_decls
        .iter()
        .map(|(what, n)| format!("{what} ×{n}"))
        .collect();
    let decls = if decls.is_empty() {
        "无".to_string()
    } else {
        decls.join("、")
    };
    Some(format!(
        "# 变体投影 {}：指令 {} → {}（-{}：{}）；连带/逐节丢弃：{}；lowering 剩 {}",
        args.join(" "),
        p.inst_count + p.dropped_insts.len(),
        p.inst_count,
        p.dropped_insts.len(),
        dropped,
        decls,
        p.lowering_count
    ))
}

fn kind_text(isa: &report::IsaSummary) -> String {
    match isa.encoding_kind.as_str() {
        "fixed" => format!("fixed {} 位", isa.widths_bits.first().copied().unwrap_or(0)),
        "mixed" => format!("mixed {:?}", isa.widths_bits),
        other => format!(
            "{other}（max_len = {}）",
            isa.max_len.map(|m| m.to_string()).unwrap_or("-".into())
        ),
    }
}

fn inst_line(r: &InstRow) -> String {
    let enc = r
        .enc
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut s = format!(
        "{:<28} width={:<8} form={:<14} opcode={:<8} ops=[{}]",
        r.name,
        r.width_bits
            .map(|w| format!("{w}b/{}B", r.len_bytes.unwrap_or(0)))
            .unwrap_or_else(|| "-".into()),
        r.form.as_deref().unwrap_or("-"),
        r.opcode
            .map(|o| format!("{o:#x}"))
            .unwrap_or_else(|| "-".into()),
        r.ops.join(",")
    );
    if let Some(t) = &r.from_template {
        s.push_str(&format!(
            " template={}[{}]",
            t,
            r.template_row
                .map(|i| i.to_string())
                .unwrap_or_else(|| "-".into())
        ));
    }
    if let Some(rf) = &r.reference {
        s.push_str(&format!(" ref={rf}"));
    }
    if let Some(rl) = &r.reloc {
        s.push_str(&format!(" reloc={rl}"));
    }
    if !enc.is_empty() {
        s.push_str(&format!(" enc={{{enc}}}"));
    }
    s.push_str(&format!(" asm=\"{}\"", r.asm));
    s
}

fn cmd_explain(file: &Path, inst: &str, json: bool) -> ExitCode {
    let spec = match report::load_spec(file) {
        Ok(s) => s,
        Err(d) => {
            print_diags(file, &d);
            return ExitCode::from(1);
        }
    };
    match report::explain_loaded(&spec, inst) {
        Err(diags) => {
            print_diags(file, &diags);
            ExitCode::from(1)
        }
        Ok(e) => {
            if json {
                println!("{}", explain_json(&e));
            } else {
                print_explain(&e);
            }
            ExitCode::SUCCESS
        }
    }
}

fn print_explain(e: &Explain) {
    println!(
        "ISA {} （encoding = {}；{} 条指令）",
        e.isa.name,
        kind_text(&e.isa),
        e.isa.instructions
    );
    println!("指令 {}", e.inst.name);
    if let Some(t) = &e.template {
        println!("  来源：[[templates.{}]] 第 {} 行", t.template, t.index);
        if !t.body.is_empty() {
            println!("  模板 body：");
            for (k, v) in &t.body {
                println!("    {k} = {v}");
            }
        }
        println!("  该行：");
        for (k, v) in &t.row {
            println!("    {k} = {v}");
        }
    } else {
        println!("  来源：[[instructions]] 手写指令");
    }
    println!("  生效规格：");
    for (k, v) in e.inst.fields() {
        if v.is_empty() {
            continue;
        }
        println!("    {k} = {v}");
    }
}

fn cmd_diff(a: &Path, b: &Path, json: bool) -> ExitCode {
    let (sa, sb) = match (report::load_spec(a), report::load_spec(b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(d), _) | (_, Err(d)) => {
            print_diags(a, &d);
            return ExitCode::from(1);
        }
    };
    match report::diff_loaded(&sa, &sb) {
        Err(diags) => {
            print_diags(a, &diags);
            ExitCode::from(1)
        }
        Ok(d) => {
            if json {
                println!("{}", diff_json(&d));
            } else {
                print_diff(a, b, &d);
            }
            ExitCode::SUCCESS
        }
    }
}

fn print_diff(a: &Path, b: &Path, d: &SpecDiff) {
    if d.is_empty() {
        println!("{} == {}：规格完全相同", a.display(), b.display());
        return;
    }
    println!("{} → {}", a.display(), b.display());
    for n in &d.added {
        println!("  + {n}");
    }
    for n in &d.removed {
        println!("  - {n}");
    }
    for (n, fields) in &d.changed {
        println!("  ~ {n}");
        for f in fields {
            println!("      {f}");
        }
    }
    println!(
        "共 {} 增 / {} 删 / {} 改",
        d.added.len(),
        d.removed.len(),
        d.changed.len()
    );
}

// ─────────────────────────── 极简 JSON 发射器 ───────────────────────────

fn jstr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn jarr(items: &[String]) -> String {
    format!("[{}]", items.join(","))
}

fn jpairs(items: &[(String, String)]) -> String {
    let body: Vec<String> = items
        .iter()
        .map(|(k, v)| format!("{}:{}", jstr(k), jstr(v)))
        .collect();
    format!("{{{}}}", body.join(","))
}

fn insts_json(isa: &report::IsaSummary, rows: &[InstRow], p: &report::Projection) -> String {
    let isa_obj = format!(
        "{{\"name\":{},\"version\":{},\"encoding\":{},\"widths_bits\":{},\"max_len\":{},\
         \"templates\":{},\"instructions\":{},\"lowering\":{},\"pseudo\":{},\"reloc\":{},\"derive\":{}}}",
        jstr(&isa.name),
        isa.version
            .as_deref()
            .map(jstr)
            .unwrap_or_else(|| "null".into()),
        jstr(&isa.encoding_kind),
        jarr(
            &isa.widths_bits
                .iter()
                .map(|w| w.to_string())
                .collect::<Vec<_>>()
        ),
        isa.max_len
            .map(|m| m.to_string())
            .unwrap_or_else(|| "null".into()),
        isa.templates,
        isa.instructions,
        isa.lowering_rules,
        isa.pseudo,
        isa.reloc,
        isa.derive
    );
    let rows_json: Vec<String> = rows.iter().map(row_json).collect();
    let params_json = format!(
        "{{{}}}",
        p.params
            .iter()
            .map(|(k, v)| format!("{}:{v}", jstr(k)))
            .collect::<Vec<_>>()
            .join(",")
    );
    let proj_json = format!(
        "{{\"params\":{params_json},\"dropped_insts\":{},\"dropped_decls\":{},\
         \"inst_count\":{},\"lowering_count\":{}}}",
        jarr(&p.dropped_insts.iter().map(|s| jstr(s)).collect::<Vec<_>>()),
        jarr(
            &p.dropped_decls
                .iter()
                .map(|(what, n)| format!("[{},{}]", jstr(what), n))
                .collect::<Vec<_>>()
        ),
        p.inst_count,
        p.lowering_count
    );
    format!(
        "{{\"isa\":{isa_obj},\"projection\":{proj_json},\"insts\":[{}]}}",
        rows_json.join(",")
    )
}

fn row_json(r: &InstRow) -> String {
    format!(
        "{{\"name\":{},\"from_template\":{},\"template_row\":{},\"width_bits\":{},\"len_bytes\":{},\
         \"form\":{},\"opcode\":{},\"ops\":{},\"asm\":{},\"ref\":{},\"reloc\":{},\"enc\":{}}}",
        jstr(&r.name),
        r.from_template
            .as_deref()
            .map(jstr)
            .unwrap_or_else(|| "null".into()),
        r.template_row
            .map(|i| i.to_string())
            .unwrap_or_else(|| "null".into()),
        r.width_bits
            .map(|w| w.to_string())
            .unwrap_or_else(|| "null".into()),
        r.len_bytes
            .map(|w| w.to_string())
            .unwrap_or_else(|| "null".into()),
        r.form.as_deref().map(jstr).unwrap_or_else(|| "null".into()),
        r.opcode
            .map(|o| o.to_string())
            .unwrap_or_else(|| "null".into()),
        jarr(&r.ops.iter().map(|o| jstr(o)).collect::<Vec<_>>()),
        jstr(&r.asm),
        r.reference
            .as_deref()
            .map(jstr)
            .unwrap_or_else(|| "null".into()),
        r.reloc
            .as_deref()
            .map(jstr)
            .unwrap_or_else(|| "null".into()),
        jpairs(&r.enc)
    )
}

fn explain_json(e: &Explain) -> String {
    let tmpl = match &e.template {
        None => "null".to_string(),
        Some(t) => format!(
            "{{\"template\":{},\"index\":{},\"body\":{},\"row\":{}}}",
            jstr(&t.template),
            t.index,
            jpairs(&t.body),
            jpairs(&t.row)
        ),
    };
    format!(
        "{{\"isa\":{},\"inst\":{},\"template\":{tmpl}}}",
        jstr(&e.isa.name),
        row_json(&e.inst)
    )
}

fn diff_json(d: &SpecDiff) -> String {
    let changed: Vec<String> = d
        .changed
        .iter()
        .map(|(n, fs)| {
            format!(
                "{{\"name\":{},\"fields\":{}}}",
                jstr(n),
                jarr(&fs.iter().map(|f| jstr(f)).collect::<Vec<_>>())
            )
        })
        .collect();
    format!(
        "{{\"added\":{},\"removed\":{},\"changed\":[{}]}}",
        jarr(&d.added.iter().map(|n| jstr(n)).collect::<Vec<_>>()),
        jarr(&d.removed.iter().map(|n| jstr(n)).collect::<Vec<_>>()),
        changed.join(",")
    )
}
