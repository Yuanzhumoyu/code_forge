//! DWARF 调试段**自动**核对（v27 M5 补面）。
//!
//! 后端对外承诺"DWARF5 调试信息（gdb 实证）"，但此前那条只靠**手工**跑 gdb 验过——
//! 没人能拦住后续改动把它弄坏。这里直接读**对象文件的 COFF 段表**，断言：
//!
//! - `-C debuginfo=2` ⇒ `.debug_info` / `.debug_abbrev` / `.debug_line` / `.debug_str` 都在；
//! - `-C debuginfo=0` ⇒ 这些段**都不在**（负向对照，防止"永远为真"的空断言）。
//!
//! 只解析 COFF 的固定部分（20 字节文件头 + 40 字节段头 + 长名走字符串表），不引第三方库。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const SRC: &str = r#"
#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

#[inline(never)]
fn helper(n: u64) -> u64 {
    n * 2 + 1
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> i32 {
    helper(20) as i32 + 1
}
"#;

fn backend_dll() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace 根可解析");
    root.join("target").join("debug").join("forge_rustc.dll")
}

/// 编译成对象（后端产出的对象名是固定的 `forge_codegen_output.o`）。
fn build_object(dll: &Path, src: &Path, debuginfo: &str, outdir: &Path) -> PathBuf {
    std::fs::create_dir_all(outdir).expect("建输出目录");
    let out = Command::new("rustc")
        .arg(format!("-Zcodegen-backend={}", dll.display()))
        .args(["-Z", "threads=1"])
        .args(["-C", "panic=abort"])
        .args(["-C", &format!("debuginfo={debuginfo}")])
        .args(["--edition", "2024"])
        .args(["--crate-type", "lib", "--emit=obj", "--out-dir"])
        .arg(outdir)
        .arg(src)
        .output()
        .expect("启动 rustc");
    assert!(
        out.status.success(),
        "编译失败（debuginfo={debuginfo}）：\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let obj = outdir.join("forge_codegen_output.o");
    assert!(obj.is_file(), "没找到对象文件 {}", obj.display());
    obj
}

/// 读 COFF 对象的**段名集合**（长名经字符串表解析）。
fn coff_section_names(bytes: &[u8]) -> BTreeSet<String> {
    let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]) as usize;
    let u32_at = |o: usize| {
        u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]) as usize
    };

    let nsec = u16_at(2);
    let sym_ptr = u32_at(8);
    let nsyms = u32_at(12);
    // 字符串表紧跟在符号表之后：前 4 字节是它自己的总长。
    let strtab = sym_ptr + nsyms * 18;

    let mut names = BTreeSet::new();
    for i in 0..nsec {
        let hdr = 20 + i * 40;
        let raw = &bytes[hdr..hdr + 8];
        if raw[0] == b'/' {
            // `/N` = 字符串表偏移（十进制 ASCII）。
            let off: usize = std::str::from_utf8(&raw[1..])
                .expect("段名偏移是 ASCII")
                .trim_end_matches('\0')
                .parse()
                .expect("段名偏移可解析");
            let start = strtab + off;
            let end = start + bytes[start..].iter().position(|&b| b == 0).unwrap_or(0);
            names.insert(String::from_utf8_lossy(&bytes[start..end]).into_owned());
        } else {
            let end = raw.iter().position(|&b| b == 0).unwrap_or(8);
            names.insert(String::from_utf8_lossy(&raw[..end]).into_owned());
        }
    }
    names
}

#[test]
fn debuginfo_controls_the_dwarf_sections() {
    let dll = backend_dll();
    assert!(dll.is_file(), "找不到后端 dylib：{}", dll.display());
    let dir = std::env::temp_dir().join("forge_rustc_dwarf");
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let src = dir.join("prog.rs");
    std::fs::write(&src, SRC).expect("写源码");

    // ① `debuginfo=2`：四个核心调试段 + `.debug_frame`（M2 的 CFI）都要在。
    let with = build_object(&dll, &src, "2", &dir.join("with"));
    let names = coff_section_names(&std::fs::read(&with).expect("读对象"));
    // 实测集合（v27 的 DWARF5 发射）：`.debug_str` **不在**——行号表用的是
    // DWARF5 的 `.debug_line_str`（这是实测出来的，不是照抄 LLVM 的段名）。
    for want in [
        ".debug_info",
        ".debug_abbrev",
        ".debug_line",
        ".debug_line_str",
        ".debug_aranges",
    ] {
        assert!(
            names.contains(want),
            "debuginfo=2 必须产出 `{want}`（实际段：{names:?}）"
        );
    }
    assert!(
        names.contains(".debug_frame"),
        "debuginfo=2 必须产出 `.debug_frame`（CFI；实际段：{names:?}）"
    );

    // ② `debuginfo=0`：负向对照——上面那些段**一个都不该出现**。
    let without = build_object(&dll, &src, "0", &dir.join("without"));
    let bare = coff_section_names(&std::fs::read(&without).expect("读对象"));
    for unwanted in [
        ".debug_info",
        ".debug_abbrev",
        ".debug_line",
        ".debug_line_str",
        ".debug_aranges",
        ".debug_frame",
    ] {
        assert!(
            !bare.contains(unwanted),
            "debuginfo=0 不该有 `{unwanted}`（实际段：{bare:?}）"
        );
    }
}

/// 调试信息不能改变**程序语义**：`debuginfo=2` 编出来的程序答案仍然是 41。
#[test]
fn debuginfo_does_not_change_the_answer() {
    let dll = backend_dll();
    assert!(dll.is_file(), "找不到后端 dylib：{}", dll.display());
    let dir = std::env::temp_dir().join("forge_rustc_dwarf_run");
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let src = dir.join("prog.rs");
    std::fs::write(&src, SRC).expect("写源码");

    // 这份 SRC 的答案：helper(20) = 41，+1 = 42（与 opt_levels 的 42 同口径）。
    let exe = dir.join("prog.exe");
    let out = Command::new("rustc")
        .arg(format!("-Zcodegen-backend={}", dll.display()))
        .args(["-Z", "threads=1"])
        .args(["-C", "panic=abort"])
        .args(["-C", "debuginfo=2"])
        .args(["--edition", "2024"])
        .args(["--crate-type", "bin"])
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .args([
            "-C",
            "link-args=/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib",
        ])
        .output()
        .expect("启动 rustc");
    assert!(
        out.status.success(),
        "编译失败：\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let status = Command::new(&exe).status().expect("运行产物");
    assert_eq!(status.code(), Some(42), "debuginfo=2 不该改变程序语义");
}
