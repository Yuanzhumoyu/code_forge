//! 静态数据段（`MonoItem::Static`）求值与落段（P4.3）。
//!
//! 职责：static 初始值求值（`tcx.eval_static_initializer`）→ 符号名 →
//! `.data`/`.rodata` 字节 + 对齐。符号名与 `const {allocN}` 引用的
//! GlobalAddr 重定位（`G{N}` → 真实符号，func_ref.rs）一致。

use crate::prelude::*;

/// 可落段的静态数据：符号 + 字节 + 对齐 + 可变性。
pub(crate) struct StaticData {
    pub(crate) sym: String,
    pub(crate) data: Vec<u8>,
    pub(crate) align: u64,
    /// true → `.data`（可变 static）；false → `.rodata`（只读 static）。
    pub(crate) mutable: bool,
}

/// 求值 static 初始值并返回可落段数据。
///
/// 失败（求值错误）返回带上下文的 `Err(String)`，调用方（backend.rs）
/// 统一 `dcx().err` 报告——绝不产出残缺符号（`const {allocN}` 引用
/// 缺失会导致 static 读取错误）。
pub(crate) fn build_static_data<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: rustc_hir::def_id::DefId,
) -> Result<StaticData, String> {
    let instantiating_crate = if def_id.is_local() {
        rustc_hir::def_id::LOCAL_CRATE
    } else {
        def_id.krate
    };
    let sym = rustc_symbol_mangling::symbol_name_for_instance_in_crate(
        tcx,
        ty::Instance::mono(tcx, def_id),
        instantiating_crate,
    )
    .to_string();
    let alloc = tcx
        .eval_static_initializer(def_id)
        .map_err(|e| format!("failed to eval static '{sym}': {e:?}"))?;
    let inner = &*alloc.0;
    let size = inner.size().bytes_usize();
    let data = inner
        .inspect_with_uninit_and_ptr_outside_interpreter(0..size)
        .to_vec();
    let align = inner.align.bytes();
    Ok(StaticData {
        sym,
        data,
        align,
        mutable: tcx.is_mutable_static(def_id),
    })
}
