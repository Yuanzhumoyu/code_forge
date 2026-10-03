//! 三架构的汇编器适配（[`AsmTarget`]）与执行适配。
//!
//! 全部走**生成物**的公开面：`Assembler::parse_insts`（[`TargetAssembler`]）、
//! `assemble`、`encode`、`could_be_instruction`（线性扫描探针）、`Inst`。
//! 测试侧不重解析 TOML、不另立助记符清单——"本 ISA 认不认这条指令"由**扫描器自己**回答。
//!
//! [`TargetAssembler`]: code_forge::backend::machine::assembler::TargetAssembler

use code_forge::backend::machine::assembler::TargetAssembler;

use super::classify::{AsmTarget, ParseErr};

/// 把生成物的 `AsmError` 折成 [`ParseErr`]（只有"标签没定义"要单独认）。
fn map_err(e: code_forge::backend::machine::assembler::AsmError) -> ParseErr {
    use code_forge::backend::machine::assembler::AsmError;
    match e {
        AsmError::UndefinedLabel(_) => ParseErr::UndefinedLabel,
        other => ParseErr::Other(other.to_string()),
    }
}

macro_rules! asm_target {
    ($ty:ident, $isa:literal, $module:ident) => {
        /// 该 ISA 的汇编器适配。
        pub struct $ty;

        impl AsmTarget for $ty {
            fn isa(&self) -> &'static str {
                $isa
            }

            fn parse(&self, src: &str) -> Result<usize, ParseErr> {
                code_forge::backend::$module::Assembler
                    .parse_insts(src)
                    .map(|v| v.len())
                    .map_err(map_err)
            }

            fn could_be_instruction(&self, src: &str) -> bool {
                code_forge::backend::$module::could_be_instruction(src)
            }
        }
    };
}

asm_target!(X86, "x86", x86);
asm_target!(Riscv64, "riscv64", riscv64);
asm_target!(Aarch64, "aarch64", arm64);

/// 按 ISA 名取适配器。
pub fn target_for(isa: &str) -> Option<&'static dyn AsmTarget> {
    match isa {
        "x86" => Some(&X86),
        "riscv64" => Some(&Riscv64),
        "aarch64" => Some(&Aarch64),
        _ => None,
    }
}

/// 汇编一段源码 → 机器码字节（parse + 逐条 encode 拼接）。
pub fn assemble_to_bytes(isa: &str, src: &str) -> Result<Vec<u8>, String> {
    macro_rules! bytes {
        ($module:ident) => {{
            let insts = code_forge::backend::$module::Assembler
                .parse_insts(src)
                .map_err(|e| format!("parse: {e}"))?;
            let mut out = Vec::new();
            for i in insts {
                let b = code_forge::backend::$module::encode(&i)
                    .map_err(|e| format!("encode {i:?}: {e}"))?;
                out.extend_from_slice(&b);
            }
            Ok(out)
        }};
    }
    match isa {
        "x86" => bytes!(x86),
        "riscv64" => bytes!(riscv64),
        "aarch64" => bytes!(arm64),
        other => Err(format!("未知 ISA `{other}`")),
    }
}

/// 解一串字节再反汇编（**等价性判定**用：两串字节若解出同一条指令、渲染同一文本，
/// 就是同一指令的两种合法编码，例如 x86 `xor rax, 12` 的 imm32 形式与符号扩展 imm8 形式）。
///
/// 要求 `decode` 把 `bytes` **吃满**（尾部有剩 ⇒ 不是同一条编码）。
pub fn disassemble_bytes(isa: &str, bytes: &[u8]) -> Result<String, String> {
    macro_rules! decompile {
        ($module:ident) => {{
            let (inst, n) = code_forge::backend::$module::decode(bytes)
                .ok_or_else(|| format!("decode 失败（{} 字节）", bytes.len()))?;
            if n != bytes.len() {
                return Err(format!("decode 只吃掉 {n}/{} 字节", bytes.len()));
            }
            Ok(code_forge::backend::$module::disassemble(&inst))
        }};
    }
    match isa {
        "x86" => decompile!(x86),
        "riscv64" => decompile!(riscv64),
        "aarch64" => decompile!(arm64),
        other => Err(format!("未知 ISA `{other}`")),
    }
}
