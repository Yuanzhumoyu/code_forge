//! cargo-forge：把 forge-rustc codegen backend 融进 rust 工具链的 cargo 子命令。
//!
//! cargo 的"外部子命令"约定：`cargo forge <args…>` → 查找并执行 `cargo-forge
//! <args…>`。因此本工具安装（`cargo install --path tools/cargo-forge` 或加入
//! PATH）后即可直接 `cargo forge build/run/doctor/backend/init`。
use clap::{Parser, Subcommand};

mod cmd;
mod env;

#[derive(Parser)]
#[command(
    name = "cargo-forge",
    version,
    about = "forge-rustc codegen backend 的 cargo 子命令",
    long_about = "把 code-forge 的 rustc codegen backend（forge_rustc.dll）融进 rust 工具链。\
                  子命令逐步加入：doctor → build/run → backend/init。"
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
    /// 环境自检矩阵（PASS/FAIL + 修复建议；全 PASS → exit 0，有 FAIL → exit 1）
    Doctor,
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
    }
}
