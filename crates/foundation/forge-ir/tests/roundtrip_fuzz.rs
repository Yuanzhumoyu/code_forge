//! round-trip fuzz：随机生成**合法** LLVM IR 文本 → parse₁ → display → parse₂，
//! 断言两次 forge IR 结构等价（模块/块/指令/终结符/常量池值）且无 panic。
//!
//! 生成器只产出文本层支持的类型/指令/常量（全部按比例抽样），SSA 顺序定义、
//! 类型匹配、终结符在块尾等合法性约束由生成器内部保证。
//!
//! 种子固定（默认 `0x9E3779B97F4A7C15`），可用环境变量覆盖保证可复现：
//! - `FORGE_FUZZ_SEED`：种子（十进制或 `0x` 十六进制）
//! - `FORGE_FUZZ_ITERS`：模块数（默认 10_000）
//!
//! 10k 随机模块 round-trip 0 失败；发现的 bug 应固定为回归测试。

use forge_ir::dfg::BlockData;
use forge_ir::function::{Function, Module};
use forge_ir::immediate::Immediate;
use forge_ir::ir_parser::parse_module;
use std::collections::HashMap;

// ── 确定性 PRNG：xorshift64*（固定种子可复现，不依赖 rand 版本行为）──

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
}

// ── 类型（文本层子集：标量 + ptr，覆盖算术/转换/内存/调用全路径）──

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Ty {
    I1,
    I8,
    I32,
    I64,
    F32,
    F64,
    Ptr,
}

impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::I1 => "i1",
            Ty::I8 => "i8",
            Ty::I32 => "i32",
            Ty::I64 => "i64",
            Ty::F32 => "f32",
            Ty::F64 => "f64",
            Ty::Ptr => "ptr",
        }
    }
}

#[derive(Clone)]
struct FuncSig {
    name: String,
    ret: Option<Ty>, // None = void
    params: Vec<Ty>,
}

struct GenCtx {
    rng: Rng,
    /// 当前函数作用域内按类型分组的 SSA 值名（函数结束时恢复）
    by_ty: HashMap<Ty, Vec<String>>,
    /// 当前函数作用域内的块名（函数结束时恢复）
    blocks: Vec<String>,
    /// 已生成的函数签名（call 只引用已生成者，避免 forward reference）
    funcs: Vec<FuncSig>,
    next: usize,
}

impl GenCtx {
    fn new(seed: u64) -> Self {
        GenCtx {
            rng: Rng::new(seed),
            by_ty: HashMap::new(),
            blocks: Vec::new(),
            funcs: Vec::new(),
            next: 0,
        }
    }
    fn fresh(&mut self, prefix: &str) -> String {
        let s = format!("{prefix}{}", self.next);
        self.next += 1;
        s
    }
    fn push_val(&mut self, ty: Ty, name: String) {
        self.by_ty.entry(ty).or_default().push(name);
    }
    fn pick_val(&mut self, ty: Ty) -> Option<String> {
        let pool = self.by_ty.get(&ty)?;
        if pool.is_empty() {
            return None;
        }
        Some(pool[self.rng.below(pool.len() as u64) as usize].clone())
    }
    fn pick_block(&mut self) -> String {
        self.blocks[self.rng.below(self.blocks.len() as u64) as usize].clone()
    }
}

// ── 操作数生成 ──
// 格式约定（与 display/parser 配套）：
// - binop 式指令（算术/icmp/fcmp）：op1 裸值（`%v`/`5`/`undef`），op2 带类型前缀（`i32 %v`/`i32 5`）
// - select/ret/转换/call 参数/store 值：一律 `ty value`（`i32 %v`/`i32 5`/`f64 1.5`/`ptr %p`）

fn int_raw(ctx: &mut GenCtx, ty: Ty) -> String {
    // 位宽内裸整数值（`i8 300` 是非法 LLVM 文本，必须取模）
    let raw = ctx.rng.below(1 << 20);
    let bits = match ty {
        Ty::I1 => 1,
        Ty::I8 => 8,
        Ty::I32 => 32,
        _ => 64,
    };
    let v = if bits >= 64 {
        raw
    } else {
        raw & ((1u64 << bits) - 1)
    };
    v.to_string()
}

fn int_const(ctx: &mut GenCtx, ty: Ty) -> String {
    format!("{} {}", ty.name(), int_raw(ctx, ty))
}

const FLOATS: &[&str] = &[
    "0.0", "1.0", "-1.0", "0.5", "-0.25", "2.75", "1.5e3", "-2.5e-2",
];

fn int_operand_value(ctx: &mut GenCtx, ty: Ty) -> String {
    // 裸整数值/undef/poison（binop op1 与带类型形式共用取值）
    match ctx.rng.below(8) {
        0 => "undef".to_string(),
        1 => "poison".to_string(),
        _ => int_raw(ctx, ty),
    }
}

/// binop 第一操作数：裸值（`%v` 或 `5`/`undef`/`poison`；浮点为 `1.5` 等）
fn pick_operand_bare(ctx: &mut GenCtx, ty: Ty) -> String {
    if let Some(v) = ctx.pick_val(ty)
        && ctx.rng.chance(60)
    {
        return format!("%{v}");
    }
    match ty {
        Ty::F32 | Ty::F64 => (*ctx.rng.pick(FLOATS)).to_string(),
        Ty::Ptr => {
            // ptr 无常量：退回池值
            match ctx.pick_val(Ty::Ptr) {
                Some(v) => format!("%{v}"),
                None => "poison".to_string(),
            }
        }
        t => int_operand_value(ctx, t),
    }
}

/// binop 第二操作数：带类型前缀（`i32 %v` / `i32 5` / `f64 1.5` / `i32 undef`）
fn pick_operand_typed(ctx: &mut GenCtx, ty: Ty) -> String {
    if let Some(v) = ctx.pick_val(ty)
        && ctx.rng.chance(60)
    {
        return format!("{} %{v}", ty.name());
    }
    match ty {
        Ty::F32 | Ty::F64 => format!("{} {}", ty.name(), ctx.rng.pick(FLOATS)),
        Ty::Ptr => match ctx.pick_val(Ty::Ptr) {
            Some(v) => format!("ptr %{v}"),
            None => "ptr poison".to_string(),
        },
        t => {
            let val = match ctx.rng.below(8) {
                0 => "undef".to_string(),
                1 => "poison".to_string(),
                _ => int_raw(ctx, t),
            };
            format!("{} {val}", t.name())
        }
    }
}

/// 任意类型操作数（select/ret/load 值/call 参数/转换源）：一律 `ty value`。
/// 常量兜底保证总有值；ptr 无常量、池空时返回 None（上层放弃该指令）。
fn pick_typed_operand(ctx: &mut GenCtx, ty: Ty) -> Option<String> {
    if let Some(v) = ctx.pick_val(ty)
        && ctx.rng.chance(60)
    {
        return Some(format!("{} %{v}", ty.name()));
    }
    match ty {
        Ty::Ptr => ctx.pick_val(Ty::Ptr).map(|v| format!("ptr %{v}")),
        Ty::F32 | Ty::F64 => Some(format!("{} {}", ty.name(), ctx.rng.pick(FLOATS))),
        t => Some(match ctx.rng.below(8) {
            0 => format!("{} undef", t.name()),
            1 => format!("{} poison", t.name()),
            _ => int_const(ctx, t),
        }),
    }
}

// ── 指令生成 ──

fn gen_int_binop(ctx: &mut GenCtx) -> Option<String> {
    let op = *ctx.rng.pick(&[
        "add", "sub", "mul", "udiv", "sdiv", "urem", "srem", "and", "or", "xor", "shl", "lshr",
        "ashr",
    ]);
    let ty = *ctx.rng.pick(&[Ty::I8, Ty::I32, Ty::I64]);
    let a = pick_operand_bare(ctx, ty);
    let b = pick_operand_typed(ctx, ty);
    let r = ctx.fresh("r");
    ctx.push_val(ty, r.clone());
    Some(format!("    %{r} = {op} {} {a}, {b}", ty.name()))
}

fn gen_icmp(ctx: &mut GenCtx) -> Option<String> {
    let pred = *ctx.rng.pick(&[
        "eq", "ne", "ugt", "uge", "ult", "ule", "sgt", "sge", "slt", "sle",
    ]);
    let ty = *ctx.rng.pick(&[Ty::I8, Ty::I32, Ty::I64]);
    let a = pick_operand_bare(ctx, ty);
    let b = pick_operand_typed(ctx, ty);
    let r = ctx.fresh("r");
    ctx.push_val(Ty::I1, r.clone());
    Some(format!("    %{r} = icmp {pred} {} {a}, {b}", ty.name()))
}

fn gen_float_binop(ctx: &mut GenCtx) -> Option<String> {
    let op = *ctx.rng.pick(&["fadd", "fsub", "fmul", "fdiv"]);
    let ty = *ctx.rng.pick(&[Ty::F32, Ty::F64]);
    let a = pick_operand_bare(ctx, ty);
    let b = pick_operand_typed(ctx, ty);
    let r = ctx.fresh("r");
    ctx.push_val(ty, r.clone());
    Some(format!("    %{r} = {op} {} {a}, {b}", ty.name()))
}

fn gen_fcmp(ctx: &mut GenCtx) -> Option<String> {
    let pred = *ctx
        .rng
        .pick(&["oeq", "olt", "ole", "ogt", "oge", "one", "ord", "uno"]);
    let ty = *ctx.rng.pick(&[Ty::F32, Ty::F64]);
    let a = pick_operand_bare(ctx, ty);
    let b = pick_operand_typed(ctx, ty);
    let r = ctx.fresh("r");
    ctx.push_val(Ty::I1, r.clone());
    Some(format!("    %{r} = fcmp {pred} {} {a}, {b}", ty.name()))
}

fn gen_conv(ctx: &mut GenCtx) -> Option<String> {
    let (op, src, dst) = match ctx.rng.below(9) {
        0 => {
            let s = *ctx.rng.pick(&[Ty::I1, Ty::I8, Ty::I32]);
            let op = *ctx.rng.pick(&["sext", "zext"]);
            let d = match s {
                Ty::I1 => *ctx.rng.pick(&[Ty::I8, Ty::I32, Ty::I64]),
                Ty::I8 => *ctx.rng.pick(&[Ty::I32, Ty::I64]),
                _ => Ty::I64,
            };
            (op, s, d)
        }
        1 => {
            let s = *ctx.rng.pick(&[Ty::I64, Ty::I32]);
            let d = if s == Ty::I64 {
                *ctx.rng.pick(&[Ty::I8, Ty::I32])
            } else {
                Ty::I8
            };
            ("trunc", s, d)
        }
        2 => {
            let s = *ctx.rng.pick(&[Ty::I8, Ty::I32, Ty::I64]);
            let d = *ctx.rng.pick(&[Ty::F32, Ty::F64]);
            let op = *ctx.rng.pick(&["sitofp", "uitofp"]);
            (op, s, d)
        }
        3 => {
            let s = *ctx.rng.pick(&[Ty::F32, Ty::F64]);
            let d = *ctx.rng.pick(&[Ty::I8, Ty::I32, Ty::I64]);
            let op = *ctx.rng.pick(&["fptosi", "fptoui"]);
            (op, s, d)
        }
        4 => ("fpext", Ty::F32, Ty::F64),
        5 => ("fptrunc", Ty::F64, Ty::F32),
        6 => ("ptrtoint", Ty::Ptr, Ty::I64),
        7 => ("inttoptr", Ty::I64, Ty::Ptr),
        _ => {
            let (s, d) = *ctx.rng.pick(&[
                (Ty::F64, Ty::I64),
                (Ty::I64, Ty::F64),
                (Ty::F32, Ty::I32),
                (Ty::I32, Ty::F32),
            ]);
            ("bitcast", s, d)
        }
    };
    // 转换源是裸值（`%v`/`5`/`1.5`）；格式 `sext i8 3 to i64`（类型在前，与 display 一致）
    let a = pick_operand_bare(ctx, src);
    let r = ctx.fresh("r");
    ctx.push_val(dst, r.clone());
    Some(format!(
        "    %{r} = {op} {} {a} to {}",
        src.name(),
        dst.name()
    ))
}

fn gen_select(ctx: &mut GenCtx) -> Option<String> {
    // 条件必须是 i1；池空则退化为生成一条 icmp（其结果可被后续指令使用）
    let c = match ctx.pick_val(Ty::I1) {
        Some(v) => v,
        None => return gen_icmp(ctx),
    };
    let ty = *ctx.rng.pick(&[Ty::I32, Ty::I64, Ty::F32, Ty::F64, Ty::Ptr]);
    let a = pick_typed_operand(ctx, ty)?;
    let b = pick_typed_operand(ctx, ty)?;
    let r = ctx.fresh("r");
    ctx.push_val(ty, r.clone());
    Some(format!("    %{r} = select i1 %{c}, {a}, {b}"))
}

fn gen_load(ctx: &mut GenCtx) -> Option<String> {
    let p = ctx.pick_val(Ty::Ptr)?;
    let ty = *ctx
        .rng
        .pick(&[Ty::I8, Ty::I32, Ty::I64, Ty::F32, Ty::F64, Ty::Ptr]);
    let r = ctx.fresh("r");
    ctx.push_val(ty, r.clone());
    Some(format!("    %{r} = load {} , ptr %{p}", ty.name()))
}

fn gen_store(ctx: &mut GenCtx) -> Option<String> {
    let p = ctx.pick_val(Ty::Ptr)?;
    let ty = *ctx.rng.pick(&[Ty::I8, Ty::I32, Ty::I64, Ty::F32, Ty::F64]);
    let v = pick_typed_operand(ctx, ty)?;
    Some(format!("    store {v}, ptr %{p}"))
}

fn gen_alloca(ctx: &mut GenCtx) -> Option<String> {
    let ty = *ctx.rng.pick(&[Ty::I32, Ty::I64, Ty::F64, Ty::F32]);
    let r = ctx.fresh("r");
    ctx.push_val(Ty::Ptr, r.clone());
    Some(format!("    %{r} = alloca {}", ty.name()))
}

fn gen_call(ctx: &mut GenCtx) -> Option<String> {
    if ctx.funcs.is_empty() {
        return None;
    }
    let sig = ctx.funcs[ctx.rng.below(ctx.funcs.len() as u64) as usize].clone();
    let mut args = String::new();
    for (i, pt) in sig.params.iter().enumerate() {
        if i > 0 {
            args.push_str(", ");
        }
        // call 参数必须是 `ty value` 形式（pick_typed_operand 已带类型前缀）
        args.push_str(&pick_typed_operand(ctx, *pt)?);
    }
    match sig.ret {
        Some(rt) => {
            let r = ctx.fresh("r");
            ctx.push_val(rt, r.clone());
            Some(format!(
                "    %{r} = call {} @{}({args})",
                rt.name(),
                sig.name
            ))
        }
        None => Some(format!("    call void @{}({args})", sig.name)),
    }
}

/// 纯常量/undef 算术（覆盖无 SSA 依赖的指令行 + 内联常量 round-trip）
fn gen_const_arith(ctx: &mut GenCtx) -> Option<String> {
    let ty = *ctx.rng.pick(&[Ty::I32, Ty::I64]);
    let a = pick_operand_bare(ctx, ty);
    let b = pick_operand_typed(ctx, ty);
    let r = ctx.fresh("r");
    ctx.push_val(ty, r.clone());
    Some(format!("    %{r} = add {} {a}, {b}", ty.name()))
}

fn gen_inst(ctx: &mut GenCtx) -> Option<String> {
    match ctx.rng.below(20) {
        0..=6 => gen_int_binop(ctx),
        7 => gen_icmp(ctx),
        8 => gen_float_binop(ctx),
        9 => gen_fcmp(ctx),
        10 => gen_conv(ctx),
        11 => gen_select(ctx),
        12 => gen_load(ctx),
        13 => gen_store(ctx),
        14 => gen_call(ctx),
        15 => gen_alloca(ctx),
        16 => gen_const_arith(ctx),
        _ => gen_int_binop(ctx),
    }
}

// ── 终结符生成 ──

fn gen_terminator(ctx: &mut GenCtx, ret: Option<Ty>) -> String {
    match ctx.rng.below(8) {
        0..=3 => {
            let Some(ty) = ret else {
                return "    ret void".to_string();
            };
            let v = match pick_typed_operand(ctx, ty) {
                Some(v) => v,
                None => return "    unreachable".to_string(),
            };
            format!("    ret {v}")
        }
        4 => {
            if ret.is_none() {
                "    ret void".to_string()
            } else {
                "    unreachable".to_string()
            }
        }
        5 => "    unreachable".to_string(),
        6 => {
            let b = ctx.pick_block();
            format!("    br label %{b}")
        }
        _ => match ctx.pick_val(Ty::I1) {
            Some(c) => {
                let t = ctx.pick_block();
                let f = ctx.pick_block();
                format!("    br i1 %{c}, label %{t}, label %{f}")
            }
            None => {
                let b = ctx.pick_block();
                format!("    br label %{b}")
            }
        },
    }
}

// ── 函数/模块生成 ──

fn gen_function(ctx: &mut GenCtx, idx: usize) -> String {
    let name = format!("f{idx}");
    let ret = match ctx.rng.below(5) {
        0 => None,
        1 => Some(Ty::I32),
        2 => Some(Ty::I64),
        3 => Some(Ty::F32),
        _ => Some(Ty::F64),
    };
    let nparams = ctx.rng.below(3) as usize;
    let mut params = Vec::new();
    for _ in 0..nparams {
        params.push(*ctx.rng.pick(&[Ty::I32, Ty::I64, Ty::F32, Ty::F64, Ty::Ptr]));
    }

    // 块与值池均为函数作用域：先保存，函数结束后恢复
    let saved_blocks = std::mem::take(&mut ctx.blocks);
    let saved_vals = std::mem::take(&mut ctx.by_ty);
    let nblocks = 1 + ctx.rng.below(3) as usize;
    for b in 0..nblocks {
        ctx.blocks.push(if b == 0 {
            "entry".to_string()
        } else {
            format!("bb{b}")
        });
    }
    for (i, p) in params.iter().enumerate() {
        ctx.push_val(*p, format!("arg{i}"));
    }

    let mut out = format!("define {} @{name}(", ret.map(Ty::name).unwrap_or("void"));
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!("{} %arg{i}", p.name()));
    }
    out.push_str(") {\n");
    for b in 0..nblocks {
        out.push_str(&format!("  %{}:\n", ctx.blocks[b]));
        let ninst = ctx.rng.below(6) as usize;
        for _ in 0..ninst {
            if let Some(line) = gen_inst(ctx) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        out.push_str(&gen_terminator(ctx, ret));
        out.push('\n');
    }
    out.push_str("}\n");

    ctx.blocks = saved_blocks;
    ctx.by_ty = saved_vals;
    ctx.funcs.push(FuncSig { name, ret, params });
    out
}

fn gen_module(ctx: &mut GenCtx) -> String {
    // funcs 是模块级作用域：call 只能引用本模块已生成的函数
    let saved_funcs = std::mem::take(&mut ctx.funcs);
    let mut out = String::new();
    let nglobals = ctx.rng.below(3) as usize;
    for _ in 0..nglobals {
        let g = ctx.fresh("g");
        let ty = *ctx.rng.pick(&[Ty::I32, Ty::I64]);
        let init = match ctx.rng.below(3) {
            0 => "0",
            1 => "1",
            _ => "42",
        };
        out.push_str(&format!("@{g} = global {} {init}\n", ty.name()));
    }
    let nfuncs = 1 + ctx.rng.below(2) as usize;
    for i in 0..nfuncs {
        out.push_str(&gen_function(ctx, i));
    }
    ctx.funcs = saved_funcs;
    out
}

// ── round-trip 断言（与 display_llvm.rs 同强度：opcode/操作数/立即数值/类型/终结符）──

fn const_eq(f1: &Function, a: &Immediate, f2: &Function, b: &Immediate) -> bool {
    match (a, b) {
        (Immediate::Const(c1), Immediate::Const(c2)) => {
            if let (Some((v1, b1)), Some((v2, b2))) =
                (f1.constants.get_int(*c1), f2.constants.get_int(*c2))
            {
                v1 == v2 && b1 == b2
            } else if let (Some(v1), Some(v2)) =
                (f1.constants.get_float(*c1), f2.constants.get_float(*c2))
            {
                v1 == v2
            } else if let (Some(v1), Some(v2)) =
                (f1.constants.get_vector(*c1), f2.constants.get_vector(*c2))
            {
                v1 == v2
            } else {
                a == b
            }
        }
        _ => a == b,
    }
}

fn assert_inst_eq(
    f1: &Function,
    i1: &forge_ir::dfg::Instruction,
    f2: &Function,
    i2: &forge_ir::dfg::Instruction,
    text: &str,
) {
    assert_eq!(i1.opcode, i2.opcode, "opcode:\n{text}");
    assert_eq!(
        i1.operands, i2.operands,
        "operands of {:?}:\n{text}",
        i1.opcode
    );
    assert_eq!(
        i1.immediates.len(),
        i2.immediates.len(),
        "immediates len of {:?}:\n{text}",
        i1.opcode
    );
    for (a, b) in i1.immediates.iter().zip(i2.immediates.iter()) {
        assert!(
            const_eq(f1, a, f2, b),
            "immediates {:?} vs {:?} of {:?}:\n{text}",
            a,
            b,
            i1.opcode
        );
    }
    for (r1, r2) in i1.results.iter().zip(i2.results.iter()) {
        assert_eq!(
            f1.dfg.value_type(*r1),
            f2.dfg.value_type(*r2),
            "result type of {:?}:\n{text}",
            i1.opcode
        );
    }
}

fn assert_block_eq(f1: &Function, b1: &BlockData, f2: &Function, b2: &BlockData, text: &str) {
    assert_eq!(b1.inst_order.len(), b2.inst_order.len(), "inst count blk");
    for (i1, i2) in b1.inst_order.iter().zip(b2.inst_order.iter()) {
        let a = &f1.dfg.insts[i1.0 as usize];
        let b = &f2.dfg.insts[i2.0 as usize];
        assert_inst_eq(f1, a, f2, b, text);
    }
    assert_eq!(b1.terminator, b2.terminator, "terminator mismatch:\n{text}");
}

fn assert_modules_eq(m1: &Module, m2: &Module, text: &str) {
    assert_eq!(
        m1.function_count(),
        m2.function_count(),
        "func count:\n{text}"
    );
    let fs1: Vec<_> = m1.iter_functions().collect();
    let fs2: Vec<_> = m2.iter_functions().collect();
    for (f1, f2) in fs1.iter().zip(fs2.iter()) {
        assert_eq!(f1.name, f2.name);
        assert_eq!(
            f1.dfg.blocks.len(),
            f2.dfg.blocks.len(),
            "block count {}",
            f1.name
        );
        for (b1, b2) in f1.dfg.blocks.iter().zip(f2.dfg.blocks.iter()) {
            assert_block_eq(f1, b1, f2, b2, text);
        }
    }
}

fn fuzz_roundtrip(src: &str, seed: u64, iter: usize) {
    let m1 = parse_module(src)
        .unwrap_or_else(|e| panic!("[seed={seed:#x} iter={iter}] parse1 failed:\n{src}\n{e}"));
    let text = m1.to_string();
    let m2 = parse_module(&text)
        .unwrap_or_else(|e| panic!("[seed={seed:#x} iter={iter}] reparse failed:\n{text}\n{e}"));
    assert_modules_eq(&m1, &m2, &text);
}

fn run_fuzz(seed: u64, iters: usize) {
    let mut ctx = GenCtx::new(seed);
    for i in 0..iters {
        let src = gen_module(&mut ctx);
        fuzz_roundtrip(&src, seed, i);
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(s) => {
            let s = s.trim();
            if let Some(hex) = s.strip_prefix("0x") {
                u64::from_str_radix(hex, 16).unwrap_or(default)
            } else {
                s.parse().unwrap_or(default)
            }
        }
        Err(_) => default,
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(default)
}

/// 主 fuzz：默认 10_000 模块（验收标准），FORGE_FUZZ_ITERS/FORGE_FUZZ_SEED 可覆盖。
#[test]
fn roundtrip_fuzz_default() {
    let seed = env_u64("FORGE_FUZZ_SEED", 0x9E37_79B9_7F4A_7C15);
    let iters = env_usize("FORGE_FUZZ_ITERS", 10_000);
    run_fuzz(seed, iters);
}

/// 第二固定种子常规回归（CI 每次都跑，代价小）。
#[test]
fn roundtrip_fuzz_seed_alt() {
    run_fuzz(0xDEAD_BEEF_CAFE_F00D, 500);
}
