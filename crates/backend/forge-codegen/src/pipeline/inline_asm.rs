//! **内联汇编的降级期展开**（v27 M2.5/S1）。
//!
//! `InlineAsm` 的文本在编译期完全确定（S1 只放行**无占位符的纯文本**），所以展开就在
//! 降级期做：把文本交给本 ISA 的汇编器（`CompileState::asm_parse`，由
//! `machine.assembler()` 传入），得到 `Vec<I>` 后连同与 vcode 平行的
//! `inst_ops`/`inst_clobbers` 推进当前虚拟块。
//!
//! 为什么单独成文件：`pipeline/lowering.rs` 有**规模棘轮**（上限 835 行），这段分支
//! ~40 行；按仓库纪律是**切文件**而不是抬上限。
//!
//! 文本怎么解析：`CompileState::new` 已把 `TypeStore` 快照放进 `ctx.type_store`
//! （v3 S3 的读路径优化），`Immediate::String` 经它取出 `&str`——**不需要**额外的
//! 字符串表通道（这是施工图里被实测推翻的一半）。

use crate::IrError;
use crate::machine::inst::MachineInst;
use crate::pipeline::compiler::CompileState;
use forge_ir::*;

impl<I: MachineInst + 'static> CompileState<I> {
    /// 若该指令是 `InlineAsm` ⇒ 展开并返回 `true`（调用方据此 `continue`）；
    /// 不是则返回 `false`。**不静默**：缺文本立即数 / 无汇编器 / 文本解析失败
    /// 都是明确的 `Unsupported`。
    pub(crate) fn expand_inline_asm(&mut self, inst: &Instruction) -> Result<bool, IrError> {
        if inst.opcode != Opcode::InlineAsm {
            return Ok(false);
        }
        let text: String = match inst.immediates.first() {
            Some(Immediate::String(sid)) => self
                .ctx
                .type_store
                .as_ref()
                .map(|s| s.lookup_str(*sid).to_string())
                .ok_or_else(|| {
                    IrError::Unsupported(
                        "InlineAsm：本次编译没有类型表快照，取不到汇编文本".to_string(),
                    )
                })?,
            _ => {
                return Err(IrError::Unsupported(
                    "InlineAsm 需要恰好一个 String 立即数（汇编文本）".to_string(),
                ));
            }
        };
        let parse = self.asm_parse.clone().ok_or_else(|| {
            IrError::Unsupported("本 ISA 未提供汇编器：内联汇编无法展开".to_string())
        })?;
        let insts = parse(&text).ok_or_else(|| {
            IrError::Unsupported(format!("内联汇编文本无法被本 ISA 的汇编器解析：{text:?}"))
        })?;
        let line = inst.loc.as_ref().and_then(|l| l.line);
        for mi in insts {
            self.inst_ops.push(Default::default());
            self.inst_clobbers.push(Vec::new());
            self.vcode.push_inst_line(mi, line);
        }
        Ok(true)
    }
}
