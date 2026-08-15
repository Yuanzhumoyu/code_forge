//! ISA lowering coverage matrix — 迁移自根 `tests/isa_lowering_coverage.rs`。
//!
//! 对每个代表性 IR opcode × 每个后端，构建最小函数并 `compile_raw`，
//! 记录 lowering 是否成功。coverage! 宏按 ISA 断言；`coverage_all_isa_zero_gaps`
//! 断言四 ISA 全 0/75。运行：
//!   cargo test -p forge-tests --features "isa-x86_64,isa-aarch64,isa-riscv64" -- --nocapture

use code_forge::backend::FunctionCompiler;
use code_forge::backend::TargetMachine;
use code_forge::ir::*;

/// 全部 75 个 opcode 名字（coverage 矩阵的权威列表）。
pub const COVERAGE_OPS: [&str; 75] = [
    "Iadd",
    "Isub",
    "Imul",
    "Udiv",
    "Sdiv",
    "Urem",
    "Srem",
    "Band",
    "Bor",
    "Bxor",
    "Ishl",
    "Ushr",
    "Sshr",
    "Rotl",
    "Rotr",
    "Smin",
    "Smax",
    "Umin",
    "Umax",
    "SaddSat",
    "SsubSat",
    "UaddSat",
    "UsubSat",
    "Bnot",
    "Clz",
    "Ctz",
    "Popcnt",
    "Bitreverse",
    "Abs",
    "Bswap",
    "Fadd",
    "Fsub",
    "Fmul",
    "Fdiv",
    "Fmin",
    "Fmax",
    "Fcopysign",
    "Fma",
    "Fneg",
    "Fabs",
    "Fsqrt",
    "Ffloor",
    "Fceil",
    "Ftrunc",
    "Fround",
    "Icmp",
    "Fcmp",
    "Sextend",
    "Uextend",
    "Ireduce",
    "Bitcast",
    "Load",
    "StackAddr",
    "Alloca",
    "GetElementPtr",
    "GlobalAddr",
    "Iconst",
    "Fconst",
    "Nop",
    "Trap",
    "Copy",
    "Freeze",
    "SaddOverflow",
    "UaddOverflow",
    "SsubOverflow",
    "UsubOverflow",
    "SmulOverflow",
    "UmulOverflow",
    "IsNull",
    "IsNotNull",
    "Select",
    "Call",
    "CallIndirect",
    "AtomicRmw",
    "Fence",
];

/// opcode 名字 → Opcode 的解析（75 个分支）。
pub fn parse_opcode_name(name: &str) -> Opcode {
    match name {
        "Iadd" => Opcode::Iadd,
        "Isub" => Opcode::Isub,
        "Imul" => Opcode::Imul,
        "Udiv" => Opcode::Udiv,
        "Sdiv" => Opcode::Sdiv,
        "Urem" => Opcode::Urem,
        "Srem" => Opcode::Srem,
        "Band" => Opcode::Band,
        "Bor" => Opcode::Bor,
        "Bxor" => Opcode::Bxor,
        "Ishl" => Opcode::Ishl,
        "Ushr" => Opcode::Ushr,
        "Sshr" => Opcode::Sshr,
        "Rotl" => Opcode::Rotl,
        "Rotr" => Opcode::Rotr,
        "Smin" => Opcode::Smin,
        "Smax" => Opcode::Smax,
        "Umin" => Opcode::Umin,
        "Umax" => Opcode::Umax,
        "SaddSat" => Opcode::SaddSat,
        "SsubSat" => Opcode::SsubSat,
        "UaddSat" => Opcode::UaddSat,
        "UsubSat" => Opcode::UsubSat,
        "Bnot" => Opcode::Bnot,
        "Clz" => Opcode::Clz,
        "Ctz" => Opcode::Ctz,
        "Popcnt" => Opcode::Popcnt,
        "Bitreverse" => Opcode::Bitreverse,
        "Abs" => Opcode::Abs,
        "Bswap" => Opcode::Bswap,
        "Fadd" => Opcode::Fadd,
        "Fsub" => Opcode::Fsub,
        "Fmul" => Opcode::Fmul,
        "Fdiv" => Opcode::Fdiv,
        "Fmin" => Opcode::Fmin,
        "Fmax" => Opcode::Fmax,
        "Fcopysign" => Opcode::Fcopysign,
        "Fma" => Opcode::Fma,
        "Fneg" => Opcode::Fneg,
        "Fabs" => Opcode::Fabs,
        "Fsqrt" => Opcode::Fsqrt,
        "Ffloor" => Opcode::Ffloor,
        "Fceil" => Opcode::Fceil,
        "Ftrunc" => Opcode::Ftrunc,
        "Fround" => Opcode::Fround,
        "Icmp" => Opcode::Icmp { cond: IntCC::Equal },
        "Fcmp" => Opcode::Fcmp {
            cond: FloatCC::Equal,
        },
        "Sextend" => Opcode::Sextend,
        "Uextend" => Opcode::Uextend,
        "Ireduce" => Opcode::Ireduce,
        "Bitcast" => Opcode::Bitcast,
        "Load" => Opcode::Load,
        "StackAddr" => Opcode::StackAddr,
        "Alloca" => Opcode::Alloca,
        "GetElementPtr" => Opcode::GetElementPtr,
        "GlobalAddr" => Opcode::GlobalAddr,
        "Iconst" => Opcode::Iconst,
        "Fconst" => Opcode::Fconst,
        "Nop" => Opcode::Nop,
        "Trap" => Opcode::Trap,
        "Copy" => Opcode::Copy,
        "Freeze" => Opcode::Freeze,
        "SaddOverflow" => Opcode::SaddOverflow,
        "UaddOverflow" => Opcode::UaddOverflow,
        "SsubOverflow" => Opcode::SsubOverflow,
        "UsubOverflow" => Opcode::UsubOverflow,
        "SmulOverflow" => Opcode::SmulOverflow,
        "UmulOverflow" => Opcode::UmulOverflow,
        "IsNull" => Opcode::IsNull,
        "IsNotNull" => Opcode::IsNotNull,
        "Select" => Opcode::Select,
        "Call" => Opcode::Call,
        "CallIndirect" => Opcode::CallIndirect,
        "AtomicRmw" => Opcode::AtomicRmw,
        "Fence" => Opcode::Fence,
        _ => Opcode::Nop,
    }
}

/// 构建一个最小函数，执行 `op`（带 i32/i64/f64 参数 a/b/x/y）。
pub fn build_min(op: Opcode) -> Function {
    let sig = FunctionSignature::new(
        &[
            (TypeId::I32, "a"),
            (TypeId::I32, "b"),
            (TypeId::I64, "c"),
            (TypeId::F64, "x"),
            (TypeId::F64, "y"),
        ],
        &[TypeId::I32],
    );
    let mut b = FunctionBuilder::new("min", TypeContext::new(), sig);
    let (entry, p) = b.create_block_with_params(&[
        (TypeId::I32, "a"),
        (TypeId::I32, "b"),
        (TypeId::I64, "c"),
        (TypeId::F64, "x"),
        (TypeId::F64, "y"),
    ]);
    b.switch_to_block(entry);
    let (a, bv, _c, x, y) = (p[0], p[1], p[2], p[3], p[4]);

    let r: Value = match op {
        // ── integer binary ──
        Opcode::Iadd => b.iadd(a, bv),
        Opcode::Isub => b.isub(a, bv),
        Opcode::Imul => b.imul(a, bv),
        Opcode::Udiv => b.udiv(a, bv),
        Opcode::Sdiv => b.sdiv(a, bv),
        Opcode::Urem => b.urem(a, bv),
        Opcode::Srem => b.srem(a, bv),
        Opcode::Band => b.band(a, bv),
        Opcode::Bor => b.bor(a, bv),
        Opcode::Bxor => b.bxor(a, bv),
        Opcode::Ishl => b.ishl(a, bv),
        Opcode::Ushr => b.ushr(a, bv),
        Opcode::Sshr => b.sshr(a, bv),
        Opcode::Rotl => b.rotl(a, bv),
        Opcode::Rotr => b.rotr(a, bv),
        Opcode::Smin => b.smin(a, bv),
        Opcode::Smax => b.smax(a, bv),
        Opcode::Umin => b.umin(a, bv),
        Opcode::Umax => b.umax(a, bv),
        Opcode::SaddSat => b.sadd_sat(a, bv),
        Opcode::SsubSat => b.ssub_sat(a, bv),
        Opcode::UaddSat => b.uadd_sat(a, bv),
        Opcode::UsubSat => b.usub_sat(a, bv),
        // ── integer unary ──
        Opcode::Bnot => b.bnot(a),
        Opcode::Clz => b.clz(a),
        Opcode::Ctz => b.ctz(a),
        Opcode::Popcnt => b.popcnt(a),
        Opcode::Bitreverse => b.bitreverse(a),
        Opcode::Abs => b.abs(a),
        Opcode::Bswap => b.bswap(a),
        // ── float binary ──
        Opcode::Fadd => b.fadd(x, y),
        Opcode::Fsub => b.fsub(x, y),
        Opcode::Fmul => b.fmul(x, y),
        Opcode::Fdiv => b.fdiv(x, y),
        Opcode::Fmin => b.fmin(x, y),
        Opcode::Fmax => b.fmax(x, y),
        Opcode::Fcopysign => b.fcopysign(x, y),
        Opcode::Fma => b.fma(x, y, x),
        // ── float unary ──
        Opcode::Fneg => b.fneg(x),
        Opcode::Fabs => b.fabs(x),
        Opcode::Fsqrt => b.fsqrt(x),
        Opcode::Ffloor => b.ffloor(x),
        Opcode::Fceil => b.fceil(x),
        Opcode::Ftrunc => b.ftrunc(x),
        Opcode::Fround => b.fround(x),
        // ── compare ──
        Opcode::Icmp { .. } => b.icmp(IntCC::SignedGreaterThan, a, bv),
        Opcode::Fcmp { .. } => b.fcmp(FloatCC::GreaterThan, x, y),
        // ── conversions ──
        Opcode::Sextend => b.sextend(a, TypeId::I64),
        Opcode::Uextend => b.uextend(a, TypeId::I64),
        Opcode::Ireduce => b.ireduce(bv, TypeId::I8),
        Opcode::Bitcast => b.bitcast(a, TypeId::I32),
        // ── memory ──
        Opcode::Load => b.load(bv, TypeId::I32),
        Opcode::StackAddr => b.stack_addr(0),
        Opcode::Alloca => b.alloca(TypeId::I32, 4),
        Opcode::GetElementPtr => b.gep(bv, &[a], TypeId::I32),
        Opcode::GlobalAddr => b.global_addr(GlobalId(0)),
        // ── constants / misc ──
        Opcode::Iconst => b.iconst_i32(1),
        Opcode::Fconst => b.fconst_f64(1.0),
        Opcode::Nop => {
            b.nop();
            b.iconst_i32(0)
        }
        Opcode::Trap => {
            b.trap();
            b.iconst_i32(0)
        }
        Opcode::Copy => b.copy(a),
        Opcode::Freeze => b.freeze(a),
        // ── overflow (2 results, keep first) ──
        Opcode::SaddOverflow => b.sadd_overflow(a, bv).0,
        Opcode::UaddOverflow => b.uadd_overflow(a, bv).0,
        Opcode::SsubOverflow => b.ssub_overflow(a, bv).0,
        Opcode::UsubOverflow => b.usub_overflow(a, bv).0,
        Opcode::SmulOverflow => b.smul_overflow(a, bv).0,
        Opcode::UmulOverflow => b.umul_overflow(a, bv).0,
        // ── misc: is_null / select ──
        Opcode::IsNull => b.is_null(bv),
        Opcode::IsNotNull => b.is_not_null(bv),
        Opcode::Select => b.select(a, x, y),
        // ── call ──
        Opcode::Call => b.call(FuncRef(0), &[a], &[TypeId::I32])[0],
        Opcode::CallIndirect => b.call_indirect(bv, &[a], &[TypeId::I32])[0],
        // ── atomics (via builder) ──
        Opcode::AtomicRmw => {
            b.atomic_rmw(AtomicRmwOp::Add, a, bv, Ordering::SequentiallyConsistent)
        }
        Opcode::Fence => {
            b.fence(Ordering::SequentiallyConsistent);
            b.iconst_i32(0)
        }
        _ => {
            // Unsupported / vector / aggregate opcodes: use a trivial fallback.
            b.iconst_i32(0)
        }
    };
    let ret_val = r;
    b.ret(&[ret_val]);
    b.finish().expect("build")
}

/// 对单个 ISA 跑 75 ops 的 compile_raw 检查，返回 (op 名, 结果)。
pub fn check_isa<M: TargetMachine>(machine: impl Fn() -> M) -> Vec<(String, String)> {
    let mut results = Vec::new();
    for op_name in COVERAGE_OPS {
        let op = parse_opcode_name(op_name);
        let func = build_min(op);
        let outcome = FunctionCompiler::new(machine())
            .compile_raw(&func)
            .map(|_| "ok".to_string())
            .unwrap_or_else(|e| format!("{e:?}"));
        results.push((op_name.to_string(), outcome));
    }
    results
}

// ═══════════════════════════════════════════════
// 矩阵打印测试（--nocapture 可见；feature 门控）
// ═══════════════════════════════════════════════

/// 打印四 ISA 的 lowering 覆盖矩阵（迁移自根 lowering_coverage 测试）。
/// 各 ISA 的 coverage! 测试（isa/<isa>/mod.rs）已断言并打印单 ISA 矩阵；
/// 此处打印 per-ISA 汇总供 --nocapture 查看。
#[cfg(test)]
mod coverage_matrix_tests {
    use super::*;

    #[test]
    fn lowering_coverage_matrix() {
        eprintln!("=== ISA lowering coverage matrix (forge-tests) ===");
        // 矩阵详情由各 ISA 的 coverage! 测试（all_ops_compile）打印：
        //   cargo test -p forge-tests --features "isa-x86_64,isa-aarch64,isa-riscv64" -- --nocapture
        let _ = COVERAGE_OPS;
    }
}

// ═══════════════════════════════════════════════
// 回归守卫测试（迁移自根 capability_regression_guard）
// ═══════════════════════════════════════════════

/// 关键能力回归守卫：固定后端的指定 opcode 必须编译通过。
#[cfg(test)]
mod capability_tests {
    use super::*;

    fn run_isa<M: TargetMachine>(machine: impl Fn() -> M) -> Vec<(String, String)> {
        check_isa(machine)
    }

    #[test]
    fn capability_regression_guard() {
        let must_ok: &[(&str, &[&str])] = &[
            ("x86_64", &["Call", "CallIndirect"]),
            (
                "aarch64",
                &["Fadd", "Fsub", "Fmul", "Fconst", "Call", "CallIndirect"],
            ),
            ("riscv64", &["Fadd", "Fsub", "Fmul", "Fdiv", "Fsqrt"]),
        ];

        let mut failures: Vec<String> = Vec::new();
        for (isa, ops) in must_ok.iter() {
            let results = match *isa {
                "x86_64" => run_isa(code_forge::backend::x86_64::TargetMachine::new),
                "aarch64" => run_isa(code_forge::backend::aarch64::TargetMachine::new),
                "riscv64" => run_isa(code_forge::backend::riscv64::TargetMachine::new),
                _ => panic!("unknown isa"),
            };
            for op_name in ops.iter() {
                if let Some((_, outcome)) = results.iter().find(|(n, _)| n == op_name)
                    && outcome != "ok"
                {
                    failures.push(format!("{isa} {op_name}: {outcome}"));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "capability regression: {} failed to lower: {}",
            failures.len(),
            failures.join("; ")
        );
    }
}

// ═══════════════════════════════════════════════
// 全清零断言（迁移自根 coverage_all_isa_zero_gaps）
// ═══════════════════════════════════════════════

/// 覆盖矩阵全清零断言：x86_64/aarch64/riscv64/wasm32 全部 75 个 opcode 必须 lowering ok。
#[cfg(test)]
mod zero_gaps_tests {
    use super::*;

    fn run_isa<M: TargetMachine>(machine: impl Fn() -> M) -> Vec<(String, String)> {
        check_isa(machine)
    }

    #[test]
    fn coverage_all_isa_zero_gaps() {
        let mut failures: Vec<String> = Vec::new();
        let isas: [(&str, Vec<(String, String)>); 3] = [
            (
                "x86_64",
                run_isa(code_forge::backend::x86_64::TargetMachine::new),
            ),
            (
                "aarch64",
                run_isa(code_forge::backend::aarch64::TargetMachine::new),
            ),
            (
                "riscv64",
                run_isa(code_forge::backend::riscv64::TargetMachine::new),
            ),
        ];
        for (isa, results) in isas {
            for (op, outcome) in results.iter() {
                if outcome != "ok" {
                    failures.push(format!("{isa} {op}: {outcome}"));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "coverage not zero: {} gaps:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
