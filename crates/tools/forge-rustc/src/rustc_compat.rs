//! rustc 内部 API 版本适配层（P0.2 方案）。
//!
//! forge-rustc 通过 `#![feature(rustc_private)]` 直接使用 rustc 内部 crate，
//! 其 API 每周变动。所有已知漂移点集中记录在本模块，升级 nightly 时
//! 优先检查此处。适配版本：**rustc 1.100.0-nightly (0dfb098f3 2026-08-31)**。
//!
//! 升级流程：
//! 1. 运行 `cargo check -p forge-rustc`，收集编译错误；
//! 2. 对照下方「漂移点清单」逐项修正；新增漂移点追加到清单；
//! 3. 跑 `cargo test -p forge-rustc --test e2e` 确认行为无回归。
//!
//! # 漂移点清单（旧 API → 新 API）
//!
//! | 旧 API（≤2026-08-07） | 新 API（2026-08-31 / 1.100） | 位置 |
//! |---|---|---|
//! | `#![feature(box_patterns)]` + `box (a, b)` 模式 | **box_patterns 特性移除**——`Rvalue::BinaryOp`/`StatementKind::Assign`/`StatementKind::Intrinsic` 的 `Box` 包装改为直接字段（模式去掉 `box` 关键字） | rvalue.rs/statement.rs |
//! | `rustc_hir::LangItem`（DropGlue 等） | `rustc_hir::attrs::lang_items::LangItem`（lang_items 子模块提升路径） | lower/mod.rs |
//! | `integer_min/max` intrinsic 缺失 | rustc 1.100 新增 `integer_min`/`integer_max`（GlobalAlloc::realloc 等使用）；按 T 符号性选比较（core 文档 "signed or unsigned depending on T"）+ 原生 Select | lower/intrinsics.rs |
//! | rustc_middle::mir::mono::MonoItem（≤2026-03） | `rustc_middle::mono::MonoItem`（`mono` 提升为顶层模块） | lib.rs import |
//! | `CodegenBackend::codegen_crate(&self, tcx, crate_info)` | `codegen_crate(&self, tcx)`（删 `crate_info` 参数） | backend.rs |
//! | `join_codegen(&self, ongoing, sess, outputs)` | `join_codegen(&self, ongoing, sess, Option<&IncrCompSession>, outputs, crate_info)` → `(CompiledModules, WorkProductMap)` | backend.rs |
//! | `CompiledModule { … }` | 新增字段 `global_asm_object: Option<PathBuf>` | backend.rs |
//! | `ty::EarlyBinder::bind(value)` | `bind(cx: TyCtxt, value)`（interner 前置参数） | lib.rs lower_body |
//! | `BackendRepr::ScalarPair(a, b)`（tuple variant） | `ScalarPair { a, b, b_offset }`（struct variant） | layout.rs/types.rs |
//! | `VariantLayout.fields: FieldsShape` | `VariantLayout.field_offsets: IndexVec<FieldIdx, Size>`（`fields.count()`→`field_offsets.len()`、`fields.offset(i)`→`field_offsets[FieldIdx::new(i)]`；空判断用 `has_fields()`） | layout.rs |
//! | `TyKind::FnDef(def_id, substs)` 的 substs 直接 `as_slice()` | substs 类型变为 `Binder<TyCtxt, GenericArgsRef>`，先 `skip_binder()`（单态化实例无 bound vars，安全）再 `as_slice()` | call lowering |
//! | `LangItem::DropInPlace` | `LangItem::DropGlue`（2026-08-07）→ `rustc_hir::attrs::lang_items::LangItem::DropGlue`（2026-08-31） | call lowering |
//! | `Instance::resolve_drop_in_place(tcx, ty)` | `Instance::resolve_drop_glue(tcx, ty)`（内部 `tcx.require_lang_item(LangItem::DropGlue)` + `mk_args`） | vtable 生成 |
//! | `Rvalue::Use(Operand)` | `Rvalue::Use(Operand, WithRetag)`（retag 标志并入） | statement/rvalue lowering |
//! | `RangeInclusive`（niche_variants）直接 `.count()`/`.enumerate()` | 需 `.into_iter().count()`/`.enumerate()`（不再直接是 Iterator） | discriminant lowering |
//! | `FieldDef::ty(tcx, substs) -> Ty` | 返回 `Unnormalized<Ty>`，需 `.skip_normalization()` | field_ty |
//! | `FxIndexMap<WorkProductId, WorkProduct>` 作 join_codegen 返回 | trait 要求 `WorkProductMap`（= `UnordMap<WorkProductId, WorkProduct>`，`rustc_middle::dep_graph`） | backend.rs |
//! | （新增）`rustc_codegen_ssa::debuginfo::mir`/`create_scope_map` 私有 | C2 debuginfo 自研 MIR 变量分析（rustc_middle `body.var_debug_info`/`source_scopes` pub）；`debuginfo::type_names::compute_debuginfo_type_name` pub 可复用 | dwarf.rs（路线图 C2） |

use rustc_middle::ty::{Binder, GenericArgsRef, Ty};

/// 从 `TyKind::FnDef` 解构出的 substs 提取第一个类型实参。
///
/// 2026-08 起 `FnDef` 的第二个字段是 `Binder<GenericArgsRef>`（即
/// `EarlyBinder<TyCtxt, &RawList<(), GenericArg>>`），单态化实例没有 bound
/// vars，`skip_binder()` 安全。返回 `None` 表示实参列表为空或第一个实参
/// 不是类型（调用方回落自身类型）。
pub fn substs_first_ty<'tcx>(substs: &Binder<GenericArgsRef<'tcx>>) -> Option<Ty<'tcx>> {
    substs
        .skip_binder()
        .as_slice()
        .first()
        .and_then(|g| g.as_type())
}
