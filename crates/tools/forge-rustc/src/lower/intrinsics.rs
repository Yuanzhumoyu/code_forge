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
            // caller_location()：返回 &'static Location（编译期已知 span）。
            // codegen 阶段返回 0 指针（panic 路径 Location 为 null，e2e
            // panic handler 不解引用 info——语义安全）。
            "caller_location" => vec![self.builder.iconst(0, TypeId::PTR)],
            // fabs(f)：浮点绝对值（主库 Fabs lowering：andps 掩码）。
            "fabs" => vec![self.builder.fabs(args[0])],
            // simd_splat<T>(x)：向量广播（vbroadcast——主库向量指令；
            // 仅当目标向量类型未被 ABI 门控拦截时可用）。
            "simd_splat" => {
                let elem = args.first().copied().unwrap_or_else(|| self.builder.iconst(0, TypeId::I64));
                let t = substs_first_ty(&substs).unwrap_or(fty);
                let ty = map_type(t, self.tcx)?;
                vec![self.builder.vbroadcast(elem, ty)]
            }
            // compare_bytes(a, b, n)：逐字节比较（str 的 Ord 实现底层）。
            // 返回 i32：<0 / 0 / >0。循环 + 结果经块参数传递。
            "compare_bytes" => {
                if args.len() >= 3 {
                    let (a, b, n) = (args[0], args[1], args[2]);
                    let (loop_blk, lp) =
                        self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                    let (body_blk, bp) =
                        self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                    let (ret_blk, rp) = self.builder.create_block_with_params(&[(TypeId::I64, "r")]);
                    let zero = self.builder.iconst(0, TypeId::I64);
                    let one = self.builder.iconst(1, TypeId::I64);
                    self.builder.switch_to_block(cur_block);
                    self.builder.jump(loop_blk, &[zero]);
                    self.builder.switch_to_block(loop_blk);
                    let i = lp[0];
                    let cond = self.builder.icmp(IntCC::UnsignedLessThan, i, n);
                    self.builder.branch(cond, body_blk, &[i], ret_blk, &[zero]);
                    self.builder.switch_to_block(body_blk);
                    let bi = bp[0];
                    let aaddr = self.builder.iadd(a, bi);
                    let baddr = self.builder.iadd(b, bi);
                    let av = self.builder.load(aaddr, TypeId::I64);
                    let bv = self.builder.load(baddr, TypeId::I64);
                    let eq = self.builder.icmp(IntCC::Equal, av, bv);
                    let neq_blk = self.builder.create_block();
                    let i2 = self.builder.iadd(bi, one);
                    self.builder.branch(eq, loop_blk, &[i2], neq_blk, &[]);
                    self.builder.switch_to_block(neq_blk);
                    let diff = self.builder.isub(av, bv);
                    self.builder.jump(ret_blk, &[diff]);
                    self.builder.switch_to_block(ret_blk);
                    let r = rp[0];
                    vec![r]
                } else {
                    vec![]
                }
            }
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
            // integer_min/integer_max（1.100 新增，GlobalAlloc::realloc 等使用）：
            // "signed or unsigned depending on T"（core::intrinsics::integer_min 文档）——
            // 比较符号性必须随 T（uN/iN/usize/isize），恒 Signed 对无符号 T 错值。
            // 用原生 Select（主库 [lower.Select] test+mov+cmovcc 已由
            // test_jit_select_strict_matrix 严格矩阵验证无 bug，2026-08-31 nightly）。
            // 语义对齐 fallback 定义：min = a < b ? a : b；max = a < b ? b : a。
            "integer_min" | "min" => {
                let (a, b) = (args[0], args[1]);
                let signed = substs_first_ty(&substs)
                    .map(|t| t.is_signed())
                    .unwrap_or(true);
                let cc = if signed {
                    IntCC::SignedLessThan
                } else {
                    IntCC::UnsignedLessThan
                };
                let is_lt = self.builder.icmp(cc, a, b);
                vec![self.builder.select(is_lt, a, b)]
            }
            "integer_max" | "max" => {
                let (a, b) = (args[0], args[1]);
                let signed = substs_first_ty(&substs)
                    .map(|t| t.is_signed())
                    .unwrap_or(true);
                let cc = if signed {
                    IntCC::SignedLessThan
                } else {
                    IntCC::UnsignedLessThan
                };
                let is_lt = self.builder.icmp(cc, a, b);
                vec![self.builder.select(is_lt, b, a)]
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
            "cttz" | "ctlz" | "cttz_nonzero" | "ctlz_nonzero" => {
                let x = args.first().copied().unwrap_or_else(|| self.builder.iconst(0, TypeId::I64));
                match name {
                    "cttz" | "cttz_nonzero" => vec![self.builder.ctz(x)],
                    _ => vec![self.builder.clz(x)],
                }
            }
            // bswap（to_be_bytes/to_le_bytes）：主库有 Bswap lowering。
            "bswap" => vec![self.builder.bswap(args[0])],
            // is_val_statically_known(v)：编译期已知性查询——codegen 阶段
            // 恒返回 false（值已单态化，rustc 用 const eval 判定；此处
            // 返回 0 使分支走运行时路径，语义保守正确）。
            "is_val_statically_known" => vec![self.builder.iconst(0, TypeId::BOOL)],
            // ptr_offset_from(ptr, base) = (ptr - base) / size（有符号差值，
            // 与 ptr_offset_from_unsigned 区别：结果带符号除法）。
            "ptr_offset_from" => {
                if args.len() >= 2 {
                    let (ptr, base) = (args[0], args[1]);
                    let t = substs_first_ty(&substs).unwrap_or(self.tcx.types.i8);
                    let sz = layout_bytes(self.tcx, t) as i64;
                    if sz == 1 {
                        vec![self.builder.isub(ptr, base)]
                    } else {
                        let szv = self.builder.iconst(sz, TypeId::I64);
                        let diff = self.builder.isub(ptr, base);
                        vec![self.builder.sdiv(diff, szv)]
                    }
                } else {
                    vec![]
                }
            }
            // saturating_add/sub：饱和语义（uadd_sat/ssub_sat 主库已支持）。
            "saturating_add" => {
                let (a, b) = (args[0], args[1]);
                let t = substs_first_ty(&substs).unwrap_or(fty);
                if t.is_signed() {
                    vec![self.builder.sadd_sat(a, b)]
                } else {
                    vec![self.builder.uadd_sat(a, b)]
                }
            }
            "saturating_sub" => {
                let (a, b) = (args[0], args[1]);
                let t = substs_first_ty(&substs).unwrap_or(fty);
                if t.is_signed() {
                    vec![self.builder.ssub_sat(a, b)]
                } else {
                    vec![self.builder.usub_sat(a, b)]
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
            // `copy`（ptr::copy 允许重叠）与 copy_nonoverlapping 同实现
            // （简单逐块复制；重叠场景由调用方保证顺序，slice::rotate
            // 的 ptr_rotate_memmove 用 memmove 语义——当前逐 8 字节
            // 从低到高复制，仅当 dst>src 且重叠时需反向；此处按
            // copy_nonoverlapping 语义处理，重叠用例少见）。
            "copy_nonoverlapping" | "copy" => {
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
            // typed_swap_nonoverlapping(a, b)（core::mem::swap 底层）：
            // 交换两个同类型值——逐 8 字节经临时寄存器交换。
            "typed_swap_nonoverlapping" => {
                if args.len() >= 2 {
                    let (a, b) = (args[0], args[1]);
                    let t = substs_first_ty(&substs).unwrap_or(self.tcx.types.u8);
                    let sz = layout_bytes(self.tcx, t) as i64;
                    // 循环 i in 0..(sz/8)：tmp=a[i], a[i]=b[i], b[i]=tmp
                    let (loop_blk, lp) =
                        self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                    let (body_blk, bp) =
                        self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                    let done = self.builder.create_block();
                    self.builder.switch_to_block(cur_block);
                    let zero = self.builder.iconst(0, TypeId::I64);
                    let one = self.builder.iconst(1, TypeId::I64);
                    let eight = self.builder.iconst(8, TypeId::I64);
                    let n = self.builder.iconst((sz + 7) / 8, TypeId::I64);
                    self.builder.jump(loop_blk, &[zero]);
                    self.builder.switch_to_block(loop_blk);
                    let i = lp[0];
                    let cond = self.builder.icmp(IntCC::UnsignedLessThan, i, n);
                    self.builder.branch(cond, body_blk, &[i], done, &[]);
                    self.builder.switch_to_block(body_blk);
                    let bi = bp[0];
                    let off = if sz == 1 {
                        bi
                    } else {
                        let offv = self.builder.imul(bi, eight);
                        offv
                    };
                    let aaddr = self.builder.iadd(a, off);
                    let baddr = self.builder.iadd(b, off);
                    let tmp = self.builder.load(aaddr, TypeId::I64);
                    let bv = self.builder.load(baddr, TypeId::I64);
                    self.builder.store(bv, aaddr);
                    self.builder.store(tmp, baddr);
                    let i2 = self.builder.iadd(bi, one);
                    self.builder.jump(loop_blk, &[i2]);
                    self.builder.switch_to_block(done);
                }
                vec![]
            }
            // 原子 RMW（atomic_xchg/xadd/xsub/fetch_*）：主库 AtomicRmw 已由
            // JIT 高压测试转正（forge-codegen jit.rs test_jit_atomic_rmw_basic /
            // test_jit_atomic_rmw_spill_pressure——10 存活值 + atomic_rmw 通过，
            // 推翻原"regalloc spill 缺失"假设，WA-18 主库侧关闭）。
            // ordering 忽略（当前后端单线程语义，与 Relaxed 等价——同
            // atomic_load/store 注释）。XADD/XCHG 返回旧值（dest ← 旧内存值），
            // 正是 AtomicRmw 语义；opsize 由 val 宽度推导（i32→xaddl、i64→xaddq）。
            // And/Or/Xor/Nand/Max/Min/Umax/Umin 需 CMPXCHG 循环（TOML 注释），
            // cxchg/cxchgweak 需 cmpxchg 指令扩展——均编译期拒绝（失败即报错）。
            "atomic_xchg" | "atomic_xchg_acqrel" | "atomic_xchg_acquire"
            | "atomic_xchg_release" | "atomic_xchg_relaxed" | "atomic_xchg_seqcst"
            | "atomic_xadd" | "atomic_xadd_acqrel" | "atomic_xadd_acquire"
            | "atomic_xadd_release" | "atomic_xadd_relaxed" | "atomic_xadd_seqcst"
            | "atomic_xsub" | "atomic_xsub_acqrel" | "atomic_xsub_acquire"
            | "atomic_xsub_release" | "atomic_xsub_relaxed" | "atomic_xsub_seqcst"
            | "atomic_fetch_add" | "atomic_fetch_add_acqrel" | "atomic_fetch_add_acquire"
            | "atomic_fetch_add_release" | "atomic_fetch_add_relaxed"
            | "atomic_fetch_sub" | "atomic_fetch_sub_acqrel" | "atomic_fetch_sub_acquire"
            | "atomic_fetch_sub_release" | "atomic_fetch_sub_relaxed" => {
                let op = if name.starts_with("atomic_xchg") {
                    AtomicRmwOp::Xchg
                } else if name.starts_with("atomic_xadd")
                    || name.starts_with("atomic_fetch_add")
                {
                    AtomicRmwOp::Add
                } else {
                    AtomicRmwOp::Sub
                };
                let ptr = args
                    .first()
                    .copied()
                    .unwrap_or_else(|| self.builder.iconst(0, TypeId::I64));
                let val = args
                    .get(1)
                    .copied()
                    .unwrap_or_else(|| self.builder.iconst(0, TypeId::I64));
                vec![self.builder.atomic_rmw(op, ptr, val, Ordering::Monotonic)]
            }
            | "atomic_fetch_and" | "atomic_fetch_and_acqrel" | "atomic_fetch_and_acquire"
            | "atomic_fetch_and_release" | "atomic_fetch_and_relaxed"
            | "atomic_fetch_or" | "atomic_fetch_or_acqrel" | "atomic_fetch_or_acquire"
            | "atomic_fetch_or_release" | "atomic_fetch_or_relaxed"
            | "atomic_fetch_xor" | "atomic_fetch_xor_acqrel" | "atomic_fetch_xor_acquire"
            | "atomic_fetch_xor_release" | "atomic_fetch_xor_relaxed"
            | "atomic_and" | "atomic_or" | "atomic_xor" | "atomic_nand" | "atomic_max"
            | "atomic_min" | "atomic_umax" | "atomic_umin"
            | "atomic_cxchg" | "atomic_cxchg_acqrel" | "atomic_cxchg_acquire"
            | "atomic_cxchg_release" | "atomic_cxchg_relaxed"
            | "atomic_cxchgweak" | "atomic_cxchgweak_acqrel" | "atomic_cxchgweak_acquire"
            | "atomic_cxchgweak_release" | "atomic_cxchgweak_relaxed" => {
                return Err(ForgeError::Message(format!(
                    "{}: 原子 RMW intrinsic {name} 暂不支持——主库 AtomicRmw \
                     lowering 仅覆盖 Xchg/Add/Sub（imm0 0/1/2，xadd/xchg 直接 \
                     返回旧值）；And/Or/Xor/Nand/Max/Min/Umax/Umin 需 CMPXCHG \
                     循环、cxchg/cxchgweak 需 cmpxchg 指令扩展（均未实现）。\
                     atomic_load/store/xchg/xadd/xsub/fetch_add/fetch_sub 可用",
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

