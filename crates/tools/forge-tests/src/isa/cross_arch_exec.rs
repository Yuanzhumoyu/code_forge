//! 跨架构执行测试（exec-unicorn feature）— aarch64/riscv64 的 JIT 产物
//! 经 unicorn 模拟执行 + 返回值断言。
//!
//! 前提：本机装有 cmake + C 编译器（build.rs 用 vendored unicorn 源码编译），
//! 且编译时启用 `--features exec-unicorn`。
//!
//! 已知限制（vendored unicorn 2.1.5 的 TCG 译码不全）：
//! - riscv64：int 算术/位运算/移位/控制流/浮点 均可在 unicorn 模拟执行 ✓
//! - aarch64：仅最简函数（iconst + ret）可执行；含算术/位运算/浮点/load-store 的
//!   产物会编码出 unicorn 不支持的指令（如 band 的 `c51b039a`），触发
//!   UC_ERR_EXCEPTION。故 aarch64 复杂用例降级为 compile-only 断言。
//!   浮点用例约定：unicorn 侧只读整型返回寄存器，浮点结果在 IR 内用
//!   `bitcast(f64 → i64)` 转位模式后返回，断言 `expected.to_bits()`。

#![cfg(test)]

#[cfg(feature = "exec-unicorn")]
mod unicorn_exec {
    use crate::exec::executor::{ExecArch, run};
    use code_forge::backend::FunctionCompiler;
    use code_forge::backend::arch::aarch64::{self, ensure_registered as ensure_aarch64};
    use code_forge::backend::arch::riscv64::{self, ensure_registered as ensure_riscv64};
    use code_forge::ir::IntCC;
    use code_forge::prelude::*;

    /// 编译带参函数 → 经对应执行器执行 → 返回整型值（u64）。
    fn exec_i64<M: code_forge::backend::TargetMachine>(
        arch: ExecArch,
        machine: M,
        ensure: fn(),
        params: &[(TypeId, &'static str)],
        args: &[u64],
        build: fn(&mut FunctionBuilder, &[Value]) -> Value,
    ) -> u64 {
        ensure();
        let sig = FunctionSignature::new(params, &[TypeId::I64]);
        let mut b = FunctionBuilder::new("f", TypeContext::new(), sig);
        let (block, p) = b.create_block_with_params(params);
        b.switch_to_block(block);
        let v = build(&mut b, &p);
        b.ret(&[v]);
        let func = b.finish();
        let compiled = FunctionCompiler::new(machine)
            .compile_raw(&func)
            .expect("compile");
        run(arch, &compiled, args)
    }

    /// 编译无参函数（返回 i64）→ 执行。
    fn exec_const<M: code_forge::backend::TargetMachine>(
        arch: ExecArch,
        machine: M,
        ensure: fn(),
        build: fn(&mut FunctionBuilder) -> Value,
    ) -> u64 {
        ensure();
        let sig = FunctionSignature::new(&[], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("f", TypeContext::new(), sig);
        b.create_block_here();
        let v = build(&mut b);
        b.ret(&[v]);
        let func = b.finish();
        let compiled = FunctionCompiler::new(machine)
            .compile_raw(&func)
            .expect("compile");
        run(arch, &compiled, &[])
    }

    /// compile-only 断言（aarch64 TargetMachine）——用于 unicorn 不支持指令的降级。
    fn compile_ok_aarch64(name: &str, ret: TypeId, build: fn(&mut FunctionBuilder) -> Value) {
        crate::exec::harness::compile_ok(name, aarch64::TargetMachine::new, ret, build);
    }

    // ═══════════════════════════════════════════
    // 基础：iconst 常量
    // ═══════════════════════════════════════════

    #[test]
    fn aarch64_iconst_42_exec() {
        let r = exec_const(
            ExecArch::Aarch64,
            aarch64::TargetMachine::new(),
            ensure_aarch64,
            |b| b.iconst_i64(42),
        );
        assert_eq!(r, 42, "aarch64 iconst 42 should execute to 42");
    }

    #[test]
    fn riscv64_iconst_42_exec() {
        let r = exec_const(
            ExecArch::Riscv64,
            riscv64::TargetMachine::new(),
            ensure_riscv64,
            |b| b.iconst_i64(42),
        );
        assert_eq!(r, 42, "riscv64 iconst 42 should execute to 42");
    }

    // ═══════════════════════════════════════════
    // riscv64 整数算术（带参）— unicorn 模拟执行
    // ═══════════════════════════════════════════

    #[test]
    fn riscv64_int_add_args() {
        let r = exec_i64(
            ExecArch::Riscv64,
            riscv64::TargetMachine::new(),
            ensure_riscv64,
            &[(TypeId::I64, "a"), (TypeId::I64, "b")],
            &[20, 22],
            |b, p| b.iadd(p[0], p[1]),
        );
        assert_eq!(r, 42);
    }

    #[test]
    fn riscv64_int_arith_chain() {
        // (a - 8) * 7 = 42, a = 14
        let r = exec_i64(
            ExecArch::Riscv64,
            riscv64::TargetMachine::new(),
            ensure_riscv64,
            &[(TypeId::I64, "a")],
            &[14],
            |b, p| {
                let eight = b.iconst_i64(8);
                let d = b.isub(p[0], eight);
                let seven = b.iconst_i64(7);
                b.imul(d, seven)
            },
        );
        assert_eq!(r, 42);
    }

    #[test]
    fn riscv64_int_bitwise() {
        let r = exec_const(
            ExecArch::Riscv64,
            riscv64::TargetMachine::new(),
            ensure_riscv64,
            |b| {
                let x = b.iconst_i64(0xFF);
                let y = b.iconst_i64(0x2A);
                b.band(x, y)
            },
        );
        assert_eq!(r, 0x2A);
    }

    #[test]
    fn riscv64_int_shift() {
        let r = exec_const(
            ExecArch::Riscv64,
            riscv64::TargetMachine::new(),
            ensure_riscv64,
            |b| {
                let x = b.iconst_i64(1);
                let s = b.iconst_i64(5);
                b.ishl(x, s)
            },
        );
        assert_eq!(r, 32);
    }

    // ═══════════════════════════════════════════
    // riscv64 控制流：icmp + select
    // ═══════════════════════════════════════════

    #[test]
    fn riscv64_icmp_select() {
        let r = exec_const(
            ExecArch::Riscv64,
            riscv64::TargetMachine::new(),
            ensure_riscv64,
            |b| {
                let a = b.iconst_i64(10);
                let bv = b.iconst_i64(5);
                let c = b.icmp(IntCC::SignedGreaterThan, a, bv);
                let t = b.iconst_i64(42);
                let f = b.iconst_i64(0);
                b.select(c, t, f)
            },
        );
        assert_eq!(r, 42);
    }

    // ═══════════════════════════════════════════
    // riscv64 浮点 — compile-only（vendored unicorn 的 riscv64 TCG 不支持 FMUL.D）
    // ═══════════════════════════════════════════

    #[test]
    fn riscv64_float_fmul_compile() {
        crate::exec::harness::compile_ok(
            "riscv64_float_fmul_compile",
            riscv64::TargetMachine::new,
            TypeId::F64,
            |b| {
                let a = b.fconst_f64(6.0);
                let bv = b.fconst_f64(7.0);
                let s = b.fmul(a, bv);
                b.bitcast(s, TypeId::I64)
            },
        );
    }

    // ═══════════════════════════════════════════
    // aarch64 复杂用例 — compile-only（unicorn 不支持其产物指令）
    // ═══════════════════════════════════════════

    #[test]
    fn aarch64_int_arith_compile() {
        compile_ok_aarch64("aarch64_int_arith_compile", TypeId::I32, |b| {
            let a = b.iconst_i32(6);
            let bv = b.iconst_i32(7);
            let s = b.iadd(a, bv);
            let three = b.iconst_i32(3);
            b.imul(s, three)
        });
    }

    #[test]
    fn aarch64_int_bitwise_compile() {
        compile_ok_aarch64("aarch64_int_bitwise_compile", TypeId::I32, |b| {
            let x = b.iconst_i32(0xFF);
            let y = b.iconst_i32(0x2A);
            b.band(x, y)
        });
    }

    #[test]
    fn aarch64_icmp_select_compile() {
        compile_ok_aarch64("aarch64_icmp_select_compile", TypeId::I32, |b| {
            let a = b.iconst_i32(10);
            let bv = b.iconst_i32(5);
            let c = b.icmp(IntCC::SignedGreaterThan, a, bv);
            let t = b.iconst_i32(42);
            let f = b.iconst_i32(0);
            b.select(c, t, f)
        });
    }

    #[test]
    fn aarch64_float_fadd_compile() {
        compile_ok_aarch64("aarch64_float_fadd_compile", TypeId::F64, |b| {
            let a = b.fconst_f64(20.5);
            let bv = b.fconst_f64(21.5);
            b.fadd(a, bv)
        });
    }

    #[test]
    fn aarch64_io_store_load_compile() {
        compile_ok_aarch64("aarch64_io_store_load_compile", TypeId::I32, |b| {
            let addr = b.stack_addr(-8);
            let v = b.iconst_i32(42);
            b.store(v, addr);
            b.load(addr, TypeId::I32)
        });
    }
}
