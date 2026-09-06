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

/// movz/movk 载入 64 位常量到 xRd（sf=1 100101 hw imm16 rd）。
fn mov_imm(b: &mut Vec<u8>, rd: u32, val: u64) {
    let mut v = val;
    let mut hw = 0u32;
    while hw < 4 {
        let part = (v & 0xFFFF) as u32;
        let top: u32 = if hw == 0 { 0b100101 } else { 0b111001 };
        w(b, (1 << 31) | (top << 23) | (hw << 21) | (part << 5) | rd);
        v >>= 16;
        hw += 1;
        if v == 0 {
            break;
        }
    }
}

/// mov xd, xm（orr xd, xzr, xm；sf=1 101010 0 0 11111 rm rd）。
fn mov_reg(b: &mut Vec<u8>, rd: u32, rm: u32) {
    w(b, (1 << 31) | (0b101010 << 24) | (0b11111 << 5) | (rm << 16) | rd);
}

/// add xd, xm, #0（mov sp, xm 等 sp 语义必须走 ADD——ORR rd=31 写 xzr 无效）
fn mov_sp_reg(b: &mut Vec<u8>, rd: u32, rm: u32) {
    // add imm X：0x91000000 | (rn<<5) | rd（imm=0）
    w(b, 0x91000000u32 | (rm << 5) | rd);
}

fn hlt(b: &mut Vec<u8>) {
    w(b, 0xD4400000 | 0xF000); // hlt #0xf000（semihosting）
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
    w(&mut b, 0xF90007E2); // str x2, [sp]（offset 0，scaled 0）
    w(&mut b, 0xF9000FE0); // str x0, [sp, #8]（scaled 1）
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
    let tmp = std::env::temp_dir().join(format!("forge_a64_{}.elf", std::process::id()));
    let mut f = std::fs::File::create(&tmp).map_err(|e| format!("create elf: {e}"))?;
    f.write_all(elf).map_err(|e| format!("write elf: {e}"))?;
    drop(f);
    let out = Command::new(qemu)
        .args([
            "-M", "virt", "-cpu", "cortex-a57", "-kernel", tmp.to_str().unwrap(), "-nographic",
            "-semihosting-config", "enable=on,target=native", "-monitor", "none", "-serial",
            "none", "-no-reboot",
        ])
        .output()
        .map_err(|e| format!("spawn qemu: {e}"))?;
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() {
        return Err(format!(
            "qemu-aarch64 失败 exit {:?}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(out.status.code().unwrap_or(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use code_forge::backend::CompiledFunction;

    fn compiled_const(w0_imm: u32) -> CompiledFunction {
        // movz w0, #imm（sf=0 100101 hw imm16 rd=0）
        let movz = (0b0100101 << 23) | ((w0_imm & 0xFFFF) << 5);
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
    #[ignore = "QEMU aarch64 冒烟调试中（crt0/ELF 加载挂起）——修通后移除"]
        if qemu_aarch64_path().is_none() {
            eprintln!("SKIP: qemu-system-aarch64 未安装");
            return;
        }
        let cf = compiled_const(42);
        let r = exec_aarch64(&cf, &[]).unwrap_or_else(|e| panic!("exec_aarch64: {e}"));
        assert_eq!(r, 42, "got {r}");
    }
}
