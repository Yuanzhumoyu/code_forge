//! x86_v12 runner：绑定 `x86_v12::TargetMachine` + 能力集，运行架构无关
//! JIT 集成矩阵。能力集 = v12 TOML 当前声明的 lowering op；未实现 op 的
//! 用例自动 Skip（后续阶段补齐后转绿，Skip 数收敛到 0）。

/// v12 x86 当前声明的 lowering op（来源：isa/x86_v12.toml [[lowering]]）。
pub const CAPS: &[&str] = &[
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
    "Bnot",
    "Icmp",
    "Sextend",
    "Copy",
    "Load",
    "StackAddr",
    "Iconst",
    "Store",
    // 迭代 6 续：整数全量（when 谓词接线后补齐）
    "Select",
    "Uextend",
    "Ireduce",
    "Rotl",
    "Rotr",
    "Smin",
    "Smax",
    "Umin",
    "Umax",
    "Abs",
    "Clz",
    "Ctz",
    "Popcnt",
    "Bswap",
    "IsNull",
    "IsNotNull",
    "Freeze",
    "Trap",
    "Fence",
    "Alloca",
    "SaddOverflow",
    "UaddOverflow",
    "SsubOverflow",
    "UsubOverflow",
    "SmulOverflow",
    "UmulOverflow",
    "SaddSat",
    "UaddSat",
    "UsubSat",
    "Bitreverse",
    // 迭代 7（Phase 4）：标量浮点
    "Fadd",
    "Fsub",
    "Fmul",
    "Fdiv",
    "Fneg",
    "Fabs",
    "Fsqrt",
    "Fcmp",
    "Fconst",
    "Fload",
    "Fstore",
    "Fmin",
    "Fmax",
    "Ffloor",
    "Fceil",
    "Ftrunc",
    "Fround",
    "Fpext",
    "Fptrunc",
    "Sitofp",
    "Fptosi",
    "Fma",
    "Fcopysign",
    "Uitofp",
    "Fptoui",
    // 迭代 8（Phase 5）：函数调用
    "Call",
    "CallIndirect",
    // 迭代 9（Phase 6）：向量
    "Vadd",
    "Vsub",
    "Vmul",
    "Vdiv",
    "Vneg",
    "Vabs",
    "Vextract",
    "Vinsert",
    "Vbitcast",
    "Vbroadcast",
    "Vsplit",
    "Vconcat",
    "ShuffleVector",
    "Vconst",
    // 迭代 10（Phase 7 收尾）：缺口补齐
    "SsubSat",
    "Ptrtoint",
    "Inttoptr",
    "Undef",
    "Poison",
    "GlobalAddr",
    "Frem",
    "AtomicRmw",
    "Cmpxchg",
    "Nop",
    "GetElementPtr",
];

/// 运行全部矩阵用例；断言无 Fail（Skip 仅报告）。
#[test]
fn jit_matrix_x86_v12() {
    use crate::jit_matrix::{Capabilities, Outcome, Runner};
    let mut caps = Capabilities::new(CAPS);
    // AVX 可用 → 标记 "AVX" 伪能力（V256 用例门控；无 AVX 机器自动 Skip）
    if code_forge::backend::avx_available() {
        caps.ops.insert("AVX");
    }
    let runner = Runner {
        machine: code_forge::backend::x86_v12::TargetMachine::new,
        caps,
        exec: None, // 本机 ExecutableMemory 快路径
    };
    let results = crate::jit_matrix::run_all(&runner);
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
        "jit_matrix x86_v12: {pass} passed, {skip} skipped, {} failed",
        fail.len()
    );
    for (n, e) in &fail {
        eprintln!("  FAIL {n}: {e}");
    }
    assert!(
        fail.is_empty(),
        "jit_matrix: {} failed:\n{}",
        fail.len(),
        fail.iter()
            .map(|(n, e)| format!("{n}: {e}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
