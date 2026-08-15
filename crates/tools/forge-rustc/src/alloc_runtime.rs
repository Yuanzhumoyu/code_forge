//! alloc 运行时 shim：`__rust_alloc`/`__rust_dealloc`/`__rust_realloc`/`__rust_alloc_zeroed` +
//! `memmove`/`memcmp`/`strlen`（FFI 调 kernel32 VirtualAlloc/VirtualFree，页面分配天然 ≥4KB 对齐，
//! 使 no_std + `extern crate alloc` 的 Vec/Box/String 可运行）。

use crate::compile::compile_with_isa;
use crate::error::ForgeError;
use crate::func_ref::FuncRefTable;
use crate::prelude::*;

/// 构建 alloc 运行时：`__rust_alloc`/`__rust_dealloc`/`__rust_realloc`/`__rust_alloc_zeroed`。
/// 通过 FFI 调用 kernel32 的 `VirtualAlloc`/`VirtualFree`（UNDEF 重定位）实现真实堆分配，
/// 使 no_std + `extern crate alloc` 的 Vec/Box/String 可运行。
/// 页面分配天然 ≥ 4KB 对齐，覆盖 Vec/Box 的 align ≤ 16（超页对齐后续精确化）。
pub fn build_alloc_runtime<'tcx>(
    tcx: TyCtxt<'tcx>,
    table: &mut FuncRefTable,
    isa_name: &str,
) -> Result<Vec<(String, CompiledFunction)>, ForgeError> {
    let valloc = table.intern("VirtualAlloc");
    let vfree = table.intern("VirtualFree");
    let rust_alloc = table.intern("__rust_alloc");
    let rust_dealloc = table.intern("__rust_dealloc");
    let mut out = Vec::new();

    // __rust_alloc(size: usize, align: usize) -> *mut u8
    // VirtualAlloc(0, size, MEM_COMMIT|MEM_RESERVE=0x3000, PAGE_READWRITE=0x04)
    {
        let sig = FunctionSignature::new(
            &[(TypeId::I64, "size"), (TypeId::I64, "align")],
            &[TypeId::I64],
        );
        let mut b = FunctionBuilder::new("__rust_alloc", TypeContext::new(), sig);
        let (_blk, p) = b.create_entry_block();
        let zero = b.iconst(0, TypeId::I64);
        let ty = b.iconst(0x3000, TypeId::I64);
        let prot = b.iconst(0x04, TypeId::I64);
        let r = b.call(FuncRef(valloc), &[zero, p[0], ty, prot], &[TypeId::I64]);
        b.ret(&r);
        let f = b.finish().expect("build");
        out.push(("__rust_alloc".to_string(), compile_with_isa(&f, isa_name)?));
    }

    // __rust_dealloc(ptr, size, align) —— VirtualFree(ptr, 0, MEM_RELEASE=0x8000)
    {
        let sig = FunctionSignature::new(
            &[
                (TypeId::I64, "ptr"),
                (TypeId::I64, "size"),
                (TypeId::I64, "align"),
            ],
            &[],
        );
        let mut b = FunctionBuilder::new("__rust_dealloc", TypeContext::new(), sig);
        let (_blk, p) = b.create_entry_block();
        let zero = b.iconst(0, TypeId::I64);
        let rel = b.iconst(0x8000, TypeId::I64);
        b.call(FuncRef(vfree), &[p[0], zero, rel], &[]);
        b.ret(&[]);
        let f = b.finish().expect("build");
        out.push((
            "__rust_dealloc".to_string(),
            compile_with_isa(&f, isa_name)?,
        ));
    }

    // __rust_alloc_zeroed(size, align) —— VirtualAlloc 新页面已清零，直接分配
    {
        let sig = FunctionSignature::new(
            &[(TypeId::I64, "size"), (TypeId::I64, "align")],
            &[TypeId::I64],
        );
        let mut b = FunctionBuilder::new("__rust_alloc_zeroed", TypeContext::new(), sig);
        let (_blk, p) = b.create_entry_block();
        let r = b.call(FuncRef(rust_alloc), &[p[0], p[1]], &[TypeId::I64]);
        b.ret(&r);
        let f = b.finish().expect("build");
        out.push((
            "__rust_alloc_zeroed".to_string(),
            compile_with_isa(&f, isa_name)?,
        ));
    }

    // __rust_realloc(ptr, old_size, align, new_size) -> *mut u8
    // 新分配 → 逐 8 字节复制 min(old, new) → 释放旧块
    {
        let sig = FunctionSignature::new(
            &[
                (TypeId::I64, "ptr"),
                (TypeId::I64, "old"),
                (TypeId::I64, "align"),
                (TypeId::I64, "new"),
            ],
            &[TypeId::I64],
        );
        let mut b = FunctionBuilder::new("__rust_realloc", TypeContext::new(), sig);
        let (entry, p) = b.create_entry_block();
        // 先创建辅助块（create_block_with_params 会切换 cur_block）
        let (loop_blk, lp) = b.create_block_with_params(&[(TypeId::I64, "i")]);
        let (body_blk, bp) = b.create_block_with_params(&[(TypeId::I64, "i")]);
        let done_blk = b.create_block();
        // 回 entry：call alloc → min(old,new) → jump loop_blk
        //（原实现把 iconst/jump 写在 create_block_with_params 之后，导致这些指令
        //  被追加到 body_blk、entry 无终结符——finish() 校验暴露，修复为显式切回）
        b.switch_to_block(entry);
        let nr = b.call(FuncRef(rust_alloc), &[p[3], p[2]], &[TypeId::I64]);
        let new_ptr = nr[0];
        let lt = b.icmp(IntCC::UnsignedLessThan, p[1], p[3]);
        let n = b.select(lt, p[1], p[3]);
        let zero = b.iconst(0, TypeId::I64);
        let eight = b.iconst(8, TypeId::I64);
        b.jump(loop_blk, &[zero]);
        b.switch_to_block(loop_blk);
        let i = lp[0];
        let cond = b.icmp(IntCC::UnsignedLessThan, i, n);
        b.branch(cond, body_blk, &[i], done_blk, &[]);
        b.switch_to_block(body_blk);
        let ib = bp[0];
        let src = b.iadd(p[0], ib);
        let dst = b.iadd(new_ptr, ib);
        let v = b.load(src, TypeId::I64);
        b.store(v, dst);
        let i2 = b.iadd(ib, eight);
        b.jump(loop_blk, &[i2]);
        b.switch_to_block(done_blk);
        b.call(FuncRef(rust_dealloc), &[p[0], p[1], p[2]], &[]);
        b.ret(&[new_ptr]);
        let f = b.finish().expect("build");
        out.push((
            "__rust_realloc".to_string(),
            compile_with_isa(&f, isa_name)?,
        ));
    }

    // __rust_no_alloc_shim_is_unstable_v2 —— std/alloc 的 allocator shim 稳定性标记
    //（std 中定义为空项；此处注入一个空函数满足链接器对符号的定义要求）
    {
        let sig = FunctionSignature::new(&[], &[]);
        let shim_sym = rustc_symbol_mangling::mangle_internal_symbol(
            tcx,
            "__rust_no_alloc_shim_is_unstable_v2",
        );
        let mut b = FunctionBuilder::new(shim_sym.as_str(), TypeContext::new(), sig);
        b.create_entry_block();
        b.ret(&[]);
        let f = b.finish().expect("build");
        out.push((shim_sym.to_string(), compile_with_isa(&f, isa_name)?));
    }

    // __rust_alloc_error_handler —— OOM 处理：直接终止（ud2/unreachable）
    {
        let oom_sym =
            rustc_symbol_mangling::mangle_internal_symbol(tcx, "__rust_alloc_error_handler");
        let sig = FunctionSignature::new(&[(TypeId::I64, "size"), (TypeId::I64, "align")], &[]);
        let mut b = FunctionBuilder::new(oom_sym.as_str(), TypeContext::new(), sig);
        b.create_entry_block();
        b.unreachable();
        let f = b.finish().expect("build");
        out.push((oom_sym.to_string(), compile_with_isa(&f, isa_name)?));
    }

    // ── CRT mem shim：memcpy/memset/memmove/memcmp/strlen ──
    // 当前 nightly msvc 目标的 compiler_builtins 只导出内部符号
    // （compiler_builtins::mem::memcpy 等），而 core/alloc 预编译 rlib 的
    // 部分路径（read_ipv6_addr、str::pattern 等）引用外部 C 符号 memcpy 等——
    // 实测 LLVM 默认后端同样 LNK2019。此处注入 C 层符号补齐链接缺口。
    // 全部用逐字节循环实现（不依赖 libc），保证正确性优先。
    // 注：Windows x64 ABI 传参顺序 = 参数列表顺序（RCX/RDX/R8/R9），与 SysV
    // 相反，故 dst 是 p[0]——与 Windows CRT 的 memcpy(dst, src, n) 签名一致。
    {
        // void* memcpy(void* dst, const void* src, size_t n)
        let sig = FunctionSignature::new(
            &[
                (TypeId::I64, "dst"),
                (TypeId::I64, "src"),
                (TypeId::I64, "n"),
            ],
            &[TypeId::I64],
        );
        let mut b = FunctionBuilder::new("memcpy", TypeContext::new(), sig);
        let (_entry, p) = b.create_entry_block();
        let loop_blk = b.create_block();
        let body_blk = b.create_block();
        let done = b.create_block();
        let zero = b.iconst(0, TypeId::I64);
        let one = b.iconst(1, TypeId::I64);
        // 循环索引存栈槽（block 参数在独立 shim 的 regalloc 未写初值）
        let slot = b.stack_addr(-8);
        b.store(zero, slot);
        b.jump(loop_blk, &[]);
        // loop：i < n ? body : done
        b.switch_to_block(loop_blk);
        let li = b.load(slot, TypeId::I64);
        let lt = b.icmp(IntCC::UnsignedLessThan, li, p[2]);
        b.branch(lt, body_blk, &[], done, &[]);
        // body：dst[i] = src[i]; i+1 存槽 → loop
        b.switch_to_block(body_blk);
        let bi = b.load(slot, TypeId::I64);
        let sa = b.iadd(p[1], bi);
        let v = b.load(sa, TypeId::I8);
        let da = b.iadd(p[0], bi);
        b.store(v, da);
        let i2 = b.iadd(bi, one);
        b.store(i2, slot);
        b.jump(loop_blk, &[]);
        b.switch_to_block(done);
        b.ret(&[p[0]]);
        let f = b.finish().expect("build");
        out.push(("memcpy".to_string(), compile_with_isa(&f, isa_name)?));
    }

    {
        // void* memset(void* dst, int c, size_t n)
        let sig = FunctionSignature::new(
            &[(TypeId::I64, "dst"), (TypeId::I64, "c"), (TypeId::I64, "n")],
            &[TypeId::I64],
        );
        let mut b = FunctionBuilder::new("memset", TypeContext::new(), sig);
        let (_entry, p) = b.create_entry_block();
        let loop_blk = b.create_block();
        let body_blk = b.create_block();
        let done = b.create_block();
        let zero = b.iconst(0, TypeId::I64);
        let one = b.iconst(1, TypeId::I64);
        let cv = b.ireduce(p[1], TypeId::I8);
        let slot = b.stack_addr(-8);
        b.store(zero, slot);
        b.jump(loop_blk, &[]);
        b.switch_to_block(loop_blk);
        let li = b.load(slot, TypeId::I64);
        let lt = b.icmp(IntCC::UnsignedLessThan, li, p[2]);
        b.branch(lt, body_blk, &[], done, &[]);
        b.switch_to_block(body_blk);
        let bi = b.load(slot, TypeId::I64);
        let da = b.iadd(p[0], bi);
        b.store(cv, da);
        let i2 = b.iadd(bi, one);
        b.store(i2, slot);
        b.jump(loop_blk, &[]);
        b.switch_to_block(done);
        b.ret(&[p[0]]);
        let f = b.finish().expect("build");
        out.push(("memset".to_string(), compile_with_isa(&f, isa_name)?));
    }

    {
        // void* memmove(void* dst, const void* src, size_t n)
        // 重叠安全：dst < src 正向复制，否则反向（从尾部开始）
        let sig = FunctionSignature::new(
            &[
                (TypeId::I64, "dst"),
                (TypeId::I64, "src"),
                (TypeId::I64, "n"),
            ],
            &[TypeId::I64],
        );
        let mut b = FunctionBuilder::new("memmove", TypeContext::new(), sig);
        let (_entry, p) = b.create_entry_block();
        let fwd_lt = b.icmp(IntCC::UnsignedLessThan, p[0], p[1]);
        // 反向起点 i = n - 1；条件 i < n（无符号回绕使 n-1 >= 0 恒成立后递减）
        let one = b.iconst(1, TypeId::I64);
        let nm1 = b.isub(p[2], one);
        let z = b.iconst(0, TypeId::I64);
        let (fwd_loop, fwd_body, rev_loop, rev_body, done) = (
            b.create_block(),
            b.create_block(),
            b.create_block(),
            b.create_block(),
            b.create_block(),
        );
        let fwd_slot = b.stack_addr(-8);
        let rev_slot = b.stack_addr(-16);
        b.store(z, fwd_slot);
        b.store(nm1, rev_slot);
        b.branch(fwd_lt, fwd_loop, &[], rev_loop, &[]);
        // 正向 loop：i < n ? fwd_body : done
        b.switch_to_block(fwd_loop);
        let fli = b.load(fwd_slot, TypeId::I64);
        let flt = b.icmp(IntCC::UnsignedLessThan, fli, p[2]);
        b.branch(flt, fwd_body, &[], done, &[]);
        // 正向 body：dst[i] = src[i]; i+1 存槽 → fwd_loop
        b.switch_to_block(fwd_body);
        let fbi = b.load(fwd_slot, TypeId::I64);
        let fsa = b.iadd(p[1], fbi);
        let fv = b.load(fsa, TypeId::I8);
        let fda = b.iadd(p[0], fbi);
        b.store(fv, fda);
        let fi2 = b.iadd(fbi, one);
        b.store(fi2, fwd_slot);
        b.jump(fwd_loop, &[]);
        // 反向 loop：i < n ? rev_body : done（i 从 n-1 递减，下溢到最大无符号后退出）
        b.switch_to_block(rev_loop);
        let rli = b.load(rev_slot, TypeId::I64);
        let rlt = b.icmp(IntCC::UnsignedLessThan, rli, p[2]);
        b.branch(rlt, rev_body, &[], done, &[]);
        // 反向 body：dst[i] = src[i]; i-1 存槽 → rev_loop
        b.switch_to_block(rev_body);
        let rbi = b.load(rev_slot, TypeId::I64);
        let rsa = b.iadd(p[1], rbi);
        let rv = b.load(rsa, TypeId::I8);
        let rda = b.iadd(p[0], rbi);
        b.store(rv, rda);
        let ri2 = b.isub(rbi, one);
        b.store(ri2, rev_slot);
        b.jump(rev_loop, &[]);
        b.switch_to_block(done);
        b.ret(&[p[0]]);
        let f = b.finish().expect("build");
        out.push(("memmove".to_string(), compile_with_isa(&f, isa_name)?));
    }

    {
        // int memcmp(const void* a, const void* b, size_t n)
        // 返回首个不同字节的差值（无符号比较）；全部相同返回 0
        let sig = FunctionSignature::new(
            &[(TypeId::I64, "a"), (TypeId::I64, "b"), (TypeId::I64, "n")],
            &[TypeId::I32],
        );
        let mut b = FunctionBuilder::new("memcmp", TypeContext::new(), sig);
        let (_entry, p) = b.create_entry_block();
        let loop_blk = b.create_block();
        let body_blk = b.create_block();
        let diff_blk = b.create_block();
        let done = b.create_block();
        let zero = b.iconst(0, TypeId::I64);
        let one = b.iconst(1, TypeId::I64);
        let slot = b.stack_addr(-8);
        b.store(zero, slot);
        b.jump(loop_blk, &[]);
        // loop：i < n ? body : done
        b.switch_to_block(loop_blk);
        let li = b.load(slot, TypeId::I64);
        let lt = b.icmp(IntCC::UnsignedLessThan, li, p[2]);
        b.branch(lt, body_blk, &[], done, &[]);
        // body：比较 a[i] 与 b[i]；不同 → 返回差值，相同 → i+1 存槽 → loop
        b.switch_to_block(body_blk);
        let bi = b.load(slot, TypeId::I64);
        let aa = b.iadd(p[0], bi);
        let ba = b.iadd(p[1], bi);
        let av8 = b.load(aa, TypeId::I8);
        let bv8 = b.load(ba, TypeId::I8);
        let av = b.uextend(av8, TypeId::I32);
        let bv = b.uextend(bv8, TypeId::I32);
        let ne = b.icmp(IntCC::NotEqual, av, bv);
        let d = b.isub(av, bv);
        let i1 = b.iadd(bi, one);
        b.store(i1, slot);
        b.branch(ne, diff_blk, &[], loop_blk, &[]);
        // diff：返回差值
        b.switch_to_block(diff_blk);
        b.ret(&[d]);
        // done：全部相同返回 0
        b.switch_to_block(done);
        let zero32 = b.iconst(0, TypeId::I32);
        b.ret(&[zero32]);
        let f = b.finish().expect("build");
        out.push(("memcmp".to_string(), compile_with_isa(&f, isa_name)?));
    }

    {
        // size_t strlen(const char* s) — 扫描到 NUL
        // 实现用栈槽存索引 i（-8(%rbp)），避开 block 参数在独立 shim 函数
        // 中 regalloc 未初始化的问题（jump 携带的常量初值会被分到 ABI 参数
        // 寄存器 r9 等，入口垃圾 → 越界）。
        let sig = FunctionSignature::new(&[(TypeId::I64, "s")], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("strlen", TypeContext::new(), sig);
        let (_entry, p) = b.create_entry_block();
        let loop_blk = b.create_block();
        let body_blk = b.create_block();
        let done = b.create_block();
        let zero = b.iconst(0, TypeId::I64);
        let one = b.iconst(1, TypeId::I64);
        let slot = b.stack_addr(-8);
        b.store(zero, slot);
        b.jump(loop_blk, &[]);
        // loop：读 i，s[i]==0 ? done : body
        b.switch_to_block(loop_blk);
        let i = b.load(slot, TypeId::I64);
        let addr = b.iadd(p[0], i);
        let c = b.load(addr, TypeId::I8);
        let c64 = b.uextend(c, TypeId::I64);
        let is_nul = b.icmp(IntCC::Equal, c64, zero);
        b.branch(is_nul, done, &[], body_blk, &[]);
        // body：i+1 存槽后跳回 loop
        b.switch_to_block(body_blk);
        let i2 = b.iadd(i, one);
        b.store(i2, slot);
        b.jump(loop_blk, &[]);
        // done：返回 i
        b.switch_to_block(done);
        let di = b.load(slot, TypeId::I64);
        b.ret(&[di]);
        let f = b.finish().expect("build");
        out.push(("strlen".to_string(), compile_with_isa(&f, isa_name)?));
    }

    Ok(out)
}
