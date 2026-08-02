//! rustc codegen backend powered by `code-forge`.
//!
//! Usage:
//! ```bash
//! cargo +nightly build --release
//! rustc +nightly -Zcodegen-backend=./target/release/forge_rustc.dll hello.rs
//! ```

#![feature(rustc_private)]
#![feature(box_patterns)]

extern crate rustc_codegen_ssa;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_metadata;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;
extern crate rustc_symbol_mangling;
extern crate rustc_target;

use std::any::Any;
use std::collections::HashMap;

use rustc_codegen_ssa::traits::CodegenBackend;
use rustc_codegen_ssa::{CompiledModule, CompiledModules, CrateInfo, ModuleKind};
use rustc_data_structures::fx::FxIndexMap;
use rustc_middle::dep_graph::{WorkProduct, WorkProductId};
use rustc_middle::mir::mono::MonoItem;
use rustc_middle::mir::{self, Body, Operand, Rvalue, StatementKind, TerminatorKind};
use rustc_middle::ty::{self, Instance, TyCtxt};
use rustc_session::Session;
use rustc_session::config::OutputFilenames;

use code_forge::backend::*;
use code_forge::forge_object::ObjectWriter;
use code_forge::forge_object::TargetConfig;
use code_forge::ir::*;

// ============================================================
// 导出符号
// ============================================================

#[unsafe(no_mangle)]
pub fn __rustc_codegen_backend() -> Box<dyn CodegenBackend> {
    Box::new(CodegenLibBackend)
}

/// 返回 codegen backend 名称——不泄漏 `rustc_private` 类型的公开 API，
/// 供下游 crate（如 forge-tests）在不启用 `rustc_private` 的情况下验证 backend 存在。
pub fn codegen_backend_name() -> &'static str {
    CodegenLibBackend.name()
}

// ============================================================
// CodegenLibBackend
// ============================================================

struct CodegenLibBackend;

impl CodegenBackend for CodegenLibBackend {
    fn name(&self) -> &'static str {
        "code-forge"
    }

    fn target_cpu(&self, _sess: &Session) -> String {
        "generic".to_string()
    }

    fn codegen_crate<'tcx>(&self, tcx: TyCtxt<'tcx>, crate_info: &CrateInfo) -> Box<dyn Any> {
        let _ = crate_info;

        // 1. 从环境变量加载外部 ISA 插件
        let _plugin_loader = code_forge::forge_plugin::PluginLoader::load_from_env();

        // 2. 根据目标三元组自动注册内置 ISA 后端
        let target_triple = format!("{}", tcx.sess.opts.target_triple);
        auto_register_isa_for_target(&target_triple);

        let mut compiled_modules = Vec::new();
        let outdir = tcx.output_filenames(()).with_extension("").to_path_buf();
        let outdir = outdir
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();

        // 收集需要编译的实例
        let instances = collect_instances(tcx);

        if instances.is_empty() {
            compiled_modules.push(CompiledModule {
                name: "empty".to_string(),
                kind: ModuleKind::Regular,
                object: None,
                dwarf_object: None,
                bytecode: None,
                assembly: None,
                llvm_ir: None,
                links_from_incr_cache: Vec::new(),
            });
            return Box::new(CompiledModules {
                modules: compiled_modules,
                allocator_module: None,
            });
        }

        // 创建对象文件写入器 (使用完整版本以支持 .data section)
        let config = TargetConfig::host().unwrap_or_default();

        let mut object_writer = ObjectWriter::new_text_only(&config).expect("create object writer");

        // 从 rlib 中提取 UNDEFINED 符号并注入 ret 桩
        inject_undefined_symbols(tcx, &mut object_writer);

        for instance in &instances {
            let def_id = instance.def_id();
            let is_local = def_id.is_local();

            // 获取符号名 — 使用定义 crate 的 CrateNum 以确保
            // 外部实例（如 allocator）的哈希与 rlib 一致
            let instantiating_crate = if is_local {
                rustc_hir::def_id::LOCAL_CRATE
            } else {
                def_id.krate
            };
            let sym_name = rustc_symbol_mangling::symbol_name_for_instance_in_crate(
                tcx,
                *instance,
                instantiating_crate,
            );

            // 只有本地实例有 MIR body 可用
            if is_local && tcx.is_mir_available(def_id.expect_local()) {
                let body = tcx.instance_mir(instance.def);

                match lower_and_compile(tcx, instance, body) {
                    Ok(compiled_func) => {
                        let _ = object_writer.add_function(&sym_name, &compiled_func);
                        // main 函数需要 C 名称别名（链接器入口点）
                        if tcx.item_name(def_id).as_str() == "main" {
                            let _ = object_writer.add_function("main", &compiled_func);
                        }
                    }
                    Err(e) => {
                        tcx.dcx()
                            .warn(format!("code-forge: failed to compile '{sym_name}': {e}"));
                        let stub = make_ret_stub();
                        let _ = object_writer.add_function(&sym_name, &stub);
                        if tcx.item_name(def_id).as_str() == "main" {
                            let _ = object_writer.add_function("main", &stub);
                        }
                    }
                }
            } else {
                // 外部实例（如 allocator 函数）没有可访问的 MIR
                // 提供最小 ret 桩以确保符号存在
                let _ = object_writer.add_function(&sym_name, &make_ret_stub());
            }
        }

        // 写入对象文件到磁盘
        let obj_path = outdir.join("forge_codegen_output.o");
        match object_writer.write_to_file(&obj_path) {
            Ok(()) => {
                compiled_modules.push(CompiledModule {
                    name: "forge_codegen_output".to_string(),
                    kind: ModuleKind::Regular,
                    object: Some(obj_path),
                    dwarf_object: None,
                    bytecode: None,
                    assembly: None,
                    llvm_ir: None,
                    links_from_incr_cache: Vec::new(),
                });
            }
            Err(e) => {
                tcx.dcx()
                    .err(format!("code-forge: failed to write object file: {e}"));
            }
        }

        Box::new(CompiledModules {
            modules: compiled_modules,
            allocator_module: None,
        })
    }

    fn join_codegen(
        &self,
        ongoing_codegen: Box<dyn Any>,
        _sess: &Session,
        _outputs: &OutputFilenames,
    ) -> (CompiledModules, FxIndexMap<WorkProductId, WorkProduct>) {
        let compiled_modules = *ongoing_codegen
            .downcast::<CompiledModules>()
            .expect("codegen results");

        let mut work_products = FxIndexMap::default();

        for module in &compiled_modules.modules {
            if let Some(ref obj_path) = module.object {
                let wp_id = WorkProductId::from_cgu_name(&module.name);
                let mut saved = rustc_data_structures::unord::UnordMap::default();
                saved.insert("o".to_string(), obj_path.to_string_lossy().to_string());
                work_products.insert(
                    wp_id,
                    WorkProduct {
                        cgu_name: module.name.clone(),
                        saved_files: saved,
                    },
                );
            }
        }

        (compiled_modules, work_products)
    }
    // link() 使用 trait 的默认实现 (link_binary)
}

// ============================================================
// 实例收集
// ============================================================

fn collect_instances<'tcx>(tcx: TyCtxt<'tcx>) -> Vec<Instance<'tcx>> {
    // 使用 rustc 单态化收集 — 获取所有需要代码生成的实例
    let partitions = tcx.collect_and_partition_mono_items(());

    // 遍历所有 codegen unit 收集实例
    partitions
        .codegen_units
        .iter()
        .flat_map(|cgu| cgu.items().iter().map(|(item, _)| *item))
        .filter_map(|item| match item {
            MonoItem::Fn(instance) => Some(instance),
            _ => None,
        })
        .collect()
}

/// 创建一个最小的 ret 桩函数（0xC3 = ret）。
fn make_ret_stub() -> CompiledFunction {
    CompiledFunction {
        code_size: 1,
        code: vec![0xC3], // ret
        relocations: vec![],
    }
}

/// 注入必需的运行时符号桩（allocator + panic/unwind）。
///
/// 当前使用硬编码符号名（与 nightly 2026-03-07 的 rlib 一致）。
///
/// 如需动态生成：扫描 `instances` 中 `def_id.is_local() == false` 的实例，
/// 使用 `rustc_symbol_mangling::symbol_name_for_instance_in_crate` 获取符号名。
/// 但注意 allocator 符号（__rust_alloc 等）可能不在 instances 中，
/// 它们是编译器内部生成的特殊符号，需通过 other: () 分支捕获。
fn inject_undefined_symbols(_tcx: TyCtxt<'_>, writer: &mut ObjectWriter<'_>) {
    let _ = _tcx;
    // 仅注入 rlib 中真正 UNDEFINED 的符号（不要重复定义 libstd 已有的）
    let known_symbols = &[
        "_RNvCs786P5YrMXVD_7___rustc12___rust_alloc",
        "_RNvCs786P5YrMXVD_7___rustc14___rust_dealloc",
        "_RNvCs786P5YrMXVD_7___rustc14___rust_realloc",
        "_RNvCs786P5YrMXVD_7___rustc19___rust_alloc_zeroed",
        "_RNvCs786P5YrMXVD_7___rustc35___rust_no_alloc_shim_is_unstable_v2",
    ];
    for name in known_symbols {
        let stub = make_ret_stub();
        let _ = writer.add_function(name, &stub);
    }
}
// ============================================================
// MIR → code-forge IR 转换
// ============================================================

struct LowerCtxt<'tcx> {
    tcx: TyCtxt<'tcx>,
    builder: FunctionBuilder,
    locals: HashMap<mir::Local, Value>,
    blocks: HashMap<mir::BasicBlock, Block>,
}

impl<'tcx> LowerCtxt<'tcx> {
    fn new(tcx: TyCtxt<'tcx>, instance: &Instance<'tcx>, body: &Body<'tcx>) -> Self {
        let sig = build_signature(tcx, instance, body);
        let name = tcx.item_name(instance.def_id()).to_string();
        LowerCtxt {
            tcx,
            builder: FunctionBuilder::new(&name, TypeContext::new(), sig),
            locals: HashMap::new(),
            blocks: HashMap::new(),
        }
    }

    fn lower_body(mut self, body: &Body<'tcx>) -> Result<Function, CompileError> {
        // 1. 为入口块创建带参数（函数参数）的 block
        let entry_args: Vec<(TypeId, String)> = body
            .args_iter()
            .map(|local| {
                let ty = body.local_decls[local].ty;
                let codegen_ty = map_type(ty, self.tcx).unwrap_or(TypeId::I32);
                (codegen_ty, format!("arg_{}", local.index()))
            })
            .collect();

        let entry_bb = mir::BasicBlock::from_usize(0);
        let (entry_blk, entry_params) = self.builder.create_block_with_params(
            &entry_args
                .iter()
                .map(|(t, s)| (*t, s.as_str()))
                .collect::<Vec<_>>(),
        );
        self.blocks.insert(entry_bb, entry_blk);

        // 映射入口参数到 MIR 局部变量
        let arg_locals: Vec<mir::Local> = body.args_iter().collect();
        for (i, &local) in arg_locals.iter().enumerate() {
            if i < entry_params.len() {
                self.locals.insert(local, entry_params[i]);
            }
        }

        // 2. 为其他 MIR 基本块创建 block
        for (bb_idx, _) in body.basic_blocks.iter().enumerate().skip(1) {
            let bb = mir::BasicBlock::from_usize(bb_idx);
            if !self.blocks.contains_key(&bb) {
                let blk = self.builder.create_block();
                self.blocks.insert(bb, blk);
            }
        }

        // 3. 切换到入口块，为其他局部变量创建占位 Value
        self.builder.switch_to_block(entry_blk);
        for (i, local_decl) in body.local_decls.iter().enumerate() {
            let local = mir::Local::from_usize(i);
            if !self.locals.contains_key(&local) {
                let ty = map_type(local_decl.ty, self.tcx).unwrap_or(TypeId::I32);
                let val = self.builder.iconst(0, ty);
                self.locals.insert(local, val);
            }
        }

        // 4. 翻译基本块
        for (bb_idx, bb_data) in body.basic_blocks.iter().enumerate() {
            let bb = mir::BasicBlock::from_usize(bb_idx);
            let block_id = self.blocks[&bb];
            self.builder.switch_to_block(block_id);

            for stmt in &bb_data.statements {
                self.lower_statement(stmt)?;
            }

            match &bb_data.terminator().kind {
                TerminatorKind::Return => {
                    let ret_local = mir::Local::from_usize(0);
                    if let Some(&val) = self.locals.get(&ret_local) {
                        self.builder.ret(&[val]);
                    } else {
                        self.builder.ret(&[]);
                    }
                }
                TerminatorKind::Goto { target } => {
                    let tgt = self.blocks[target];
                    self.builder.jump(tgt, &[]);
                }
                TerminatorKind::SwitchInt { discr, targets } => {
                    let discr_val = self.lower_operand(discr)?;
                    let otherwise = targets.otherwise();
                    let targets_vec: Vec<_> = targets.iter().collect();

                    if targets_vec.len() == 1 && targets_vec[0].0 == 0 {
                        // 布尔条件：switchInt(_1) -> [0: false_bb, otherwise: true_bb]
                        let false_blk = self.blocks[&targets_vec[0].1];
                        let true_blk = self.blocks[&otherwise];
                        self.builder
                            .branch(discr_val, true_blk, &[], false_blk, &[]);
                    } else {
                        // 通用 case：if-else 链
                        let mut cur_blk = block_id;
                        for &(value, tgt_bb) in &targets_vec {
                            self.builder.switch_to_block(cur_blk);
                            let const_val = self.builder.iconst_i32(value as i32);
                            let eq = self.builder.icmp(IntCC::Equal, discr_val, const_val);
                            let tgt_blk = self.blocks[&tgt_bb];
                            let next_blk = self.builder.create_block();
                            // create_block() auto-switches cur_block, switch back
                            self.builder.switch_to_block(cur_blk);
                            self.builder.branch(eq, tgt_blk, &[], next_blk, &[]);
                            cur_blk = next_blk;
                        }
                        let otherwise_blk = self.blocks[&otherwise];
                        self.builder.switch_to_block(cur_blk);
                        self.builder.jump(otherwise_blk, &[]);
                    }
                }
                TerminatorKind::Call {
                    func,
                    args,
                    destination,
                    target,
                    ..
                } => {
                    let func_val = self.lower_operand(func)?;
                    let arg_vals: Result<Vec<_>, _> = args
                        .iter()
                        .map(|arg| self.lower_operand(&arg.node))
                        .collect();
                    let args = arg_vals?;

                    let ret_ty = map_type(body.local_decls[destination.local].ty, self.tcx)
                        .unwrap_or(TypeId::VOID);

                    let ret_tys: Vec<TypeId> = if ret_ty == TypeId::VOID {
                        vec![]
                    } else {
                        vec![ret_ty]
                    };

                    let results = self.builder.call_indirect(func_val, &args, &ret_tys);

                    if !results.is_empty() {
                        self.locals.insert(destination.local, results[0]);
                    }

                    if let Some(target_bb) = target {
                        let tgt = self.blocks[target_bb];
                        self.builder.jump(tgt, &[]);
                    }
                }
                TerminatorKind::Unreachable => {
                    self.builder.unreachable();
                }
                TerminatorKind::Assert { cond, expected, .. } => {
                    let cond_val = self.lower_operand(cond)?;
                    let fail_blk = self.builder.create_block();
                    let merge_blk = self.builder.create_block();
                    // create_block() auto-switches cur_block, switch back to source
                    self.builder.switch_to_block(block_id);
                    if *expected {
                        self.builder.branch(cond_val, merge_blk, &[], fail_blk, &[]);
                        self.builder.switch_to_block(fail_blk);
                    } else {
                        self.builder.branch(cond_val, fail_blk, &[], merge_blk, &[]);
                        self.builder.switch_to_block(fail_blk);
                    }
                    self.builder.unreachable();
                }
                TerminatorKind::Yield { .. } => {
                    self.builder.unreachable();
                }
                _ => {
                    self.builder.unreachable();
                }
            }
        }

        Ok(self.builder.finish())
    }

    fn lower_statement(
        &mut self,
        stmt: &rustc_middle::mir::Statement<'tcx>,
    ) -> Result<(), CompileError> {
        match &stmt.kind {
            StatementKind::Assign(box (place, rvalue)) => {
                let val = self.lower_rvalue(rvalue)?;
                self.locals.insert(place.local, val);
            }
            StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {}
            _ => {}
        }
        Ok(())
    }

    fn lower_rvalue(&mut self, rvalue: &Rvalue<'tcx>) -> Result<Value, CompileError> {
        match rvalue {
            Rvalue::Use(op) => self.lower_operand(op),
            Rvalue::BinaryOp(bin_op, box (op1, op2)) => {
                let lhs = self.lower_operand(op1)?;
                let rhs = self.lower_operand(op2)?;
                self.lower_binary_op(*bin_op, lhs, rhs)
            }
            Rvalue::UnaryOp(un_op, op) => {
                let val = self.lower_operand(op)?;
                self.lower_unary_op(*un_op, val)
            }
            Rvalue::Cast(cast_kind, op, to_ty) => {
                let val = self.lower_operand(op)?;
                let to_type = map_type(*to_ty, self.tcx)?;
                match cast_kind {
                    rustc_middle::mir::CastKind::IntToInt
                    | rustc_middle::mir::CastKind::FloatToInt
                    | rustc_middle::mir::CastKind::IntToFloat => {
                        Ok(self.builder.ireduce(val, to_type))
                    }
                    rustc_middle::mir::CastKind::PtrToPtr
                    | rustc_middle::mir::CastKind::FnPtrToPtr => Ok(val),
                    _ => Ok(val),
                }
            }
            Rvalue::Ref(..) => Ok(self.builder.iconst(0, TypeId::PTR)),
            Rvalue::Discriminant(_) => Ok(self.builder.iconst_i32(0)),
            Rvalue::Repeat(op, _len) => self.lower_operand(op),
            Rvalue::Aggregate(_kind, _fields) => {
                // 结构体/元组构造 → 返回 0 占位
                Ok(self.builder.iconst_i32(0))
            }
            _ => Err(CompileError::Unsupported(format!(
                "unsupported rvalue: {:?}",
                rvalue
            ))),
        }
    }

    fn lower_operand(&mut self, operand: &Operand<'tcx>) -> Result<Value, CompileError> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => {
                let val = self.get_or_create_local(place.local)?;
                for proj in place.projection.iter() {
                    match proj {
                        mir::ProjectionElem::Field(_idx, _ty) => {
                            // 聚合字段访问 — 对于 (value, bool) 等，直接使用基础值
                        }
                        mir::ProjectionElem::Deref => {
                            // 指针解引用 — 简化处理
                        }
                        _ => {
                            return Err(CompileError::Unsupported(format!(
                                "unsupported projection: {:?}",
                                proj
                            )));
                        }
                    }
                }
                Ok(val)
            }
            Operand::Constant(constant) => {
                let ty = map_type(constant.const_.ty(), self.tcx)?;
                // 尝试求值标量常量 — 根据类型大小处理
                let val = constant
                    .const_
                    .try_to_scalar_int()
                    .map(|s| match ty.bits().div_ceil(8) {
                        1 => s.to_i8() as i64,
                        2 => s.to_i16() as i64,
                        4 => s.to_i32() as i64,
                        _ => s.to_i64(),
                    })
                    .unwrap_or(0);
                Ok(self.builder.iconst(val, ty))
            }
            Operand::RuntimeChecks(_) => Ok(self.builder.iconst_i32(0)),
        }
    }

    fn lower_binary_op(
        &mut self,
        op: mir::BinOp,
        lhs: Value,
        rhs: Value,
    ) -> Result<Value, CompileError> {
        match op {
            mir::BinOp::Add | mir::BinOp::AddWithOverflow | mir::BinOp::AddUnchecked => {
                Ok(self.builder.iadd(lhs, rhs))
            }
            mir::BinOp::Sub | mir::BinOp::SubWithOverflow | mir::BinOp::SubUnchecked => {
                Ok(self.builder.isub(lhs, rhs))
            }
            mir::BinOp::Mul | mir::BinOp::MulWithOverflow | mir::BinOp::MulUnchecked => {
                Ok(self.builder.imul(lhs, rhs))
            }
            mir::BinOp::Div => Ok(self.builder.sdiv(lhs, rhs)),
            mir::BinOp::Rem => Ok(self.builder.urem(lhs, rhs)),
            mir::BinOp::BitAnd => Ok(self.builder.band(lhs, rhs)),
            mir::BinOp::BitOr => Ok(self.builder.bor(lhs, rhs)),
            mir::BinOp::BitXor => Ok(self.builder.bxor(lhs, rhs)),
            mir::BinOp::Shl | mir::BinOp::ShlUnchecked => Ok(self.builder.ishl(lhs, rhs)),
            mir::BinOp::Shr | mir::BinOp::ShrUnchecked => Ok(self.builder.ushr(lhs, rhs)),
            mir::BinOp::Eq => Ok(self.builder.icmp(IntCC::Equal, lhs, rhs)),
            mir::BinOp::Ne => Ok(self.builder.icmp(IntCC::NotEqual, lhs, rhs)),
            mir::BinOp::Lt => Ok(self.builder.icmp(IntCC::SignedLessThan, lhs, rhs)),
            mir::BinOp::Le => Ok(self.builder.icmp(IntCC::SignedLessThanOrEqual, lhs, rhs)),
            mir::BinOp::Gt => Ok(self.builder.icmp(IntCC::SignedGreaterThan, lhs, rhs)),
            mir::BinOp::Ge => Ok(self.builder.icmp(IntCC::SignedGreaterThanOrEqual, lhs, rhs)),
            _ => Err(CompileError::Unsupported(format!(
                "unsupported binary op: {:?}",
                op
            ))),
        }
    }

    fn lower_unary_op(&mut self, op: mir::UnOp, val: Value) -> Result<Value, CompileError> {
        match op {
            mir::UnOp::Not => Ok(self.builder.bnot(val)),
            mir::UnOp::Neg => {
                let zero = self.builder.iconst_i32(0);
                Ok(self.builder.isub(zero, val))
            }
            _ => Err(CompileError::Unsupported(format!(
                "unsupported unary op: {:?}",
                op
            ))),
        }
    }

    fn get_or_create_local(&mut self, local: mir::Local) -> Result<Value, CompileError> {
        if let Some(&val) = self.locals.get(&local) {
            return Ok(val);
        }
        let new_val = self.builder.iconst_i32(0);
        self.locals.insert(local, new_val);
        Ok(new_val)
    }
}

// ============================================================
// 函数签名构造
// ============================================================

fn build_signature<'tcx>(
    tcx: TyCtxt<'tcx>,
    _instance: &Instance<'tcx>,
    body: &Body<'tcx>,
) -> FunctionSignature {
    // 提取参数类型
    let params: Vec<(TypeId, String)> = body
        .args_iter()
        .map(|local| {
            let ty = body.local_decls[local].ty;
            let codegen_ty = map_type(ty, tcx).unwrap_or(TypeId::I32);
            (codegen_ty, format!("arg_{}", local.index()))
        })
        .collect();

    // 提取返回值类型
    let return_ty = body.return_ty();
    let returns = if return_ty.is_unit() || return_ty.is_never() {
        vec![]
    } else {
        vec![map_type(return_ty, tcx).unwrap_or(TypeId::I32)]
    };

    FunctionSignature::new(
        &params
            .iter()
            .map(|(t, s)| (*t, s.as_str()))
            .collect::<Vec<_>>(),
        &returns,
    )
}

// ============================================================
// 类型映射
// ============================================================

fn map_type(ty: rustc_middle::ty::Ty<'_>, tcx: TyCtxt<'_>) -> Result<TypeId, CompileError> {
    match ty.kind() {
        ty::TyKind::Bool => Ok(TypeId::BOOL),
        ty::TyKind::Int(ty::IntTy::I8) => Ok(TypeId::I8),
        ty::TyKind::Int(ty::IntTy::I16) => Ok(TypeId::I16),
        ty::TyKind::Int(ty::IntTy::I32) => Ok(TypeId::I32),
        ty::TyKind::Int(ty::IntTy::I64) => Ok(TypeId::I64),
        ty::TyKind::Int(ty::IntTy::Isize) => match tcx.data_layout.pointer_size().bits() {
            64 => Ok(TypeId::I64),
            32 => Ok(TypeId::I32),
            _ => Ok(TypeId::I64),
        },
        ty::TyKind::Uint(ty::UintTy::U8) => Ok(TypeId::I8),
        ty::TyKind::Uint(ty::UintTy::U16) => Ok(TypeId::I16),
        ty::TyKind::Uint(ty::UintTy::U32) => Ok(TypeId::I32),
        ty::TyKind::Uint(ty::UintTy::U64) => Ok(TypeId::I64),
        ty::TyKind::Uint(ty::UintTy::Usize) => match tcx.data_layout.pointer_size().bits() {
            64 => Ok(TypeId::I64),
            32 => Ok(TypeId::I32),
            _ => Ok(TypeId::I64),
        },
        ty::TyKind::Float(ty::FloatTy::F32) => Ok(TypeId::F32),
        ty::TyKind::Float(ty::FloatTy::F64) => Ok(TypeId::F64),
        ty::TyKind::Char => Ok(TypeId::I32),
        ty::TyKind::Ref(..) | ty::TyKind::RawPtr(..) => Ok(TypeId::PTR),
        ty::TyKind::FnDef(..) | ty::TyKind::FnPtr(..) => Ok(TypeId::PTR),
        ty::TyKind::Tuple(tys) if tys.is_empty() => Ok(TypeId::VOID),
        ty::TyKind::Never => Ok(TypeId::VOID),
        // 零大小类型用 I8 占位（不占栈空间，仅用于类型系统）
        _ => {
            if ty.is_unit() {
                Ok(TypeId::VOID)
            } else {
                Ok(TypeId::PTR)
            }
        }
    }
}

// ============================================================
// 编译入口
// ============================================================

fn lower_and_compile<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: &Instance<'tcx>,
    body: &Body<'tcx>,
) -> Result<CompiledFunction, CompileError> {
    let lctx = LowerCtxt::new(tcx, instance, body);
    let func = lctx.lower_body(body)?;
    code_forge::backend::arch::x86_64::ensure_registered();
    compile_with_isa(&func, "x86_64")
}

// ============================================================
// ISA 后端选择
// ============================================================

/// 根据目标三元组自动注册对应的 ISA 后端。
pub fn auto_register_isa_for_target(target_triple: &str) {
    if target_triple.contains("x86_64") || target_triple.contains("amd64") {
        code_forge::backend::arch::x86_64::ensure_registered();
    }
}

/// 从目标三元组确定 ISA 名称。
pub fn isa_name_for_target(target_triple: &str) -> &'static str {
    if target_triple.contains("x86_64") || target_triple.contains("amd64") {
        "x86_64"
    } else if target_triple.contains("aarch64") {
        "aarch64"
    } else {
        "x86_64"
    }
}

/// 使用指定名称的 ISA 后端编译函数。
/// ISA 必须已通过 `register_backend!` 注册。
fn compile_with_isa(func: &Function, isa_name: &str) -> Result<CompiledFunction, CompileError> {
    let registry = Registry::global();
    let compiler = registry
        .lookup(isa_name)
        .ok_or_else(|| CompileError::BackendNotFound(isa_name.into()))?;
    compiler.compile(func)
}
