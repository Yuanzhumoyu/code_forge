//! `-C opt-level` **全档**回归（v27 M5 补面：e2e 只在 debug 档跑过）。
//!
//! 为什么单独一个文件：e2e 夹具的用例结构里没有"rustc 旗标"这一维（`extra` 是**源码级**
//! 附加），而"后端在 `-O` 各档也正确"是"完整后端"的硬承诺——这里用**同一个程序**钉住它：
//! 每个档位编译一次、链接、真执行，断言退出码。
//!
//! 程序本身刻意混了几条最容易在优化档出事的路：循环、递归、乘法、窄化与溢出检查。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 目标程序：`fib(10)=55` + `sum(4)=30` + `45` − `88` = **42**。
///
/// 刻意混了循环（`while`）、递归（`fib`）、乘法（`i * i`）、有符号窄化（`as i32`）
/// 与溢出检查（`wrapping_add` 之外还有常规 `+`）。
const SRC: &str = r#"
#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

fn fib(n: u64) -> u64 {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn sum_squares(n: u64) -> u64 {
    let mut s = 0u64;
    let mut i = 1u64;
    while i <= n {
        s += i * i;
        i += 1;
    }
    s
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> i32 {
    let v = fib(10) + sum_squares(4) + 45 - 88;
    v as i32
}
"#;

/// 后端 dylib（与 e2e 同一个产物）。
fn backend_dll() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace 根可解析");
    root.join("target").join("debug").join("forge_rustc.dll")
}

/// 编译 + 链接 + 真执行一个源码，返回进程退出码（**任意额外旗标**，`-C opt-level` 只是其中一种）。
fn run_with(src: &Path, dll: &Path, extra: &[&str], out: &Path) -> Result<i32, String> {
    let status = Command::new("rustc")
        .arg(format!("-Zcodegen-backend={}", dll.display()))
        .args(["-Z", "threads=1"])
        .args(["-C", "panic=abort"])
        .args(["-C", "overflow-checks=on"])
        .args(extra)
        .args(["--edition", "2024"])
        .args(["--crate-type", "bin"])
        .arg(src)
        .arg("-o")
        .arg(out)
        // 无 std：自定入口 + 控制台子系统（与 e2e 同口径）。
        .args([
            "-C",
            "link-args=/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib",
        ])
        .output()
        .map_err(|e| format!("启动 rustc 失败：{e}"))?;
    if !status.status.success() {
        return Err(format!(
            "编译失败（旗标 {extra:?}）：\n{}",
            String::from_utf8_lossy(&status.stderr)
        ));
    }
    let mut child = Command::new(out)
        .spawn()
        .map_err(|e| format!("启动产物失败：{e}"))?;
    let status = child.wait().map_err(|e| format!("等待产物失败：{e}"))?;
    Ok(status.code().unwrap_or(-1))
}

/// `-C opt-level=<level>` 的特例（薄包装）。
fn run_at_level(src: &Path, dll: &Path, level: &str, out: &Path) -> Result<i32, String> {
    run_with(src, dll, &["-C", &format!("opt-level={level}")], out)
}

#[test]
fn all_opt_levels_produce_the_same_answer() {
    let dll = backend_dll();
    assert!(
        dll.is_file(),
        "找不到后端 dylib：{}（先 `cargo build -p forge-rustc`）",
        dll.display()
    );

    let dir = std::env::temp_dir().join("forge_rustc_opt_levels");
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let src = dir.join("prog.rs");
    std::fs::write(&src, SRC).expect("写源码");

    // `-C opt-level` 的六个合法档位；`s`/`z` 是体积档（走另一条优化路径）。
    let mut failures: Vec<String> = Vec::new();
    for level in ["0", "1", "2", "3", "s", "z"] {
        let out = dir.join(format!("prog_{level}.exe"));
        match run_at_level(&src, &dll, level, &out) {
            Ok(42) => {}
            Ok(code) => failures.push(format!("opt-level={level} ⇒ 退出码 {code}（期望 42）")),
            Err(e) => failures.push(format!("opt-level={level} ⇒ {e}")),
        }
    }
    assert!(
        failures.is_empty(),
        "各优化档必须给出同一答案（42）：\n{}",
        failures.join("\n")
    );
}

/// 只编译成**对象文件**（不链接）：确定性与 `.exe` 无关——`.exe` 里有链接器时间戳/PDB 路径，
/// 拿它比逐字节相等**测的是链接器**，不是后端。
fn emit_obj(src: &Path, dll: &Path, level: &str, outdir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(outdir).map_err(|e| format!("建输出目录失败：{e}"))?;
    let status = Command::new("rustc")
        .arg(format!("-Zcodegen-backend={}", dll.display()))
        .args(["-Z", "threads=1"])
        .args(["-C", "panic=abort"])
        .args(["-C", "overflow-checks=on"])
        .args(["-C", &format!("opt-level={level}")])
        .args(["--edition", "2024"])
        .args(["--emit=obj", "--out-dir"])
        .arg(outdir)
        .arg(src)
        .output()
        .map_err(|e| format!("启动 rustc 失败：{e}"))?;
    if !status.status.success() {
        return Err(format!(
            "编译失败（opt-level={level}）：\n{}",
            String::from_utf8_lossy(&status.stderr)
        ));
    }
    // 后端固定产出 `forge_codegen_output.o`（rustc 不改名）⇒ 按 `--out-dir` 定位。
    let obj = outdir.join("forge_codegen_output.o");
    if !obj.is_file() {
        return Err(format!("编译成功但没找到对象文件：{}", obj.display()));
    }
    Ok(obj)
}

/// 确定性：**同一档位编两次**必须产物逐字节相等（优化档也要稳）。
#[test]
fn same_level_is_bytewise_deterministic() {
    let dll = backend_dll();
    assert!(dll.is_file(), "找不到后端 dylib：{}", dll.display());
    let dir = std::env::temp_dir().join("forge_rustc_opt_levels_det");
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let src = dir.join("prog.rs");
    std::fs::write(&src, SRC).expect("写源码");

    for level in ["0", "2"] {
        let a = emit_obj(&src, &dll, level, &dir.join(format!("outA_{level}")))
            .unwrap_or_else(|e| panic!("opt-level={level} 首次：{e}"));
        let b = emit_obj(&src, &dll, level, &dir.join(format!("outB_{level}")))
            .unwrap_or_else(|e| panic!("opt-level={level} 第二次：{e}"));
        let (ba, bb) = (
            std::fs::read(&a).expect("读产物 a"),
            std::fs::read(&b).expect("读产物 b"),
        );
        assert_eq!(
            ba,
            bb,
            "opt-level={level} 两次编译的对象/可执行内容应逐字节相等（len {} vs {}）",
            ba.len(),
            bb.len()
        );
    }
}

/// 其它 codegen 旋钮：**要么支持、要么点名拒绝**（不许静默产出坏码）。
///
/// - `-C panic=unwind`：本后端没有 SEH 表 ⇒ 必须**明确拒绝**且诊断带 `[cap:UNWIND]`
///   （台账 `docs/reference/backend-gaps.md` 的同一 ID）；
/// - `-C debuginfo=0/2`、`-C codegen-units=1/8`、`-Z threads=4`：都得能编能跑、答案不变。
#[test]
fn codegen_knobs_are_either_supported_or_rejected_cleanly() {
    let dll = backend_dll();
    assert!(dll.is_file(), "找不到后端 dylib：{}", dll.display());
    let dir = std::env::temp_dir().join("forge_rustc_knobs");
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let src = dir.join("prog.rs");
    std::fs::write(&src, SRC).expect("写源码");

    // ① `panic=unwind`：必须**干净拒绝**（不是静默产出坏码）。
    //
    //   实测口径：**no_std** crate 在 `-C panic=unwind` 下会被 **rustc 前端**先拒
    //   （"unwinding panics are not supported without std"），走不到后端的
    //   `[cap:UNWIND]`——那条只对 **std** 场景有意义，而 std 场景在本地跑不了
    //   （见方案 M3-②）。所以这里断言"拒绝 + 诊断提到 unwind"，两种拒绝都算通过。
    match run_with(&src, &dll, &["-C", "panic=unwind"], &dir.join("unwind.exe")) {
        Ok(code) => panic!("panic=unwind 应被拒绝，却编出并返回 {code}"),
        Err(e) => assert!(
            e.to_lowercase().contains("unwind") || e.contains("[cap:UNWIND]"),
            "拒绝信息应当点名 unwind（前端或后端），实际：\n{e}"
        ),
    }

    // ② 其余旋钮：各档都答卷 42。
    let cases: Vec<Vec<&str>> = vec![
        vec!["-C", "debuginfo=0"],
        vec!["-C", "debuginfo=1"],
        vec!["-C", "debuginfo=2"],
        vec!["-C", "codegen-units=1"],
        vec!["-C", "codegen-units=8"],
        vec!["-C", "codegen-units=8", "-Z", "threads=4"],
    ];
    let mut failures: Vec<String> = Vec::new();
    for (i, extra) in cases.iter().enumerate() {
        let out = dir.join(format!("knob_{i}.exe"));
        match run_with(&src, &dll, extra, &out) {
            Ok(42) => {}
            Ok(code) => failures.push(format!("{extra:?} ⇒ 退出码 {code}（期望 42）")),
            Err(e) => failures.push(format!("{extra:?} ⇒ {e}")),
        }
    }
    assert!(
        failures.is_empty(),
        "各旋钮档必须给出同一答案（42）：\n{}",
        failures.join("\n")
    );
}
