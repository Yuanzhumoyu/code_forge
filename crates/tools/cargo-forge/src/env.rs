//! 环境定位与子进程/flag 组装：repo / backend dll / wrapper / rustup 重定向 /
//! RUSTFLAGS 配方——cargo-forge 的"位置与配方"层。
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::bail;

/// forge 链接参数。RUSTFLAGS 里是三个**独立空格分隔项**；`.cargo/config.toml`
/// 里是三个独立数组元素（init 用，无空格分词问题）。
pub const LINK_ARGS: [&str; 3] = [
    "-Clink-arg=/SUBSYSTEM:CONSOLE",
    "-Clink-arg=/DEFAULTLIB:kernel32.lib",
    "-Clink-arg=/DEFAULTLIB:vcruntime.lib",
];

/// 仓库根判据：该目录含 Cargo.toml 且含 `tools/forge-rustc-wrapper/Cargo.toml`
/// （code-forge 仓库布局特征）。
fn is_repo_root(d: &Path) -> bool {
    d.join("Cargo.toml").is_file()
        && d.join("tools")
            .join("forge-rustc-wrapper")
            .join("Cargo.toml")
            .is_file()
}

/// 仓库根探测。优先编译期 `CARGO_MANIFEST_DIR`（构建时所在仓库）：从该目录
/// 向上逐级找仓库根——cargo-forge 曾位于 `tools/`（2 级）、现位于
/// `crates/tools/`（3 级），不固定层数；编译期路径失效（仓库移动/删除/在另一
/// 副本安装）时回退：当前工作目录向上找。都失败 → None（调用方报错提示
/// --backend-dll / --backend-src）。
///
/// 注意：不做 canonicalize——Windows 下会得到 `\\?\` 扩展前缀，污染打印与
/// RUSTFLAGS；parent()/祖先链本就给出干净绝对路径。
pub fn detect_repo() -> Option<PathBuf> {
    // 编译期锚点（构建时仓库路径，exe 装到别处后仍指向仓库）
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut dir = Some(manifest.to_path_buf());
    for _ in 0..6 {
        let d = dir?;
        if is_repo_root(&d) {
            return Some(d);
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    // 运行时回退：cwd 上溯（仓库移动/删除后于新位置运行）
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..10 {
        if is_repo_root(&dir) {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

/// 解析后的运行环境。
///
/// backend dll 定位优先级（规格 §env）：`--backend-dll` > env `FORGE_RUSTC_DLL`，
/// 再到仓库的 `target/debug/forge_rustc.dll`。wrapper 固定为仓库内的
/// 相对路径 `tools/forge-rustc-wrapper/target/debug/forge-rustc-wrapper.exe`。
pub struct Env {
    pub repo_root: Option<PathBuf>,
    /// 已解析的 backend dll（可能不存在——存在性由各子命令自检/报错）
    pub backend_dll: Option<PathBuf>,
    pub wrapper_exe: Option<PathBuf>,
    /// rustup 工具链参数（默认 "+nightly"；原样作为 rustc/cargo 首参）
    pub toolchain: String,
    /// 当前进程未设 RUSTUP_HOME 且 `<repo>/target/rustup_home` 存在时，注入该值
    /// （本仓库后端 dll 用该重定向 rustup 的浮点 nightly 构建，见 README 验证路径）
    pub rustup_home_redirect: Option<PathBuf>,
}

impl Env {
    pub fn resolve(cli_backend_dll: Option<&Path>, toolchain: String) -> Env {
        let repo_root = detect_repo();
        let backend_dll = match cli_backend_dll {
            Some(p) => Some(p.to_path_buf()),
            None => std::env::var_os("FORGE_RUSTC_DLL")
                .map(PathBuf::from)
                .or_else(|| {
                    repo_root
                        .as_ref()
                        .map(|r| r.join("target").join("debug").join("forge_rustc.dll"))
                }),
        };
        let wrapper_exe = repo_root.as_ref().map(|r| {
            r.join("tools")
                .join("forge-rustc-wrapper")
                .join("target")
                .join("debug")
                .join("forge-rustc-wrapper.exe")
        });
        let rustup_home_redirect = if std::env::var_os("RUSTUP_HOME").is_none() {
            repo_root
                .as_ref()
                .map(|r| r.join("target").join("rustup_home"))
                .filter(|p| p.is_dir())
        } else {
            None
        };
        Env {
            repo_root,
            backend_dll,
            wrapper_exe,
            toolchain,
            rustup_home_redirect,
        }
    }

    /// 把 RUSTUP_HOME 重定向（如需）应用到子进程。
    pub fn apply_rustup(&self, cmd: &mut Command) {
        if let Some(h) = &self.rustup_home_redirect {
            cmd.env("RUSTUP_HOME", h);
        }
    }

    /// 渠道名去 '+'（用于 rustup component add 等建议文案）。
    pub fn toolchain_hint(&self) -> &str {
        self.toolchain.trim_start_matches('+')
    }
}

/// rustc/cargo 的 forge 参数配方（RUSTFLAGS 组装 / 单文件 rustc 参数共用）。
pub struct FlagSpec<'a> {
    pub backend_dll: &'a Path,
    /// alloc 模式：-Zshare-generics=yes（配合 cargo 模式 build-std=core,alloc）
    pub alloc: bool,
    /// 保留溢出检查（缺省注入 -Coverflow-checks=off）
    pub overflow_on: bool,
    /// -Cdebuginfo=1|2
    pub debuginfo: Option<u8>,
    /// -Ccodegen-units=N
    pub codegen_units: Option<u32>,
    /// -Zthreads=N（rustc 前端并行池）
    pub threads: Option<u32>,
}

/// 逐项参数（每个元素 = rustc 一个独立参数）。
pub fn rustflags_list(s: &FlagSpec) -> Vec<String> {
    let mut v = vec![
        format!("-Zcodegen-backend={}", s.backend_dll.display()),
        "-Cpanic=abort".to_string(),
    ];
    if !s.overflow_on {
        v.push("-Coverflow-checks=off".to_string());
    }
    if let Some(n) = s.debuginfo {
        v.push(format!("-Cdebuginfo={n}"));
    }
    if let Some(n) = s.codegen_units {
        v.push(format!("-Ccodegen-units={n}"));
    }
    if let Some(n) = s.threads {
        v.push(format!("-Zthreads={n}"));
    }
    if s.alloc {
        v.push("-Zshare-generics=yes".to_string());
    }
    v.extend(LINK_ARGS.iter().map(|s| s.to_string()));
    v
}

/// RUSTFLAGS 整体字符串（cargo 按空格分词——**dll 路径含空格会解析错误**，
/// 由调用方在 cargo 模式告警；.cargo/config 数组元素模式无此限制）。
pub fn rustflags_string(s: &FlagSpec) -> String {
    rustflags_list(s).join(" ")
}

/// dll 路径含空格时打警告（仅 cargo 模式需要——RUSTFLAGS 按空格分词）。
pub fn warn_dll_path_space(dll: &Path) {
    if dll.to_string_lossy().contains(' ') {
        eprintln!(
            "警告: backend dll 路径含空格（cargo 模式的 RUSTFLAGS 按空格分词会解析错误）:\n  {}\n\
             建议: 将 dll 置于无空格路径，或改用 `cargo forge init`（.cargo/config 的\
             rustflags 是数组元素，无此限制）。",
            dll.display()
        );
    }
}

/// 子进程运行结果。
pub struct RunOutcome {
    pub code: Option<i32>,
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// 运行子进程：verbose 时继承 stdio 直通；否则捕获输出（失败时打尾部）。
pub fn run_child(cmd: &mut Command, verbose: bool) -> anyhow::Result<RunOutcome> {
    if verbose {
        let st = cmd
            .status()
            .map_err(|e| anyhow::anyhow!("spawn 失败: {e}"))?;
        return Ok(RunOutcome {
            code: st.code(),
            ok: st.success(),
            stdout: String::new(),
            stderr: String::new(),
        });
    }
    let o = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("spawn 失败: {e}"))?;
    Ok(RunOutcome {
        code: o.status.code(),
        ok: o.status.success(),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    })
}

/// 打印输出尾部（非空行截断，默认 40 行）。
pub fn print_tail(what: &str, text: &str, max_lines: usize) {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let n = lines.len();
    let start = n.saturating_sub(max_lines);
    eprintln!("--- {what} 输出尾部（非空 {n} 行）---");
    for l in &lines[start..] {
        eprintln!("{l}");
    }
}

/// 失败输出：优先 stderr 尾部（rustc/cargo 诊断都在 stderr）；stderr 为空时
/// 退化到 stdout 尾部。
pub fn print_fail_output(what: &str, out: &RunOutcome) {
    if out.stderr.trim().is_empty() {
        print_tail(what, &out.stdout, 40);
    } else {
        print_tail(what, &out.stderr, 40);
    }
}

/// wrapper exe 缺失时自动构建（`cargo build --manifest-path …/Cargo.toml`）。
pub fn ensure_wrapper(env: &Env, verbose: bool) -> anyhow::Result<()> {
    if let Some(w) = &env.wrapper_exe
        && w.is_file()
    {
        return Ok(());
    }
    let repo = env.repo_root.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "未找到 forge-rustc-wrapper 工程（仓库未知）。wrapper 由仓库内构建：\
             请在 code-forge 仓库内运行本工具，或先手动构建 wrapper。"
        )
    })?;
    let manifest = repo
        .join("tools")
        .join("forge-rustc-wrapper")
        .join("Cargo.toml");
    eprintln!(
        "forge-rustc-wrapper 缺失，自动构建（{}）…",
        manifest.display()
    );
    let mut cmd = Command::new("cargo");
    cmd.arg(&env.toolchain)
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .current_dir(repo);
    env.apply_rustup(&mut cmd);
    let out = run_child(&mut cmd, verbose)?;
    if !out.ok {
        print_fail_output("cargo(wrapper 构建)", &out);
        bail!("forge-rustc-wrapper 构建失败（exit {:?}）", out.code);
    }
    Ok(())
}

/// rustc 在 PATH 缺失时的报错提示（spawn 失败统一走这里）。
pub fn spawn_err_hint(program: &str, e: &std::io::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "无法启动 {program}: {e}\n\
         修复: 安装 rustup + nightly（rustup toolchain install nightly \
         --profile minimal --component rustc-dev,rust-src），并把 rustup 的 \
         bin 目录加入 PATH。"
    )
}
