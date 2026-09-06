//! `build`：用 forge 后端构建。
//!
//! 两种模式（目标判定）：
//! - `--file <single.rs>`：裸 rustc 单文件模式（README「方式二」标准入口形态），
//!   免 build-std——sysroot 直接提供 core/alloc rlib；
//! - 否则当前目录必须有 Cargo.toml：cargo 模式
//!   `cargo +nightly -Zbuild-std=core[,alloc] build` + RUSTFLAGS/RUSTC_WRAPPER。
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::bail;

use crate::env::{self, Env, FlagSpec};

/// build / run 共享的参数（clap derive：`#[derive(clap::Args)]` 全限定，
/// 避免与 struct 名 `Args` 同模块冲突）。
#[derive(clap::Args, Debug, Clone, Default)]
pub struct BuildArgs {
    /// release 构建（默认 debug）
    #[arg(long)]
    pub release: bool,

    /// 启用 alloc（cargo 模式 build-std=core,alloc + -Zshare-generics=yes；
    /// 单文件模式 sysroot alloc rlib 直链，无需额外参数）
    #[arg(long)]
    pub alloc: bool,

    /// debuginfo 等级（-Cdebuginfo=1|2）
    #[arg(long, value_name = "1|2", value_parser = clap::value_parser!(u8).range(1..=2))]
    pub debuginfo: Option<u8>,

    /// codegen units（-Ccodegen-units=N；rustc 侧 CGU 划分，配合 forge 并行）
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub codegen_units: Option<u32>,

    /// rustc 前端并行线程（-Zthreads=N；forge M4 并行池需 -Zthreads>=2 +
    /// FORGE_CODEGEN_THREADS>1 才真正上池）
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub threads: Option<u32>,

    /// 保留溢出检查（缺省注入 -Coverflow-checks=off）
    #[arg(long)]
    pub overflow_on: bool,

    /// 单文件模式：裸 rustc 编译该 .rs（产物 = 同名 .exe）
    #[arg(long, value_name = "FILE")]
    pub file: Option<PathBuf>,

    /// cargo 模式透传给 cargo 的尾参（-- 之后；run 的 -- 是给 exe 的）
    #[arg(last = true, value_name = "CARGO_ARGS")]
    pub trailing: Vec<String>,
}

/// 构建并（能确定的）产物 exe 路径。返回：
/// - `--file` 模式：Some(<同名 .exe>)；
/// - cargo 模式：None（exe 定位由 run/元数据负责——见 `run::resolve_exe`）。
pub fn execute(args: &BuildArgs, env: &Env, verbose: bool) -> anyhow::Result<Option<PathBuf>> {
    // backend dll 前置检查（两种模式都需要）
    let dll = match &env.backend_dll {
        Some(p) => p.clone(),
        None => bail!(
            "未定位 forge backend dll：仓库探测失败且未给 --backend-dll / \
             FORGE_RUSTC_DLL。仓库内运行（自动 target\\debug\\forge_rustc.dll），或 \
             --backend-dll <path>。"
        ),
    };
    if !dll.is_file() {
        bail!(
            "backend dll 不存在: {}\n修复: 仓库根 `cargo build -p forge-rustc`（或 \
             `cargo forge backend`）；或用 --backend-dll 指向已有 dll。",
            dll.display()
        );
    }

    match &args.file {
        Some(f) => file_mode(args, env, verbose, &dll, f),
        None => cargo_mode(args, env, verbose, &dll),
    }
}

fn spec<'a>(args: &'a BuildArgs, dll: &'a Path) -> FlagSpec<'a> {
    FlagSpec {
        backend_dll: dll,
        alloc: args.alloc,
        overflow_on: args.overflow_on,
        debuginfo: args.debuginfo,
        codegen_units: args.codegen_units,
        threads: args.threads,
    }
}

/// 单文件裸 rustc 模式（README 方式二标准入口形态 + 可选 -C/-Z 扩展参数）。
fn file_mode(args: &BuildArgs,
    env: &Env,
    verbose: bool,
    dll: &Path,
    file: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    if !file.is_file() {
        bail!("--file 不存在: {}", file.display());
    }
    if !args.trailing.is_empty() {
        // -- 尾参是给 cargo 的；单文件模式无 cargo
        if verbose {
            eprintln!("提示: --file 单文件模式忽略 -- 尾参（那是 cargo 模式透传给 cargo 的）");
        }
    }
    let out = file.with_extension("exe");
    let flags = env::rustflags_list(&spec(args, dll));

    let mut cmd = Command::new("rustc");
    cmd.arg(&env.toolchain).args(&flags);
    cmd.arg("--edition").arg("2024");
    cmd.arg(file).arg("-o").arg(&out);
    env.apply_rustup(&mut cmd);
    if verbose {
        eprintln!(
            "单文件编译: rustc {} {} … {} -o {}",
            env.toolchain,
            flags.join(" "),
            file.display(),
            out.display()
        );
    }
    let r = env::run_child(&mut cmd, verbose)?;
    if !r.ok {
        env::print_fail_output("rustc(forge)", &r);
        bail!("forge 单文件编译失败（rustc exit {:?}）", r.code);
    }
    Ok(Some(out))
}

/// cargo 工程模式：RUSTFLAGS + RUSTC_WRAPPER + `cargo +tc -Zbuild-std=core[,alloc] build`。
fn cargo_mode(args: &BuildArgs,
    env: &Env,
    verbose: bool,
    dll: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let cwd = std::env::current_dir().map_err(|e| anyhow::anyhow!("当前目录读取失败: {e}"))?;
    if !cwd.join("Cargo.toml").is_file() {
        bail!(
            "当前目录非 cargo 工程（无 Cargo.toml）: {}\n\
             修复: 进入含 Cargo.toml 的目录后重试；对单个源文件用 \
             `cargo-forge build --file <x.rs>`（免 build-std）；或先 `cargo init`。",
            cwd.display()
        );
    }

    env::ensure_wrapper(env, verbose)?;
    let wrapper = env.wrapper_exe.as_deref().ok_or_else(|| {
        anyhow::anyhow!("未定位 forge-rustc-wrapper（仓库未知）——仓库内运行本工具。")
    })?;
    env::warn_dll_path_space(dll);

    let flags = env::rustflags_string(&spec(args, dll));
    if verbose {
        eprintln!(
            "cargo 模式 env 摘要:\n  RUSTFLAGS={flags}\n  RUSTC_WRAPPER={}\n  toolchain={}",
            wrapper.display(),
            env.toolchain
        );
    }

    let build_std = if args.alloc { "core,alloc" } else { "core" };
    let mut cmd = Command::new("cargo");
    cmd.arg(&env.toolchain)
        .arg(format!("-Zbuild-std={build_std}"))
        .arg("build");
    if args.release {
        cmd.arg("--release");
    }
    if !args.trailing.is_empty() {
        cmd.arg("--").args(&args.trailing);
    }
    cmd.env("RUSTFLAGS", &flags).env("RUSTC_WRAPPER", wrapper);
    env.apply_rustup(&mut cmd);

    if verbose {
        eprintln!(
            "执行: cargo {} -Zbuild-std={build_std} build{}{}",
            env.toolchain,
            if args.release { " --release" } else { "" },
            if args.trailing.is_empty() {
                String::new()
            } else {
                format!(" -- {}", args.trailing.join(" "))
            }
        );
    }
    let r = env::run_child(&mut cmd, verbose)?;
    if !r.ok {
        env::print_fail_output("cargo(forge)", &r);
        bail!(
            "forge cargo 构建失败（cargo exit {:?}）。\n提示: 单个源文件可改用 \
             `--file <x.rs>`（免 build-std）",
            r.code
        );
    }
    if !verbose {
        eprintln!(
            "构建成功（profile={}，backend {}）",
            if args.release { "release" } else { "debug" },
            dll.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        );
    }
    Ok(None)
}

