//! **线程局部静态**（`#[thread_local]`）的**动态 TLS** 运行时（v27 TLS 真实现）。
//!
//! ## 为什么是动态模型
//!
//! 静态 TLS 的机器码形态是 `mov rax, gs:[58]`（TEB → ThreadLocalStoragePointer）+
//! `[rax + 8*_tls_index + off]`——需要 **段前缀**，而 IR 的 `MemRef` 只有
//! base/disp/index/scale，表达不了。动态模型用 kernel32 的 `TlsAlloc`/`TlsGetValue`/
//! `TlsSetValue`，**不需要新的 IR 能力**，代价是每次访问一次函数调用。
//!
//! ## 形态
//!
//! 每个 TLS 静态配**两个隐藏数据符号**（名字由静态符号名确定，降级侧不必查表）：
//!
//! - `__forge_tls_slot_<sym>`：8 字节索引槽（0 = 尚未 `TlsAlloc`，否则是 **索引 + 1**）；
//! - `__forge_tls_init_<sym>`：初始值模板（与静态本身的初值一致）。
//!
//! 访问时调 [`TLS_RUNTIME`]：`__forge_tls_addr(slot_addr, size, align, init_addr) -> ptr`
//! ——首次访问为当前线程 `TlsAlloc`/分配/拷贝初值，之后直接返回该线程的块指针。
//!
//! ## 边界（明确、不静默）
//!
//! - 每个静态**独占**一个 TLS 索引（不共享槽）——静态数量少时可接受；
//! - `TlsAlloc` 失败（索引耗尽）⇒ 直接 `ExitProcess(3)`（与 `abort()` 同码），不做半吊子降级；
//! - 析构不做（Rust 的 `#[thread_local]` 静态本就无析构语义）。

use crate::compile::compile_with_isa;
use crate::error::ForgeError;
use crate::func_ref::FuncRefTable;
use crate::prelude::*;

/// 运行时函数名（调用点与生成点共用这一个常量）。
pub(crate) const TLS_RUNTIME: &str = "__forge_tls_addr";

/// 静态符号名 → 初值模板符号名。
pub(crate) fn tls_init_sym(sym: &str) -> String {
    format!("__forge_tls_init_{sym}")
}

/// 静态符号名 → 索引槽符号名。
pub(crate) fn tls_slot_sym(sym: &str) -> String {
    format!("__forge_tls_slot_{sym}")
}

/// 生成 [`TLS_RUNTIME`] 函数（每个 crate 至多一次；只在真有 TLS 静态时生成）。
pub(crate) fn build_tls_runtime(
    table: &mut FuncRefTable,
    isa_name: &str,
) -> Result<Vec<(String, CompiledFunction)>, ForgeError> {
    let tls_alloc = table.intern("TlsAlloc");
    let tls_get = table.intern("TlsGetValue");
    let tls_set = table.intern("TlsSetValue");
    let rust_alloc = table.intern("__rust_alloc");
    let memcpy = table.intern("memcpy");
    let exit = table.intern("ExitProcess");

    let sig = FunctionSignature::new(
        &[
            (TypeId::I64, "slot"),
            (TypeId::I64, "size"),
            (TypeId::I64, "align"),
            (TypeId::I64, "init"),
        ],
        &[TypeId::I64],
    );
    let mut b = FunctionBuilder::new(TLS_RUNTIME, TypeContext::new(), sig);
    let (entry, params) = b.create_entry_block();
    let _ = entry;
    let (slot, size, align, init) = (params[0], params[1], params[2], params[3]);
    let zero = b.iconst(0, TypeId::I64);

    let use_bb = b.create_block();
    let alloc_bb = b.create_block();
    let ok_bb = b.create_block();
    let die_bb = b.create_block();
    let (merge_bb, merge_p) = b.create_block_with_params(&[(TypeId::I64, "idx")]);
    let allocp_bb = b.create_block();
    let (done_bb, done_p) = b.create_block_with_params(&[(TypeId::I64, "ptr")]);

    // ⚠️ `create_block_with_params` **会切换当前块**（尽管它的 doc 写着"不切换"——
    // 实测以它调用的 `create_block_with_tys` 为准，那里有一行 `self.cur_block = block`）。
    // 所以建完所有块之后必须**显式切回入口块**，否则入口块没有终结符。
    b.switch_to_block(entry);

    // entry：已分配过索引？
    let idx = b.load(slot, TypeId::I64);
    let have_idx = b.icmp(IntCC::Ne, idx, zero);
    b.branch(have_idx, use_bb, &[], alloc_bb, &[]);

    // use_bb：沿用已有索引
    b.switch_to_block(use_bb);
    b.jump(merge_bb, &[idx]);

    // alloc_bb：TlsAlloc（失败 ⇒ ExitProcess(3)）
    b.switch_to_block(alloc_bb);
    let raw = b.call(FuncRef::new(tls_alloc), &[], &[TypeId::I32])[0];
    let raw64 = b.uextend(raw, TypeId::I64);
    let bad = b.iconst(0xFFFF_FFFF, TypeId::I64);
    let failed = b.icmp(IntCC::Eq, raw64, bad);
    b.branch(failed, die_bb, &[], ok_bb, &[]);

    // ok_bb：槽里存 **索引 + 1**（0 保留为"未分配"）
    b.switch_to_block(ok_bb);
    let one = b.iconst(1, TypeId::I64);
    let stored = b.iadd(raw64, one);
    b.store(stored, slot);
    b.jump(merge_bb, &[stored]);

    // die_bb：索引耗尽 —— 明确终止，不做半吊子降级
    b.switch_to_block(die_bb);
    let code = b.iconst(3, TypeId::I64);
    b.call(FuncRef::new(exit), &[code], &[]);
    b.unreachable();

    // merge：本线程的块已存在？
    b.switch_to_block(merge_bb);
    let mi = merge_p[0];
    let one2 = b.iconst(1, TypeId::I64);
    let idx0 = b.isub(mi, one2);
    let cur = b.call(FuncRef::new(tls_get), &[idx0], &[TypeId::I64])[0];
    let have_ptr = b.icmp(IntCC::Ne, cur, zero);
    b.branch(have_ptr, done_bb, &[cur], allocp_bb, &[]);

    // allocp_bb：首次访问 —— 分配 + 拷贝初值 + 记回本线程
    b.switch_to_block(allocp_bb);
    let q = b.call(FuncRef::new(rust_alloc), &[size, align], &[TypeId::I64])[0];
    b.call(FuncRef::new(memcpy), &[q, init, size], &[]);
    b.call(FuncRef::new(tls_set), &[idx0, q], &[]);
    b.jump(done_bb, &[q]);

    // done：返回块指针
    b.switch_to_block(done_bb);
    let r = done_p[0];
    b.ret(&[r]);

    let f = b.finish().map_err(ForgeError::from)?;
    Ok(vec![(
        TLS_RUNTIME.to_string(),
        compile_with_isa(&f, isa_name)?,
    )])
}
