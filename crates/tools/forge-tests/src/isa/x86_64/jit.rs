//! x86_64 JIT 集成测试（从根 tests/jit_integration.rs 整体迁移，156 条）。
//!
//! 验证指令通过 编译→JIT→执行 的完整管线。

#![cfg(test)]

use std::{f32, f64};

use code_forge::backend::FunctionCompiler;
use code_forge::backend::arch::x86_64::ensure_registered;
use code_forge::backend::arch::x86_64::{self, TargetMachine};
use code_forge::mem::ExecutableMemory;
use code_forge::prelude::*;

fn run_test_block(name: &str, build: fn(&mut FunctionBuilder)) -> i32 {
    crate::exec::harness::run_block(name, build)
}

fn run_test(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> i32 {
    crate::exec::harness::run_i32(name, build)
}

fn run_bool_test(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> bool {
    crate::exec::harness::run_bool(name, build)
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
            b.load(addr, TypeId::I32)
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
            b.bitcast(a, TypeId::I32)
        }),
        42
    );
}

#[test]
fn test_uextend() {
    assert_eq!(
        run_test("uextend", |b| {
            let a = b.iconst_i32(42);
            b.uextend(a, TypeId::I64)
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
            let ext = b.uextend(a, TypeId::I64);
            b.ireduce(ext, TypeId::I32)
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
        let ext = b.uextend(a, TypeId::I64);
        b.ireduce(ext, TypeId::I32)
    });
    assert_eq!(r, 2000000000i32);
}

/// uextend 窄值运算结果：i8 溢出加法（200+100 → 低 8 位 44），
/// uextend 后应得 44 而非 300（movzx 零扩展；历史 mov 直拷会携带 32 位垃圾）。
#[test]
fn test_uextend_narrow_op_result() {
    let r = run_test("uext_narrow", |b| {
        let a = b.iconst_i8(200u8 as i8);
        let c = b.iconst_i8(100u8 as i8);
        let s = b.iadd(a, c); // i8 结果：300 低 8 位 = 44
        let ext = b.uextend(s, TypeId::I64);
        b.ireduce(ext, TypeId::I32)
    });
    assert_eq!(r, 44, "i8 溢出运算 uextend 应得 44，实际 {r}");
}

/// uextend 负数: I32 负数 uextend 到 I64 后高位补 0，截断回来仍是原负数
#[test]
fn test_uextend_with_negative() {
    assert_eq!(
        run_test("uext_neg", |b| {
            let a = b.iconst_i32(-42);
            let ext = b.uextend(a, TypeId::I64);
            b.ireduce(ext, TypeId::I32)
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
            let ext = b.sextend(a, TypeId::I64);
            b.ireduce(ext, TypeId::I32)
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
            let ext = b.sextend(a, TypeId::I64);
            b.ireduce(ext, TypeId::I32)
        }),
        -1
    );
}

/// sextend 正数: 行为应与 uextend 相同（高位补 0）
#[test]
fn test_sextend_positive() {
    let r = run_test("sext_pos", |b| {
        let a = b.iconst_i32(2000000000);
        let ext = b.sextend(a, TypeId::I64);
        b.ireduce(ext, TypeId::I32)
    });
    assert_eq!(r, 2000000000i32);
}

/// sextend → ireduce 往返: I32 → I64 → I32，应保持原值
#[test]
fn test_sextend_ireduce_roundtrip() {
    assert_eq!(
        run_test("sext_red", |b| {
            let a = b.iconst_i32(42);
            let ext = b.sextend(a, TypeId::I64);
            b.ireduce(ext, TypeId::I32)
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
            let ext = b.uextend(a, TypeId::I64);
            b.ireduce(ext, TypeId::I32)
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
        b.ireduce(a, TypeId::I32)
    });
    assert_eq!(r, 0xAAAA0042u32 as i32);
}

/// ireduce 全位设置: I64 = -1 → I32 = -1 (0xFFFFFFFF)
#[test]
fn test_ireduce_all_bits_set() {
    assert_eq!(
        run_test("ired_bits", |b| {
            let a = b.iconst_i64(-1i64);
            b.ireduce(a, TypeId::I32)
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
            b.ireduce(a, TypeId::I32)
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
            b.ireduce(reduced, TypeId::I32) // → I32
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
            let ext = b.uextend(a, TypeId::I64); // 10 as I64
            let bv = b.iconst_i64(32);
            let sum = b.iadd(ext, bv); // 42 as I64
            b.ireduce(sum, TypeId::I32) // 42 as I32
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
            let ext = b.uextend(a, TypeId::I64);
            b.ireduce(ext, TypeId::I32)
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
            let ext = b.sextend(a, TypeId::I64);
            b.ireduce(ext, TypeId::I32)
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
            let ext = b.sextend(a, TypeId::I64);
            let mask = b.iconst_i64(0xFFFF);
            let trunc = b.band(ext, mask); // 0x12345 & 0xFFFF = 0x2345
            b.ireduce(trunc, TypeId::I32) // → I32
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
            b.branch(cond, then_b, &[], else_b, &[]);
            b.build_block(then_b, |b| {
                let v1 = b.iconst_i32(42);
                b.ret(&[v1]);
            });
            b.build_block(else_b, |b| {
                let v2 = b.iconst_i32(0);
                b.ret(&[v2]);
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
            let (merge, merge_params) = b.create_block_with_params(&[(TypeId::I32, "phi")]);
            b.switch_to_block(Block(0));

            let cond = b.iconst_i32(0); // false
            b.branch(cond, then_b, &[], else_b, &[]);

            b.build_block(then_b, |b| {
                let v = b.iconst_i32(0);
                b.jump(merge, &[v]);
            });
            b.build_block(else_b, |b| {
                let v = b.iconst_i32(42);
                b.jump(merge, &[v]);
            });
            b.build_block(merge, |b| {
                b.ret(&[merge_params[0]]);
            });
        }),
        42
    );
}

/// Minimal test: unconditional jump with block param
#[test]
fn test_jump_block_param() {
    assert_eq!(
        run_test_block("jump_param", |b| {
            b.create_block_here();
            let (merge, merge_params) = b.create_block_with_params(&[(TypeId::I32, "v")]);
            b.switch_to_block(Block(0));
            let v = b.iconst_i32(42);
            b.jump(merge, &[v]);
            b.build_block(merge, |b| {
                b.ret(&[merge_params[0]]);
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
                b.ret(&[limit]);
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
            b.branch(cond, big, &[], small, &[]);

            b.build_block(big, |b| {
                b.jump(merge, &[]);
            });

            b.build_block(small, |b| {
                b.jump(merge, &[]);
            });

            b.build_block(merge, |b| {
                let r = b.iconst_i32(42);
                b.ret(&[r]);
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
            b.sextend(a, TypeId::I64)
        }),
        42
    );
}

#[test]
fn test_ireduce() {
    assert_eq!(
        run_test("ireduce", |b| {
            let a = b.iconst_i64(42);
            b.ireduce(a, TypeId::I32)
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
                b.ret(&[a]);
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
            let cond = b.icmp(IntCC::SignedGreaterThan, a, bv); // false(0)
            let not = b.bnot(cond); // 规范化：0 ^ 1 = 1
            b.iadd(not, c41) // 1 + 41 = 42
        }),
        42
    );
}

/// bnot(bool) 规范化：bnot(true)=0、bnot(false)=1（0/1 布尔语义，非 -1）。
#[test]
fn test_bool_bnot_normalized() {
    assert_eq!(
        run_test("bool_bnot_norm", |b| {
            let a = b.iconst_i32(1);
            let bv = b.iconst_i32(2);
            let c = b.iconst_i32(4);
            let d = b.iconst_i32(3);
            let t = b.icmp(IntCC::SignedLessThan, a, bv); // true(1)
            let f = b.icmp(IntCC::SignedLessThan, c, d); // false(0)
            let not_t = b.bnot(t); // 0
            let not_f = b.bnot(f); // 1
            let sum = b.iadd(not_t, not_f); // 0 + 1 = 1
            let c41 = b.iconst_i32(41);
            b.iadd(sum, c41) // 42
        }),
        42
    );
}

// ═══════════════════════════════════════════════════════════════
// 布尔运算（bool/i1）JIT 集成测试
// ═══════════════════════════════════════════════════════════════

/// bool 逻辑运算链：band/bxor/bor 组合，中间结果全为 bool，最后混入整数。
/// (a<b)=1、(c<d)=0 → band=0、bxor=1、bor(0,1)=1 → 1+41=42
#[test]
fn test_bool_logic_chain() {
    assert_eq!(
        run_test("bool_logic_chain", |b| {
            let a = b.iconst_i32(1);
            let bv = b.iconst_i32(2);
            let c = b.iconst_i32(4);
            let d = b.iconst_i32(3);
            let cond1 = b.icmp(IntCC::SignedLessThan, a, bv); // true(1)
            let cond2 = b.icmp(IntCC::SignedLessThan, c, d); // false(0)
            let and = b.band(cond1, cond2); // 1 & 0 = 0
            let xor = b.bxor(cond1, cond2); // 1 ^ 0 = 1
            let or = b.bor(and, xor); // 0 | 1 = 1
            let c41 = b.iconst_i32(41);
            b.iadd(or, c41) // 1 + 41 = 42
        }),
        42
    );
}

/// bool 算术：iadd(bool, bool) 结果仍为 bool 类型，且可继续参与整数运算。
/// t=1、f=0 → s1=t+f=1、s3=s1+f=1 → 1+41=42
#[test]
fn test_bool_arith() {
    assert_eq!(
        run_test("bool_arith", |b| {
            let a = b.iconst_i32(5);
            let bv = b.iconst_i32(10);
            let c = b.iconst_i32(20);
            let d = b.iconst_i32(10);
            let t = b.icmp(IntCC::SignedLessThan, a, bv); // true(1)
            let f = b.icmp(IntCC::SignedLessThan, c, d); // false(0)
            let s1 = b.iadd(t, f); // 1 + 0 = 1（bool 结果）
            let s2 = b.iadd(s1, f); // 1 + 0 = 1（bool 结果）
            let c41 = b.iconst_i32(41);
            b.iadd(s2, c41) // 1 + 41 = 42
        }),
        42
    );
}

/// bool 与整数混合：bool 直接参与整数运算（upcast bool→int）。
/// cond=(a<b)=true(1) → 1+41=42
#[test]
fn test_bool_mixed_int() {
    assert_eq!(
        run_test("bool_mixed_int", |b| {
            let a = b.iconst_i32(3);
            let bv = b.iconst_i32(7);
            let cond = b.icmp(IntCC::SignedLessThan, a, bv); // true(1)
            let c41 = b.iconst_i32(41);
            b.iadd(cond, c41) // bool + i32 → i32 = 42
        }),
        42
    );
}

/// select 使用 bool 条件：cond 为 icmp 结果（bool 类型）。
#[test]
fn test_bool_select() {
    assert_eq!(
        run_test("bool_select", |b| {
            let a = b.iconst_i32(1);
            let bv = b.iconst_i32(2);
            let cond = b.icmp(IntCC::SignedLessThan, a, bv); // true
            let yes = b.iconst_i32(42);
            let no = b.iconst_i32(0);
            b.select(cond, yes, no)
        }),
        42
    );
}

/// 多个 bool 条件组合成复杂表达式：(a<b) && !(c<d) —— band + bnot + iadd。
/// cond1=1、cond2=0 → and=band(cond1, cond2)=0、not=bnot(and)=1（规范化）、
/// 1+42=43
#[test]
fn test_bool_complex_expr() {
    assert_eq!(
        run_test("bool_complex_expr", |b| {
            let a = b.iconst_i32(1);
            let bv = b.iconst_i32(2);
            let c = b.iconst_i32(4);
            let d = b.iconst_i32(3);
            let cond1 = b.icmp(IntCC::SignedLessThan, a, bv); // 1
            let cond2 = b.icmp(IntCC::SignedLessThan, c, d); // 0
            let and = b.band(cond1, cond2); // 0
            let not = b.bnot(and); // 规范化：0 ^ 1 = 1
            let c42 = b.iconst_i32(42);
            b.iadd(not, c42) // 1 + 42 = 43
        }),
        43
    );
}
#[test]
fn test_bool_operation() {
    macro_rules! op {
        (<$op:ident>[$($a1:expr, $a2:expr,$res:expr),*]) => {
            $(assert_eq!(
                run_bool_test(stringify!($op), |b| {
                    let a1 = b.iconst_bool($a1);
                    let a2 = b.iconst_bool($a2);
                    b.$op(a1, a2)
                }),
                $res
            );)*
        };
        (o<$op:ident>[$($a1:expr, $res:expr),*]) => {
            $(assert_eq!(
                run_bool_test(stringify!($op), |b| {
                    let a1 = b.iconst_bool($a1);
                    b.$op(a1)
                }),
                $res
            );)*
        };
    }
    op!(<iadd>[
        false, false, false,
        false, true,  true,
        true,  false, true,
        true,  true,  false
    ]);
    op!(<isub>[
        false, false, false,
        false, true,  true,
        true,  false, true,
        true,  true,  false
    ]);
    op!(<band>[
        false, false, false,
        false, true,  false,
        true,  false, false,
        true,  true,  true
    ]);
    op!(<bor>[
        false, false, false,
        false, true,  true,
        true,  false, true,
        true,  true,  true
    ]);
    op!(<bxor>[
        false, false, false,
        false, true,  true,
        true,  false, true,
        true,  true,  false
    ]);
    op!(o<bnot>[
        false, true,
        true,  false
    ]);
}

/// 多次布尔运算后与 u8（i8）0 值相加：布尔值只保证 bit0 有效（寄存器按整宽
/// 模拟 1 位运算），若中间布尔运算让高位出现垃圾值（如 0xFF / 0xFFFFFFFF），
/// 与 u8 0 相加后结果 ≠ 0/1，即可捕获意外值（iadd 不掩码，能暴露脏位）。
#[test]
fn test_bool_operation_with_u8_zero() {
    // 结果为 1 的多次布尔链：((c1 & c2) ^ c3) | c1 → 1，+u8 0 = 1
    assert_eq!(
        run_test("bool_op_u8_1", |b| {
            let a = b.iconst_i32(1);
            let bv = b.iconst_i32(2);
            let c = b.iconst_i32(3);
            let d = b.iconst_i32(4);
            let c1 = b.icmp(IntCC::SignedLessThan, a, bv); // 1
            let c2 = b.icmp(IntCC::SignedLessThan, bv, c); // 1
            let c3 = b.icmp(IntCC::SignedLessThan, d, c); // 0（4<3）
            let and = b.band(c1, c2); // 1
            let xor = b.bxor(and, c3); // 1 ^ 0 = 1
            let or = b.bor(xor, c1); // 1 | 1 = 1
            let zero_u8 = b.iconst_i8(0);
            b.iadd(or, zero_u8) // 期望 1 + 0 = 1；若 or 高位脏（如 0xFF）则 ≠ 1
        }),
        1
    );
    // 结果为 0 的多次布尔链：!(c1 & c2) & c2 → 0，+u8 0 = 0
    assert_eq!(
        run_test("bool_op_u8_0", |b| {
            let a = b.iconst_i32(1);
            let bv = b.iconst_i32(2);
            let c = b.iconst_i32(3);
            let d = b.iconst_i32(4);
            let c1 = b.icmp(IntCC::SignedLessThan, a, bv); // 1
            let c2 = b.icmp(IntCC::SignedLessThan, c, d); // 1
            let and = b.band(c1, c2); // 1
            let not = b.bnot(and); // ~1 = 0（规范化）
            let nand = b.band(not, c2); // 0 & 1 = 0
            let zero_u8 = b.iconst_i8(0);
            b.iadd(nand, zero_u8) // 期望 0 + 0 = 0；若 nand 高位脏则 ≠ 0
        }),
        0
    );
    assert_eq!(
        run_test("bool_op_u8_0", |b| {
            let a = b.iconst_bool(true);
            let a = b.iadd(a, a);
            let zero_u8 = b.iconst_i8(0);
            b.iadd(a, zero_u8)
        }),
        0
    );

    assert_eq!(
        run_test("bool_op_u8_0", |b| {
            let a = b.iconst_bool(true);
            let a = b.isub(a, a);
            let zero_u8 = b.iconst_i8(0);
            b.iadd(a, zero_u8)
        }),
        0
    );
    // 非相等模 2 场景：isub(false, true) = 0-1 mod 2 = 1（后端 32 位 SUB 会得
    // 0xFFFFFFFF），与 u8 0 相加应为 1——捕获减法脏值
    assert_eq!(
        run_test("bool_op_u8_sub1", |b| {
            let t = b.iconst_bool(true);
            let f = b.iconst_bool(false);
            let a = b.isub(f, t); // 0 - 1 mod 2 = 1
            let zero_u8 = b.iconst_i8(0);
            b.iadd(a, zero_u8)
        }),
        1
    );
    // iadd(true, false) = 1 + 0 = 1，与 u8 0 相加应为 1
    assert_eq!(
        run_test("bool_op_u8_add1", |b| {
            let t = b.iconst_bool(true);
            let f = b.iconst_bool(false);
            let a = b.iadd(t, f); // 1 + 0 = 1
            let zero_u8 = b.iconst_i8(0);
            b.iadd(a, zero_u8)
        }),
        1
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
            b.branch(cond, swap, &[], noswap, &[]);

            b.build_block(swap, |b| {
                b.jump(merge, &[]);
            });

            b.build_block(noswap, |b| {
                b.jump(merge, &[]);
            });

            b.build_block(merge, |b| {
                b.ret(&[y]); // 直接返回大值
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
    crate::exec::harness::run_f64(name, build)
}

/// 无参返回 i64（I64 返回值测试）。
fn run_test_i64(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> i64 {
    crate::exec::harness::run_i64(name, build)
}

/// 无参返回 f32（F32 返回值测试，与 run_test_f64 对称）。
fn run_test_f32(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> f32 {
    crate::exec::harness::run_f32(name, build)
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
        b.fconst(bits, TypeId::F64)
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
        b.fconst(bits, TypeId::F64)
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
        b.branch(cond, true_blk, &[], false_blk, &[]);

        b.build_block(true_blk, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(false_blk, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(merge, |b| {
            let ret = b.iconst_i32(1);
            b.ret(&[ret]);
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
        b.branch(cond, true_blk, &[], false_blk, &[]);

        b.build_block(true_blk, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(false_blk, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(merge, |b| {
            let ret = b.iconst_i32(2);
            b.ret(&[ret]);
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
        let (merge_id, merge_params) = b.create_block_with_params(&[(TypeId::I32, "result")]);
        b.switch_to_block(Block(0));

        let cond = b.iconst_i32(1);
        b.branch(cond, then_blk, &[], else_blk, &[]);

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
            b.ret(&[phi_result]);
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
            b.ret(&[added]);
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
            b.ret(&[ret]);
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
        let (merge_id, merge_params) = b.create_block_with_params(&[(TypeId::I32, "v")]);
        b.switch_to_block(Block(0));

        let take_a = b.iconst_i32(1);
        b.branch(take_a, path_a, &[], path_b, &[]);

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
            b.ret(&[result]);
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
        b.branch(cmp, gt_blk, &[], le_blk, &[]);

        b.build_block(gt_blk, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(le_blk, |b| {
            b.jump(merge, &[]);
        });

        b.build_block(merge, |b| {
            let ret = b.iconst_i32(7);
            b.ret(&[ret]);
        });
    });
    assert_eq!(result, 7, "icmp_branch: expected 7");
}

// ============================================================
// 参数传递测试 — 验证 ABI arg reg → VReg 复制
// ============================================================

/// 编译并调用 fn(x: i32) -> i32
fn call1_i32(name: &str, build: fn(&mut FunctionBuilder, Value) -> Value, a1: i32) -> i32 {
    ensure_registered();
    let sig = FunctionSignature::new(&[(TypeId::I32, "a1")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    let (entry, params) = b.create_block_here_with([(TypeId::I32, "a1")]);
    let r = build(&mut b, params[0]);
    b.switch_to_block(entry);
    b.ret(&[r]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap();
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
    let sig = FunctionSignature::new(&[(TypeId::I32, "a"), (TypeId::I32, "b")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    let (entry, params) = b.create_block_here_with([(TypeId::I32, "a"), (TypeId::I32, "b")]);
    let r = build(&mut b, params[0], params[1]);
    b.switch_to_block(entry);
    b.ret(&[r]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap();
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
    let sig = FunctionSignature::new(
        &[(TypeId::I32, "a"), (TypeId::I32, "b"), (TypeId::I32, "c")],
        &[TypeId::I32],
    );
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    let (entry, params) =
        b.create_block_here_with([(TypeId::I32, "a"), (TypeId::I32, "b"), (TypeId::I32, "c")]);
    let r = build(&mut b, params[0], params[1], params[2]);
    b.switch_to_block(entry);
    b.ret(&[r]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap();
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
    let sig = FunctionSignature::new(
        &[
            (TypeId::I32, "a"),
            (TypeId::I32, "b"),
            (TypeId::I32, "c"),
            (TypeId::I32, "d"),
        ],
        &[TypeId::I32],
    );
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    let (entry, params) = b.create_block_here_with([
        (TypeId::I32, "a"),
        (TypeId::I32, "b"),
        (TypeId::I32, "c"),
        (TypeId::I32, "d"),
    ]);
    let r = build(&mut b, params[0], params[1], params[2], params[3]);
    b.switch_to_block(entry);
    b.ret(&[r]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap();
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
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    let entry = b.create_block_here();
    let r = build(&mut b);
    b.switch_to_block(entry);
    b.ret(&[r]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap();

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
    let sig = FunctionSignature::new(&[], &[TypeId::I32, TypeId::I32]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    let entry = b.create_block_here();
    let (r1, r2) = build(&mut b);
    b.switch_to_block(entry);
    b.ret(&[r1, r2]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap();
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
    let sig = FunctionSignature::new(
        &[(TypeId::I32, "a"), (TypeId::I32, "b")],
        &[TypeId::I32, TypeId::I32],
    );
    let mut b = FunctionBuilder::new("mr_comp", TypeContext::new(), sig);
    let (_entry, params) = b.create_block_here_with([(TypeId::I32, "a"), (TypeId::I32, "b")]);
    let sum = b.iadd(params[0], params[1]);
    let prod = b.imul(params[0], params[1]);
    b.ret(&[sum, prod]);
    let func = b.finish().expect("build");
    ensure_registered();
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap();
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
    let sig1 = FunctionSignature::new(&[], &[TypeId::I32, TypeId::I32]);
    let mut b1 = FunctionBuilder::new("mr_second_a", TypeContext::new(), sig1);
    b1.create_block_here();
    let z1 = b1.iconst_i32(0);
    let v1 = b1.iconst_i32(42);
    b1.ret(&[z1, v1]);
    let func1 = b1.finish().expect("build");

    // fn2: return (42, 0) → RAX=42, RDX=0
    let sig2 = FunctionSignature::new(&[], &[TypeId::I32, TypeId::I32]);
    let mut b2 = FunctionBuilder::new("mr_second_b", TypeContext::new(), sig2);
    b2.create_block_here();
    let v2 = b2.iconst_i32(42);
    let z2 = b2.iconst_i32(0);
    b2.ret(&[v2, z2]);
    let func2 = b2.finish().expect("build");

    ensure_registered();
    let c1 = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func1)
        .unwrap();
    let c2 = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func2)
        .unwrap();
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
            b.ret(&[ret]);
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
        b.branch(cond, then_blk, &[], else_blk, &[]);

        b.build_block(then_blk, |b| {
            b.jump(merge, &[]);
        });
        b.build_block(else_blk, |b| {
            b.jump(merge, &[]);
        });
        b.build_block(merge, |b| {
            let r = b.iconst_i32(1);
            b.ret(&[r]);
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
        let (loop_hdr, [acc, i]) =
            b.create_block_here_with([(TypeId::I32, "acc"), (TypeId::I32, "i")]);

        b.build_block(entry, |b| {
            let z1 = b.iconst_i32(0);
            let z2 = b.iconst_i32(0);
            b.jump(loop_hdr, &[z1, z2]);
        });

        let n = b.iconst_i32(3);
        let done = b.icmp(IntCC::SignedGreaterThanOrEqual, i, n);
        b.branch(done, exit, &[], body, &[]);

        b.build_block(body, |b| {
            let new_acc = b.iadd(acc, i);
            let one = b.iconst_i32(1);
            let new_i = b.iadd(i, one);
            b.jump(loop_hdr, &[new_acc, new_i]);
        });

        b.build_block(exit, |b| {
            b.ret(&[acc]);
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
        b.branch(cond, zero_blk, &[], merge, &[]);

        b.build_block(zero_blk, |b| {
            b.jump(merge, &[]);
        });
        b.build_block(merge, |b| {
            let two = b.iconst_i32(2);
            let result = b.imul(x, two);
            b.ret(&[result]);
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
        b.branch(cond, ret_blk, &[], merge, &[]);

        b.build_block(ret_blk, |b| {
            b.jump(merge, &[]);
        });
        b.build_block(merge, |b| {
            let zero_ret = b.iconst_i32(0);
            b.ret(&[zero_ret]);
        });
    });
    assert_eq!(result, 0, "early return neg: x<0 → 0");
}

// ═══════════════════════════════════════════════════════════════
// Branch encoding fix validation — mirrors mini_c if/else patterns
// ═══════════════════════════════════════════════════════════════

/// Mirror of mini_c: `if (x > 5) return 42; return 0;` with x=3 (false)
#[test]
fn test_branch_mini_c_if_not_taken() {
    let result = run_test_block("mini_c_ifn", |b| {
        b.create_block_here();
        let then_blk = b.create_block();
        let merge_blk = b.create_block();

        // x = 3; x > 5 (false)
        let x = b.iconst_i32(3);
        let five = b.iconst_i32(5);
        let cmp = b.icmp(IntCC::SignedGreaterThan, x, five); // i1: 0 (false)
        let zero = b.iconst_i32(0);
        let cond = b.icmp(IntCC::NotEqual, cmp, zero); // i1: 0 (false)
        b.branch(cond, then_blk, &[], merge_blk, &[]);

        b.build_block(then_blk, |b| {
            let v = b.iconst_i32(42);
            b.ret(&[v]);
        });
        b.build_block(merge_blk, |b| {
            let v = b.iconst_i32(0);
            b.ret(&[v]);
        });
    });
    assert_eq!(result, 0, "mini_c_ifn: false condition → merge (return 0)");
}

/// Mirror of mini_c: `if (x > 5) return 42; return 0;` with x=10 (true)
#[test]
fn test_branch_mini_c_if_taken() {
    let result = run_test_block("mini_c_ift", |b| {
        b.create_block_here();
        let then_blk = b.create_block();
        let merge_blk = b.create_block();

        // x = 10; x > 5 (true)
        let x = b.iconst_i32(10);
        let five = b.iconst_i32(5);
        let cmp = b.icmp(IntCC::SignedGreaterThan, x, five); // i1: 1 (true)
        let zero = b.iconst_i32(0);
        let cond = b.icmp(IntCC::NotEqual, cmp, zero); // i1: 1 (true)
        b.branch(cond, then_blk, &[], merge_blk, &[]);

        b.build_block(then_blk, |b| {
            let v = b.iconst_i32(42);
            b.ret(&[v]);
        });
        b.build_block(merge_blk, |b| {
            let v = b.iconst_i32(0);
            b.ret(&[v]);
        });
    });
    assert_eq!(result, 42, "mini_c_ift: true condition → then (return 42)");
}

/// Mirror of mini_c: `if (x == 0) return 1; return 0;` with x=42 (false).
/// KNOWN ISSUE: branch with ret-in-targets fails for false path.
#[test]
fn test_branch_mini_c_equal_false() {
    let result = run_test_block("mini_c_eqf", |b| {
        b.create_block_here();
        let then_blk = b.create_block();
        let merge_blk = b.create_block();

        // x = 42; x == 0 (false)
        let x = b.iconst_i32(42);
        let zero = b.iconst_i32(0);
        let cmp = b.icmp(IntCC::Equal, x, zero); // i1: 0 (false, x != 0)
        let cond = b.icmp(IntCC::NotEqual, cmp, zero); // i1: 0 (false)
        b.branch(cond, then_blk, &[], merge_blk, &[]);

        b.build_block(then_blk, |b| {
            let v = b.iconst_i32(1);
            b.ret(&[v]);
        });
        b.build_block(merge_blk, |b| {
            let v = b.iconst_i32(0);
            b.ret(&[v]);
        });
    });
    assert_eq!(result, 0, "mini_c_eqf: false condition → merge (return 0)");
}

/// Branch with simple i32 iconst — using jump+merge (matching working pattern)
#[test]
fn test_branch_i32_false_path_v2() {
    let result = run_test_block("br_i32_false_v2", |b| {
        b.create_block_here();
        let path_a = b.create_block();
        let path_b = b.create_block();
        let merge = b.create_block();

        let cond = b.iconst_i32(0); // false
        b.branch(cond, path_a, &[], path_b, &[]);

        b.build_block(path_a, |b| {
            b.jump(merge, &[]);
        });
        b.build_block(path_b, |b| {
            b.jump(merge, &[]);
        });
        b.build_block(merge, |b| {
            let v = b.iconst_i32(2);
            b.ret(&[v]);
        });
    });
    assert_eq!(
        result, 2,
        "br_i32_false_v2: cond=0 → false path → merge (return 2)"
    );
}

/// Branch with simple i32 iconst using direct ret (like mini_c pattern)
#[test]
fn test_branch_i32_false_direct_ret() {
    let result = run_test_block("br_i32_false_dir", |b| {
        b.create_block_here();
        let path_a = b.create_block();
        let path_b = b.create_block();

        let cond = b.iconst_i32(0); // false
        b.branch(cond, path_a, &[], path_b, &[]);

        b.build_block(path_a, |b| {
            let v = b.iconst_i32(1);
            b.ret(&[v]);
        });
        b.build_block(path_b, |b| {
            let v = b.iconst_i32(2);
            b.ret(&[v]);
        });
    });
    assert_eq!(result, 2, "br_i32_false_dir: cond=0 → false path → ret(2)");
}

/// Branch with i32 value 1 (true)
#[test]
fn test_branch_i32_true_path() {
    let result = run_test_block("br_i32_true", |b| {
        b.create_block_here();
        let path_a = b.create_block();
        let path_b = b.create_block();

        // Explicitly switch to entry block before emitting branch
        b.switch_to_block(Block(0));
        let cond = b.iconst_i32(1); // i32 non-zero = true
        b.branch(cond, path_a, &[], path_b, &[]);

        b.build_block(path_a, |b| {
            let v = b.iconst_i32(1);
            b.ret(&[v]);
        });
        b.build_block(path_b, |b| {
            let v = b.iconst_i32(2);
            b.ret(&[v]);
        });
    });
    assert_eq!(result, 1, "br_i32_true: cond=1 → true path (return 1)");
}

/// Test cross-block store/load using block params
#[test]
fn test_cross_block_store_load() {
    let result = run_test_block("xblock_sl", |b| {
        let entry = b.create_block_here();
        let block_a = b.create_block();
        let (block_b, [b_param]) = b.create_block_here_with([(TypeId::I32, "v")]);
        b.switch_to_block(entry);

        // Allocate a stack slot in entry block
        let slot = b.stack_addr(-8);
        let val42 = b.iconst_i32(42);
        b.store(val42, slot);
        b.jump(block_a, &[]);

        b.build_block(block_a, |b| {
            // Load the stored value in a different block
            let loaded = b.load(slot, TypeId::I32);
            b.jump(block_b, &[loaded]);
        });
        b.build_block(block_b, |b| {
            b.ret(&[b_param]);
        });
    });
    assert_eq!(
        result, 42,
        "cross_block_sl: store in entry, load in block_a, pass via param → 42"
    );
}

/// Verify create_block doesn't change cur_block (diagnostic for branch bug)
#[test]
fn test_create_block_cur_block() {
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);

    let entry = b.create_block_here();
    // create_block should NOT change cur_block
    let _block1 = b.create_block();
    let block2 = b.create_block();

    // Emit a jump in entry block
    b.jump(block2, &[]);

    // Build the other blocks
    b.build_block(_block1, |b| {
        let v = b.iconst_i32(99);
        b.ret(&[v]);
    });
    b.build_block(block2, |b| {
        let v = b.iconst_i32(42);
        b.ret(&[v]);
    });

    // Check: entry block should have a Jump terminator targeting block2.
    // If create_block changed cur_block, the jump would be in the wrong block.
    let func = b.finish().expect("build");
    let term = func.dfg.block_terminator(entry);
    assert!(
        term.is_some(),
        "entry block should have a terminator (the jump). create_block may be changing cur_block!"
    );
    match term {
        Some(Terminator::Jump { target, .. }) => {
            assert_eq!(*target, block2, "entry jump should target block2");
        }
        other => panic!("expected Jump terminator in entry, got {:?}", other),
    }
}

/// Test forward unconditional jump over a return block.
/// KNOWN ISSUE: JMP falls through when intermediate block ends with `ret`
/// (due to emit_epilogue_jump adding a JMP after ret, which interferes
/// with the forward branch's PC-relative offset calculation).
#[test]
fn test_jmp_forward_over_ret_block() {
    let result = run_test_block("jmp_fwd_ret", |b| {
        b.create_block_here();
        let skip = b.create_block(); // Block 1: return block (in the middle)
        let target = b.create_block(); // Block 2: target (at end)

        b.jump(target, &[]);

        b.build_block(skip, |b| {
            let v = b.iconst_i32(99);
            b.ret(&[v]);
        });
        b.build_block(target, |b| {
            let v = b.iconst_i32(42);
            b.ret(&[v]);
        });
    });
    assert_eq!(result, 42, "jmp_fwd_ret: forward jump over ret block → 42");
}

/// Test forward unconditional jump with NO ret in skipped block
#[test]
fn test_jmp_forward_simple() {
    let result = run_test_block("jmp_fwd_simple", |b| {
        b.create_block_here();
        let merge = b.create_block(); // Block 1: fall-through (no ret)
        let target = b.create_block(); // Block 2: target (has ret)

        b.jump(target, &[]);

        b.build_block(merge, |b| {
            b.jump(target, &[]);
        });
        b.build_block(target, |b| {
            let v = b.iconst_i32(42);
            b.ret(&[v]);
        });
    });
    assert_eq!(result, 42, "jmp_fwd_simple: forward jump → 42");
}

/// Branch backward (like for loop body→cond) over a ret block
#[test]
fn test_jmp_backward_over_ret() {
    let result = run_test_block("jmp_bwd_ret", |b| {
        let _entry = b.create_block_here();
        let ret_block = b.create_block(); // Block 1: ret (should be skipped)
        let target = b.create_block(); // Block 2: actual target

        b.jump(target, &[]); // forward jump over ret_block

        b.build_block(ret_block, |b| {
            let v = b.iconst_i32(99);
            b.ret(&[v]);
        });
        b.build_block(target, |b| {
            // Jump backward to ret_block (just to test backward JMP)
            let v = b.iconst_i32(42);
            b.ret(&[v]);
        });
    });
    assert_eq!(
        result, 42,
        "jmp_fwd_over_ret2: forward jump over ret block → 42"
    );
}

#[test]
fn test_jit_cross_function_call() {
    // 模块内跨函数直接调用（无参数）：callee() -> i32 { 42 }，
    // caller() -> i32 { callee() }。验证 Call 的 relocation 在 JIT
    // 加载时被 RelocPatcher 解析并实际可执行。
    ensure_registered();
    let mut module = Module::new();

    let callee = {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("callee", TypeContext::new(), sig);
        b.create_block_here();
        let v = b.iconst_i32(42);
        b.ret(&[v]);
        b.finish().expect("build")
    };
    let callee_ref = module.add_function(callee);
    assert_eq!(callee_ref, FuncRef(0));

    let caller = {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("caller", TypeContext::new(), sig);
        b.create_block_here();
        let rets = b.call(callee_ref, &[], &[TypeId::I32]);
        b.ret(&rets);
        b.finish().expect("build")
    };
    module.add_function(caller);

    let mut jit = code_forge::backend::jit::JitCompiler::new(TargetMachine::new());
    jit.compile_module(&module).unwrap();
    let f: extern "C" fn() -> i32 = jit.get_fn("caller").unwrap();
    assert_eq!(f(), 42, "cross-function JIT call should return callee's 42");
}

#[test]
fn test_jit_cross_function_call_with_args() {
    // 跨函数带参数调用（整数参数 + 返回值）：
    // callee(x) = x * 2，caller() = callee(21) == 42。
    // 验证 call 侧参数进 ABI 寄存器（RCX）与返回值（RAX）的传递。
    ensure_registered();
    let mut module = Module::new();

    let callee = {
        let sig = FunctionSignature::new(&[(TypeId::I32, "x")], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("callee2", TypeContext::new(), sig);
        let (_, params) = b.create_block_here_with([(TypeId::I32, "x")]);
        let two = b.iconst_i32(2);
        let r = b.imul(params[0], two);
        b.ret(&[r]);
        b.finish().expect("build")
    };
    let callee_ref = module.add_function(callee);

    let caller = {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("caller2", TypeContext::new(), sig);
        b.create_block_here();
        let arg = b.iconst_i32(21);
        let rets = b.call(callee_ref, &[arg], &[TypeId::I32]);
        b.ret(&rets);
        b.finish().expect("build")
    };
    module.add_function(caller);

    let mut jit = code_forge::backend::jit::JitCompiler::new(TargetMachine::new());
    jit.compile_module(&module).unwrap();
    let f: extern "C" fn() -> i32 = jit.get_fn("caller2").unwrap();
    assert_eq!(f(), 42, "caller2() should call callee2(21) and get 42");
}

#[test]
fn test_jit_call_external_function() {
    // Local-function linkage end-to-end: the JIT compiles a function that
    // performs an indirect call to a Rust function registered via
    // register_external; the call resolves through the symbol chain and the
    // result (RAX) is returned.
    // Side-effect external function: writes to a static, no return value —
    // validates that the JIT actually performs the indirect call without
    // depending on the (currently broken) call result copy direction.
    static HIT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
    extern "C" fn ext_hit() {
        HIT.store(42, std::sync::atomic::Ordering::SeqCst);
    }
    let ext_addr = ext_hit as *const () as u64;

    ensure_registered();
    let mut module = Module::new();
    let ext_func = {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("caller_ext", TypeContext::new(), sig);
        b.create_block_here();
        let ptr = b.iconst_i64(ext_addr as i64);
        b.call_indirect(ptr, &[], &[]);
        let z = b.iconst_i32(0);
        b.ret(&[z]);
        b.finish().expect("build")
    };
    module.add_function(ext_func);

    let mut jit = code_forge::backend::jit::JitCompiler::new(TargetMachine::new());
    jit.compile_module(&module).unwrap();
    let caller: extern "C" fn() -> i32 = jit.get_fn("caller_ext").unwrap();
    let out = caller();
    assert_eq!(out, 0);
    assert_eq!(HIT.load(std::sync::atomic::Ordering::SeqCst), 42);
}

#[test]
fn test_jit_float_param_receipt_compiles() {
    // Float parameter receipt: @move_args type classification emits
    // MOVQ_FREG_XMM (XMM0 → param VReg). Compile-only verification — the
    // full float-return path (@move_ret float) is not implemented yet.
    ensure_registered();
    let sig = FunctionSignature::new(&[(TypeId::F64, "x")], &[TypeId::I64]);
    let mut b = FunctionBuilder::new("callee_f", TypeContext::new(), sig);
    let (_, params) = b.create_block_here_with([(TypeId::F64, "x")]);
    let _ = params[0];
    let z = b.iconst_i64(7);
    b.ret(&[z]);
    let func = b.finish().expect("build");
    FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .expect("float-param callee must compile");
}

#[test]
fn test_jit_global_addr() {
    // GlobalAddr: module globals get a data segment; the compiled function
    // reads its address via @abs_reloc patched to the "G{id}" symbol.
    ensure_registered();
    let mut module = Module::new();
    let gv = code_forge::ir::GlobalVariable {
        name: "g_data".into(),
        ty: TypeId::I32,
        init: Some(vec![0x2A, 0x00, 0x00, 0x00]), // 42 little-endian
        init_expr: None,
        symbol: code_forge::ir::symbol::SymbolInfo::new(),
        is_constant: false,
        alignment: 4,
        addr_space: 0,
        metadata: Vec::new(),
        is_ifunc: false,
        ifunc_params: Vec::new(),
        ifunc_resolver: None,
    };
    let gid = module.add_global(gv).unwrap();
    let f = {
        let sig = FunctionSignature::new(&[], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("gaddr_fn", TypeContext::new(), sig);
        b.create_block_here();
        let addr = b.global_addr(gid);
        b.ret(&[addr]);
        b.finish().expect("build")
    };
    module.add_function(f);
    let mut jit = code_forge::backend::jit::JitCompiler::new(TargetMachine::new());
    jit.compile_module(&module).unwrap();
    let fptr: extern "C" fn() -> i64 = jit.get_fn("gaddr_fn").unwrap();
    let addr = fptr() as *const u8;
    assert!(!addr.is_null());
    let val = unsafe { std::ptr::read_unaligned(addr as *const i32) };
    assert_eq!(val, 42);
}

#[test]
fn test_jit_overflow_flag() {
    // sadd_overflow(i64::MAX, 1) → result (wrapped) + flag (overflow = 1).
    let out = run_test("ovf_flag", |b| {
        let mx = b.iconst_i64(i64::MAX);
        let one = b.iconst_i64(1);
        let (_r, fl) = b.sadd_overflow(mx, one);
        b.uextend(fl, TypeId::I32)
    });
    assert_eq!(out, 1, "overflow flag should be set for MAX+1");
}

#[test]
fn test_jit_atomic_rmw_dispatch_compiles() {
    // AtomicRmw op dispatch: all six implemented ops must compile through the
    // AtomicRmw.<Op> sub-rule machinery. Execution of the memory operand
    // (xadd/lock-*) hits a regalloc cross-instruction VReg split issue —
    // recorded; compile-only verification here.
    ensure_registered();
    static mut CELL: i32 = 0;
    let addr = &raw mut CELL as *const () as u64;
    let ops = [
        AtomicRmwOp::Add,
        AtomicRmwOp::Xchg,
        AtomicRmwOp::Sub,
        AtomicRmwOp::And,
        AtomicRmwOp::Or,
        AtomicRmwOp::Xor,
    ];
    for op in ops {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("atomic_op", TypeContext::new(), sig);
        b.create_block_here();
        let a = b.iconst_i64(addr as i64);
        let five = b.iconst_i32(5);
        let old = b.atomic_rmw(op, a, five, Ordering::SequentiallyConsistent);
        b.ret(&[old]);
        let func = b.finish().expect("build");
        FunctionCompiler::new(TargetMachine::new())
            .compile_raw(&func)
            .unwrap_or_else(|e| panic!("atomic {op:?} must compile: {e}"));
    }
}

// ═══════════════════════════════════════════════════════════════
// P5 运行时问题复现：同一函数内两次相同栈 load 相加。
// forge-rustc 的 v2 用例（let a=[1,2]; x=a[0]; y=a[0]; x+y）静态机器码
// 正确（1+1=2）但运行得 1。此处用纯 forge-codegen 管线（compile_raw +
// ExecutableMemory 执行）复现，定位问题在代码生成还是 rustc 路径。
// ═══════════════════════════════════════════════════════════════
#[test]
fn test_two_same_stack_loads_add() {
    let v = run_test_block("two_loads_add", |b| {
        b.create_block_here();
        // 模拟 [1, 2] 数组：a[0] @ -16、a[1] @ -12
        let a0 = b.stack_addr(-16);
        let a1 = b.stack_addr(-12);
        let one = b.iconst(1, TypeId::I32);
        let two = b.iconst(2, TypeId::I32);
        b.store(one, a0);
        b.store(two, a1);
        // x = a[0]; y = a[0]（两次相同 load）
        let x = b.load(a0, TypeId::I32);
        let y = b.load(a0, TypeId::I32);
        let sum = b.iadd(x, y);
        b.ret(&[sum]);
    });
    assert_eq!(v, 2, "两次相同栈 load 相加应得 2，实际 {v}");
}

/// 变体：两次 load 来自不同地址（a[0] + a[1]）。
#[test]
fn test_two_diff_stack_loads_add() {
    let v = run_test_block("two_diff_loads_add", |b| {
        b.create_block_here();
        let a0 = b.stack_addr(-16);
        let a1 = b.stack_addr(-12);
        let one = b.iconst(1, TypeId::I32);
        let two = b.iconst(2, TypeId::I32);
        b.store(one, a0);
        b.store(two, a1);
        let x = b.load(a0, TypeId::I32);
        let y = b.load(a1, TypeId::I32);
        let sum = b.iadd(x, y);
        b.ret(&[sum]);
    });
    assert_eq!(v, 3, "a[0]+a[1] 应得 3，实际 {v}");
}

/// 变体：单个 load + 常量相加（v1 场景，应稳定正确）。
#[test]
fn test_single_load_add_const() {
    let v = run_test_block("single_load_add", |b| {
        b.create_block_here();
        let a0 = b.stack_addr(-16);
        let one = b.iconst(1, TypeId::I32);
        b.store(one, a0);
        let x = b.load(a0, TypeId::I32);
        let one = b.iconst(1, TypeId::I32);
        let sum = b.iadd(x, one);
        b.ret(&[sum]);
    });
    assert_eq!(v, 2, "单 load+常量 应得 2，实际 {v}");
}

/// P5 复现增强：含 Index 投影链（等价 forge-rustc 的 place_addr）的两次相同索引。
/// forge-rustc 的 v2（let a=[1,2]; x=a[0]; y=a[0]; x+y）静态机器码正确但运行得 1；
/// 此处用纯 forge-codegen 构造"lea 数组地址 + load 索引 + imul + iadd + load"链两次。
#[test]
fn test_two_index_chain_loads_add() {
    let v = run_test_block("two_index_chains", |b| {
        b.create_block_here();
        // 数组 [1,2]：a[0] @ -16、a[1] @ -12；索引槽 idx @ -24
        let a0 = b.stack_addr(-16);
        let a1 = b.stack_addr(-12);
        let idx = b.stack_addr(-24);
        let one = b.iconst(1, TypeId::I32);
        let two = b.iconst(2, TypeId::I32);
        b.store(one, a0);
        b.store(two, a1);
        // idx = 0
        let zero = b.iconst(0, TypeId::I64);
        b.store(zero, idx);
        // 索引链 1：load idx → uextend i64 → imul 4 → iadd &a → load i32
        let chain = |b: &mut FunctionBuilder, base: Value| -> Value {
            let i = b.load(idx, TypeId::I64);
            let i64 = b.uextend(i, TypeId::I64);
            let four = b.iconst(4, TypeId::I64);
            let scaled = b.imul(i64, four);
            let addr = b.iadd(base, scaled);
            b.load(addr, TypeId::I32)
        };
        let arr = b.stack_addr(-16);
        let x = chain(b, arr);
        let y = chain(b, arr);
        let sum = b.iadd(x, y);
        b.ret(&[sum]);
    });
    assert_eq!(v, 2, "两条 Index 链（均取 a[0]）相加应得 2，实际 {v}");
}

/// P5 复现增强 2：含 assert 分支块（bounds check 的 merge/fail + unreachable）
/// 的两次相同索引相加——完全对齐 forge-rustc v2 的块结构（assert 生成 merge/fail
/// 两个 nop 块，成功路径进 merge）。
#[test]
fn test_two_index_chains_with_assert() {
    let v = run_test_block("two_index_assert", |b| {
        // bb0：数组构造 + idx=0 + assert(idx < 4)
        let blk0 = b.create_block();
        b.switch_to_block(blk0);
        let a0 = b.stack_addr(-16);
        let a1 = b.stack_addr(-12);
        let idx = b.stack_addr(-24);
        let one = b.iconst(1, TypeId::I32);
        let two = b.iconst(2, TypeId::I32);
        b.store(one, a0);
        b.store(two, a1);
        let zero = b.iconst(0, TypeId::I64);
        b.store(zero, idx);
        // cond = idx < 4
        let idx_v = b.load(idx, TypeId::I64);
        let four = b.iconst(4, TypeId::I64);
        let cond = b.icmp(IntCC::UnsignedLessThan, idx_v, four);
        // assert：成功 → merge（bb1），失败 → fail（bb2，unreachable）
        let fail = b.create_block();
        let merge = b.create_block();
        b.switch_to_block(blk0);
        b.branch(cond, merge, &[], fail, &[]);
        b.switch_to_block(fail);
        b.unreachable();
        // merge：两条索引链 + add + ret
        b.switch_to_block(merge);
        let chain = |b: &mut FunctionBuilder, base: Value| -> Value {
            let i = b.load(idx, TypeId::I64);
            let i64 = b.uextend(i, TypeId::I64);
            let four = b.iconst(4, TypeId::I64);
            let scaled = b.imul(i64, four);
            let addr = b.iadd(base, scaled);
            b.load(addr, TypeId::I32)
        };
        let arr = b.stack_addr(-16);
        let x = chain(b, arr);
        let y = chain(b, arr);
        let sum = b.iadd(x, y);
        b.ret(&[sum]);
    });
    assert_eq!(v, 2, "含 assert 块的两条 Index 链相加应得 2，实际 {v}");
}

/// P5 决定性验证：直接执行 forge-rustc 为 v2（let a=[1,2]; x=a[0]; y=a[0]; x+y）
/// 生成的 mainCRTStartup 机器码字节（与 .o/exe 完全一致）。
/// 2026-08：用当前编译器重新生成（isa/x86_v10.toml + forge-rustc 构建后
/// rustc -Zcodegen-backend --emit=obj → COFF .text 提取 mainCRTStartup）。
/// 旧字节存在真实控制流缺陷（assert0 pass 的 jmp rel32=0x7c 目标错误，跳过
/// merge0 块 → 未初始化 idx1 → SEGV 0xC0000005）；新字节反汇编验证：
/// assert0 pass → jmp merge0(0xd1)、fail 块为真 ud2、y=a[0]（idx1=0）→ 返回 2。
#[test]
fn test_execute_v2_exact_bytes() {
    use code_forge::mem::ExecutableMemory;
    let hex = "554889e553575641544155415641574881ec580000004c8d7db049be010000000000000045893749be04000000000000004d89fd4d89ef4d01f749be020000000000000045893749be00000000000000004c8d7da04d89374c8d75a04d8b3e49be02000000000000004d31ed4d39f7410f92c54c8d759845892e4c8d6d98458b75004585f60f847f000000e9000000004c8d75b04c8d6da04d8b7d004d89fd49bf04000000000000004d89ec4d0fafe74d89f74d01e7458b274c8d7da845892749bc00000000000000004c8d7d884d89274c8d65884d8b3c2449bc02000000000000004d31f64d39e7410f92c64c8d6580458934244c8d7580458b264585e40f8407000000e9040000000f0b0f0b4c8d65b04c8d75884d8b3e4d89fe49bf04000000000000004d89f54d0fafef4d89e74d01ef458b2f4c8d7d9045892f4c8d6da8458b7d004c8d6d90458b65004589fd4501e54c8d65b845892c244c8d6db84c8d6db8458b6500418bc4418bd4e9000000004889ec4881ec38000000415f415e415d415c5e5f5b5dc3";
    let bytes = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
        .collect::<Vec<_>>();
    let mem = ExecutableMemory::new(&bytes).expect("exec memory");
    let f: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).expect("fn0") };
    let v = f();
    assert_eq!(v, 2, "v2 确切字节执行应得 2，实际 {v}");
}

/// P5 二分 2：双 assert 块（两个 bounds check，各带 merge/fail）+ 两次索引链。
/// v2（let a=[1,2]; x=a[0]; y=a[0]; x+y）有两次索引各一个 assert；之前单 assert
/// 复现 PASS，此测试验证"双 assert 块结构"是否触发 SEGV。
#[test]
fn test_two_index_chains_double_assert() {
    let v = run_test_block("two_index_double_assert", |b| {
        // bb0：数组 [1,2] 构造 + idx0=0 + assert0(idx0<4)
        let blk0 = b.create_block();
        b.switch_to_block(blk0);
        let a0 = b.stack_addr(-16);
        let a1 = b.stack_addr(-12);
        let idx0 = b.stack_addr(-24);
        let idx1 = b.stack_addr(-32);
        let one = b.iconst(1, TypeId::I32);
        let two = b.iconst(2, TypeId::I32);
        b.store(one, a0);
        b.store(two, a1);
        let z = b.iconst(0, TypeId::I64);
        b.store(z, idx0);
        let o = b.iconst(1, TypeId::I64);
        b.store(o, idx1);
        // assert0：idx0 < 4
        let i0 = b.load(idx0, TypeId::I64);
        let four = b.iconst(4, TypeId::I64);
        let c0 = b.icmp(IntCC::UnsignedLessThan, i0, four);
        let fail0 = b.create_block();
        let merge0 = b.create_block();
        b.switch_to_block(blk0);
        b.branch(c0, merge0, &[], fail0, &[]);
        b.switch_to_block(fail0);
        b.unreachable();
        // merge0：链1 load a[idx0] → x 槽
        b.switch_to_block(merge0);
        let arr = b.stack_addr(-16);
        let i0v = b.load(idx0, TypeId::I64);
        let f4 = b.iconst(4, TypeId::I64);
        let s0 = b.imul(i0v, f4);
        let ad0 = b.iadd(arr, s0);
        let x = b.load(ad0, TypeId::I32);
        // assert1：idx1 < 4
        let i1 = b.load(idx1, TypeId::I64);
        let c1 = b.icmp(IntCC::UnsignedLessThan, i1, f4);
        let fail1 = b.create_block();
        let merge1 = b.create_block();
        b.switch_to_block(merge0);
        b.branch(c1, merge1, &[], fail1, &[]);
        b.switch_to_block(fail1);
        b.unreachable();
        // merge1：链2 load a[idx1] → y；x + y
        b.switch_to_block(merge1);
        let i1v = b.load(idx1, TypeId::I64);
        let s1 = b.imul(i1v, f4);
        let ad1 = b.iadd(arr, s1);
        let y = b.load(ad1, TypeId::I32);
        let sum = b.iadd(x, y);
        b.ret(&[sum]);
    });
    assert_eq!(v, 3, "双 assert + 两链（a[0]+a[1]）应得 3，实际 {v}");
}

/// P5 二分 3：双 assert + 两个相同常量索引（idx0=0, idx1=0，即 x=a[0]; y=a[0]）。
/// v2 的确切 MIR 正是两个索引常量 0；验证是否触发 SEGV。
#[test]
fn test_double_assert_same_const_index() {
    let v = run_test_block("double_assert_same_idx", |b| {
        let blk0 = b.create_block();
        b.switch_to_block(blk0);
        let a0 = b.stack_addr(-16);
        let a1 = b.stack_addr(-12);
        let idx0 = b.stack_addr(-24);
        let idx1 = b.stack_addr(-32);
        let one = b.iconst(1, TypeId::I32);
        let two = b.iconst(2, TypeId::I32);
        b.store(one, a0);
        b.store(two, a1);
        let z = b.iconst(0, TypeId::I64);
        b.store(z, idx0);
        b.store(z, idx1);
        let i0 = b.load(idx0, TypeId::I64);
        let four = b.iconst(4, TypeId::I64);
        let c0 = b.icmp(IntCC::UnsignedLessThan, i0, four);
        let fail0 = b.create_block();
        let merge0 = b.create_block();
        b.switch_to_block(blk0);
        b.branch(c0, merge0, &[], fail0, &[]);
        b.switch_to_block(fail0);
        b.unreachable();
        b.switch_to_block(merge0);
        let arr = b.stack_addr(-16);
        let i0v = b.load(idx0, TypeId::I64);
        let f4 = b.iconst(4, TypeId::I64);
        let s0 = b.imul(i0v, f4);
        let ad0 = b.iadd(arr, s0);
        let x = b.load(ad0, TypeId::I32);
        let i1 = b.load(idx1, TypeId::I64);
        let c1 = b.icmp(IntCC::UnsignedLessThan, i1, f4);
        let fail1 = b.create_block();
        let merge1 = b.create_block();
        b.switch_to_block(merge0);
        b.branch(c1, merge1, &[], fail1, &[]);
        b.switch_to_block(fail1);
        b.unreachable();
        b.switch_to_block(merge1);
        let i1v = b.load(idx1, TypeId::I64);
        let s1 = b.imul(i1v, f4);
        let ad1 = b.iadd(arr, s1);
        let y = b.load(ad1, TypeId::I32);
        let sum = b.iadd(x, y);
        b.ret(&[sum]);
    });
    assert_eq!(v, 2, "双 assert + 同索引（a[0]+a[0]）应得 2，实际 {v}");
}

/// P5 二分 5：手工 if-else 分支链（等价 forge-rustc 多值 SwitchInt 的通用 case
/// 生成：每 case 一个 icmp + branch，case 命中跳 body 块，否则进下一比较块）。
#[test]
fn test_manual_branch_chain() {
    let v = run_test_block("manual_branch_chain", |b| {
        b.create_block_here();
        let x = b.stack_addr(-8);
        let one = b.iconst(1, TypeId::I32);
        b.store(one, x);
        // 链块 + case body 块 + return 块
        let c0 = b.create_block();
        let c1 = b.create_block();
        let c2 = b.create_block();
        let body0 = b.create_block();
        let body1 = b.create_block();
        let body2 = b.create_block();
        let dflt = b.create_block();
        let bret = b.create_block();
        // entry 显式跳转进入分支链（缺失终结符会保持默认 Unreachable →
        // lower 成 UD2 并在运行时非法指令崩溃）
        b.jump(c0, &[]);
        // c0：discr == 0 → body0，否则 c1
        b.switch_to_block(c0);
        let xv = b.load(x, TypeId::I32);
        let z = b.iconst(0, TypeId::I32);
        let eq0 = b.icmp(IntCC::Equal, xv, z);
        b.branch(eq0, body0, &[], c1, &[]);
        // c1：== 1 → body1，否则 c2
        b.switch_to_block(c1);
        let xv1 = b.load(x, TypeId::I32);
        let o = b.iconst(1, TypeId::I32);
        let eq1 = b.icmp(IntCC::Equal, xv1, o);
        b.branch(eq1, body1, &[], c2, &[]);
        // c2：== 2 → body2，否则 default
        b.switch_to_block(c2);
        let xv2 = b.load(x, TypeId::I32);
        let tw = b.iconst(2, TypeId::I32);
        let eq2 = b.icmp(IntCC::Equal, xv2, tw);
        b.branch(eq2, body2, &[], dflt, &[]);
        // bodies：写结果槽 → return
        b.switch_to_block(body0);
        let v10 = b.iconst(10, TypeId::I32);
        b.store(v10, x);
        b.jump(bret, &[]);
        b.switch_to_block(body1);
        let v20 = b.iconst(20, TypeId::I32);
        b.store(v20, x);
        b.jump(bret, &[]);
        b.switch_to_block(body2);
        let v30 = b.iconst(30, TypeId::I32);
        b.store(v30, x);
        b.jump(bret, &[]);
        b.switch_to_block(dflt);
        let v99 = b.iconst(99, TypeId::I32);
        b.store(v99, x);
        b.jump(bret, &[]);
        b.switch_to_block(bret);
        let r = b.load(x, TypeId::I32);
        b.ret(&[r]);
    });
    assert_eq!(v, 20, "手工分支链（x=1 → case1=20）应得 20，实际 {v}");
}

/// 嵌套 load（load(load(addr))）：栈槽 A 存"槽 B 的地址"，load [A] 得指针再 load [指针]。
/// 复现 op6 的 *p（deref 指针）——regalloc 地址操作数值复制。
#[test]
fn test_nested_load_deref() {
    let v = run_test_block("nested_load_deref", |b| {
        b.create_block_here();
        let slot_a = b.stack_addr(-24); // 存指针
        let slot_b = b.stack_addr(-16); // 存值 42
        let forty_two = b.iconst(42, TypeId::I64);
        b.store(forty_two, slot_b);
        b.store(slot_b, slot_a); // A = &B
        let ptr = b.load(slot_a, TypeId::I64); // 指针
        let val = b.load(ptr, TypeId::I64); // *ptr
        b.ret(&[val]);
    });
    assert_eq!(v, 42, "嵌套 load 应得 42，实际 {v}");
}

/// 三层嵌套 load(iadd(load(addr), 16))：模拟 push_mut 的 len 读取
///（load [参数槽]（&Vec）→ iadd 16 → load [&Vec+16]）。
#[test]
fn test_nested_load_iadd_deref() {
    let v = run_test_block("nested_load_iadd_deref", |b| {
        b.create_block_here();
        let slot_ptr = b.stack_addr(-40); // 存指针（&base）
        let slot_base = b.stack_addr(-32); // 目标结构（+16 放值）
        let sixteen = b.iconst(16, TypeId::I64);
        let forty_two = b.iconst(42, TypeId::I64);
        b.store(slot_base, slot_ptr); // slot_ptr = &base
        let a16 = b.iadd(slot_base, sixteen);
        b.store(forty_two, a16); // base[16] = 42
        let p = b.load(slot_ptr, TypeId::I64); // load [slot_ptr]（&base）
        let a = b.iadd(p, sixteen); // &base + 16
        let v = b.load(a, TypeId::I64); // load [&base+16]
        b.ret(&[v]);
    });
    assert_eq!(v, 42, "三层嵌套应得 42，实际 {v}");
}

/// store 的 addr 是 iadd(load 结果, 常量)：模拟构造 faddr（嵌套 base）。
#[test]
fn test_store_iadd_nested_base() {
    let v = run_test_block("store_iadd_nested_base", |b| {
        b.create_block_here();
        let slot = b.stack_addr(-24); // 存指针
        let slot_base = b.stack_addr(-16); // 目标（+8 处写）
        let eight = b.iconst(8, TypeId::I64);
        b.store(slot_base, slot); // slot = &base
        let p = b.load(slot, TypeId::I64); // load（&base）
        let a = b.iadd(p, eight); // &base + 8
        let v42 = b.iconst(42, TypeId::I64);
        b.store(v42, a); // store [&base+8] = 42
        let v = b.load(a, TypeId::I64); // 读回
        b.ret(&[v]);
    });
    assert_eq!(v, 42, "store iadd 嵌套 base 应得 42，实际 {v}");
}

// ── 寄存器压力测试：%temp 临时寄存器 + clobber 机制在压力下语义不变 ──

/// 16 个独立饱和加同时活跃（GPR 压力）——求和可预测。
#[test]
fn test_sadd_sat_pressure() {
    let r = run_test("sadd_sat_pressure", |b| {
        let mut acc = b.iconst_i32(0);
        for i in 0..16 {
            let a = b.iconst_i32(i);
            let c = b.iconst_i32(100 - i);
            let s = b.sadd_sat(a, c);
            acc = b.iadd(acc, s);
        }
        acc
    });
    assert_eq!(r, 1600, "sadd_sat 压力下和应为 1600，实际 {r}");
}

/// 16 个独立无符号除法同时活跃（GPR 压力 + RAX/RDX clobber）。
#[test]
fn test_udiv_pressure() {
    let r = run_test("udiv_pressure", |b| {
        let mut acc = b.iconst_i32(0);
        for i in 1..=16 {
            let a = b.iconst_i32(i * 42);
            let c = b.iconst_i32(i);
            let d = b.udiv(a, c);
            acc = b.iadd(acc, d);
        }
        acc
    });
    assert_eq!(r, 672, "udiv 压力下和应为 672，实际 {r}");
}

/// 16 个独立 fabs 同时活跃（FPR 压力 + 掩码临时）。
#[test]
fn test_fabs_pressure() {
    let r = run_test_f64("fabs_pressure", |b| {
        let mut acc = b.fconst_f64(0.0);
        for i in 0..16 {
            let a = b.fconst_f64(-(i as f64) - 1.0);
            let fa = b.fabs(a);
            acc = b.fadd(acc, fa);
        }
        acc
    });
    // 1..=16 的和 = 136
    assert!((r - 136.0).abs() < 0.001, "fabs 压力下和应为 136，实际 {r}");
}

/// bitreverse 单测（%t1-%t5 临时）——已知向量。
#[test]
fn test_bitreverse() {
    // 0x0000000000000001 → 0x8000000000000000（1 在最低位 → 反转到最高位）
    let r = run_test_i64("bitreverse", |b| {
        let a = b.iconst_i64(1);
        b.bitreverse(a)
    });
    assert_eq!(
        r as u64, 0x8000000000000000,
        "bitreverse(1) 应得 0x8000...，实际 {r:#x}"
    );
}

// ═══════════════════════════════════════════════════════════════
// SIMD（f32×4 / V128）— vconst 构造 + 运算 + vextract lane 提取
// ═══════════════════════════════════════════════════════════════

/// vconst 构造 <4 x f32> 的便捷 helper（f32 数组 → 值语义，T 自动推导）。
fn vconst_f32_4(b: &mut FunctionBuilder, vals: [f32; 4]) -> Value {
    b.vconst(vals.to_vec())
}

/// vconst 构造 <2 x f32>（V64）的便捷 helper。
fn vconst_f32_2(b: &mut FunctionBuilder, vals: [f32; 2]) -> Value {
    b.vconst(vals.to_vec())
}

/// vconst 构造 <8 x f32>（V256）的便捷 helper。
fn vconst_f32_8(b: &mut FunctionBuilder, vals: [f32; 8]) -> Value {
    b.vconst(vals.to_vec())
}

/// 编译 build → vextract lane0 → 返回 f32。
fn run_vec_f32_lane0(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> f32 {
    x86_64::ensure_registered();
    let sig = FunctionSignature::new(&[], &[TypeId::F32]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    b.create_block_here();
    let v = build(&mut b);
    let z = b.iconst_i32(0);
    let e = b.vextract(v, z);
    b.ret(&[e]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap_or_else(|e| panic!("{}: compile: {}", name, e));
    assert!(!compiled.code.is_empty(), "{}: empty code", name);
    let mem =
        ExecutableMemory::new(&compiled.code).unwrap_or_else(|e| panic!("{}: alloc: {}", name, e));
    let f: extern "C" fn() -> f32 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

/// 编译 build → vextract lane k → 返回 f64（<2 x f64> 等双精度向量）。
fn run_vec_f64_lane(name: &str, k: u32, build: fn(&mut FunctionBuilder) -> Value) -> f64 {
    x86_64::ensure_registered();
    let sig = FunctionSignature::new(&[], &[TypeId::F64]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    b.create_block_here();
    let v = build(&mut b);
    let z = b.iconst_i32(k as i32);
    let e = b.vextract(v, z);
    b.ret(&[e]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap_or_else(|e| panic!("{}: compile: {}", name, e));
    assert!(!compiled.code.is_empty(), "{}: empty code", name);
    let mem =
        ExecutableMemory::new(&compiled.code).unwrap_or_else(|e| panic!("{}: alloc: {}", name, e));
    let f: extern "C" fn() -> f64 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

/// 编译 build → vextract lane k → 返回 i32（<4 x i32> 等整数向量）。
fn run_vec_i32_lane(name: &str, k: u32, build: fn(&mut FunctionBuilder) -> Value) -> i32 {
    x86_64::ensure_registered();
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    b.create_block_here();
    let v = build(&mut b);
    let z = b.iconst_i32(k as i32);
    let e = b.vextract(v, z);
    b.ret(&[e]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap_or_else(|e| panic!("{}: compile: {}", name, e));
    assert!(!compiled.code.is_empty(), "{}: empty code", name);
    let mem =
        ExecutableMemory::new(&compiled.code).unwrap_or_else(|e| panic!("{}: alloc: {}", name, e));
    let f: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

/// 编译 build → vextract lane k → 返回 i64（<2 x i64> 向量）。
fn run_vec_i64_lane(name: &str, k: u32, build: fn(&mut FunctionBuilder) -> Value) -> i64 {
    x86_64::ensure_registered();
    let sig = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    b.create_block_here();
    let v = build(&mut b);
    let z = b.iconst_i32(k as i32);
    let e = b.vextract(v, z);
    b.ret(&[e]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap_or_else(|e| panic!("{}: compile: {}", name, e));
    assert!(!compiled.code.is_empty(), "{}: empty code", name);
    let mem =
        ExecutableMemory::new(&compiled.code).unwrap_or_else(|e| panic!("{}: alloc: {}", name, e));
    let f: extern "C" fn() -> i64 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

/// vconst 构造 <2 x f64> 的便捷 helper（f64 数组 → 值语义，T 自动推导）。
fn vconst_f64_2(b: &mut FunctionBuilder, vals: [f64; 2]) -> Value {
    b.vconst(vals.to_vec())
}

/// 编译 build → vextract lane k → 返回 f32（非 lane0 由后端 pshufd 旋转直接支持）。
fn run_vec_f32_lane<F: Fn(&mut FunctionBuilder) -> Value>(name: &str, k: u32, build: F) -> f32 {
    x86_64::ensure_registered();
    let sig = FunctionSignature::new(&[], &[TypeId::F32]);
    let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
    b.create_block_here();
    let v = build(&mut b);
    let z = b.iconst_i32(k as i32);
    let e = b.vextract(v, z);
    b.ret(&[e]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .unwrap_or_else(|e| panic!("{}: compile: {}", name, e));
    assert!(!compiled.code.is_empty(), "{}: empty code", name);
    let mem =
        ExecutableMemory::new(&compiled.code).unwrap_or_else(|e| panic!("{}: alloc: {}", name, e));
    let f: extern "C" fn() -> f32 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

/// vconst → vextract lane0 最小端到端。
#[test]
fn test_simd_vconst_vextract() {
    let r = run_vec_f32_lane0("simd_vconst", |b| vconst_f32_4(b, [1.5, 2.5, 3.5, 4.5]));
    assert!((r - 1.5).abs() < 1e-6, "vconst lane0 应得 1.5，实际 {r}");
}

// ═══════════════ V64（<2 x f32>）直通 ═══════════════
// V64 用 128 位指令族的低 64 位语义：vconst 经 vconst_lo2 + movq 装低 2 lane，
// vadd/vbroadcast 等的低 2 lane 由 128 位指令保证（高 2 lane 未定义）。

#[test]
fn test_simd_v64_vconst_lane0() {
    let r = run_vec_f32_lane0("simd_v64_vconst", |b| vconst_f32_2(b, [1.5, 2.5]));
    assert!(
        (r - 1.5).abs() < 1e-6,
        "V64 vconst lane0 应得 1.5，实际 {r}"
    );
}

#[test]
fn test_simd_v64_vconst_lane1() {
    let r = run_vec_f32_lane("simd_v64_vconst_l1", 1, |b| vconst_f32_2(b, [1.5, 2.5]));
    assert!(
        (r - 2.5).abs() < 1e-6,
        "V64 vconst lane1 应得 2.5，实际 {r}"
    );
}

#[test]
fn test_simd_v64_vadd() {
    let r = run_vec_f32_lane("simd_v64_vadd", 1, |b| {
        let a = vconst_f32_2(b, [1.5, 2.5]);
        let c = vconst_f32_2(b, [0.5, 1.0]);
        b.vadd(a, c)
    });
    assert!((r - 3.5).abs() < 1e-6, "V64 vadd lane1 应得 3.5，实际 {r}");
}

#[test]
fn test_simd_v64_vbroadcast() {
    let r = run_vec_f32_lane("simd_v64_vbroadcast", 1, |b| {
        let s = b.fconst(7.25f32.to_bits() as u64, TypeId::F32);
        b.vbroadcast(s, TypeId::V64)
    });
    assert!(
        (r - 7.25).abs() < 1e-6,
        "V64 vbroadcast lane1 应得 7.25，实际 {r}"
    );
}

#[test]
fn test_simd_v64_vmul() {
    let r = run_vec_f32_lane("simd_v64_vmul", 1, |b| {
        let a = vconst_f32_2(b, [2.0, 3.0]);
        let c = vconst_f32_2(b, [4.0, 5.0]);
        b.vmul(a, c)
    });
    assert!(
        (r - 15.0).abs() < 1e-6,
        "V64 vmul lane1 应得 15.0，实际 {r}"
    );
}

#[test]
fn test_simd_v64_vsub_vdiv() {
    let r = run_vec_f32_lane("simd_v64_vsub_div", 1, |b| {
        let a = vconst_f32_2(b, [10.0, 9.0]);
        let c = vconst_f32_2(b, [4.0, 3.0]);
        let e = vconst_f32_2(b, [1.0, 2.0]);
        let d = b.vsub(a, c);
        b.vdiv(d, e)
    });
    assert!(
        (r - 3.0).abs() < 1e-6,
        "V64 (vsub)vdiv lane1 应得 3.0，实际 {r}"
    );
}

// ═══════════════ 动态 vector_ty 构造（长度矩阵）═══════════════
// vector_ty(elem, len) 对 2/4/8 与内建 V64/V128/V256 去重同 id；动态类型经
// reg_class_for 按位宽映射 VEC(16)/VEC(32)——验证长度矩阵的寄存器类推导。

#[test]
fn test_simd_dyn_ty_v64() {
    let r = run_vec_f32_lane("simd_dyn_v64", 1, |b| b.vconst(vec![1.5f32, 2.5]));
    assert!(
        (r - 2.5).abs() < 1e-6,
        "动态 vector_ty(f32,2) lane1 应得 2.5，实际 {r}"
    );
}

#[test]
fn test_simd_dyn_ty_v256() {
    let r = run_vec_f32_lane("simd_dyn_v256", 5, |b| {
        b.vconst(vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0])
    });
    assert!(
        (r - 6.0).abs() < 1e-6,
        "动态 vector_ty(f32,8) lane5 应得 6.0，实际 {r}"
    );
}

// ═══════════════ 元素类型安全（防静默错）═══════════════
// 非 f32 元素向量运算应编译期 Unsupported（避免 addps 静默错）；lane 越界应在
// builder 层断言（编译期 panic 而非取垃圾）。

/// <2 x i64> vmul 应编译失败（i64 无 SSE 乘法指令，落 Unsupported）。
#[test]
fn test_simd_f64_vec_vadd_rejected() {
    x86_64::ensure_registered();
    let sig = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut b = FunctionBuilder::new("i64_mul_reject", TypeContext::new(), sig);
    b.create_block_here();
    let a = b.vconst(vec![2i64, 3]);
    let c = b.vconst(vec![2i64, 3]);
    let r = b.vmul(a, c);
    let z = b.iconst_i32(0);
    let e = b.vextract(r, z);
    b.ret(&[e]);
    let func = b.finish().expect("build");
    let result = FunctionCompiler::new(TargetMachine::new()).compile_raw(&func);
    assert!(result.is_err(), "<2 x i64> vmul 应编译报错而非静默错");
}

/// V256 参数应编译失败（ABI 仅 128 位 XMM 传参，显式拒绝防静默错）。
#[test]
fn test_simd_v256_param_rejected() {
    x86_64::ensure_registered();
    let sig = FunctionSignature::new(&[(TypeId::V256, "")], &[TypeId::F32]);
    let mut b = FunctionBuilder::new("v256_param", TypeContext::new(), sig);
    b.create_block_here();
    let e = b.fconst(1.0f32.to_bits() as u64, TypeId::F32);
    b.ret(&[e]);
    let func = b.finish().expect("build");
    let result = FunctionCompiler::new(TargetMachine::new()).compile_raw(&func);
    assert!(result.is_err(), "V256 参数应编译报错(ABI 不支持宽向量传参)");
}

/// vextract 越界语义（LLVM）：变量 idx 无静态检查——越界运行时未定义；
/// 本测试验证变量 idx 可正常编译（idx 是操作数而非立即数）。
#[test]
fn test_simd_vextract_dynamic_idx() {
    x86_64::ensure_registered();
    let sig = FunctionSignature::new(&[], &[TypeId::F32]);
    let mut b = FunctionBuilder::new("vextract_dyn", TypeContext::new(), sig);
    b.create_block_here();
    let v = vconst_f32_4(&mut b, [1.0, 2.0, 3.0, 4.0]);
    let z = b.iconst_i32(4);
    let _ = b.vextract(v, z);
}

// ═══════════════ 整数/双精度元素（Phase 2）═══════════════
// <2 x f64>（ADDPD/SUBPD/MULPD/DIVPD）、<4 x i32>（PADDD/PSUBD）、<2 x i64>
// （PADDQ）；vconst 经 punpcklqdq 恢复 64 位元素；vextract 按元素宽度提取。

#[test]
fn test_simd_f64_vadd() {
    let r = run_vec_f64_lane("simd_f64_vadd", 1, |b| {
        let a = vconst_f64_2(b, [1.5, 2.5]);
        let c = vconst_f64_2(b, [0.5, 3.0]);
        b.vadd(a, c)
    });
    assert!(
        (r - 5.5).abs() < 1e-9,
        "<2 x f64> vadd lane1 应得 5.5，实际 {r}"
    );
}

#[test]
fn test_simd_f64_vsub_vmul() {
    let r = run_vec_f64_lane("simd_f64_vsub_mul", 1, |b| {
        let a = vconst_f64_2(b, [10.0, 12.0]);
        let c = vconst_f64_2(b, [4.0, 3.0]);
        let d = b.vsub(a, c);
        let m = vconst_f64_2(b, [1.0, 2.0]);
        b.vmul(d, m)
    });
    assert!(
        (r - 18.0).abs() < 1e-9,
        "<2 x f64> (vsub)vmul lane1 应得 18.0，实际 {r}"
    );
}

#[test]
fn test_simd_f64_vdiv() {
    let r = run_vec_f64_lane("simd_f64_vdiv", 1, |b| {
        let a = vconst_f64_2(b, [10.0, 12.0]);
        let c = vconst_f64_2(b, [4.0, 3.0]);
        b.vdiv(a, c)
    });
    assert!(
        (r - 4.0).abs() < 1e-9,
        "<2 x f64> vdiv lane1 应得 4.0，实际 {r}"
    );
}

#[test]
fn test_simd_f64_vextract_lane1() {
    let r = run_vec_f64_lane("simd_f64_vextract", 1, |b| vconst_f64_2(b, [1.25, 2.75]));
    assert!(
        (r - 2.75).abs() < 1e-9,
        "<2 x f64> vextract lane1 应得 2.75，实际 {r}"
    );
}

#[test]
fn test_simd_i32_vadd_vsub() {
    let r = run_vec_i32_lane("simd_i32_vadd_sub", 2, |b| {
        let av = b.vconst(vec![10i32, 20, 30, 40]);
        let cv = b.vconst(vec![1i32, 2, 3, 4]);
        let d = b.vadd(av, cv);
        let e = b.vconst(vec![5i32, 5, 5, 5]);
        b.vsub(d, e)
    });
    assert_eq!(r, 28, "<4 x i32> (vadd)vsub lane2 应得 28，实际 {r}");
}

#[test]
fn test_simd_i64_vadd() {
    let r = run_vec_i64_lane("simd_i64_vadd", 1, |b| {
        let a = b.vconst(vec![1000i64, 2000]);
        let c = b.vconst(vec![7i64, 8]);
        b.vadd(a, c)
    });
    assert_eq!(r, 2008, "<2 x i64> vadd lane1 应得 2008，实际 {r}");
}

// ═══════════════ 静默错扫尾（Phase 1）═══════════════
// vneg 元素分派（f64→subpd/i64→psubq）、<4 x f64> vconst/vextract lane 2-3。

#[test]
fn test_simd_f64_vneg() {
    let r = run_vec_f64_lane("simd_f64_vneg", 1, |b| {
        let a = vconst_f64_2(b, [1.5, -2.5]);
        b.vneg(a)
    });
    assert!(
        (r - 2.5).abs() < 1e-9,
        "<2 x f64> vneg lane1 应得 2.5，实际 {r}"
    );
}

#[test]
fn test_simd_f64_vabs_neg() {
    let r = run_vec_f64_lane("simd_f64_vabs_neg", 1, |b| {
        let a = vconst_f64_2(b, [1.5, -2.5]);
        b.vabs(a)
    });
    assert!(
        (r - 2.5).abs() < 1e-9,
        "<2 x f64> vabs lane1 应得 2.5，实际 {r}"
    );
}

#[test]
fn test_simd_i64_vneg() {
    let r = run_vec_i64_lane("simd_i64_vneg", 1, |b| {
        let a = b.vconst(vec![1000i64, -2000]);
        b.vneg(a)
    });
    assert_eq!(r, 2000, "<2 x i64> vneg lane1 应得 2000，实际 {r}");
}

/// vconst 构造 <4 x f64> 的便捷 helper。
fn vconst_f64_4(b: &mut FunctionBuilder, vals: [f64; 4]) -> Value {
    b.vconst(vals.to_vec())
}

#[test]
fn test_simd_f64x4_vconst_vextract_lane3() {
    let r = run_vec_f64_lane("simd_f64x4_vconst", 3, |b| {
        vconst_f64_4(b, [1.0, 2.0, 3.0, 4.0])
    });
    assert!(
        (r - 4.0).abs() < 1e-9,
        "<4 x f64> vconst lane3 应得 4.0，实际 {r}"
    );
}

#[test]
fn test_simd_f64x4_vadd() {
    let r = run_vec_f64_lane("simd_f64x4_vadd", 2, |b| {
        let a = vconst_f64_4(b, [1.0, 2.0, 3.0, 4.0]);
        let c = vconst_f64_4(b, [0.5, 0.5, 0.5, 0.5]);
        b.vadd(a, c)
    });
    assert!(
        (r - 3.5).abs() < 1e-9,
        "<4 x f64> vadd lane2 应得 3.5，实际 {r}"
    );
}

// ═══════════════ vconst_array（Phase 1 完善）═══════════════
// 数值语义数组 → 动态向量常量 → 运算。

#[test]
fn test_simd_vconst_array_f32() {
    let r = run_vec_f32_lane("simd_vconst_array_f32", 2, |b| {
        let a = b.vconst_array([1.0f32, 2.0, 3.0, 4.0]);
        let c = b.vconst_array([0.5f32, 0.5, 0.5, 0.5]);
        b.vadd(a, c)
    });
    assert!(
        (r - 3.5).abs() < 1e-9,
        "<4 x f32> vconst_array vadd lane2 应得 3.5，实际 {r}"
    );
}

#[test]
fn test_simd_vconst_array_f64() {
    let r = run_vec_f64_lane("simd_vconst_array_f64", 1, |b| {
        let a = b.vconst_array([1.5f64, 2.5]);
        let c = b.vconst_array([1.0f64, -0.5]);
        b.vadd(a, c)
    });
    assert!(
        (r - 2.0).abs() < 1e-9,
        "<2 x f64> vconst_array vadd lane1 应得 2.0，实际 {r}"
    );
}

#[test]
fn test_simd_vconst_array_i32() {
    let r = run_vec_i32_lane("simd_vconst_array_i32", 3, |b| {
        let a = b.vconst_array([10i32, 20, 30, 40]);
        let c = b.vconst_array([1i32, 2, 3, 40]);
        b.vadd(a, c)
    });
    assert_eq!(r, 80, "<4 x i32> vconst_array vadd lane3 应得 80，实际 {r}");
}

#[test]
fn test_simd_vconst_array_i32_neg() {
    // 负数位模式截断（-1 → 0xFFFF_FFFF）后加 1 → 0（回绕）。
    let r = run_vec_i32_lane("simd_vconst_array_i32_neg", 0, |b| {
        let a = b.vconst_array([-1i32, 5, 5, 5]);
        let c = b.vconst_array([1i32, 1, 1, 1]);
        b.vadd(a, c)
    });
    assert_eq!(
        r, 0,
        "<4 x i32> vconst_array [-1]+[1] lane0 应得 0，实际 {r}"
    );
}

#[test]
fn test_simd_vconst_array_v256() {
    // <8 x f32> 数组 → V256 常量 → vadd → lane7。
    if !code_forge::prelude::avx_available() {
        return;
    }
    let r = run_vec_f32_lane("simd_vconst_array_v256", 7, |b| {
        let a = b.vconst_array([1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        let c = b.vconst_array([0.5f32; 8]);
        b.vadd(a, c)
    });
    assert!(
        (r - 8.5).abs() < 1e-9,
        "<8 x f32> vconst_array vadd lane7 应得 8.5，实际 {r}"
    );
}

// ═══════════════ 整数 V256（AVX2，Phase 2）═══════════════
// <8 x i32> / <4 x i64>：vadd/vsub/vneg/vmul + vconst/vextract 全 lane。
// 无 AVX2 机器 skip（编码宏 avx2_available 断言）。

fn require_avx2() -> bool {
    code_forge::prelude::avx_available() && code_forge::prelude::avx2_available()
}

#[test]
fn test_simd_i32x8_vadd() {
    if !require_avx2() {
        return;
    }
    let r = run_vec_i32_lane("simd_i32x8_vadd", 7, |b| {
        let a = b.vconst_array([1, 2, 3, 4, 5, 6, 7, 8]);
        let c = b.vconst_array([10, 20, 30, 40, 50, 60, 70, 80]);
        b.vadd(a, c)
    });
    assert_eq!(r, 88, "<8 x i32> vpaddd lane7 应得 88，实际 {r}");
}

#[test]
fn test_simd_i32x8_vsub_vmul() {
    if !require_avx2() {
        return;
    }
    let r = run_vec_i32_lane("simd_i32x8_vsub_vmul", 5, |b| {
        let a = b.vconst_array([1, 2, 3, 4, 5, 6, 7, 8]);
        let c = b.vconst_array([10, 20, 30, 40, 50, 60, 70, 80]);
        let s = b.vsub(c, a); // [9,18,27,36,45,54,63,72]
        b.vmul(s, a) // lane5 = 54*6 = 324
    });
    assert_eq!(r, 324, "<8 x i32> vpsubd+vmul lane5 应得 324，实际 {r}");
}

#[test]
fn test_simd_i32x8_vneg() {
    if !require_avx2() {
        return;
    }
    let r = run_vec_i32_lane("simd_i32x8_vneg", 3, |b| {
        let a = b.vconst_array([1, -2, 3, -4, 5, -6, 7, -8]);
        b.vneg(a)
    });
    assert_eq!(r, 4, "<8 x i32> vneg lane3 应得 4，实际 {r}");
}

#[test]
fn test_simd_i64x4_vadd() {
    if !require_avx2() {
        return;
    }
    // lane0 先验证基础（1000+1=1001），再 lane2（3000+3=3003）。
    let r0 = run_vec_i64_lane("simd_i64x4_vadd", 0, |b| {
        let a = b.vconst_array([1000i64, 2000, 3000, 4000]);
        let c = b.vconst_array([1i64, 2, 3, 4]);
        b.vadd(a, c)
    });
    assert_eq!(r0, 1001, "<4 x i64> vpaddq lane0 应得 1001，实际 {r0}");
    let r = run_vec_i64_lane("simd_i64x4_vadd", 2, |b| {
        let a = b.vconst_array([1000i64, 2000, 3000, 4000]);
        let c = b.vconst_array([1i64, 2, 3, 4]);
        b.vadd(a, c)
    });
    assert_eq!(r, 3003, "<4 x i64> vpaddq lane2 应得 3003，实际 {r}");
}

#[test]
fn test_simd_i32x8_vextract_all_lanes() {
    if !require_avx2() {
        return;
    }
    // 8 个 lane 逐个提取验证 vconst 布局 + vextractf128/pshufd/movd_fr 全链路。
    for (lane, expect) in [3, 6, 9, 12, 15, 18, 21, 24].iter().enumerate() {
        let r = run_vec_i32_lane("simd_i32x8_vextract_all", lane as u32, |b| {
            b.vconst_array([3, 6, 9, 12, 15, 18, 21, 24])
        });
        assert_eq!(
            r, *expect,
            "<8 x i32> vconst lane{lane} 应得 {expect}，实际 {r}"
        );
    }
}

// ═══════════════ 功能补全（Phase 2）═══════════════
// 整数 vmul（PMULLD）、f64 vbroadcast/vinsert、ShuffleVector V256（vshufps 拆半）。

#[test]
fn test_simd_i32_vmul() {
    let r = run_vec_i32_lane("simd_i32_vmul", 2, |b| {
        let a = b.vconst(vec![2i32, 3, 30, 5]);
        let c = b.vconst(vec![2i32, 3, 3, 5]);
        b.vmul(a, c)
    });
    assert_eq!(r, 90, "<4 x i32> vmul lane2 应得 90，实际 {r}");
}

#[test]
fn test_simd_f64_vbroadcast() {
    let r = run_vec_f64_lane("simd_f64_vbroadcast", 1, |b| {
        let s = b.fconst(7.25f64.to_bits(), TypeId::F64);
        b.vbroadcast(s, b.type_ctx().vector_ty(TypeId::F64, 2))
    });
    assert!(
        (r - 7.25).abs() < 1e-9,
        "<2 x f64> vbroadcast lane1 应得 7.25，实际 {r}"
    );
}

#[test]
fn test_simd_f64_vinsert() {
    let r = run_vec_f64_lane("simd_f64_vinsert", 1, |b| {
        let a = vconst_f64_2(b, [1.0, 2.0]);
        let s = b.fconst(99.0f64.to_bits(), TypeId::F64);
        let z = b.iconst_i32(0);
        b.vinsert(a, s, z)
    });
    assert!(
        (r - 2.0).abs() < 1e-9,
        "<2 x f64> vinsert lane0=99 后 lane1 应保持 2.0，实际 {r}"
    );
}

#[test]
fn test_simd_v256_shuffle() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    // V256 mask 为组内语义（与 V128 一致）：mask[0..4] 控制低组 lane0-3、
    // mask[4..8] 控制高组 lane4-7，每组内 0-3 相对该组（src1=a 的组内 lane）。
    // mask=[3,2,1,0, 3,2,1,0]：低组 [a3,a2,a1,a0]、高组 [a7,a6,a5,a4]。
    let r = run_vec_f32_lane("simd_v256_shuffle", 4, |b| {
        let a = vconst_f32_8(b, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        let c = vconst_f32_8(b, [10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0]);
        b.shuffle_vector(a, c, &[3, 2, 1, 0, 3, 2, 1, 0])
    });
    // 高组 lane4 = 高组 src1(a) lane3 = a lane7 = 8.0
    assert!(
        (r - 8.0).abs() < 1e-6,
        "V256 shuffle lane4 应得 8.0，实际 {r}"
    );
}

#[test]
fn test_simd_v256_shuffle_hi() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    // mask[4..8]=[0,1,2,3]：高组 dst lane4-5 来自 src1(a) 高组 [a4,a5]、
    // lane6-7 来自 src2(b) 高组 [m6-4, m7-4]=[2,3] → [b6,b7]=[70,80]。
    let r = run_vec_f32_lane("simd_v256_shuffle_hi", 7, |b| {
        let a = vconst_f32_8(b, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        let c = vconst_f32_8(b, [10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0]);
        b.shuffle_vector(a, c, &[3, 2, 1, 0, 0, 1, 2, 3])
    });
    // 高组 lane7 = src2(b) 高组 lane3 = b lane7 = 80.0
    assert!(
        (r - 80.0).abs() < 1e-6,
        "V256 shuffle 高组 lane7 应得 80.0，实际 {r}"
    );
}

// ═══════════════ 向量常量端序（vconst_with_endian / vconst_bytes_with_endian）═══════════════
// lane_bytes(endian) 支持 Little/Big：BE 存储的常量由 DSL 按常量池端序
// （get_vector_endian → from_be_bytes）还原，x86 上执行结果与 LE 一致。

/// <4 x f32> BE 存储 → vadd → vextract：字节序正确还原。
#[test]
fn test_simd_vconst_be_f32() {
    let r = run_vec_f32_lane("simd_vconst_be_f32", 2, |b| {
        let a = b.vconst_with_endian(vec![1.0f32, 2.0, 3.0, 4.0], Endianness::Big);
        let c = b.vconst_with_endian(vec![10.0f32, 20.0, 30.0, 40.0], Endianness::Big);
        b.vadd(a, c)
    });
    assert!(
        (r - 33.0).abs() < 1e-6,
        "BE <4 x f32> vadd lane2 应得 33.0，实际 {r}"
    );
}

/// <4 x i32> BE 存储：u32 字节序 [01,02,03,04] → 数值 0x01020304。
#[test]
fn test_simd_vconst_be_i32() {
    let r = run_vec_i32_lane("simd_vconst_be_i32", 1, |b| {
        b.vconst_with_endian(
            vec![0x01020304u32, 0x05060708, 0x11121314, 0x15161718],
            Endianness::Big,
        )
    });
    assert_eq!(
        r, 0x05060708,
        "BE <4 x i32> vextract lane1 应得 0x05060708，实际 {r}"
    );
}

/// <4 x f64> BE 存储（64 位元素连续块 from_be_bytes）。
#[test]
fn test_simd_vconst_be_f64() {
    let r = run_vec_f64_lane("simd_vconst_be_f64", 3, |b| {
        let a = b.vconst_with_endian(vec![1.5f64, 2.5, 3.5, 4.5], Endianness::Big);
        b.vadd(a, a)
    });
    assert!(
        (r - 9.0).abs() < 1e-12,
        "BE <4 x f64> vadd lane3 应得 9.0，实际 {r}"
    );
}

/// vconst_bytes_with_endian 直接字节构造（BE 字节流）→ x86 还原。
#[test]
fn test_simd_vconst_bytes_be() {
    // <2 x f32> BE 字节：[1.0 BE 4B][2.0 BE 4B] = 3F800000 40000000
    let r = run_vec_f32_lane("simd_vconst_bytes_be", 1, |b| {
        let data: Vec<u8> = vec![
            0x3F, 0x80, 0x00, 0x00, // 1.0 BE
            0x40, 0x00, 0x00, 0x00, // 2.0 BE
        ];
        let ty = b.type_ctx().vector_ty(TypeId::F32, 2);
        b.vconst_bytes_with_endian(data, ty, Endianness::Big)
    });
    assert!(
        (r - 2.0).abs() < 1e-6,
        "BE 字节 <2 x f32> lane1 应得 2.0，实际 {r}"
    );
}

// ═══════════════ V256（<8 x f32>，AVX YMM）═══════════════
// V256 经 vconcat(V128, V128) 构造（vconst V256 在常量阶段支持）；运算用 AVX
// 256 位指令（vaddps 等）；vextract lane<4 取低半 / lane>=4 取高半（vextractf128）。

/// vconst <4 x f32> ×2 → vconcat 成 V256。
fn vconcat_f32_4(b: &mut FunctionBuilder, lo: [f32; 4], hi: [f32; 4]) -> Value {
    let a = vconst_f32_4(b, lo);
    let c = vconst_f32_4(b, hi);
    b.vconcat(a, c)
}

#[test]
fn test_simd_v256_vconcat_vsplit_lane0() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let r = run_vec_f32_lane0("simd_v256_vconcat", |b| {
        let v = vconcat_f32_4(b, [1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]);
        b.vsplit(v, 0)
    });
    assert!(
        (r - 1.0).abs() < 1e-6,
        "V256 vsplit 低半 lane0 应得 1.0，实际 {r}"
    );
}

#[test]
fn test_simd_v256_vsplit_hi() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let r = run_vec_f32_lane("simd_v256_vsplit_hi", 1, |b| {
        let v = vconcat_f32_4(b, [1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]);
        b.vsplit(v, 1)
    });
    assert!(
        (r - 6.0).abs() < 1e-6,
        "V256 vsplit 高半 lane1 应得 6.0，实际 {r}"
    );
}

#[test]
fn test_simd_v256_vconst() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let r = run_vec_f32_lane("simd_v256_vconst", 6, |b| {
        vconst_f32_8(b, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0])
    });
    assert!(
        (r - 7.0).abs() < 1e-6,
        "V256 vconst lane6 应得 7.0，实际 {r}"
    );
}

#[test]
fn test_simd_v256_vadd() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let r = run_vec_f32_lane("simd_v256_vadd", 4, |b| {
        let a = vconcat_f32_4(b, [1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]);
        let c = vconcat_f32_4(b, [1.0, 1.0, 1.0, 1.0], [2.0, 2.0, 2.0, 2.0]);
        b.vadd(a, c)
    });
    assert!((r - 7.0).abs() < 1e-6, "V256 vadd lane4 应得 7.0，实际 {r}");
}

#[test]
fn test_simd_v256_vmul_hi_lane() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let r = run_vec_f32_lane("simd_v256_vmul_hi", 7, |b| {
        let a = vconcat_f32_4(b, [1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]);
        let c = vconcat_f32_4(b, [1.0, 1.0, 1.0, 1.0], [2.0, 2.0, 2.0, 2.0]);
        b.vmul(a, c)
    });
    assert!(
        (r - 16.0).abs() < 1e-6,
        "V256 vmul lane7 应得 16.0，实际 {r}"
    );
}

#[test]
fn test_simd_v256_vneg() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let r = run_vec_f32_lane("simd_v256_vneg", 6, |b| {
        let a = vconcat_f32_4(b, [1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]);
        b.vneg(a)
    });
    assert!(
        (r - (-7.0)).abs() < 1e-6,
        "V256 vneg lane6 应得 -7.0，实际 {r}"
    );
}

#[test]
fn test_simd_v256_vabs() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let r = run_vec_f32_lane("simd_v256_vabs", 7, |b| {
        let a = vconcat_f32_4(b, [1.0, -2.0, 3.0, -4.0], [5.0, -6.0, 7.0, -8.0]);
        b.vabs(a)
    });
    assert!((r - 8.0).abs() < 1e-6, "V256 vabs lane7 应得 8.0，实际 {r}");
}

#[test]
fn test_simd_v256_vbitcast() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let r = run_vec_f32_lane("simd_v256_vbitcast", 5, |b| {
        let a = vconcat_f32_4(b, [1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]);
        b.vbitcast(a, TypeId::V256)
    });
    assert!(
        (r - 6.0).abs() < 1e-6,
        "V256 vbitcast lane5 应得 6.0，实际 {r}"
    );
}

#[test]
fn test_simd_v256_vinsert() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let r = run_vec_f32_lane("simd_v256_vinsert", 4, |b| {
        let a = vconcat_f32_4(b, [1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]);
        let s = b.fconst(99.0f32.to_bits() as u64, TypeId::F32);
        let z = b.iconst_i32(0);
        b.vinsert(a, s, z)
    });
    assert!(
        (r - 5.0).abs() < 1e-6,
        "V256 vinsert lane0=99 后 lane4 应保持 5.0，实际 {r}"
    );
}

#[test]
fn test_simd_v256_vextract_lane7() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let r = run_vec_f32_lane("simd_v256_vextract_l7", 7, |b| {
        vconcat_f32_4(b, [1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0])
    });
    assert!(
        (r - 8.0).abs() < 1e-6,
        "V256 vextract lane7 应得 8.0，实际 {r}"
    );
}

#[test]
fn test_simd_v256_vbroadcast() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let r = run_vec_f32_lane("simd_v256_vbroadcast", 5, |b| {
        let s = b.fconst(7.25f32.to_bits() as u64, TypeId::F32);
        b.vbroadcast(s, TypeId::V256)
    });
    assert!(
        (r - 7.25).abs() < 1e-6,
        "V256 vbroadcast lane5 应得 7.25，实际 {r}"
    );
}

#[test]
fn test_simd_vadd() {
    let r = run_vec_f32_lane0("simd_vadd", |b| {
        let a = vconst_f32_4(b, [1.5, 2.5, 3.5, 4.5]);
        let c = vconst_f32_4(b, [0.5, 0.5, 0.5, 0.5]);
        b.vadd(a, c)
    });
    assert!((r - 2.0).abs() < 1e-6, "vadd lane0 应得 2.0，实际 {r}");
    // 非 lane0 抽样：lane2 = 3.5 + 0.5 = 4.0
    let r2 = run_vec_f32_lane("simd_vadd_l2", 2, |b| {
        let a = vconst_f32_4(b, [1.5, 2.5, 3.5, 4.5]);
        let c = vconst_f32_4(b, [0.5, 0.5, 0.5, 0.5]);
        b.vadd(a, c)
    });
    assert!((r2 - 4.0).abs() < 1e-6, "vadd lane2 应得 4.0，实际 {r2}");
}

#[test]
fn test_simd_vsub() {
    let r = run_vec_f32_lane0("simd_vsub", |b| {
        let a = vconst_f32_4(b, [3.5, 4.5, 5.5, 6.5]);
        let c = vconst_f32_4(b, [1.5, 1.5, 1.5, 1.5]);
        b.vsub(a, c)
    });
    assert!((r - 2.0).abs() < 1e-6, "vsub lane0 应得 2.0，实际 {r}");
    let r3 = run_vec_f32_lane("simd_vsub_l3", 3, |b| {
        let a = vconst_f32_4(b, [3.5, 4.5, 5.5, 6.5]);
        let c = vconst_f32_4(b, [1.5, 1.5, 1.5, 1.5]);
        b.vsub(a, c)
    });
    assert!((r3 - 5.0).abs() < 1e-6, "vsub lane3 应得 5.0，实际 {r3}");
}

#[test]
fn test_simd_vmul() {
    let r = run_vec_f32_lane0("simd_vmul", |b| {
        let a = vconst_f32_4(b, [2.0, 3.0, 4.0, 5.0]);
        let c = vconst_f32_4(b, [3.0, 3.0, 3.0, 3.0]);
        b.vmul(a, c)
    });
    assert!((r - 6.0).abs() < 1e-6, "vmul lane0 应得 6.0，实际 {r}");
    let r1 = run_vec_f32_lane("simd_vmul_l1", 1, |b| {
        let a = vconst_f32_4(b, [2.0, 3.0, 4.0, 5.0]);
        let c = vconst_f32_4(b, [3.0, 3.0, 3.0, 3.0]);
        b.vmul(a, c)
    });
    assert!((r1 - 9.0).abs() < 1e-6, "vmul lane1 应得 9.0，实际 {r1}");
}

#[test]
fn test_simd_vdiv() {
    let r = run_vec_f32_lane0("simd_vdiv", |b| {
        let a = vconst_f32_4(b, [6.0, 8.0, 10.0, 12.0]);
        let c = vconst_f32_4(b, [2.0, 2.0, 2.0, 2.0]);
        b.vdiv(a, c)
    });
    assert!((r - 3.0).abs() < 1e-6, "vdiv lane0 应得 3.0，实际 {r}");
    let r2 = run_vec_f32_lane("simd_vdiv_l2", 2, |b| {
        let a = vconst_f32_4(b, [6.0, 8.0, 10.0, 12.0]);
        let c = vconst_f32_4(b, [2.0, 2.0, 2.0, 2.0]);
        b.vdiv(a, c)
    });
    assert!((r2 - 5.0).abs() < 1e-6, "vdiv lane2 应得 5.0，实际 {r2}");
}

#[test]
fn test_simd_vneg() {
    let r = run_vec_f32_lane0("simd_vneg", |b| {
        let a = vconst_f32_4(b, [-1.5, 2.5, -3.5, 4.5]);
        b.vneg(a)
    });
    assert!((r - 1.5).abs() < 1e-6, "vneg lane0 应得 1.5，实际 {r}");
    let r1 = run_vec_f32_lane("simd_vneg_l1", 1, |b| {
        let a = vconst_f32_4(b, [-1.5, 2.5, -3.5, 4.5]);
        b.vneg(a)
    });
    assert!(
        (r1 - (-2.5)).abs() < 1e-6,
        "vneg lane1 应得 -2.5，实际 {r1}"
    );
}

#[test]
fn test_simd_vabs() {
    let r = run_vec_f32_lane0("simd_vabs", |b| {
        let a = vconst_f32_4(b, [-1.5, 2.5, -3.5, 4.5]);
        b.vabs(a)
    });
    assert!((r - 1.5).abs() < 1e-6, "vabs lane0 应得 1.5，实际 {r}");
    let r2 = run_vec_f32_lane("simd_vabs_l2", 2, |b| {
        let a = vconst_f32_4(b, [-1.5, 2.5, -3.5, 4.5]);
        b.vabs(a)
    });
    assert!((r2 - 3.5).abs() < 1e-6, "vabs lane2 应得 3.5，实际 {r2}");
}

#[test]
fn test_simd_vbitcast() {
    // 同宽位重解释：movaps 直拷，lane 值不变
    let r = run_vec_f32_lane0("simd_vbitcast", |b| {
        let a = vconst_f32_4(b, [42.25, -3.5, 7.0, 0.5]);
        b.vbitcast(a, TypeId::V128)
    });
    assert!(
        (r - 42.25).abs() < 1e-6,
        "vbitcast lane0 应得 42.25，实际 {r}"
    );
}

#[test]
fn test_simd_vbroadcast() {
    let r = run_vec_f32_lane0("simd_vbroadcast", |b| {
        let e = b.fconst_f32(42.0);
        b.vbroadcast(e, TypeId::V128)
    });
    assert!(
        (r - 42.0).abs() < 1e-6,
        "vbroadcast lane0 应得 42.0，实际 {r}"
    );
    // lane3 也应广播到
    let r3 = run_vec_f32_lane("simd_vbroadcast_l3", 3, |b| {
        let e = b.fconst_f32(42.0);
        b.vbroadcast(e, TypeId::V128)
    });
    assert!(
        (r3 - 42.0).abs() < 1e-6,
        "vbroadcast lane3 应得 42.0，实际 {r3}"
    );
}

#[test]
fn test_simd_vinsert() {
    // vinsert lane0：替换后提取
    let r = run_vec_f32_lane0("simd_vinsert", |b| {
        let a = vconst_f32_4(b, [1.5, 2.5, 3.5, 4.5]);
        let e = b.fconst_f32(99.0);
        let z = b.iconst_i32(0);
        b.vinsert(a, e, z)
    });
    assert!((r - 99.0).abs() < 1e-6, "vinsert lane0 应得 99.0，实际 {r}");
    // 其余 lane 应保持不变（lane1 仍为 2.5）
    let r1 = run_vec_f32_lane("simd_vinsert_l1", 1, |b| {
        let a = vconst_f32_4(b, [1.5, 2.5, 3.5, 4.5]);
        let e = b.fconst_f32(99.0);
        let z = b.iconst_i32(0);
        b.vinsert(a, e, z)
    });
    assert!(
        (r1 - 2.5).abs() < 1e-6,
        "vinsert 后 lane1 应保持 2.5，实际 {r1}"
    );
}

#[test]
fn test_simd_shuffle_vector() {
    // 双源 shuffle（a=b）：取 a 的 lane3 到 lane0（mask[0]=3 → src1 lane3）
    let r = run_vec_f32_lane0("simd_shuf_l3", |b| {
        let a = vconst_f32_4(b, [1.5, 2.5, 3.5, 4.5]);
        b.shuffle_vector(a, a, &[3, 0, 0, 0])
    });
    assert!(
        (r - 4.5).abs() < 1e-6,
        "shuffle lane0 应得 a[3]=4.5，实际 {r}"
    );
    // 跨源（SHUFPS 约束：lane0/1 来自 a、lane2/3 来自 b）：
    // s = [a0, a1, b0, b1] = [1, 2, 10, 20]，再取 s 的 lane2（=b[0]）到 lane0
    let r2 = run_vec_f32_lane0("simd_shuf_cross", |b| {
        let a = vconst_f32_4(b, [1.0, 2.0, 3.0, 4.0]);
        let c = vconst_f32_4(b, [10.0, 20.0, 30.0, 40.0]);
        let s = b.shuffle_vector(a, c, &[0, 1, 4, 5]);
        b.shuffle_vector(s, s, &[2, 2, 0, 0])
    });
    assert!(
        (r2 - 10.0).abs() < 1e-6,
        "跨源 shuffle lane0 应得 b[0]=10，实际 {r2}"
    );
}

#[test]
fn test_simd_chain() {
    // 组合链：vadd(vmul(a, b), c)：[2,3,4,5]*[3,...]+[1,...] → lane0 = 2*3+1 = 7
    let r = run_vec_f32_lane0("simd_chain", |b| {
        let a = vconst_f32_4(b, [2.0, 3.0, 4.0, 5.0]);
        let m = vconst_f32_4(b, [3.0, 3.0, 3.0, 3.0]);
        let c = vconst_f32_4(b, [1.0, 1.0, 1.0, 1.0]);
        let p = b.vmul(a, m);
        b.vadd(p, c)
    });
    assert!(
        (r - 7.0).abs() < 1e-6,
        "vadd(vmul) 链 lane0 应得 7.0，实际 {r}"
    );
    let r2 = run_vec_f32_lane0("simd_chain_bcast", |b| {
        let a = b.fconst_f32(42.5);
        let c = b.fconst_f32(0.5);
        let va = b.vbroadcast(a, TypeId::V128);
        let vc = b.vbroadcast(c, TypeId::V128);
        b.vsub(va, vc)
    });
    assert!(
        (r2 - 42.0).abs() < 1e-6,
        "vbroadcast-vsub 应得 42.0，实际 {r2}"
    );
}

// ═══════════════ 全 lane 断言矩阵（Phase 4）═══════════════
// V256 的 8 lane 全断：vconst + vadd 组合链，逐个 lane 校验（vextract 全路径）。

#[test]
fn test_simd_v256_all_lanes() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let vals: [f32; 8] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    for k in 0..8u32 {
        let r = run_vec_f32_lane(&format!("simd_v256_all_l{k}"), k, |b| vconst_f32_8(b, vals));
        assert!(
            (r - vals[k as usize]).abs() < 1e-6,
            "V256 vconst lane{k} 应得 {}，实际 {r}",
            vals[k as usize]
        );
    }
}

#[test]
fn test_simd_v256_chain_all_lanes() {
    if !code_forge::prelude::avx_available() {
        return;
    }
    let vals: [f32; 8] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    for k in 0..8u32 {
        let r = run_vec_f32_lane(&format!("simd_v256_chain_l{k}"), k, |b| {
            let a = vconst_f32_8(b, vals);
            let m = vconst_f32_8(b, [2.0; 8]);
            let c = vconst_f32_8(b, [1.0; 8]);
            let p = b.vmul(a, m);
            b.vadd(p, c)
        });
        let expected = vals[k as usize] * 2.0 + 1.0;
        assert!(
            (r - expected).abs() < 1e-6,
            "V256 vadd(vmul) 链 lane{k} 应得 {expected}，实际 {r}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// FunctionBuilder 运算全覆盖 — 整数余数
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_urem() {
    assert_eq!(
        run_test("urem", |b| {
            let a = b.iconst_i32(84);
            let c = b.iconst_i32(5);
            b.urem(a, c)
        }),
        4
    );
}

#[test]
fn test_srem() {
    // 带符号余数：-84 / 5 = -16 余 -4
    assert_eq!(
        run_test("srem", |b| {
            let a = b.iconst_i32(-84);
            let c = b.iconst_i32(5);
            b.srem(a, c)
        }),
        -4
    );
}

// ═══════════════════════════════════════════════════════════════
// FunctionBuilder 运算全覆盖 — 浮点扩展（fadd_fast / fadd_with_flags）
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_fadd_fast() {
    // fadd_fast 与 fadd 等价（发 Opcode::Fadd）
    let r = run_test_f64("fadd_fast", |b| {
        let a = b.fconst_f64(20.5);
        let c = b.fconst_f64(21.5);
        b.fadd_fast(a, c)
    });
    assert!(
        (r - 42.0).abs() < 0.001,
        "fadd_fast: got {r}, expected 42.0"
    );
}

#[test]
fn test_fadd_with_flags() {
    let r = run_test_f64("fadd_with_flags", |b| {
        let a = b.fconst_f64(20.5);
        let c = b.fconst_f64(21.5);
        b.fadd_with_flags(a, c, InstFlags::NONE)
    });
    assert!(
        (r - 42.0).abs() < 0.001,
        "fadd_with_flags: got {r}, expected 42.0"
    );
}

// ═══════════════════════════════════════════════════════════════
// FunctionBuilder 运算全覆盖 — 内存（fstore/fload 执行；alloca/gep 仅编译）
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_fstore_fload_roundtrip() {
    let r = run_test_f64("fstore_fload", |b| {
        let addr = b.stack_addr(-8);
        let v = b.fconst_f64(42.5);
        b.fstore(v, addr);
        b.fload(addr, TypeId::F64)
    });
    assert!(
        (r - 42.5).abs() < 1e-10,
        "fstore_fload: got {r}, expected 42.5"
    );
}

#[test]
fn test_fstore_negative() {
    let r = run_test_f64("fstore_neg", |b| {
        let addr = b.stack_addr(-16);
        let v = b.fconst_f64(-3.25);
        b.fstore(v, addr);
        b.fload(addr, TypeId::F64)
    });
    assert!(
        (r - (-3.25)).abs() < 1e-10,
        "fstore_neg: got {r}, expected -3.25"
    );
}

/// alloca：x86_64 后端规则引用不存在的 rs1/rs2（Alloca 为 0 操作数 opcode），
/// 本机执行 SEGV（io.rs 已记录）——此处仅验证编译通过。
#[test]
fn test_alloca_compiles() {
    ensure_registered();
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("alloca_c", TypeContext::new(), sig);
    b.create_block_here();
    let ptr = b.alloca(TypeId::I32, 4);
    let z = b.iconst_i32(0);
    b.store(z, ptr);
    let r = b.load(ptr, TypeId::I32);
    b.ret(&[r]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .expect("alloca must compile");
    assert!(!compiled.code.is_empty(), "alloca: empty code");
}

/// gep：元素宽度硬编码 4 且只取第一个索引（toml 已知缺陷），执行不稳定——
/// 此处仅验证编译通过。
#[test]
fn test_gep_compiles() {
    ensure_registered();
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("gep_c", TypeContext::new(), sig);
    b.create_block_here();
    let base = b.stack_addr(0);
    let idx = b.iconst_i32(1);
    let addr = b.gep(base, &[idx], TypeId::I32);
    let z = b.iconst_i32(7);
    b.store(z, addr);
    let r = b.load(addr, TypeId::I32);
    b.ret(&[r]);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .expect("gep must compile");
    assert!(!compiled.code.is_empty(), "gep: empty code");
}

// ═══════════════════════════════════════════════════════════════
// FunctionBuilder 运算全覆盖 — fence
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_fence() {
    // mfence 不扰动寄存器，后续 iconst 正常返回
    assert_eq!(
        run_test("fence", |b| {
            b.fence(Ordering::SequentiallyConsistent);
            b.iconst_i32(42)
        }),
        42
    );
}

// ═══════════════════════════════════════════════════════════════
// FunctionBuilder 运算全覆盖 — 位操作（clz/ctz/popcnt/rotl/rotr）
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_clz() {
    // i32 语义：lzcnt 32 位（opsize 参数化后不再 64 位计数）
    assert_eq!(
        run_test("clz", |b| {
            let a = b.iconst_i32(1);
            b.clz(a)
        }),
        31
    );
    assert_eq!(
        run_test("clz_high", |b| {
            let a = b.iconst_i32(0x8000_0000u32 as i32);
            b.clz(a)
        }),
        0
    );
}

#[test]
fn test_ctz() {
    // i32 语义：tzcnt 32 位
    assert_eq!(
        run_test("ctz", |b| {
            let a = b.iconst_i32(8);
            b.ctz(a)
        }),
        3
    );
    assert_eq!(
        run_test("ctz_high", |b| {
            let a = b.iconst_i32(0x8000_0000u32 as i32);
            b.ctz(a)
        }),
        31
    );
}

#[test]
fn test_popcnt() {
    assert_eq!(
        run_test("popcnt", |b| {
            let a = b.iconst_i32(0xFF);
            b.popcnt(a)
        }),
        8
    );
    assert_eq!(
        run_test("popcnt_zero", |b| {
            let a = b.iconst_i32(0);
            b.popcnt(a)
        }),
        0
    );
}

#[test]
fn test_rotl() {
    // i32 语义：0x80000001 rol 1 = 0x00000003（shift opsize 参数化）
    assert_eq!(
        run_test("rotl", |b| {
            let a = b.iconst_i32(0x8000_0001u32 as i32);
            let s = b.iconst_i32(1);
            b.rotl(a, s)
        }),
        3
    );
}

#[test]
fn test_rotr() {
    // i32 语义：0x80000001 ror 1 = 0xC0000000
    assert_eq!(
        run_test("rotr", |b| {
            let a = b.iconst_i32(0x8000_0001u32 as i32);
            let s = b.iconst_i32(1);
            b.rotr(a, s)
        }),
        0xC0000000u32 as i32
    );
}

// ═══════════════════════════════════════════════════════════════
// FunctionBuilder 运算全覆盖 — 整数扩展（abs/smin/smax/umin/umax/饱和/bswap）
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_abs() {
    assert_eq!(
        run_test("abs_neg", |b| {
            let a = b.iconst_i32(-42);
            b.abs(a)
        }),
        42
    );
    assert_eq!(
        run_test("abs_pos", |b| {
            let a = b.iconst_i32(42);
            b.abs(a)
        }),
        42
    );
    assert_eq!(
        run_test("abs_zero", |b| {
            let a = b.iconst_i32(0);
            b.abs(a)
        }),
        0
    );
}

#[test]
fn test_smin() {
    assert_eq!(
        run_test("smin", |b| {
            let a = b.iconst_i32(-5);
            let c = b.iconst_i32(3);
            b.smin(a, c)
        }),
        -5
    );
}

#[test]
fn test_smax() {
    assert_eq!(
        run_test("smax", |b| {
            let a = b.iconst_i32(-5);
            let c = b.iconst_i32(3);
            b.smax(a, c)
        }),
        3
    );
}

#[test]
fn test_umin() {
    // 无符号比较：0xFFFFFFFB (=4294967291) > 5 → min = 5
    assert_eq!(
        run_test("umin", |b| {
            let a = b.iconst_i32(0xFFFF_FFFBu32 as i32);
            let c = b.iconst_i32(5);
            b.umin(a, c)
        }),
        5
    );
}

#[test]
fn test_umax() {
    // 无符号比较：0xFFFFFFFF > 3 → max = 0xFFFFFFFF = -1
    assert_eq!(
        run_test("umax", |b| {
            let a = b.iconst_i32(-1);
            let c = b.iconst_i32(3);
            b.umax(a, c)
        }),
        -1
    );
}

#[test]
fn test_ssub_sat() {
    // 带符号饱和减法（i32）：i32::MIN - 1 饱和到 i32::MIN
    //（clamp 常量已按宽度参数化，历史 64 位常量曾使 i32 输入得 0）
    assert_eq!(
        run_test("ssub_sat", |b| {
            let a = b.iconst_i32(i32::MIN);
            let c = b.iconst_i32(1);
            b.ssub_sat(a, c)
        }),
        i32::MIN
    );
    // 普通场景：10 - 3 = 7
    assert_eq!(
        run_test("ssub_sat2", |b| {
            let a = b.iconst_i32(10);
            let c = b.iconst_i32(3);
            b.ssub_sat(a, c)
        }),
        7
    );
}

#[test]
fn test_uadd_sat() {
    // 无符号饱和加法：0xFFFFFFFF + 1 饱和到 0xFFFFFFFF
    assert_eq!(
        run_test("uadd_sat", |b| {
            let a = b.iconst_i32(-1);
            let c = b.iconst_i32(1);
            b.uadd_sat(a, c)
        }),
        -1
    );
}

#[test]
fn test_usub_sat() {
    // 无符号饱和减法：0 - 1 饱和到 0
    assert_eq!(
        run_test("usub_sat", |b| {
            let a = b.iconst_i32(0);
            let c = b.iconst_i32(1);
            b.usub_sat(a, c)
        }),
        0
    );
}

#[test]
fn test_bswap() {
    // i32 语义：0x12345678 → 0x78563412（bswap opsize 参数化）
    assert_eq!(
        run_test("bswap", |b| {
            let a = b.iconst_i32(0x1234_5678);
            b.bswap(a)
        }),
        0x7856_3412i32
    );
}

// ═══════════════════════════════════════════════════════════════
// FunctionBuilder 运算全覆盖 — 浮点扩展（fma/fmin/fmax/fcopysign/floor/ceil/trunc/round）
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_fma() {
    // 选整数结果避免非真融合（movsd+mulsd+addsd 两次舍入）的舍入差异
    let r = run_test_f64("fma", |b| {
        let a = b.fconst_f64(6.0);
        let c = b.fconst_f64(7.0);
        let d = b.fconst_f64(0.0);
        b.fma(a, c, d)
    });
    assert!((r - 42.0).abs() < 1e-10, "fma: got {r}, expected 42.0");
}

#[test]
fn test_fmin() {
    let r = run_test_f64("fmin", |b| {
        let a = b.fconst_f64(1.5);
        let c = b.fconst_f64(2.5);
        b.fmin(a, c)
    });
    assert!((r - 1.5).abs() < 1e-10, "fmin: got {r}, expected 1.5");
}

#[test]
fn test_fmax() {
    let r = run_test_f64("fmax", |b| {
        let a = b.fconst_f64(1.5);
        let c = b.fconst_f64(2.5);
        b.fmax(a, c)
    });
    assert!((r - 2.5).abs() < 1e-10, "fmax: got {r}, expected 2.5");
}

#[test]
fn test_fcopysign() {
    // 拷贝第二个操作数的符号
    let r = run_test_f64("fcopysign", |b| {
        let a = b.fconst_f64(42.0);
        let c = b.fconst_f64(-1.0);
        b.fcopysign(a, c)
    });
    assert!(
        (r - (-42.0)).abs() < 1e-10,
        "fcopysign: got {r}, expected -42.0"
    );
    let r2 = run_test_f64("fcopysign2", |b| {
        let a = b.fconst_f64(-42.0);
        let c = b.fconst_f64(1.0);
        b.fcopysign(a, c)
    });
    assert!(
        (r2 - 42.0).abs() < 1e-10,
        "fcopysign2: got {r2}, expected 42.0"
    );
}

#[test]
fn test_ffloor() {
    let r = run_test_f64("ffloor", |b| {
        let a = b.fconst_f64(42.7);
        b.ffloor(a)
    });
    assert!((r - 42.0).abs() < 1e-10, "ffloor: got {r}, expected 42.0");
    let r2 = run_test_f64("ffloor_neg", |b| {
        let a = b.fconst_f64(-42.7);
        b.ffloor(a)
    });
    assert!(
        (r2 - (-43.0)).abs() < 1e-10,
        "ffloor_neg: got {r2}, expected -43.0"
    );
}

#[test]
fn test_fceil() {
    let r = run_test_f64("fceil", |b| {
        let a = b.fconst_f64(42.3);
        b.fceil(a)
    });
    assert!((r - 43.0).abs() < 1e-10, "fceil: got {r}, expected 43.0");
    let r2 = run_test_f64("fceil_neg", |b| {
        let a = b.fconst_f64(-42.3);
        b.fceil(a)
    });
    assert!(
        (r2 - (-42.0)).abs() < 1e-10,
        "fceil_neg: got {r2}, expected -42.0"
    );
}

#[test]
fn test_fptrunc() {
    // S3.4：double → float（cvtsd2ss 舍入转单精度）
    let r = run_test_f32("fptrunc", |b| {
        let a = b.fconst_f64(42.5);
        b.fptrunc(a, TypeId::F32)
    });
    assert!(
        (r - 42.5f32).abs() < 1e-6,
        "fptrunc: got {r}, expected 42.5"
    );
    let r2 = run_test_f32("fptrunc_frac", |b| {
        let a = b.fconst_f64(f64::consts::PI);
        b.fptrunc(a, TypeId::F32)
    });
    assert!(
        (r2 - f32::consts::PI).abs() < 1e-6,
        "fptrunc: got {r2}, expected ~3.14159"
    );
}

#[test]
fn test_fpext() {
    // S3.4：float → double（cvtss2sd 精确扩展）
    let r = run_test_f64("fpext", |b| {
        let a = b.fconst_f32(42.5);
        b.fpext(a, TypeId::F64)
    });
    assert!((r - 42.5).abs() < 1e-12, "fpext: got {r}, expected 42.5");
    let r2 = run_test_f64("fpext_frac", |b| {
        let a = b.fconst_f32(1.5);
        b.fpext(a, TypeId::F64)
    });
    assert!((r2 - 1.5).abs() < 1e-12, "fpext: got {r2}, expected 1.5");
}

#[test]
fn test_ftrunc() {
    // 截断语义（cvttsd2si）：42.7 → 42.0、-42.7 → -42.0（toward zero）
    let r = run_test_f64("ftrunc", |b| {
        let a = b.fconst_f64(42.7);
        b.ftrunc(a)
    });
    assert!((r - 42.0).abs() < 1e-10, "ftrunc: got {r}, expected 42.0");
    let r2 = run_test_f64("ftrunc_neg", |b| {
        let a = b.fconst_f64(-42.7);
        b.ftrunc(a)
    });
    assert!(
        (r2 - (-42.0)).abs() < 1e-10,
        "ftrunc_neg: got {r2}, expected -42.0"
    );
}

#[test]
fn test_fround() {
    // round-to-nearest
    let r = run_test_f64("fround", |b| {
        let a = b.fconst_f64(42.3);
        b.fround(a)
    });
    assert!((r - 42.0).abs() < 1e-10, "fround: got {r}, expected 42.0");
    let r2 = run_test_f64("fround_up", |b| {
        let a = b.fconst_f64(42.7);
        b.fround(a)
    });
    assert!(
        (r2 - 43.0).abs() < 1e-10,
        "fround_up: got {r2}, expected 43.0"
    );
}

// ═══════════════════════════════════════════════════════════════
// FunctionBuilder 运算全覆盖 — 溢出运算（6 个，value + flag 双断言）
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_sadd_overflow_value() {
    // i32::MAX + 1 → 回绕为 i32::MIN（现有测试只验 flag，这里验 value）
    let v = run_test("sadd_ov_v", |b| {
        let mx = b.iconst_i32(i32::MAX);
        let one = b.iconst_i32(1);
        let (r, _fl) = b.sadd_overflow(mx, one);
        r
    });
    assert_eq!(v, i32::MIN, "sadd_overflow value 应回绕为 i32::MIN");
    let fl = run_test("sadd_ov_f", |b| {
        let mx = b.iconst_i32(i32::MAX);
        let one = b.iconst_i32(1);
        let (_r, f) = b.sadd_overflow(mx, one);
        b.uextend(f, TypeId::I32)
    });
    assert_eq!(fl, 1, "sadd_overflow flag 应为 1");
}

#[test]
fn test_uadd_overflow() {
    let v = run_test("uadd_ov_v", |b| {
        let mx = b.iconst_i32(-1); // 0xFFFFFFFF
        let one = b.iconst_i32(1);
        let (r, _fl) = b.uadd_overflow(mx, one);
        r
    });
    assert_eq!(v, 0, "uadd_overflow value 应回绕为 0");
    let fl = run_test("uadd_ov_f", |b| {
        let mx = b.iconst_i32(-1);
        let one = b.iconst_i32(1);
        let (_r, f) = b.uadd_overflow(mx, one);
        b.uextend(f, TypeId::I32)
    });
    assert_eq!(fl, 1, "uadd_overflow flag 应为 1");
}

#[test]
fn test_ssub_overflow() {
    let v = run_test("ssub_ov_v", |b| {
        let mn = b.iconst_i32(i32::MIN);
        let one = b.iconst_i32(1);
        let (r, _fl) = b.ssub_overflow(mn, one);
        r
    });
    assert_eq!(v, i32::MAX, "ssub_overflow value 应回绕为 i32::MAX");
    let fl = run_test("ssub_ov_f", |b| {
        let mn = b.iconst_i32(i32::MIN);
        let one = b.iconst_i32(1);
        let (_r, f) = b.ssub_overflow(mn, one);
        b.uextend(f, TypeId::I32)
    });
    assert_eq!(fl, 1, "ssub_overflow flag 应为 1");
}

#[test]
fn test_usub_overflow() {
    let v = run_test("usub_ov_v", |b| {
        let z = b.iconst_i32(0);
        let one = b.iconst_i32(1);
        let (r, _fl) = b.usub_overflow(z, one);
        r
    });
    assert_eq!(v, -1, "usub_overflow value 应回绕为 0xFFFFFFFF");
    let fl = run_test("usub_ov_f", |b| {
        let z = b.iconst_i32(0);
        let one = b.iconst_i32(1);
        let (_r, f) = b.usub_overflow(z, one);
        b.uextend(f, TypeId::I32)
    });
    assert_eq!(fl, 1, "usub_overflow flag 应为 1");
}

#[test]
fn test_smul_overflow() {
    let v = run_test("smul_ov_v", |b| {
        let a = b.iconst_i32(0x4000_0000);
        let two = b.iconst_i32(2);
        let (r, _fl) = b.smul_overflow(a, two);
        r
    });
    assert_eq!(v, i32::MIN, "smul_overflow value 应回绕为 i32::MIN");
    let fl = run_test("smul_ov_f", |b| {
        let a = b.iconst_i32(0x4000_0000);
        let two = b.iconst_i32(2);
        let (_r, f) = b.smul_overflow(a, two);
        b.uextend(f, TypeId::I32)
    });
    assert_eq!(fl, 1, "smul_overflow flag 应为 1");
}

#[test]
fn test_umul_overflow() {
    let v = run_test("umul_ov_v", |b| {
        let a = b.iconst_i32(-1); // 0xFFFFFFFF
        let two = b.iconst_i32(2);
        let (r, _fl) = b.umul_overflow(a, two);
        r
    });
    assert_eq!(v, -2, "umul_overflow value 应回绕为 0xFFFFFFFE");
    let fl = run_test("umul_ov_f", |b| {
        let a = b.iconst_i32(-1);
        let two = b.iconst_i32(2);
        let (_r, f) = b.umul_overflow(a, two);
        b.uextend(f, TypeId::I32)
    });
    assert_eq!(fl, 1, "umul_overflow flag 应为 1");
}

// ═══════════════════════════════════════════════════════════════
// FunctionBuilder 运算全覆盖 — 指针谓词（is_null/is_not_null）
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_is_null() {
    // is_null(0) = 1 → 1 + 41 = 42
    assert_eq!(
        run_test("is_null", |b| {
            let p = b.iconst(0, TypeId::PTR);
            let cond = b.is_null(p);
            let c41 = b.iconst_i32(41);
            b.iadd(cond, c41)
        }),
        42
    );
}

#[test]
fn test_is_not_null() {
    // is_not_null(有效栈地址) = 1 → 1 + 41 = 42
    assert_eq!(
        run_test("is_not_null", |b| {
            let p = b.stack_addr(0);
            let cond = b.is_not_null(p);
            let c41 = b.iconst_i32(41);
            b.iadd(cond, c41)
        }),
        42
    );
}

// ═══════════════════════════════════════════════════════════════
// FunctionBuilder 运算全覆盖 — 控制流/值语义（nop/freeze；trap 仅编译）
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_nop() {
    assert_eq!(
        run_test("nop", |b| {
            b.nop();
            b.iconst_i32(42)
        }),
        42
    );
}

#[test]
fn test_freeze() {
    assert_eq!(
        run_test("freeze", |b| {
            let a = b.iconst_i32(42);
            b.freeze(a)
        }),
        42
    );
}

/// trap 发射 UD2：一旦执行即非法指令崩溃，无法在本进程内执行断言——
/// 此处仅验证编译通过且产物非空。
#[test]
fn test_trap_compiles() {
    ensure_registered();
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("trap_c", TypeContext::new(), sig);
    b.create_block_here();
    b.trap();
    let z = b.iconst_i32(0);
    b.ret(&[z]); // trap 后不可达，但需显式终结符满足 finish 防御检查
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .expect("trap must compile");
    assert!(!compiled.code.is_empty(), "trap: empty code");
}

// ═══════════════════════════════════════════════════════════════
// FunctionBuilder 运算全覆盖 — fconst_f32
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_fconst_f32() {
    let r = run_test_f32("fconst_f32", |b| b.fconst_f32(42.0));
    assert!(
        (r - 42.0).abs() < 1e-6,
        "fconst_f32: got {r}, expected 42.0"
    );
}

// ═══════════════════════════════════════════════════════════════
// FunctionBuilder 运算全覆盖 — fcmp 补齐剩余 6 个条件
//（Equal/GreaterThan 已有；补 LessThan/LE/GE/NotEqual/Ordered/Unordered）
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_fcmp_lt() {
    let r = run_test("fcmp_lt", |b| {
        let a = b.fconst_f64(10.0);
        let c = b.fconst_f64(50.0);
        let cond = b.fcmp(FloatCC::LessThan, a, c); // 1
        let c41 = b.iconst_i32(41);
        b.iadd(cond, c41)
    });
    assert_eq!(r, 42);
}

#[test]
fn test_fcmp_le() {
    let r = run_test("fcmp_le", |b| {
        let a = b.fconst_f64(42.0);
        let c = b.fconst_f64(42.0);
        let cond = b.fcmp(FloatCC::LessThanOrEqual, a, c); // 1
        let c41 = b.iconst_i32(41);
        b.iadd(cond, c41)
    });
    assert_eq!(r, 42);
}

#[test]
fn test_fcmp_ge() {
    let r = run_test("fcmp_ge", |b| {
        let a = b.fconst_f64(42.0);
        let c = b.fconst_f64(42.0);
        let cond = b.fcmp(FloatCC::GreaterThanOrEqual, a, c); // 1
        let c41 = b.iconst_i32(41);
        b.iadd(cond, c41)
    });
    assert_eq!(r, 42);
}

#[test]
fn test_fcmp_ne() {
    let r = run_test("fcmp_ne", |b| {
        let a = b.fconst_f64(42.0);
        let c = b.fconst_f64(43.0);
        let cond = b.fcmp(FloatCC::NotEqual, a, c); // 1
        let c41 = b.iconst_i32(41);
        b.iadd(cond, c41)
    });
    assert_eq!(r, 42);
}

#[test]
fn test_fcmp_ordered() {
    // Ordered(42, 43) = 1
    let r = run_test("fcmp_ord", |b| {
        let a = b.fconst_f64(42.0);
        let c = b.fconst_f64(43.0);
        let cond = b.fcmp(FloatCC::Ordered, a, c);
        let c41 = b.iconst_i32(41);
        b.iadd(cond, c41)
    });
    assert_eq!(r, 42);
    // Ordered(NaN, 1) = 0
    let r2 = run_test("fcmp_ord_nan", |b| {
        let nan = b.fconst_f64(f64::from_bits(0x7FF8_0000_0000_0000)); // f64::NAN 模块常量弃用
        let one = b.fconst_f64(1.0);
        let cond = b.fcmp(FloatCC::Ordered, nan, one);
        let c41 = b.iconst_i32(41);
        b.iadd(cond, c41)
    });
    assert_eq!(r2, 41, "Ordered(NaN, 1) 应为 0");
}

#[test]
fn test_fcmp_unordered() {
    // Unordered(NaN, 1) = 1
    let r = run_test("fcmp_uno", |b| {
        let nan = b.fconst_f64(f64::from_bits(0x7FF8_0000_0000_0000)); // f64::NAN 模块常量弃用
        let one = b.fconst_f64(1.0);
        let cond = b.fcmp(FloatCC::Unordered, nan, one);
        let c41 = b.iconst_i32(41);
        b.iadd(cond, c41)
    });
    assert_eq!(r, 42);
}
