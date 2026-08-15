//! 编译入口：`lower_and_compile`（LowerCtxt → ISA 编译）与 ISA 后端选择。
//!
//! `lower_and_compile` 被 backend.rs 的 codegen_crate 调用；
//! `auto_register_isa_for_target` / `isa_name_for_target` 供 backend 与
//! alloc_runtime 使用。

use crate::error::ForgeError;
use crate::func_ref::FuncRefTable;
use crate::lower::LowerCtxt;
use crate::prelude::*;

/// 单个实例的完整编译管线：LowerCtxt::new → lower_body → ISA 编译。
pub(crate) fn lower_and_compile<'tcx, 'f>(
    tcx: TyCtxt<'tcx>,
    instance: &Instance<'tcx>,
    body: &'tcx Body<'tcx>,
    func_refs: &'f mut FuncRefTable,
) -> Result<CompiledFunction, ForgeError> {
    // 按目标三元组选择 ISA 后端（当前宿主为 x86_64；aarch64/riscv64 已注册
    // 但调用约定/指令覆盖未经端到端验证，见 README 支持矩阵）
    crate::trace::set_panic_context(Some(format!(
        "lowering {} [{}]",
        tcx.def_path_str(instance.def_id()),
        crate::lower::symbol_name_for_instance(tcx, instance),
    )));
    let target_triple = format!("{}", tcx.sess.opts.target_triple);
    let isa_name = isa_name_for_target(&target_triple);
    let lctx = LowerCtxt::new(tcx, instance, body, func_refs);
    if crate::trace::trace_enabled("GLOBAL") {
        eprintln!(
            "[forge] lowering fn {:?}",
            tcx.def_path_str(instance.def_id())
        );
    }
    let func = lctx.lower_body(body)?;
    code_forge::backend::arch::x86_64::ensure_registered();
    let r = compile_with_isa(&func, isa_name);
    crate::trace::set_panic_context(None);
    r
}

// ISA 后端选择
// ============================================================

/// 根据目标三元组自动注册对应的 ISA 后端。
pub fn auto_register_isa_for_target(target_triple: &str) {
    if target_triple.contains("x86_64") || target_triple.contains("amd64") {
        code_forge::backend::arch::x86_64::ensure_registered();
    } else if target_triple.contains("aarch64") {
        code_forge::backend::arch::aarch64::ensure_registered();
    } else if target_triple.contains("riscv64") {
        code_forge::backend::arch::riscv64::ensure_registered();
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
pub(crate) fn compile_with_isa(
    func: &Function,
    isa_name: &str,
) -> Result<CompiledFunction, ForgeError> {
    let registry = Registry::global();
    let compiler = registry
        .lookup(isa_name)
        .ok_or_else(|| ForgeError::BackendNotFound(isa_name.into()))?;
    Ok(compiler.compile(func)?)
}
