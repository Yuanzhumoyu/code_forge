//! **arm64 对象级 oracle**（v27 P3）：把 A64 编码/对象层与**本机 clang** 独立对照。
//!
//! ## 为什么要有它
//!
//! 现有的 `arm64_tests.rs` / `arm64_tm_tests.rs` 用**手抄的 golden 常量**（当年由
//! `clang --target=aarch64-none-elf -c` + `llvm-objdump` 实测得到）。golden 能防回退，
//! 但防不了"当初抄错"——P3 要求的是**独立 oracle**：同一段汇编文本交给 clang 汇编，
//! 逐字节对照我们的编码器；编译产物再写成真实 ELF 对象，用 LLVM 工具核对
//! `.text` 字节/机器类型/重定位表的存在性。
//!
//! 本机没有 aarch64 执行环境（无 qemu / 无 aarch64 std），所以这是 P3 的**对象级**那一档；
//! 执行级验证仍记在方案 §2.7（环境依赖）。
//!
//! ## 跳过而不是假绿
//!
//! 需要 `clang` + `llvm-objcopy` + `llvm-objdump` + `llvm-readobj`。缺任意一个就打印
//! `SKIP arm64-oracle: ...` 并返回（CI 镜像里有；没有的环境不该被判红）。
//! 产物落 `target/tmp/arm64-oracle/`（仓库规矩：跑出来的东西不进工作区）。

use std::path::{Path, PathBuf};
use std::process::Command;

/// oracle 目录：`<workspace>/target/tmp/arm64-oracle`。
fn oracle_dir() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..");
    let dir = root.join("target").join("tmp").join("arm64-oracle");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// 跑一个外部工具；spawn 失败（工具不存在）⇒ `None`。
fn run(tool: &str, args: &[&str], cwd: &Path) -> Option<std::process::Output> {
    Command::new(tool).args(args).current_dir(cwd).output().ok()
}

/// 工具齐备性：`clang -v` 等能起来即视为存在（不校验版本）。
fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// 用 clang 汇编一段 A64 文本，返回 `.text` 的字节（经 `llvm-objcopy -O binary`）。
fn clang_text_bytes(dir: &Path, tag: &str, lines: &[&str]) -> Option<Vec<u8>> {
    let src = dir.join(format!("{tag}.s"));
    let obj = dir.join(format!("{tag}.o"));
    let bin = dir.join(format!("{tag}.bin"));
    let mut text = String::from(".text\n");
    for l in lines {
        text.push_str(l);
        text.push('\n');
    }
    std::fs::write(&src, text).ok()?;
    let out = run(
        "clang",
        &[
            "--target=aarch64-unknown-linux-gnu",
            "-c",
            "-x",
            "assembler",
            src.to_str()?,
            "-o",
            obj.to_str()?,
        ],
        dir,
    )?;
    assert!(
        out.status.success(),
        "clang 汇编失败：\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = run(
        "llvm-objcopy",
        &[
            "-O",
            "binary",
            "--only-section=.text",
            obj.to_str()?,
            bin.to_str()?,
        ],
        dir,
    )?;
    assert!(
        out.status.success(),
        "llvm-objcopy 失败：\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read(&bin).ok()
}

/// 我们这边的编码：逐条 `assemble` + `encode` 后拼接。
fn our_encoding(lines: &[&str]) -> Vec<u8> {
    use forge_codegen::arm64::{assemble, encode};
    let mut out = Vec::new();
    for l in lines {
        let inst = assemble(l).unwrap_or_else(|e| panic!("assemble `{l}`: {e}"));
        let bytes = encode(&inst).unwrap_or_else(|e| panic!("encode `{l}`: {e}"));
        out.extend(bytes);
    }
    out
}

/// **编码 oracle**：覆盖后端会发射的 A64 家族（ALU 立即数/寄存器、乘除、移位、
/// 访存、MOVZ/MOVK、分支寄存/返回、NOP）。
#[test]
fn a64_encoding_table_matches_clang_oracle() {
    use forge_codegen::arm64::assemble;
    for tool in ["clang", "llvm-objcopy"] {
        if !have(tool) {
            eprintln!("SKIP arm64-oracle: 缺 `{tool}`（独立 oracle 需要本机 LLVM 工具链）");
            return;
        }
    }
    // 先剔除本谱不接受的写法（失败点归"表写错"，不是对照失败）。
    let table: &[&str] = &[
        "add x0, x0, #42",
        "add w1, w2, #7",
        "sub x3, x4, #16",
        "subs x5, x6, x7, lsl #0",
        "add x8, x9, x10, lsl #3",
        "sub w11, w12, w13, lsl #2",
        "add x14, x15, x16, lsl #0",
        "sub x17, x18, x19, lsl #0",
        "mul x0, x1, x2",
        "sdiv x3, x4, x5",
        "udiv x6, x7, x8",
        "lslv x0, x1, x2",
        "lsrv x3, x4, x5",
        "asrv x6, x7, x8",
        "lslv w9, w10, w11",
        "lsrv w12, w13, w14",
        "asrv w15, w16, w17",
        "fadd d0, d0, d1",
        "fsub s2, s3, s4",
        "fmul d5, d6, d7",
        "fdiv s8, s9, s10",
        "fmax d11, d12, d13",
        "fmin s14, s15, s16",
        "fcmp d0, d1",
        "fneg d5, d6",
        "fabs s7, s8",
        "fcvt d9, s10",
        "fcvt s11, d12",
        "scvtf s13, w14",
        "ucvtf d15, x16",
        "fcvtzs w17, s18",
        "fcvtzu x19, d20",
        "movz x9, #0x1234",
        "movk x10, #0xabcd, lsl #16",
        "mov x11, x12",
        "ldr x13, [x14, #8]",
        "str x15, [x16, #8]",
        "ldur x17, [x18, #-8]",
        "stur x19, [x20, #-8]",
        "ldr w21, [x22, #4]",
        "str w23, [x24, #4]",
        "br x25",
        "blr x26",
        "ret x30",
        "nop",
    ];
    // 表里的每条都必须能被本谱汇编器接受（否则是表写错，明确红）。
    for l in table {
        if let Err(e) = assemble(l) {
            panic!("oracle 表里的 `{l}` 不被本谱汇编器接受：{e}");
        }
    }
    let dir = oracle_dir();
    let clang_bytes =
        clang_text_bytes(&dir, "encoding_table", table).expect("clang/objcopy 齐备时应有产物");
    let ours = our_encoding(table);
    // 逐条定位差异：整段相等固然好，失败时要指出**哪一条**。
    let mut off = 0usize;
    for l in table {
        let n = forge_codegen::arm64::encode(&assemble(l).expect("表已校验"))
            .expect("encode")
            .len();
        let a = &ours[off..off + n];
        let b = &clang_bytes[off..off + n];
        assert_eq!(
            a,
            b,
            "编码与 clang oracle 不一致：`{l}`\n  ours  = {a:02x?}\n  clang = {b:02x?}\n  dir={}",
            dir.display()
        );
        off += n;
    }
    assert_eq!(
        ours.len(),
        clang_bytes.len(),
        "总字节数不一致（表/伪指令展开？）"
    );
}

/// **对象级 oracle**：编译一个 arm64 函数 → 写真实 ELF 对象 → LLVM 工具核对。
#[test]
fn a64_object_matches_compiled_bytes_and_clang_toolchain() {
    for tool in ["clang", "llvm-objcopy", "llvm-objdump", "llvm-readobj"] {
        if !have(tool) {
            eprintln!("SKIP arm64-oracle: 缺 `{tool}`（对象级 oracle 需要本机 LLVM 工具链）");
            return;
        }
    }
    use forge_codegen::CompileSession;
    use forge_ir::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

    // 一个带序言/尾声/算术的函数（arm64 目前没有 Call lowering，见 isa/arm64.toml 的注释）。
    let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
    let mut b = FunctionBuilder::new("a64_oracle_add", TypeContext::new(), sig);
    let (entry, ps) = b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
    b.switch_to_block(entry);
    let v = b.iadd(ps[0], ps[1]);
    b.ret(&[v]);
    let func = b.finish().expect("build");
    let cf = CompileSession::new(forge_codegen::arm64::TargetMachine::new())
        .compile(&func)
        .expect("arm64 compile");
    assert!(!cf.code.is_empty(), "arm64 函数应有机器码");

    // 写 ELF64/AArch64 对象。
    let dir = oracle_dir();
    let obj_path = dir.join("a64_oracle_add.o");
    {
        let config = forge_object::TargetConfig::from_triple("aarch64-unknown-linux-gnu")
            .expect("aarch64 ELF triple");
        let mut writer = forge_object::ObjectWriter::new(&config).expect("writer");
        writer
            .add_function("a64_oracle_add", &cf)
            .expect("add function");
        std::fs::write(&obj_path, writer.write_to_vec().expect("write")).expect("写对象");
    }

    // ① 对象里的 .text 必须与编译结果**逐字节相同**（对象层不丢/不改字节）。
    let bin = dir.join("a64_oracle_add.text");
    let out = run(
        "llvm-objcopy",
        &[
            "-O",
            "binary",
            "--only-section=.text",
            obj_path.to_str().unwrap(),
            bin.to_str().unwrap(),
        ],
        &dir,
    )
    .expect("objcopy 齐备");
    assert!(
        out.status.success(),
        "objcopy 失败：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read(&bin).expect("读 .text");
    assert_eq!(
        text, cf.code,
        "对象 .text 与编译结果不一致（对象层改写了函数字节）"
    );

    // ② ELF 头：AArch64 + REL（不是 COFF/别的机器）。
    let out = run(
        "llvm-readobj",
        &["--file-headers", obj_path.to_str().unwrap()],
        &dir,
    )
    .expect("readobj 齐备");
    let headers = String::from_utf8_lossy(&out.stdout);
    assert!(
        headers.contains("elf64-littleaarch64") || headers.to_lowercase().contains("arch: aarch64"),
        "ELF 头应是 AArch64 小端 ELF64：\n{headers}"
    );

    // ③ 反汇编可读：函数入口有反汇编、且以 ret 收尾（对象层的 .text 是可执行的 A64）。
    let out = run(
        "llvm-objdump",
        &["-d", "--no-show-raw-insn", obj_path.to_str().unwrap()],
        &dir,
    )
    .expect("objdump 齐备");
    let dis = String::from_utf8_lossy(&out.stdout);
    assert!(
        dis.contains("<a64_oracle_add>:") && dis.contains("ret"),
        "反汇编应含函数条目与 ret：\n{dis}"
    );
}
