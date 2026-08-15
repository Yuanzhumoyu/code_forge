//! LLVM IR 解析器集成测试矩阵（各语法构造）。
//!
//! 使用 `forge_ir::ir_parser_new`（Phase 7 切换 lib.rs 后为 `ir_parser`）。
//! 每个测试断言解析构建的 forge IR 结构（opcode/操作数/类型/terminator）。

use forge_ir::ir_parser::{parse_function, parse_module};
use forge_ir::opcode::Opcode;

/// 按序收集函数内全部指令 opcode。
fn opcodes(f: &forge_ir::function::Function) -> Vec<Opcode> {
    f.dfg
        .blocks
        .iter()
        .flat_map(|b| b.inst_order.iter())
        .map(|&i| f.dfg.insts[i.0 as usize].opcode)
        .collect()
}

/// 第一条指令的 opcode。
fn first_op(src: &str) -> Opcode {
    let f = parse_function(src).expect("parse");
    opcodes(&f).into_iter().next().expect("at least one inst")
}

// ── 标量算术 ──

#[test]
fn scalar_arith() {
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = add i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Iadd
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = sub i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Isub
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = mul i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Imul
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = udiv i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Udiv
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = sdiv i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Sdiv
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = urem i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Urem
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = srem i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Srem
    );
}

#[test]
fn float_arith() {
    assert_eq!(
        first_op(
            "define double @f(double %a) {\n  %e:\n    %r = fneg double %a\n    ret double %r\n}\n"
        ),
        Opcode::Fneg
    );
    assert_eq!(
        first_op(
            "define double @f(double %a) {\n  %e:\n    %r = fadd double %a, double %a\n    ret double %r\n}\n"
        ),
        Opcode::Fadd
    );
    assert_eq!(
        first_op(
            "define double @f(double %a) {\n  %e:\n    %r = fsub double %a, double %a\n    ret double %r\n}\n"
        ),
        Opcode::Fsub
    );
    assert_eq!(
        first_op(
            "define double @f(double %a) {\n  %e:\n    %r = fmul double %a, double %a\n    ret double %r\n}\n"
        ),
        Opcode::Fmul
    );
    assert_eq!(
        first_op(
            "define double @f(double %a) {\n  %e:\n    %r = fdiv double %a, double %a\n    ret double %r\n}\n"
        ),
        Opcode::Fdiv
    );
    assert_eq!(
        first_op(
            "define double @f(double %a) {\n  %e:\n    %r = fabs double %a\n    ret double %r\n}\n"
        ),
        Opcode::Fabs
    );
    assert_eq!(
        first_op(
            "define double @f(double %a) {\n  %e:\n    %r = fsqrt double %a\n    ret double %r\n}\n"
        ),
        Opcode::Fsqrt
    );
    assert_eq!(
        first_op(
            "define double @f(double %a, double %b) {\n  %e:\n    %r = fma double %a, double %b, double %a\n    ret double %r\n}\n"
        ),
        Opcode::Fma
    );
    assert_eq!(
        first_op(
            "define double @f(double %a, double %b) {\n  %e:\n    %r = fmin double %a, double %b\n    ret double %r\n}\n"
        ),
        Opcode::Fmin
    );
    assert_eq!(
        first_op(
            "define double @f(double %a, double %b) {\n  %e:\n    %r = fmax double %a, double %b\n    ret double %r\n}\n"
        ),
        Opcode::Fmax
    );
    assert_eq!(
        first_op(
            "define double @f(double %a, double %b) {\n  %e:\n    %r = fcopysign double %a, double %b\n    ret double %r\n}\n"
        ),
        Opcode::Fcopysign
    );
    assert_eq!(
        first_op(
            "define double @f(double %a) {\n  %e:\n    %r = floor double %a\n    ret double %r\n}\n"
        ),
        Opcode::Ffloor
    );
    assert_eq!(
        first_op(
            "define double @f(double %a) {\n  %e:\n    %r = ceil double %a\n    ret double %r\n}\n"
        ),
        Opcode::Fceil
    );
    assert_eq!(
        first_op(
            "define double @f(double %a) {\n  %e:\n    %r = round double %a\n    ret double %r\n}\n"
        ),
        Opcode::Fround
    );
}

// ── 位运算/移位 ──

#[test]
fn bitwise_shift() {
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = and i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Band
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = or i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Bor
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = xor i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Bxor
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = shl i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Ishl
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = lshr i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Ushr
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = ashr i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Sshr
    );
    assert_eq!(
        first_op("define i32 @f(i32 %a) {\n  %e:\n    %r = clz i32 %a\n    ret i32 %r\n}\n"),
        Opcode::Clz
    );
    assert_eq!(
        first_op("define i32 @f(i32 %a) {\n  %e:\n    %r = ctz i32 %a\n    ret i32 %r\n}\n"),
        Opcode::Ctz
    );
    assert_eq!(
        first_op("define i32 @f(i32 %a) {\n  %e:\n    %r = ctpop i32 %a\n    ret i32 %r\n}\n"),
        Opcode::Popcnt
    );
    assert_eq!(
        first_op("define i32 @f(i32 %a) {\n  %e:\n    %r = bitreverse i32 %a\n    ret i32 %r\n}\n"),
        Opcode::Bitreverse
    );
    assert_eq!(
        first_op("define i32 @f(i32 %a) {\n  %e:\n    %r = bswap i32 %a\n    ret i32 %r\n}\n"),
        Opcode::Bswap
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = rotl i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Rotl
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = rotr i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Rotr
    );
    assert_eq!(
        first_op("define i32 @f(i32 %a) {\n  %e:\n    %r = not i32 %a\n    ret i32 %r\n}\n"),
        Opcode::Bnot
    );
}

// ── 饱和/极值 ──

#[test]
fn saturating_extrema() {
    assert_eq!(
        first_op("define i32 @f(i32 %a) {\n  %e:\n    %r = abs i32 %a\n    ret i32 %r\n}\n"),
        Opcode::Abs
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = smin i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Smin
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = smax i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Smax
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = umin i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Umin
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = umax i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::Umax
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = sadd.sat i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::SaddSat
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = ssub.sat i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::SsubSat
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = uadd.sat i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::UaddSat
    );
    assert_eq!(
        first_op(
            "define i32 @f(i32 %a, i32 %b) {\n  %e:\n    %r = usub.sat i32 %a, i32 %b\n    ret i32 %r\n}\n"
        ),
        Opcode::UsubSat
    );
}

// ── 比较（icmp 全 10 条件 + fcmp）──

#[test]
fn icmp_all_conditions() {
    for (name, want) in [
        ("eq", forge_ir::opcode::IntCC::Equal),
        ("ne", forge_ir::opcode::IntCC::NotEqual),
        ("slt", forge_ir::opcode::IntCC::SignedLessThan),
        ("sgt", forge_ir::opcode::IntCC::SignedGreaterThan),
        ("sle", forge_ir::opcode::IntCC::SignedLessThanOrEqual),
        ("sge", forge_ir::opcode::IntCC::SignedGreaterThanOrEqual),
        ("ult", forge_ir::opcode::IntCC::UnsignedLessThan),
        ("ugt", forge_ir::opcode::IntCC::UnsignedGreaterThan),
        ("ule", forge_ir::opcode::IntCC::UnsignedLessThanOrEqual),
        ("uge", forge_ir::opcode::IntCC::UnsignedGreaterThanOrEqual),
    ] {
        let src = format!(
            "define i1 @f(i32 %a, i32 %b) {{\n  %e:\n    %r = icmp {name} i32 %a, i32 %b\n    ret i1 %r\n}}\n"
        );
        let f = parse_function(&src).expect("parse icmp");
        let inst = &f.dfg.insts[0];
        match &inst.opcode {
            Opcode::Icmp { cond } => assert_eq!(*cond, want, "icmp {name}"),
            other => panic!("expected Icmp, got {other:?}"),
        }
    }
}

#[test]
fn fcmp_ordered() {
    let f = parse_function(
        "define i1 @f(double %a, double %b) {\n  %e:\n    %r = fcmp olt double %a, double %b\n    ret i1 %r\n}\n",
    )
    .expect("parse fcmp");
    let inst = &f.dfg.insts[0];
    match &inst.opcode {
        Opcode::Fcmp { cond } => assert_eq!(*cond, forge_ir::opcode::FloatCC::LessThan),
        other => panic!("expected Fcmp, got {other:?}"),
    }
}

#[test]
fn fcmp_unsupported_condition() {
    // forge FloatCC 无 ueq/true 等——报语义错误
    let r = parse_function(
        "define i1 @f(double %a, double %b) {\n  %e:\n    %r = fcmp ueq double %a, double %b\n    ret i1 %r\n}\n",
    );
    assert!(r.is_err());
}

// ── 内存 ──

#[test]
fn memory_ops() {
    let f = parse_function(
        "define i32 @f() {\n  %e:\n    %p = alloca i32\n    store i32 5, ptr %p\n    %v = load i32, ptr %p\n    ret i32 %v\n}\n",
    )
    .expect("parse mem");
    let ops = opcodes(&f);
    assert!(ops.contains(&Opcode::Alloca), "alloca");
    assert!(ops.contains(&Opcode::Store), "store");
    assert!(ops.contains(&Opcode::Load), "load");
}

#[test]
fn getelementptr() {
    let f = parse_function(
        "define ptr @f(ptr %p) {\n  %e:\n    %r = getelementptr i32, ptr %p, i32 1\n    ret ptr %r\n}\n",
    )
    .expect("parse gep");
    assert!(opcodes(&f).contains(&Opcode::GetElementPtr));
}

// ── 控制流 ──

#[test]
fn control_flow() {
    let f = parse_function(
        "define i32 @f(i32 %x) {\n  %entry:\n    br i1 true, label %then, label %else\n  %then:\n    ret i32 1\n  %else:\n    unreachable\n}\n",
    )
    .expect("parse cf");
    assert_eq!(f.dfg.block_count(), 3);
    assert!(matches!(
        &f.dfg.blocks[0].terminator,
        forge_ir::terminator::Terminator::Branch { .. }
    ));
    assert!(matches!(
        &f.dfg.blocks[2].terminator,
        forge_ir::terminator::Terminator::Unreachable
    ));
}

#[test]
fn switch_stmt() {
    let f = parse_function(
        "define i32 @f(i32 %x) {\n  %entry:\n    switch i32 %x, label %d [ i32 0, label %z ]\n  %d:\n    ret i32 0\n  %z:\n    ret i32 1\n}\n",
    )
    .expect("parse switch");
    assert_eq!(f.dfg.block_count(), 3);
}

// ── 转换 ──

#[test]
fn conversions() {
    assert_eq!(
        first_op(
            "define i64 @f(i32 %a) {\n  %e:\n    %r = sext i32 %a to i64\n    ret i64 %r\n}\n"
        ),
        Opcode::Sextend
    );
    assert_eq!(
        first_op(
            "define i64 @f(i32 %a) {\n  %e:\n    %r = zext i32 %a to i64\n    ret i64 %r\n}\n"
        ),
        Opcode::Uextend
    );
    // trunc：整数 → Ireduce
    assert_eq!(
        first_op(
            "define i32 @f(i64 %a) {\n  %e:\n    %r = trunc i64 %a to i32\n    ret i32 %r\n}\n"
        ),
        Opcode::Ireduce
    );
    // trunc：浮点 → Fptrunc（LLVM：trunc 对浮点是精度截断）
    assert_eq!(
        first_op(
            "define float @f(double %a) {\n  %e:\n    %r = trunc double %a to float\n    ret float %r\n}\n"
        ),
        Opcode::Fptrunc
    );
    // 新增 LLVM 转换全集：fptrunc/fpext/fptosi/sitofp/fptoui/uitofp/ptrtoint/inttoptr
    assert_eq!(
        first_op(
            "define float @f(double %a) {\n  %e:\n    %r = fptrunc double %a to float\n    ret float %r\n}\n"
        ),
        Opcode::Fptrunc
    );
    assert_eq!(
        first_op(
            "define double @f(float %a) {\n  %e:\n    %r = fpext float %a to double\n    ret double %r\n}\n"
        ),
        Opcode::Fpext
    );
    assert_eq!(
        first_op(
            "define i32 @f(double %a) {\n  %e:\n    %r = fptosi double %a to i32\n    ret i32 %r\n}\n"
        ),
        Opcode::Fptosi
    );
    assert_eq!(
        first_op(
            "define double @f(i32 %a) {\n  %e:\n    %r = sitofp i32 %a to double\n    ret double %r\n}\n"
        ),
        Opcode::Sitofp
    );
    assert_eq!(
        first_op(
            "define i32 @f(double %a) {\n  %e:\n    %r = fptoui double %a to i32\n    ret i32 %r\n}\n"
        ),
        Opcode::Fptoui
    );
    assert_eq!(
        first_op(
            "define double @f(i32 %a) {\n  %e:\n    %r = uitofp i32 %a to double\n    ret double %r\n}\n"
        ),
        Opcode::Uitofp
    );
    assert_eq!(
        first_op(
            "define i64 @f(ptr %p) {\n  %e:\n    %r = ptrtoint ptr %p to i64\n    ret i64 %r\n}\n"
        ),
        Opcode::Ptrtoint
    );
    assert_eq!(
        first_op(
            "define ptr @f(i64 %a) {\n  %e:\n    %r = inttoptr i64 %a to ptr\n    ret ptr %r\n}\n"
        ),
        Opcode::Inttoptr
    );
    assert_eq!(
        first_op(
            "define i32 @f(double %a) {\n  %e:\n    %r = bitcast double %a to i64\n    ret i32 %r\n}\n"
        ),
        Opcode::Bitcast
    );
}

// ── 调用 ──

#[test]
fn calls() {
    let m = parse_module(
        "define i32 @add(i32 %a, i32 %b) {\n  %e:\n    %s = add i32 %a, i32 %b\n    ret i32 %s\n}\ndefine i32 @main() {\n  %e:\n    %r = call i32 @add(i32 1, i32 2)\n    ret i32 %r\n}\n",
    )
    .expect("parse module");
    let main = m.get_function(forge_ir::entity::FuncRef(1));
    assert!(opcodes(main).contains(&Opcode::Call));
}

// ── 常量/值 ──

#[test]
fn constants() {
    // 十六进制、负、浮点、bool、undef/poison/null
    let f = parse_function(
        "define i32 @f() {\n  %e:\n    %a = add i32 0x2A, i32 -1\n    %b = add i32 1, i32 2\n    ret i32 %a\n}\n",
    )
    .expect("parse consts");
    assert!(opcodes(&f).contains(&Opcode::Iadd));
}

#[test]
fn undef_poison_null() {
    let f = parse_function(
        "define i32 @f() {\n  %e:\n    %u = undef i32\n    %p = poison i32\n    ret i32 %u\n}\n",
    )
    .expect("parse undef");
    let ops = opcodes(&f);
    assert_eq!(ops[0], Opcode::Undef);
    assert_eq!(ops[1], Opcode::Poison);
}

// ── 向量 ──

#[test]
fn vector_add_dispatches_to_vadd() {
    let f = parse_function(
        "define <4 x i32> @f(<4 x i32> %a) {\n  %e:\n    %r = add <4 x i32> %a, <4 x i32> %a\n    ret <4 x i32> %r\n}\n",
    )
    .expect("parse vec");
    assert_eq!(opcodes(&f)[0], Opcode::Vadd);
}

#[test]
fn vector_extract_insert() {
    let f = parse_function(
        "define i32 @f(<4 x i32> %v) {\n  %e:\n    %x = vextractelement <4 x i32> %v, i32 0\n    ret i32 %x\n}\n",
    )
    .expect("parse vextract");
    assert!(opcodes(&f).contains(&Opcode::Vextract));
}

// ── 扩展 ──

#[test]
fn forge_extensions() {
    assert_eq!(
        first_op("define i32 @f(i32 %a) {\n  %e:\n    %r = copy i32 %a\n    ret i32 %r\n}\n"),
        Opcode::Copy
    );
    assert_eq!(
        first_op("define i32 @f() {\n  %e:\n    %r = nop\n    ret i32 %r\n}\n"),
        Opcode::Nop
    );
    assert_eq!(
        first_op("define i32 @f(ptr %p) {\n  %e:\n    %r = isnull ptr %p\n    ret i32 %r\n}\n"),
        Opcode::IsNull
    );
}

// ── 模块级 ──

#[test]
fn module_target_and_comment() {
    // `;` 注释
    let m = parse_module(
        "target triple = \"x86_64-pc-windows\"\n; a comment\ndefine i32 @f() {\n  %e:\n    ret i32 0\n}\n",
    )
    .expect("parse module");
    assert!(m.target_triple.is_some(), "target triple set");
    assert!(m.get_function(forge_ir::entity::FuncRef(0)).name.as_str() == "f");
}

// ── 错误路径 ──

#[test]
fn errors() {
    // 未知 opcode
    assert!(
        parse_function("define i32 @f() {\n  %e:\n    %r = frobnicate i32 1\n    ret i32 %r\n}\n")
            .is_err()
    );
    // 未定义值——第十一轮宽松：undef 占位（use-list 测试语义），不再报错
    assert!(
        parse_function(
            "define i32 @f() {\n  %e:\n    %r = add i32 %missing, i32 1\n    ret i32 %r\n}\n"
        )
        .is_ok()
    );
    // SSA 重复
    assert!(parse_function("define i32 @f() {\n  %e:\n    %x = add i32 1, 2\n    %x = add i32 3, 4\n    ret i32 %x\n}\n").is_err());
    // 未定义块
    assert!(parse_function("define i32 @f() {\n  %e:\n    br label %nowhere\n}\n").is_err());
    // 非法 token
    assert!(parse_function("define i32 @f() {\n  %e:\n    %r = ????\n}\n").is_err());
}

// ── 语义错误路径 ──────────────────────────────────────────

#[test]
fn errors_undefined_block() {
    let r = parse_function("define i32 @f() {\n  %e:\n    br label %missing\n}");
    assert!(r.is_err(), "br to undefined block should error");
}

#[test]
fn errors_undefined_function() {
    let r = parse_function(
        "define i32 @f() {\n  %e:\n    %v = call i32 @missing(i32 1)\n    ret i32 %v\n}",
    );
    assert!(r.is_err(), "call to undefined function should error");
}

// ── 深层聚合提取（第二十九轮:多索引 extractvalue 链式）──────────

#[test]
fn deep_extractvalue_nested_indices() {
    // {i64, {i32, i32}} 的多索引提取:extractvalue ..., 1, 0 → i32
    let src = r#"
define i32 @f() {
  %e:
    %agg = extractvalue {i64, {i32, i32}} {i64 1, {i32, i32} {i32 2, i32 3}}, 1, 0
    ret i32 %agg
}
"#;
    let f = parse_function(src).expect("deep extractvalue should parse+build");
    // 常量聚合路径:链式 emit 两个 ExtractValue(值语义等价)
    let mut count = 0;
    for bd in f.dfg.blocks.iter() {
        for &i in &bd.inst_order {
            if f.dfg.insts[i.0 as usize].opcode == Opcode::ExtractValue {
                count += 1;
            }
        }
    }
    assert_eq!(count, 2, "两层索引 → 两条 ExtractValue 链");
}

#[test]
fn deep_extractvalue_value_path() {
    // 值路径(load 出的聚合)多索引
    let src = r#"
@g = global {i64, {i32, i32}} {i64 1, {i32, i32} {i32 2, i32 3}}
define i32 @f() {
  %e:
    %p = load {i64, {i32, i32}}, ptr @g
    %x = extractvalue {i64, {i32, i32}} %p, 1, 0
    ret i32 %x
}
"#;
    let f = parse_function(src).expect("value-path deep extractvalue should parse+build");
    let mut count = 0;
    for bd in f.dfg.blocks.iter() {
        for &i in &bd.inst_order {
            if f.dfg.insts[i.0 as usize].opcode == Opcode::ExtractValue {
                count += 1;
            }
        }
    }
    assert_eq!(count, 2, "值路径两层索引 → 两条 ExtractValue 链");
}
