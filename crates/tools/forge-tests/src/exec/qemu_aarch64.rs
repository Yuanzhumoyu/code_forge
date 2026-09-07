//! QEMU aarch64 执行器（system 模式 + semihosting 退出）。
//!
//! 验证链路（2026-09 本机实测校准）：
//!   1. 编译产物（aarch64 机器码）打包成裸机 ELF（ENTRY=0x4000_0000，
//!      QEMU virt RAM 基址）。
//!   2. crt0 壳（32 位定宽指令字节流）：
//!      - 设栈：mov x0, #STACK_TOP → mov sp, x0（0x4008_0000）
//!      - 参数按序 mov x0..x7
//!      - `bl <main>`（imm26 构建期回填）
//!      - 返回后 semihosting 退出块：arg block（reason=0x20026
//!        ADP_Stopped_ApplicationExit, status=x0 返回值）→
//!        mov x1,sp; mov x0,#0x18; hlt #0xf000 → QEMU
//!        `-semihosting-config enable=on,target=native` 返回退出码
//!        （实测 main→42 ⇒ 进程退出码 42）。
//!      - main 机器码紧随退出块（LR=bl+4 → 回主，不会误入 main）。
//!   3. 子进程：`qemu-system-aarch64 -M virt -cpu cortex-a57 -kernel
//!      app.elf -nographic -semihosting-config enable=on,target=native
//!      -monitor none -serial none -no-reboot`；退出码 = 函数返回值。
//!
//! QEMU 路径：`$env:QEMU_AARCH64` → 默认安装路径 → PATH。缺失时调用方
//! 降级 CompileOnly（不失败）。

use code_forge::backend::CompiledFunction;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

/// QEMU system-mode aarch64 模拟器路径（None = 未找到）。
pub fn qemu_aarch64_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("QEMU_AARCH64") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = if cfg!(windows) {
        "qemu-system-aarch64.exe"
    } else {
        "qemu-system-aarch64"
    };
    for c in [
        r"D:\Program Files\qemu\qemu-system-aarch64.exe",
        r"C:\Program Files\qemu\qemu-system-aarch64.exe",
        r"C:\Program Files (x86)\qemu\qemu-system-aarch64.exe",
        "/usr/bin/qemu-system-aarch64",
        "/usr/local/bin/qemu-system-aarch64",
    ] {
        let p = PathBuf::from(c);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(paths) = std::env::var("PATH") {
        for d in std::env::split_paths(&paths) {
            let p = d.join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

const ENTRY: u64 = 0x4000_0000;
const STACK_TOP: u64 = 0x4008_0000;
const EM_AARCH64: u16 = 183;

/// 运行单个编译函数（无全局数据）：退出码 = 函数返回值。
pub fn exec_aarch64(compiled: &CompiledFunction, args: &[u64]) -> Result<u64, String> {
    let qemu = qemu_aarch64_path().ok_or("qemu-system-aarch64 未找到")?;
    let elf = build_elf(&[(0u32, compiled.clone())], &[], 0, args)?;
    run_qemu(&qemu, &elf)
}

/// 运行多函数模块：main_idx 为入口函数序，globals 拼数据段。
pub fn exec_aarch64_module(
    funcs: &[(u32, CompiledFunction)],
    globals: &[(String, Vec<u8>)],
    main_idx: u32,
    args: &[u64],
) -> Result<u64, String> {
    let qemu = qemu_aarch64_path().ok_or("qemu-system-aarch64 未找到")?;
    let elf = build_elf(funcs, globals, main_idx, args)?;
    run_qemu(&qemu, &elf)
}

// ─────────────────── 指令发射小工具 ───────────────────

fn w(b: &mut Vec<u8>, inst: u32) {
    b.extend_from_slice(&inst.to_le_bytes());
}

/// movz/movk 载入 64 位常量到 xRd。
/// 全字 = op9 基值（[31:23]）<<23 | hw<<21 | imm16<<5 | rd：
///   movz x 基 0x1A5<<23=0xD2800000；movk x 基 0x1E5<<23=0xF2800000
fn mov_imm(b: &mut Vec<u8>, rd: u32, val: u64) {
    let mut v = val;
    let mut hw = 0u32;
    while hw < 4 {
        let part = (v & 0xFFFF) as u32;
        let top: u32 = if hw == 0 { 0x1A5 } else { 0x1E5 };
        w(b, (top << 23) | (hw << 21) | (part << 5) | rd);
        v >>= 16;
        hw += 1;
        if v == 0 {
            break;
        }
    }
}

/// mov xd, xm（orr xd, xzr, xm；sf=1 101010 0 0 11111 rm rd）。
fn mov_reg(b: &mut Vec<u8>, rd: u32, rm: u32) {
    w(
        b,
        (1 << 31) | (0b101010 << 24) | (0b11111 << 5) | (rm << 16) | rd,
    );
}

/// add xd, xm, #0（mov sp, xm 等 sp 语义必须走 ADD——ORR rd=31 写 xzr 无效）
fn mov_sp_reg(b: &mut Vec<u8>, rd: u32, rm: u32) {
    // add imm X：0x91000000 | (rn<<5) | rd（imm=0）
    w(b, 0x91000000u32 | (rm << 5) | rd);
}

fn hlt(b: &mut Vec<u8>) {
    w(b, 0xD4400000 | (0xF000 << 5)); // hlt #0xf000（semihosting；imm16 在 [21:5]）
}

fn bl_here(b: &mut Vec<u8>) -> usize {
    // bl #0 占位（返回该指令字节偏移以便回填 imm26）
    let off = b.len();
    w(b, 0x94000000);
    off
}

/// crt0 前缀：设栈 + 参数 + bl 占位 + semihosting 退出块。
/// 返回 (bytes, bl_off) —— bl_off 由调用方回填跳 main。
fn crt0_with_exit(args: &[u64]) -> (Vec<u8>, usize) {
    let mut b: Vec<u8> = Vec::new();
    mov_imm(&mut b, 0, STACK_TOP);
    mov_sp_reg(&mut b, 31, 0); // mov sp, x0（add 形式）
    for (i, a) in args.iter().enumerate().take(8) {
        mov_imm(&mut b, i as u32, *a);
    }
    let bl_off = bl_here(&mut b);
    // ── 退出块（main ret 后执行；LR 指向 bl+4=本块开头）──
    // sub sp, sp, #16（d10043ff 形式：sub sp,sp,#16）
    w(&mut b, 0xD10043FF);
    mov_imm(&mut b, 2, 0x20026);
    w(&mut b, 0xF90003E2); // str x2, [sp]
    w(&mut b, 0xF90007E0); // str x0, [sp, #8]
    mov_sp_reg(&mut b, 1, 31); // mov x1, sp（add 形式）
    mov_imm(&mut b, 0, 0x18);
    hlt(&mut b);
    w(&mut b, 0x14000000); // b .（保险自旋，正常不会到达）
    (b, bl_off)
}

// ─────────────────── ELF 打包 ───────────────────

fn build_elf(
    funcs: &[(u32, CompiledFunction)],
    globals: &[(String, Vec<u8>)],
    main_idx: u32,
    args: &[u64],
) -> Result<Vec<u8>, String> {
    let (mut code, bl_off) = crt0_with_exit(args);
    // 在退出块后拼接入口函数机器码
    let main = funcs
        .iter()
        .find(|(i, _)| *i == main_idx)
        .ok_or_else(|| format!("main_idx {main_idx} 不存在"))?;
    let main_start = code.len();
    code.extend_from_slice(&main.1.code);
    // 回填 bl imm26 = (main_start - bl_off)/4（A64：target = PC + imm26<<2）
    let imm = ((main_start - bl_off) / 4) as u32;
    let inst = 0x94000000u32 | (imm & 0x03FF_FFFF);
    code[bl_off..bl_off + 4].copy_from_slice(&inst.to_le_bytes());

    // 数据段（8 对齐）
    let mut data: Vec<u8> = Vec::new();
    for g in globals {
        let pad = data.len() % 8;
        if pad != 0 {
            data.extend(std::iter::repeat(0u8).take(8 - pad));
        }
        data.extend_from_slice(&g.1);
    }

    let text_off = 0x1000u64;
    let data_off = text_off + code.len() as u64;
    let hdr_len = 64u64;
    let phentsize = 56u64;
    let phnum = if data.is_empty() { 1u64 } else { 2u64 };

    let mut e: Vec<u8> = Vec::new();
    // ELF64 header（little endian）
    e.extend(&[0x7F, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    e.extend(&2u16.to_le_bytes()); // ET_EXEC
    e.extend(&EM_AARCH64.to_le_bytes());
    e.extend(&1u32.to_le_bytes()); // version
    e.extend(&ENTRY.to_le_bytes());
    e.extend(&hdr_len.to_le_bytes()); // phoff
    e.extend(&0u64.to_le_bytes()); // shoff
    e.extend(&0u32.to_le_bytes()); // flags
    e.extend(&(hdr_len as u16).to_le_bytes());
    e.extend(&(phentsize as u16).to_le_bytes());
    e.extend(&(phnum as u16).to_le_bytes());
    e.extend(&0u16.to_le_bytes());
    e.extend(&0u16.to_le_bytes());
    e.extend(&0u16.to_le_bytes());
    debug_assert_eq!(e.len(), 64);

    // PT_LOAD helper（aarch64：seg 地址须 4K 对齐语义可放宽——QEMU 接受）
    let ph = |off: u64, vaddr: u64, filesz: u64, flags: u32| {
        let mut p: Vec<u8> = Vec::new();
        p.extend(&1u32.to_le_bytes()); // PT_LOAD
        p.extend(&flags.to_le_bytes());
        p.extend(&off.to_le_bytes());
        p.extend(&vaddr.to_le_bytes());
        p.extend(&vaddr.to_le_bytes());
        p.extend(&filesz.to_le_bytes());
        p.extend(&filesz.to_le_bytes());
        p.extend(&0x1000u64.to_le_bytes());
        p
    };
    e.extend(ph(text_off, ENTRY, code.len() as u64, 5)); // R+X
    if !data.is_empty() {
        let dv = (ENTRY + code.len() as u64 + 0xFFF) & !0xFFF;
        e.extend(ph(data_off, dv, data.len() as u64, 6)); // R+W
    }
    while (e.len() as u64) < text_off {
        e.push(0);
    }
    e.extend_from_slice(&code);
    if !data.is_empty() {
        while (e.len() as u64) < data_off {
            e.push(0);
        }
        e.extend_from_slice(&data);
    }
    Ok(e)
}

fn run_qemu(qemu: &PathBuf, elf: &[u8]) -> Result<u64, String> {
    // 文件名必须唯一：并行测试（同进程多线程跑同一 lib）会互相覆盖
    // `{pid}.elf`——qemu 启动读到别的用例的 ELF，返回值串扰
    //（曾现：smoke(42) 拿到大常量用例的 0x12345678）。
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!(
        "forge_a64_{}_{}.elf",
        std::process::id(),
        n
    ));
    let mut f = std::fs::File::create(&tmp).map_err(|e| format!("create elf: {e}"))?;
    f.write_all(elf).map_err(|e| format!("write elf: {e}"))?;
    drop(f);
    let mut cmd = Command::new(qemu);
    cmd.args([
        "-M",
        "virt",
        "-cpu",
        "cortex-a57",
        "-kernel",
        tmp.to_str().unwrap(),
        "-nographic",
        "-semihosting-config",
        "enable=on,target=native",
        "-monitor",
        "none",
        "-serial",
        "none",
        "-no-reboot",
    ]);
    if let Ok(d) = std::env::var("FORGE_A64_DUMP") {
        let _ = std::fs::copy(&tmp, d);
    }
    if let Ok(t) = std::env::var("FORGE_A64_TRACE") {
        cmd.args(["-d", "in_asm", "-D", &t]);
    }
    let out = cmd.output().map_err(|e| format!("spawn qemu: {e}"))?;
    let _ = std::fs::remove_file(&tmp);
    match out.status.code() {
        Some(code) => Ok(code as u64),
        None => Err(format!(
            "qemu-aarch64 异常终止（signal）\n{}",
            String::from_utf8_lossy(&out.stderr)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use code_forge::backend::CompiledFunction;

    fn compiled_const(w0_imm: u32) -> CompiledFunction {
        // movz w0, #imm（op9 W 基 0xA5<<23=0x52800000 | imm<<5）
        let movz = (0xA5u32 << 23) | ((w0_imm & 0xFFFF) << 5);
        // ret
        let ret = 0xD65F03C0u32;
        let mut code = Vec::new();
        code.extend_from_slice(&movz.to_le_bytes());
        code.extend_from_slice(&ret.to_le_bytes());
        CompiledFunction {
            code,
            relocations: vec![],
            code_size: 8,
            line_entries: vec![],
            cfi: None,
        }
    }

    #[test]
    fn qemu_aarch64_exec_returns_const() {
        if qemu_aarch64_path().is_none() {
            eprintln!("SKIP: qemu-system-aarch64 未安装");
            return;
        }
        let cf = compiled_const(42);
        let r = exec_aarch64(&cf, &[]).unwrap_or_else(|e| panic!("exec_aarch64: {e}"));
        assert_eq!(r, 42, "got {r}");
    }

    /// P3① 大立即数（完整编译链 + QEMU 真执行）：
    /// return 0x12345678 (i32) → W 序列 movz w,hw1 + movk w,hw0 → 寄存器
    /// 低 8 位 0x78（退出码符号扩展前 120）。0x12345678 > imm16u 单条域，
    /// 必须走多序列（MOVZW1+MOVKW）。
    #[test]
    fn qemu_aarch64_exec_large_const_i32() {
        if qemu_aarch64_path().is_none() {
            eprintln!("SKIP: qemu-system-aarch64 未安装");
            return;
        }
        use code_forge::backend::arm64_v12::TargetMachine;
        use code_forge::backend::FunctionCompiler;
        use code_forge::prelude::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("ret_0x12345678", TypeContext::new(), sig);
        b.create_block_here();
        let c = b.iconst_i32(0x1234_5678);
        b.ret(&[c]);
        let func = b.finish().expect("build");
        let compiled = FunctionCompiler::new(TargetMachine::new())
            .compile_raw(&func)
            .expect("compile");
        let r = exec_aarch64(&compiled, &[]).unwrap_or_else(|e| panic!("exec_aarch64: {e}"));
        // QEMU semihost SYS_EXIT 的 host 退出码 = status word（低 8 位在
        // Windows/qemu 传递中按字节保留）；0x12345678 低 8 位 = 0x78
        assert_eq!(
            r & 0xFF,
            0x78,
            "0x12345678 低 8 位应为 0x78；got {r:#x}"
        );
    }

    /// i64 负大值：return -1_000_000_007 → X 序列 movz(hw3)+movk(hw2/1/0)
    /// 构造两补码位型 0xFFFF_FFFF_C465_35F9；低 8 位 0xF9 符号扩展 = -7。
    #[test]
    fn qemu_aarch64_exec_large_const_i64_neg() {
        if qemu_aarch64_path().is_none() {
            eprintln!("SKIP: qemu-system-aarch64 未安装");
            return;
        }
        use code_forge::backend::arm64_v12::TargetMachine;
        use code_forge::backend::FunctionCompiler;
        use code_forge::prelude::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};
        let sig = FunctionSignature::new(&[], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("ret_neg1b", TypeContext::new(), sig);
        b.create_block_here();
        let c = b.iconst_i64(-1_000_000_007);
        b.ret(&[c]);
        let func = b.finish().expect("build");
        let compiled = FunctionCompiler::new(TargetMachine::new())
            .compile_raw(&func)
            .expect("compile");
        let r = exec_aarch64(&compiled, &[]).unwrap_or_else(|e| panic!("exec_aarch64: {e}"));
        // -1_000_000_007 低 8 位 = 0xF9（有符号字节 = -7）
        assert_eq!(r & 0xFF, 0xF9, "-1_000_000_007 低 8 位应为 0xF9；got {r:#x}");
    }
}
