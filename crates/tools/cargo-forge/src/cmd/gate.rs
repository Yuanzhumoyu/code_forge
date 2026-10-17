//! `cargo forge gate` —— **一条命令跑全套门禁**（v26 M3）。
//!
//! ## 为什么要它
//!
//! 立项审查实测：全套门禁（fmt / clippy×2 / workspace tests / codegen lib / 十几条守卫 /
//! JIT 两矩阵 / asm 三档 / 文档 lint）此前**只写在 `CLAUDE.md` 里**，靠人手抄；抄漏一条
//! 没人发现，失败也不知该找谁。这个子命令把那份清单变成**可执行的注册表**。
//!
//! ## 形态
//!
//! * **一张守卫注册表** [`GUARDS`]：新增守卫只加一行；`--guards` 只跑它（改一条守卫时的快循环）。
//! * **逐项报告**：每步打印 `[i/n] 名称 —— 在验什么`、用时与退出码；末尾汇总表 + 非零退出。
//! * **子集** `--subset quick|linux|windows`：CI 直接复用（Linux 子集不含 `forge-codegen` 的
//!   JIT 测试——那套按 Windows x64 ABI 生成，Linux 宿主上确定性 SIGSEGV，见 `ci.yml` 注释）。
//! * **文档步**：`git ls-files '*.md'` **去掉 `docs/archive/**`**（归档文档冻结、明示豁免；
//!   实测全仓 116 条存量问题全在那 8 个归档文件里），`npx` 缺失时打印 SKIP 而不是失败。
//! * `--verbose` 之外的默认行为：子进程**继承 stdio**（长测试能看到进度），失败直接可见。

use std::process::Command;
use std::time::Instant;

use clap::Args as ClapArgs;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// 只跑某个子集：`quick`（fmt + 守卫 + 文档）/ `linux` / `windows`
    #[arg(long, value_name = "SUBSET")]
    pub subset: Option<String>,

    /// 只跑守卫注册表（改一条守卫时的快循环）
    #[arg(long)]
    pub guards: bool,

    /// 跳过文档 lint
    #[arg(long)]
    pub no_docs: bool,

    /// 打印机器可读汇总（JSON）
    #[arg(long)]
    pub json: bool,

    /// 只列出将要执行的步骤，不运行
    #[arg(long)]
    pub list: bool,
}

/// 守卫注册表：`(crate, 测试目标, 这条守卫钉什么)`。
///
/// 判据：**跨切面的台账/边界守卫**（"两个地方必须一致"这一类）。行为型测试套件不在此列——
/// 它们由 `workspace` 步整体覆盖。
pub const GUARDS: &[(&str, &str, &str)] = &[
    (
        "forge-codegen",
        "file_size_ratchet",
        "文件规模棘轮 + 叶子方向 + 契约文档的规模表/目录表",
    ),
    (
        "forge-codegen",
        "error_taxonomy",
        "错误分类学：`[cap:ID]` / `Internal` / `Semantic` + 文档分类表",
    ),
    (
        "forge-codegen",
        "events_guard",
        "事件名与事件通道三方双向（`EVENTS`/`CHANNELS`/契约文档）",
    ),
    (
        "forge-codegen",
        "classification_single_entry",
        "ABI 落点唯一入口（分类器唯一通道）",
    ),
    (
        "forge-codegen",
        "capability_gaps",
        "能力缺口台账与源码缺口集合双向相等",
    ),
    (
        "forge-codegen",
        "abi_target_real",
        "ABI 落点真机核对（含 v27 M4-T2：抽象 `c` 按目标落 win64/aapcs64）",
    ),
    (
        "forge-codegen",
        "reg_name_table",
        "`TargetRegInfo::reg_name` 给出汇编器认得的寄存器拼写（v27 S2 前置）",
    ),
    (
        "forge-codegen",
        "rustc_doc_counts",
        "forge-rustc 文档计数与 `tests/e2e.rs` 双向相等",
    ),
    (
        "forge-codegen",
        "workarounds_guard",
        "`[WA-NN]` 编号唯一 + 代码引用可解析",
    ),
    (
        "forge-codegen",
        "trace_knobs",
        "观测开关三方双向（源码字面量 / `KNOBS` / 文档表）",
    ),
    (
        "forge-codegen",
        "entity_tables",
        "实体表单一来源（读写路径不各存一份）",
    ),
    (
        "forge-codegen",
        "generic_pipeline_is_isa_neutral",
        "通用管线与具体 ISA 解耦",
    ),
    (
        "forge-codegen",
        "no_hardcoded_widths",
        "宽度不写死（白名单带理由且必须被命中）",
    ),
    (
        "forge-codegen",
        "library_surface",
        "库本体不含夹具谱（demo 谱只住 tests/）",
    ),
    ("forge-codegen", "lowering_read_path", "合法化读路径预算"),
    (
        "forge-codegen",
        "conv_registry_read_path",
        "调用约定注册表的读路径",
    ),
    (
        "forge-ir",
        "read_path_budget",
        "IR 读路径预算（borrow 计数棘轮）",
    ),
    (
        "forge-ir",
        "text_feature_gate",
        "文本层可关：核心不依赖 `ir_parser`/`display`",
    ),
    (
        "forge-ir",
        "data_layout_single_source",
        "`DataLayout` 单一来源",
    ),
    ("forge-ir", "metadata_single_write", "metadata 单写约定"),
    ("forge-ir", "entity_privatization", "实体字段私有化"),
    ("forge-ir", "open_set_boundary", "开放集合的边界语义"),
    (
        "forge-isa-dsl",
        "error_codes_match_doc",
        "错误码台账双向（源码字面量 ↔ 文档 §2 表）",
    ),
    (
        "forge-isa-dsl",
        "schema_guard",
        "schema ↔ 模型 ↔ 文档三方针 + 签入的 JSON Schema",
    ),
    (
        "forge-isa-dsl",
        "lint_shipped",
        "发行谱 lint 零结论（真阳性请修谱）",
    ),
    ("forge-isa-dsl", "determinism", "生成物确定性"),
    ("forge-isa-dsl", "field_names", "字段名 = 操作数声明名"),
    ("forge-isa-dsl", "short_form_fold", "短名折叠后无长形残留"),
    (
        "forge-isa-dsl",
        "machine_shape_table",
        "形状表与两个访问器覆盖同一批变体",
    ),
    (
        "forge-isa-dsl",
        "lowering_attrs_once",
        "谓词属性源只发射一次",
    ),
    ("forge-isa-dsl", "generality_guard", "通用性守卫"),
    ("forge-isa-dsl", "move_derive", "搬运族按操作数结构派生"),
];

/// 一步门禁：名字 + 在验什么 + 命令（`cargo` 或 `npx`）。
struct Step {
    name: &'static str,
    what: &'static str,
    program: &'static str,
    args: Vec<String>,
}

/// 哪些步骤属于哪个子集。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// 所有子集（fmt / 守卫 / 文档）
    Always,
    /// 只有非 `quick` 子集
    Full,
    /// 只有 `windows` / `all`
    Windows,
}

impl Args {
    fn subset_name(&self) -> &'static str {
        match self.subset.as_deref() {
            Some("quick") => "quick",
            Some("linux") => "linux",
            Some("windows") => "windows",
            Some("rustc") => "rustc",
            Some(other) => {
                eprintln!(
                    "未知子集 `{other}`（可用：quick / linux / windows / rustc）——按 all 处理"
                );
                "all"
            }
            None => "all",
        }
    }
}

fn cargo_step(
    name: &'static str,
    what: &'static str,
    scope: Scope,
    args: &[&str],
) -> (Step, Scope) {
    (
        Step {
            name,
            what,
            program: "cargo",
            args: args.iter().map(|s| (*s).to_string()).collect(),
        },
        scope,
    )
}

fn asm_step(tier: &'static str) -> (Step, Scope) {
    cargo_step(
        match tier {
            "asm_parse" => "asm-parse",
            "asm_encoding" => "asm-encoding",
            _ => "asm-exec",
        },
        match tier {
            "asm_parse" => "汇编语料解析档（四桶归因 + 计数棘轮）",
            "asm_encoding" => "汇编语料编码档（与上游 encoding 字节对拍）",
            _ => "汇编语料真执行档（x86 原生 / riscv64·aarch64 走 QEMU）",
        },
        Scope::Full,
        &["test", "-p", "forge-tests", "--test", tier, "-j", "4"],
    )
}

/// 该步骤在本子集里要不要跑。
fn keep(scope: Scope, subset: &str) -> bool {
    match scope {
        Scope::Always => true,
        Scope::Full => subset != "quick",
        Scope::Windows => subset != "linux",
    }
}

/// 组装本次要跑的步骤。
fn build_steps(a: &Args) -> Vec<Step> {
    let subset = a.subset_name();
    let mut out = Vec::new();
    // 用宏而不是闭包：闭包会借住 `out`，而注册表循环还要直接 push（E0499 实测撞到）。
    // 宏只取一个 `(Step, Scope)` 表达式，调用点形状与原来的 `push(...)` 完全一致。
    macro_rules! push {
        ($e:expr) => {{
            let (step, scope) = $e;
            if keep(scope, subset) {
                out.push(step);
            }
        }};
    }

    // ── 快循环：只看守卫 ──────────────────────────────────────────────
    if a.guards {
        for (krate, target, what) in GUARDS {
            out.push(Step {
                name: target,
                what,
                program: "cargo",
                args: vec![
                    "test".into(),
                    "-p".into(),
                    (*krate).into(),
                    "--test".into(),
                    (*target).into(),
                    "-j".into(),
                    "4".into(),
                ],
            });
        }
        return out;
    }

    // ── 后端真值（v27 M5：把 forge-rustc 的 e2e 纳入门禁；此前只跑 `check -p forge-rustc`，
    //    110 条"编译→链接→真执行"用例只活在 CI 作业里 ⇒ 本地"全套门禁通过"并不覆盖后端）──
    //    仅 Windows 口径：e2e 目前只验证 x86_64-pc-windows-msvc 宿主（跨目标见方案 M4）。
    //    `--release` 无关；单线程跑（用例会起 rustc 子进程 + 真跑产物）。
    push!(cargo_step(
        "rustc-e2e",
        "rustc 后端 e2e（编译→链接→真执行，`--test-threads=1`；用例数以 tests/e2e.rs 为准）",
        Scope::Windows,
        &[
            "test",
            "-p",
            "forge-rustc",
            "--test",
            "e2e",
            "--",
            "--test-threads=1",
            // 环境性用例：它要 rustup **下载固定的 dated 通道**（实测在本机/沙箱里因
            // 写 `~/.rustup` 或没有网络而失败）。门禁跑"能跑的"，这个用例由 CI 的
            // `forge-rustc-e2e` 作业（有网络、有全量通道）覆盖——不在这里装作跑过。
            "--skip",
            "e2e_cargo_template_workflow",
        ],
    ));
    push!(cargo_step(
        "rustc-e2e-guards",
        "rustc 后端 e2e 不变量（trap 语义 / 零容忍名单 / 模板契约）",
        Scope::Windows,
        &[
            "test",
            "-p",
            "forge-rustc",
            "--test",
            "e2e",
            "--",
            "e2e_trap_semantics_invariants",
        ],
    ));

    // ── 格式与静态检查 ────────────────────────────────────────────────
    // 多 CGU / 并行 / 增量的**确定性**（WA-38/39 的核心承诺）：此前只在 CI 作业里跑，
    // 本地"全套门禁"其实没覆盖它。实测 3 个测试 16s，够便宜 ⇒ 进门禁。
    push!(cargo_step(
        "rustc-determinism",
        "rustc 后端确定性（多 CGU 逐字节相等 / 并行==串行 / 增量）",
        Scope::Windows,
        &[
            "test",
            "-p",
            "forge-rustc",
            "--test",
            "determinism",
            "--",
            "--test-threads=1",
        ],
    ));
    // `-C opt-level` 六个档位 + 同档确定性（v27 M5 补面：e2e 只在 debug 档跑过）。
    push!(cargo_step(
        "rustc-opt-levels",
        "rustc 后端 `-C opt-level` 全档回归（0/1/2/3/s/z 同一答案 + 同档逐字节相等）",
        Scope::Windows,
        &[
            "test",
            "-p",
            "forge-rustc",
            "--test",
            "opt_levels",
            "--",
            "--test-threads=1",
        ],
    ));
    // DWARF 调试段的**自动**核对（此前"DWARF5 已实现"只靠手工跑 gdb 验过）。
    push!(cargo_step(
        "rustc-dwarf",
        "rustc 后端 DWARF 段核对（debuginfo=2 有段 / =0 无段 / 语义不变）",
        Scope::Windows,
        &[
            "test",
            "-p",
            "forge-rustc",
            "--test",
            "dwarf_sections",
            "--",
            "--test-threads=1",
        ],
    ));
    push!(cargo_step(
        "fmt",
        "格式（`cargo fmt --all -- --check`）",
        Scope::Always,
        &["fmt", "--all", "--", "--check",]
    ));
    push!(cargo_step(
        "clippy",
        "全量 lint（workspace，`-D warnings`，`--all-features`）",
        Scope::Full,
        &[
            "clippy",
            "--workspace",
            "--exclude",
            "forge-rustc",
            "--all-targets",
            "--all-features",
            "-j",
            "4",
            "--",
            "-D",
            "warnings",
        ],
    ));
    push!(cargo_step(
        "text-gate",
        "文本层门（`forge-ir --no-default-features`，核心不得依赖文本层）",
        Scope::Full,
        &[
            "clippy",
            "-p",
            "forge-ir",
            "--no-default-features",
            "--lib",
            "-j",
            "4",
            "--",
            "-D",
            "warnings",
        ],
    ));
    push!(cargo_step(
        "rustc-check",
        "forge-rustc 编译（rustc_private 侧）",
        Scope::Windows,
        &["check", "-p", "forge-rustc", "-j", "4"],
    ));

    // ── 测试 ──────────────────────────────────────────────────────────
    let workspace_args: Vec<&str> = if subset == "linux" {
        // CI 的 Linux 作业排除 forge-codegen：那套 JIT 测试按 Windows x64 ABI 生成，
        // Linux 宿主上确定性 SIGSEGV（见 ci.yml 的长注释）。守卫另有注册表覆盖。
        vec![
            "test",
            "--workspace",
            "--exclude",
            "forge-rustc",
            "--exclude",
            "forge-codegen",
            "-j",
            "4",
        ]
    } else {
        vec!["test", "--workspace", "--exclude", "forge-rustc", "-j", "4"]
    };
    push!(cargo_step(
        "workspace",
        "workspace 测试（除 forge-rustc）",
        Scope::Full,
        &workspace_args,
    ));
    push!(cargo_step(
        "codegen-lib",
        "`forge-codegen` 单元测试（`--all-features`：`jit` + `parallel`）",
        Scope::Windows,
        &[
            "test",
            "-p",
            "forge-codegen",
            "--lib",
            "--all-features",
            "-j",
            "4",
        ],
    ));

    // ── 守卫注册表 ────────────────────────────────────────────────────
    for (krate, target, what) in GUARDS {
        out.push(Step {
            name: target,
            what,
            program: "cargo",
            args: vec![
                "test".into(),
                "-p".into(),
                (*krate).into(),
                "--test".into(),
                (*target).into(),
                "-j".into(),
                "4".into(),
            ],
        });
    }

    // ── 真执行矩阵 ────────────────────────────────────────────────────
    push!(cargo_step(
        "jit-x86",
        "JIT 集成矩阵（x86_64 宿主真执行）",
        Scope::Full,
        &[
            "test",
            "-p",
            "forge-tests",
            "--lib",
            "jit_matrix_x86",
            "-j",
            "4",
            "--",
            "--nocapture",
        ],
    ));
    push!(cargo_step(
        "jit-riscv64",
        "JIT 集成矩阵（riscv64 走 QEMU 真执行；需 `--test-threads=1`）",
        Scope::Full,
        &[
            "test",
            "-p",
            "forge-tests",
            "--lib",
            "jit_matrix_riscv64",
            "-j",
            "4",
            "--",
            "--test-threads=1",
            "--nocapture",
        ],
    ));
    for tier in ["asm_parse", "asm_encoding", "asm_exec"] {
        push!(asm_step(tier));
    }

    out
}

/// 文档步：`git ls-files '*.md'` 去掉归档（归档文档冻结，明示豁免），交给 markdownlint-cli2。
fn docs_step() -> Option<Step> {
    let out = Command::new("git")
        .args(["ls-files", "*.md"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let files: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        // 归档文档冻结：`docs/archive/**` 明示豁免（实测全仓存量问题全在那里）
        .filter(|l| !l.starts_with("docs/archive/"))
        .map(str::to_string)
        .collect();
    if files.is_empty() {
        return None;
    }
    let mut args = npx_prefix();
    args.push("--yes".to_string());
    args.push("markdownlint-cli2".to_string());
    args.extend(files);
    Some(Step {
        name: "docs",
        what: "文档 lint（全部纳入版本管理的 *.md，归档除外；`npx` 缺失则 SKIP）",
        program: npx_program(),
        args,
    })
}

/// `npx` 的缓存目录兜底。
///
/// **实测坑**：本机（DSH 沙箱）npx 的默认缓存放不开，`npm_config_cache` 没设时
/// markdownlint-cli2 会**秒退非零**——单独在 shell 里跑（已设该变量）却全绿，
/// 于是门禁的文档步时红时绿。这里在「调用方没设」时兜底到 `<repo>/target/tmp/npm-cache`
/// （workspace 内、可写；`docs/guides/lint.md` 记的就是这个路径）。
fn npm_cache_fallback() -> Option<std::path::PathBuf> {
    if std::env::var_os("npm_config_cache").is_some() {
        return None;
    }
    // 从可执行文件往上找仓库根（`target/debug/cargo-forge` ⇒ 上两级）
    let exe = std::env::current_exe().ok()?;
    let root = exe.parent()?.parent()?.parent()?;
    let dir = root.join("target").join("tmp").join("npm-cache");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

/// `npx` 的可执行程序名。
///
/// **Windows 实测坑**：`npx` 是 `npx.cmd`，而 `Command::new("npx")` 走
/// `CreateProcess`、不按 `PATHEXT` 解析 ⇒ spawn 失败、文档步被**静默 SKIP**
/// （第一次跑全套门禁时就是这样，汇总里少了一步）。⇒ Windows 下经 `cmd /c npx` 调。
fn npx_program() -> &'static str {
    if cfg!(windows) { "cmd" } else { "npx" }
}

/// `npx` 调用前缀（Windows 需要 `/c npx`）。
fn npx_prefix() -> Vec<String> {
    if cfg!(windows) {
        vec!["/c".to_string(), "npx".to_string()]
    } else {
        Vec::new()
    }
}

fn npx_available() -> bool {
    let mut cmd = Command::new(npx_program());
    cmd.args(npx_prefix());
    cmd.arg("--version");
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

pub fn run(a: &Args, verbose: bool) -> anyhow::Result<i32> {
    let mut steps = build_steps(a);

    // v27：`--subset rustc` = **只跑后端（forge-rustc）相关步骤**，给"改后端"这条工作流
    // 一条一两分钟能跑完的命令。用**显式白名单**而不是给 `Scope` 加变体：
    // ① 后端步骤横跨 fmt/守卫/e2e 三类，语义上不属于同一个 Scope；
    // ② 白名单本身就是这个子集的文档。名单外的步骤在这里被丢掉——**新增后端步骤时记得加进来**。
    if a.subset.as_deref() == Some("rustc") {
        const RUSTC_STEPS: &[&str] = &[
            "fmt",
            "rustc-e2e",
            "rustc-e2e-guards",
            "rustc-determinism",
            "rustc-opt-levels",
            "rustc-dwarf",
            "capability_gaps",
            // 后端 `Unsupported(` 的错误分类学（v27 S1–S4 曾整族违规而本子集看不见）
            "error_taxonomy",
            // 发行谱 lint（`InlineAsm` 归入"宿主直查"op 的缺口口径）
            "lint_shipped",
            "file_size_ratchet", // 管线文件的规模棘轮：v27 S1 曾漏跑它而把 lowering.rs 撑破上限
            "abi_target_real",
            "reg_name_table",
            "rustc_doc_counts",
            "workarounds_guard",
            "docs",
        ];
        steps.retain(|s| RUSTC_STEPS.contains(&s.name));
    }

    // 文档步（放在最后：它最可能 SKIP，且失败信息与代码无关）
    if !a.no_docs && !a.guards {
        match docs_step() {
            Some(s) => steps.push(s),
            None => eprintln!("[skip] 文档步：`git ls-files '*.md'` 拿不到文件清单"),
        }
    }

    if a.list {
        println!("子集 = {}，共 {} 步：", a.subset_name(), steps.len());
        for (i, s) in steps.iter().enumerate() {
            println!("{:>3}. {:<22} {} （{}）", i + 1, s.name, s.what, s.program);
        }
        return Ok(0);
    }

    let total = steps.len();
    let mut results: Vec<(&str, bool, f64)> = Vec::new();
    let mut failed: Vec<&str> = Vec::new();
    let mut docs_skipped = false;

    for (i, step) in steps.iter().enumerate() {
        if step.name == "docs" && !npx_available() {
            println!(
                "[{:>2}/{total}] {:<22} SKIP（npx 不可用）",
                i + 1,
                step.name
            );
            docs_skipped = true;
            continue;
        }
        println!(
            "\n===== [{:>2}/{total}] {} —— {} =====",
            i + 1,
            step.name,
            step.what
        );
        let started = Instant::now();
        let mut cmd = Command::new(step.program);
        cmd.args(&step.args);
        // 仓库纪律：不开增量缓存（会涨到几十 GB），测试不生成 debug info
        cmd.env("CARGO_INCREMENTAL", "0");
        if step.program == "cargo" && step.args.first().is_some_and(|a| a == "test") {
            cmd.env("CARGO_PROFILE_TEST_DEBUG", "0");
        }
        if step.name == "docs"
            && let Some(cache) = npm_cache_fallback()
        {
            cmd.env("npm_config_cache", cache);
        }
        let mut outcome = crate::env::run_child(&mut cmd, true)?;
        // `run_child(verbose=true)` 直通 stdio ⇒ 失败时没有捕获输出可打印
        if !outcome.ok {
            if outcome.stdout.is_empty() && outcome.stderr.is_empty() {
                println!(
                    "[FAIL] {} 退出码 = {:?}（输出见上方直通日志）",
                    step.name, outcome.code
                );
            } else {
                crate::env::print_fail_output(step.name, &outcome);
            }
            failed.push(step.name);
            outcome.ok = false;
        }
        let secs = started.elapsed().as_secs_f64();
        println!(
            "[{}] {} （{secs:.1}s）",
            if outcome.ok { "ok" } else { "FAIL" },
            step.name
        );
        results.push((step.name, outcome.ok, secs));
    }

    // ── 汇总 ──────────────────────────────────────────────────────────
    let _ = verbose;
    if a.json {
        let items: Vec<String> = results
            .iter()
            .map(|(n, ok, s)| format!("{{\"step\":\"{n}\",\"ok\":{ok},\"secs\":{s:.2}}}"))
            .collect();
        println!(
            "{{\"subset\":\"{}\",\"ok\":{},\"failed\":[{}],\"steps\":[{}]}}",
            a.subset_name(),
            failed.is_empty(),
            failed
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect::<Vec<_>>()
                .join(","),
            items.join(",")
        );
    } else {
        println!("\n===== 门禁汇总（子集 {}）=====", a.subset_name());
        for (n, ok, s) in &results {
            println!("  {:<22} {}  {s:.1}s", n, if *ok { "ok" } else { "FAIL" });
        }
        if docs_skipped {
            println!("  （文档步 SKIP：本机没有 npx）");
        }
        let sum: f64 = results.iter().map(|(_, _, s)| *s).sum();
        println!(
            "共 {} 步，失败 {} 步，总用时 {sum:.1}s",
            results.len(),
            failed.len()
        );
        if failed.is_empty() {
            println!("全部通过 ✔");
        } else {
            println!("失败：{}", failed.join(", "));
        }
    }
    Ok(if failed.is_empty() { 0 } else { 1 })
}
