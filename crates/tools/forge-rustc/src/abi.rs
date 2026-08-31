//! rustc `PassMode` 穷举翻译 + 双向转换（P4.1）。
//!
//! 统一 forge-rustc 散落的启发式判定（`is_scalar_pair_abi` / `is_agg_mem`）
//! 为显式 ABI 形态枚举 `AbiKind`，并对照 rustc 的精确 `FnAbi`（
//! `tcx.fn_abi_of_instance`）做 debug-only 一致性校验——ABI 判定一旦漂移
//! （参数个数/双寄存器展开），e2e 的 debug 构建立即暴露。
//!
//! 形态约定（Windows x64）：
//! - `Direct`   → 单寄存器/栈槽（RAX/RCX/... 或 XMM），1 个参数
//! - `Pair`     → ScalarPair 拆两个标量（RAX+RDX 或 XMM 对），2 个参数
//! - `Indirect` → 聚合按内存传递（sret 指针 / 间接参数），1 个指针参数
//! - `Ignore`   → ZST 不占参数
//!
//! 注意：rustc 的 `PassMode` 定义在 `rustc_target::callconv`（非 rustc_abi），
//! nightly 升级时优先检查此处（参见 rustc_compat.rs 漂移清单约定）。

use crate::prelude::*;
use rustc_middle::ty::layout::{
    FnAbiError, FnAbiOf, FnAbiOfHelpers, FnAbiRequest, HasTyCtxt, HasTypingEnv, LayoutError,
    LayoutOfHelpers, TyAndLayout,
};
use rustc_target::callconv::PassMode;

/// 单态化布局/ABI 上下文包装（对齐 CGCL 的 `FullyMonomorphizedLayoutCx`：
/// TyCtxt 实现 `LayoutOfHelpers`/`FnAbiOfHelpers` 的桥，供 `fn_abi_of_instance`）。
pub(crate) struct MonoLayoutCx<'tcx>(pub(crate) TyCtxt<'tcx>);

impl<'tcx> HasTyCtxt<'tcx> for MonoLayoutCx<'tcx> {
    fn tcx<'b>(&'b self) -> TyCtxt<'tcx> {
        self.0
    }
}

impl<'tcx> rustc_abi::HasDataLayout for MonoLayoutCx<'tcx> {
    fn data_layout(&self) -> &rustc_abi::TargetDataLayout {
        &self.0.data_layout
    }
}

impl<'tcx> HasTypingEnv<'tcx> for MonoLayoutCx<'tcx> {
    fn typing_env(&self) -> ty::TypingEnv<'tcx> {
        ty::TypingEnv::fully_monomorphized()
    }
}

impl<'tcx> rustc_target::spec::HasTargetSpec for MonoLayoutCx<'tcx> {
    fn target_spec(&self) -> &rustc_target::spec::Target {
        &self.0.sess.target
    }
}

impl<'tcx> LayoutOfHelpers<'tcx> for MonoLayoutCx<'tcx> {
    type LayoutOfResult = Result<TyAndLayout<'tcx>, &'tcx LayoutError<'tcx>>;

    fn handle_layout_err(
        &self,
        err: LayoutError<'tcx>,
        _: rustc_span::Span,
        _: Ty<'tcx>,
    ) -> &'tcx LayoutError<'tcx> {
        self.0.arena.alloc(err)
    }
}

impl<'tcx> FnAbiOfHelpers<'tcx> for MonoLayoutCx<'tcx> {
    type FnAbiOfResult =
        Result<&'tcx rustc_target::callconv::FnAbi<'tcx, Ty<'tcx>>, &'tcx FnAbiError<'tcx>>;

    fn handle_fn_abi_err(
        &self,
        err: FnAbiError<'tcx>,
        _: rustc_span::Span,
        _: FnAbiRequest<'tcx>,
    ) -> &'tcx FnAbiError<'tcx> {
        self.0.arena.alloc(err)
    }
}

/// forge 视角的参数/返回传递形态（rustc `PassMode` 的投影）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AbiKind {
    /// 单标量/向量直接传寄存器。
    Direct,
    /// ScalarPair：两个标量分别传。
    Pair,
    /// 聚合内存传递（sret / 间接参数）。
    Indirect,
    /// ZST，忽略。
    Ignore,
}

/// 穷举翻译 rustc 的 `PassMode` → forge 形态（覆盖全部变体）。
pub(crate) fn abi_kind_of_mode(mode: &PassMode) -> AbiKind {
    match mode {
        PassMode::Ignore => AbiKind::Ignore,
        PassMode::Direct(_) => AbiKind::Direct,
        PassMode::Pair(..) => AbiKind::Pair,
        // Cast：rustc 按整数寄存器重铸传递，forge 无 CastTarget 概念，
        // 按内存/间接处理（与 is_agg_mem 的 ≥16 字节规则行为一致）。
        PassMode::Cast { .. } => AbiKind::Indirect,
        PassMode::Indirect { .. } => AbiKind::Indirect,
    }
}

/// 按类型布局判定传递形态（现有启发式的形式化：
/// `backend_repr` 判定 ScalarPair，16 字节边界判定聚合内存传递）。
pub(crate) fn abi_kind_of_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> AbiKind {
    if ty.is_unit() || ty.is_never() {
        return AbiKind::Ignore;
    }
    let l = match tcx.layout_of(ty::PseudoCanonicalInput {
        typing_env: ty::TypingEnv::fully_monomorphized(),
        value: ty,
    }) {
        Ok(l) => l.layout,
        // 布局失败（不应发生）：按标量处理，避免 panic（同 layout.rs 约定）
        Err(_) => return AbiKind::Direct,
    };
    match l.backend_repr {
        rustc_abi::BackendRepr::ScalarPair { .. } => AbiKind::Pair,
        rustc_abi::BackendRepr::SimdVector { .. } => {
            // 向量（含 128 位 __m128 等）在 Windows x64 走 XMM 寄存器
            AbiKind::Direct
        }
        _ => {
            // Scalar / Aggregate：16 字节边界判定内存传递。注意 16 字节
            // Scalar（i128/u128/f128）在 Windows x64 也是 Indirect——不能
            // 因 backend_repr 是 Scalar 就按 Direct 处理（i128 按单寄存器
            // 传会被截断成 8 字节）。
            if l.size().bytes() >= 16 {
                AbiKind::Indirect
            } else {
                AbiKind::Direct
            }
        }
    }
}

/// 取实例的 FnAbi 参数/返回形态序列（rustc 精确值）。
/// 失败（布局错误等）返回 `None`，调用方按无 FnAbi 处理。
pub(crate) fn fn_abi_kinds<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: &Instance<'tcx>,
) -> Option<(Vec<AbiKind>, AbiKind)> {
    let fn_abi = MonoLayoutCx(tcx)
        .fn_abi_of_instance(*instance, ty::List::empty())
        .ok()?;
    if crate::trace::trace_enabled("ABI") {
        eprintln!(
            "[abi] {instance:?} args={} ret.mode={:?} args.modes={:?}",
            fn_abi.args.len(),
            fn_abi.ret.mode,
            fn_abi.args.iter().map(|a| &a.mode).collect::<Vec<_>>()
        );
    }
    let args: Vec<AbiKind> = fn_abi
        .args
        .iter()
        .map(|a| abi_kind_of_mode(&a.mode))
        .collect();
    let ret = abi_kind_of_mode(&fn_abi.ret.mode);
    Some((args, ret))
}

/// ABI 一致性校验（A2）：本后端按类型判定的参数展开数必须与 rustc FnAbi
/// 精确形态一致。不一致即 panic（编译期捕获 ABI 漂移——release 构建同样
/// 生效，不再仅 debug）。可用 `FORGE_STRICT_ABI=0` 关闭（不推荐，仅供
/// 上游 API 漂移过渡期绕过）。
pub(crate) fn check_arg_count_consistency<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: &Instance<'tcx>,
    actual_arg_count: usize,
    ctx: &str,
) {
    if std::env::var_os("FORGE_STRICT_ABI").map_or(false, |v| v == "0") {
        return;
    }
    let Some((kinds, ret_kind)) = fn_abi_kinds(tcx, instance) else {
        return;
    };
    let mut expected: usize = kinds
        .iter()
        .map(|k| match k {
            AbiKind::Pair => 2,
            AbiKind::Ignore => 0,
            _ => 1,
        })
        .sum();
    if ret_kind == AbiKind::Indirect {
        // sret 返回：rustc 的 FnAbi.args 不含 ret 缓冲指针（ret.mode=Indirect
        // 隐含），但调用方必须额外传第 0 参数（rcx）——计数时补 1。
        // 否则（如 current_memory 的 24 字节 Option<(NonNull, Layout)> 返回）
        // 误报 "expects 3, packs 4"。
        expected += 1;
    }
    assert_eq!(
        expected, actual_arg_count,
        "[forge] ABI arg count mismatch in {ctx} for {instance:?}: \
         rustc FnAbi expects {expected} (modes {kinds:?}), forge packs {actual_arg_count}"
    );
}

/// P4.1：直接调用时按 rustc FnAbi 补齐参数（track_caller 的隐藏
/// `&Location` 参数等——MIR 调用只含显式参数，被调方期望更多）。
/// 无法查询 FnAbi 时返回原参数（不改变行为）。
/// `zero` 回调生成占位值（Location 传 null 指针，panic 路径仍可执行，
/// 仅 panic 信息里的 Location 为 null）。
pub(crate) fn pad_call_args<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: &Instance<'tcx>,
    mut values: Vec<Value>,
    mut zero: impl FnMut() -> Value,
) -> Vec<Value> {
    let Some((kinds, ret_kind)) = fn_abi_kinds(tcx, instance) else {
        return values;
    };
    let mut expected: usize = kinds
        .iter()
        .map(|k| match k {
            AbiKind::Pair => 2,
            AbiKind::Ignore => 0,
            _ => 1,
        })
        .sum();
    if ret_kind == AbiKind::Indirect {
        // sret 返回：rustc FnAbi.args 不含 ret 指针（ret.mode=Indirect 隐含），
        // 但调用方必须传第 0 参数——与 check_arg_count_consistency 的计数
        // 一致（否则 pad 补不满、断言误报，如 String::from 的 String 返回）
        expected += 1;
    }
    while values.len() < expected {
        values.push(zero());
    }
    values
}
