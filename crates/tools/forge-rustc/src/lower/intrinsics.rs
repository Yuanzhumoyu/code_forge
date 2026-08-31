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
