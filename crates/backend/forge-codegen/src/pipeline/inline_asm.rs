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
        // S2a：immediates = [约束, 文本]；S1：immediates = [文本]。
        let lookup = |sid: forge_ir::InternedStr| -> Result<String, IrError> {
            self.ctx
                .type_store
                .as_ref()
                .map(|s| s.lookup_str(sid).to_string())
                .ok_or_else(|| {
                    IrError::Unsupported(
                        "InlineAsm：本次编译没有类型表快照，取不到汇编文本".to_string(),
                    )
                })
        };
        let (constraints, text) = match (inst.immediates.first(), inst.immediates.get(1)) {
            (Some(Immediate::String(a)), Some(Immediate::String(b))) => (lookup(*a)?, lookup(*b)?),
            (Some(Immediate::String(a)), None) => (String::new(), lookup(*a)?),
            _ => {
                return Err(IrError::Unsupported(
                    "InlineAsm 需要 1~2 个 String 立即数（约束、汇编文本）".to_string(),
                ));
            }
        };
        let class = self.ctx.value_gpr_class;
        if !constraints.is_empty() {
            return self
                .expand_inline_asm_s2a(inst, &constraints, &text, class)
                .map(|()| true);
        }

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

    /// **S2a**：带寄存器约束的内联汇编（`in`/`out`/`inout` + `reg` 类）。
    ///
    /// 机制（不需要新的分配器能力）：挑中的物理寄存器由我们**写死**在搬运指令的一侧，
    /// 另一侧绑 IR 值对应的 vreg（分配器回填），并把挑中的寄存器写进该指令的
    /// `inst_clobbers`——于是分配器知道"此处这些寄存器被破坏"，不会把别的值放那儿。
    fn expand_inline_asm_s2a(
        &mut self,
        inst: &Instruction,
        constraints: &str,
        text: &str,
        class: RegClass,
    ) -> Result<(), IrError> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Role {
            In,
            Out,
            InOut,
        }
        // ① 解析约束（`角色,类;…`）
        let mut specs: Vec<Role> = Vec::new();
        for part in constraints.split(';').filter(|s| !s.is_empty()) {
            let mut it = part.split(',');
            let role = match it.next() {
                Some("i") => Role::In,
                Some("o") => Role::Out,
                Some("io") => Role::InOut,
                other => {
                    return Err(IrError::Unsupported(format!(
                        "内联 asm 的约束角色不支持：{other:?}（S2a 只有 i/o/io）"
                    )));
                }
            };
            match it.next() {
                Some("g") => {}
                other => {
                    return Err(IrError::Unsupported(format!(
                        "内联 asm 的寄存器类不支持：{other:?}（S2a 只有 g；显式寄存器名见 S2b）"
                    )));
                }
            }
            specs.push(role);
        }
        // 输入走 operands、输出走 results：先按约束算各自应有几个。
        let need_in = specs
            .iter()
            .filter(|r| matches!(r, Role::In | Role::InOut))
            .count();
        let need_out = specs
            .iter()
            .filter(|r| matches!(r, Role::Out | Role::InOut))
            .count();
        if need_in != inst.operands.len() || need_out != inst.results.len() {
            return Err(IrError::Unsupported(format!(
                "内联 asm 的约束与实参不符：约束要 {need_in} 个输入 / {need_out} 个输出，\
                 实到 {} 个操作数 / {} 个结果",
                inst.operands.len(),
                inst.results.len()
            )));
        }
        // ② 挑寄存器（可分配 GPR 顺序，互不重复）
        let mut chosen: Vec<u32> = Vec::with_capacity(specs.len());
        for _ in 0..specs.len() {
            let reg = *self
                .gp_alloc_order
                .iter()
                .find(|r| !chosen.contains(r))
                .ok_or_else(|| {
                    IrError::Unsupported("内联 asm 的寄存器需求超过可分配 GPR 数量".to_string())
                })?;
            chosen.push(reg);
        }
        // ③ 名字 → 文本替换 → 汇编
        let name_of = self.reg_name_of.clone().ok_or_else(|| {
            IrError::Unsupported("本 ISA 未提供寄存器名表（S2a 无法替换占位符）".to_string())
        })?;
        let mut names: Vec<String> = Vec::with_capacity(chosen.len());
        for r in &chosen {
            let n = name_of(*r, class)
                .ok_or_else(|| IrError::Unsupported(format!("寄存器 {r} 没有汇编拼写（S2a）")))?;
            names.push(n.to_string());
        }
        let asm_text = substitute_placeholders(text, &names)?;
        let parse = self.asm_parse.clone().ok_or_else(|| {
            IrError::Unsupported("本 ISA 未提供汇编器：内联汇编无法展开".to_string())
        })?;
        let asm_insts = parse(&asm_text).ok_or_else(|| {
            IrError::Unsupported(format!("内联汇编文本无法被汇编器解析：{asm_text:?}"))
        })?;
        let line = inst.loc.as_ref().and_then(|l| l.line);
        let clobbers: Vec<(u32, RegClass)> = chosen.iter().map(|r| (*r, class)).collect();

        // ④ in/inout：搬入（dst 侧写死 = chosen[i]，src 侧绑 vreg）
        let mut next_in = 0usize;
        for (i, role) in specs.iter().enumerate() {
            if matches!(role, Role::In | Role::InOut) {
                let vreg = self.vreg_of(inst.operands[next_in])?;
                next_in += 1;
                let mov = self.make_mov_reg(parse.as_ref(), name_of.as_ref(), chosen[i], class)?;
                self.inst_ops
                    .push(smallvec::smallvec![crate::machine::lowering::OperandBind {
                        vreg,
                        field: 1,
                        role: crate::machine::inst::OperandRole::Use,
                        class: Some(class),
                    }]);
                self.inst_clobbers.push(clobbers.clone());
                self.vcode.push_inst_line(mov, line);
            }
        }
        // ⑤ asm 本体（指令里的寄存器是字面量 ⇒ 不需要绑定；但要带 clobbers）
        for mi in asm_insts {
            self.inst_ops.push(Default::default());
            self.inst_clobbers.push(clobbers.clone());
            self.vcode.push_inst_line(mi, line);
        }
        // ⑥ out/inout：搬出（dst 侧绑 vreg，src 侧写死 = chosen[i]）
        let mut next_out = 0usize;
        for (i, role) in specs.iter().enumerate() {
            if matches!(role, Role::Out | Role::InOut) {
                let vreg = self.vreg_of(inst.results[next_out])?;
                next_out += 1;
                let mov = self.make_mov_reg(parse.as_ref(), name_of.as_ref(), chosen[i], class)?;
                self.inst_ops
                    .push(smallvec::smallvec![crate::machine::lowering::OperandBind {
                        vreg,
                        field: 0,
                        role: crate::machine::inst::OperandRole::Def,
                        class: Some(class),
                    }]);
                self.inst_clobbers.push(clobbers.clone());
                self.vcode.push_inst_line(mov, line);
            }
        }
        Ok(())
    }

    /// IR 值 → 已分配的 vreg（asm 的操作数必须已经拿到寄存器）。
    fn vreg_of(&self, v: Value) -> Result<XReg, IrError> {
        self.value_to_xreg.get(v).copied().ok_or_else(|| {
            IrError::Unsupported(format!(
                "内联 asm 的操作数 {v:?} 没有分配寄存器（值未落寄存器？）"
            ))
        })
    }

    /// 造一条 `mov preg, preg`（两侧同寄存器），供搬入/搬出用：一侧保持字面量，
    /// 另一侧由 `OperandBind` 在分配后改写。
    fn make_mov_reg(
        &self,
        parse: &(dyn Fn(&str) -> Option<Vec<I>> + Send + Sync),
        name_of: &(dyn Fn(u32, RegClass) -> Option<&'static str> + Send + Sync),
        preg: u32,
        class: RegClass,
    ) -> Result<I, IrError> {
        let name = name_of(preg, class).ok_or_else(|| {
            IrError::Unsupported(format!("寄存器 {preg} 没有汇编拼写（搬运指令）"))
        })?;
        let text = format!("mov {name}, {name}");
        let mut insts = parse(&text)
            .ok_or_else(|| IrError::Unsupported(format!("搬运指令无法汇编：{text:?}")))?;
        if insts.len() != 1 {
            return Err(IrError::Unsupported(format!(
                "搬运指令应恰好编成一条：{text:?}（得到 {} 条）",
                insts.len()
            )));
        }
        Ok(insts.remove(0))
    }
}

/// 把内联汇编文本里的占位符换成寄存器名。
///
/// 支持 `{}`（按出现顺序递增）与 `{n}`（显式序号）。**其他修饰一律拒绝**（`reg`/`eax`/
/// 命名参数等）：S2a 只承诺"无修饰的位置占位符"，静默猜错语义比报错糟得多。
fn substitute_placeholders(text: &str, names: &[String]) -> Result<String, IrError> {
    let mut out = String::with_capacity(text.len() + names.len() * 4);
    let mut next = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '{' {
            out.push(c);
            continue;
        }
        let mut spec = String::new();
        loop {
            match chars.next() {
                Some('}') => break,
                Some(ch) => spec.push(ch),
                None => {
                    return Err(IrError::Unsupported(
                        "内联汇编文本里有未闭合的 `{`".to_string(),
                    ));
                }
            }
        }
        let idx = if spec.is_empty() {
            let i = next;
            next += 1;
            i
        } else if let Ok(i) = spec.parse::<usize>() {
            i
        } else {
            return Err(IrError::Unsupported(format!(
                "内联汇编占位符修饰不支持：{{{spec}}}（S2a 只支持位置占位符）"
            )));
        };
        let name = names.get(idx).ok_or_else(|| {
            IrError::Unsupported(format!(
                "内联汇编占位符 {{{idx}}} 越界（只有 {} 个操作数）",
                names.len()
            ))
        })?;
        out.push_str(name);
    }
    Ok(out)
}
