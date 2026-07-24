//! JIT 集成演示 — 检测每一条指令能通过编译→JIT→执行的完整管线。
//!
//! 每个测试编译一个 IR 函数 → 生成 x86_64 机器码 → JIT 执行 → 验证结果。

use codegen_lib::backend::x86_64::{X86Isa, ensure_registered};
use codegen_lib::backend::{FunctionCompiler, Registry};
use codegen_lib::executable_memory::ExecutableMemory;
use codegen_lib::prelude::*;

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
        b.return_(&[v]);
    });

    // ═══ 整数算术 ═══
    t!("iadd (20+22)", 42, |b| {
        let a = b.iconst_i32(20);
        let c = b.iconst_i32(22);
        let r = b.iadd(a, c);
        b.return_(&[r]);
    });
    t!("isub (84-42)", 42, |b| {
        let a = b.iconst_i32(84);
        let c = b.iconst_i32(42);
        let r = b.isub(a, c);
        b.return_(&[r]);
    });
    t!("imul (6*7)", 42, |b| {
        let a = b.iconst_i32(6);
        let c = b.iconst_i32(7);
        let r = b.imul(a, c);
        b.return_(&[r]);
    });
    t!("imul+iadd (5*8+2)", 42, |b| {
        let a = b.iconst_i32(5);
        let c = b.iconst_i32(8);
        let m = b.imul(a, c);
        let d = b.iconst_i32(2);
        let r = b.iadd(m, d);
        b.return_(&[r]);
    });

    // ═══ 位运算 ═══
    t!("band (0xFF & 42)", 42, |b| {
        let a = b.iconst_i32(0xFF);
        let c = b.iconst_i32(42);
        let r = b.band(a, c);
        b.return_(&[r]);
    });
    t!("bor (40|2)", 42, |b| {
        let a = b.iconst_i32(40);
        let c = b.iconst_i32(2);
        let r = b.bor(a, c);
        b.return_(&[r]);
    });
    t!("bxor (63^21)", 42, |b| {
        let a = b.iconst_i32(63);
        let c = b.iconst_i32(21);
        let r = b.bxor(a, c);
        b.return_(&[r]);
    });
    t!("bnot (~213 unsigned)", 42, |b| {
        let a = b.iconst_i64(!42i64);
        let r = b.bnot(a);
        b.return_(&[r]);
    });

    // ═══ 移位 ═══
    t!("ishl (21<<1)", 42, |b| {
        let a = b.iconst_i32(21);
        let s = b.iconst_i32(1);
        let r = b.ishl(a, s);
        b.return_(&[r]);
    });
    t!("ushr (168>>2)", 42, |b| {
        let a = b.iconst_i32(168);
        let s = b.iconst_i32(2);
        let r = b.ushr(a, s);
        b.return_(&[r]);
    });

    // ═══ 大数值 64 位算术 (验证 REX.W) ═══
    t!("iadd_i64_large (2B+1=3B)", 3_000_000_000i64, |b| {
        let a = b.iconst_i64(2_000_000_000i64);
        let c = b.iconst_i64(1_000_000_000i64);
        let r = b.iadd(a, c);
        b.return_(&[r]);
    });
    t!("imul_i64_large (1G*3=3G)", 3_000_000_000i64, |b| {
        let a = b.iconst_i64(1_000_000_000i64);
        let c = b.iconst_i64(3);
        let r = b.imul(a, c);
        b.return_(&[r]);
    });

    // ═══ 结果 ═══
    println!("\n  ─────────────────────────────────────");
    println!("  Total: {} passed, {} failed", passed, failed);
    if failed == 0 {
        println!("  All tests passed!");
    }
}

fn run_test(build: fn(&mut FunctionBuilder)) -> Result<i64, String> {
    let sig = Signature::new(&[], &[Type::I64]);
    let mut b = FunctionBuilder::new("test", sig);
    b.create_block_here();
    build(&mut b);
    let func = b.finish();
    let compiled = FunctionCompiler::<X86Isa>::compile_raw(&func).map_err(|e| format!("{}", e))?;
    let mem = ExecutableMemory::new(&compiled.code).map_err(|e| format!("alloc: {}", e))?;
    let f: extern "C" fn() -> i64 = unsafe { mem.get_fn(0).unwrap() };
    Ok(f())
}
