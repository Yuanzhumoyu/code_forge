//! riscv64_v12 runner：绑定 `riscv64_v12::TargetMachine` + 能力集 + QEMU
//! 执行器，运行架构无关 JIT 集成矩阵。
//!
//! 执行走 QEMU system-mode（`exec::qemu`；exit_seq 的 `sw; beq` 重试循环
//! 绕过 QEMU 11.0.92 在 jal/ret 返回路径的 MMIO 写丢失）。**值域过滤**：
//! QEMU 退出码 8 位 → |期望值| > 255 的用例 Skip（"value-range"）。QEMU
//! 缺失时 exec 为 None → 单函数用例经本机 ExecutableMemory 会崩溃（riscv
//! 产物不可本机执行）→ 此时矩阵全 Skip（qemu_exec 冒烟测试独立降级）。

/// riscv64_v12 的矩阵能力集：与 TOML `[[lowering]]` 实际实现的 op 对齐
/// （整数基础 + 内存 + 常量 + Call/递归 + 分支 + 除法/取模；无浮点/向量/
/// 溢出/位操作变体）。
pub const CAPS: &[&str] = &[
    "Iconst",
    "Iadd",
    "Isub",
    "Imul",
    "Band",
    "Bor",
    "Bxor",
    "Ishl",
    "Ushr",
    "Sshr",
    "Bnot",
    "Icmp",
    "Sdiv",
    "Udiv",
    "Srem",
    "Urem",
    "Smin",
    "Smax",
    "Umin",
    "Umax",
    "Rotl",
    "Rotr",
    "Sextend",
    "Uextend",
    "Ireduce",
    "Abs",
    "Fence",
    "Trap",
    "Freeze",
    "IsNull",
    "IsNotNull",
    "Alloca",
    "Select",
    "SaddOverflow",
    "UaddOverflow",
    "SsubOverflow",
    "UsubOverflow",
    "SmulOverflow",
    "UmulOverflow",
    "UaddSat",
    "UsubSat",
    "SaddSat",
    "SsubSat",
    "Clz",
    "Ctz",
    "Popcnt",
    "Bitreverse",
    "Nop",
    "Undef",
    "Poison",
    "GetElementPtr",
    "Fconst",
    "Fcmp",
    "Fptosi",
    "Fptoui",
    "GlobalAddr",
    "AtomicRmw",
    "Cmpxchg",
    "Copy",
    "Load",
    "Store",
    "StackAddr",
    "Call",
    "Module",
];

/// 运行全部矩阵用例；断言无 Fail（Skip 仅报告）。
#[test]
fn jit_matrix_riscv64_v12() {
    use crate::jit_matrix::{Capabilities, Outcome, Runner};
    let caps = Capabilities::new(CAPS);
    let runner = Runner {
        machine: code_forge::backend::riscv64_v12::TargetMachine::new,
        caps,
        exec: crate::exec::qemu::qemu_riscv64_path().map(|_| {
            Box::new(crate::exec::executor::QemuRiscv64Executor)
                as Box<dyn crate::exec::executor::Executor>
        }),
    };
    // QEMU 缺失 → 无执行器（矩阵全 Skip，避免本机执行 riscv 崩溃）
    let results = if runner.exec.is_some() {
        crate::jit_matrix::run_all_filtered(&runner, crate::jit_matrix::expected_fits_u8)
    } else {
        eprintln!("[riscv64] QEMU 未找到——矩阵用例全部 Skip（编译验证）");
        crate::jit_matrix::run_all_filtered(&runner, |_| false)
    };
    let mut pass = 0;
    let mut skip = 0;
    let mut fail: Vec<(String, String)> = Vec::new();
    for (name, outcome) in &results {
        match outcome {
            Outcome::Pass => pass += 1,
            Outcome::Skip(_) => skip += 1,
            Outcome::Fail(e) => fail.push((name.clone(), e.clone())),
        }
    }
    eprintln!(
        "jit_matrix riscv64_v12: {pass} passed, {skip} skipped, {} failed",
        fail.len()
    );
    for (n, e) in &fail {
        eprintln!("  FAIL {n}: {e}");
    }
    assert!(
        fail.is_empty(),
        "jit_matrix riscv64: {} failed:\n{}",
        fail.len(),
        fail.iter()
            .map(|(n, e)| format!("{n}: {e}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    // P0-18：QEMU 存在时矩阵必须真实执行（非全 Skip）——否则 CI 上
    // riscv 执行正确性回归无人守门（Skip 掩盖实现倒退）。
    if runner.exec.is_some() {
        assert!(
            pass > 0,
            "jit_matrix riscv64: QEMU 已安装但全部 Skip（{} skip）——执行验证未发生",
            skip
        );
    }
}

// ─────────────────────── QEMU 直接执行冒烟 ───────────────────────
// （helpers 与测试仅在 test profile 编译，避免 lib 构建的 dead_code 警告）

#[cfg(test)]
mod smoke {
    fn compile_binop(
        name: &str,
        build: impl FnOnce(
            &mut crate::prelude::FunctionBuilder,
            &[code_forge::prelude::Value],
        ) -> code_forge::prelude::Value,
    ) -> code_forge::CompiledFunction {
        use code_forge::prelude::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};
        let sig = FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
        let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
        b.switch_to_block(entry);
        let v = build(&mut b, &params);
        b.ret(&[v]);
        let func = b.finish().expect("build");
        code_forge::backend::FunctionCompiler::new(
            code_forge::backend::riscv64_v12::TargetMachine::new(),
        )
        .compile_raw(&func)
        .expect("compile")
    }

    fn compile_const(name: &str, v: i64) -> code_forge::CompiledFunction {
        use code_forge::prelude::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};
        let sig = FunctionSignature::new(&[], &[TypeId::I64]);
        let mut b = FunctionBuilder::new(name, TypeContext::new(), sig);
        b.create_block_here();
        let c = b.iconst_i64(v);
        b.ret(&[c]);
        let func = b.finish().expect("build");
        code_forge::backend::FunctionCompiler::new(
            code_forge::backend::riscv64_v12::TargetMachine::new(),
        )
        .compile_raw(&func)
        .expect("compile const")
    }

    /// QEMU 执行；无 QEMU → None（仅编译验证，不失败）。
    fn qemu_exec(cf: &code_forge::CompiledFunction, args: &[u64]) -> Option<u64> {
        if crate::exec::qemu::qemu_riscv64_path().is_none() {
            eprintln!("[riscv64] QEMU 未找到——跳过执行（仅编译验证）");
            return None;
        }
        Some(crate::exec::qemu::exec_riscv64(cf, args).unwrap_or_else(|e| panic!("qemu exec: {e}")))
    }

    #[test]
    fn qemu_exec_const42() {
        let cf = compile_const("c42", 42);
        if let Some(code) = qemu_exec(&cf, &[]) {
            assert_eq!(code, 42, "QEMU const42 应返回 42");
        }
    }

    /// 参数化用例的编译验证（QEMU 执行受 11.0.92 sifive_test 怪癖限制）：
    /// 断言编译产物含 ADD（opcode 0x33）与 RET（0x8067）。
    #[test]
    fn compile_add_smoke() {
        let cf = compile_binop("add", |b, p| b.iadd(p[0], p[1]));
        let has_add = cf
            .code
            .windows(4)
            .any(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) & 0x7F == 0x33);
        let has_ret = cf
            .code
            .windows(4)
            .any(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) == 0x0000_8067);
        assert!(has_add, "add 编译产物应含 ADD (0x33): {:02x?}", cf.code);
        assert!(has_ret, "add 编译产物应含 RET: {:02x?}", cf.code);
    }

    /// 参数化算术（sub+imul）的编译验证。
    #[test]
    fn compile_sub_mul_smoke() {
        let cf = compile_binop("sub_mul", |b, p| {
            let five = b.iconst_i64(5);
            let three = b.iconst_i64(3);
            let d = b.isub(p[0], five);
            b.imul(d, three)
        });
        // SUB opcode 0x33（funct3=0, funct7=0x20）、MUL opcode 0x33（funct7=0x01）
        let has_mul = cf.code.windows(4).any(|c| {
            let w = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            w & 0x7F == 0x33 && (w >> 25) & 0x7F == 0x01
        });
        assert!(
            has_mul,
            "sub_mul 编译产物应含 MUL (funct7=1): {:02x?}",
            cf.code
        );
    }
}
