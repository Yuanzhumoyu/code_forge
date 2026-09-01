//! 布局 / ABI 辅助函数（不依赖 LowerCtxt 私有状态，均可独立测试）。
//!
//! 16 字节规则：ScalarPair（如 NonNull<[u8]>）走 RAX/RDX 双寄存器；Layout、
//! Result<Layout, LayoutError> 等非 ScalarPair 走内存（sret/间接参数）。

use crate::prelude::*;

/// 16 字节及以上、非 ScalarPair 的聚合值：按内存传递（sret 返回 / 间接参数）。
/// 16 字节边界：ScalarPair（如 NonNull<[u8]>）走 RAX/RDX 双寄存器；Layout、
/// Result<Layout, LayoutError> 等非 ScalarPair 若走单值路径会被截断成 8 字节。
/// 收敛到 abi.rs 的 `abi_kind_of_ty`（P4.1 统一判定：PassMode 投影）。
pub fn is_agg_mem<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> bool {
    if ty.is_unit() || ty.is_never() {
        return false;
    }
    let r = crate::abi::abi_kind_of_ty(tcx, ty) == crate::abi::AbiKind::Indirect;
    if crate::trace::trace_enabled("PAIR") {
        eprintln!("[forge] is_agg_mem({ty}) = {r}");
    }
    r
}

/// ScalarPair 两个标量在内存中的字节偏移。rustc 允许字段重排（如 Layout 的
/// align@0/size@8、RawVecInner 的 cap@0/ptr@8），pair_offsets 随之变化——
/// 不能假定 0/8，否则寄存器与内存错位（cap/len 串槽）。
pub fn scalar_pair_offsets<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> (i64, i64) {
    let l = match tcx.layout_of(ty::PseudoCanonicalInput {
        typing_env: ty::TypingEnv::fully_monomorphized(),
        value: ty,
    }) {
        Ok(l) => l.layout,
        // layout_of 失败（ty 已单态化，正常不应发生）：回退标准标量对
        // 偏移 (0, 8)，避免 ICE——调用方（unpack/pack_sp）对错位偏移
        // 的容忍度低，但 (0,8) 是最常见布局，风险远小于 panic。
        Err(_) => return (0, 8),
    };
    // ScalarPair 的 fields() 对普通结构体是 [0, 8]（重排后 [8, 0]）；
    // 对 enum（tag+payload 组合）fields() 只含 tag 1 个字段——需从
    // variants 布局取 payload 标量偏移（不能回退固定 0/8：Result<_, E> 等
    // 的 payload 在 tag 之后的对齐位置，回退错位会读错槽——Vec grow 链
    // 的 Result 解构崩溃根因）。
    let n = l.fields().count();
    if n < 2 {
        use rustc_abi::Variants;
        // niche/单字段 ScalarPair（Option<i32>、Result<(), E> 等）：
        // fields()/variants 可能缺字段（tag_field 越界、空 variant——实测
        // rustc_abi:1688 越界 panic）——优先用 BackendRepr::ScalarPair 的
        // 标量宽度。**注意**：第 2 标量偏移必须用 b_offset（2026 nightly
        // ScalarPair struct 的显式字段）——payload 是聚合（如 (usize,&i32)
        // 16 字节）时对齐后不在 tag 之后立即（mn2 实证：Option<(usize,&i32)>
        // 的 payload 在 offset 8，旧 (0, a.size)= (0,1) 错位 → 返回垃圾判别）。
        if let rustc_abi::BackendRepr::ScalarPair { a, b_offset, .. } = l.backend_repr {
            let b_off = b_offset.bytes() as i64;
            if crate::trace::trace_enabled("PAIR") {
                eprintln!("[forge] pair_offsets niche a_size={} b_off={b_off} ty={ty}", a.primitive().size(&tcx).bytes());
            }
            return (0, b_off);
        }
        if let Variants::Multiple {
            tag_field,
            variants,
            ..
        } = l.variants()
        {
            if let Some(v0) = variants.iter().next() {
                let vn = v0.field_offsets.len();
                if vn >= 2 {
                    // 嵌套 ScalarPair payload（2 标量）：两个 payload 标量偏移
                    let f0 = v0.field_offsets[FieldIdx::new(0)].bytes() as i64;
                    let f1 = v0.field_offsets[FieldIdx::new(1)].bytes() as i64;
                    return (f0, f1);
                }
                // tag + 1 标量 payload：两个标量 = (tag 偏移, payload 偏移)
                let n = l.fields().count();
                let tag_off = if tag_field.index() < n {
                    l.fields().offset(tag_field.index()).bytes() as i64
                } else {
                    0
                };
                let payload_off = if vn > 0 {
                    v0.field_offsets[FieldIdx::new(0)].bytes() as i64
                } else {
                    0
                };
                return (tag_off, payload_off);
            }
        }
        return (0, 8);
    }
    let f0 = l.fields().offset(0).bytes() as i64;
    let f1 = l.fields().offset(1).bytes() as i64;
    if crate::trace::trace_enabled("PAIR") {
        eprintln!("[forge] pair_offsets fields (f0={f0}, f1={f1}) ty={ty} n={n}");
    }
    (f0, f1)
}

/// 类型真实布局大小（字节）。
pub fn layout_bytes<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> u32 {
    tcx.layout_of(ty::PseudoCanonicalInput {
        typing_env: ty::TypingEnv::fully_monomorphized(),
        value: ty,
    })
    .map(|l| l.layout.size().bytes() as u32)
    .unwrap_or(8)
}

/// 栈槽大小（字节）：按布局大小对齐到 8（槽间距用）。
pub fn layout_size<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> u32 {
    layout_bytes(tcx, ty).max(8).div_ceil(8) * 8
}

/// ScalarPair 两个标量在内存中的真实字节宽度。pack_sp/unpack_sp 按此宽度
/// 做字段 load/store——恒用 I64（8 字节）会对窄字段越界（Option<i32> 的
/// value@4 8 字节写覆盖相邻槽，range1 for 循环挂起实证：main 收 next 返回
/// pack_sp 的 hi 8 字节写 -0x6c..-0x65 覆盖 _4 槽 -0x68 的 start → 循环不终止）。
pub fn scalar_pair_widths<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> (u32, u32) {
    if let Ok(l) = tcx.layout_of(ty::PseudoCanonicalInput {
        typing_env: ty::TypingEnv::fully_monomorphized(),
        value: ty,
    }) && let rustc_abi::BackendRepr::ScalarPair { a, b, .. } = l.layout.backend_repr
    {
        let wa = a.primitive().size(&tcx).bytes() as u32;
        let wb = b.primitive().size(&tcx).bytes() as u32;
        return (wa.max(1), wb.max(1));
    }
    (8, 8)
}

/// ScalarPair 标量字节宽度 → forge-ir 整数类型（8/16/32/64，其他回退 I64）。
pub fn scalar_width_type(w: u32) -> TypeId {
    match w {
        1 | 2 | 4 => TypeId::I32, // 窄标量按 I32 域处理（写侧 ireduce 到 I32 不越界）
        _ => TypeId::I64,
    }
}

/// 该类型是否按 ScalarPair ABI 传参/返回（两个标量，如 (i64,i64) 结构体）。
///
/// 判断依据是 rustc 布局的 `backend_repr`，而非字节大小区间——8 字节的
/// i64/f64/指针是 Scalar（单个寄存器），绝不能被当作两个标量拆开。
/// 否则 entry 收参与调用方传参错位（Windows x64 浮点走 XMM、整数走 GPR），
/// 混合参数/跨 crate 泛型（如 i64.wrapping_add）会读到垃圾。
pub fn is_scalar_pair_abi<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> bool {
    let r = crate::abi::abi_kind_of_ty(tcx, ty) == crate::abi::AbiKind::Pair;
    if crate::trace::trace_enabled("PAIR") {
        eprintln!("[forge] is_scalar_pair_abi({ty}) = {r}");
    }
    r
}
