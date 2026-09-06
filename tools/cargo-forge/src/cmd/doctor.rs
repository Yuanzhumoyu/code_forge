//! `doctor`：环境自检矩阵。逐项独立检查并打印全部项（不短路），
//! 全 PASS → exit 0；任一 FAIL → exit 1。每项附修复建议。
//!
//! 检查项（规格 a–e）：
//! (a) toolchain rustc 可用；      (b) rustc-dev / rust-src 组件（务实判据）；
//! (c) backend dll 存在（§env 定位）； (d) wrapper exe 存在； (e) host 目标提示。
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::env::Env;

struct Item {
    key: char,
    label: &'static str,
    ok: bool,
    detail: String,
    advice: Vec<String>,
}

pub fn run(env: &Env) -> anyhow::Result<i32> {
    println!("cargo-forge doctor —— 环境检查矩阵（toolchain {}）", env.toolchain);

    // (a) rustc 可用
    let version = probe_rustc_version(env);
    let (a_ok, a_detail) = match &version {
        Some(v) => (true, v.clone()),
        None => (
            false,
            "rustc 启动/版本查询失败（未安装 rustup+nightly，或不在 PATH）".to_string(),
        ),
    };
    let mut items = vec![Item {
        key: 'a',
        label: "toolchain rustc 可用",
        ok: a_ok,
        detail: a_detail,
        advice: vec![
            "安装: rustup toolchain install nightly --profile minimal --component rustc-dev,rust-src".to_string(),
            format!("或用 --toolchain 指定已装渠道（当前: {}）", env.toolchain_hint()),
        ],
    }];

    // (b) rustc-dev / rust-src 组件（务实判据：backend dll 已存在即视为
    // forge-rustc 可用——组件只影响 build-std 的 core/alloc 源码重编）
    let sysroot = probe_rustc_print(env, "--print", "sysroot").map(PathBuf::from);
    let host = probe_rustc_print(env, "--print", "host-tuple");
    let (b_ok, b_detail, b_advice) = check_components(env, sysroot.as_deref(), host.as_deref());
    items.push(Item {
        key: 'b',
        label: "rustc-dev / rust-src 组件",
        ok: b_ok,
        detail: b_detail,
        advice: b_advice,
    });

    // (c) backend dll
    let dll = env.backend_dll.clone();
    let (c_ok, c_detail, c_advice) = match &dll {
        None => (
            false,
            "未定位：仓库探测失败且未给 --backend-dll / FORGE_RUSTC_DLL".to_string(),
            vec![
                "仓库内运行本工具（自动 target\\debug\\forge_rustc.dll），或".to_string(),
                "--backend-dll <path> 指向已有 dll".to_string(),
            ],
        ),
        Some(p) if p.is_file() => (
            true,
            format!("{}（{} 字节）", p.display(), file_len(p).unwrap_or(0)),
            vec![],
        ),
        Some(p) => (
            false,
            format!("未找到: {}", p.display()),
            vec![
                "构建: 仓库根 `cargo build -p forge-rustc`，或 `cargo forge backend`".to_string(),
                "或用 --backend-dll 指向已有 dll".to_string(),
            ],
        ),
    };
    items.push(Item {
        key: 'c',
        label: "forge backend dll",
        ok: c_ok,
        detail: c_detail,
        advice: c_advice,
    });

    // (d) wrapper exe
    let wrapper = env.wrapper_exe.clone();
    let (d_ok, d_detail, d_advice) = match &wrapper {
        None => (
            false,
            "未定位（仓库未知，wrapper 路径依赖仓库 tools/forge-rustc-wrapper）".to_string(),
            vec!["在 code-forge 仓库内运行本工具".to_string()],
        ),
        Some(w) if w.is_file() => (true, w.display().to_string(), vec![]),
        Some(w) => (
            false,
            format!("未找到: {}", w.display()),
            vec![
                format!(
                    "构建: cargo build --manifest-path {}\\tools\\forge-rustc-wrapper\\Cargo.toml",
                    env.repo_root
                        .as_ref()
                        .map(|r| r.display().to_string())
                        .unwrap_or_else(|| "<repo>".to_string())
                ),
                "或直接运行 `cargo forge build`（wrapper 缺失会自动构建）".to_string(),
            ],
        ),
    };
    items.push(Item {
        key: 'd',
        label: "forge-rustc-wrapper exe",
        ok: d_ok,
        detail: d_detail,
        advice: d_advice,
    });

    // (e) host 目标提示
    let (e_ok, e_detail, e_advice) = match &host {
        Some(h) if h.contains("x86_64-pc-windows-msvc") => {
            (true, h.clone(), vec![])
        }
        Some(h) => (
            false,
            format!("{h}（forge-rustc 端到端仅验证于 x86_64-pc-windows-msvc）"),
            vec!["forge backend dll 是 PE-COFF；其他宿主请自行对照支持矩阵".to_string()],
        ),
        None => (
            false,
            "无法获取 host（rustc 不可用）".to_string(),
            vec![],
        ),
    };
    items.push(Item {
        key: 'e',
        label: "host 目标",
        ok: e_ok,
        detail: e_detail,
        advice: e_advice,
    });

    // 打印矩阵
    let mut any_fail = false;
    for it in &items {
        let status = if it.ok { "PASS" } else { "FAIL" };
        any_fail |= !it.ok;
        println!("  ({}) {:<24} {}  {}", it.key, it.label, status, it.detail);
        for a in &it.advice {
            println!("       修复: {a}");
        }
    }
    println!(
        "结果: {} {} → exit {}",
        items.iter().filter(|i| !i.ok).count(),
        if any_fail { "FAIL" } else { "全部 PASS" },
        if any_fail { 1 } else { 0 }
    );
    Ok(if any_fail { 1 } else { 0 })
}

fn probe_rustc_version(env: &Env) -> Option<String> {
    let mut cmd = Command::new("rustc");
    cmd.arg(&env.toolchain).arg("--version");
    env.apply_rustup(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().next().map(|l| l.trim().to_string())
}

fn probe_rustc_print(env: &Env, flag: &str, what: &str) -> Option<String> {
    let mut cmd = Command::new("rustc");
    cmd.arg(&env.toolchain).arg(flag).arg(what);
    env.apply_rustup(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn check_components(
    env: &Env,
    sysroot: Option<&Path>,
    host: Option<&str>,
) -> (bool, String, Vec<String>) {
    let dll_ok = env
        .backend_dll
        .as_ref()
        .map(|p| p.is_file())
        .unwrap_or(false);

    // sysroot 探测成功才谈得上"组件缺失"；探测失败视为无法判断
    let mut missing: Vec<String> = Vec::new();
    let sysroot_ok = sysroot.is_some();
    if let Some(sr) = sysroot {
        let rust_src_marker = sr
            .join("lib").join("rustlib").join("src").join("rust").join("library").join("core");
        let dev_lib = host.map(|h| sr.join("lib").join("rustlib").join(h).join("lib"));
        let rustc_dev_ok = dev_lib
            .map(|d| {
                std::fs::read_dir(&d)
                    .map(|rd| {
                        rd.filter_map(|e| e.ok()).any(|e| {
                            let name = e.file_name().to_string_lossy().into_owned();
                            name.starts_with("rustc_driver")
                        })
                    })
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if !rust_src_marker.is_dir() {
            missing.push("rust-src".to_string());
        }
        if !rustc_dev_ok {
            missing.push("rustc-dev".to_string());
        }
    }

    if sysroot_ok && missing.is_empty() {
        (
            true,
            format!("rust-src + rustc-dev 齐备（sysroot: {}）", sysroot.unwrap().display()),
            vec![],
        )
    } else if dll_ok {
        // 务实判据：backend dll 已存在 = forge-rustc 可用（规格 §doctor.b）；
        // 组件缺失只影响 cargo build-std 的 core/alloc 源码重编，仍给建议
        (
            true,
            format!(
                "务实判据: backend dll 已存在视为可用（rust-src/rustc-dev 探测: {}）",
                if missing.is_empty() {
                    "无法探测（rustc --print sysroot 失败）".to_string()
                } else {
                    format!("缺失 {}", missing.join(", "))
                }
            ),
            component_advice(env, &missing),
        )
    } else if missing.is_empty() {
        (
            false,
            "无法探测（rustc --print sysroot 失败）".to_string(),
            vec!["修复 (a) 项使 rustc 可用后重跑 doctor".to_string()],
        )
    } else {
        (
            false,
            format!(
                "缺失组件: {}（sysroot: {}）",
                missing.join(", "),
                sysroot.map(|s| s.display().to_string()).unwrap_or_default()
            ),
            component_advice(env, &missing),
        )
    }
}

fn component_advice(env: &Env, missing: &[String]) -> Vec<String> {
    if missing.is_empty() {
        return vec!["先构建 backend: `cargo build -p forge-rustc`（或 cargo forge backend）".to_string()];
    }
    vec![format!(
        "rustup component add {} --toolchain {}",
        missing.join(" "),
        env.toolchain_hint()
    )]
}

fn file_len(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok().map(|m| m.len())
}
