//! `backend`：构建 forge_rustc.dll（仓库根 `cargo build -p forge-rustc`）。
use std::path::PathBuf;
use std::process::Command;

use anyhow::bail;

use crate::env;

/// `backend` 子命令参数（全限定 derive，避免与 struct 名同模块冲突）。
#[derive(clap::Args, Debug)]
pub struct Args {
    /// release 构建（dll 在 target/release/forge_rustc.dll——默认探测只找
    /// target/debug，release 产物需 --backend-dll 显式指向）
    #[arg(long)]
    pub release: bool,

    /// code-forge 仓库根（工具不在仓库内时必填）
    #[arg(long, value_name = "REPO")]
    pub backend_src: Option<PathBuf>,
}

pub fn run(args: &Args, env: &env::Env, verbose: bool) -> anyhow::Result<i32> {
    // 仓库定位：--backend-src > 探测（CARGO_MANIFEST_DIR 上溯两级 / cwd 上溯）
    let repo = match &args.backend_src {
        Some(p) => {
            if !p.join("Cargo.toml").is_file() {
                bail!(
                    "--backend-src 不是 code-forge 仓库根（无 Cargo.toml）: {}",
                    p.display()
                );
            }
            p.clone()
        }
        None => match &env.repo_root {
            Some(r) => r.clone(),
            None => bail!(
                "非仓库环境：未能定位 code-forge 仓库。仓库内运行本工具，或 \
                 --backend-src <code-forge 仓库根>。"
            ),
        },
    };

    let profile = if args.release { "release" } else { "debug" };
    let mut cmd = Command::new("cargo");
    cmd.arg(&env.toolchain)
        .arg("build")
        .arg("-p")
        .arg("forge-rustc");
    if args.release {
        cmd.arg("--release");
    }
    cmd.current_dir(&repo);
    // rustup 重定向：当前进程未设 RUSTUP_HOME 且仓库 target/rustup_home 存在时注入
    //（backend 与本仓库工具链一致性关键，见 env::Env::resolve 注释）
    env.apply_rustup(&mut cmd);
    if std::env::var_os("RUSTUP_HOME").is_none() {
        let redirect = repo.join("target").join("rustup_home");
        if redirect.is_dir() {
            cmd.env("RUSTUP_HOME", &redirect);
        }
    }

    if verbose {
        eprintln!(
            "backend 构建: cargo {} build -p forge-rustc{}（cwd={}）",
            env.toolchain,
            if args.release { " --release" } else { "" },
            repo.display()
        );
    }
    let out = env::run_child(&mut cmd, verbose)?;
    if !out.ok {
        env::print_fail_output("cargo(forge-rustc)", &out);
        bail!("forge backend 构建失败（cargo exit {:?}）", out.code);
    }

    let dll = repo.join("target").join(profile).join("forge_rustc.dll");
    if dll.is_file() {
        println!("backend 构建完成: {}", dll.display());
        if args.release {
            println!(
                "提示: 默认探测只找 target\\debug\\forge_rustc.dll——release 产物请用 \
                 --backend-dll 指向: {}",
                dll.display()
            );
        }
    } else {
        println!(
            "backend 构建成功，但未在预期位置找到 dll: {}",
            dll.display()
        );
    }
    Ok(0)
}
