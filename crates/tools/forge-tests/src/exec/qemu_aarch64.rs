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
    let elf = build_elf(&[(0u32, compiled.clone())], &[], 0, args, false)?;
    run_qemu(&qemu, &elf)
}

/// 浮点实参执行（v27 M6-101）：实参按 AAPCS64 装进 **d0..d7**（位模式），
/// 返回值仍是 x0（退出码 = 低 8 位；调用方按有符号字节解释）。
pub fn exec_aarch64_f64(compiled: &CompiledFunction, args: &[u64]) -> Result<u64, String> {
    let qemu = qemu_aarch64_path().ok_or("qemu-system-aarch64 未找到")?;
    let elf = build_elf(&[(0u32, compiled.clone())], &[], 0, args, true)?;
    run_qemu(&qemu, &elf)
}

/// 运行多函数模块：`main_idx` 为入口函数序（= 列表位置），globals 拼数据段。
/// 跨函数 `Call` 的 `@N` 与 `GlobalAddr` 的 `G{id}` 由 `build_elf` 就地补丁。
pub fn exec_aarch64_module(
    funcs: &[(u32, CompiledFunction)],
    globals: &[(String, Vec<u8>)],
    main_idx: u32,
    args: &[u64],
) -> Result<u64, String> {
    let qemu = qemu_aarch64_path().ok_or("qemu-system-aarch64 未找到")?;
    let elf = build_elf(funcs, globals, main_idx, args, false)?;
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
fn crt0_with_exit(args: &[u64], f64_args: bool) -> (Vec<u8>, usize) {
    let mut b: Vec<u8> = Vec::new();
    mov_imm(&mut b, 0, STACK_TOP);
    mov_sp_reg(&mut b, 31, 0); // mov sp, x0（add 形式）
    // **开 FP/SIMD**（v27 M6-101）：A64 复位时 CPACR_EL1.FPEN=0，访问 d/v 寄存器
    // 会 trap 到异常向量（实测：浮点用例在 `fmov d0, x9` 处跳 0x200 死循环）。
    // 无条件开（整数路径无害）：mov x9,#0x300000; msr cpacr_el1,x9; isb。
    w(&mut b, 0xD2A0_0609);
    w(&mut b, 0xD518_1049);
    w(&mut b, 0xD503_3FDF);
    for (i, a) in args.iter().enumerate().take(8) {
        if f64_args {
            // 浮点实参走 AAPCS64 的 d0..d7：位模式先进 x9，再 `fmov dN, x9`
            // （0x9E670000 | (M<<5) | N，M=9）。
            mov_imm(&mut b, 9, *a);
            w(&mut b, 0x9E67_0000 | (9 << 5) | i as u32);
        } else {
            mov_imm(&mut b, i as u32, *a);
        }
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
    f64_args: bool,
) -> Result<Vec<u8>, String> {
    let (mut code, bl_off) = crt0_with_exit(args, f64_args);
    // **布局全部函数**（v27 M6-97）：旧实现只拼 `main`，所以带 `Call` 的模块根本
    // 没有"另一个函数"可跳。现在按 `(idx, code)` 顺序各自 4 对齐落进代码段，
    // 记下函数下标 → 段内偏移，供 crt0 的 `bl main` 与函数内 `Call` 重定位回填。
    let mut starts: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
    for (idx, cf) in funcs {
        while code.len() % 4 != 0 {
            code.push(0);
        }
        starts.insert(*idx, code.len());
        code.extend_from_slice(&cf.code);
    }
    let main_start = *starts
        .get(&main_idx)
        .ok_or_else(|| format!("main_idx {main_idx} 不存在"))?;
    // 回填 crt0 的 bl imm26 = (main_start - bl_off)/4（A64：target = PC + imm26<<2）
    let imm = ((main_start - bl_off) / 4) as u32;
    let inst = 0x94000000u32 | (imm & 0x03FF_FFFF);
    code[bl_off..bl_off + 4].copy_from_slice(&inst.to_le_bytes());

    // **函数间 `Call` 重定位**：`Call` 降级成 `@N` 符号（N = 目标函数下标），
    // 编码器留 `0x97FFFFFF` 占位；这里按 `RelocKind::patch_value` 统一口径算出
    // 目标地址差，再写 A64 `BL` 的 imm26。**全局地址**（`GlobalAddr`）走
    // `G{id}`/名字 → 数据段地址 + MOVW_UABS_G0..G3 的 16 位切片补丁（v27 M6-112）。
    // 其余形态明确报错，不猜。
    // 数据段（8 对齐）+ 每个全局变量的段内偏移（补丁期要把它折成绝对地址）。
    let mut data: Vec<u8> = Vec::new();
    let mut global_offsets: Vec<usize> = Vec::new();
    for g in globals {
        let pad = data.len() % 8;
        if pad != 0 {
            data.extend(std::iter::repeat_n(0u8, 8 - pad));
        }
        global_offsets.push(data.len());
        data.extend_from_slice(&g.1);
    }
    // 数据段**虚拟地址**按页对齐（与程序头 p_vaddr 同口径）：全局地址补丁必须用它。
    let data_vaddr = (ENTRY + code.len() as u64 + 0xFFF) & !0xFFF;

    for (idx, cf) in funcs {
        let fstart = *starts.get(idx).expect("刚插入");
        for r in &cf.relocations {
            if r.symbol.is_empty() {
                continue; // 函数内 label fixup 已由 CodeSink 的补丁器解析
            }
            let site = ENTRY + (fstart + r.offset) as u64;
            let at = fstart + r.offset;
            let sym = r.symbol.as_str();
            let target = if let Some(n) = sym.strip_prefix('@') {
                let ti: u32 = n
                    .parse()
                    .map_err(|_| format!("函数符号 '@{n}' 不是函数下标"))?;
                let tstart = *starts
                    .get(&ti)
                    .ok_or_else(|| format!("重定位 @{ti} 指向的函数不在本次模块里"))?;
                ENTRY + tstart as u64
            } else if let Some(n) = sym.strip_prefix('G') {
                let gi: usize = n
                    .parse()
                    .map_err(|_| format!("全局符号 'G{n}' 不是全局下标"))?;
                let goff = *global_offsets
                    .get(gi)
                    .ok_or_else(|| format!("重定位 G{gi} 指向的全局不在本次模块里"))?;
                data_vaddr + goff as u64
            } else {
                return Err(format!(
                    "未解析的符号 '{sym}'（qemu aarch64 夹具只接 @N 函数与 G{{id}} 全局）"
                ));
            };
            match r.kind {
                code_forge::RelocKind::Absolute(4) => {
                    let name = r.reloc.as_str();
                    let (shift, ok) = match name {
                        "abs_g0" => (0u32, true),
                        "abs_g1" => (16, true),
                        "abs_g2" => (32, true),
                        "abs_g3" => (48, true),
                        _ => (0, false),
                    };
                    if !ok {
                        return Err(format!(
                            "不支持的绝对重定位 '{name}'（只接 MOVW_UABS_G0..G3）"
                        ));
                    }
                    let slice = ((target >> shift) & 0xFFFF) as u32;
                    let mut word = [0u8; 4];
                    word.copy_from_slice(&code[at..at + 4]);
                    let w = u32::from_le_bytes(word) & !0x001F_FFE0 | (slice << 5);
                    code[at..at + 4].copy_from_slice(&w.to_le_bytes());
                }
                code_forge::RelocKind::Relative(4, _) => {
                    let value = r.kind.patch_value(target, site) as i64;
                    if value % 4 != 0 {
                        return Err(format!("BL 目标未 4 字节对齐（delta={value}）"));
                    }
                    let imm26 = value >> 2;
                    if !(-(1 << 25)..(1 << 25)).contains(&imm26) {
                        return Err(format!("BL 超出 imm26 范围（delta={value}）"));
                    }
                    let patched = 0x94000000u32 | ((imm26 as u32) & 0x03FF_FFFF);
                    code[at..at + 4].copy_from_slice(&patched.to_le_bytes());
                }
                other => {
                    return Err(format!(
                        "不支持的重定位形态：kind={other:?} symbol='{sym}' addend={}",
                        r.addend
                    ));
                }
            }
        }
    }

    let text_off = 0x1000u64;
    let data_off = text_off + code.len() as u64;
    // 数据段**虚拟地址**按页对齐（与程序头 p_vaddr 同口径）：全局地址补丁必须用它。
    let data_vaddr = (ENTRY + code.len() as u64 + 0xFFF) & !0xFFF;
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
        e.extend(ph(data_off, data_vaddr, data.len() as u64, 6)); // R+W
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
    let _qemu_guard = super::qemu_serialize();
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("forge_a64_{}_{}.elf", std::process::id(), n));
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
        use code_forge::backend::CompileSession;
        use code_forge::backend::arm64::TargetMachine;
        use code_forge::prelude::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("ret_0x12345678", TypeContext::new(), sig);
        b.create_block_here();
        let c = b.constant(0x1234_5678i32);
        b.ret(&[c]);
        let func = b.finish().expect("build");
        let compiled = CompileSession::new(TargetMachine::new())
            .compile(&func)
            .expect("compile");
        let r = exec_aarch64(&compiled, &[]).unwrap_or_else(|e| panic!("exec_aarch64: {e}"));
        // QEMU semihost SYS_EXIT 的 host 退出码 = status word（低 8 位在
        // Windows/qemu 传递中按字节保留）；0x12345678 低 8 位 = 0x78
        assert_eq!(r & 0xFF, 0x78, "0x12345678 低 8 位应为 0x78；got {r:#x}");
    }

    /// i64 负大值：return -1_000_000_007 → X 序列 movz(hw3)+movk(hw2/1/0)
    /// 构造两补码位型 0xFFFF_FFFF_C465_35F9；低 8 位 0xF9 符号扩展 = -7。
    #[test]
    fn qemu_aarch64_exec_large_const_i64_neg() {
        if qemu_aarch64_path().is_none() {
            eprintln!("SKIP: qemu-system-aarch64 未安装");
            return;
        }
        use code_forge::backend::CompileSession;
        use code_forge::backend::arm64::TargetMachine;
        use code_forge::prelude::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};
        let sig = FunctionSignature::new(&[], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("ret_neg1b", TypeContext::new(), sig);
        b.create_block_here();
        let c = b.constant(-1_000_000_007);
        b.ret(&[c]);
        let func = b.finish().expect("build");
        let compiled = CompileSession::new(TargetMachine::new())
            .compile(&func)
            .expect("compile");
        let r = exec_aarch64(&compiled, &[]).unwrap_or_else(|e| panic!("exec_aarch64: {e}"));
        // -1_000_000_007 低 8 位 = 0xF9（有符号字节 = -7）
        assert_eq!(
            r & 0xFF,
            0xF9,
            "-1_000_000_007 低 8 位应为 0xF9；got {r:#x}"
        );
    }

    /// 构建多块 if/else + phi merge 函数（P3②：跨块分支/跳转/多 return
    /// block + 统一尾声经 epilogue_label）。cond=true → then 翼 40、
    /// cond=false → else 翼 2；end 块 phi 参数承载两翼值。
    fn compile_if_else(cond: bool) -> code_forge::backend::CompiledFunction {
        use code_forge::backend::CompileSession;
        use code_forge::backend::arm64::TargetMachine;
        use code_forge::prelude::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("ifelse", TypeContext::new(), sig);
        let entry = b.create_block();
        let then = b.create_block();
        let els = b.create_block();
        let (end, phi) = b.create_block_with_params(&[(TypeId::I32, "phi")]);
        b.switch_to_block(entry);
        let c = b.const_bool(cond);
        b.branch(c, then, &[], els, &[]);
        b.switch_to_block(then);
        let v1 = b.constant(40i32);
        b.jump(end, &[v1]);
        b.switch_to_block(els);
        let v2 = b.constant(2i32);
        b.jump(end, &[v2]);
        b.switch_to_block(end);
        let z = b.constant(0i32);
        let r = b.iadd(phi[0], z);
        b.ret(&[r]);
        let func = b.finish().expect("build");
        CompileSession::new(TargetMachine::new())
            .compile(&func)
            .expect("compile")
    }

    /// P3② 端到端：B（imm26）+ CBZ（imm19）label 槽跨块分支 + 多 return
    /// block 经统一尾声（epilogue_label=true）——reloc patcher 位段重排
    /// 后 QEMU 真执行验证控制流。
    #[test]
    fn qemu_aarch64_exec_multi_block_if_else() {
        if qemu_aarch64_path().is_none() {
            eprintln!("SKIP: qemu-system-aarch64 未安装");
            return;
        }
        // cond=true → then 翼 40
        let cf = compile_if_else(true);
        let r = exec_aarch64(&cf, &[]).unwrap_or_else(|e| panic!("exec_aarch64: {e}"));
        assert_eq!(r & 0xFF, 40, "if(true) 应走 then 翼返回 40；got {r:#x}");
        // cond=false → else 翼 2
        let cf2 = compile_if_else(false);
        let r2 = exec_aarch64(&cf2, &[]).unwrap_or_else(|e| panic!("exec_aarch64: {e}"));
        assert_eq!(r2 & 0xFF, 2, "if(false) 应走 else 翼返回 2；got {r2:#x}");
    }

    /// **P3 执行级：函数间 `Call`（v27 M6-97）**。`add(a,b)` 与 `main = add(20,22)`
    /// 分别编译，模块级 ELF 布局 + `@N` 重定位回填后交给 QEMU 真执行。
    /// 这是 arm64 `[inst.BL]` 拿到 `roles=["call"]` 后的端到端验收：寄存器实参
    /// （x0/x1）、寄存器返回（x0）、BL 的 imm26 相对位移全部走真实机器码。
    #[test]
    fn qemu_aarch64_exec_direct_call() {
        if qemu_aarch64_path().is_none() {
            eprintln!("SKIP: qemu-system-aarch64 未安装");
            return;
        }
        use code_forge::backend::CompileSession;
        use code_forge::backend::arm64::TargetMachine;
        use code_forge::prelude::{
            FuncRef, FunctionBuilder, FunctionSignature, TypeContext, TypeId,
        };
        // add(a, b) = a + b
        let mut b = FunctionBuilder::new(
            "add",
            TypeContext::new(),
            FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]),
        );
        let (e, ps) = b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        b.switch_to_block(e);
        let s = b.iadd(ps[0], ps[1]);
        b.ret(&[s]);
        let add = b.finish().expect("add");
        // main() = add(20, 22)
        let mut b = FunctionBuilder::new(
            "main",
            TypeContext::new(),
            FunctionSignature::new(&[], &[TypeId::I64]),
        );
        b.create_block_here();
        let x = b.constant(20i64);
        let y = b.constant(22i64);
        let r = b.call(FuncRef::new(0), &[x, y], &[TypeId::I64])[0];
        b.ret(&[r]);
        let main = b.finish().expect("main");

        let sess = CompileSession::new(TargetMachine::new());
        let add_cf = sess.compile(&add).expect("compile add");
        let main_cf = sess.compile(&main).expect("compile main");
        assert!(
            !main_cf.relocations.is_empty(),
            "caller 应带 @N 函数重定位（BL 的 imm26 由夹具回填）"
        );
        let r = exec_aarch64_module(&[(0, add_cf), (1, main_cf)], &[], 1, &[])
            .unwrap_or_else(|e| panic!("exec_aarch64_module: {e}"));
        assert_eq!(r & 0xFF, 42, "add(20,22) 应返回 42；got {r:#x}");
    }

    /// **P3 执行级：带栈实参的 `Call`（v27 M6-97）**。AAPCS64 前 8 个整型实参走
    /// x0..x7，第 9 个起走调用者栈；`main = sum9(1..9) = 45`。这条同时验证
    /// 调用点栈实参区与 `@move_args` 的栈形参收参（两侧独立布局）。
    #[test]
    fn qemu_aarch64_exec_call_with_stack_args() {
        if qemu_aarch64_path().is_none() {
            eprintln!("SKIP: qemu-system-aarch64 未安装");
            return;
        }
        use code_forge::backend::CompileSession;
        use code_forge::backend::arm64::TargetMachine;
        use code_forge::prelude::{
            FuncRef, FunctionBuilder, FunctionSignature, TypeContext, TypeId,
        };
        let names = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
        let params: Vec<(TypeId, &str)> = names.iter().map(|n| (TypeId::I64, *n)).collect();
        let mut b = FunctionBuilder::new(
            "sum9",
            TypeContext::new(),
            FunctionSignature::new(&params, &[TypeId::I64]),
        );
        let (e, ps) = b.create_block_with_params(&params);
        b.switch_to_block(e);
        let mut acc = ps[0];
        for p in &ps[1..] {
            acc = b.iadd(acc, *p);
        }
        b.ret(&[acc]);
        let sum9 = b.finish().expect("sum9");
        // main() = sum9(1..=9) = 45
        let mut b = FunctionBuilder::new(
            "main",
            TypeContext::new(),
            FunctionSignature::new(&[], &[TypeId::I64]),
        );
        b.create_block_here();
        let args: Vec<_> = (1..=9).map(|v| b.constant(v as i64)).collect();
        let r = b.call(FuncRef::new(0), &args, &[TypeId::I64])[0];
        b.ret(&[r]);
        let main = b.finish().expect("main");

        let sess = CompileSession::new(TargetMachine::new());
        let sum9_cf = sess.compile(&sum9).expect("compile sum9");
        let main_cf = sess.compile(&main).expect("compile main");
        let r = exec_aarch64_module(&[(0, sum9_cf), (1, main_cf)], &[], 1, &[])
            .unwrap_or_else(|e| panic!("exec_aarch64_module: {e}"));
        assert_eq!(r & 0xFF, 45, "sum9(1..9) 应返回 45；got {r:#x}");
    }
}
