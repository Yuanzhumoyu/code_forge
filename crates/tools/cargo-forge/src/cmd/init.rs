//! `init`：为**当前目录的现有 cargo 工程**写集成配置，之后可裸 `cargo build`
//! （不经本工具）：`.cargo/config.toml`（rustflags 组 + rustc-wrapper +
//! [unstable] build-std）+ `rust-toolchain.toml`（channel = "nightly"）。
//!
//! 幂等覆盖：重跑生成相同文件；已有非本工具生成的 config 先备份为
//! `.cargo/config.toml.forge.bak`。dll/wrapper 缺失只警告/自动构建，仍照写
//! 绝对路径（路径在 .cargo/config 数组元素里，无 RUSTFLAGS 空格分词问题）。
use std::path::Path;

use anyhow::bail;

use crate::env::{self, Env, LINK_ARGS};

const GEN_MARKER: &str = "# 由 cargo-forge init 生成";

/// `init` 子命令参数（全限定 derive，避免与 struct 名同模块冲突）。
#[derive(clap::Args, Debug)]
pub struct Args {
    /// 启用 alloc：build-std=["core","alloc"] 且 rustflags 加 -Zshare-generics=yes
    #[arg(long)]
    pub alloc: bool,
}

pub fn run(args: &Args, env: &Env, verbose: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().map_err(|e| anyhow::anyhow!("当前目录读取失败: {e}"))?;
    if !cwd.join("Cargo.toml").is_file() {
        bail!(
            "init 需要现有 cargo 工程（当前目录 {} 无 Cargo.toml）。\n修复: 先 \
             `cargo init`/`cargo new`，再在工程目录运行 `cargo forge init`。",
            cwd.display()
        );
    }

    // backend dll：未定位则无法写绝对路径 → 报错；已定位但文件缺失 → 警告照写
    let dll = match &env.backend_dll {
        Some(p) => p.clone(),
        None => bail!(
            "未定位 forge backend dll（仓库探测失败）：init 需要写入 dll 绝对路径。\n\
             修复: 仓库内运行（自动 target\\debug\\forge_rustc.dll），或 \
             --backend-dll <path>。"
        ),
    };
    if !dll.is_file() {
        eprintln!(
            "提示: backend dll 当前不存在（{}）——config 已写入该路径；构建前先 \
             `cargo forge backend`（仓库内）或补齐 dll。",
            dll.display()
        );
    }

    // wrapper：缺失自动构建（仓库内）；构建仍缺（仓库未知）→ 报错
    env::ensure_wrapper(env, verbose)?;
    let wrapper = env.wrapper_exe.as_deref().ok_or_else(|| {
        anyhow::anyhow!("未定位 forge-rustc-wrapper（仓库未知）——仓库内运行本工具。")
    })?;

    // 写 .cargo/config.toml（先备份非本工具生成的旧文件）
    let cargo_dir = cwd.join(".cargo");
    std::fs::create_dir_all(&cargo_dir)
        .map_err(|e| anyhow::anyhow!("创建 {} 失败: {e}", cargo_dir.display()))?;
    let config_path = cargo_dir.join("config.toml");
    if let Ok(old) = std::fs::read_to_string(&config_path)
        && !old.contains(GEN_MARKER)
        && old.trim() != ""
    {
        let backup = backup_path(&config_path);
        std::fs::rename(&config_path, &backup)
            .map_err(|e| anyhow::anyhow!("备份 {} 失败: {e}", config_path.display()))?;
        println!("备份旧配置 → {}", backup.display());
    }
    let config_body = config_toml(&dll, wrapper, args.alloc);
    std::fs::write(&config_path, &config_body)
        .map_err(|e| anyhow::anyhow!("写入 {} 失败: {e}", config_path.display()))?;
    println!("写入 {}", config_path.display());

    // 写 rust-toolchain.toml（幂等覆盖）
    let toolchain_path = cwd.join("rust-toolchain.toml");
    let tc_body = "# 由 cargo-forge init 生成：forge backend 需 nightly rustc（见\n\
                   # crates/tools/forge-rustc README；版本与 backend 构建所用 nightly 对齐）。\n\
                   [toolchain]\nchannel = \"nightly\"\n";
    std::fs::write(&toolchain_path, tc_body)
        .map_err(|e| anyhow::anyhow!("写入 {} 失败: {e}", toolchain_path.display()))?;
    println!("写入 {}", toolchain_path.display());

    println!(
        "完成。之后直接 `cargo build` / `cargo run` 即走 forge 后端（config 自动生效，\
         系统 crate 经 wrapper 回退 LLVM）。"
    );
    Ok(())
}

/// .cargo/config.toml 内容（幂等：重跑生成一致文本）。
fn config_toml(dll: &Path, wrapper: &Path, alloc: bool) -> String {
    let esc = |p: &Path| p.display().to_string().replace('\\', "\\\\");
    let mut flags: Vec<String> = vec![
        format!("-Zcodegen-backend={}", esc(dll)),
        "-Cpanic=abort".to_string(),
        "-Coverflow-checks=off".to_string(),
    ];
    if alloc {
        flags.push("-Zshare-generics=yes".to_string());
    }
    flags.extend(LINK_ARGS.iter().map(|s| s.to_string()));

    let mut s = String::new();
    s.push_str(GEN_MARKER);
    s.push_str("（幂等覆盖；手工改动前请自行备份）。\n");
    s.push_str("# forge-rustc codegen backend 集成：rustflags 组 + rustc-wrapper + build-std。\n");
    s.push_str("# 之后 `cargo build`/`cargo run` 即走 forge 后端（core/alloc 等系统 crate\n");
    s.push_str("# 经 wrapper 剥离 -Zcodegen-backend 回退 LLVM）。\n");
    s.push_str("[build]\nrustflags = [\n");
    for f in &flags {
        s.push_str(&format!("    \"{f}\",\n"));
    }
    s.push_str("]\n");
    s.push_str(&format!("rustc-wrapper = \"{}\"\n\n", esc(wrapper)));
    s.push_str("[unstable]\n");
    s.push_str(if alloc {
        "build-std = [\"core\", \"alloc\"]\n"
    } else {
        "build-std = [\"core\"]\n"
    });
    s
}

/// 找不冲突的备份文件名：config.toml.forge.bak / .bak.1 / .bak.2 …
fn backup_path(p: &Path) -> std::path::PathBuf {
    let base = p.with_file_name(format!(
        "{}.forge.bak",
        p.file_name().unwrap().to_string_lossy()
    ));
    if !base.exists() {
        return base;
    }
    for i in 1.. {
        let cand = p.with_file_name(format!(
            "{}.forge.bak.{}",
            p.file_name().unwrap().to_string_lossy(),
            i
        ));
        if !cand.exists() {
            return cand;
        }
    }
    unreachable!()
}
