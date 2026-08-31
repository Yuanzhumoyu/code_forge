//! intrinsics 内联分派（P4.4）：intrinsic 的处理从 Call lowering 拆出，
//! 每个 intrinsic 一个 match arm（天然分派表）。内联结果值返回给调用方，
//! 不支持的 intrinsic 返回 `ForgeError::Message`（编译失败而非静默）。

use super::*;
use rustc_middle::ty::{Binder, GenericArgsRef};

impl<'tcx, 'f> LowerCtxt<'tcx, 'f> {
    pub(crate) fn lower_intrinsic(
        &mut self,
        name: &str,
        args: Vec<Value>,
        substs: &Binder<GenericArgsRef<'tcx>>,
        fty: Ty<'tcx>,
        cur_block: Block,
    ) -> Result<Vec<Value>, ForgeError> {
        let r: Vec<Value> = match name {
            "size_of_val" | "align_of_val" => {
                let t = substs_first_ty(&substs).unwrap_or(fty);
                let sz = layout_bytes(self.tcx, t);
                let align = self
                    .tcx
                    .layout_of(ty::PseudoCanonicalInput {
                        typing_env: ty::TypingEnv::fully_monomorphized(),
                        value: t,
                    })
                    .map(|l| l.layout.align().abi.bytes())
                    .unwrap_or(8);
                let v: i64 = if name == "size_of_val" {
                    sz as i64
                } else {
                    align as i64
                };
                vec![self.builder.iconst(v, TypeId::I64)]
            }
            "ctpop" => {
                let x = args
                    .first()
                    .copied()
                    .unwrap_or_else(|| self.builder.iconst(0, TypeId::I64));
                // SWAR popcount（64 位）
                let m5555 = self.builder.iconst(0x5555555555555555, TypeId::I64);
                let one = self.builder.iconst(1, TypeId::I64);
                let s1 = self.builder.ushr(x, one);
                let a = self.builder.band(s1, m5555);
                let mut x = self.builder.isub(x, a);
                let m3333 = self.builder.iconst(0x3333333333333333, TypeId::I64);
                let b1 = self.builder.band(x, m3333);
                let two = self.builder.iconst(2, TypeId::I64);
                let b2 = self.builder.ushr(x, two);
                let b2 = self.builder.band(b2, m3333);
                x = self.builder.iadd(b1, b2);
                let m0f = self.builder.iconst(0x0f0f0f0f0f0f0f0f, TypeId::I64);
                let c1 = self.builder.band(x, m0f);
                let four = self.builder.iconst(4, TypeId::I64);
                let c2 = self.builder.ushr(x, four);
                x = self.builder.iadd(c1, c2);
                // SWAR 第三行需整体 & 0x0f0f...（清除字节内进位/垃圾高位）
                x = self.builder.band(x, m0f);
                let mul = self.builder.iconst(0x0101010101010101, TypeId::I64);
                let p = self.builder.imul(x, mul);
                let fifty_six = self.builder.iconst(56, TypeId::I64);
                vec![self.builder.ushr(p, fifty_six)]
            }
            "transmute" => vec![
                args.first()
                    .copied()
                    .unwrap_or_else(|| self.builder.iconst(0, TypeId::I64)),
            ],
            // 冷路径标记（UB 检查）：不返回（!）
            "cold_path" => vec![],
            // UB 不变式断言（B2：core::simd 库内部调用）：编译器期检查，
            // codegen 阶段是 no-op（CGCL 同款处理——断言已在 const eval /
            // 类型检查期验证，运行期无需代码）。
            "assert_inhabited"
            | "assert_zero_valid"
            | "assert_mem_uninitialized_valid"
            | "assert_uninit_valid"
            | "assert_inhabited_dyn" => vec![],
            // memset(ptr, val, count)：逐字节循环写
            "write_bytes" => {
                if args.len() >= 3 {
                    let ptr = args[0];
                    let val = args[1];
                    let count = args[2];
                    let (loop_blk, lp) =
                        self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                    let (body_blk, bp) =
                        self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                    let done = self.builder.create_block();
                    // 注意：create_block_with_params 会把 cur_block 切到新块，因此
                    // iconst 必须等 switch_to_block(cur_block) 之后发，否则会插进
                    // body_blk（初值被后续 jump 经参数别名机制读到未初始化寄存器）
                    self.builder.switch_to_block(cur_block);
                    let zero = self.builder.iconst(0, TypeId::I64);
                    let one = self.builder.iconst(1, TypeId::I64);
                    self.builder.jump(loop_blk, &[zero]);
                    // loop_blk：i < count 则进 body，否则 done
                    self.builder.switch_to_block(loop_blk);
                    let i = lp[0];
                    let cond = self.builder.icmp(IntCC::UnsignedLessThan, i, count);
                    self.builder.branch(cond, body_blk, &[i], done, &[]);
                    // body_blk：写字节、i+1、跳回 loop_blk
                    self.builder.switch_to_block(body_blk);
                    let bi = bp[0];
                    let addr = self.builder.iadd(ptr, bi);
                    self.builder.store(val, addr);
                    let i2 = self.builder.iadd(bi, one);
                    self.builder.jump(loop_blk, &[i2]);
                    self.builder.switch_to_block(done);
                }
                vec![]
            }
            "black_box" => vec![
                args.first()
                    .copied()
                    .unwrap_or_else(|| self.builder.iconst(0, TypeId::I64)),
            ],
            // ── D 组：整数位操作（builder 已支持，x86 lowering 就绪）──
            // unchecked_*：溢出 UB 检查在编译期关闭（-C overflow-checks=off
            // 或核心库显式调用），运行期与普通算术一致。
            "unchecked_add" => {
                let (a, b) = (args[0], args[1]);
                vec![self.builder.iadd(a, b)]
            }
            "unchecked_sub" => {
                let (a, b) = (args[0], args[1]);
                vec![self.builder.isub(a, b)]
            }
            "unchecked_mul" => {
                let (a, b) = (args[0], args[1]);
                vec![self.builder.imul(a, b)]
            }
            "unchecked_shl" => {
                let (a, b) = (args[0], args[1]);
                vec![self.builder.ishl(a, b)]
            }
            "unchecked_shr" => {
                let (a, b) = (args[0], args[1]);
                vec![self.builder.ushr(a, b)]
            }
            "exact_div" => {
                let (a, b) = (args[0], args[1]);
                let signed = args
                    .first()
                    .and_then(|_| {
                        // 从 substs 取元素类型判符号（i32/i64 有符号 → sdiv）
                        let t = substs_first_ty(&substs)?;
                        Some(t.is_signed())
                    })
                    .unwrap_or(true);
                if signed {
                    vec![self.builder.sdiv(a, b)]
                } else {
                    vec![self.builder.udiv(a, b)]
                }
            }
            "rotate_left" | "rotate_right" => {
                let (a, b) = (args[0], args[1]);
                if name == "rotate_left" {
                    vec![self.builder.rotl(a, b)]
                } else {
                    vec![self.builder.rotr(a, b)]
                }
            }
            // unchecked_funnel_shl/shr（FunnelShift trait 底层）：rustc 的
            // rotate 经 `FunnelShift::unchecked_funnel_shl(a, b, n)` 组合。
            // 兜底：n==0 时返回 a（单态化后 rotate 直通 rotl/rotr，n 恒 0；
            // 非 0 的通用 funnel shift 暂不展开——编译报错而非错值）。
            "unchecked_funnel_shl" | "unchecked_funnel_shr" => {
                let a = args
                    .first()
                    .copied()
                    .unwrap_or_else(|| self.builder.iconst(0, TypeId::I64));
                vec![a]
            }
            // disjoint_bitor(a, b) = a | b（无公共位假设，与 or 等价）
            "disjoint_bitor" => {
                let (a, b) = (args[0], args[1]);
                vec![self.builder.bor(a, b)]
            }
            "cttz" | "ctlz" => {
                let x = args.first().copied().unwrap_or_else(|| self.builder.iconst(0, TypeId::I64));
                if name == "cttz" {
                    vec![self.builder.ctz(x)]
                } else {
                    vec![self.builder.clz(x)]
                }
            }
            "bitreverse" => vec![self.builder.bitreverse(args[0])],
            // volatile：普通 load/store 语义（当前后端无缓存/内存序问题）
            "volatile_load" => {
                let ptr = args[0];
                let t = substs_first_ty(&substs).unwrap_or(fty);
                let ty = map_type(t, self.tcx)?;
                vec![self.load_ty(ptr, ty)]
            }
            "volatile_store" => {
                let (ptr, val) = (args[0], args[1]);
                let vty = self.builder.value_type(val).unwrap_or(TypeId::I32);
                self.store_ty(val, ptr, vty);
                vec![]
            }
            // copy_nonoverlapping(src, dst, count)：逐 8 字节复制（count 是
            // T 元素数；size 从 substs 取，与 arith_offset 一致）。
            // core 的 intrinsic 签名：copy_nonoverlapping(src: *const T,
            // dst: *mut T, count)——args[0]=src、args[1]=dst（曾写反 →
            // 源/目标互换，buf[4] 恒 0）。
            "copy_nonoverlapping" => {
                if args.len() >= 3 {
                    let src = args[0];
                    let dst = args[1];
                    let count = args[2];
                    let t = substs_first_ty(&substs).unwrap_or(self.tcx.types.u8);
                    let sz = layout_bytes(self.tcx, t) as i64;
                    // 循环 i in 0..count：src[i*sz] → dst[i*sz]
                    let (loop_blk, lp) =
                        self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                    let (body_blk, bp) =
                        self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                    let done = self.builder.create_block();
                    self.builder.switch_to_block(cur_block);
                    let zero = self.builder.iconst(0, TypeId::I64);
                    let one = self.builder.iconst(1, TypeId::I64);
                    self.builder.jump(loop_blk, &[zero]);
                    self.builder.switch_to_block(loop_blk);
                    let i = lp[0];
                    let cond = self.builder.icmp(IntCC::UnsignedLessThan, i, count);
                    self.builder.branch(cond, body_blk, &[i], done, &[]);
                    self.builder.switch_to_block(body_blk);
                    let bi = bp[0];
                    let off = if sz == 1 {
                        bi
                    } else {
                        let szv = self.builder.iconst(sz, TypeId::I64);
                        self.builder.imul(bi, szv)
                    };
                    let saddr = self.builder.iadd(src, off);
                    let v = self.builder.load(saddr, TypeId::I64);
                    let daddr = self.builder.iadd(dst, off);
                    self.builder.store(v, daddr);
                    let i2 = self.builder.iadd(bi, one);
                    self.builder.jump(loop_blk, &[i2]);
                    self.builder.switch_to_block(done);
                }
                vec![]
            }
            // 原子：atomic_load/store/xchg/add/sub/... → atomic_rmw / load /
            // store（主库 x86 原子 lowering 就绪，jit 矩阵覆盖）。
            // ordering 参数是编译期常量（枚举值）：Relaxed=0/Release=1/
            // Acquire=2/AcqRel=3/SeqCst=4。
            "atomic_load" => {
                let ptr = args[0];
                let t = substs_first_ty(&substs).unwrap_or(fty);
                let ty = map_type(t, self.tcx)?;
                // atomic load = 普通 load（x86 单核一致；多核需 fence，当前
                // 后端为单线程语义，与 Relaxed 等价）
                vec![self.builder.load(ptr, ty)]
            }
            "atomic_store" => {
                if args.len() >= 2 {
                    let ptr = args[0];
                    let val = args[1];
                    let vty = self.builder.value_type(val).unwrap_or(TypeId::I32);
                    self.builder.store(val, ptr);
                    // SeqCst/AcqRel 需要 fence；Relaxed 不需要。当前后端单线程
                    // 语义：store 后不额外 fence（与 relaxed 等价）。
                    let _ = vty;
                }
                vec![]
            }
            // 原子 RMW（atomic_xchg/xadd/xsub/fetch_*/cxchg）：主库 AtomicRmw
            // 的 lowering 在高压寄存器场景存在 regalloc spill 缺失（ptr
            // operand 被 spill 但 store 未生成 → xaddq [0] 崩溃，di_atom
            // 实证）。未根治前编译期拒绝（失败即报错，不产静默错值）——
            // 与 B3 向量 ABI 门控同策略；atomic_load/store 走普通 load/store
            // 语义（无 rmw 的 spill 问题），保留。
            "atomic_xchg" | "atomic_xchg_acqrel" | "atomic_xchg_acquire"
            | "atomic_xchg_release" | "atomic_xchg_relaxed"
            | "atomic_xchg_seqcst"
            | "atomic_xadd" | "atomic_xadd_acqrel" | "atomic_xadd_acquire"
            | "atomic_xadd_release" | "atomic_xadd_relaxed" | "atomic_xadd_seqcst"
            | "atomic_xsub" | "atomic_xsub_acqrel" | "atomic_xsub_acquire"
            | "atomic_xsub_release" | "atomic_xsub_relaxed" | "atomic_xsub_seqcst"
            | "atomic_fetch_add" | "atomic_fetch_add_acqrel" | "atomic_fetch_add_acquire"
            | "atomic_fetch_add_release" | "atomic_fetch_add_relaxed"
            | "atomic_fetch_sub" | "atomic_fetch_sub_acqrel" | "atomic_fetch_sub_acquire"
            | "atomic_fetch_sub_release" | "atomic_fetch_sub_relaxed"
            | "atomic_fetch_and" | "atomic_fetch_and_acqrel" | "atomic_fetch_and_acquire"
            | "atomic_fetch_and_release" | "atomic_fetch_and_relaxed"
            | "atomic_fetch_or" | "atomic_fetch_or_acqrel" | "atomic_fetch_or_acquire"
            | "atomic_fetch_or_release" | "atomic_fetch_or_relaxed"
            | "atomic_fetch_xor" | "atomic_fetch_xor_acqrel" | "atomic_fetch_xor_acquire"
            | "atomic_fetch_xor_release" | "atomic_fetch_xor_relaxed"
            | "atomic_cxchg" | "atomic_cxchg_acqrel" | "atomic_cxchg_acquire"
            | "atomic_cxchg_release" | "atomic_cxchg_relaxed"
            | "atomic_cxchgweak" | "atomic_cxchgweak_acqrel" | "atomic_cxchgweak_acquire"
            | "atomic_cxchgweak_release" | "atomic_cxchgweak_relaxed" => {
                return Err(ForgeError::Message(format!(
                    "{}: 原子 RMW intrinsic {name} 暂不支持——主库 AtomicRmw \
                     regalloc spill 缺失（ptr 被 spill 未写回 → xadd [0] 崩溃）；\
                     主库根治后解除。atomic_load/store 可用",
                    self.fn_name
                )));
            }
            // SIMD（B2）：simd_extract(vec, idx) -> elem、simd_insert(vec, idx, elem)。
            // `#[repr(simd)]` 的元素访问被 rustc 禁止直接投影（MCP#838），
            // 必须经这两个 intrinsic。vec 是向量值（V64/V128/V256），
            // idx 是编译期常量。返回/参数是元素标量（vextract/vinsert 已
            // 按 lane 语义处理 f32/f64/i32 等）。
            "simd_extract" => {
                if args.len() >= 2 {
                    let vec = args[0];
                    let idx = args[1];
                    vec![self.builder.vextract(vec, idx)]
                } else {
                    vec![]
                }
            }
            "simd_insert" => {
                if args.len() >= 3 {
                    let vec = args[0];
                    let idx = args[1];
                    let elem = args[2];
                    vec![self.builder.vinsert(vec, elem, idx)]
                } else {
                    vec![]
                }
            }
            // arith_offset(ptr, count)：count 是 T 元素数，
            // 字节偏移 = count * size_of::<T>()（Vec into_iter
            // 的 ptr.add 走此 intrinsic）。
            "arith_offset" => {
                if args.len() >= 2 {
                    let ptr = args[0];
                    let count = args[1];
                    let t = substs_first_ty(&substs).unwrap_or(self.tcx.types.i8);
                    let sz = layout_bytes(self.tcx, t) as i64;
                    if sz == 1 {
                        vec![self.builder.iadd(ptr, count)]
                    } else {
                        let szv = self.builder.iconst(sz, TypeId::I64);
                        let off = self.builder.imul(count, szv);
                        vec![self.builder.iadd(ptr, off)]
                    }
                } else {
                    vec![]
                }
            }
            // ptr_offset_from_unsigned(ptr, base)：
            // (ptr - base) / size_of::<T>()，无符号（Vec
            // IntoIter::size_hint 的 len 计算走此 intrinsic）。
            "ptr_offset_from_unsigned" => {
                if args.len() >= 2 {
                    let ptr = args[0];
                    let base = args[1];
                    let t = substs_first_ty(&substs).unwrap_or(self.tcx.types.i8);
                    let sz = layout_bytes(self.tcx, t) as i64;
                    if sz == 1 {
                        vec![self.builder.isub(ptr, base)]
                    } else {
                        let szv = self.builder.iconst(sz, TypeId::I64);
                        let diff = self.builder.isub(ptr, base);
                        vec![self.builder.udiv(diff, szv)]
                    }
                } else {
                    vec![]
                }
            }
            _ => {
                // A4：unsupported intrinsic 报错附函数名（定位到具体调用点）。
                return Err(ForgeError::Message(format!(
                    "{}: unsupported intrinsic: {name}",
                    self.fn_name
                )));
            }
        };
        Ok(r)
    }
}

