//! cargo-forge：把 forge-rustc codegen backend 融进 rust 工具链的 cargo 子命令。
//!
//! cargo 的"外部子命令"约定：`cargo forge <args…>` → 查找并执行 `cargo-forge
//! <args…>`。因此本工具安装（`cargo install --path tools/cargo-forge` 或加入
//! PATH）后即可直接 `cargo forge build/run/doctor/backend/init`。
//!
//! 子命令一览：
//! - `backend`：构建 forge_rustc.dll（`cargo build -p forge-rustc`）
//! - `doctor`：环境自检矩阵（PASS/FAIL + 修复建议），全 PASS 退出 0
//! - `build`：用 forge 后端构建（cargo 工程 build-std 模式，或 `--file` 裸 rustc 模式）
//! - `run`：build 成功后运行产物（`--` 后参数透传 exe）
//! - `init`：为当前 cargo 工程写 `.cargo/config.toml` + `rust-toolchain.toml`，
//!   之后可裸 `cargo build`（不经本工具）
use clap::{Parser, Subcommand};

mod cmd;
mod env;

#[derive(Parser)]
#[command(
    name = "cargo-forge",
    version,
    about = "forge-rustc codegen backend 的 cargo 子命令（build/run/init/doctor/backend）",
    long_about = "把 code-forge 的 rustc codegen backend（forge_rustc.dll）融进 rust 工具链：\n\
        cargo forge build / run —— 用 forge 后端编译/运行 no_std 工程（-Zbuild-std + rustc wrapper）\n\
        cargo forge init —— 写 .cargo/config.toml + rust-toolchain.toml（之后可裸 cargo build）\n\
        cargo forge doctor / backend —— 环境自检 / 构建 backend dll\n\
        详见 crates/tools/forge-rustc/README.md「推荐：cargo forge」"
)]
struct Cli {
    /// forge_rustc.dll 路径（缺省：env FORGE_RUSTC_DLL → 仓库 target/debug）
    #[arg(long, global = true, value_name = "PATH", help_heading = "全局参数")]
    backend_dll: Option<std::path::PathBuf>,

    /// 工具链（rustup 语法，原样作为 rustc/cargo 首参）
    #[arg(long, global = true, default_value = "+nightly", value_name = "CHANNEL", help_heading = "全局参数")]
    toolchain: String,

    /// 打印诊断与完整子进程输出（失败时也打印 stderr 尾部）
    #[arg(short, long, global = true, help_heading = "全局参数")]
    verbose: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 构建 forge backend dll（仓库根 cargo build -p forge-rustc）
    Backend(cmd::backend::Args),
    /// 环境自检矩阵（PASS/FAIL + 修复建议；全 PASS → exit 0，有 FAIL → exit 1）
    Doctor,
    /// 用 forge 后端构建（当前 cargo 工程，或 --file 单文件裸 rustc 模式）
    Build(cmd::build::BuildArgs),
    /// 构建并运行（-- 后的参数透传 exe；run 不透传 cargo 参数）
    Run(cmd::build::BuildArgs),
    /// 为当前 cargo 工程写 .cargo/config.toml + rust-toolchain.toml（幂等覆盖）
    Init(cmd::init::Args),
}

fn main() {
    let cli = Cli::parse();
    let code = match dispatch(&cli) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("错误: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn dispatch(cli: &Cli) -> anyhow::Result<i32> {
    let env = env::Env::resolve(cli.backend_dll.as_deref(), cli.toolchain.clone());
    match &cli.cmd {
        Cmd::Doctor => cmd::doctor::run(&env),
        Cmd::Backend(a) => cmd::backend::run(a, &env, cli.verbose),
        Cmd::Build(a) => {
            cmd::build::execute(a, &env, cli.verbose)?;
            Ok(0)
        }
        Cmd::Run(a) => cmd::run::execute(a, &env, cli.verbose),
        Cmd::Init(a) => {
            cmd::init::run(a, &env, cli.verbose)?;
            Ok(0)
        }
    }
}
