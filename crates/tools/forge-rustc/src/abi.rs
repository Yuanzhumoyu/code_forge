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
    /// 128 位整数（`i128/u128`）：两个整数寄存器（lo/hi）。与 `Pair` **语义相同但独立**，
    /// 便于将来单独演进（v27 实测：先例是把 128 位塞进 `Pair` 时，配合"计数补偿"会打断闭包）。
    Wide,
}

/// 穷举翻译 rustc 的 `PassMode` → forge 形态（覆盖全部变体）。
pub(crate) fn abi_kind_of_mode(mode: &PassMode) -> AbiKind {
    match mode {
        PassMode::Ignore => AbiKind::Ignore,
        PassMode::Direct(_) => AbiKind::Direct,
        PassMode::Pair(..) => AbiKind::Pair,
        // Cast：保持按内存/间接处理（v27 实测：rustc 的 FnAbi 对 u128 报
        // `modes [Direct, Direct]`——每实参**一个槽位**，与"Pair 计 2"不符）。
        PassMode::Cast { .. } => AbiKind::Wide, // v27 M5-③：16B 标量 ⇒ 两个整数寄存器
        PassMode::Indirect { .. } => AbiKind::Indirect,
    }
}

/// 按类型布局判定传递形态（现有启发式的形式化：
/// `backend_repr` 判定 ScalarPair，16 字节边界判定聚合内存传递）。
pub(crate) fn abi_kind_of_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> AbiKind {
    if ty.is_unit() || ty.is_never() {
        return AbiKind::Ignore;
    }
    if matches!(
        ty.kind(),
        ty::TyKind::Int(ty::IntTy::I128) | ty::TyKind::Uint(ty::UintTy::U128)
    ) {
        return AbiKind::Wide;
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
        rustc_abi::BackendRepr::ScalarPair { .. } => {
            // **Pair 协议必须两半都落在类型大小内**（v27 M6-126）：`Map<I, F>`
            // 这类 16 字节结构体（两个 8 字节字段 + ZST 尾巴）被 rustc 标成
            // ScalarPair，但 `scalar_pair_widths` 推出的第二半跑到类型外——
            // 旧实现仍归 `Pair`，而 `is_scalar_pair_abi` 判 false ⇒ 调用方按
            // "单值"只传 Map 的首字、被调方把地址当字段读 ⇒ `Map::fold` 死循环
            //（实测 `map().sum()` 在 -O0 永不退出）。改归 `Indirect`（按引用传
            // 整个聚合），与 `is_scalar_pair_abi == false` 一致。
            let (o0, o1) = crate::layout::scalar_pair_offsets(tcx, ty);
            let (w0, w1) = crate::layout::scalar_pair_widths(tcx, ty);
            let size = l.size().bytes() as i64;
            if (o0 + w0 as i64) <= size && (o1 + w1 as i64) <= size {
                AbiKind::Pair
            } else {
                AbiKind::Indirect
            }
        }
        rustc_abi::BackendRepr::SimdVector { .. } => {
            // 向量（含 128 位 __m128 等）在 Windows x64 ≤16B 走 XMM 寄存器
            //（Direct——V64/V128 按值 XMM 全宽 2026-09 已放行，WA-37 D3：
            // 主库 DSL vec_mov_inst=MOVAPS 全宽 128 位收参/传参/返回）；
            // >16B（V256 32B）无 YMM 参数寄存器：rustc/LLVM Win64 与主库
            // by-ref/sret 均按内存间接传递（WA-37 D1 实证：LLVM Win64 把
            // 32B 向量归类 MEMORY → PassMode::Indirect）→ AbiKind::Indirect，
            // forge-rustc 走通用聚合槽路径（arg 指针 + 收参 copy_agg /
            // sret ret 缓冲），向量值全程以内存建模、绝不物化向量寄存器值。
            if l.size().bytes() > 16 {
                AbiKind::Indirect
            } else {
                AbiKind::Direct
            }
        }
        _ => {
            // Scalar / Aggregate。**阈值随目标 ABI 走**（v27 M6-126）：
            // - x86_64（Win64）：聚合 >8 字节按引用（Indirect/sret）——v27 M5 实测
            //   阈值定成 16 会让 9..=15 字节聚合走"单寄存器"只搬 8 字节；
            // - aarch64/riscv64：AAPCS/LP64D 的复合类型 ≤16 字节**按两个寄存器传**，
            //   >16 才按引用。旧实现沿用 Win64 阈值 ⇒ `Map<I,F>` 这类 16 字节聚合
            //   被当 Indirect（sret/指针），与 rustc `FnAbi` 的 `PassMode::Pair`
            //   不一致 —— 调用点断言 `ABI arg count mismatch`，且 `Map::fold`
            //   在 -O0 读错 `self`（`map().sum()` 死循环）。
            // 16 字节标量（i128/u128/f128）已在上面单独归 `Wide`。
            let wide_regs = !matches!(tcx.sess.target.arch, rustc_target::spec::Arch::X86_64);
            let size = l.size().bytes();
            if size > 8 && size <= 16 && wide_regs {
                // 非 ScalarPair 的 9..16 字节聚合：AAPCS 两寄存器（lo@0/hi@8）。
                AbiKind::Pair
            } else if size > if wide_regs { 16 } else { 8 } {
                AbiKind::Indirect
            } else {
                AbiKind::Direct
            }
        }
    }
}

/// C 变参**定义**（`extern "C" fn f(n: i32, ...)`）的 fail-closed 错误。
///
/// 为什么不做：Win64 上要正确实现它，必须由**被调方**在序言里建"寄存器保存区"
/// （4 个 GPR + 4 个 XMM home 到栈上），`va_start` 才能算出第一个变参地址，
/// `va_arg` 再按 ABI 槽推进——这是**序言 + 三处 intrinsic** 的联动改造。
/// 而 C 变参定义在 Rust 里是 nightly 特性、真实用法极少（我们支持的是**调用**变参函数，
/// 那是 `variadic` 分支负责的另一件事）。
/// 宁可明确拒绝，也不要给一个"看起来能用、边界上错值"的实现。
///
/// 只保留**一处** `capability(` 调用点：缺口守卫要求每个 ID 在源码里恰好一处
/// （`compiler_gaps` 双向相等），所以定义侧与 intrinsic 侧都走这个助手。
pub(crate) fn varargs_def_unsupported(what: &str) -> crate::error::ForgeError {
    crate::error::ForgeError::from(IrError::capability(
        "VARARGS_DEF",
        format!(
            "{what}：本后端不支持 **C 变参函数定义**（`extern \"C\" fn f(n: i32, ...)`）。\
             真做需要在被调方序言里建寄存器保存区 + 实现 `va_start`/`va_arg`/`va_end` \
             三处 intrinsic；当前只支持**调用**变参函数（实参侧）。\
             请改用固定参数或切片/指针传参"
        ),
    ))
}

/// 该实例是不是 **C 变参函数**（`extern "C" fn f(n: i32, ...)`）。
///
/// 用途有两处：① 调用点的 arg-count 一致性校验必须放过它（rustc 的 `FnAbi.args`
/// 只含固定参数）；② 定义侧拒绝它（`[cap:VARARGS_DEF]`，见 `compile.rs`）。
pub(crate) fn fn_abi_is_c_variadic<'tcx>(tcx: TyCtxt<'tcx>, instance: &Instance<'tcx>) -> bool {
    MonoLayoutCx(tcx)
        .fn_abi_of_instance(*instance, ty::List::empty())
        .is_ok_and(|abi| abi.c_variadic)
}

/// 该实例的调用约定是不是 **Rust ABI**（`extern "Rust"`，即没有写 `extern`）。
///
/// 为什么降级层要区分它（v27 M6-86，`WA-52`）：同一台机器上 `u128` 的传参/返回形态
/// **随约定而变**——Rust ABI 是两整数槽/`RAX:RDX`（实测 LLVM `mov %rcx,%rdx; mov $3,%eax`），
/// 而 `extern "C"`（win64）是**按引用 + XMM0**。把后者套到 Rust ABI 上会立刻破坏
/// 现已正确的宽参数/宽返回。
pub(crate) fn abi_is_rust<'tcx>(tcx: TyCtxt<'tcx>, instance: &Instance<'tcx>) -> bool {
    MonoLayoutCx(tcx)
        .fn_abi_of_instance(*instance, ty::List::empty())
        .is_ok_and(|abi| format!("{:?}", abi.conv) == "Rust")
}

/// **16 字节整数标量**（`i128`/`u128`）在一个函数边界上的形态——**问 rustc 的 `FnAbi`**。
///
/// # 为什么不能按"约定名"分
///
/// 同一台机器、同一份约定数据下，`u128` 的形态由**目标 ABI 的传递规则**决定，而 rustc
/// 的 `FnAbi` 就是这条规则在本目标上的权威投影（实测，2026-10-24，x86_64-pc-windows-msvc）：
///
/// | 边界 | Rust ABI | `extern "C"`（Win64） |
/// | --- | --- | --- |
/// | 实参 | `Direct`（≡ 两个整数槽） | `Indirect { pointee_size: 16 }`（按引用） |
/// | 返回 | `Direct`（≡ `RAX:RDX`） | `Cast { unit: Vector { size: 16 } }`（`XMM0`） |
///
/// 所以降级层问的是**数据**（`PassMode`），不写"如果是 C 就一定按引用"这类判断：
/// 换目标、换约定、rustc 改分类，这里都跟着走；问不到就 fail-closed（不猜）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WideIntArg {
    /// 两个整数槽（lo/hi）——Rust ABI、SysV 等。
    Pairs,
    /// 按引用：调用方副本的指针（Win64 的 C 家族）。
    ByRef,
}

/// 16 字节整数标量的**返回**落点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WideIntRet {
    /// 两个整数寄存器（`RAX:RDX`）——Rust ABI、SysV 等。
    Registers,
    /// 一个 16 字节向量寄存器（`XMM0`）——Win64 的 C 家族（`PassMode::Cast` 的
    /// `Vector { size: 16 }` 单元就是它）。
    Vector,
    /// 隐藏 sret 指针（本后端未见；见到即由降级层明确拒绝，不猜形态）。
    Indirect,
}

/// 一个签名的 128 位边界形态（两侧按需给：只关心存在的那一侧）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WideIntAbi {
    pub arg: Option<WideIntArg>,
    pub ret: Option<WideIntRet>,
}

/// 该类型是不是 16 字节整数标量（`i128`/`u128`）。
fn is_wide_int_ty(ty: Ty<'_>) -> bool {
    matches!(
        ty.kind(),
        ty::TyKind::Int(ty::IntTy::I128) | ty::TyKind::Uint(ty::UintTy::U128)
    )
}

/// 从一份 `FnAbi` 读出 128 位边界的形态（没有 128 位边界 ⇒ `None`）。
fn shapes_of_fn_abi(abi: &rustc_target::callconv::FnAbi<'_, Ty<'_>>) -> Option<WideIntAbi> {
    let mut arg = None;
    for a in &abi.args {
        if is_wide_int_ty(a.layout.ty) {
            // 同一签名里同一类型的形态一致（类型驱动）；取到一条即可。
            arg = Some(match &a.mode {
                PassMode::Indirect { .. } => WideIntArg::ByRef,
                _ => WideIntArg::Pairs,
            });
        }
    }
    let ret = if is_wide_int_ty(abi.ret.layout.ty) {
        Some(match &abi.ret.mode {
            PassMode::Cast { .. } => WideIntRet::Vector,
            PassMode::Indirect { .. } => WideIntRet::Indirect,
            _ => WideIntRet::Registers,
        })
    } else {
        None
    };
    if arg.is_none() && ret.is_none() {
        return None;
    }
    Some(WideIntAbi { arg, ret })
}

/// 问该实例的 `FnAbi`：128 位边界的形态（没有 128 位边界 ⇒ `None`）。
///
/// 返回 `None` 还有一种情形：`FnAbi` 求不出来（布局错误等）——调用点拿到 `None`
/// 而手上又有 128 位值时**必须 fail-closed**（`WA-52` 的成因就是"猜一个继续"）。
pub(crate) fn wide_int_abi<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: &Instance<'tcx>,
) -> Option<WideIntAbi> {
    let abi = MonoLayoutCx(tcx)
        .fn_abi_of_instance(*instance, ty::List::empty())
        .ok()?;
    shapes_of_fn_abi(abi)
}

/// 从**函数类型**（`fn(..)`/`extern "C" fn(..)` 指针或未解析的 `FnDef`）问 `FnAbi`。
///
/// 为什么需要它（v27 M6-89）：间接调用（fn 指针、`dyn Trait` 方法）**拿不到被调方实例**，
/// 但 MIR 的操作数类型里带着 `FnSig`——`FnSig.abi` 就是"这个指针指向的函数用哪份调用
/// 约定"的权威声明。`fn_abi_of_fn_ptr` 让形态判定从"实例"扩展到"类型"，于是
/// `fn(u128) -> u64` / `dyn Trait { fn m(&self, u128) }` 不再 fail-closed。
pub(crate) fn wide_int_abi_of_callee_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: Ty<'tcx>,
) -> Option<WideIntAbi> {
    let sig: ty::PolyFnSig<'tcx> = match ty.kind() {
        // 本 nightly 把 `FnSig` 拆成 (tys, header) 两块（省 8 字节）；`with` 合回去。
        ty::TyKind::FnPtr(sig_tys, header) => (*sig_tys).with(*header),
        ty::TyKind::FnDef(def_id, substs) => tcx
            .fn_sig(*def_id)
            .instantiate(tcx, substs.skip_binder())
            // 本 nightly 的 `fn_sig` 查询把结果包在 `Unnormalized` 里（查询输入/输出
            // 的规范化状态）；降级层只做位点判定，不需要再归一化。
            .skip_normalization(),
        _ => return None,
    };
    let abi = MonoLayoutCx(tcx)
        .fn_abi_of_fn_ptr(sig, ty::List::empty())
        .ok()?;
    shapes_of_fn_abi(abi)
}

/// 调用点：从 MIR 的 `Operand` 解析**被调方实例**。
///
/// 返回 `None` = 间接调用 / 解析不了。调用方拿不到被调方时就**必须 fail-closed**：
/// 128 位值的形态取决于被调方，猜一个约定继续正是 `WA-52` 的成因。
pub(crate) fn callee_instance<'tcx>(
    tcx: TyCtxt<'tcx>,
    func: &Operand<'tcx>,
) -> Option<Instance<'tcx>> {
    let Operand::Constant(c) = func else {
        return None;
    };
    let ty::TyKind::FnDef(def_id, substs) = c.const_.ty().kind() else {
        return None;
    };
    ty::Instance::try_resolve(
        tcx,
        ty::TypingEnv::fully_monomorphized(),
        *def_id,
        substs.skip_binder(),
    )
    .ok()
    .flatten()
}

/// 调用点：被调方的 128 位边界形态。
///
/// 先按**实例**问（直接调用最精确）；实例解析不出（fn 指针 / 虚调用 / 部分替换）时
/// 退回**操作数类型上的 `FnSig`**（`wide_int_abi_of_callee_ty`）。两条都拿不到才 `None`。
pub(crate) fn callee_wide_int_abi<'tcx>(
    tcx: TyCtxt<'tcx>,
    func: &Operand<'tcx>,
    body: &Body<'tcx>,
) -> Option<WideIntAbi> {
    if let Some(inst) = callee_instance(tcx, func)
        && let Some(wide) = wide_int_abi(tcx, &inst)
    {
        return Some(wide);
    }
    wide_int_abi_of_callee_ty(tcx, func.ty(&body.local_decls, tcx))
}

/// 128 位值跨 ABI 边界的形态求不出来 ⇒ **明确拒绝**（fail-closed）。
///
/// **单点构造**：缺口守卫要求每个 `[cap:ID]` 在源码里恰好一处 `capability(` 调用，
/// 所以降级层的各站点都经这个助手（只说明"哪一侧、哪个值"）。见 `backend-gaps.md`
/// 的 `WIDE_INT_ABI`——间接调用 / `FnAbi` 查询失败时**不猜**形态，正是 `WA-52` 的教训。
pub(crate) fn wide_int_shape_unsupported(what: impl std::fmt::Display) -> crate::error::ForgeError {
    crate::error::ForgeError::from(IrError::capability(
        "WIDE_INT_ABI",
        format!(
            "{what}：128 位值跨 ABI 边界的形态求不出来（rustc `FnAbi` 查询失败，或间接调用\
             拿不到被调方）——不猜形态，也不按某一侧约定继续"
        ),
    ))
}

/// 本后端**实现**的调用约定白名单（Win64 一族）；不在其中的一律 fail-closed。
///
/// 为什么必须拒绝而不是当成 C 处理：收参/传参按 **Win64** 硬编码
/// （参数 `RCX/RDX/R8/R9`、shadow space `[rsp+0x20]`、`XMM0-3`、sret 在 `RCX`）。
/// 实测（2026-10-19，`FORGE_TRACE_ABI=1` 打 conv）：rustc 把各 ABI 映射成
/// `Conv::Rust` / `Conv::C` / `Conv::X86(X86Conv::Win64)` / `Conv::X86(X86Conv::SysV64)`
/// ——`extern "sysv64"` 属最后一类，我们此前把它当 C 打包 ⇒ **寄存器全错位**；
/// 而调用方/被调方都是我们编的，本地测试"自洽"看不出来，一旦跨到 LLVM 编的代码
/// （或 `extern` 声明的外部函数）就是错值。
pub(crate) fn check_conv_supported<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: &Instance<'tcx>,
    ctx: &str,
) -> Result<(), crate::error::ForgeError> {
    let Some(fn_abi) = MonoLayoutCx(tcx)
        .fn_abi_of_instance(*instance, ty::List::empty())
        .ok()
    else {
        return Ok(());
    };
    let name = format!("{:?}", fn_abi.conv);
    if matches!(name.as_str(), "Rust" | "C" | "X86(Win64)") {
        return Ok(());
    }
    Err(crate::error::ForgeError::from(IrError::capability(
        "CALL_CONV",
        format!(
            "{ctx}：不支持调用约定 `{name}`（实例 {instance:?}）。本后端只实现 Win64 一族\
             （`C`/`Rust`/`win64`：RCX/RDX/R8/R9 + shadow space + XMM0-3，sret 在 RCX）；\
             `sysv64`/`vectorcall` 等需要另一套收参与传参，继续按 C 打包会**静默错值**。\
             请改用 `extern \"C\"` 或去掉该 ABI"
        ),
    )))
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
            "[abi] {instance:?} conv={:?} args={} ret.mode={:?} args.modes={:?}",
            fn_abi.conv,
            fn_abi.args.len(),
            fn_abi.ret.mode,
            fn_abi.args.iter().map(|a| &a.mode).collect::<Vec<_>>()
        );
    }
    // **128 位标量的槽数取决于 `PassMode`**（v27 M6-88，`WA-52`）：按引用 = 1 个指针槽，
    // 其余 = 2 个整数槽。旧实现按**类型**一律记 2 ⇒ C 家族（按引用）会被算成"期望 2 个
    // 实参而实际打包 1 个"，一致性断言当场 panic（这正是"按约定猜"留下的口径）。
    let args: Vec<AbiKind> = fn_abi
        .args
        .iter()
        .map(|a| {
            if is_wide_int_ty(a.layout.ty) {
                if matches!(a.mode, PassMode::Indirect { .. }) {
                    AbiKind::Direct
                } else {
                    AbiKind::Wide
                }
            } else {
                abi_kind_of_mode(&a.mode)
            }
        })
        .collect();
    let ret = if is_wide_int_ty(fn_abi.ret.layout.ty) {
        if matches!(fn_abi.ret.mode, PassMode::Indirect { .. }) {
            AbiKind::Indirect
        } else {
            AbiKind::Wide
        }
    } else {
        abi_kind_of_mode(&fn_abi.ret.mode)
    };
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
    if std::env::var_os("FORGE_STRICT_ABI").is_some_and(|v| v == "0") {
        return;
    }
    // **C 变参被调方不参与这个校验**：rustc 的 `FnAbi.args` 只含**固定**参数，
    // 而调用点当然会多传变参（`sum(1, 2i64, 39i64)` ⇒ expects 1 / packs 2）。
    // 不排除它会直接 `assert!` **panic 掉 rustc**（2026-10-19 实测：用户写一个
    // `extern "C" fn f(n: i32, ...)` 就 ICE）——编译器对合法用户代码 panic 是最糟的
    // 失败方式。变参本身的形态由调用点的 `variadic` 分支单独照顾。
    if fn_abi_is_c_variadic(tcx, instance) {
        return;
    }
    let Some((kinds, ret_kind)) = fn_abi_kinds(tcx, instance) else {
        return;
    };
    let mut expected: usize = kinds
        .iter()
        .map(|k| match k {
            AbiKind::Pair | AbiKind::Wide => 2,
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
            AbiKind::Pair | AbiKind::Wide => 2,
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
