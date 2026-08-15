//! JIT 集成演示 — 检测每一条指令能通过编译→JIT→执行的完整管线。
//!
//! 每个测试编译一个 IR 函数 → 生成 x86_64 机器码 → JIT 执行 → 验证结果。

use code_forge::backend::arch::x86_64::{self, ensure_registered};
use code_forge::backend::{FunctionCompiler, Registry};
use code_forge::mem::ExecutableMemory;
use code_forge::prelude::*;

fn main() {
    ensure_registered();
    assert!(Registry::global().contains("x86_64"));
    println!("=== x86_64 JIT Integration Demo ===\n");

    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! t {
        ($name:expr, $expected:expr, $build:expr) => {{
            print!("  {:40} ", $name);
            match run_test($build) {
                Ok(v) if v == $expected => {
                    println!("✓ {} (expected {})", v, $expected);
                    passed += 1;
                }
                Ok(v) => {
                    println!("✗ got {} expected {}", v, $expected);
                    failed += 1;
                }
                Err(e) => {
                    println!("✗ error: {}", e);
                    failed += 1;
                }
            }
        }};
    }

    // ═══ 数据移动 ═══
    t!("iconst (42)", 42, |b| {
        let v = b.iconst_i32(42);
        b.ret(&[v]);
    });

    // ═══ 整数算术 ═══
    t!("iadd (20+22)", 42, |b| {
        let a = b.iconst_i32(20);
        let c = b.iconst_i32(22);
        let r = b.iadd(a, c);
        b.ret(&[r]);
    });
    t!("isub (84-42)", 42, |b| {
        let a = b.iconst_i32(84);
        let c = b.iconst_i32(42);
        let r = b.isub(a, c);
        b.ret(&[r]);
    });
    t!("imul (6*7)", 42, |b| {
        let a = b.iconst_i32(6);
        let c = b.iconst_i32(7);
        let r = b.imul(a, c);
        b.ret(&[r]);
    });
    t!("imul+iadd (5*8+2)", 42, |b| {
        let a = b.iconst_i32(5);
        let c = b.iconst_i32(8);
        let m = b.imul(a, c);
        let d = b.iconst_i32(2);
        let r = b.iadd(m, d);
        b.ret(&[r]);
    });

    // ═══ 位运算 ═══
    t!("band (0xFF & 42)", 42, |b| {
        let a = b.iconst_i32(0xFF);
        let c = b.iconst_i32(42);
        let r = b.band(a, c);
        b.ret(&[r]);
    });
    t!("bor (40|2)", 42, |b| {
        let a = b.iconst_i32(40);
        let c = b.iconst_i32(2);
        let r = b.bor(a, c);
        b.ret(&[r]);
    });
    t!("bxor (63^21)", 42, |b| {
        let a = b.iconst_i32(63);
        let c = b.iconst_i32(21);
        let r = b.bxor(a, c);
        b.ret(&[r]);
    });
    t!("bnot (~213 unsigned)", 42, |b| {
        let a = b.iconst_i64(!42i64);
        let r = b.bnot(a);
        b.ret(&[r]);
    });

    // ═══ 移位 ═══
    t!("ishl (21<<1)", 42, |b| {
        let a = b.iconst_i32(21);
        let s = b.iconst_i32(1);
        let r = b.ishl(a, s);
        b.ret(&[r]);
    });
    t!("ushr (168>>2)", 42, |b| {
        let a = b.iconst_i32(168);
        let s = b.iconst_i32(2);
        let r = b.ushr(a, s);
        b.ret(&[r]);
    });

    // ═══ 大数值 64 位算术 (验证 REX.W) ═══
    t!("iadd_i64_large (2B+1=3B)", 3_000_000_000i64, |b| {
        let a = b.iconst_i64(2_000_000_000i64);
        let c = b.iconst_i64(1_000_000_000i64);
        let r = b.iadd(a, c);
        b.ret(&[r]);
    });
    t!("imul_i64_large (1G*3=3G)", 3_000_000_000i64, |b| {
        let a = b.iconst_i64(1_000_000_000i64);
        let c = b.iconst_i64(3);
        let r = b.imul(a, c);
        b.ret(&[r]);
    });

    // ═══ 结果 ═══
    println!("\n  ─────────────────────────────────────");
    println!("  Total: {} passed, {} failed", passed, failed);
    if failed == 0 {
        println!("  All tests passed!");
    }
}

fn run_test(build: fn(&mut FunctionBuilder)) -> Result<i64, String> {
    let sig = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
    b.create_block_here();
    build(&mut b);
    let func = b.finish().expect("build");
    let compiled = FunctionCompiler::new(x86_64::TargetMachine::new())
        .compile_raw(&func)
        .map_err(|e| format!("{}", e))?;
    let mem = ExecutableMemory::new(&compiled.code).map_err(|e| format!("alloc: {}", e))?;
    let f: extern "C" fn() -> i64 = unsafe { mem.get_fn(0).unwrap() };
    Ok(f())
}
