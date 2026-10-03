//! 执行档：把**真语法汇编小程序**汇编成字节、装进 [`CompiledFunction`]，交给本 crate
//! 现有的执行器**真跑**（x86 原生 / riscv64 与 aarch64 走 QEMU system+semihosting）。
//!
//! 小程序约定（各机 ABI 的**叶子函数**，不碰栈）：
//!
//! | ISA | 参数 | 返回值 | 结束 |
//! | --- | --- | --- | --- |
//! | x86 | `RCX`/`RDX`/`R8`/`R9`（Windows x64；仅在 windows-x86_64 宿主可跑） | `RAX` | `ret` |
//! | riscv64 | `a0`…（LP64D） | `a0` | `ret` |
//! | aarch64 | `x0`…（AAPCS64） | `x0` | `ret` |
//!
//! `asm/exec/<isa>/<名>.s` 是源码；同名 `.expect` 给期望：`args = 40 2` / `ret = 42`。
//!
//! [`CompiledFunction`]: code_forge::backend::CompiledFunction

use std::path::Path;

use code_forge::backend::CompiledFunction;

use crate::exec::executor::ExecArch;

/// 一条汇编小程序。
#[derive(Debug, Clone)]
pub struct AsmProgram {
    /// 名字（文件名去扩展名）。
    pub name: String,
    /// 源码（逐字）。
    pub src: String,
    /// 实参（u64；按该机 ABI 进参数寄存器）。
    pub args: Vec<u64>,
    /// 期望返回值。
    pub ret: u64,
}

/// 读一套执行档小程序（`asm/exec/<isa>/`）。目录不存在 ⇒ 空（该档 skip）。
pub fn load_programs(isa: &str) -> Result<Vec<AsmProgram>, String> {
    let dir = super::corpus_root().join("exec").join(isa);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("读 {} 失败：{e}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("s"))
        .collect();
    entries.sort();
    for p in entries {
        let src =
            std::fs::read_to_string(&p).map_err(|e| format!("读 {} 失败：{e}", p.display()))?;
        let name = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let expect = p.with_extension("expect");
        let (args, ret) = parse_expect(&expect)?;
        out.push(AsmProgram {
            name,
            src,
            args,
            ret,
        });
    }
    Ok(out)
}

/// 解析 `.expect`：`args = 40 2` / `ret = 42`。
///
/// `#` 开头的整行是注释（**整行**，不是行内——注释里写 `=` 不该被当成键值）。
fn parse_expect(path: &Path) -> Result<(Vec<u64>, u64), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读期望文件 {} 失败：{e}", path.display()))?;
    let mut args = Vec::new();
    let mut ret = 0u64;
    let mut saw_ret = false;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else {
            return Err(format!("{}: `{l}` 不是 `键 = 值`", path.display()));
        };
        match k.trim() {
            "args" => {
                args = v
                    .split_whitespace()
                    .map(|n| {
                        n.parse::<u64>()
                            .map_err(|e| format!("args `{n}` 解析失败：{e}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
            "ret" => {
                ret = v
                    .trim()
                    .parse::<u64>()
                    .map_err(|e| format!("ret `{}` 解析失败：{e}", v.trim()))?;
                saw_ret = true;
            }
            other => return Err(format!("{}: 未知键 `{other}`", path.display())),
        }
    }
    if !saw_ret {
        return Err(format!("{}: 缺 `ret = …`", path.display()));
    }
    Ok((args, ret))
}

/// 汇编 → [`CompiledFunction`]（无重定位：小程序自包含）。
pub fn compile_program(isa: &str, p: &AsmProgram) -> Result<CompiledFunction, String> {
    let code = super::targets::assemble_to_bytes(isa, &p.src)?;
    let n = code.len();
    Ok(CompiledFunction {
        code,
        relocations: Vec::new(),
        code_size: n,
        line_entries: Vec::new(),
        cfi: None,
    })
}

/// ISA 名 → 执行架构（跑真执行要用）。
pub fn run_arch_for(isa: &str) -> Result<ExecArch, String> {
    match isa {
        "x86" => Ok(ExecArch::X86_64),
        "riscv64" => Ok(ExecArch::Riscv64),
        "aarch64" => Ok(ExecArch::AArch64),
        other => Err(format!("未知 ISA `{other}`")),
    }
}

/// 真跑一条小程序，返回执行器的返回值。
pub fn run_program(isa: &str, p: &AsmProgram) -> Result<u64, String> {
    let arch = run_arch_for(isa)?;
    let cf = compile_program(isa, p)?;
    Ok(crate::exec::executor::run(arch, &cf, &p.args))
}

/// 该 ISA 现在能不能真跑（缺 QEMU / 非 Windows-x64 宿主时给出**原因**）。
pub fn unavailable_reason(isa: &str) -> Option<String> {
    match isa {
        "x86" => {
            if !(cfg!(windows) && cfg!(target_arch = "x86_64")) {
                return Some(
                    "x86 原生执行只支持 windows-x86_64 宿主（代码按 Windows x64 ABI 生成）".into(),
                );
            }
            None
        }
        "riscv64" => crate::exec::qemu::qemu_riscv64_path()
            .is_none()
            .then(|| "未找到 qemu-system-riscv64（`$env:QEMU_RISCV64` 或默认安装路径）".into()),
        "aarch64" => crate::exec::qemu_aarch64::qemu_aarch64_path()
            .is_none()
            .then(|| "未找到 qemu-system-aarch64（`$env:QEMU_AARCH64` 或默认安装路径）".into()),
        other => Some(format!("未知 ISA `{other}`")),
    }
}
