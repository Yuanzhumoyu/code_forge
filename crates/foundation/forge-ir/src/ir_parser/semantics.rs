//! 语义构建：ParsedModule（grammar.lalrpop 产出）→ forge IR。
//!
//! 职责：名称解析（`@`/`%` 符号表 + SSA 唯一性）、ParsedType → TypeId、
//! LLVM opcode → forge Opcode（含 icmp/fcmp 条件、向量精化、trunc 分发）、
//! 类型化操作数 → Value、常量、terminator、跨函数 `@` 解析。

use std::collections::HashMap;

use crate::builder::FunctionBuilder;
use crate::entity::{Block, FuncRef, GlobalId, TypeId, Value};
use crate::function::Module;
use crate::immediate::Immediate;
use crate::inst_flags::InstFlags;
use crate::opcode::Opcode;
use crate::types::{FunctionSignature, TypeContext};

use super::ParseError;
use super::ast_items::*;
use super::lexer::TokenStream;
use super::llvm_mapping;

/// 解析 LLVM IR module（多函数 + target）。
pub fn parse_module(source: &str) -> Result<Module, ParseError> {
    let ast = parse_to_ast(source)?;
    build_module(&ast)
}

/// 解析单个 LLVM IR 函数（module 的第一个 define）。
pub fn parse_function(source: &str) -> Result<crate::function::Function, ParseError> {
    let ast = parse_to_ast(source)?;
    for item in &ast.items {
        if let ParsedItem::Function(f) = item {
            if f.declare {
                continue;
            }
            let ctx = TypeContext::new();
            return build_function(f, &HashMap::new(), &ctx);
        }
    }
    Err(ParseError::Semantic(
        "no function definition found".to_string(),
    ))
}

fn parse_to_ast(source: &str) -> Result<ParsedModule, ParseError> {
    let lexer = TokenStream::new(source);
    let parser = super::grammar::ModuleParser::new();
    parser
        .parse(lexer)
        .map_err(|e| ParseError::Parse(format!("{:?}", e)))
}

// ── 模块构建 ──

fn build_module(ast: &ParsedModule) -> Result<Module, ParseError> {
    let mut module = Module::new();
    // 所有函数共享 module 的 TypeContext（bind_name 的 InternedStr 进入
    // module.types 的 pool——display 用同一 pool lookup，避免跨 pool 越界）
    let ctx = module.types.clone();
    let mut func_refs: HashMap<String, FuncRef> = HashMap::new();
    // 单遍顺序构建：每个 define 构建后注册 FuncRef（call @name 引用此前已定义的函数；
    // 前向引用/declare 外部函数暂不支持，报未定义）
    for item in &ast.items {
        match item {
            ParsedItem::Target(k, v) => {
                if k.as_str() == "triple" {
                    module.set_target_triple(v);
                }
                // datalayout 后续可用（Module.data_layout 为 pub 字段）
            }
            ParsedItem::Function(f) => {
                if f.declare {
                    continue; // declare 外部函数暂不注册
                }
                let func = build_function(f, &func_refs, &ctx)?;
                let fr = module.add_function(func);
                func_refs.insert(f.name.clone(), fr);
            }
        }
    }
    Ok(module)
}

fn signature_of(f: &ParsedFunction, ctx: &TypeContext) -> FunctionSignature {
    let params: Vec<(TypeId, &str)> = f
        .params
        .iter()
        .map(|(pt, name)| (to_type(pt, ctx), name.trim_start_matches('%')))
        .collect();
    let rets = vec![to_type(&f.ret_ty, ctx)];
    FunctionSignature::new(&params, &rets)
}

fn to_type(pt: &ParsedType, ctx: &TypeContext) -> TypeId {
    match pt {
        ParsedType::Int(bits) => ctx.int_ty(*bits),
        ParsedType::Float(bits) => ctx.float_ty(*bits),
        ParsedType::Ptr => ctx.ptr_ty(),
        ParsedType::Vec(n, e) => ctx.vector_ty(to_type(e, ctx), *n),
        ParsedType::Array(n, e) => ctx.array_ty(to_type(e, ctx), *n),
        ParsedType::Struct(tys) => {
            let field_tys: Vec<TypeId> = tys.iter().map(|t| to_type(t, ctx)).collect();
            ctx.borrow_mut().struct_anon(field_tys, false)
        }
        ParsedType::Void | ParsedType::Label | ParsedType::Metadata => ctx.void_ty(),
    }
}

// ── 函数构建 ──

fn build_function(
    f: &ParsedFunction,
    func_refs: &HashMap<String, FuncRef>,
    ctx: &TypeContext,
) -> Result<crate::function::Function, ParseError> {
    let sig = signature_of(f, ctx);
    let name = f.name.trim_start_matches('@').to_string();
    let mut fb = FunctionBuilder::new(&name, ctx.clone(), sig);

    // 值符号表（%name → Value），SSA 唯一性检查
    let mut value_map: HashMap<String, Value> = HashMap::new();
    let mut block_map: HashMap<String, Block> = HashMap::new();

    // 第一遍：创建所有块（entry 带函数参数）
    let (entry, params) = if f.params.is_empty() {
        let b = fb.create_block();
        fb.bind_block_name(b, f.blocks[0].label.trim_start_matches('%'));
        (b, vec![])
    } else {
        let params: Vec<(TypeId, &str)> = f
            .params
            .iter()
            .map(|(pt, name)| (to_type(pt, ctx), name.trim_start_matches('%')))
            .collect();
        let (b, vals) = fb.create_block_with_params(&params);
        fb.bind_block_name(b, f.blocks[0].label.trim_start_matches('%'));
        (b, vals)
    };
    block_map.insert(f.blocks[0].label.clone(), entry);
    for (i, p) in f.params.iter().enumerate() {
        if i < params.len() {
            value_map.insert(p.1.clone(), params[i]);
        }
    }
    for b in f.blocks.iter().skip(1) {
        let blk = fb.create_block();
        fb.bind_block_name(blk, b.label.trim_start_matches('%'));
        block_map.insert(b.label.clone(), blk);
    }

    // 第二遍：填充指令与终结符
    for pb in &f.blocks {
        let block = block_map[&pb.label];
        fb.switch_to_block(block);
        for inst in &pb.insts {
            build_inst(inst, &mut fb, ctx, &mut value_map, func_refs, block)?;
        }
        build_terminator(&pb.terminator, &mut fb, ctx, &value_map, &block_map, block)?;
    }

    Ok(fb.finish())
}

// ── 指令构建 ──

fn build_inst(
    inst: &ParsedInst,
    fb: &mut FunctionBuilder,
    ctx: &TypeContext,
    value_map: &mut HashMap<String, Value>,
    func_refs: &HashMap<String, FuncRef>,
    block: Block,
) -> Result<(), ParseError> {
    let op_name = inst.opcode.as_str();
    let cond = inst.cond.as_deref();

    // 特例指令
    match op_name {
        "call" => {
            // args[0] = (ret_ty, callee)；其余为实参
            let (ret_ty_pt, callee) = (&inst.args[0].ty, &inst.args[0].op);
            let ret_ty = to_type(ret_ty_pt, ctx);
            let ret_ty2 = ret_ty_pt.clone();
            let call_args: Vec<Value> = inst.args[1..]
                .iter()
                .map(|a| operand_to_value(a, ctx, value_map, func_refs, fb, block))
                .collect::<Result<_, _>>()?;
            let vals = match callee {
                Operand::Global(name) => {
                    let fr = func_refs.get(name).ok_or_else(|| {
                        ParseError::Semantic(format!("undefined function {name}"))
                    })?;
                    fb.irb(block).call(*fr, &call_args, &[ret_ty])
                }
                _ => {
                    let po = ParsedOperand {
                        ty: ret_ty2.clone(),
                        op: callee.clone(),
                    };
                    let ptr = operand_to_value(&po, ctx, value_map, func_refs, fb, block)?;
                    fb.irb(block).call_indirect(ptr, &call_args, &[ret_ty])
                }
            };
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, vals.first().copied(), fb, value_map)?;
            }
            return Ok(());
        }
        "load" => {
            // args[0] = (val_ty, undef)；args[1] = (addr_ty, addr)
            let val_ty = to_type(&inst.args[0].ty, ctx);
            let addr = operand_to_value(&inst.args[1], ctx, value_map, func_refs, fb, block)?;
            let v = fb.irb(block).load(addr, val_ty);
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
            }
            return Ok(());
        }
        "store" => {
            let val = operand_to_value(&inst.args[0], ctx, value_map, func_refs, fb, block)?;
            let addr = operand_to_value(&inst.args[1], ctx, value_map, func_refs, fb, block)?;
            fb.irb(block).store(val, addr);
            return Ok(());
        }
        "alloca" => {
            let ty = to_type(&inst.args[0].ty, ctx);
            let v = fb.irb(block).alloca(ty, 1);
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
            }
            return Ok(());
        }
        "getelementptr" => {
            // args[0] = (base_ty, undef)；args[1] = (ptr_ty, ptr)；args[2..] = indices
            let base_ty = to_type(&inst.args[0].ty, ctx);
            let ptr = operand_to_value(&inst.args[1], ctx, value_map, func_refs, fb, block)?;
            let indices: Vec<Value> = inst.args[2..]
                .iter()
                .map(|a| operand_to_value(a, ctx, value_map, func_refs, fb, block))
                .collect::<Result<_, _>>()?;
            let mut ops = indices.clone();
            ops.insert(0, ptr);
            let v = fb.irb(block).emit1(
                Opcode::GetElementPtr,
                ops,
                vec![Immediate::Type(base_ty)],
                ctx.pointer_ty(0),
                InstFlags::NONE,
            );
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
            }
            return Ok(());
        }
        "stack_addr" => {
            let offset = match inst.args.first().map(|a| &a.op) {
                Some(Operand::Int(n)) => *n as i32,
                _ => 0,
            };
            let v = fb.irb(block).stack_addr(offset);
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
            }
            return Ok(());
        }
        _ => {}
    }

    // undef/poison 指令：无操作数（args[0].ty 作结果类型）
    if matches!(op_name, "undef" | "poison") {
        let ty = inst
            .args
            .first()
            .map(|a| to_type(&a.ty, ctx))
            .unwrap_or_else(|| ctx.i32_ty());
        let op = llvm_mapping::opcode(op_name).map_err(ParseError::Semantic)?;
        let v = fb.irb(block).emit1(op, vec![], vec![], ty, InstFlags::NONE);
        if let Some(r) = inst.result.as_ref() {
            bind_result(r, Some(v), fb, value_map)?;
        }
        return Ok(());
    }

    // 通用路径：operands → Value（转换指令的 args[1] 是 to <dst> 类型占位，跳过）
    let is_conv = matches!(op_name, "sext" | "zext" | "trunc" | "bitcast");
    let operands: Vec<Value> = inst
        .args
        .iter()
        .take(if is_conv { 1 } else { inst.args.len() })
        .map(|a| operand_to_value(a, ctx, value_map, func_refs, fb, block))
        .collect::<Result<_, _>>()?;

    // 结果类型：单结果指令从第一个操作数类型推导；显式类型（undef/poison 等）用 args[0].ty
    let result_ty = if operands.is_empty() {
        inst.args
            .first()
            .map(|a| to_type(&a.ty, ctx))
            .unwrap_or_else(|| ctx.i32_ty())
    } else {
        fb.func
            .dfg
            .value_type(operands[0])
            .unwrap_or_else(|| ctx.i32_ty())
    };

    let op = match op_name {
        "icmp" | "fcmp" if cond.is_some() => build_cmp(op_name, cond.unwrap())?,
        "trunc" => {
            // 整数→Ireduce、浮点→Ftrunc（按源类型分发）
            if operands
                .first()
                .and_then(|v| fb.func.dfg.value_type(*v))
                .is_some_and(|t| ctx.is_float(t))
            {
                Opcode::Ftrunc
            } else {
                Opcode::Ireduce
            }
        }
        _ => llvm_mapping::opcode(op_name).map_err(ParseError::Semantic)?,
    };
    // 向量精化（add <4 x i32> → Vadd）
    let op = if ctx.is_vector(result_ty) {
        llvm_mapping::vector_op(op)
    } else {
        op
    };
    // 转换指令的目标类型立即数
    let immediates: Vec<Immediate> = if matches!(
        op,
        Opcode::Uextend | Opcode::Sextend | Opcode::Ireduce | Opcode::Ftrunc | Opcode::Bitcast
    ) {
        vec![Immediate::Type(
            inst.args
                .get(1)
                .map(|a| to_type(&a.ty, ctx))
                .unwrap_or_else(|| ctx.i32_ty()),
        )]
    } else {
        vec![]
    };

    let v = fb
        .irb(block)
        .emit1(op, operands, immediates, result_ty, InstFlags::NONE);
    if let Some(r) = inst.result.as_ref() {
        bind_result(r, Some(v), fb, value_map)?;
    }
    Ok(())
}

fn build_cmp(op: &str, cond: &str) -> Result<Opcode, ParseError> {
    if op == "icmp" {
        let cc = llvm_mapping::int_cc(cond).map_err(ParseError::Semantic)?;
        Ok(Opcode::Icmp { cond: cc })
    } else {
        let cc = llvm_mapping::float_cc(cond).map_err(ParseError::Semantic)?;
        Ok(Opcode::Fcmp { cond: cc })
    }
}

/// 结果绑定：SSA 唯一性检查（同名重复定义报错）+ bind_name。
fn bind_result(
    name: &str,
    val: Option<Value>,
    fb: &mut FunctionBuilder,
    value_map: &mut HashMap<String, Value>,
) -> Result<(), ParseError> {
    let Some(v) = val else {
        return Ok(());
    };
    if value_map.contains_key(name) {
        return Err(ParseError::Semantic(format!(
            "SSA value %{} defined more than once",
            name.trim_start_matches('%')
        )));
    }
    fb.bind_name(v, name.trim_start_matches('%'));
    value_map.insert(name.to_string(), v);
    Ok(())
}

// ── 操作数 → Value ──

fn operand_to_value(
    op: &ParsedOperand,
    ctx: &TypeContext,
    value_map: &HashMap<String, Value>,
    func_refs: &HashMap<String, FuncRef>,
    fb: &mut FunctionBuilder,
    block: Block,
) -> Result<Value, ParseError> {
    let ty = to_type(&op.ty, ctx);
    let v = match &op.op {
        Operand::Local(name) => value_map
            .get(name)
            .copied()
            .ok_or_else(|| ParseError::Semantic(format!("undefined value {name}")))?,
        Operand::Global(name) => {
            // 函数引用（call 目标）或全局地址
            if let Some(fr) = func_refs.get(name) {
                // 函数指针（间接调用场景用 GlobalAddr 近似）
                fb.irb(block).global_addr(GlobalId(fr.0))
            } else {
                fb.irb(block).global_addr(GlobalId(0))
            }
        }
        Operand::Int(n) => fb.irb(block).iconst(*n, ty),
        Operand::UInt(n) => fb.irb(block).iconst(*n as i64, ty),
        Operand::Float(fv) => fb.irb(block).fconst(fv.to_bits(), ty),
        Operand::Bool(b) => fb.irb(block).iconst(if *b { 1 } else { 0 }, ty),
        Operand::Null => fb.irb(block).iconst(0, ty),
        Operand::Undef => fb
            .irb(block)
            .emit1(Opcode::Undef, vec![], vec![], ty, InstFlags::NONE),
        Operand::Poison => fb
            .irb(block)
            .emit1(Opcode::Poison, vec![], vec![], ty, InstFlags::NONE),
    };
    Ok(v)
}

// ── 终结符 ──

fn build_terminator(
    term: &ParsedTerminator,
    fb: &mut FunctionBuilder,
    ctx: &TypeContext,
    value_map: &HashMap<String, Value>,
    block_map: &HashMap<String, Block>,
    block: Block,
) -> Result<(), ParseError> {
    match term {
        ParsedTerminator::Return(ops) => {
            let vals: Vec<Value> = ops
                .iter()
                .map(|o| operand_to_value(o, ctx, value_map, &HashMap::new(), fb, block))
                .collect::<Result<_, _>>()?;
            fb.irb(block).ret(&vals);
        }
        ParsedTerminator::Jump(target) => {
            let t = block_map
                .get(target)
                .ok_or_else(|| ParseError::Semantic(format!("undefined block {target}")))?;
            fb.irb(block).jump(*t, &[]);
        }
        ParsedTerminator::Branch(cond, t, f) => {
            let t = block_map
                .get(t)
                .ok_or_else(|| ParseError::Semantic(format!("undefined block {t}")))?;
            let f = block_map
                .get(f)
                .ok_or_else(|| ParseError::Semantic(format!("undefined block {f}")))?;
            if let Some(c) = cond {
                let c = operand_to_value(c, ctx, value_map, &HashMap::new(), fb, block)?;
                fb.irb(block).branch(c, *t, &[], *f, &[]);
            } else {
                fb.irb(block).jump(*t, &[]);
            }
        }
        ParsedTerminator::Switch(cond, default, cases) => {
            let c = operand_to_value(cond, ctx, value_map, &HashMap::new(), fb, block)?;
            let d = block_map
                .get(default)
                .ok_or_else(|| ParseError::Semantic(format!("undefined block {default}")))?;
            let cases: Vec<(i64, Block, &[Value])> = cases
                .iter()
                .map(|(n, b)| {
                    block_map
                        .get(b)
                        .map(|bb| (*n, *bb, &[][..]))
                        .ok_or_else(|| ParseError::Semantic(format!("undefined block {b}")))
                })
                .collect::<Result<_, _>>()?;
            fb.irb(block).switch(c, *d, &cases);
        }
        ParsedTerminator::Unreachable => {
            // LLVM：unreachable 是 terminator，不生成 Trap 指令（round-trip 结构一致）
            fb.irb(block).unreachable();
        }
    }
    Ok(())
}
