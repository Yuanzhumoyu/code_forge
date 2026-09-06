//! `run`：先执行 build 逻辑，成功后运行产物并透传退出码。
//!
//! - cargo 模式：`<workspace target>/<debug|release>/<包名>.exe`（包名 = 当前
//!   Cargo.toml package.name；exe 文件名按 cargo 现状保留连字符，另兼容
//!   '-'→'_' 变体）；
//! - `--file` 模式：`<file stem>.exe`。
//! - `--` 后的尾参全给 exe（run 不透传 cargo 参数）。
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::bail;

use crate::cmd::build::{self, BuildArgs};
use crate::env::{self, Env};

pub fn execute(args: &BuildArgs, env: &Env, verbose: bool) -> anyhow::Result<i32> {
    let built = build::execute(args, env, verbose)?;
    let exe = match built {
        Some(exe) => exe,
        None => resolve_cargo_exe(args.release, verbose).ok_or_else(|| {
            anyhow::anyhow!(
                "构建成功但未能定位可执行文件（工程无 bin target？）。期望: \
                 <target>\\{}\\<包名>.exe",
                if args.release { "release" } else { "debug" }
            )
        })?,
    };
    if !exe.is_file() {
        bail!(
            "可执行文件不存在: {}（编译产物被清理或包名不符？）",
            exe.display()
        );
    }
    if verbose {
        eprintln!("运行: {} {}", exe.display(), args.trailing.join(" "));
    }
    let st = Command::new(&exe)
        .args(&args.trailing)
        .status()
        .map_err(|e| env::spawn_err_hint(&exe.display().to_string(), &e))?;
    // 入口函数返回值 = 进程退出码（Windows x64 no_std 标准入口形态）
    Ok(st.code().unwrap_or(1))
}

/// cargo 模式产物 exe 定位：
/// 1. 简单解析当前 Cargo.toml 的 `[package] name = "…"`（规格做法）；
/// 2. workspace target 根 = 最近的、含 `[workspace]` 的 manifest 所在目录
///    （自身 Cargo.toml 含 [workspace] → 自身；ancestor 链上第一个含
///    `[workspace]` 的 → 那里；都没有 → 当前目录即隐式根）；
/// 3. exe 文件名 = 包名原样（cargo 现状保留 '-'），并兼容 '-'→'_' 变体，
///    取存在者。
fn resolve_cargo_exe(release: bool, verbose: bool) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let toml_text = std::fs::read_to_string(cwd.join("Cargo.toml")).ok()?;
    let pkg = parse_package_name(&toml_text)?;
    let target_root = find_target_root(&cwd)?;
    let profile_dir = target_root
        .join("target")
        .join(if release { "release" } else { "debug" });

    // 候选：原名（cargo 对 bin 名保留 '-'，实测）→ 下划线变体（规格描述）
    let mut candidates = vec![
        format!("{pkg}.exe"),
        format!("{}.exe", pkg.replace('-', "_")),
    ];
    if verbose {
        eprintln!(
            "exe 候选: {}（root={}）",
            candidates.join(", "),
            profile_dir.display()
        );
    }
    candidates.dedup();
    candidates
        .into_iter()
        .find(|c| profile_dir.join(c).is_file())
        .map(|c| profile_dir.join(c))
}

/// 从 Cargo.toml 文本解析 `[package]` 段内 `name = "…"`。
fn parse_package_name(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_package = t == "[package]";
            continue;
        }
        if in_package && let Some(rest) = t.strip_prefix("name") {
            let rest = rest.trim_start();
            if let Some(v) = rest.strip_prefix('=')
                && let Some(n) = v.trim().strip_prefix('"')
                && let Some(end) = n.find('"')
            {
                return Some(n[..end].to_string());
            }
        }
    }
    None
}

/// workspace target 根：自身/祖先 manifest 中第一个含 `[workspace]` 的所在目录。
fn find_target_root(cwd: &Path) -> Option<PathBuf> {
    let mut dir = Some(cwd.to_path_buf());
    for _ in 0..8 {
        let d = dir?;
        let manifest = d.join("Cargo.toml");
        if let Ok(text) = std::fs::read_to_string(&manifest) {
            let has_workspace = text.lines().any(|l| {
                let t = l.trim();
                t == "[workspace]" || t.starts_with("[workspace.")
            });
            if has_workspace {
                return Some(d);
            }
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    Some(cwd.to_path_buf())
}
