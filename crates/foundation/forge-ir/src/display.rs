//! IR 文本输出 — Display impl for Function, Module, DFG entities.

use super::dfg::{DataFlowGraph, Instruction, ValueDef};
use super::entity::*;
use super::function::{Function, Module};
use super::immediate::Immediate;
use super::inst_flags::InstFlags;
use super::mem_flags::MemFlags;
use super::terminator::Terminator;
use super::types::TypeContext;
use std::fmt;

// ============================================================
// Module Display
// ============================================================

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref triple) = self.target_triple {
            writeln!(f, "target triple = \"{}\"", triple)?;
        }
        writeln!(f, "target datalayout = \"{:?}\"", self.data_layout)?;
        writeln!(f)?;
        for func in self.iter_functions() {
            writeln!(
                f,
                "{}",
                FunctionDisplay {
                    func,
                    store: &self.types
                }
            )?;
            writeln!(f)?;
        }
        Ok(())
    }
}

struct FunctionDisplay<'a> {
    func: &'a Function,
    store: &'a TypeContext,
}

impl<'a> fmt::Display for FunctionDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let func = self.func;
        let sig = self.store.get_signature(func.signature);
        write!(f, "fn ")?;

        write!(f, "{}", func.name)?;

        // Signature
        write!(f, "(")?;
        for (i, (ty, name)) in sig.params.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}: {}", name, self.store.borrow().fmt_type(*ty))?;
        }
        write!(f, ")")?;

        if !sig.returns.is_empty() {
            write!(f, " -> ")?;
            for (i, ty) in sig.returns.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", self.store.borrow().fmt_type(*ty))?;
            }
        }
        writeln!(f, " {{")?;

        // Blocks in layout order
        for &block in &func.layout.block_order {
            writeln!(
                f,
                "{}",
                BlockDisplay {
                    func,
                    store: self.store,
                    block
                }
            )?;
        }

        writeln!(f, "}}")
    }
}

struct BlockDisplay<'a> {
    func: &'a Function,
    store: &'a TypeContext,
    block: Block,
}

impl<'a> fmt::Display for BlockDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dfg = &self.func.dfg;
        let block_data = match dfg.blocks.get(self.block.0 as usize) {
            Some(bd) => bd,
            None => return write!(f, "  block {} {{ /* removed */ }}", self.block),
        };

        // Block header with optional name
        let block_label = self
            .func
            .block_names
            .get(&self.block)
            .map(|s| format!("%{}", self.store.borrow().lookup_str(*s)))
            .unwrap_or_else(|| format!("b{}", self.block));
        write!(f, "  {}", block_label)?;

        // Block params
        let params = dfg.block_params(self.block);
        if !params.is_empty() {
            let param_values = dfg.block_param_values(self.block);
            write!(f, "(")?;
            for (i, ty) in params.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                if i < param_values.len() {
                    let v = param_values[i];
                    let v_name = self
                        .func
                        .value_names
                        .get(&v)
                        .map(|s| format!("%{}", self.store.borrow().lookup_str(*s)))
                        .unwrap_or_else(|| format!("v{}", v));
                    write!(f, "{}: {}", v_name, self.store.borrow().fmt_type(*ty))?;
                } else {
                    write!(f, "{}", self.store.borrow().fmt_type(*ty))?;
                }
            }
            write!(f, ")")?;
        }
        writeln!(f, ":")?;

        // Instructions
        for inst in dfg.block_inst_iter(self.block) {
            writeln!(
                f,
                "{}",
                InstDisplay {
                    func: self.func,
                    store: self.store,
                    inst
                }
            )?;
        }

        // Terminator
        write!(
            f,
            "{}",
            TerminatorDisplay {
                term: &block_data.terminator
            }
        )?;
        writeln!(f)
    }
}

struct InstDisplay<'a> {
    func: &'a Function,
    store: &'a TypeContext,
    inst: &'a Instruction,
}

impl<'a> fmt::Display for InstDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let instruction = self.inst;

        write!(f, "    ")?;

        // Results with type annotations
        if !instruction.results.is_empty() {
            for (i, &r) in instruction.results.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                // Show value name if available
                if let Some(name) = self.func.value_names.get(&r) {
                    write!(f, "%{}", self.store.borrow().lookup_str(*name))?;
                } else {
                    write!(f, "v{}", r)?;
                }
                // Show type annotation
                if let Some(ty) = self.func.dfg.value_type(r) {
                    write!(f, ": {}", self.store.borrow().fmt_type(ty))?;
                }
            }
            write!(f, " = ")?;
        }

        // Opcode
        write!(f, "{}", instruction.opcode.mnemonic())?;

        // Operands with names
        if !instruction.operands.is_empty() {
            write!(f, " ")?;
            for (i, &op) in instruction.operands.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                fmt_value_name(f, op, self.func, self.store)?;
            }
        }

        // Immediates
        if !instruction.immediates.is_empty() {
            write!(f, " [")?;
            for (i, imm) in instruction.immediates.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                fmt_immediate(f, imm, self.store)?;
            }
            write!(f, "]")?;
        }

        // MemFlags for load/store
        if instruction.mem_flags != MemFlags::NONE {
            write!(f, " <")?;
            fmt_mem_flags(f, instruction.mem_flags)?;
            write!(f, ">")?;
        }

        // InstFlags summary (only show non-trivial flags)
        fmt_flags_summary(f, instruction.flags)?;

        // Metadata attachments
        if !instruction.metadata.is_empty() {
            write!(f, " [")?;
            for (i, meta) in instruction.metadata.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "!{} !{}", meta.kind.name(), meta.node.0)?;
            }
            write!(f, "]")?;
        }

        // Source location
        if let Some(ref loc) = instruction.loc {
            write!(f, "  ; {}", loc)?;
        }

        Ok(())
    }
}

/// Format a Value with its optional name.
fn fmt_value_name(
    f: &mut fmt::Formatter<'_>,
    v: Value,
    func: &Function,
    store: &TypeContext,
) -> fmt::Result {
    if let Some(name) = func.value_names.get(&v) {
        write!(f, "%{}", store.borrow().lookup_str(*name))
    } else {
        write!(f, "v{}", v)
    }
}

/// Format MemFlags as a compact string.
fn fmt_mem_flags(f: &mut fmt::Formatter<'_>, mem: MemFlags) -> fmt::Result {
    let mut first = true;
    let mut flag = |cond: bool, text: &str| -> fmt::Result {
        if cond {
            if !first {
                write!(f, ", ")?;
            }
            write!(f, "{}", text)?;
            first = false;
        }
        Ok(())
    };
    flag(mem.is_volatile(), "volatile")?;
    flag(mem.is_nontemporal(), "nontemporal")?;
    flag(mem.is_acquire(), "acquire")?;
    flag(mem.is_release(), "release")?;
    Ok(())
}

/// Format InstFlags summary (only if non-trivial).
fn fmt_flags_summary(f: &mut fmt::Formatter<'_>, flags: InstFlags) -> fmt::Result {
    if flags == InstFlags::NONE {
        return Ok(());
    }
    write!(f, " {{")?;
    let mut first = true;
    let mut flag = |cond: bool, text: &str| -> fmt::Result {
        if cond {
            if !first {
                write!(f, ", ")?;
            }
            write!(f, "{}", text)?;
            first = false;
        }
        Ok(())
    };
    flag(flags.contains(InstFlags::SIDE_EFFECT), "sidefx")?;
    flag(flags.contains(InstFlags::MAY_UB), "may_ub")?;
    flag(flags.contains(InstFlags::ATOMIC), "atomic")?;
    flag(flags.contains(InstFlags::TAIL_CALL), "tail")?;
    flag(flags.contains(InstFlags::NOUNDEF), "noundef")?;
    if flags.contains(InstFlags::FMF_FAST) {
        flag(true, "fast")?;
    } else {
        flag(flags.contains(InstFlags::FMF_NNAN), "nnan")?;
        flag(flags.contains(InstFlags::FMF_NINF), "ninf")?;
        flag(flags.contains(InstFlags::FMF_NSZ), "nsz")?;
        flag(flags.contains(InstFlags::FMF_ARCP), "arcp")?;
        flag(flags.contains(InstFlags::FMF_REASSOC), "reassoc")?;
    }
    write!(f, "}}")
}

struct TerminatorDisplay<'a> {
    term: &'a Terminator,
}

impl<'a> fmt::Display for TerminatorDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "    ")?;
        match self.term {
            Terminator::Branch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                write!(f, "br {}, {}, {}", cond, then_block, else_block)?;
                if !then_args.is_empty() || !else_args.is_empty() {
                    write!(f, " (")?;
                    for v in then_args {
                        write!(f, "{} ", v)?;
                    }
                    write!(f, "| ")?;
                    for v in else_args {
                        write!(f, "{} ", v)?;
                    }
                    write!(f, ")")?;
                }
                Ok(())
            }
            Terminator::Jump { target, args } => {
                write!(f, "jmp {}", target)?;
                if !args.is_empty() {
                    write!(f, " (")?;
                    for v in args {
                        write!(f, "{} ", v)?;
                    }
                    write!(f, ")")?;
                }
                Ok(())
            }
            Terminator::Return { values } => {
                write!(f, "ret")?;
                if !values.is_empty() {
                    write!(f, " ")?;
                    for (i, v) in values.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", v)?;
                    }
                }
                Ok(())
            }
            Terminator::Switch {
                discriminant,
                default_block,
                default_args,
                cases,
            } => {
                write!(f, "switch {} default={}", discriminant, default_block)?;
                if !default_args.is_empty() {
                    write!(f, "(")?;
                    for (i, v) in default_args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "v{}", v)?;
                    }
                    write!(f, ")")?;
                }
                write!(f, " [")?;
                for (val, target, _) in cases.iter() {
                    write!(f, " {}->{}", val, target)?;
                }
                write!(f, " ]")
            }
            Terminator::Unreachable => write!(f, "unreachable"),
        }
    }
}

// ============================================================
// Immediate Display
// ============================================================

fn fmt_immediate(f: &mut fmt::Formatter<'_>, imm: &Immediate, store: &TypeContext) -> fmt::Result {
    match imm {
        Immediate::Int(v) => write!(f, "{}", v),
        Immediate::Uint(v) => write!(f, "{}", v),
        Immediate::Const(c) => write!(f, "const {}", c),
        Immediate::Block(b) => write!(f, "block {}", b),
        Immediate::Func(fr) => write!(f, "@{}", fr),
        Immediate::Global(g) => write!(f, "global {}", g),
        Immediate::Type(t) => write!(f, "{}", store.borrow().fmt_type(*t)),
        Immediate::String(_s) => write!(f, "\"...\""),
    }
}

// ============================================================
// Debug Display for DFG
// ============================================================

impl fmt::Display for DataFlowGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "DFG: {} values, {} insts, {} blocks",
            self.value_count(),
            self.inst_count(),
            self.block_count()
        )?;

        writeln!(f, "Values:")?;
        for (v, vd) in self.values() {
            let def_str = match vd.def {
                ValueDef::Inst(i, idx) => format!("inst {}.{}", i, idx),
                ValueDef::Param(b, idx) => format!("block {} param {}", b, idx),
            };
            writeln!(f, "  {} = {} : t{}", v, def_str, vd.ty.0)?;
        }
        Ok(())
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::FunctionBuilder;
    use crate::types::{FunctionSignature, TypeContext};

    /// Helper: build a simple function returning i32 and return its text.
    fn display_simple_func(name: &str) -> String {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "x")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new(name, ctx.clone(), sig);
        let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "x")]);
        fb.switch_to_block(entry);
        let r = fb.iadd(params[0], params[0]);
        fb.ret(&[r]);
        let store = fb.type_ctx().clone();
        let func = fb.finish();
        format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
            }
        )
    }

    #[test]
    fn display_simple_arithmetic_function() {
        let text = display_simple_func("double");
        // Should contain fn name, param, opcode, ret
        assert!(
            text.contains("fn double"),
            "expected 'fn double', got: {}",
            text
        );
        assert!(
            text.contains("x: i32"),
            "expected param type, got: {}",
            text
        );
        assert!(text.contains("iadd"), "expected iadd opcode, got: {}", text);
        assert!(
            text.contains("ret"),
            "expected ret terminator, got: {}",
            text
        );
    }

    #[test]
    fn display_value_type_annotation() {
        let text = display_simple_func("typed");
        // Values should have type annotations: "v2: i32 = iadd"
        assert!(
            text.contains(": i32 = iadd"),
            "expected type annotation, got: {}",
            text
        );
    }

    #[test]
    fn display_block_has_label() {
        let text = display_simple_func("blocks");
        // Block should have a label and colon
        assert!(text.contains(":"), "expected block colon, got: {}", text);
        // Should have indentation for instructions
        assert!(text.contains("    "), "expected indentation, got: {}", text);
    }

    #[test]
    fn display_function_with_branch() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "c")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "c")]);
        let then_blk = fb.create_block();
        let else_blk = fb.create_block();
        fb.switch_to_block(entry);
        fb.branch(params[0], then_blk, &[], else_blk, &[]);
        fb.switch_to_block(then_blk);
        let v1 = fb.iconst_i32(1);
        fb.ret(&[v1]);
        fb.switch_to_block(else_blk);
        let v2 = fb.iconst_i32(0);
        fb.ret(&[v2]);
        let store = fb.type_ctx().clone();
        let func = fb.finish();
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
            }
        );
        assert!(text.contains("br "), "expected br, got: {}", text);
        assert!(
            text.contains("iadd") || text.contains("iconst"),
            "expected opcodes, got: {}",
            text
        );
    }

    #[test]
    fn display_dfg_debug() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let func = fb.finish();
        let text = format!("{}", func.dfg);
        assert!(text.contains("DFG:"), "expected DFG header, got: {}", text);
        assert!(text.contains("values"), "expected values, got: {}", text);
        assert!(text.contains("insts"), "expected insts, got: {}", text);
        assert!(text.contains("blocks"), "expected blocks, got: {}", text);
    }

    #[test]
    fn display_terminator_switch() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "c")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "c")]);
        let default_blk = fb.create_block();
        let case1_blk = fb.create_block();
        fb.switch_to_block(entry);
        fb.switch(params[0], default_blk, &[(1, case1_blk, &[])]);
        fb.switch_to_block(default_blk);
        let v = fb.iconst_i32(0);
        fb.ret(&[v]);
        fb.switch_to_block(case1_blk);
        let v1 = fb.iconst_i32(1);
        fb.ret(&[v1]);
        let store = fb.type_ctx().clone();
        let func = fb.finish();
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
            }
        );
        assert!(text.contains("switch"), "expected switch, got: {}", text);
        assert!(text.contains("default"), "expected default, got: {}", text);
    }

    #[test]
    fn display_terminator_return_void() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        fb.ret(&[]);
        let store = fb.type_ctx().clone();
        let func = fb.finish();
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
            }
        );
        assert!(text.contains("ret"), "expected ret, got: {}", text);
    }

    #[test]
    fn display_terminator_jump() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        let target = fb.create_block();
        fb.switch_to_block(entry);
        fb.jump(target, &[]);
        fb.switch_to_block(target);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let store = fb.type_ctx().clone();
        let func = fb.finish();
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
            }
        );
        assert!(text.contains("jmp"), "expected jmp, got: {}", text);
    }

    #[test]
    fn display_terminator_unreachable() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        fb.unreachable();
        let store = fb.type_ctx().clone();
        let func = fb.finish();
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
            }
        );
        assert!(
            text.contains("unreachable"),
            "expected unreachable, got: {}",
            text
        );
    }

    #[test]
    fn display_function_signature_with_return_types() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(
            &[(ctx.i32_ty(), "a"), (ctx.f64_ty(), "b")],
            &[ctx.i32_ty(), ctx.f64_ty()],
        );
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _params) =
            fb.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::F64, "b")]);
        fb.switch_to_block(entry);
        let v = fb.iconst_i32(2);
        let fv = fb.fconst_f64(1.0);
        fb.ret(&[v, fv]);
        let store = fb.type_ctx().clone();
        let func = fb.finish();
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
            }
        );
        assert!(
            text.contains("a: i32"),
            "expected first param, got: {}",
            text
        );
        assert!(
            text.contains("b: f64"),
            "expected second param, got: {}",
            text
        );
        assert!(
            text.contains("-> i32, f64"),
            "expected return types, got: {}",
            text
        );
    }
}
