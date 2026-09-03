//! QEMU riscv64 执行器（B2）。
//!
//! 本机 Windows 无 user-mode `qemu-riscv64`，只有 system 模式
//! `qemu-system-riscv64.exe`。验证链路（实测校准过）：
//!
//! 1. 把编译产物（riscv64 机器码）打包成裸机 ELF：
//!    - 入口 0x8000_0000（QEMU virt 机器 RAM 基址；RAM 128MB）
//!    - crt0 壳：`li sp, 0x8001_0000`（固定栈顶）；参数按序 `li a0..a3`；
//!      `jal x1, main`（跳机器码起始，偏移恒 4）；返回后 sifive_test
//!      退出：`li a6, 0x100000`；`li a5, 0x5555 | (ret<<16)`；`sw a5, 0(a6)`
//!      ——QEMU virt 的 sifive_test 设备读到 magic 0x5555 即以
//!      `(value >> 16) & 0xFF` 为退出码终止（实测：写 42<<16 退出码=42）。
//!    - 机器码紧随 crt0；尾部预留栈区。
//! 2. 子进程运行：
//!    `qemu-system-riscv64 -M virt -bios none -kernel app.elf
//!     -display none -monitor none -serial none`
//! 3. 进程退出码 = 编译函数返回值（**16 位有符号**：sifive_test 的
//!    `(value >> 16) & 0xFFFF`；Windows 保留 16 位退出码，实测 a0=-1 →
//!    65535、a0=-42 → 65494——负值经 `sign_extend_exit` 符号扩展回 i64）。
//!
//! QEMU 路径探测：`$env:QEMU_RISCV64` → 默认安装路径 → PATH。找不到时
//! 调用方（runner）降级 CompileOnly（不失败）。

use code_forge::backend::CompiledFunction;
use std::path::PathBuf;
use std::process::Command;

/// QEMU system-mode riscv64 模拟器路径（None = 未找到）。
pub fn qemu_riscv64_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("QEMU_RISCV64") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let candidates = [
        r"D:\Program Files\qemu\qemu-system-riscv64.exe",
        r"C:\Program Files\qemu\qemu-system-riscv64.exe",
        r"C:\Program Files (x86)\qemu\qemu-system-riscv64.exe",
        "/usr/bin/qemu-system-riscv64",
        "/usr/local/bin/qemu-system-riscv64",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.is_file() {
            return Some(p);
        }
    }
    // PATH 搜索
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let p = dir.join(if cfg!(windows) {
                "qemu-system-riscv64.exe"
            } else {
                "qemu-system-riscv64"
            });
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────
// RISC-V 指令编码 helper（RV64I 子集）
// ─────────────────────────────────────────────────────────────

/// addi rd, rs1, imm12（opcode 0x13, funct3=0）
fn inst_addi(rd: u32, rs1: u32, imm12: i64) -> [u8; 4] {
    let w = 0x13u32 | (rd & 0x1F) << 7 | (rs1 & 0x1F) << 15 | ((imm12 as u32 & 0xFFF) << 20);
    w.to_le_bytes()
}
/// lui rd, imm20（opcode 0x37）
fn inst_lui(rd: u32, imm20: u64) -> [u8; 4] {
    let w = 0x37u32 | (rd & 0x1F) << 7 | ((imm20 & 0xFFFFF) as u32) << 12;
    w.to_le_bytes()
}
/// slli rd, rs1, shamt（opcode 0x13, funct3=1）
fn inst_slli(rd: u32, rs1: u32, shamt: u32) -> [u8; 4] {
    let w = 0x13u32 | (rd & 0x1F) << 7 | (rs1 & 0x1F) << 15 | (shamt & 0x3F) << 20 | (1u32 << 12);
    w.to_le_bytes()
}
/// srli rd, rs1, shamt（opcode 0x13, funct3=5）
fn inst_srli(rd: u32, rs1: u32, shamt: u32) -> [u8; 4] {
    let w = 0x13u32 | (rd & 0x1F) << 7 | (rs1 & 0x1F) << 15 | (shamt & 0x3F) << 20 | (5u32 << 12);
    w.to_le_bytes()
}
/// or rd, rs1, rs2（opcode 0x33, funct3=6）
fn inst_or(rd: u32, rs1: u32, rs2: u32) -> [u8; 4] {
    let w = 0x33u32 | (rd & 0x1F) << 7 | (rs1 & 0x1F) << 15 | (rs2 & 0x1F) << 20 | (6u32 << 12);
    w.to_le_bytes()
}
/// jal rd, imm20（opcode 0x6F）
fn inst_jal(rd: u32, imm: i64) -> [u8; 4] {
    let v = imm as u32 & 0x1F_FFFF;
    let enc =
        (v >> 20 & 1) << 31 | (v >> 1 & 0x3FF) << 21 | (v >> 11 & 1) << 20 | (v >> 12 & 0xFF) << 12;
    let w = 0x6Fu32 | (rd & 0x1F) << 7 | enc;
    w.to_le_bytes()
}

/// `lui`+`addi` 的位段拆分：(hi20, lo12)。`addi` 加的是符号扩展 12 位，
/// 故 hi20 需 `+0x800` 预补偿。
fn li32_parts(v: i32) -> (u64, i64) {
    let v = v as i64;
    let hi = (((v + 0x800) >> 12) & 0xFFFFF) as u64;
    let lo = v << 52 >> 52; // 低 12 位符号扩展（-2048..=2047）
    (hi, lo)
}

/// `lui hi20; addi lo12` 在 RV64 上实际装入的 64 位值（`lui` 符号扩展）。
/// 不是所有 i32 都可达：`+0x800` 预补偿在 `0x7FFF_F800..=0x7FFF_FFFF`
/// 溢出到 hi20=0x80000（负），结果比目标少 2^32——这些值走分段路径。
fn li32_eval(hi: u64, lo: i64) -> i64 {
    ((((hi as u32) << 12) as i32) as i64).wrapping_add(lo)
}

/// 32 位有符号立即数 → `lui` + `addi`。RV64 上结果符号扩展到 64 位；
/// 调用方需保证 `li32_eval` 精确，或只依赖低 32 位（见 `inst_li64` 分段路径）。
fn inst_li32(rd: u32, v: i32) -> Vec<u8> {
    let (hi, lo) = li32_parts(v);
    let mut out = Vec::new();
    out.extend(inst_lui(rd, hi));
    out.extend(inst_addi(rd, rd, lo));
    out
}

/// 64 位立即数 → 指令序列（精确，无静默截断）。
///
/// - 小值（±2047）→ 单 `addi`；
/// - `lui`+`addi` 精确可达 → 两条；
/// - 其余 → 高 32 位左移 32（`slli` 丢弃 `lui` 的符号扩展位）+ 低 32 位
///   零扩展（`slli`/`srli` 对）后 `or` 合并，scratch 用 t0(x5)。分段路径
///   只依赖各半的低 32 位，故不受 `li32_eval` 的不可达窗口影响。
///
/// 早期版本只有前两个分支且无精确性检查，>32 位的值被静默算成 0（`lui` 的
/// hi20 被 `& 0xFFFFF` 截掉）——矩阵用例传 `0x2_0000_0000` 时实际收到 0，
/// 断言拿到的是"比较 0 与 0"的结果而非被测语义（混宽 icmp 用例实证）。
fn inst_li64(rd: u32, v: i64) -> Vec<u8> {
    let mut out = Vec::new();
    if (-2048..2048).contains(&v) {
        out.extend(inst_addi(rd, 0, v));
        return out;
    }
    let (hi20, lo12) = li32_parts(v as i32);
    if v == (v as i32) as i64 && li32_eval(hi20, lo12) == v {
        out.extend(inst_li32(rd, v as i32));
        return out;
    }
    const SCRATCH: u32 = 5; // t0——非参数寄存器，crt0 中无后续依赖
    debug_assert_ne!(rd, SCRATCH, "li64 目标不能是 scratch t0");
    out.extend(inst_li32(rd, (v >> 32) as i32));
    out.extend(inst_slli(rd, rd, 32));
    out.extend(inst_li32(SCRATCH, v as i32));
    out.extend(inst_slli(SCRATCH, SCRATCH, 32));
    out.extend(inst_srli(SCRATCH, SCRATCH, 32));
    out.extend(inst_or(rd, rd, SCRATCH));
    out
}

// ─────────────────────────────────────────────────────────────
// 裸机 ELF 打包（手写最小 ELF64：header + 1 个 PT_LOAD）
// ─────────────────────────────────────────────────────────────

/// 常量：入口 / 栈顶 / 栈区字节数。
const ENTRY: u64 = 0x8000_0000;
const STACK_TOP: u64 = 0x8001_0000; // RAM 128MB 内固定栈顶
const STACK_BYTES: usize = 0x4000; // 16KB 栈区（image 尾部）

/// crt0：li sp + 参数 li a0..a3 + jal x1, main + exit_seq（sifive_test）。
/// `main_off` = main 相对镜像起点的字节偏移（jal 目标；单函数 = crt0 末尾，
/// 模块版 = 函数布局偏移）。返回 crt0 字节。
fn gen_crt0(args: &[u64], main_off: usize) -> Vec<u8> {
    let mut crt0: Vec<u8> = Vec::new();
    // li sp, STACK_TOP（0x80010000 = 0x80010 << 12）：lui 符号扩展后
    // slli 32 把低 32 位移到高 32 位，srli 32 清零高 32 位 → sp = 0x80010000
    crt0.extend(inst_lui(2, (STACK_TOP >> 12) & 0xFFFFF));
    crt0.extend(inst_slli(2, 2, 32));
    crt0.extend(inst_srli(2, 2, 32));
    // mstatus.FS = Dirty（bits 14:13 = 11）：QEMU 复位 FS=Off → 浮点指令
    // 非法（fadd/feq/fmv/fcvt 等）→ 挂起。crt0 显式置位：
    //   lui t0, 6（0x6000 >> 12 = 6）；csrrs x0, mstatus(0x300), t0
    // 编码：csr 0x300<<20 | rs1 t0(5)<<15 | funct3 010<<12 | rd x0<<7 | 0x73
    //   = 0x3002_A073（注意 funct3 必须是 010=CSRRS；0x3002_F073 是 CSRRC
    //   清位，FS 设不上 → 浮点仍非法 → QEMU 挂起——曾用该错值排查数小时）
    crt0.extend(inst_lui(5, 6));
    crt0.extend(0x3002_A073u32.to_le_bytes()); // csrrs x0, mstatus, t0(x5)
    // 参数 li a0..a3（最多 4 个；inst_li64 精确装载任意 64 位值）
    for (i, &arg) in args.iter().take(4).enumerate() {
        crt0.extend(inst_li64(10 + i as u32, arg as i64));
    }
    // sifive_test 退出序列（在 jal 之后；函数体返回后经此退出）。
    // QEMU 11.0.92 的 TCG 在 **jal/ret 返回路径后的 MMIO 写可能丢失**
    //（实测：无函数顺序执行时单 sw 触发；经 jal 调用函数返回后首次 sw 不
    // 触发、PC 继续）。绕过：exit_seq 用 `sw a5, 0(a6); beq x0,x0,0` 重试
    // 循环——反复写 sifive_test 直到 QEMU 以退出码终止（实测稳定触发）。
    let mut exit_seq: Vec<u8> = Vec::new();
    exit_seq.extend(inst_li64(16, 0x100000)); // a6 = 0x100000（TEST 寄存器）
    exit_seq.extend(inst_li64(15, 0x5555)); // a5 = 0x5555（magic）
    exit_seq.extend(inst_slli(10, 10, 16)); // a0 = ret << 16
    exit_seq.extend(inst_or(15, 15, 10)); // a5 = 0x5555 | (ret<<16)
    // sw a5, 0(a6)：opcode 0x23, funct3=2, rs1=16(a6)<<15, rs2=15(a5)<<20
    let sw = 0x23u32 | (15u32 << 20) | (2u32 << 12) | (16u32 << 15);
    exit_seq.extend(sw.to_le_bytes());
    // 重试循环：beq x0, x0, 0（自跳；sw 未触发时反复写）
    exit_seq.extend(0x63u32.to_le_bytes());
    // jal x1, main —— jal 的 imm = main 绝对偏移 - jal 自身绝对位置
    let jal_abs = crt0.len();
    let jal_imm = main_off as i64 - jal_abs as i64;
    crt0.extend(inst_jal(1, jal_imm));
    crt0.extend(&exit_seq);
    crt0
}

/// 打包最小 ELF64（header + 1 个 PT_LOAD），镜像 = crt0 + code + 栈。
/// 单函数：main = crt0 末尾（code 起始）。
fn build_riscv_elf(code: &[u8], args: &[u64]) -> Vec<u8> {
    // main_off = crt0 最终长度（exit_seq 后）。两遍：先用占位 main_off=0
    // 生成，取其长度作为真实 main_off（args 一致 → 长度一致），再重新生成。
    let placeholder = gen_crt0(args, 0);
    let main_off = placeholder.len();
    let crt0 = gen_crt0(args, main_off);
    let mut image = crt0;
    image.extend_from_slice(code);
    image.extend(vec![0u8; STACK_BYTES]);
    wrap_elf(&image)
}

/// 把镜像（crt0+code+栈）包成最小 ELF64。
fn wrap_elf(image: &[u8]) -> Vec<u8> {
    // ── ELF header（64 字节）──
    let mut elf = Vec::new();
    elf.extend_from_slice(&[0x7F, b'E', b'L', b'F']); // magic
    elf.push(2); // ELFCLASS64
    elf.push(1); // ELFDATA2LSB
    elf.push(1); // EV_CURRENT
    elf.push(0); // ELFOSABI_NONE
    elf.push(0); // EI_ABIVERSION
    elf.extend([0u8; 7]); // padding（e_ident 共 16 字节）
    elf.extend(&2u16.to_le_bytes()); // ET_EXEC
    elf.extend(&243u16.to_le_bytes()); // e_machine = EM_RISCV
    elf.extend(&1u32.to_le_bytes()); // e_version
    elf.extend(&ENTRY.to_le_bytes()); // e_entry
    elf.extend(&64u64.to_le_bytes()); // e_phoff
    elf.extend(&0u64.to_le_bytes()); // e_shoff
    elf.extend(&0u32.to_le_bytes()); // e_flags
    elf.extend(&64u16.to_le_bytes()); // e_ehsize
    elf.extend(&56u16.to_le_bytes()); // e_phentsize
    elf.extend(&1u16.to_le_bytes()); // e_phnum
    elf.extend(&0u16.to_le_bytes()); // e_shentsize
    elf.extend(&0u16.to_le_bytes()); // e_shnum
    elf.extend(&0u16.to_le_bytes()); // e_shstrndx

    // ── Program header（56 字节）：PT_LOAD，vaddr=ENTRY、offset=120 ──
    elf.extend(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend(&7u32.to_le_bytes()); // p_flags = PF_R|PF_W|PF_X
    elf.extend(&120u64.to_le_bytes()); // p_offset = 120（ELF 头之后）
    elf.extend(&ENTRY.to_le_bytes()); // p_vaddr
    elf.extend(&ENTRY.to_le_bytes()); // p_paddr
    elf.extend(&(image.len() as u64).to_le_bytes()); // p_filesz
    elf.extend(&(image.len() as u64).to_le_bytes()); // p_memsz
    elf.extend(&4096u64.to_le_bytes()); // p_align

    elf.extend_from_slice(image);
    elf
}

/// 运行 ELF（子进程），返回退出码。超时（默认 15s）防 exit_seq 重试死循环
/// 挂起 CI（sw 永不触发时 beq 自跳死循环）。
fn run_qemu(elf: &[u8], label: &str) -> Result<u64, String> {
    let qemu = qemu_riscv64_path().ok_or_else(|| {
        "QEMU riscv64 未找到（设置 QEMU_RISCV64 或安装 qemu-system-riscv64）".to_string()
    })?;
    // 调试钩子：FORGE_QEMU_DUMP=路径 时保存 ELF 供 qemu -d in_asm 分析
    if let Ok(dump) = std::env::var("FORGE_QEMU_DUMP") {
        let _ = std::fs::write(&dump, elf);
    }
    if let Ok(dp) = std::env::var("FGE_QEMU_DUMP") {
        let dir = std::env::temp_dir();
        static S2: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = S2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = dir.join(format!("fge3_{label}_{}_{}.elf", std::process::id(), n));
        let _ = std::fs::write(&p, elf);
        let _ = dp;
    }
    let dir = std::env::temp_dir();
    // 并行测试线程可能同 pid + 同 label → 文件名冲突互相覆盖；加原子序号。
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!(
        "forge_qemu_{label}_{}_{}.elf",
        std::process::id(),
        seq
    ));
    std::fs::write(&path, elf).map_err(|e| format!("写 ELF: {e}"))?;
    let mut child = Command::new(&qemu)
        .args([
            "-M",
            "virt",
            "-bios",
            "none",
            "-kernel",
            path.to_str().unwrap(),
            "-display",
            "none",
            "-monitor",
            "none",
            "-serial",
            "none",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("qemu 启动: {e}"))?;
    // 超时等待（QEMU 正常应 <1s 退出；死循环 15s 兜底）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| format!("qemu wait: {e}"))? {
            break st;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = std::fs::remove_file(&path);
            return Err(format!("qemu 超时（{label}：exit_seq 重试死循环?）"));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let _ = std::fs::remove_file(&path);
    let exit = status.code().unwrap_or(-1);
    if exit < 0 {
        return Err(format!("qemu 异常退出: {exit}"));
    }
    Ok(exit as u64)
}

/// 运行编译产物（QEMU riscv64）并返回退出码（= 函数返回值低 8 位）。
pub fn exec_riscv64(compiled: &CompiledFunction, args: &[u64]) -> Result<u64, String> {
    let elf = build_riscv_elf(&compiled.code, args);
    run_qemu(&elf, "single")
}

// ─────────────────────────────────────────────────────────────
// 模块执行：多函数打包 + @N 符号重定位（跨函数 Call/递归）
// ─────────────────────────────────────────────────────────────

/// 运行多函数模块（QEMU riscv64）。
///
/// `funcs` 为 (函数名, CompiledFunction) 的 **FuncRef 序**（0..n）；
/// `globals` 为 (名字, init 字节) 序——打包时在镜像尾部（栈前）布局
/// 数据段，符号 "G{id}" 与全局名均指向其地址（GlobalAddr 的 PC-relative
/// reloc 在打包时 patch）。crt0 的 main 经 `main_name` 定位（jal 目标 =
/// 该函数入口）。跨函数 reloc（符号 `@N` = 函数序号）在打包时用
/// `RiscvRelocPatcher` 直接 patch（目标 = ENTRY + 函数布局偏移）。
pub fn exec_riscv64_module(
    funcs: &[(String, CompiledFunction)],
    globals: &[(String, Vec<u8>)],
    main_name: &str,
    args: &[u64],
) -> Result<u64, String> {
    use code_forge::{RelocPatcher, RiscvRelocPatcher};

    // 1. 布局：crt0 在前，函数顺序排列，数据段（globals）随后，末尾栈区。
    let placeholder = gen_crt0(args, 0);
    let crt0_len = placeholder.len();
    let mut layout: Vec<(String, usize)> = Vec::new(); // name -> 镜像内偏移
    let mut off = crt0_len;
    for (name, cf) in funcs {
        layout.push((name.clone(), off));
        off += cf.code.len();
    }
    let main_off = layout
        .iter()
        .find(|(n, _)| n == main_name)
        .map(|(_, o)| *o)
        .ok_or_else(|| format!("main 函数 '{main_name}' 不在模块中"))?;
    // 数据段：每个 global 顺序排布（对齐 8 字节）。符号 = 绝对地址
    // ENTRY + data_off。函数地址 ≤ data_off，auipc/addi PC-relative
    // 距离在 ±2GiB 内（QEMU virt RAM 128MB）→ 单对 auipc/addi 足够。
    // **data_off 必须 8 字节对齐**：AMO（lr/sc/amoadd.d）要求自然对齐，
    // 函数代码长度非 8 倍数时 misaligned_store → 挂起（实测 0xD4）。
    let data_off = (off + 7) & !7;
    let mut data: Vec<u8> = Vec::new();
    let mut global_addrs: Vec<(String, u64)> = Vec::new();
    for (name, init) in globals {
        let a = data.len();
        global_addrs.push((name.clone(), ENTRY + (data_off + a) as u64));
        data.extend_from_slice(init);
        // 对齐 8
        while !data.len().is_multiple_of(8) {
            data.push(0);
        }
    }

    // 2. crt0（jal 目标 = main 布局偏移）+ 所有函数代码 + 数据段
    //    **对齐 padding**：data_off 已 8 对齐，image 里数据段必须从 data_off
    //    开始（否则符号地址错位 → 读 padding 0，实测 globaladdr got 0）。
    let mut image: Vec<u8> = gen_crt0(args, main_off);
    debug_assert_eq!(image.len(), crt0_len);
    for (_, cf) in funcs {
        image.extend_from_slice(&cf.code);
    }
    debug_assert_eq!(image.len(), off);
    image.extend(vec![0u8; data_off - off]);
    debug_assert_eq!(image.len(), data_off);
    image.extend_from_slice(&data);

    // 3. 符号表：@N（FuncRef 序）与函数名 → 绝对地址；G{id} 与全局名 →
    //    数据段地址。
    let symbol_addr = |sym: &str| -> Option<u64> {
        if let Some(n) = sym.strip_prefix('@') {
            let idx: usize = n.parse().ok()?;
            layout.get(idx).map(|(_, o)| ENTRY + *o as u64)
        } else if let Some(n) = sym.strip_prefix('G') {
            // GlobalAddr 的 reloc 符号 "G{id}"（id = 全局变量序号）
            let idx: usize = n.parse().ok()?;
            global_addrs.get(idx).map(|(_, a)| *a)
        } else {
            layout
                .iter()
                .find(|(name, _)| name == sym)
                .map(|(_, o)| ENTRY + *o as u64)
                .or_else(|| {
                    global_addrs
                        .iter()
                        .find(|(name, _)| name == sym)
                        .map(|(_, a)| *a)
                })
        }
    };

    // 4. 重定位 patch（RiscvRelocPatcher 编码 JAL/B 型位段）
    let patcher = RiscvRelocPatcher;
    let mut func_off = crt0_len;
    for (name, cf) in funcs {
        for reloc in &cf.relocations {
            // 函数内 label fixup（symbol 为空）已由 CodeSink.finish 的 patcher
            // 解析进位段——模块打包时跳过，只处理跨函数符号（@N/函数名）。
            if reloc.symbol.is_empty() {
                continue;
            }
            let target = symbol_addr(reloc.symbol.as_str())
                .ok_or_else(|| format!("{name}: 未解析符号 '{}'", reloc.symbol.as_str()))?;
            let site = ENTRY + (func_off + reloc.offset) as u64;
            patcher
                .apply(
                    &mut image,
                    func_off + reloc.offset,
                    reloc.kind,
                    target,
                    site,
                )
                .map_err(|e| format!("{name}: reloc patch: {e}"))?;
        }
        func_off += cf.code.len();
    }

    // 5. 栈区 + 打包 + 运行
    image.extend(vec![0u8; STACK_BYTES]);
    let elf = wrap_elf(&image);
    run_qemu(&elf, "module")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qemu_path_detected_or_none() {
        let _ = qemu_riscv64_path();
    }

    #[test]
    fn elf_header_layout() {
        let cf = CompiledFunction {
            code: vec![0x67, 0x80, 0x00, 0x00], // ret
            code_size: 4,
            relocations: vec![],
            line_entries: vec![],
            cfi: None, // riscv 机器码无 x86 prologue——scan 必 None
        };
        let elf = build_riscv_elf(&cf.code, &[]);
        assert_eq!(&elf[0..4], &[0x7F, b'E', b'L', b'F']);
        assert_eq!(u16::from_le_bytes([elf[18], elf[19]]), 243, "e_machine");
        assert_eq!(u16::from_le_bytes([elf[56], elf[57]]), 1, "e_phnum");
        assert!(elf.len() > 120);
    }

    /// 解释 `inst_li64` 生成的序列（addi/lui/slli/srli/or 子集），返回 rd 的
    /// 最终值。长度断言证明不了装载值正确——这里直接求值比对。
    fn eval_li_seq(seq: &[u8], rd: u32) -> u64 {
        let mut regs = [0u64; 32];
        for c in seq.chunks_exact(4) {
            let w = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            let d = ((w >> 7) & 0x1F) as usize;
            let f3 = (w >> 12) & 7;
            let s1 = ((w >> 15) & 0x1F) as usize;
            let val = match w & 0x7F {
                // lui：imm20 << 12 后符号扩展到 64 位
                0x37 => (w & 0xFFFF_F000) as i32 as i64 as u64,
                0x13 => {
                    let shamt = (w >> 20) & 0x3F;
                    let imm12 = ((w as i32) >> 20) as i64; // 算术右移 → 符号扩展
                    match f3 {
                        0 => regs[s1].wrapping_add(imm12 as u64), // addi
                        1 => regs[s1] << shamt,                   // slli
                        5 => regs[s1] >> shamt,                   // srli
                        _ => panic!("eval_li_seq: 未预期 funct3={f3}"),
                    }
                }
                0x33 => {
                    assert_eq!(f3, 6, "eval_li_seq: 只应出现 OR");
                    regs[s1] | regs[((w >> 20) & 0x1F) as usize]
                }
                op => panic!("eval_li_seq: 未预期 opcode 0x{op:02x}"),
            };
            if d != 0 {
                regs[d] = val;
            }
        }
        regs[rd as usize]
    }

    #[test]
    fn li64_sequences() {
        // 三条路径的指令数：单 addi / lui+addi / 分段（li32×2 + slli×2 + srli + or）
        assert_eq!(inst_li64(10, 42).len(), 4);
        assert_eq!(inst_li64(10, 0x1234_5678).len(), 8);
        assert_eq!(inst_li64(10, 0x1234_5678_9ABC_DEF0).len(), 32);
        // 值精确性：小值 / 32 位边界 / lui+addi 不可达窗口 / 超 32 位 / 负值
        for v in [
            0i64,
            42,
            -42,
            2047,
            -2048,
            4096,
            0x1234_5678,
            -0x1234_5678,
            0x7FFF_F7FF,
            0x7FFF_F800, // lui+addi 不可达窗口下界 → 分段路径
            i32::MAX as i64,
            i32::MIN as i64,
            0x2_0000_0000, // 矩阵混宽用例实参
            0x2_0000_00FF,
            -0x2_0000_0000,
            0x1234_5678_9ABC_DEF0,
            i64::MAX,
            i64::MIN,
        ] {
            assert_eq!(
                eval_li_seq(&inst_li64(10, v), 10),
                v as u64,
                "li64({v:#x}) 装载值错误"
            );
        }
    }

    #[test]
    fn jal_imm4_encodes_correctly() {
        // jal x1, 4：imm=4 → 位段 [20|10:1|11|19:12] 应为 0x0040_00EF 级
        let bytes = inst_jal(1, 4);
        let w = u32::from_le_bytes(bytes);
        assert_eq!(w & 0x7F, 0x6F, "JAL opcode");
        assert_eq!(w, 0x0040_00EF, "jal x1, +4 标准编码");
    }

    #[test]
    fn module_cross_function_call() {
        // 两函数模块：callee(a,b)=a+b；main()=callee(20,22)=42
        // 用 riscv64_v12::TargetMachine 编译（FunctionCompiler::compile_raw）。
        // 注意：当前 riscv 的 Call lowering 尚未接线（阶段 3）——此测试验证
        // exec_riscv64_module 的**打包 + reloc patch** 机制：手工构造两个
        // 无跨函数 reloc 的函数（顺序执行），main 返回常量 42。
        use code_forge::backend::FunctionCompiler;
        use code_forge::prelude::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

        fn compile(name: &str, v: i64) -> (String, CompiledFunction) {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
            b.create_block_here();
            let c = b.iconst_i64(v);
            b.ret(&[c]);
            let func = b.finish().expect("build");
            let cf = FunctionCompiler::new(code_forge::backend::riscv64_v12::TargetMachine::new())
                .compile_raw(&func)
                .expect("compile");
            (name.to_string(), cf)
        }

        let funcs = vec![compile("callee", 99), compile("main", 42)];
        let r = exec_riscv64_module(&funcs, &[], "main", &[]);
        // 无 QEMU → Err（含 "未找到"）；有 QEMU → 42
        match r {
            Ok(code) => assert_eq!(code, 42, "模块执行 main 应返回 42"),
            Err(e) if e.contains("未找到") => {
                eprintln!("[qemu] 未找到——跳过模块执行");
            }
            Err(e) => panic!("模块执行失败: {e}"),
        }
    }

    #[test]
    fn module_main_not_found_error() {
        use code_forge::backend::FunctionCompiler;
        use code_forge::prelude::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};
        let sig = FunctionSignature::new(&[], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("only", TypeContext::new(), sig);
        b.create_block_here();
        let c = b.iconst_i64(1);
        b.ret(&[c]);
        let func = b.finish().expect("build");
        let cf = FunctionCompiler::new(code_forge::backend::riscv64_v12::TargetMachine::new())
            .compile_raw(&func)
            .expect("compile");
        let funcs = vec![("only".to_string(), cf)];
        let err = exec_riscv64_module(&funcs, &[], "missing", &[]).unwrap_err();
        assert!(err.contains("main 函数"), "err: {err}");
    }

    #[test]
    fn module_cross_function_call_real() {
        // 真实跨函数 Call：callee(a,b)=a+b；main()=callee(20,22)=42。
        // 覆盖：Call lowering（jal ra, @N）→ reloc patch（RiscvRelocPatcher）
        // → QEMU 模块执行。
        use code_forge::backend::FunctionCompiler;
        use code_forge::prelude::{
            FuncRef, FunctionBuilder, FunctionSignature, Module, TypeContext, TypeId,
        };

        let mut module = Module::new();
        let sig_c =
            FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
        let mut bc = FunctionBuilder::new("callee", TypeContext::new(), sig_c);
        let (blk, p) = bc.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        bc.switch_to_block(blk);
        let s = bc.iadd(p[0], p[1]);
        bc.ret(&[s]);
        let _callee_ref = module.add_function(bc.finish().expect("callee"));

        let sig_m = FunctionSignature::new(&[], &[TypeId::I64]);
        let mut bm = FunctionBuilder::new("main", TypeContext::new(), sig_m);
        bm.create_block_here();
        let a = bm.iconst_i64(20);
        let b2 = bm.iconst_i64(22);
        let r = bm.call(_callee_ref, &[a, b2], &[TypeId::I64]);
        bm.ret(&r);
        let _main_ref = module.add_function(bm.finish().expect("main"));

        let funcs: Vec<(String, CompiledFunction)> = (0..module.function_count())
            .map(|i| {
                let fr = FuncRef(i as u32);
                let func = module.get_function(fr);
                let cf =
                    FunctionCompiler::new(code_forge::backend::riscv64_v12::TargetMachine::new())
                        .compile_raw(func)
                        .expect("compile fn");
                (func.name.as_str().to_string(), cf)
            })
            .collect();

        let r = exec_riscv64_module(&funcs, &[], "main", &[]);
        match r {
            Ok(code) => assert_eq!(code, 42, "跨函数 Call 模块应返回 42"),
            Err(e) if e.contains("未找到") => {
                eprintln!("[qemu] 未找到——跳过跨函数 Call 执行");
            }
            Err(e) => panic!("跨函数 Call 模块失败: {e}"),
        }
    }
}
// touch-qemu
// touch-qemu2
