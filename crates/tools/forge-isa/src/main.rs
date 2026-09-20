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
//!
//! 约定：默认人类可读输出，`--json` 走机读（手写发射器，不引 `serde_json`——
//! 与"不新增依赖"的显式假设一致，见 `docs/plans/forge-dsl-v18-plan.md` §10.4）。
//! 退出码：`0` 成功、`1` 诊断/失败、`2` 用法错误。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use forge_isa_dsl::report::{self, DiagLine, Explain, InstRow, SpecDiff};

const USAGE: &str = "\
forge-isa — ISA-DSL 工具链（v18 S7b）

用法：
  forge-isa validate <谱.toml>...            解析 + 校验，打印全部诊断
  forge-isa insts    <谱.toml> [--json]      列出展开后的指令与生效规格
  forge-isa explain  <谱.toml> <指令名> [--json]
                                             单条指令的来源（模板行 + 生效编码键）
  forge-isa diff     <a.toml> <b.toml> [--json]
                                             两份谱的规格 diff（增/删/改字段）
  forge-isa schema   [--out <file>]         打印（或写出）ISA-DSL 的 JSON Schema
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
            let files = paths(&args[1..], &[])?;
            if files.is_empty() {
                return Err("validate 需要一个或多个谱文件".into());
            }
            Ok(cmd_validate(&files))
        }
        "insts" => {
            let json = has_flag(&args[1..], "--json");
            let files = paths(&args[1..], &["--json"])?;
            let [file] = files.as_slice() else {
                return Err("insts 需要恰好一个谱文件".into());
            };
            Ok(cmd_insts(file, json))
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
        println!(
            "{}:{}:{}: {}: {}",
            path.display(),
            d.line,
            d.col,
            d.code,
            d.msg
        );
        for n in &d.notes {
            println!("  = {n}");
        }
    }
    !diags.is_empty()
}

fn cmd_validate(files: &[PathBuf]) -> ExitCode {
    let mut bad = false;
    for f in files {
        let (diags, name) = report::validate_file(f);
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

fn cmd_insts(file: &Path, json: bool) -> ExitCode {
    let source = match report::read_source(file) {
        Ok(s) => s,
        Err(d) => {
            print_diags(file, &d);
            return ExitCode::from(1);
        }
    };
    match report::insts(&source) {
        Err(diags) => {
            print_diags(file, &diags);
            ExitCode::from(1)
        }
        Ok((isa, rows)) => {
            if json {
                println!("{}", insts_json(&isa, &rows));
            } else {
                println!(
                    "# {} （schema {}；encoding = {}；{} 条指令 / {} 条模板 / {} 条 lowering）",
                    isa.name,
                    isa.version.as_deref().unwrap_or("-"),
                    kind_text(&isa),
                    isa.instructions,
                    isa.templates,
                    isa.lowering_rules
                );
                for r in &rows {
                    println!("{}", inst_line(r));
                }
            }
            ExitCode::SUCCESS
        }
    }
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
    let source = match report::read_source(file) {
        Ok(s) => s,
        Err(d) => {
            print_diags(file, &d);
            return ExitCode::from(1);
        }
    };
    match report::explain(&source, inst) {
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
    let (sa, sb) = match (report::read_source(a), report::read_source(b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(d), _) | (_, Err(d)) => {
            print_diags(a, &d);
            return ExitCode::from(1);
        }
    };
    match report::diff(&sa, &sb) {
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

fn insts_json(isa: &report::IsaSummary, rows: &[InstRow]) -> String {
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
    format!("{{\"isa\":{isa_obj},\"insts\":[{}]}}", rows_json.join(","))
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
