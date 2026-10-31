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
    referenced: &mut Vec<(String, Instance<'tcx>)>,
    // 目标对应的 ISA 名——由调用方（`backend::codegen_crate`）选定并校验过目标。
    // **本函数不再二次推导**：目标校验只能有一个失败点（缺口守卫要求 ID 恰好一处）。
    isa_name: &str,
) -> Result<CompiledFunction, ForgeError> {
    // 按目标三元组选择 ISA 后端（仅 x86_64 支持——唯一的发行后端；
    // aarch64/riscv64 v11 后端已随 v11 语法层删除）
    crate::trace::set_panic_context(Some(format!(
        "lowering {} [{}]",
        tcx.def_path_str(instance.def_id()),
        crate::lower::symbol_name_for_instance(tcx, instance),
    )));

    // `#[unsafe(naked)]`：既不建栈槽（`LowerCtxt::new` 的 naked 形参），
    // 也不发射序/尾声（IR `NO_FRAME`）——两条都到位才是"没有编译器痕迹"的函数。
    let naked = tcx
        .codegen_fn_attrs(instance.def_id())
        .flags
        .contains(rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::NAKED);
    // 定义侧调用约定先 fail-closed（`extern "sysv64"` 之类不是 Win64 的约定，
    // 按 C 打包会静默错值）。调用点另有一查，覆盖 `extern` 块里声明的外部函数。
    crate::abi::check_conv_supported(
        tcx,
        instance,
        &format!("定义 `{}`", tcx.def_path_str(instance.def_id())),
    )?;
    // C 变参**定义** fail-closed（`[cap:VARARGS_DEF]`）：真做需要被调方序言建寄存器保存区，
    // 见 `abi::varargs_def_unsupported` 的说明。不这样拦，用户只会看到一串
    // `unsupported intrinsic: va_end`（且此前还会先 panic 在 ABI 计数断言上）。
    if crate::abi::fn_abi_is_c_variadic(tcx, instance) {
        return Err(crate::abi::varargs_def_unsupported(&format!(
            "定义 `{}`",
            tcx.def_path_str(instance.def_id())
        )));
    }
    let lctx = LowerCtxt::new(tcx, instance, body, func_refs, referenced, naked);
    if crate::trace::trace_enabled("GLOBAL") {
        eprintln!(
            "[forge] lowering fn {:?}",
            tcx.def_path_str(instance.def_id())
        );
    }
    let mut func = lctx.lower_body(body)?;
    // `#[unsafe(naked)]`：函数体只有汇编、**没有**编译器序/尾声与栈帧 ⇒ 打上 `NO_FRAME`，
    // 由发射侧跳过序言/尾声（`pipeline/emission.rs` 里那条硬约束会兜住"naked 却要栈槽"）。
    if naked {
        func.attributes.set(FunctionAttributes::NO_FRAME);
    }
    // IR 层诊断（用户建议：先确认后端 IR 是否正确，再查字节码）：
    // FORGE_TRACE_IR=1 时 dump forge-ir（LLVM 风格）——确认 StackAddr/Store
    // 的槽偏移与构造/判别的一致性（enumerate None 路径错位排查）。
    if crate::trace::trace_enabled("IR") {
        eprintln!(
            "[forge] === IR dump: {} ===\n{}",
            tcx.def_path_str(instance.def_id()),
            code_forge::ir::text::function_to_string(&func)
        );
        // ret values 数诊断（ScalarPair 双返回：被调方 Return 的 values 数
        // 决定是否生成 RDX 返回 mov——enumerate None 路径 r3.b 残留排查）
        for bi in 0..func.dfg.block_count() {
            if let Some(values) = func.dfg.term_return_values(Block::new(bi as u32)) {
                eprintln!("[forge] IR ret b{bi} values={}", values.len());
            }
        }
    }
    // 注册**两件事**，缺一不可：
    // ① 生成物注册（ISA 表 + reloc patcher）——`backend::<isa>::ensure_registered`；
    // ② **编译管线工厂**——`backend::pipeline_hooks::ensure_registered()`。
    // 漏掉 ② 时任何编译入口都会报 "backend: Unsupported: ISA `x86_64` 未注册编译管线：
    // 宿主需调用 forge_isa_runtime::register_pipeline(...)"（CLI 经 backend dll 调用时最先撞上，
    // 2026-10-07 实测：cargo-forge 的 6 条 cli_tests 全部因此红）。
    code_forge::backend::pipeline_hooks::ensure_registered();
    code_forge::backend::x86::ensure_registered();
    let r = compile_with_isa(&func, isa_name);
    crate::trace::set_panic_context(None);
    r
}

// ISA 后端选择
// ============================================================

/// 根据目标三元组自动注册对应的 ISA 后端。v28 M2 起由 `isa/manifest.toml`
/// 生成注册表：这里只做“全部注册”（幂等且便宜），不再写目标分支。
pub fn auto_register_isa_for_target(_target_triple: &str) {
    // 管线注册与目标无关（manifest 里的后端一起注册）——只注册生成物、不注册管线
    // ⇒ 编译期 "未注册编译管线"。
    code_forge::backend::pipeline_hooks::ensure_registered();
    code_forge::backend::arch::ensure_all();
}

/// 从目标三元组确定 ISA 名称（Registry 注册名 = manifest 的 `name`）。
///
/// `None` = 本后端**没有**该目标的 ISA 后端。
///
/// v27 M4-T1：**不再回落 x86_64**。回落的后果是给 riscv64 之类的目标静默编出 x86
/// 机器码—— 这正是仓库明令禁止的"静默降级"。调用方必须把 `None` 变成明确的诊断。
pub fn isa_name_for_target(target_triple: &str) -> Option<&'static str> {
    if let Some(name) = code_forge::backend::arch::isa_for_target(target_triple) {
        return Some(name);
    }
    // v28 M3：动态插件声明的目标（`export_isa_plugin!`）。名字来自插件，量级很小；
    // 泄漏成 'static 供既有 `&'static str` 调用面复用。
    code_forge::backend::target_registry::isa_for_target(target_triple)
        .map(|s| &*Box::leak(s.into_boxed_str()))
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
    let cf = compiler.compile(func)?;
    // 字节码层诊断（用户建议：IR 层确认后查字节码）：
    // FORGE_TRACE_BC=1 时 hex dump 机器码 + reloc（配 llvm-objdump 反汇编对照）。
    if crate::trace::trace_enabled("BC") {
        let hex: Vec<String> = cf
            .code
            .iter()
            .take(4096)
            .map(|b| format!("{b:02x}"))
            .collect();
        eprintln!(
            "[forge] BC {} code_size={} relocs={} bytes=[{}]",
            func.name,
            cf.code_size,
            cf.relocations.len(),
            hex.join(" ")
        );
        for r in &cf.relocations {
            eprintln!(
                "[forge] BC reloc {} @0x{:x} addend={}",
                r.symbol, r.offset, r.addend
            );
        }
    }
    Ok(cf)
}
