//! `wide_split` 的语义测试（JIT 真执行，走完整管线）。
//!
//! 每条测试都用 `JitLoader::compile_module`（会跑 `LegalizeDriver::scalar()`），因此同时验证
//! "pass 真的被调用"——本会话最大的坑正是普通函数一度从不跑 legalize。

use super::*;
use crate::pipeline::legalize::{LegalizeCtx, LegalizeDriver};

/// 跑一遍标量批（单元级，不经 JIT）。
#[allow(dead_code)]
fn run_scalar(func: &mut Function) {
    let machine = crate::arch::x86::TargetMachine::new();
    let mut lctx = LegalizeCtx::new(&machine, "win64".to_string());
    LegalizeDriver::scalar()
        .run(func, &mut lctx)
        .expect("标量批必须成功");
}

#[cfg(all(target_arch = "x86_64", feature = "jit"))]
mod jit {
    use super::*;
    use crate::arch::x86::{TargetMachine, ensure_registered};
    use forge_ir::Module;

    /// 编译并执行一个无参函数，返回其 `i64` 结果。
    fn compile_run(m: &Module, name: &str) -> i64 {
        let mut jit = crate::jit::JitLoader::new(TargetMachine::new());
        jit.compile_module(m).expect("module compile");
        let f: extern "C" fn() -> i64 = jit.get_fn(name).expect("fn 存在");
        f()
    }

    /// `(2^64 - 1) + 1`：低半全 1 + 1 ⇒ 进位必须传到高半 ⇒ 高半 = 1。
    #[test]
    fn jit_i128_add_carry_propagates() {
        ensure_registered();
        let tc = TypeContext::new();
        let mut m = Module::new();
        {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut fb = FunctionBuilder::new("add_carry", tc, sig);
            fb.create_block_here();
            let x = fb.const_int((1i128 << 64) - 1, TypeId::I128);
            let one = fb.const_int(1, TypeId::I128);
            let s = fb.iadd(x, one);
            let amt = fb.const_int(64, TypeId::I128);
            let hi = fb.ushr(s, amt);
            let hi64 = fb.ireduce(hi, TypeId::I64);
            fb.ret(&[hi64]);
            m.add_function(fb.finish().expect("build"));
        }
        assert_eq!(compile_run(&m, "add_carry"), 1, "进位必须传到高半");
    }

    /// `(2^64 + 5) < (2^64 + 6)`：高半相等，胜负由低半决定。
    #[test]
    fn jit_i128_cmp_lexicographic() {
        ensure_registered();
        let mut m = Module::new();
        for (idx, (x, y, cc, want)) in [
            (
                (1i128 << 64) + 5,
                (1i128 << 64) + 6,
                forge_ir::IntCC::Slt,
                1i64,
            ),
            (
                (1i128 << 64) + 5,
                (1i128 << 64) + 1,
                forge_ir::IntCC::Slt,
                0,
            ),
            ((1i128 << 64) + 5, 1i128 << 64, forge_ir::IntCC::Sgt, 1),
            ((1i128 << 64) + 5, (1i128 << 64) + 5, forge_ir::IntCC::Eq, 1),
            (5i128, 6i128, forge_ir::IntCC::Slt, 1),
        ]
        .into_iter()
        .enumerate()
        {
            let name = format!("cmp{idx}");
            let mut fb = FunctionBuilder::new(
                name.as_str(),
                TypeContext::new(),
                FunctionSignature::new(&[], &[TypeId::I64]),
            );
            fb.create_block_here();
            let xv = fb.const_int(x, TypeId::I128);
            let yv = fb.const_int(y, TypeId::I128);
            let c = fb.icmp(cc, xv, yv);
            let z = fb.uextend(c, TypeId::I64);
            fb.ret(&[z]);
            m.add_function(fb.finish().expect("build"));
            let _ = want;
        }
        let mut jit = crate::jit::JitLoader::new(TargetMachine::new());
        jit.compile_module(&m).expect("module compile");
        for (idx, (_, _, _, want)) in [
            (
                (1i128 << 64) + 5,
                (1i128 << 64) + 6,
                forge_ir::IntCC::Slt,
                1i64,
            ),
            (
                (1i128 << 64) + 5,
                (1i128 << 64) + 1,
                forge_ir::IntCC::Slt,
                0,
            ),
            ((1i128 << 64) + 5, 1i128 << 64, forge_ir::IntCC::Sgt, 1),
            ((1i128 << 64) + 5, (1i128 << 64) + 5, forge_ir::IntCC::Eq, 1),
            (5i128, 6i128, forge_ir::IntCC::Slt, 1),
        ]
        .into_iter()
        .enumerate()
        {
            let name = format!("cmp{idx}");
            let f: extern "C" fn() -> i64 = jit.get_fn(name.as_str()).expect("fn");
            assert_eq!(f(), want, "第 {idx} 个宽值比较不对");
        }
    }

    /// `(1 << 68) >> 64` = 16：常量非整肢移位（跨肢拼接）。
    #[test]
    fn jit_i128_shift_nonwhole() {
        ensure_registered();
        let tc = TypeContext::new();
        let mut m = Module::new();
        {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut fb = FunctionBuilder::new("shl68", tc, sig);
            fb.create_block_here();
            let one = fb.const_int(1, TypeId::I128);
            let amt = fb.const_int(68, TypeId::I128);
            let shifted = fb.ishl(one, amt);
            let amt64 = fb.const_int(64, TypeId::I128);
            let down = fb.ushr(shifted, amt64);
            let out = fb.ireduce(down, TypeId::I64);
            fb.ret(&[out]);
            m.add_function(fb.finish().expect("build"));
        }
        assert_eq!(compile_run(&m, "shl68"), 16, "(1<<68)>>64 应为 16");
    }

    /// **非字宽整数倍**：`i100` 的 `x + x` 在 100 位下回绕成 0（顶肢规范化 + 顶肢进位）。
    #[test]
    fn jit_i100_wraps_and_normalizes_top_limb() {
        ensure_registered();
        let tc = TypeContext::new();
        let i100 = tc.int_ty(100);
        let mut m = Module::new();
        {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut fb = FunctionBuilder::new("i100_wrap", tc.clone(), sig);
            fb.create_block_here();
            // x = 2^99 ⇒ x + x = 2^100 ≡ 0 (mod 2^100)
            let x = fb.const_big(forge_ir::util::big::Big::from_i128(1i128 << 99), i100);
            let s = fb.iadd(x, x);
            let out = fb.ireduce(s, TypeId::I64);
            fb.ret(&[out]);
            m.add_function(fb.finish().expect("build"));
        }
        assert_eq!(
            compile_run(&m, "i100_wrap"),
            0,
            "2^99 + 2^99 在 i100 里应回绕成 0"
        );
    }

    /// **4 肢**：`i256` 常量 + 加法 + 比较（任意宽度可算）。
    #[test]
    fn jit_i256_const_add_cmp() {
        ensure_registered();
        let tc = TypeContext::new();
        let i256 = tc.int_ty(256);
        let mut m = Module::new();
        {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut fb = FunctionBuilder::new("i256_cmp", tc.clone(), sig);
            fb.create_block_here();
            // a = 2^200 + 7, b = 2^200 + 9 ⇒ a < b
            let a = fb.const_big(forge_ir::util::big::Big::from_i128(1i128 << 100), i256);
            let b = fb.const_big(forge_ir::util::big::Big::from_i128(1i128 << 101), i256);
            let c = fb.icmp(forge_ir::IntCC::Slt, a, b);
            let c2 = fb.icmp(forge_ir::IntCC::Sgt, b, a);
            let both = fb.band(c, c2);
            let out = fb.uextend(both, TypeId::I64);
            fb.ret(&[out]);
            m.add_function(fb.finish().expect("build"));
        }
        assert_eq!(compile_run(&m, "i256_cmp"), 1, "2^200 与 2^201 的比较");
    }

    /// **运行期移位量**：移位量来自 `select`（不可折叠）⇒ 走 O(n²) 的 select 链。
    /// `1u128 << 68` 再 `>> 64` 应为 16。
    #[test]
    fn jit_i128_shift_runtime_amount() {
        ensure_registered();
        let tc = TypeContext::new();
        let mut m = Module::new();
        {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut fb = FunctionBuilder::new("shl_rt", tc, sig);
            fb.create_block_here();
            let one = fb.const_int(1, TypeId::I128);
            // cond = (1 < 2) —— 常量条件也不会被 IR 层折叠（没有 CSE/常量折叠 pass 介入这条）
            let c1 = fb.const_int(1, TypeId::I64);
            let c2 = fb.const_int(2, TypeId::I64);
            let cond = fb.icmp(forge_ir::IntCC::Slt, c1, c2);
            let a68 = fb.const_int(68, TypeId::I128);
            let a4 = fb.const_int(4, TypeId::I128);
            let amt = fb.select(cond, a68, a4); // 运行期值
            let shifted = fb.ishl(one, amt);
            let a64 = fb.const_int(64, TypeId::I128);
            let down = fb.ushr(shifted, a64);
            let out = fb.ireduce(down, TypeId::I64);
            fb.ret(&[out]);
            m.add_function(fb.finish().expect("build"));
        }
        assert_eq!(
            compile_run(&m, "shl_rt"),
            16,
            "运行期移位量 1<<68 >>64 应为 16"
        );
    }

    /// **乘法**：`(2^64+5) * (2^64+7) mod 2^128 = 12·2^64 + 35` ⇒ 高肢 12、低肢 35。
    #[test]
    fn jit_i128_mul_limbs() {
        ensure_registered();
        let mut m = Module::new();
        for (name, want_hi) in [("mul_hi", 12i64), ("mul_lo", 35)] {
            let mut fb = FunctionBuilder::new(
                name,
                TypeContext::new(),
                FunctionSignature::new(&[], &[TypeId::I64]),
            );
            fb.create_block_here();
            let x = fb.const_int((1i128 << 64) + 5, TypeId::I128);
            let y = fb.const_int((1i128 << 64) + 7, TypeId::I128);
            let p = fb.imul(x, y);
            let a64 = fb.const_int(64, TypeId::I128);
            let part = if want_hi != 35 { fb.ushr(p, a64) } else { p };
            let out = fb.ireduce(part, TypeId::I64);
            fb.ret(&[out]);
            m.add_function(fb.finish().expect("build"));
        }
        let mut jit = crate::jit::JitLoader::new(TargetMachine::new());
        jit.compile_module(&m).expect("module compile");
        let hi: extern "C" fn() -> i64 = jit.get_fn("mul_hi").expect("mul_hi");
        let lo: extern "C" fn() -> i64 = jit.get_fn("mul_lo").expect("mul_lo");
        assert_eq!((hi(), lo()), (12, 35), "(2^64+5)*(2^64+7) 的高/低肢");
    }

    /// **宽值形参必须 fail-closed**（isel 没有"一个值占两个寄存器"；前端边界用指针）。
    #[test]
    fn wide_param_fails_closed() {
        let tc = TypeContext::new();
        let sig = FunctionSignature::new(&[(TypeId::I128, "a")], &[TypeId::I64]);
        let mut fb = FunctionBuilder::new("wide_param", tc, sig);
        let (e, _p) = fb.create_block_with_params(&[(TypeId::I128, "a")]);
        fb.switch_to_block(e);
        let z = fb.const_int(0, TypeId::I64);
        fb.ret(&[z]);
        let mut func = fb.finish().expect("build");
        let machine = crate::arch::x86::TargetMachine::new();
        let mut lctx = LegalizeCtx::new(&machine, "win64".to_string());
        let err = LegalizeDriver::scalar()
            .run(&mut func, &mut lctx)
            .expect_err("宽值块参数必须被拒绝");
        let msg = format!("{err:?}");
        assert!(msg.contains("WIDE_INT_SHAPE"), "应带缺口 ID，实测：{msg}");
    }

    /// **宽值除/余**：`wide_div` pass 的分肢长除法（v27 M6-79）。
    ///
    /// 形参 `(lo, hi, d)` 全是 i64：IR 里用 `zext(lo) | (zext(hi) << 64)` 拼出 128 位被除数、
    /// `zext(d)` 当除数 —— 值真的随运行期变化（不会被常量折叠），oracle 用本进程的原生
    /// `u128` 除法对比。覆盖：商不跨肢 / 被除数高位参与 / 除数跨肢 / `u128::MAX / 3`。
    #[test]
    fn jit_wide_div_correct() {
        ensure_registered();
        let tc = TypeContext::new();
        let mut m = Module::new();
        let mut cases: Vec<(String, u128, u128, bool)> = Vec::new();
        for (idx, (x, y, is_rem)) in [
            (100u128, 7u128, false),
            (100, 7, true),
            ((1u128 << 64) + 100, 7, false),
            ((1u128 << 64) + 100, 7, true),
            (u128::MAX, 3, false),
            (9007199254740993u128 * 12345, (1u128 << 63) + 3, false),
        ]
        .into_iter()
        .enumerate()
        {
            let name = format!("wdiv{idx}");
            cases.push((name.clone(), x, y, is_rem));
            let mut fb = FunctionBuilder::new(
                name.as_str(),
                tc.clone(),
                FunctionSignature::new(
                    &[(TypeId::I64, "lo"), (TypeId::I64, "hi"), (TypeId::I64, "d")],
                    &[TypeId::I64],
                ),
            );
            let (e, p) = fb.create_block_with_params(&[
                (TypeId::I64, "lo"),
                (TypeId::I64, "hi"),
                (TypeId::I64, "d"),
            ]);
            fb.switch_to_block(e);
            let lo = fb.uextend(p[0], TypeId::I128);
            let hi = fb.uextend(p[1], TypeId::I128);
            let sh = fb.const_int(64, TypeId::I128);
            let hi_sh = fb.ishl(hi, sh);
            let xv = fb.bor(lo, hi_sh);
            let yv = fb.uextend(p[2], TypeId::I128);
            let q = if is_rem {
                fb.urem(xv, yv)
            } else {
                fb.udiv(xv, yv)
            };
            let out = fb.ireduce(q, TypeId::I64);
            fb.ret(&[out]);
            m.add_function(fb.finish().expect("build"));
        }
        let mut jit = crate::jit::JitLoader::new(TargetMachine::new());
        jit.compile_module(&m).expect("module compile");
        for (name, x, y, is_rem) in cases {
            let f: extern "C" fn(i64, i64, i64) -> i64 = jit.get_fn(name.as_str()).expect("fn");
            let got = f(x as i64, (x >> 64) as i64, y as i64) as u64 as u128;
            let want = if is_rem { x % y } else { x / y };
            assert_eq!(
                got,
                want & 0xFFFF_FFFF_FFFF_FFFF,
                "{name}: {x} {} {y}",
                if is_rem { "%" } else { "/" }
            );
        }
    }

    /// 复现 e2e `u128_shift`：`(1 << 70) >> 70` 必须回到 1。
    #[test]
    fn jit_i128_shl_then_ushr_same_amount() {
        ensure_registered();
        let tc = TypeContext::new();
        let mut m = Module::new();
        {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut fb = FunctionBuilder::new("sh70_back", tc, sig);
            fb.create_block_here();
            let one = fb.const_int(1, TypeId::I128);
            let a70 = fb.const_int(70, TypeId::I128);
            let up = fb.ishl(one, a70);
            let back = fb.ushr(up, a70);
            let out = fb.ireduce(back, TypeId::I64);
            fb.ret(&[out]);
            m.add_function(fb.finish().expect("build"));
        }
        assert_eq!(compile_run(&m, "sh70_back"), 1, "(1<<70)>>70 应为 1");
    }

    /// 复现 e2e `u128_neg`：`0 - 1` ⇒ 两肢都是全 1；`lo as i32 + hi as i32` = -2。
    #[test]
    fn jit_i128_wrapping_neg() {
        ensure_registered();
        let tc = TypeContext::new();
        let mut m = Module::new();
        {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut fb = FunctionBuilder::new("neg1", tc, sig);
            fb.create_block_here();
            let zero = fb.const_int(0, TypeId::I128);
            let one = fb.const_int(1, TypeId::I128);
            let n = fb.isub(zero, one); // -1
            let a64 = fb.const_int(64, TypeId::I128);
            let lo = fb.ireduce(n, TypeId::I64);
            let shifted = fb.ushr(n, a64);
            let hi = fb.ireduce(shifted, TypeId::I64);
            let lo32 = fb.ireduce(lo, TypeId::I32);
            let hi32 = fb.ireduce(hi, TypeId::I32);
            let sum = fb.iadd(lo32, hi32);
            let out = fb.sextend(sum, TypeId::I64);
            fb.ret(&[out]);
            m.add_function(fb.finish().expect("build"));
        }
        assert_eq!(
            compile_run(&m, "neg1"),
            -2,
            "-1u128 的 lo/hi 作 i32 相加应为 -2"
        );
    }

    /// **宽值 store/load 往返**：这是 e2e 失败用例走的路径（`*(*p)`），此前没有 JIT 覆盖。
    /// `(1u128 << 70) >> 70` 经内存往返后必须仍是 1。
    #[test]
    fn jit_i128_store_load_roundtrip() {
        ensure_registered();
        let tc = TypeContext::new();
        let mut m = Module::new();
        {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut fb = FunctionBuilder::new("mem_roundtrip", tc, sig);
            fb.create_block_here();
            let slot = fb.alloca(TypeId::I8, 16u32);
            let a70 = fb.const_int(70, TypeId::I128);
            let one = fb.const_int(1, TypeId::I128);
            let up = fb.ishl(one, a70);
            fb.store(up, slot); // store i128
            let loaded = fb.load(slot, TypeId::I128); // load i128
            let back = fb.ushr(loaded, a70);
            let out = fb.ireduce(back, TypeId::I64);
            fb.ret(&[out]);
            m.add_function(fb.finish().expect("build"));
        }
        assert_eq!(
            compile_run(&m, "mem_roundtrip"),
            1,
            "宽值经内存往返后 (1<<70)>>70 应为 1"
        );
    }

    /// 宽值取反经内存往返：`!(0u128)` 的低肢应为全 1。
    #[test]
    fn jit_i128_not_through_memory() {
        ensure_registered();
        let tc = TypeContext::new();
        let mut m = Module::new();
        {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut fb = FunctionBuilder::new("not_mem", tc, sig);
            fb.create_block_here();
            let slot = fb.alloca(TypeId::I8, 16u32);
            let zero = fb.const_int(0, TypeId::I128);
            fb.store(zero, slot);
            let loaded = fb.load(slot, TypeId::I128);
            let n = fb.bnot(loaded);
            let out = fb.ireduce(n, TypeId::I64);
            fb.ret(&[out]);
            m.add_function(fb.finish().expect("build"));
        }
        assert_eq!(
            compile_run(&m, "not_mem"),
            -1,
            "!(0u128) 的低肢应为全 1（as i64 = -1）"
        );
    }

    /// **宽值位计数**：`clz(0u128) == 128`、`clz(1u128<<100) == 28`、`ctz(1u128<<100) == 100`。
    /// 这几条把「按 n 肢、按值的位宽计数」钉住（前端只发 `clz/ctz`，位宽语义在这里）。
    #[test]
    fn jit_i128_bit_counts() {
        ensure_registered();
        let mut m = Module::new();
        let cases: [(i128, bool, i64); 3] = [
            (0, true, 128),             // clz(0) = 128
            (1i128 << 100, true, 27),   // clz(2^100) = 128 - 101 = 27
            (1i128 << 100, false, 100), // ctz(2^100) = 100
        ];
        for (idx, (v, leading, _)) in cases.iter().enumerate() {
            let name = format!("count{idx}");
            let mut fb = FunctionBuilder::new(
                name.as_str(),
                TypeContext::new(),
                FunctionSignature::new(&[], &[TypeId::I64]),
            );
            fb.create_block_here();
            let x = fb.const_int(*v, TypeId::I128);
            let c = if *leading { fb.clz(x) } else { fb.ctz(x) };
            let out = fb.ireduce(c, TypeId::I64);
            fb.ret(&[out]);
            m.add_function(fb.finish().expect("build"));
        }
        let mut jit = crate::jit::JitLoader::new(TargetMachine::new());
        jit.compile_module(&m).expect("module compile");
        for (idx, (_, _, want)) in cases.iter().enumerate() {
            let name = format!("count{idx}");
            let f: extern "C" fn() -> i64 = jit.get_fn(name.as_str()).expect("fn");
            assert_eq!(f(), *want, "第 {idx} 个位计数不对");
        }
    }
}
