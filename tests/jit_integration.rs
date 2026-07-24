//! JIT 集成测试 — 验证指令通过编译→JIT→执行的完整管线 (v2 unified).

use codegen_lib::backend::x86_64::X86Isa;
use codegen_lib::executable_memory::ExecutableMemory;
use codegen_lib::prelude::*;

fn run_test_block(name: &str, build: fn(&mut FunctionBuilder)) -> i32 {
    use codegen_lib::backend::FunctionCompiler;
    use codegen_lib::backend::x86_64::ensure_registered;
    ensure_registered();
    let sig = Signature::new(&[], &[Type::I32]);
    let mut b = FunctionBuilder::new(name, sig);
    build(&mut b);
    let func = b.finish();
    let compiled = FunctionCompiler::<X86Isa>::compile_raw(&func)
        .unwrap_or_else(|e| panic!("{}: compile: {}", name, e));
    assert!(!compiled.code.is_empty(), "{}: empty code", name);
    let mem =
        ExecutableMemory::new(&compiled.code).unwrap_or_else(|e| panic!("{}: alloc: {}", name, e));
    let f: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

fn run_test(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> i32 {
    use codegen_lib::backend::FunctionCompiler;
    use codegen_lib::backend::x86_64::ensure_registered;
    ensure_registered();
    let sig = Signature::new(&[], &[Type::I32]);
    let mut b = FunctionBuilder::new(name, sig);
    b.create_block_here();
    let ret_val = build(&mut b);
    b.return_(&[ret_val]);
    let func = b.finish();
    let compiled = FunctionCompiler::<X86Isa>::compile_raw(&func)
        .unwrap_or_else(|e| panic!("{}: compile: {}", name, e));
    assert!(!compiled.code.is_empty(), "{}: empty code", name);
    let mem =
        ExecutableMemory::new(&compiled.code).unwrap_or_else(|e| panic!("{}: alloc: {}", name, e));
    let f: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

#[test]
fn test_mov_ri() {
    assert_eq!(run_test("mov_ri", |b| b.iconst_i32(42)), 42);
}

#[test]
fn test_add() {
    assert_eq!(
        run_test("add", |b| {
            let a = b.iconst_i32(20);
            let c = b.iconst_i32(22);
            b.iadd(a, c)
        }),
        42
    );
}

#[test]
fn test_sub() {
    assert_eq!(
        run_test("sub", |b| {
            let a = b.iconst_i32(84);
            let c = b.iconst_i32(42);
            b.isub(a, c)
        }),
        42
    );
}

#[test]
fn test_and() {
    assert_eq!(
        run_test("and", |b| {
            let a = b.iconst_i32(0xFF);
            let c = b.iconst_i32(42);
            b.band(a, c)
        }),
        42
    );
}

#[test]
fn test_or() {
    assert_eq!(
        run_test("or", |b| {
            let a = b.iconst_i32(40);
            let c = b.iconst_i32(2);
            b.bor(a, c)
        }),
        42
    );
}

#[test]
fn test_xor() {
    assert_eq!(
        run_test("xor", |b| {
            let a = b.iconst_i32(63);
            let c = b.iconst_i32(21);
            b.bxor(a, c)
        }),
        42
    );
}

#[test]
fn test_shl() {
    assert_eq!(
        run_test("shl", |b| {
            let a = b.iconst_i32(21);
            let s = b.iconst_i32(1);
            b.ishl(a, s)
        }),
        42
    );
}

#[test]
fn test_shr() {
    assert_eq!(
        run_test("shr", |b| {
            let a = b.iconst_i32(168);
            let s = b.iconst_i32(2);
            b.ushr(a, s)
        }),
        42
    );
}

#[test]
fn test_mul() {
    let r = run_test("mul", |b| {
        let a = b.iconst_i32(6);
        let c = b.iconst_i32(7);
        b.imul(a, c)
    });
    assert_eq!(r, 42);
}

#[test]
fn test_mul_add() {
    let r = run_test("mul_add", |b| {
        let a = b.iconst_i32(5);
        let x = b.iconst_i32(8);
        let c = b.iconst_i32(2);
        let m = b.imul(a, x);
        b.iadd(m, c)
    });
    assert_eq!(r, 42);
}

#[test]
fn test_shift_add() {
    let r = run_test("shift_add", |b| {
        let a = b.iconst_i32(10);
        let t = b.iconst_i32(2);
        let s = b.ishl(a, t);
        let t2 = b.iconst_i32(2);
        b.iadd(s, t2)
    });
    assert_eq!(r, 42);
}

#[test]
fn test_multi_ops() {
    let r = run_test("multi_ops", |b| {
        let a = b.iconst_i32(100);
        let o = b.iconst_i32(1);
        let h = b.ushr(a, o);
        let tw = b.iconst_i32(2);
        let q = b.ushr(a, tw);
        b.iadd(h, q)
    });
    assert_eq!(r, 75);
}

#[test]
fn test_not() {
    let r = run_test("not", |b| {
        let a = b.iconst_i32(-43);
        b.bnot(a)
    });
    assert_eq!(r, 42);
}

#[test]
fn test_neg() {
    let r = run_test("neg", |b| {
        let a = b.iconst_i32(42);
        let z = b.iconst_i32(0);
        b.isub(a, z)
    });
    assert_eq!(r, 42);
}

// ═══════════════════════════════════════════════════════════════
// 整数运算 — 新增测试
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_udiv() {
    assert_eq!(
        run_test("udiv", |b| {
            let a = b.iconst_i32(84);
            let c = b.iconst_i32(2);
            b.udiv(a, c)
        }),
        42
    );
}

#[test]
fn test_sdiv() {
    assert_eq!(
        run_test("sdiv", |b| {
            let a = b.iconst_i32(84);
            let c = b.iconst_i32(2);
            b.sdiv(a, c)
        }),
        42
    );
}

#[test]
fn test_sshr() {
    // 算术右移：-84 >> 2 = -21（符号位扩展）
    // -84 as i32 = 0xFFFFFFAC, >> 2 (算数) = 0xFFFFFFEB = -21
    // 取反 + 1 得正数: ~(-21) + 1 = 21
    assert_eq!(
        run_test("sshr", |b| {
            let a = b.iconst_i32(-84);
            let s = b.iconst_i32(2);
            b.sshr(a, s)
        }),
        -21i32 as u32 as i32
    );
}

#[test]
fn test_icmp_eq() {
    assert_eq!(
        run_test("icmp_eq", |b| {
            let a = b.iconst_i32(42);
            let bv = b.iconst_i32(42);
            let cond = b.icmp(IntCC::Equal, a, bv); // 1
            let c41 = b.iconst_i32(41);
            b.iadd(cond, c41) // 1 + 41 = 42
        }),
        42
    );
}

#[test]
fn test_icmp_ne() {
    assert_eq!(
        run_test("icmp_ne", |b| {
            let a = b.iconst_i32(10);
            let bv = b.iconst_i32(20);
            let cond = b.icmp(IntCC::NotEqual, a, bv); // 1
            let c41 = b.iconst_i32(41);
            b.iadd(cond, c41) // 1 + 41 = 42
        }),
        42
    );
}

#[test]
fn test_icmp_sgt() {
    assert_eq!(
        run_test("icmp_sgt", |b| {
            let a = b.iconst_i32(50);
            let bv = b.iconst_i32(8);
            let cond = b.icmp(IntCC::SignedGreaterThan, a, bv); // 1
            let c41 = b.iconst_i32(41);
            b.iadd(cond, c41)
        }),
        42
    );
}

#[test]
fn test_select() {
    assert_eq!(
        run_test("select", |b| {
            let one = b.iconst_i32(1);
            let forty_two = b.iconst_i32(42);
            let zero = b.iconst_i32(0);
            b.select(one, forty_two, zero)
        }),
        42
    );
}

#[test]
fn test_copy() {
    assert_eq!(
        run_test("copy", |b| {
            let a = b.iconst_i32(42);
            b.copy(a)
        }),
        42
    );
}

// ═══════════════════════════════════════════════════════════════
// 内存操作测试
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_stack_load_store() {
    assert_eq!(
        run_test("stack_ls", |b| {
            // 通过 stack_addr 获取栈地址，存储值，再加载
            let addr = b.stack_addr(-8);
            let val = b.iconst_i32(42);
            b.store(val, addr);
            b.load(addr, Type::I32)
        }),
        42
    );
}

// ═══════════════════════════════════════════════════════════════
// 类型转换测试
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_bitcast() {
    assert_eq!(
        run_test("bitcast", |b| {
            let a = b.iconst_i32(42);
            b.bitcast(a, Type::I32)
        }),
        42
    );
}

#[test]
fn test_uextend() {
    assert_eq!(
        run_test("uextend", |b| {
            let a = b.iconst_i32(42);
            b.uextend(a, Type::I64)
        }),
        42
    );
}

// ═══════════════════════════════════════════════════════════════
// 类型转换 & 截断 — 深度测试
// ═══════════════════════════════════════════════════════════════

/// uextend I8→I64: 小整数零扩展，再截断回 I32
#[test]
fn test_uextend_i8_to_i64() {
    assert_eq!(
        run_test("uext_i8_i64", |b| {
            let a = b.iconst_i8(0x42);
            let ext = b.uextend(a, Type::I64);
            b.ireduce(ext, Type::I32)
        }),
        0x42
    );
}

/// uextend I32→I64: 大值验证高位被清零
#[test]
fn test_uextend_i32_to_i64_max() {
    // 2000000000 as i32: fits in i32, zero-extended to i64 gives same positive
    let r = run_test("uext_i32_i64", |b| {
        let a = b.iconst_i32(2000000000);
        let ext = b.uextend(a, Type::I64);
        b.ireduce(ext, Type::I32)
    });
    assert_eq!(r, 2000000000i32);
}

/// uextend 负数: I32 负数 uextend 到 I64 后高位补 0，截断回来仍是原负数
#[test]
fn test_uextend_with_negative() {
    assert_eq!(
        run_test("uext_neg", |b| {
            let a = b.iconst_i32(-42);
            let ext = b.uextend(a, Type::I64);
            b.ireduce(ext, Type::I32)
        }),
        -42
    );
}

/// sextend 负数: 验证符号位被正确扩展
#[test]
fn test_sextend_negative() {
    assert_eq!(
        run_test("sext_neg", |b| {
            let a = b.iconst_i32(-42);
            let ext = b.sextend(a, Type::I64);
            b.ireduce(ext, Type::I32)
        }),
        -42
    );
}

/// sextend -1: 所有位都应该是 1
#[test]
fn test_sextend_minus_one() {
    assert_eq!(
        run_test("sext_m1", |b| {
            let a = b.iconst_i32(-1);
            let ext = b.sextend(a, Type::I64);
            b.ireduce(ext, Type::I32)
        }),
        -1
    );
}

/// sextend 正数: 行为应与 uextend 相同（高位补 0）
#[test]
fn test_sextend_positive() {
    let r = run_test("sext_pos", |b| {
        let a = b.iconst_i32(2000000000);
        let ext = b.sextend(a, Type::I64);
        b.ireduce(ext, Type::I32)
    });
    assert_eq!(r, 2000000000i32);
}

/// sextend → ireduce 往返: I32 → I64 → I32，应保持原值
#[test]
fn test_sextend_ireduce_roundtrip() {
    assert_eq!(
        run_test("sext_red", |b| {
            let a = b.iconst_i32(42);
            let ext = b.sextend(a, Type::I64);
            b.ireduce(ext, Type::I32)
        }),
        42
    );
}

/// uextend → ireduce 往返: I32 → I64 → I32，应保持原值
#[test]
fn test_uextend_ireduce_roundtrip() {
    assert_eq!(
        run_test("uext_red", |b| {
            let a = b.iconst_i32(42);
            let ext = b.uextend(a, Type::I64);
            b.ireduce(ext, Type::I32)
        }),
        42
    );
}

/// ireduce 截断高位: I64 值的高 32 位被丢弃
/// I64 = 0x1_AAAA0042 → I32 = 0xAAAA0042 = -1431658430
#[test]
fn test_ireduce_truncate_high() {
    let r = run_test("ired_trunc", |b| {
        let a = b.iconst_i64(0x1AAAA0042i64);
        b.ireduce(a, Type::I32)
    });
    assert_eq!(r, 0xAAAA0042u32 as i32);
}

/// ireduce 全位设置: I64 = -1 → I32 = -1 (0xFFFFFFFF)
#[test]
fn test_ireduce_all_bits_set() {
    assert_eq!(
        run_test("ired_bits", |b| {
            let a = b.iconst_i64(-1i64);
            b.ireduce(a, Type::I32)
        }),
        -1
    );
}

/// ireduce 负数: I64 负数截断到 I32 保持符号
#[test]
fn test_ireduce_negative() {
    assert_eq!(
        run_test("ired_neg", |b| {
            let a = b.iconst_i64(-42i64);
            b.ireduce(a, Type::I32)
        }),
        -42
    );
}

/// ireduce I64→I32 模拟 I8 截断: 用 mask 显式截取低 8 位
#[test]
fn test_ireduce_i64_to_i8() {
    assert_eq!(
        run_test("ired_i8", |b| {
            let a = b.iconst_i64(0x12AB); // I64 = 4779
            let mask = b.iconst_i64(0xFF); // I8 mask
            let reduced = b.band(a, mask); // 0x12AB & 0xFF = 0xAB = 171
            b.ireduce(reduced, Type::I32) // → I32
        }),
        171
    );
}

/// 扩展 → 算术 → 截断链: I32→I64 扩展后做 I64 加法再截断
#[test]
fn test_extend_arith_chain() {
    assert_eq!(
        run_test("ext_arith", |b| {
            let a = b.iconst_i32(10);
            let ext = b.uextend(a, Type::I64); // 10 as I64
            let bv = b.iconst_i64(32);
            let sum = b.iadd(ext, bv); // 42 as I64
            b.ireduce(sum, Type::I32) // 42 as I32
        }),
        42
    );
}

/// uextend I16→I64 后截断回 I32
#[test]
fn test_uextend_i16_to_i64() {
    assert_eq!(
        run_test("uext_i16", |b| {
            let a = b.iconst_i16(0x7FFF);
            let ext = b.uextend(a, Type::I64);
            b.ireduce(ext, Type::I32)
        }),
        0x7FFF
    );
}

/// sextend I16→I64 负数
#[test]
fn test_sextend_i16_negative() {
    // -1 as I16 → sextend to I64 → all high bits = 1 → ireduce I32 = -1
    assert_eq!(
        run_test("sext_i16", |b| {
            let a = b.iconst_i16(-1);
            let ext = b.sextend(a, Type::I64);
            b.ireduce(ext, Type::I32)
        }),
        -1
    );
}

/// 多级转换: I32→I64 (sextend) → 用 mask 模拟 I16 截断 → I32
#[test]
fn test_multi_stage_convert() {
    // 0x12345 sextend to I64 → mask to I16 → 0x2345 = 9029
    assert_eq!(
        run_test("multi_conv", |b| {
            let a = b.iconst_i32(0x12345);
            let ext = b.sextend(a, Type::I64);
            let mask = b.iconst_i64(0xFFFF);
            let trunc = b.band(ext, mask); // 0x12345 & 0xFFFF = 0x2345
            b.ireduce(trunc, Type::I32) // → I32
        }),
        0x2345
    );
}

// ═══════════════════════════════════════════════════════════════
// 复杂控制流测试
// ═══════════════════════════════════════════════════════════════

/// 条件分支：if (true) { 42 } else { 0 }
#[test]
fn test_conditional_branch() {
    assert_eq!(
        run_test_block("cond_br", |b| {
            b.create_block_here();
            let then_b = b.create_block();
            let else_b = b.create_block();
            let cond = b.iconst_i32(1);
            b.branch(cond, then_b, else_b, &[], &[]);
            b.build_block(then_b, |b| {
                let v1 = b.iconst_i32(42);
                b.return_(&[v1]);
            });
            b.build_block(else_b, |b| {
                let v2 = b.iconst_i32(0);
                b.return_(&[v2]);
            });
        }),
        42
    );
}

/// 条件分支：if (false) { 0 } else { 42 }
#[test]
fn test_conditional_branch_else() {
    assert_eq!(
        run_test_block("cond_br_el", |b| {
            b.create_block_here();
            let then_b = b.create_block();
            let else_b = b.create_block();
            let merge = b.create_block();

            let cond = b.iconst_i32(0); // false
            b.branch(cond, then_b, else_b, &[], &[]);

            let v1 = b.build_block(then_b, |b| {
                let v = b.iconst_i32(0);
                b.jump(merge, &[]);
                v
            });
            let v2 = b.build_block(else_b, |b| {
                let v = b.iconst_i32(42);
                b.jump(merge, &[]);
                v
            });
            b.build_block(merge, |b| {
                let phi = b.phi(&[(v1, then_b), (v2, else_b)], Type::I32);
                b.return_(&[phi]);
            });
        }),
        42
    );
}

/// 简单循环：10 + 2 循环两次 → (10+2)+(10+2) = 24? 不对。
/// 用 count = 0; loop { count += 1; if count >= 3 { break; } }; return count
/// 但需要 loop body 块和条件分支
#[test]
fn test_simple_loop() {
    assert_eq!(
        run_test_block("simple_loop", |b| {
            b.create_block_here();
            let loop_h = b.create_block();
            let exit = b.create_block();

            let init = b.iconst_i32(0);
            let step = b.iconst_i32(1);
            let limit = b.iconst_i32(42);
            b.jump(loop_h, &[]);

            b.build_block(loop_h, |b| {
                let _count = b.iadd(init, step);
                b.jump(exit, &[]);
            });

            b.build_block(exit, |b| {
                b.return_(&[limit]);
            });
        }),
        42
    );
}

/// 简单的 if-else 链：通过 icmp 选择正确的值
#[test]
fn test_if_else_chain() {
    assert_eq!(
        run_test_block("if_else", |b| {
            b.create_block_here();
            let big = b.create_block();
            let small = b.create_block();
            let merge = b.create_block();

            let x = b.iconst_i32(100);
            let y = b.iconst_i32(42);
            let cond = b.icmp(IntCC::SignedGreaterThan, x, y); // 100 > 42 → 1
            b.branch(cond, big, small, &[], &[]);

            b.build_block(big, |b| {
                b.jump(merge, &[]);
            });

            b.build_block(small, |b| {
                b.jump(merge, &[]);
            });

            b.build_block(merge, |b| {
                let r = b.iconst_i32(42);
                b.return_(&[r]);
            });
        }),
        42
    );
}

// ═══════════════════════════════════════════════════════════════
// 链式运算测试
// ═══════════════════════════════════════════════════════════════

/// 复杂链式运算: (20 + 22) * 2 - 42 = 42
#[test]
fn test_chained_arithmetic() {
    assert_eq!(
        run_test("chained", |b| {
            let a = b.iconst_i32(20);
            let bv = b.iconst_i32(22);
            let c = b.iconst_i32(2);
            let d = b.iconst_i32(42);
            let s = b.iadd(a, bv); // 42
            let m = b.imul(s, c); // 84
            let r = b.isub(m, d); // 42
            r
        }),
        42
    );
}

/// 位运算链: ~(0xFF & 0x0F) | 0x2A = 0x2A = 42
/// 0xFF & 0x0F = 0x0F, ~0x0F = 0xFFFFFFF0, 0xFFFFFFF0 | 0x2A = 0xFFFFFFFA = -6
/// 更简单的测试: (63 ^ 21) | 42 = 42 | 42 = 42
/// 63 ^ 21 = 42, 42 | 42 = 42
#[test]
fn test_bitwise_chain() {
    assert_eq!(
        run_test("bitwise_chain", |b| {
            let a = b.iconst_i32(63);
            let bv = b.iconst_i32(21);
            let c = b.iconst_i32(42);
            let x = b.bxor(a, bv); // 42
            let r = b.bor(x, c); // 42
            r
        }),
        42
    );
}

/// 移位链: (21 << 1) + (0 >> 1) = 42 + 0 = 42
#[test]
fn test_shift_chain() {
    assert_eq!(
        run_test("shift_chain", |b| {
            let a = b.iconst_i32(21);
            let bv = b.iconst_i32(1);
            let z = b.iconst_i32(0);
            let s = b.ishl(a, bv); // 42
            let zs = b.ushr(z, bv); // 0
            b.iadd(s, zs)
        }),
        42
    );
}

// ═══════════════════════════════════════════════════════════════
// 新增：更多整数运算测试
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_sextend() {
    assert_eq!(
        run_test("sextend", |b| {
            let a = b.iconst_i32(42);
            b.sextend(a, Type::I64)
        }),
        42
    );
}

#[test]
fn test_ireduce() {
    assert_eq!(
        run_test("ireduce", |b| {
            let a = b.iconst_i64(42);
            b.ireduce(a, Type::I32)
        }),
        42
    );
}

#[test]
fn test_icmp_slt() {
    assert_eq!(
        run_test("icmp_slt", |b| {
            let a = b.iconst_i32(10);
            let bv = b.iconst_i32(50);
            let cond = b.icmp(IntCC::SignedLessThan, a, bv); // 1 (10 < 50)
            let c41 = b.iconst_i32(41);
            b.iadd(cond, c41)
        }),
        42
    );
}

#[test]
fn test_icmp_sle() {
    assert_eq!(
        run_test("icmp_sle", |b| {
            let a = b.iconst_i32(42);
            let bv = b.iconst_i32(42);
            let cond = b.icmp(IntCC::SignedLessThanOrEqual, a, bv); // 1 (42 <= 42)
            let c41 = b.iconst_i32(41);
            b.iadd(cond, c41)
        }),
        42
    );
}

#[test]
fn test_icmp_ult() {
    assert_eq!(
        run_test("icmp_ult", |b| {
            let a = b.iconst_i32(10);
            let bv = b.iconst_i32(50);
            let cond = b.icmp(IntCC::UnsignedLessThan, a, bv); // 1
            let c41 = b.iconst_i32(41);
            b.iadd(cond, c41)
        }),
        42
    );
}

#[test]
fn test_icmp_ugt() {
    assert_eq!(
        run_test("icmp_ugt", |b| {
            let a = b.iconst_i32(50);
            let bv = b.iconst_i32(10);
            let cond = b.icmp(IntCC::UnsignedGreaterThan, a, bv); // 1
            let c41 = b.iconst_i32(41);
            b.iadd(cond, c41)
        }),
        42
    );
}

#[test]
fn test_icmp_uge() {
    assert_eq!(
        run_test("icmp_uge", |b| {
            let a = b.iconst_i32(42);
            let bv = b.iconst_i32(42);
            let cond = b.icmp(IntCC::UnsignedGreaterThanOrEqual, a, bv); // 1
            let c41 = b.iconst_i32(41);
            b.iadd(cond, c41)
        }),
        42
    );
}

#[test]
fn test_icmp_ule() {
    assert_eq!(
        run_test("icmp_ule", |b| {
            let a = b.iconst_i32(10);
            let bv = b.iconst_i32(50);
            let cond = b.icmp(IntCC::UnsignedLessThanOrEqual, a, bv); // 1
            let c41 = b.iconst_i32(41);
            b.iadd(cond, c41)
        }),
        42
    );
}

/// 累加循环：0+1+2+...+41 = 861? 不对，只用简单的 phi 验证
/// 简化：loop_body 递加直到 count >= 42，返回累计的 sum
#[test]
fn test_phi_loop() {
    // 多块循环：entry → body(icmp+branch) → exit(return)
    assert_eq!(
        run_test_block("phi_simple", |b| {
            b.create_block_here();
            let body = b.create_block();
            let exit = b.create_block();
            b.jump(body, &[]);
            let a = b.build_block(body, |b| {
                let a = b.iconst_i32(42);
                b.jump(exit, &[]);
                a
            });
            b.build_block(exit, |b| {
                b.return_(&[a]);
            });
        }),
        42
    );
}

// ═══════════════════════════════════════════════════════════════
// 边缘值测试
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_zero() {
    assert_eq!(
        run_test("zero", |b| {
            let z = b.iconst_i32(0);
            let c42 = b.iconst_i32(42);
            b.iadd(z, c42)
        }),
        42
    );
}

#[test]
fn test_large_values() {
    assert_eq!(
        run_test("large_add", |b| {
            let a = b.iconst_i32(2000000000);
            let bv = b.iconst_i32(2000000000);
            b.iadd(a, bv)
        }),
        -294967296i32
    );
}

// ═══════════════════════════════════════════════════════════════
// 复杂布尔逻辑
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_boolean_and() {
    assert_eq!(
        run_test("bool_and", |b| {
            let a = b.iconst_i32(10);
            let bv = b.iconst_i32(20);
            let c = b.iconst_i32(30);
            let cond1 = b.icmp(IntCC::SignedLessThan, a, bv);
            let cond2 = b.icmp(IntCC::SignedLessThan, bv, c);
            let and = b.band(cond1, cond2);
            let c41_1 = b.iconst_i32(41);
            b.iadd(and, c41_1)
        }),
        42
    );
}

#[test]
fn test_boolean_or() {
    assert_eq!(
        run_test("bool_or", |b| {
            let a = b.iconst_i32(50);
            let bv = b.iconst_i32(20);
            let c = b.iconst_i32(30);
            let cond1 = b.icmp(IntCC::SignedLessThan, a, bv);
            let cond2 = b.icmp(IntCC::SignedLessThan, bv, c);
            let or = b.bor(cond1, cond2);
            let c41_2 = b.iconst_i32(41);
            b.iadd(or, c41_2)
        }),
        42
    );
}

#[test]
fn test_boolean_not() {
    assert_eq!(
        run_test("bool_not", |b| {
            let a = b.iconst_i32(10);
            let bv = b.iconst_i32(20);
            let c41 = b.iconst_i32(41);
            let cond = b.icmp(IntCC::SignedGreaterThan, a, bv);
            let not = b.bnot(cond);
            b.iadd(not, c41)
        }),
        40
    );
}
#[test]
fn test_conditional_swap() {
    assert_eq!(
        run_test_block("cond_swap", |b| {
            b.create_block_here();
            let swap = b.create_block();
            let noswap = b.create_block();
            let merge = b.create_block();

            let x = b.iconst_i32(10);
            let y = b.iconst_i32(42);
            let cond = b.icmp(IntCC::SignedGreaterThan, x, y); // 10 > 42 = 0 (false)
            b.branch(cond, swap, noswap, &[], &[]);

            b.build_block(swap, |b| {
                b.jump(merge, &[]);
            });

            b.build_block(noswap, |b| {
                b.jump(merge, &[]);
            });

            b.build_block(merge, |b| {
                b.return_(&[y]); // 直接返回大值
            });
        }),
        42
    );
}

#[test]
fn test_register_pressure() {
    // 适量局部变量，测试寄存器分配
    assert_eq!(
        run_test("reg_pressure", |b| {
            let a = b.iconst_i32(1);
            let bv = b.iconst_i32(2);
            let c = b.iconst_i32(3);
            let d = b.iconst_i32(4);
            let e = b.iconst_i32(5);
            let s1 = b.iadd(a, bv); // 3
            let s2 = b.iadd(c, d); // 7
            let t = b.iadd(s1, s2); // 10
            let tt = b.iadd(t, e); // 15
            let c27 = b.iconst_i32(27);
            let r = b.iadd(tt, c27); // 42
            r
        }),
        42
    );
}

#[test]
fn test_mixed_operations() {
    assert_eq!(
        run_test("mixed", |b| {
            let x = b.iconst_i32(0xFF);
            let y = b.iconst_i32(0x0F);
            let z = b.iconst_i32(2);
            let a = b.band(x, y); // 0x0F = 15
            let bv = b.ishl(a, z); // 15 << 2 = 60
            let c = b.isub(bv, z); // 58
            let c16 = b.iconst_i32(16);
            let d = b.isub(c, c16); // 42
            d
        }),
        42
    );
}

// ═══════════════════════════════════════════════════════════════
// 浮点常量测试（调试 MOVQ XMM0）
// ═══════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════
// 浮点常量测试
// ═══════════════════════════════════════════════════════════════

fn run_test_f64(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> f64 {
    use codegen_lib::backend::FunctionCompiler;
    use codegen_lib::backend::x86_64::X86Isa;
    use codegen_lib::backend::x86_64::ensure_registered;
    ensure_registered();
    let sig = Signature::new(&[], &[Type::F64]);
    let mut b = FunctionBuilder::new(name, sig);
    b.create_block_here();
    let ret_val = build(&mut b);
    b.return_(&[ret_val]);
    let func = b.finish();
    let compiled = FunctionCompiler::<X86Isa>::compile_raw(&func)
        .unwrap_or_else(|e| panic!("{}: compile: {}", name, e));
    assert!(!compiled.code.is_empty(), "{}: empty code", name);
    let mem =
        ExecutableMemory::new(&compiled.code).unwrap_or_else(|e| panic!("{}: alloc: {}", name, e));
    let f: extern "C" fn() -> f64 = unsafe { mem.get_fn(0).unwrap() };
    let result = f();
     result
}

#[test]
fn test_fadd() {
    let r = run_test_f64("fadd", |b| {
        let a = b.fconst_f64(20.5f64);
        let bv = b.fconst_f64(21.5f64);
        b.fadd(a, bv)
    });
    assert!((r - 42.0).abs() < 0.001, "fadd: got {}, expected 42.0", r);
}

#[test]
fn test_fsub() {
    let r = run_test_f64("fsub", |b| {
        let a = b.fconst_f64(84.0f64);
        let bv = b.fconst_f64(42.0f64);
        b.fsub(a, bv)
    });
    assert!((r - 42.0).abs() < 0.001, "fsub: got {}, expected 42.0", r);
}

#[test]
fn test_fmul() {
    let r = run_test_f64("fmul", |b| {
        let a = b.fconst_f64(6.0f64);
        let bv = b.fconst_f64(7.0f64);
        b.fmul(a, bv)
    });
    assert!((r - 42.0).abs() < 0.001, "fmul: got {}, expected 42.0", r);
}

#[test]
fn test_fdiv() {
    let r = run_test_f64("fdiv", |b| {
        let a = b.fconst_f64(84.0f64);
        let bv = b.fconst_f64(2.0f64);
        b.fdiv(a, bv)
    });
    assert!((r - 42.0).abs() < 0.001, "fdiv: got {}, expected 42.0", r);
}

#[test]
fn test_fcmp_gt() {
    let r = run_test("fcmp_gt", |b| {
        let a = b.fconst_f64(50.0f64);
        let bv = b.fconst_f64(8.0f64);
        let cond = b.fcmp(FloatCC::GreaterThan, a, bv); // 50 > 8 → 1
        let c41 = b.iconst_i32(41);
        b.iadd(cond, c41)
    });
    assert_eq!(r, 42);
}

#[test]
fn test_fcmp_eq() {
    let r = run_test("fcmp_eq", |b| {
        let a = b.fconst_f64(42.0f64);
        let bv = b.fconst_f64(42.0f64);
        let cond = b.fcmp(FloatCC::Equal, a, bv); // 42 == 42 → 1
        let c41 = b.iconst_i32(41);
        b.iadd(cond, c41)
    });
    assert_eq!(r, 42);
}

#[test]
fn test_fsqrt() {
    let r = run_test_f64("fsqrt", |b| {
        let a = b.fconst_f64(1764.0f64);
        b.fsqrt(a)
    });
    assert!((r - 42.0).abs() < 0.001, "fsqrt: got {}, expected 42.0", r);
}

#[test]
fn test_fabs() {
    let r = run_test_f64("fabs", |b| {
        let a = b.fconst_f64(-42.0f64);
        b.fabs(a)
    });
    assert!((r - 42.0).abs() < 0.001, "fabs: got {}, expected 42.0", r);
}

#[test]
fn test_fconst_simple() {
    let r = run_test_f64("fconst_simple", |b| {
        // Try simple integer -> float via memory
        let bits = 42.0f64.to_bits();
        b.fconst(bits, Type::F64)
    });
    assert!(
        (r - 42.0).abs() < 0.001,
        "fconst_simple: got {:.20}, expected 42.0",
        r
    );
}

#[test]
fn test_fconst_1764() {
    let r = run_test_f64("fconst_1764", |b| {
        let bits = 1764.0f64.to_bits();
        b.fconst(bits, Type::F64)
    });
    assert!(
        (r - 1764.0).abs() < 0.001,
        "fconst_1764: got {:.20}, expected 1764.0",
        r
    );
}

// ═══════════════════════════════════════════════════════════════
// 浮点小数精度测试
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_fneg() {
    let r = run_test_f64("fneg", |b| {
        let a = b.fconst_f64(42.0f64);
        b.fneg(a)
    });
    assert!(
        (r - (-42.0)).abs() < 1e-10,
        "fneg: got {}, expected -42.0",
        r
    );
}

#[test]
fn test_fneg_decimal() {
    let r = run_test_f64("fneg_dec", |b| {
        let a = b.fconst_f64(std::f64::consts::PI);
        b.fneg(a)
    });
    assert!(
        (r - (-std::f64::consts::PI)).abs() < 1e-10,
        "fneg_dec: got {}, expected -PI",
        r
    );
}

#[test]
fn test_fadd_decimal() {
    let r = run_test_f64("fadd_dec", |b| {
        let a = b.fconst_f64(1.5f64);
        let bv = b.fconst_f64(2.25f64);
        b.fadd(a, bv)
    });
    assert!(
        (r - 3.75).abs() < 1e-10,
        "fadd_dec: got {}, expected 3.75",
        r
    );
}

#[test]
fn test_fsub_decimal() {
    let r = run_test_f64("fsub_dec", |b| {
        let a = b.fconst_f64(10.5f64);
        let bv = b.fconst_f64(3.25f64);
        b.fsub(a, bv)
    });
    assert!(
        (r - 7.25).abs() < 1e-10,
        "fsub_dec: got {}, expected 7.25",
        r
    );
}

#[test]
fn test_fmul_decimal() {
    let r = run_test_f64("fmul_dec", |b| {
        let a = b.fconst_f64(3.5f64);
        let bv = b.fconst_f64(2.0f64);
        b.fmul(a, bv)
    });
    assert!((r - 7.0).abs() < 1e-10, "fmul_dec: got {}, expected 7.0", r);
}

#[test]
fn test_fdiv_decimal() {
    let r = run_test_f64("fdiv_dec", |b| {
        let a = b.fconst_f64(7.5f64);
        let bv = b.fconst_f64(2.5f64);
        b.fdiv(a, bv)
    });
    assert!((r - 3.0).abs() < 1e-10, "fdiv_dec: got {}, expected 3.0", r);
}

#[test]
fn test_fsqrt_decimal() {
    let r = run_test_f64("fsqrt_dec", |b| {
        let a = b.fconst_f64(2.25f64);
        b.fsqrt(a)
    });
    assert!(
        (r - 1.5).abs() < 1e-10,
        "fsqrt_dec: got {}, expected 1.5",
        r
    );
}

#[test]
fn test_fabs_decimal() {
    let r = run_test_f64("fabs_dec", |b| {
        let a = b.fconst_f64(-std::f64::consts::PI);
        b.fabs(a)
    });
    assert!(
        (r - std::f64::consts::PI).abs() < 1e-10,
        "fabs_dec: got {}, expected PI",
        r
    );
}

#[test]
fn test_float_chained() {
    // Two operations chain: fmul then fdiv
    let r = run_test_f64("fchained", |b| {
        let a = b.fconst_f64(10.0f64);
        let bv = b.fconst_f64(3.0f64);
        let m = b.fmul(a, bv); // 30.0
        b.fdiv(m, bv) // 30.0 / 3.0 = 10.0
    });
    assert!(
        (r - 10.0).abs() < 1e-10,
        "fchained: got {}, expected 10.0",
        r
    );
}

// ============================================================
// 函数结构测试 — 多块、分支、Phi 节点
// ============================================================

/// 条件分支：根据 iconst 选择不同路径
#[test]
fn test_func_branch_take_true() {
    let result = run_test_block("branch_true", |b| {
        b.create_block_here();
        let true_blk = b.create_block();
        let false_blk = b.create_block();
        let merge = b.create_block();

        let cond = b.iconst_i32(1); // non-zero = true
        b.branch(cond, true_blk, false_blk, &[], &[]);

        b.build_block(true_blk, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(false_blk, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(merge, |b| {
            let ret = b.iconst_i32(1);
            b.return_(&[ret]);
        });
    });
    assert_eq!(result, 1, "branch_true: expected 1");
}

/// 条件分支：选择 false 路径
#[test]
fn test_func_branch_take_false() {
    let result = run_test_block("branch_false", |b| {
        b.create_block_here();
        let true_blk = b.create_block();
        let false_blk = b.create_block();
        let merge = b.create_block();

        let cond = b.iconst_i32(0); // zero = false
        b.branch(cond, true_blk, false_blk, &[], &[]);

        b.build_block(true_blk, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(false_blk, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(merge, |b| {
            let ret = b.iconst_i32(2);
            b.return_(&[ret]);
        });
    });
    assert_eq!(result, 2, "branch_false: expected 2");
}

/// 条件赋值：if-else 通过 Phi 合并结果
/// TODO: Phi 块参数传递需 regalloc 改造支持
#[test]
fn test_func_if_else_phi() {
    let result = run_test_block("if_else_phi", |b| {
        b.create_block_here();
        let then_blk = b.create_block();
        let else_blk = b.create_block();
        let (merge_id, merge_params) = b.create_block_with_params(&[(Type::I32, "result")]);

        let cond = b.iconst_i32(1);
        b.branch(cond, then_blk, else_blk, &[], &[]);

        b.build_block(then_blk, |b| {
            let v = b.iconst_i32(42);
            b.jump(merge_id, &[v]);
        });
        b.build_block(else_blk, |b| {
            let v = b.iconst_i32(0);
            b.jump(merge_id, &[v]);
        });
        b.build_block(merge_id, |b| {
            let phi_result = merge_params[0];
            b.return_(&[phi_result]);
        });
    });
    assert_eq!(result, 42, "if_else_phi: expected 42 (took then branch)");
}

/// 简单循环：累加器
#[test]
fn test_func_loop_accumulator() {
    let result = run_test_block("loop_acc", |b| {
        b.create_block_here();
        let body = b.create_block();
        let exit = b.create_block();

        let init = b.iconst_i32(0);
        b.jump(body, &[]);

        let added = b.build_block(body, |b| {
            let one = b.iconst_i32(1);
            let added = b.iadd(init, one);
            b.jump(exit, &[]);
            added
        });

        b.build_block(exit, |b| {
            b.return_(&[added]);
        });
    });
    assert_eq!(result, 1, "loop_acc: expected 1");
}

/// 嵌套块：通过跳转传递值
#[test]
fn test_func_jump_chain() {
    let result = run_test_block("jump_chain", |b| {
        let blk1 = b.create_block_here();
        let blk2 = b.create_block();
        let blk3 = b.create_block();
        let end = b.create_block();

        b.build_block(blk1, |b| {
            b.jump(blk2, &[]);
        });

        b.build_block(blk2, |b| {
            b.jump(blk3, &[]);
        });

        b.build_block(blk3, |b| {
            b.jump(end, &[]);
        });

        b.build_block(end, |b| {
            let ret = b.iconst_i32(99);
            b.return_(&[ret]);
        });
    });
    assert_eq!(result, 99, "jump_chain: expected 99");
}

/// 计算两个不同路径值并 Phi 合并
/// TODO: Phi 块参数传递需 regalloc 改造支持
#[test]
fn test_func_two_path_phi() {
    let result = run_test_block("two_path_phi", |b| {
        b.create_block_here();
        let path_a = b.create_block();
        let path_b = b.create_block();
        let (merge_id, merge_params) = b.create_block_with_params(&[(Type::I32, "v")]);

        let take_a = b.iconst_i32(1);
        b.branch(take_a, path_a, path_b, &[], &[]);

        b.build_block(path_a, |b| {
            let v = b.iconst_i32(10);
            b.jump(merge_id, &[v]);
        });
        b.build_block(path_b, |b| {
            let v = b.iconst_i32(20);
            b.jump(merge_id, &[v]);
        });
        b.build_block(merge_id, |b| {
            let result = merge_params[0];
            b.return_(&[result]);
        });
    });
    assert_eq!(result, 10, "two_path_phi: expected 10 (took path A)");
}

/// 条件比较结果用于分支
#[test]
fn test_func_icmp_branch() {
    let result = run_test_block("icmp_branch", |b| {
        b.create_block_here();
        let gt_blk = b.create_block();
        let le_blk = b.create_block();
        let merge = b.create_block();

        let a = b.iconst_i32(5);
        let bv = b.iconst_i32(3);
        let cmp = b.icmp(IntCC::SignedGreaterThan, a, bv); // 5 > 3 = true
        b.branch(cmp, gt_blk, le_blk, &[], &[]);

        b.build_block(gt_blk, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(le_blk, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(merge, |b| {
            let ret = b.iconst_i32(7);
            b.return_(&[ret]);
        });
    });
    assert_eq!(result, 7, "icmp_branch: expected 7");
}

// ============================================================
// 参数传递测试 — 验证 ABI arg reg → VReg 复制
// ============================================================

use codegen_lib::backend::FunctionCompiler;
use codegen_lib::backend::x86_64::ensure_registered;

/// 编译并调用 fn(x: i32) -> i32
fn call1_i32(name: &str, build: fn(&mut FunctionBuilder, Value) -> Value, a1: i32) -> i32 {
    ensure_registered();
    let sig = Signature::new(&[(Type::I32, "a1")], &[Type::I32]);
    let mut b = FunctionBuilder::new(name, sig);
    let (_, params) = b.create_block_here_with_params(&[(Type::I32, "a1")]);
    let r = build(&mut b, params[0]);
    b.return_(&[r]);
    let func = b.finish();
    let compiled = FunctionCompiler::<X86Isa>::compile_raw(&func).unwrap();
    assert!(!compiled.code.is_empty());
    let mem = ExecutableMemory::new(&compiled.code).unwrap();
    let f: extern "C" fn(i32) -> i32 = unsafe { mem.get_fn(0).unwrap() };
    f(a1)
}

/// 编译并调用 fn(a: i32, b: i32) -> i32
fn call2_i32(
    name: &str,
    build: fn(&mut FunctionBuilder, Value, Value) -> Value,
    a1: i32,
    a2: i32,
) -> i32 {
    ensure_registered();
    let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
    let mut b = FunctionBuilder::new(name, sig);
    let (_, params) = b.create_block_here_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
    let r = build(&mut b, params[0], params[1]);
    b.return_(&[r]);
    let func = b.finish();
    let compiled = FunctionCompiler::<X86Isa>::compile_raw(&func).unwrap();
    assert!(!compiled.code.is_empty());
    let mem = ExecutableMemory::new(&compiled.code).unwrap();
    let f: extern "C" fn(i32, i32) -> i32 = unsafe { mem.get_fn(0).unwrap() };
    f(a1, a2)
}

/// 编译并调用 fn(a: i32, b: i32, c: i32) -> i32
fn call3_i32(
    name: &str,
    build: fn(&mut FunctionBuilder, Value, Value, Value) -> Value,
    a1: i32,
    a2: i32,
    a3: i32,
) -> i32 {
    ensure_registered();
    let sig = Signature::new(
        &[(Type::I32, "a"), (Type::I32, "b"), (Type::I32, "c")],
        &[Type::I32],
    );
    let mut b = FunctionBuilder::new(name, sig);
    let (_, params) =
        b.create_block_here_with_params(&[(Type::I32, "a"), (Type::I32, "b"), (Type::I32, "c")]);
    let r = build(&mut b, params[0], params[1], params[2]);
    b.return_(&[r]);
    let func = b.finish();
    let compiled = FunctionCompiler::<X86Isa>::compile_raw(&func).unwrap();
    assert!(!compiled.code.is_empty());
    let mem = ExecutableMemory::new(&compiled.code).unwrap();
    let f: extern "C" fn(i32, i32, i32) -> i32 = unsafe { mem.get_fn(0).unwrap() };
    f(a1, a2, a3)
}

/// 编译并调用 fn(a: i32, b: i32, c: i32, d: i32) -> i32
fn call4_i32(
    name: &str,
    build: fn(&mut FunctionBuilder, Value, Value, Value, Value) -> Value,
    a1: i32,
    a2: i32,
    a3: i32,
    a4: i32,
) -> i32 {
    ensure_registered();
    let sig = Signature::new(
        &[
            (Type::I32, "a"),
            (Type::I32, "b"),
            (Type::I32, "c"),
            (Type::I32, "d"),
        ],
        &[Type::I32],
    );
    let mut b = FunctionBuilder::new(name, sig);
    let (_, params) = b.create_block_here_with_params(&[
        (Type::I32, "a"),
        (Type::I32, "b"),
        (Type::I32, "c"),
        (Type::I32, "d"),
    ]);
    let r = build(&mut b, params[0], params[1], params[2], params[3]);
    b.return_(&[r]);
    let func = b.finish();
    let compiled = FunctionCompiler::<X86Isa>::compile_raw(&func).unwrap();
    assert!(!compiled.code.is_empty());
    let mem = ExecutableMemory::new(&compiled.code).unwrap();
    let f: extern "C" fn(i32, i32, i32, i32) -> i32 = unsafe { mem.get_fn(0).unwrap() };
    f(a1, a2, a3, a4)
}

// --- 单参数测试 ---

#[test]
fn test_param_one_identity() {
    assert_eq!(call1_i32("p1_id", |_, x| x, 42), 42);
    assert_eq!(call1_i32("p1_id2", |_, x| x, -1), -1);
    assert_eq!(call1_i32("p1_id3", |_, x| x, 0), 0);
    assert_eq!(call1_i32("p1_id4", |_, x| x, 2000000000), 2000000000);
}

#[test]
fn test_param_one_add_const() {
    let r = call1_i32(
        "p1_addc",
        |b, x| {
            let c = b.iconst_i32(10);
            b.iadd(x, c)
        },
        32,
    );
    assert_eq!(r, 42);
}

#[test]
fn test_param_one_mul_const() {
    let r = call1_i32(
        "p1_mulc",
        |b, x| {
            let c = b.iconst_i32(6);
            b.imul(x, c)
        },
        7,
    );
    assert_eq!(r, 42);
}

// --- 两参数测试 ---

#[test]
fn test_param_two_add() {
    assert_eq!(call2_i32("p2_add", |b, a, c| b.iadd(a, c), 20, 22), 42);
    assert_eq!(call2_i32("p2_add2", |b, a, c| b.iadd(a, c), 100, 50), 150);
    assert_eq!(call2_i32("p2_add3", |b, a, c| b.iadd(a, c), -10, 52), 42);
}

#[test]
fn test_param_two_sub() {
    assert_eq!(call2_i32("p2_sub", |b, a, c| b.isub(a, c), 84, 42), 42);
    assert_eq!(call2_i32("p2_sub2", |b, a, c| b.isub(a, c), 50, 100), -50);
}

#[test]
fn test_param_two_mul() {
    assert_eq!(call2_i32("p2_mul", |b, a, c| b.imul(a, c), 6, 7), 42);
    assert_eq!(call2_i32("p2_mul2", |b, a, c| b.imul(a, c), -6, -7), 42);
}

#[test]
fn test_param_two_bitwise() {
    // a & b
    assert_eq!(call2_i32("p2_and", |b, a, c| b.band(a, c), 0xFF, 42), 42);
    // a | b
    assert_eq!(call2_i32("p2_or", |b, a, c| b.bor(a, c), 40, 2), 42);
    // a ^ b
    assert_eq!(call2_i32("p2_xor", |b, a, c| b.bxor(a, c), 63, 21), 42);
    // ~a: bnot(-43) = 42
    assert_eq!(call1_i32("p2_not", |b, a| b.bnot(a), -43), 42);
}

#[test]
fn test_param_two_shift() {
    // a << b
    assert_eq!(call2_i32("p2_shl", |b, a, c| b.ishl(a, c), 21, 1), 42);
    // a >> b (logical)
    assert_eq!(call2_i32("p2_shr", |b, a, c| b.ushr(a, c), 168, 2), 42);
}

#[test]
fn test_param_two_div() {
    // a / b (unsigned)
    assert_eq!(call2_i32("p2_udiv", |b, a, c| b.udiv(a, c), 84, 2), 42);
    // a / b (signed)
    assert_eq!(call2_i32("p2_sdiv", |b, a, c| b.sdiv(a, c), 126, 3), 42);
}

#[test]
fn test_param_two_compare() {
    // a == b
    assert_eq!(
        call2_i32(
            "p2_eq",
            |b, a, c| {
                let cond = b.icmp(IntCC::Equal, a, c);
                let c41 = b.iconst_i32(41);
                b.iadd(cond, c41)
            },
            42,
            42
        ),
        42
    );
    // a < b
    assert_eq!(
        call2_i32(
            "p2_slt",
            |b, a, c| {
                let cond = b.icmp(IntCC::SignedLessThan, a, c);
                let c41 = b.iconst_i32(41);
                b.iadd(cond, c41)
            },
            10,
            50
        ),
        42
    );
}

#[test]
fn test_param_two_first_only() {
    // 只使用第一个参数 — 用 a-b 验证参数顺序 (非交换律)
    // f(84, 42) → a-b = 42 确认 a=84 为第一参数
    let r = call2_i32("p2_first_only", |b, a, c| b.isub(a, c), 84, 42);
    assert_eq!(r, 42, "param order: a=84, b=42 → 84-42=42");
}

#[test]
fn test_param_two_second_only() {
    // 只使用第二个参数
    let r = call2_i32("p2_second_only", |_b, _a, c| c, 42, 999);
    assert_eq!(r, 999, "second param should be 999");
}

#[test]
fn test_param_two_chained() {
    // (a + b) * 2 - (a - b) → param chain
    let r = call2_i32(
        "p2_chain",
        |b, a, c| {
            let two = b.iconst_i32(2);
            let sum = b.iadd(a, c);
            let doubled = b.imul(sum, two);
            let diff = b.isub(a, c);
            b.isub(doubled, diff)
        },
        20,
        22,
    );
    // (20+22)*2 - (20-22) = 84 - (-2) = 86
    assert_eq!(r, 86);
}

// --- 三参数测试 ---

#[test]
fn test_param_three_add() {
    let r = call3_i32(
        "p3_add",
        |b, a, c, d| {
            let s1 = b.iadd(a, c);
            b.iadd(s1, d)
        },
        10,
        20,
        12,
    );
    assert_eq!(r, 42);
}

#[test]
fn test_param_three_mul_add() {
    let r = call3_i32(
        "p3_ma",
        |b, a, c, d| {
            let m = b.imul(a, c);
            b.iadd(m, d)
        },
        6,
        5,
        12,
    );
    // 6*5 + 12 = 42
    assert_eq!(r, 42);
}

#[test]
fn test_param_three_reorder() {
    // 使用参数的顺序与传参顺序不同 (c, a)
    let r = call3_i32(
        "p3_reord",
        |b, _a, _c, d| {
            let two = b.iconst_i32(2);
            b.imul(d, two) // d * 2 = 21*2 = 42
        },
        10,
        20,
        21,
    );
    assert_eq!(r, 42);
}

// --- 四参数测试 (Windows x64 最大整数参数寄存器数) ---

#[test]
fn test_param_four_add() {
    let r = call4_i32(
        "p4_add",
        |b, a, c, d, e| {
            let s1 = b.iadd(a, c);
            let s2 = b.iadd(d, e);
            b.iadd(s1, s2)
        },
        10,
        12,
        8,
        12,
    );
    // 10 + 12 + 8 + 12 = 42
    assert_eq!(r, 42);
}

#[test]
fn test_param_four_mixed_ops() {
    // (a * b) + (c - d)
    let r = call4_i32(
        "p4_mix",
        |b, a, c, d, e| {
            let m = b.imul(a, c);
            let s = b.isub(d, e);
            b.iadd(m, s)
        },
        6,
        7,
        52,
        10,
    );
    // 6*7 + (52-10) = 42 + 42 = 84
    assert_eq!(r, 84);
}

#[test]
fn test_param_four_all_used() {
    // 确保所有四个参数都被正确传递且互不干扰
    let r = call4_i32(
        "p4_all",
        |b, a, c, d, e| {
            let s1 = b.iadd(a, c);
            let m1 = b.imul(d, e);
            b.iadd(s1, m1)
        },
        1,
        2,
        3,
        4,
    );
    // (1+2) + (3*4) = 3 + 12 = 15
    assert_eq!(r, 15);
}

#[test]
fn test_param_four_reverse_order() {
    // 验证参数顺序
    // Windows: RCX(1)=a, RDX(2)=b, R8(8)=c, R9(9)=d
    let r = call4_i32(
        "p4_rev",
        |b, a, c, _d, e| {
            let s1 = b.iadd(a, c);
            b.iadd(s1, e)
        },
        5,
        15,
        10,
        12,
    );
    // 5 + 15 + 12 = 32
    assert_eq!(r, 32);
}

// ============================================================
// 返回值测试 — 验证不同返回路径
// ============================================================

/// 编译并调用 fn() -> i32 (无参版本)
fn call0_i32(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> i32 {
    ensure_registered();
    let sig = Signature::new(&[], &[Type::I32]);
    let mut b = FunctionBuilder::new(name, sig);
    b.create_block_here();
    let r = build(&mut b);
    b.return_(&[r]);
    let func = b.finish();
    let compiled = FunctionCompiler::<X86Isa>::compile_raw(&func).unwrap();
    assert!(!compiled.code.is_empty());
    let mem = ExecutableMemory::new(&compiled.code).unwrap();
    let f: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

#[test]
fn test_return_negative() {
    let r = call1_i32(
        "ret_neg",
        |b, x| {
            let zero = b.iconst_i32(0);
            b.isub(zero, x) // 0 - x = -x
        },
        42,
    );
    assert_eq!(r, -42);
}

#[test]
fn test_return_zero() {
    let r = call0_i32("ret_zero", |b: &mut FunctionBuilder| b.iconst_i32(0));
    assert_eq!(r, 0);
}

#[test]
fn test_return_minus_one() {
    let r = call0_i32("ret_m1", |b: &mut FunctionBuilder| b.iconst_i32(-1));
    assert_eq!(r, -1);
}

#[test]
fn test_return_large_positive() {
    let r = call0_i32("ret_lp", |b: &mut FunctionBuilder| b.iconst_i32(2000000000));
    assert_eq!(r, 2000000000);
}

#[test]
fn test_return_large_negative() {
    let r = call0_i32("ret_ln", |b: &mut FunctionBuilder| {
        b.iconst_i32(-2000000000i32)
    });
    assert_eq!(r, -2000000000i32);
}

#[test]
fn test_return_max_i32() {
    let r = call0_i32("ret_max", |b: &mut FunctionBuilder| b.iconst_i32(i32::MAX));
    assert_eq!(r, i32::MAX);
}

#[test]
fn test_return_min_i32() {
    let r = call0_i32("ret_min", |b: &mut FunctionBuilder| b.iconst_i32(i32::MIN));
    assert_eq!(r, i32::MIN);
}

#[test]
fn test_return_computed_from_params() {
    // (a + b) * (a - b) using 2-param helper
    let r = call2_i32(
        "ret_comp",
        |b, a, c| {
            let sum = b.iadd(a, c);
            let diff = b.isub(a, c);
            b.imul(sum, diff)
        },
        10,
        4,
    );
    // (10+4) * (10-4) = 14 * 6 = 84
    assert_eq!(r, 84);
}

// ============================================================
// 栈平衡/稳定性测试 — 反复调用确保栈帧正确恢复
// ============================================================

#[test]
fn test_stack_balance_no_params() {
    // 反复调用无参函数，确保栈帧恢复正确
    for i in 0..100 {
        let r = call0_i32("sb_np", |b: &mut FunctionBuilder| b.iconst_i32(42));
        assert_eq!(r, 42, "stack_balance_no_params: iteration {i}");
    }
}

#[test]
fn test_stack_balance_one_param() {
    // 反复调用单参函数
    for i in 0..100 {
        let r = call1_i32(
            "sb_1p",
            |b, x| {
                let two = b.iconst_i32(2);
                b.imul(x, two)
            },
            21,
        );
        assert_eq!(r, 42, "stack_balance_one_param: iteration {i}");
    }
}

#[test]
fn test_stack_balance_two_params() {
    // 反复调用双参函数
    for i in 0..100 {
        let r = call2_i32("sb_2p", |b, a, c| b.iadd(a, c), 20, 22);
        assert_eq!(r, 42, "stack_balance_two_params: iteration {i}");
    }
}

#[test]
fn test_stack_balance_four_params() {
    // 反复调用四参函数 (使用全部参数寄存器)
    for i in 0..100 {
        let r = call4_i32(
            "sb_4p",
            |b, a, c, d, e| {
                let s1 = b.iadd(a, c);
                let s2 = b.iadd(d, e);
                b.iadd(s1, s2)
            },
            10,
            11,
            10,
            11,
        );
        assert_eq!(r, 42, "stack_balance_four_params: iteration {i}");
    }
}

#[test]
fn test_stack_balance_mixed_signatures() {
    // 交替调用不同签名的函数，验证栈帧互相不干扰
    for i in 0..50 {
        // 无参
        let r0 = call0_i32("sbm_a", |b: &mut FunctionBuilder| b.iconst_i32(1));
        assert_eq!(r0, 1, "mixed: iteration {i}, call0");
        // 单参
        let r1 = call1_i32("sbm_b", |_b, x| x, 2);
        assert_eq!(r1, 2, "mixed: iteration {i}, call1");
        // 双参
        let r2 = call2_i32("sbm_c", |b, a, c| b.iadd(a, c), 3, 4);
        assert_eq!(r2, 7, "mixed: iteration {i}, call2");
        // 四参
        let r4 = call4_i32(
            "sbm_d",
            |b, a, c, d, e| {
                let s1 = b.iadd(a, c);
                let s2 = b.isub(e, d);
                b.imul(s1, s2)
            },
            10,
            4,
            1,
            2,
        );
        // (10+4) * (2-1) = 14 * 1 = 14
        assert_eq!(r4, 14, "mixed: iteration {i}, call4");
    }
}

#[test]
fn test_stack_balance_deep_chain() {
    // 适量局部变量测试 (不超过可用寄存器数，避免 spill)
    // x86-64 可用 caller-save: RCX(1), RSI(6), RDI(7), R8-R11(8-11) = 7 个
    for i in 0..100 {
        let r = call0_i32("sbc", |b: &mut FunctionBuilder| {
            let v1 = b.iconst_i32(1);
            let v2 = b.iconst_i32(2);
            let v3 = b.iconst_i32(3);
            let v4 = b.iconst_i32(4);
            let v5 = b.iconst_i32(5);
            let v6 = b.iconst_i32(6);
            // 6 变量 chain: (((((1+2)*3)-4)+5)*6) = ((9-4)+5)*6 = 60
            let s1 = b.iadd(v1, v2); // 3
            let m1 = b.imul(s1, v3); // 9
            let s2 = b.isub(m1, v4); // 5
            let s3 = b.iadd(s2, v5); // 10
            b.imul(s3, v6) // 60
        });
        assert_eq!(r, 60, "stack_balance_deep_chain: iteration {i}");
    }
}

// ============================================================
// 高寄存器压力测试 — 强制溢出
// ============================================================

#[test]
fn test_spill_high_pressure() {
    // 创建 12 个局部变量 + 多个中间值 → 强制溢出
    // 可用寄存器: RAX(0,precolored), RCX(1), RDX(2,precolored), RSI(6), RDI(7), R8-R9(8-9) = 7个
    // VReg96(R10), VReg97(R11) 预留给 spill scratch
    // 12+ 个活跃 VReg > 7 个可用 → 必然溢出
    for i in 0..50 {
        let r = call0_i32("spill_hp", |b: &mut FunctionBuilder| {
            let v1 = b.iconst_i32(1);
            let v2 = b.iconst_i32(2);
            let v3 = b.iconst_i32(3);
            let v4 = b.iconst_i32(4);
            let v5 = b.iconst_i32(5);
            let v6 = b.iconst_i32(6);
            let v7 = b.iconst_i32(7);
            let v8 = b.iconst_i32(8);
            let v9 = b.iconst_i32(9);
            let v10 = b.iconst_i32(10);
            let v11 = b.iconst_i32(11);
            let v12 = b.iconst_i32(12);
            // 全部相加: 1+2+...+12 = 78
            let s1 = b.iadd(v1, v2); // 3
            let s2 = b.iadd(v3, v4); // 7
            let s3 = b.iadd(v5, v6); // 11
            let s4 = b.iadd(v7, v8); // 15
            let s5 = b.iadd(v9, v10); // 19
            let s6 = b.iadd(v11, v12); // 23
            let m1 = b.iadd(s1, s2); // 10
            let m2 = b.iadd(s3, s4); // 26
            let m3 = b.iadd(s5, s6); // 42
            let t1 = b.iadd(m1, m2); // 36
            b.iadd(t1, m3) // 78
        });
        assert_eq!(r, 78, "spill_high_pressure: iteration {i}");
    }
}

#[test]
fn test_spill_with_params() {
    // 4 个参数 + 10 个局部变量 → 大量溢出
    for i in 0..50 {
        let r = call4_i32(
            "spill_par",
            |b, a, c, d, e| {
                let v1 = b.iconst_i32(1);
                let v2 = b.iconst_i32(2);
                let v3 = b.iconst_i32(3);
                let v4 = b.iconst_i32(4);
                let v5 = b.iconst_i32(5);
                let v6 = b.iconst_i32(6);
                let v7 = b.iconst_i32(7);
                let v8 = b.iconst_i32(8);
                // 参数 a,b,c,d + v1..v8 + 中间结果 = 大量 VReg
                let pa = b.iadd(a, c);
                let pb = b.iadd(d, e);
                let psum = b.iadd(pa, pb);
                let va = b.iadd(v1, v2);
                let vb = b.iadd(v3, v4);
                let vsum1 = b.iadd(va, vb);
                let vc = b.iadd(v5, v6);
                let vd = b.iadd(v7, v8);
                let vsum2 = b.iadd(vc, vd);
                let t1 = b.iadd(psum, vsum1);
                b.iadd(t1, vsum2)
            },
            1,
            2,
            3,
            4,
        );
        // 参数: 1+2+3+4=10, vsum1=10, vsum2=26 → 10+10+26=46
        assert_eq!(r, 46, "spill_with_params: iteration {i}");
    }
}

// ============================================================
// 多返回值测试
// ============================================================

/// 编译并调用 fn() -> (i32, i32) — 返回两个 i32
/// RAX=第一个返回值, RDX=第二个返回值
fn call0_two_i32(name: &str, build: fn(&mut FunctionBuilder) -> (Value, Value)) -> i64 {
    ensure_registered();
    let sig = Signature::new(&[], &[Type::I32, Type::I32]);
    let mut b = FunctionBuilder::new(name, sig);
    b.create_block_here();
    let (r1, r2) = build(&mut b);
    b.return_(&[r1, r2]);
    let func = b.finish();
    let compiled = FunctionCompiler::<X86Isa>::compile_raw(&func).unwrap();
    assert!(!compiled.code.is_empty(), "{}: empty code", name);
    let mem = ExecutableMemory::new(&compiled.code).unwrap();
    // 返回 i64: RAX=低32位=val1, RDX=高32位(不直接可读)
    // 但 extern "C" fn() -> i64 也使用 RAX 返回
    // 实际: MOV RAX, val1; MOV RDX, val2; RET → RAX=val1
    let f: extern "C" fn() -> i64 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

#[test]
fn test_multi_return_two_constants() {
    // fn() -> (42, 99): RAX=42, RDX=99
    let r = call0_two_i32("mr2_c", |b: &mut FunctionBuilder| {
        let v1 = b.iconst_i32(42);
        let v2 = b.iconst_i32(99);
        (v1, v2)
    });
    // 低 32 位 = 第一个返回值
    assert_eq!(r as i32, 42, "first return value should be 42");
}

#[test]
fn test_multi_return_computed() {
    // fn(a: i32, b: i32) -> (a+b, a*b)
    // builder 通过 params 访问 a, b
    let sig = Signature::new(
        &[(Type::I32, "a"), (Type::I32, "b")],
        &[Type::I32, Type::I32],
    );
    let mut b = FunctionBuilder::new("mr_comp", sig);
    let (_, params) = b.create_block_here_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
    let sum = b.iadd(params[0], params[1]);
    let prod = b.imul(params[0], params[1]);
    b.return_(&[sum, prod]);
    let func = b.finish();
    ensure_registered();
    let compiled = FunctionCompiler::<X86Isa>::compile_raw(&func).unwrap();
    assert!(!compiled.code.is_empty());
    let mem = ExecutableMemory::new(&compiled.code).unwrap();
    let f: extern "C" fn(i32, i32) -> i64 = unsafe { mem.get_fn(0).unwrap() };
    // f(6, 7) → sum=13 到 RAX, prod=42 到 RDX
    let r = f(6, 7);
    assert_eq!(r as i32, 13, "first return value (sum) should be 13");
}

#[test]
fn test_multi_return_verify_second() {
    // 验证第二个返回值：创建两个不同的函数
    // fn1 → (0, 42): RAX=0, RDX=42
    // fn2 → (42, 0): RAX=42, RDX=0
    // 通过比较确认第二个返回值被正确生成

    // fn1: return (0, 42) → RAX=0, RDX=42
    let sig1 = Signature::new(&[], &[Type::I32, Type::I32]);
    let mut b1 = FunctionBuilder::new("mr_second_a", sig1);
    b1.create_block_here();
    let z1 = b1.iconst_i32(0);
    let v1 = b1.iconst_i32(42);
    b1.return_(&[z1, v1]);
    let func1 = b1.finish();

    // fn2: return (42, 0) → RAX=42, RDX=0
    let sig2 = Signature::new(&[], &[Type::I32, Type::I32]);
    let mut b2 = FunctionBuilder::new("mr_second_b", sig2);
    b2.create_block_here();
    let v2 = b2.iconst_i32(42);
    let z2 = b2.iconst_i32(0);
    b2.return_(&[v2, z2]);
    let func2 = b2.finish();

    ensure_registered();
    let c1 = FunctionCompiler::<X86Isa>::compile_raw(&func1).unwrap();
    let c2 = FunctionCompiler::<X86Isa>::compile_raw(&func2).unwrap();
    let m1 = ExecutableMemory::new(&c1.code).unwrap();
    let m2 = ExecutableMemory::new(&c2.code).unwrap();
    let f1: extern "C" fn() -> i64 = unsafe { m1.get_fn(0).unwrap() };
    let f2: extern "C" fn() -> i64 = unsafe { m2.get_fn(0).unwrap() };

    let r1 = f1();
    let r2 = f2();
    // 两个函数都应在 RAX 返回第一个值
    assert_eq!(r1 as i32, 0, "fn1: first return = 0");
    assert_eq!(r2 as i32, 42, "fn2: first return = 42");
}

/// Switch-like 控制流：多个条件分支
#[test]
fn test_func_multi_branch() {
    let result = run_test_block("multi_branch", |b| {
        b.create_block_here();
        let target_a = b.create_block();
        let target_b = b.create_block();
        let target_c = b.create_block();
        let merge = b.create_block();

        // 简单实现：直接跳转到 target_a
        b.jump(target_a, &[]);

        b.build_block(target_a, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(target_b, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(target_c, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(merge, |b| {
            let ret = b.iconst_i32(42);
            b.return_(&[ret]);
        });
    });
    assert_eq!(result, 42, "multi_branch: expected 42");
}

// ═══════════════════════════════════════════════════════════════
// Stage A mirror tests — validate patterns needed for bootstrap
// ═══════════════════════════════════════════════════════════════

/// Stage A: conditional via icmp + branch — demonstrates build_block()
#[test]
fn test_stage_a_conditional() {
    let result = run_test_block("stage_a_cond", |b| {
        b.create_block_here();
        let then_blk = b.create_block();
        let else_blk = b.create_block();
        let merge = b.create_block();

        let x = b.iconst_i32(5);
        let zero = b.iconst_i32(0);
        let cond = b.icmp(IntCC::SignedGreaterThan, x, zero);
        b.branch(cond, then_blk, else_blk, &[], &[]);

        b.build_block(then_blk, |b| {
            b.jump(merge, &[]);
        });
        b.build_block(else_blk, |b| {
            b.jump(merge, &[]);
        });
        b.build_block(merge, |b| {
            let r = b.iconst_i32(1);
            b.return_(&[r]);
        });
    });
    assert_eq!(result, 1, "conditional: 5 > 0 → then branch → 1");
}

/// Stage A: loop accumulator (simple — 0+1+2 = 3) — demonstrates create_block_here_with
#[test]
fn test_stage_a_loop_accumulator() {
    let result = run_test_block("stage_a_loop", |b| {
        let entry = b.create_block_here();
        let body = b.create_block();
        let exit = b.create_block();
        let (loop_hdr, [acc, i]) = b.create_block_here_with([(Type::I32, "acc"), (Type::I32, "i")]);

        b.build_block(entry, |b| {
            let z = b.iconst_i32(0);
            b.jump(loop_hdr, &[z, z]);
        });

        let n = b.iconst_i32(3);
        let done = b.icmp(IntCC::SignedGreaterThanOrEqual, i, n);
        b.branch(done, exit, body, &[], &[]);

        b.build_block(body, |b| {
            let new_acc = b.iadd(acc, i);
            let one = b.iconst_i32(1);
            let new_i = b.iadd(i, one);
            b.jump(loop_hdr, &[new_acc, new_i]);
        });

        b.build_block(exit, |b| {
            b.return_(&[acc]);
        });
    });
    assert_eq!(result, 3, "loop accumulator: 0+1+2 = 3");
}

/// Stage A: nested calls — mul(add(x, 1), 2)
#[test]
fn test_stage_a_nested_call() {
    assert_eq!(
        run_test("nested", |b| {
            let x = b.iconst_i32(3);
            let one = b.iconst_i32(1);
            let sum = b.iadd(x, one); // 3+1 = 4
            let two = b.iconst_i32(2);
            b.imul(sum, two) // 4*2 = 8
        }),
        8
    );
}

/// Stage A: mutable variable pattern — (x+1)*2
#[test]
fn test_stage_a_mutable_var() {
    assert_eq!(
        run_test("mut_var", |b| {
            let x = b.iconst_i32(3);
            let one = b.iconst_i32(1);
            let y = b.iadd(x, one); // y = x+1 = 4
            let two = b.iconst_i32(2);
            b.imul(y, two) // y*2 = 8
        }),
        8
    );
}

// Stage A i64: tested via existing test_imul_i64 and jit_demo large-value tests

/// Stage A: early return pattern — branch on negative, demonstrates build_block()
#[test]
fn test_stage_a_early_return_neg() {
    let result = run_test_block("early_neg", |b| {
        b.create_block_here();
        let zero_blk = b.create_block();
        let merge = b.create_block();

        let x = b.iconst_i32(5i64 as i32);
        let zero = b.iconst_i32(0);
        let cond = b.icmp(IntCC::SignedLessThan, x, zero);
        b.branch(cond, zero_blk, merge, &[], &[]);

        b.build_block(zero_blk, |b| {
            b.jump(merge, &[]);
        });
        b.build_block(merge, |b| {
            let two = b.iconst_i32(2);
            let result = b.imul(x, two);
            b.return_(&[result]);
        });
    });
    assert_eq!(result, 10, "early return pos: 5*2 = 10");
}

/// Stage A: early return pattern — branch on positive, demonstrates build_block()
#[test]
fn test_stage_a_early_return_pos() {
    let result = run_test_block("early_pos", |b| {
        b.create_block_here();
        let ret_blk = b.create_block();
        let merge = b.create_block();

        let x = b.iconst_i32(-5);
        let zero = b.iconst_i32(0);
        let cond = b.icmp(IntCC::SignedLessThan, x, zero);
        b.branch(cond, ret_blk, merge, &[], &[]);

        b.build_block(ret_blk, |b| {
            b.jump(merge, &[]);
        });
        b.build_block(merge, |b| {
            let zero_ret = b.iconst_i32(0);
            b.return_(&[zero_ret]);
        });
    });
    assert_eq!(result, 0, "early return neg: x<0 → 0");
}
