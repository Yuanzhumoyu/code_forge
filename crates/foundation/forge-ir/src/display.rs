//! IR 文本输出 — Display impl for Function, Module, DFG entities。
//!
//! 输出严格 LLVM IR 文本（`define i32 @add(i32 %a, i32 %b)`、类型化操作数、
//! `icmp eq`/`sext ... to`、`ret`/`br`/`switch`/`unreachable`），可与
//! `ir_parser`（logos+lalrpop）双向 round-trip：
//! parse → display → parse 结构等价（见 tests/display_llvm.rs）。
//! 约定：iconst/undef/poison 内联为操作数字面量（不输出指令行）；
//! 块参数经 LLVM 扩展 `label %t(i32 %v)` 传递；Nop tombstone 省略；
//! 名字由 NameResolver 消歧（%x/%x_1），参数名优先 value_names 绑定。

use super::dfg::{DataFlowGraph, Instruction, ValueDef};
use super::entity::*;
use super::function::{Function, Module};
use super::immediate::Immediate;
use super::opcode::Opcode;
use super::terminator::Terminator;
use super::types::TypeContext;
use crate::ir_parser::llvm_mapping::llvm_mnemonic;
use std::collections::{HashMap, HashSet};
use std::fmt;

// ============================================================
// 值/块名称消歧
// ============================================================

/// 预计算函数内所有值/块的最终显示名（含 `%` 前缀），保证 SSA 唯一：
/// 绑定名作基础名、无名自动 `v{index}`/`b{index}`；同名冲突追加 `_1`/`_2` 后缀。
/// 值命名空间与块命名空间分开（LLVM 中 label 与 SSA 值可同名）。
struct NameResolver {
    values: HashMap<Value, String>,
    blocks: HashMap<Block, String>,
}

fn disambiguate(base: String, used: &mut HashSet<String>) -> String {
    let mut name = base.clone();
    let mut n = 1;
    while !used.insert(name.clone()) {
        name = format!("{}_{}", base, n);
        n += 1;
    }
    name
}

impl NameResolver {
    fn new(func: &Function, store: &TypeContext) -> Self {
        let mut used_values: HashSet<String> = HashSet::new();
        let mut used_blocks: HashSet<String> = HashSet::new();
        let mut values: HashMap<Value, String> = HashMap::new();
        let mut blocks: HashMap<Block, String> = HashMap::new();

        // 块名（layout 顺序，确定性）
        for &block in &func.layout.block_order {
            let base = func
                .block_names
                .get(&block)
                .map(|s| store.borrow().lookup_str(*s).to_string())
                .unwrap_or_else(|| format!("b{}", block.0));
            blocks.insert(block, disambiguate(base, &mut used_blocks));
        }

        // 值名（块参数 + 指令结果，layout + inst 顺序）
        for &block in &func.layout.block_order {
            for &v in func.dfg.block_param_values(block) {
                let base = func
                    .value_names
                    .get(&v)
                    .map(|s| store.borrow().lookup_str(*s).to_string())
                    .unwrap_or_else(|| format!("v{}", v.0));
                values.insert(v, disambiguate(base, &mut used_values));
            }
            for inst in func.dfg.block_inst_iter(block) {
                for &v in &inst.results {
                    let base = func
                        .value_names
                        .get(&v)
                        .map(|s| store.borrow().lookup_str(*s).to_string())
                        .unwrap_or_else(|| format!("v{}", v.0));
                    values.insert(v, disambiguate(base, &mut used_values));
                }
            }
        }

        Self { values, blocks }
    }

    fn value(&self, v: Value) -> &str {
        self.values.get(&v).map(String::as_str).unwrap_or("?")
    }

    fn block(&self, b: Block) -> &str {
        self.blocks.get(&b).map(String::as_str).unwrap_or("?")
    }
}

// ============================================================
// Module Display
// ============================================================

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref triple) = self.target_triple {
            writeln!(f, "target triple = \"{}\"", triple)?;
        }
        if !self.data_layout.is_default() {
            writeln!(f, "target datalayout = \"{:?}\"", self.data_layout)?;
        }
        writeln!(f)?;
        for func in self.iter_functions() {
            writeln!(
                f,
                "{}",
                FunctionDisplay {
                    func,
                    store: &self.types,
                    module: Some(self),
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
    module: Option<&'a Module>,
}

impl<'a> fmt::Display for FunctionDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let func = self.func;
        // 预计算值/块消歧名（一次遍历，确定性）
        let names = NameResolver::new(func, self.store);
        let sig = self.store.get_signature(func.signature);

        // define <retty> @name(<argty> %argname, ...) {
        write!(f, "define ")?;
        match sig.returns.first() {
            Some(ty) => write!(f, "{}", self.store.borrow().fmt_type(*ty))?,
            None => write!(f, "void")?,
        }
        write!(f, " @{}(", func.name)?;

        // 参数：entry 块参数的值名（LLVM 参数名绑定到 entry 参数值）
        let entry_params: Vec<Value> = func
            .layout
            .block_order
            .first()
            .map(|b| func.dfg.block_param_values(*b).to_vec())
            .unwrap_or_default();
        for (i, (ty, _name)) in sig.params.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{} ", self.store.borrow().fmt_type(*ty))?;
            match entry_params.get(i) {
                Some(v) => write!(f, "%{}", names.value(*v))?,
                None => write!(f, "%arg{}", i)?,
            }
        }
        writeln!(f, ") {{")?;

        // Blocks in layout order
        for &block in &func.layout.block_order {
            writeln!(
                f,
                "{}",
                BlockDisplay {
                    func,
                    store: self.store,
                    module: self.module,
                    block,
                    names: &names,
                }
            )?;
        }

        write!(f, "}}")
    }
}

struct BlockDisplay<'a> {
    func: &'a Function,
    store: &'a TypeContext,
    module: Option<&'a Module>,
    block: Block,
    names: &'a NameResolver,
}

impl<'a> fmt::Display for BlockDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dfg = &self.func.dfg;
        let block_data = match dfg.blocks.get(self.block.0 as usize) {
            Some(bd) => bd,
            None => return write!(f, "  block {} {{ /* removed */ }}", self.block),
        };

        // LLVM 严格：块标签无参数（entry 参数由 define 头绑定；
        // 非 entry 块参数经 br label %t(%args) 扩展传递）
        writeln!(f, "  %{}:", self.names.block(self.block))?;

        // Instructions
        for inst in dfg.block_inst_iter(self.block) {
            writeln!(
                f,
                "{}",
                InstDisplay {
                    func: self.func,
                    store: self.store,
                    module: self.module,
                    inst,
                    names: self.names,
                }
            )?;
        }

        // Terminator
        write!(
            f,
            "{}",
            TerminatorDisplay {
                func: self.func,
                store: self.store,
                term: &block_data.terminator,
                names: self.names,
            }
        )?;
        writeln!(f)
    }
}

struct InstDisplay<'a> {
    func: &'a Function,
    store: &'a TypeContext,
    module: Option<&'a Module>,
    inst: &'a Instruction,
    names: &'a NameResolver,
}

impl<'a> fmt::Display for InstDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let instruction = self.inst;

        // 值定义指令（iconst/fconst/undef/poison）内联为操作数，不输出指令行
        if matches!(
            instruction.opcode,
            Opcode::Iconst | Opcode::Fconst | Opcode::Undef | Opcode::Poison
        ) {
            return Ok(());
        }

        write!(f, "    ")?;

        // Results（LLVM 无类型注解）
        if !instruction.results.is_empty() {
            for (i, &r) in instruction.results.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "%{}", self.names.value(r))?;
            }
            write!(f, " = ")?;
        }

        // LLVM 指令名（含 icmp/fcmp 条件）
        write!(f, "{}", llvm_mnemonic(&instruction.opcode))?;

        // call / call_indirect：call <retty> @callee(<ty> <arg>, ...)
        if matches!(instruction.opcode, Opcode::Call | Opcode::CallIndirect) {
            if let Some(rty) = instruction
                .results
                .first()
                .and_then(|r| self.func.dfg.value_type(*r))
            {
                write!(f, " {}", self.store.borrow().fmt_type(rty))?;
            }
            if let Some(Immediate::Func(fr)) = instruction.immediates.first() {
                match self.module {
                    Some(m) => write!(f, " @{}", m.get_function(*fr).name)?,
                    None => write!(f, " @f{}", fr.0)?,
                }
            }
            write!(f, "(")?;
            let start = if matches!(instruction.opcode, Opcode::CallIndirect) {
                1
            } else {
                0
            };
            for (i, &op) in instruction.operands.iter().enumerate().skip(start) {
                if i > start {
                    write!(f, ", ")?;
                }
                match value_as_literal(self.func, op) {
                    Some(lit) => write!(f, "{}", lit)?,
                    None => {
                        if let Some(ty) = self.func.dfg.value_type(op) {
                            write!(f, "{} ", self.store.borrow().fmt_type(ty))?;
                        }
                        write!(f, "%{}", self.names.value(op))?;
                    }
                }
            }
            write!(f, ")")?;
            return Ok(());
        }

        // 类型化操作数（常量/undef 内联）
        if !instruction.operands.is_empty() {
            write!(f, " ")?;
            for (i, &op) in instruction.operands.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                match value_as_literal(self.func, op) {
                    Some(lit) => write!(f, "{}", lit)?,
                    None => {
                        if let Some(ty) = self.func.dfg.value_type(op) {
                            write!(f, "{} ", self.store.borrow().fmt_type(ty))?;
                        }
                        write!(f, "%{}", self.names.value(op))?;
                    }
                }
            }
        }

        // 转换指令：... to <dstty>
        if matches!(
            instruction.opcode,
            Opcode::Sextend | Opcode::Uextend | Opcode::Ireduce | Opcode::Ftrunc | Opcode::Bitcast
        ) && let Some(rty) = instruction
            .results
            .first()
            .and_then(|r| self.func.dfg.value_type(*r))
        {
            write!(f, " to {}", self.store.borrow().fmt_type(rty))?;
        }

        Ok(())
    }
}

/// Format a Value with its disambiguated name.
///
/// Format MemFlags as a compact string.
///
/// Format InstFlags summary (only if non-trivial).
struct TerminatorDisplay<'a> {
    func: &'a Function,
    store: &'a TypeContext,
    term: &'a Terminator,
    names: &'a NameResolver,
}

impl<'a> fmt::Display for TerminatorDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dfg = &self.func.dfg;
        match self.term {
            Terminator::Return { values } => {
                write!(f, "    ret ")?;
                match values.first() {
                    None => write!(f, "void"),
                    Some(v) => match value_as_literal(self.func, *v) {
                        Some(lit) => write!(f, "{}", lit),
                        None => match dfg.value_type(*v) {
                            Some(ty) => write!(
                                f,
                                "{} %{}",
                                self.store.borrow().fmt_type(ty),
                                self.names.value(*v)
                            ),
                            None => write!(f, "%{}", self.names.value(*v)),
                        },
                    },
                }
            }
            Terminator::Jump { target, args } => {
                write!(f, "    br label %{}", self.names.block(*target))?;
                self.fmt_block_args(f, args)?;
                Ok(())
            }
            Terminator::Branch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                write!(
                    f,
                    "  br i1 %{}, label %{}",
                    self.names.value(*cond),
                    self.names.block(*then_block)
                )?;
                self.fmt_block_args(f, then_args)?;
                write!(f, ", label %{}", self.names.block(*else_block))?;
                self.fmt_block_args(f, else_args)?;
                Ok(())
            }
            Terminator::Switch {
                discriminant,
                default_block,
                default_args,
                cases,
            } => {
                write!(f, "    switch ")?;
                match value_as_literal(self.func, *discriminant) {
                    Some(lit) => write!(f, "{}", lit)?,
                    None => match dfg.value_type(*discriminant) {
                        Some(ty) => write!(
                            f,
                            "{} %{}",
                            self.store.borrow().fmt_type(ty),
                            self.names.value(*discriminant)
                        )?,
                        None => write!(f, "%{}", self.names.value(*discriminant))?,
                    },
                }
                write!(f, ", label %{}", self.names.block(*default_block))?;
                self.fmt_block_args(f, default_args)?;
                write!(f, " [")?;
                for (i, (val, blk, _args)) in cases.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "i32 {}, label %{}", val, self.names.block(*blk))?;
                }
                write!(f, " ]")?;
                Ok(())
            }
            Terminator::Unreachable => write!(f, "    unreachable"),
        }
    }
}

impl<'a> TerminatorDisplay<'a> {
    /// LLVM 扩展：块参数经 `label %t(i32 %v)` 传递（forge 块参数无 LLVM 标准语法）。
    fn fmt_block_args(&self, f: &mut fmt::Formatter<'_>, args: &[Value]) -> fmt::Result {
        if args.is_empty() {
            return Ok(());
        }
        write!(f, "(")?;
        for (i, &a) in args.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            match value_as_literal(self.func, a) {
                Some(lit) => write!(f, "{}", lit)?,
                None => match self.func.dfg.value_type(a) {
                    Some(ty) => write!(
                        f,
                        "{} %{}",
                        self.store.borrow().fmt_type(ty),
                        self.names.value(a)
                    )?,
                    None => write!(f, "%{}", self.names.value(a))?,
                },
            }
        }
        write!(f, ")")
    }
}

// ============================================================
// Immediate Display
// ============================================================

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

/// 若 Value 由常量/undef/poison 指令定义，返回 LLVM 字面量文本
/// （`i32 42`/`undef`/`poison`）；否则 None（正常值，用 %name）。
/// display 遍历指令时跳过这些"值定义"指令行，使用处内联。
fn value_as_literal(func: &Function, v: Value) -> Option<String> {
    let def = func.dfg.values.get(v.0 as usize)?.def;
    let ValueDef::Inst(inst, _) = def else {
        return None;
    };
    let inst_data = func.dfg.insts.get(inst.0 as usize)?;
    match inst_data.opcode {
        Opcode::Iconst => {
            let Immediate::Const(cid) = inst_data.immediates.first().copied()? else {
                return None;
            };
            let (val, bits) = func.constants.get_int(cid)?;
            Some(format!("i{} {}", bits, val))
        }
        Opcode::Fconst => {
            let Immediate::Const(cid) = inst_data.immediates.first().copied()? else {
                return None;
            };
            let bits = func.constants.get_float(cid)?;
            let f = f64::from_bits(bits);
            Some(format!("f64 {}", f))
        }
        Opcode::Undef => Some("undef".to_string()),
        Opcode::Poison => Some("poison".to_string()),
        _ => None,
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
                module: None,
            }
        )
    }

    #[test]
    fn display_simple_arithmetic_function() {
        let text = display_simple_func("double");
        // Should contain fn name, param, opcode, ret
        assert!(
            text.contains("define i32 @double(i32 %x)"),
            "expected define header, got: {}",
            text
        );
        assert!(
            text.contains("i32 %x"),
            "expected typed param, got: {}",
            text
        );
        assert!(
            text.contains("add i32 %x, i32 %x"),
            "expected add opcode, got: {}",
            text
        );
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
            text.contains("= add i32"),
            "expected LLVM instruction, got: {}",
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
                module: None,
            }
        );
        assert!(text.contains("br "), "expected br, got: {}", text);
        assert!(
            text.contains("br i1 %c, label %b1, label %b2"),
            "expected branch, got: {}",
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
                module: None,
            }
        );
        assert!(text.contains("switch"), "expected switch, got: {}", text);
        assert!(
            text.contains("switch i32 %c, label %b1"),
            "expected switch header, got: {}",
            text
        );
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
                module: None,
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
                module: None,
            }
        );
        assert!(
            text.contains("br label"),
            "expected br label, got: {}",
            text
        );
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
                module: None,
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
                module: None,
            }
        );
        assert!(
            text.contains("define i32 @test(i32 %a, f64 %b)"),
            "expected define header with params, got: {}",
            text
        );
        assert!(
            text.contains("i32 %a, f64 %b"),
            "expected typed params, got: {}",
            text
        );
        assert!(text.contains("ret"), "expected ret, got: {}", text);
    }

    #[test]
    fn display_param_names_bound() {
        // create_block_with_params 的参数名现在绑定到参数 value → 块参数显示 %x
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "x")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
        let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "x")]);
        fb.switch_to_block(entry);
        let r = fb.iadd(params[0], params[0]);
        fb.ret(&[r]);
        let store = fb.type_ctx().clone();
        let func = fb.finish();
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(
            text.contains("define i32 @f(i32 %x)"),
            "param value should bind to its name, got: {}",
            text
        );
        assert!(
            text.contains("add i32 %x, i32 %x"),
            "param name used in operands, got: {}",
            text
        );
        assert!(
            text.contains("ret i32 %"),
            "ret should use % name, got: {}",
            text
        );
    }

    #[test]
    fn display_unnamed_values_get_indexed_names() {
        // 不绑定的值自动编号（%v{index}），唯一且带 % 前缀
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        let a = fb.iconst_i32(1);
        let b = fb.iconst_i32(2);
        let r = fb.iadd(a, b);
        fb.ret(&[r]);
        let store = fb.type_ctx().clone();
        let func = fb.finish();
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(
            text.contains("%v"),
            "unnamed values get %v names, got: {}",
            text
        );
        // %v 前缀（LLVM 合法），不是裸 v
        assert!(!text.contains("iadd v"), "no bare v names, got: {}", text);
    }

    #[test]
    fn display_disambiguates_duplicate_names() {
        // 两个值绑定同名 → display 消歧为 %x / %x_1（SSA 唯一）
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "x")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
        let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "x")]);
        fb.switch_to_block(entry);
        let a = fb.iadd(params[0], params[0]);
        fb.bind_name(a, "x"); // 与参数同名 → 消歧 %x / %x_1
        let r = fb.iadd(a, a);
        fb.ret(&[r]);
        let store = fb.type_ctx().clone();
        let func = fb.finish();
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(
            text.contains("%x_1 = add i32 %x, i32 %x"),
            "duplicate names disambiguated, got: {}",
            text
        );
        assert!(
            text.contains("ret i32 %"),
            "result uses name, got: {}",
            text
        );
    }

    #[test]
    fn bind_name_overwrites() {
        // bind_name 可覆盖更新（同名重绑）
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        let x = fb.iconst_i32(1);
        let a = fb.iadd(x, x);
        fb.bind_name(a, "first");
        fb.bind_name(a, "second");
        let r = fb.iadd(a, a);
        fb.ret(&[r]);
        let store = fb.type_ctx().clone();
        let func = fb.finish();
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(
            text.contains("%second"),
            "bind_name overwrites, got: {}",
            text
        );
        assert!(!text.contains("%first"), "old name gone, got: {}", text);
    }

    #[test]
    fn bind_block_name_shows_label() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.bind_block_name(entry, "entry");
        fb.switch_to_block(entry);
        let r = fb.iconst_i32(1);
        fb.ret(&[r]);
        let store = fb.type_ctx().clone();
        let func = fb.finish();
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(text.contains("%entry:"), "block name shown, got: {}", text);
    }
}

// ============================================================
// LLVM 字面量内联（round-trip：iconst/undef/poison 作操作数输出）
// ============================================================
