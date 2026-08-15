//! LLVM IR Display round-trip：parse → display → parse 结构等价。
//! forge IR 的 Display 输出严格 LLVM 文本，可被新解析器完整读回。
//!
//! 断言强度：除 opcode/operands 外，还比较 immediates（常量按值语义比较）、
//! 指令结果类型、终结符（含块参数），避免"伪 round-trip"（结构相同但
//! 类型/立即数/终结符参数丢失）。

use forge_ir::FunctionBuilder;
use forge_ir::dfg::BlockData;
use forge_ir::function::{Function, Module};
use forge_ir::immediate::Immediate;
use forge_ir::ir_parser::parse_module;
use forge_ir::types::{FunctionSignature, TypeContext};
use forge_ir::verify::{Verifier, VerifyError};

/// parse₁ → display → parse₂，断言两次 forge IR 结构等价。
fn assert_roundtrip(src: &str) {
    let m1 = parse_module(src).unwrap_or_else(|e| panic!("parse1 failed for:\n{src}\n{e}"));
    let text = m1.to_string();
    let m2 = parse_module(&text).unwrap_or_else(|e| panic!("reparse failed for:\n{text}\n{e}"));
    assert_modules_eq(&m1, &m2, &text);
}

/// 两个常量池中的 ConstId 按值语义比较（int/float/vector 内容，而非索引）。
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
        // 聚合常量按值比较（AggId 是池索引，跨模块可能不同）
        (Immediate::Agg(a_id), Immediate::Agg(b_id)) => {
            fn agg_eq(
                f1: &Function,
                a: forge_ir::AggId,
                f2: &Function,
                b: forge_ir::AggId,
            ) -> bool {
                match (f1.constants.get_aggregate(a), f2.constants.get_aggregate(b)) {
                    (Some(ga), Some(gb))
                        if ga.ty == gb.ty && ga.children.len() == gb.children.len() =>
                    {
                        use forge_ir::constant::AggChild;
                        ga.children
                            .iter()
                            .zip(&gb.children)
                            .all(|(ca, cb)| match (ca, cb) {
                                (AggChild::Scalar(x), AggChild::Scalar(y)) => {
                                    const_eq(f1, &Immediate::Const(*x), f2, &Immediate::Const(*y))
                                }
                                (AggChild::Agg(x), AggChild::Agg(y)) => agg_eq(f1, *x, f2, *y),
                                _ => false,
                            })
                    }
                    _ => false,
                }
            }
            agg_eq(f1, *a_id, f2, *b_id)
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
    // 结果类型（TypeId 索引在预填充 TypeStore 中一致）
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
    // 终结符（含块参数）整体比较
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

// ── 基础 round-trip ────────────────────────────────────────

#[test]
fn roundtrip_simple_function() {
    assert_roundtrip(
        "define i32 @add(i32 %a, i32 %b) {
  %entry:
    %s = add i32 %a, i32 %b
    ret i32 %s
}",
    );
}

#[test]
fn roundtrip_icmp_and_branch() {
    assert_roundtrip(
        "define i1 @gt(i32 %a, i32 %b) {
  %entry:
    %c = icmp sgt i32 %a, i32 %b
    br i1 %c, label %t, label %f
  %t:
    ret i1 true
  %f:
    ret i1 false
}",
    );
}

#[test]
fn roundtrip_constants_inline() {
    // iconst 内联：常量作操作数，不输出独立指令行
    assert_roundtrip(
        "define i32 @c() {
  %entry:
    %r = add i32 40, i32 2
    ret i32 %r
}",
    );
}

#[test]
fn roundtrip_f32_precision() {
    // round-trip fuzz 回归：f32 小数常量（非精确二进制）display 输出 hex 位模式
    // 后 reparse 数值必须不变（此前 f32 小数以 f64 存、hex 以 f32 提升，二者不等）
    assert_roundtrip(
        "define f32 @c(f32 %a) {
  %entry:
    %r = fadd f32 %a, f32 -2.5e-2
    ret f32 %r
}",
    );
}

#[test]
fn roundtrip_conversion_to() {
    assert_roundtrip(
        "define i64 @sext8(i8 %a) {
  %entry:
    %r = sext i8 %a to i64
    ret i64 %r
}",
    );
}

#[test]
fn roundtrip_switch_and_unreachable() {
    assert_roundtrip(
        "define i32 @sw(i32 %x) {
  %entry:
    switch i32 %x, label %default [ i32 1, label %one
                                     i32 2, label %two ]
  %default:
    unreachable
  %one:
    ret i32 1
  %two:
    ret i32 2
}",
    );
}

#[test]
fn roundtrip_multi_function_module() {
    assert_roundtrip(
        "target triple = \"x86_64-pc-windows\"
define i32 @add(i32 %a, i32 %b) {
  %entry:
    %s = add i32 %a, i32 %b
    ret i32 %s
}
define i32 @main() {
  %entry:
    %v = call i32 @add(i32 1, i32 2)
    ret i32 %v
}",
    );
}

#[test]
fn roundtrip_vector_add() {
    // 向量 op：Vadd 输出 LLVM 名 add + 向量类型
    assert_roundtrip(
        "define <4 x i32> @vadd(<4 x i32> %a, <4 x i32> %b) {
  %entry:
    %r = add <4 x i32> %a, <4 x i32> %b
    ret <4 x i32> %r
}",
    );
}

// ── 内存指令 round-trip（曾读不回：load/alloca/gep/offset）──

#[test]
fn roundtrip_load_store() {
    assert_roundtrip(
        "define i32 @f(ptr %p) {
  %entry:
    %v = load i32, ptr %p
    store i32 %v, ptr %p
    ret i32 %v
}",
    );
}

#[test]
fn roundtrip_alloca_with_count() {
    assert_roundtrip(
        "define ptr @f() {
  %entry:
    %a = alloca i32
    %b = alloca i64, i32 8
    ret ptr %b
}",
    );
}

#[test]
fn roundtrip_getelementptr() {
    // 带索引与零索引两种形态
    assert_roundtrip(
        "define ptr @f(ptr %p, i64 %i) {
  %entry:
    %q = getelementptr {i32, i64}, ptr %p, i64 %i, i32 1
    %r = getelementptr i32, ptr %q
    ret ptr %r
}",
    );
}

// ── phi round-trip（标准 LLVM；forge 块参数 ↔ phi 双向转换）────

#[test]
fn roundtrip_phi_jump_and_branch() {
    assert_roundtrip(
        "define i32 @f(i32 %x) {
  %entry:
    br label %loop
  %loop:
    %v = phi i32 [ %x, %entry ]
    %s = add i32 %v, i32 1
    br i1 true, label %a, label %b
  %a:
    ret i32 1
  %b:
    ret i32 2
}",
    );
}

#[test]
fn roundtrip_phi_switch() {
    assert_roundtrip(
        "define i32 @f(i32 %x) {
  %entry:
    switch i32 %x, label %def [ i32 1, label %one
                                i32 2, label %two ]
  %def:
    unreachable
  %one:
    ret i32 1
  %two:
    ret i32 2
}",
    );
}

#[test]
fn roundtrip_phi_loop_multiple_incoming() {
    // 循环：phi 多入边（entry 常量 + 循环自边值）、多个 phi 共享同一前驱、
    // 入边引用后续指令定义的值（%s/%i2 在 phi 之后定义）
    assert_roundtrip(
        "define i32 @fib(i32 %n) {
  %entry:
    br label %loop
  %loop:
    %a = phi i32 [ 0, %entry ], [ %b, %loop ]
    %b = phi i32 [ 1, %entry ], [ %s, %loop ]
    %i = phi i32 [ 0, %entry ], [ %i2, %loop ]
    %s = add i32 %a, i32 %b
    %i2 = add i32 %i, i32 1
    %cmp = icmp ult i32 %i, i32 %n
    br i1 %cmp, label %loop, label %done
  %done:
    ret i32 %a
}",
    );
}

#[test]
fn roundtrip_phi_literal_incoming() {
    // 常量入边：`[ 0, %a ]` / `[ 42, %b ]`（字面量在对应前驱终结符前插常量）
    assert_roundtrip(
        "define i32 @f() {
  %entry:
    br i1 true, label %a, label %b
  %a:
    br label %merge
  %b:
    br label %merge
  %merge:
    %v = phi i32 [ 0, %a ], [ 42, %b ]
    ret i32 %v
}",
    );
}

// ── 常量/向量 round-trip ───────────────────────────────────

#[test]
fn roundtrip_vector_constant_literal() {
    // LLVM 标准向量常量字面量：`<4 x float> <1.5, 2.5, -3.5, 4.25>`
    //（forge 小端 vconst 的文本形式；display 内联、parse 重建 vconst 值）
    assert_roundtrip(
        "define <4 x float> @v() {
  %entry:
    %x = fadd <4 x float> <1.5, 2.5, -3.5, 4.25>, <4 x float> <0.0, 1.0, 2.0, 3.0>
    ret <4 x float> %x
}",
    );
}

#[test]
fn roundtrip_vector_zeroinitializer() {
    // `zeroinitializer` 关键字（向量）：内部为全零 vconst
    assert_roundtrip(
        "define <4 x float> @z() {
  %entry:
    %x = fadd <4 x float> zeroinitializer, <4 x float> <1.0, 2.0, 3.0, 4.0>
    ret <4 x float> %x
}",
    );
}

#[test]
fn roundtrip_scalar_zeroinitializer() {
    // `zeroinitializer` 关键字（标量 i32）
    assert_roundtrip(
        "define i32 @z() {
  %entry:
    %x = add i32 zeroinitializer, i32 1
    ret i32 %x
}",
    );
}

#[test]
fn roundtrip_undef_poison_operands() {
    // undef/poison 作为常量操作数（display 内联；parse 重建 undef/poison 值）
    assert_roundtrip(
        "define i32 @f(i32 %a) {
  %entry:
    %x = add i32 undef, i32 %a
    %y = xor i32 poison, i32 %x
    ret i32 %y
}",
    );
}

#[test]
fn roundtrip_conversion_full_set() {
    // LLVM 全 12 个转换指令 round-trip（fptrunc/fpext/fptosi/sitofp/fptoui/uitofp/ptrtoint/inttoptr）
    assert_roundtrip(
        "define ptr @f(i32 %a, double %d, float %g, ptr %p) {
  %entry:
    %e1 = sext i32 %a to i64
    %e2 = zext i32 %a to i64
    %e3 = trunc i64 %e1 to i32
    %e4 = fptrunc double %d to float
    %e5 = fpext float %g to double
    %e6 = fptosi double %d to i32
    %e7 = sitofp i32 %a to double
    %e8 = fptoui double %d to i32
    %e9 = uitofp i32 %a to double
    %e10 = ptrtoint ptr %p to i64
    %e11 = inttoptr i64 %e10 to ptr
    %e12 = bitcast double %d to i64
    ret ptr %e11
}",
    );
}

#[test]
fn roundtrip_mem_attrs() {
    // load/store 的 volatile + align；alloca 的 align
    assert_roundtrip(
        "define i32 @f(ptr %p) {
  %entry:
    %a = alloca i32, align 16
    %v = load volatile i32, ptr %p, align 4
    store volatile i32 %v, ptr %p, align 4
    ret i32 %v
}",
    );
}

#[test]
fn roundtrip_func_attrs_and_callconv() {
    // 调用约定 fastcc + 函数属性 nounwind noinline optnone
    assert_roundtrip(
        "define fastcc i32 @f(i32 %a, ptr %p) nounwind noinline optnone {
  %entry:
    %v = load volatile i32, ptr %p, align 4
    %s = add i32 %v, i32 %a
    ret i32 %s
}",
    );
}

#[test]
fn unknown_func_attr_rejected() {
    // 未知函数属性严格报错
    let r = parse_module(
        "define i32 @f() bogusattr {
  %entry:
    ret i32 0
}",
    );
    assert!(r.is_err(), "unknown attribute should be rejected");
}

#[test]
fn roundtrip_custom_callconv() {
    // cc N 数值调用约定（LLVM 标准）：define + call
    assert_roundtrip(
        "declare cc 10 i32 @hw(i32 %a)
define cc 10 i32 @f(i32 %x) {
  %entry:
    %r = call cc 10 i32 @hw(i32 %x)
    %s = call fastcc i32 @hw(i32 %r)
    ret i32 %s
}",
    );
}

#[test]
fn roundtrip_vector_dynamic_idx() {
    // extractelement/insertelement 变量 idx（此前仅支持常量立即数）
    assert_roundtrip(
        "define float @f(<4 x float> %v, i32 %i, float %x) {
  %entry:
    %e = extractelement <4 x float> %v, i32 %i
    %c = extractelement <4 x float> %v, i32 2
    %n = insertelement <4 x float> %v, float %x, i32 %i
    ret float %e
}
",
    );
}

#[test]
fn roundtrip_agg_literal_extract() {
    // 聚合字面量 agg 完整提取（此前 Unsupported）
    assert_roundtrip(
        "define i32 @f() {
  %entry:
    %a = extractvalue [4 x i32] [i32 1, i32 2, i32 3, i32 4], 2
    %b = extractvalue { i32, i64 } { i32 7, i64 9 }, 0
    %d = extractvalue { i32, i64 } zeroinitializer, 1
    ret i32 %a
}
",
    );
}

#[test]
fn roundtrip_visibility_comdat() {
    assert_roundtrip(
        "$myc = comdat any
@g = hidden global i32 0
@h = protected constant i64 42
@t = dso_local thread_local hidden global i32 0
@c = global i32 0, comdat($myc)
@d = global i32 0, align 8, comdat($myc)
",
    );
}

#[test]
fn roundtrip_callconv_win64cc() {
    assert_roundtrip(
        "declare win64cc i32 @puts(ptr)
define win64cc i32 @f(i32 %x) {
  %entry:
    ret i32 %x
}
define cc 7 i32 @g(i32 %x) {
  %entry:
    ret i32 %x
}
",
    );
}

#[test]
fn roundtrip_struct_type_defs() {
    assert_roundtrip(
        "%struct.Point = type { i32, i64 }
%struct.Node = type { i32, ptr }
define i32 @f(%struct.Point %p, ptr %n) {
  %entry:
    %x = extractvalue %struct.Point %p, 0
    ret i32 %x
}",
    );
}

#[test]
fn roundtrip_global_extensions() {
    assert_roundtrip(
        "@g = addrspace(1) global i32 0
@t = thread_local global i32 0
@tl = dso_local thread_local private global i64 42, align 8",
    );
}

#[test]
fn roundtrip_vector_lane_ops() {
    assert_roundtrip(
        "define <4 x float> @f(<4 x float> %v, float %x, <4 x float> %a, <4 x float> %b) {
  %entry:
    %e = extractelement <4 x float> %v, i32 1
    %i = insertelement <4 x float> %v, float %x, i32 2
    %s = shufflevector <4 x float> %a, <4 x float> %b, <4 x i32> <i32 0, i32 4, i32 1, i32 5>
    ret <4 x float> %s
}",
    );
}

#[test]
fn roundtrip_metadata_str_escapes() {
    // 回归（review nit）：字符串转义必须 lexer 解码 ↔ display 编码对称。
    // 含 \n/\t/\r/\0 的 metadata 字符串 round-trip 后内容不变且文本可再解析。
    let src = "!0 = !{!\"line1\\nline2\"}
!1 = !{!\"tab\\there\\0nul\\rret\"}
define i32 @f() {
  %entry:
    ret i32 1
}";
    let m1 = parse_module(src).unwrap();
    let text = m1.to_string();
    let m2 = parse_module(&text).expect("reparse must succeed");
    assert_modules_eq(&m1, &m2, &text);
    // 内容断言：转义字符 round-trip 后不变（防宽松 lexer 掩盖编码不对称）
    assert_eq!(collect_metadata_strings(&m1), collect_metadata_strings(&m2));
    // 文本断言：display 必须输出转义形式（严格 LLVM 文本，无裸控制字符）
    assert!(
        text.contains("\\nline2") && text.contains("\\0nul") && text.contains("\\rret"),
        "display 应转义控制字符:\n{text}"
    );
    assert!(
        !text.contains("line1\nline2"),
        "display 不得输出裸换行:\n{text}"
    );
}

/// 收集模块 metadata 树中所有字符串值（Leaf/Tuple/Named 递归）。
fn collect_metadata_strings(m: &Module) -> Vec<String> {
    use forge_ir::metadata::{MetadataNode, MetadataValue};
    let mut out = Vec::new();
    for (_, node) in m.metadata_store.iter() {
        let vals: &[MetadataValue] = match node {
            MetadataNode::Leaf(v) => std::slice::from_ref(v),
            MetadataNode::Tuple(v) => v,
            MetadataNode::Named { ops, .. } => ops,
        };
        for v in vals {
            if let MetadataValue::String(s) = v {
                out.push(s.to_string());
            }
        }
    }
    out
}

#[test]
fn roundtrip_metadata() {
    assert_roundtrip(
        "!0 = !{i32 1}
!1 = !{!0, i32 42, !\"hello\"}
!2 = !DILocation(line: 10, column: 3, scope: !1)
define i32 @f(i32 %x) !dbg !2 {
  %entry:
    %p = alloca i32
    store i32 %x, ptr %p, !dbg !2
    %a = load i32, ptr %p, align 4, !dbg !2
    ret i32 %a
}",
    );
    // 引用已定义节点 + 附加组合（!dbg 必须指向 DILocation）
    assert_roundtrip(
        "!4 = !{}
!5 = !DILocation(line: 1, column: 1, scope: !4)
declare void @g()
define void @h() {
  %entry:
    %r = call i32 @f() nounwind, !dbg !5
    ret void
}
define i32 @f() !dbg !5 {
  %entry:
    ret i32 0
}",
    );
}

#[test]
fn roundtrip_call_attrs_and_groups() {
    // call-site 函数属性 + attribute groups（#N 展开；组定义行不 round-trip——属性位等价）
    assert_roundtrip(
        "declare i32 @g(i32)
attributes #0 = { nounwind noinline }
define i32 @f(i32 %x) {
  %entry:
    %r1 = call i32 @g(i32 %x) nounwind
    %r2 = call i32 @g(i32 %r1) #0
    ret i32 %r2
}",
    );
    // define 引用组：`define ... #0`
    assert_roundtrip(
        "attributes #1 = { optnone norecurse }
define i32 @h(i32 %x) #1 {
  %entry:
    ret i32 %x
}",
    );
}

#[test]
fn roundtrip_unnamed_declare_params() {
    // LLVM 标准：declare 裸类型参数（clang 常见输出 `declare i32 @puts(ptr)`）
    assert_roundtrip(
        "declare i32 @puts(ptr)
declare void @memset(ptr, i8, i64, i1)
declare fastcc i32 @fib(i32, i32)
define i32 @main(i32, ptr) {
  %entry:
    %r = call i32 @fib(i32 5, i32 1)
    ret i32 %r
}",
    );
}

#[test]
fn roundtrip_declare_tail_attrs() {
    // P0 2.1：declare 尾部属性（clang 最常见输出）。
    // `declare i32 @printf(ptr noundef, ...) nounwind`——参数属性 + 可变参数 + 函数属性。
    assert_roundtrip(
        "declare i32 @printf(ptr noundef, ...) nounwind
declare i32 @memcpy(ptr, ptr, i64)
define i32 @main() {
  %entry:
    ret i32 0
}",
    );
    // 无固定参数纯 varargs
    assert_roundtrip(
        "declare void @f(...)
define void @g() {
  %entry:
    ret void
}",
    );
}

#[test]
fn roundtrip_declare_attr_group() {
    // P0 2.1：`declare i32 @f(i32) #0`——属性组引用经展开后 round-trip 等价
    //（display 输出展开的具体属性名，parse₂ 得到相同 FunctionAttributes）。
    assert_roundtrip(
        "attributes #0 = { nounwind noinline }
declare i32 @f(i32) #0
define i32 @main() {
  %entry:
    %r = call i32 @f(i32 1)
    ret i32 %r
}",
    );
    // 回归：define 同时带函数属性 + 函数尾 metadata（display 先 attrs 后 metas；!dbg → DILocation）
    assert_roundtrip(
        "!0 = !DILocation(line: 1, column: 1, scope: !0)
define i32 @g(i32 %x) nounwind noinline !dbg !0 {
  %entry:
    ret i32 %x
}",
    );
}

#[test]
fn roundtrip_declare_metadata() {
    // P0 2.1：`declare void @f() !dbg !0`——函数尾 metadata 附加（!dbg → DILocation）
    assert_roundtrip(
        "!0 = !DILocation(line: 1, column: 1, scope: !0)
declare void @f() !dbg !0
define void @g() {
  %entry:
    ret void
}",
    );
}

#[test]
fn roundtrip_named_metadata() {
    // P0 2.2：命名 metadata `!t = !{...}` 定义 + `!dbg !t` 引用（!dbg → DILocation）。
    assert_roundtrip(
        "!0 = !{i32 1}
!t = !DILocation(line: 1, column: 1, scope: !0)
define void @f() !dbg !t {
  %entry:
    ret void
}",
    );
    // 命名定义引用其它命名节点（`!{!t}` 嵌套引用；!dbg 另指 DILocation）
    assert_roundtrip(
        "!t = !{i32 1}
!0 = !{!t}
!1 = !DILocation(line: 1, column: 1, scope: !0)
define void @f() !dbg !1 {
  %entry:
    ret void
}",
    );
    // 指令级 attach 命名引用（`!tbaa !t`——!t 须为 tuple tag 节点；!dbg 另指 DILocation）
    assert_roundtrip(
        "!t = !{!{\"root\", i64 0}, i64 0}
!0 = !DILocation(line: 1, column: 1, scope: !0)
define i32 @f(i32 %a, ptr %p) !dbg !0 {
  %entry:
    %v = load i32, ptr %p, align 4, !tbaa !t
    ret i32 %v
}",
    );
}

#[test]
fn roundtrip_icmp_metadata() {
    // P0 2.3：通用指令 metadata 附加——icmp/fcmp（CompareOp 显式分支）+ `, !prof !0`。
    assert_roundtrip(
        "!0 = !{!\"branch_weights\", i32 64, i32 4}
define i1 @f(i32 %a, i32 %b) {
  %entry:
    %c = icmp slt i32 %a, i32 %b, !prof !0
    %d = fcmp olt float 1.0, float 2.0, !prof !0
    %r = and i1 %c, i1 %d
    ret i1 %r
}",
    );
}

#[test]
fn roundtrip_terminator_metadata() {
    // P0 2.4：终结符 metadata——`ret ..., !range !0` / `br ..., !prof !1`。
    assert_roundtrip(
        "!0 = !{i64 0, i64 100}
!1 = !{!\"branch_weights\", i32 64, i32 4}
define i32 @f(i32 %x) {
  %entry:
    %c = icmp sgt i32 %x, i32 0
    br i1 %c, label %then, label %else, !prof !1
  %then:
    ret i32 %x, !range !0
  %else:
    ret i32 0, !range !0
}",
    );
    // 无条件 br + metadata（`br label %t, !prof !1`）
    assert_roundtrip(
        "!1 = !{!\"branch_weights\", i32 1}
define void @f() {
  %entry:
    br label %exit, !prof !1
  %exit:
    ret void
}",
    );
}

#[test]
fn roundtrip_addrspacecast_and_vaarg() {
    // P0 2.5：addrspacecast（指针地址空间转换）+ va_arg（可变参数读取，文本层）。
    assert_roundtrip(
        "define ptr addrspace(1) @f(ptr %p) {
  %entry:
    %p2 = addrspacecast ptr %p to ptr addrspace(1)
    ret ptr addrspace(1) %p2
}",
    );
    // va_arg：declare 可变参数函数 + va_arg 读取
    assert_roundtrip(
        "declare i32 @printf(ptr, ...)
define i32 @g(ptr %ap) {
  %entry:
    %v = va_arg ptr %ap, i32
    ret i32 %v
}",
    );
}

#[test]
fn roundtrip_linkage_and_dll() {
    // P0 2.6：linkage 补全（weak/linkonce/appending/available_externally）+ dll storage。
    assert_roundtrip(
        "@g1 = weak global i32 0
@g2 = linkonce_odr global i32 0
@g3 = available_externally global i32 0
@g4 = dllimport global i32 0
@g5 = weak_odr dso_local hidden global i32 0
@g6 = linkonce constant i32 1
@g7 = common global i32 0
define i32 @main() {
  %entry:
    ret i32 0
}",
    );
}

#[test]
fn roundtrip_param_attr_extension() {
    // P0 2.7：参数/返回属性补全——byval(Type) / sret(Type) / inreg / align N。
    assert_roundtrip(
        "%struct.S = type { i32, i64 }
declare void @f(ptr byval(%struct.S) %s, i32 inreg signext %x)
define sret(%struct.S) ptr @g(ptr byval(%struct.S) %p) {
  %entry:
    ret ptr %p
}",
    );
    // align N 参数属性 + call 实参属性
    assert_roundtrip(
        "declare void @h(ptr align 16 %p)
define void @g(ptr %p) {
  %entry:
    call void @h(ptr align 16 %p)
    ret void
}",
    );
}

#[test]
fn roundtrip_module_items() {
    // P0 2.8：模块级 item——source_filename / module asm。
    assert_roundtrip(
        "source_filename = \"test.c\"
module asm \".globl _start\"
module asm \".section .note, \\\"a\\\"\"
define i32 @main() {
  %entry:
    ret i32 0
}",
    );
}

#[test]
fn parse_rejects_legacy_pointer_types() {
    // 仅支持最新版 LLVM IR：旧式指针 `T*`（pre-opaque-ptr）全部拒绝
    for src in [
        "define i32 @f(i32* %p) {\n  %entry:\n  ret i32 0\n}\n",
        "define void @f(ptr %a) {\n  %entry:\n  %b = bitcast ptr %a to ptr*\n  ret void\n}\n",
        "define i32 @f([4 x i32]* %p) {\n  %entry:\n  ret i32 0\n}\n",
    ] {
        assert!(
            parse_module(src).is_err(),
            "legacy pointer type must be rejected: {src}"
        );
    }
}

#[test]
fn roundtrip_global_func_attrs() {
    assert_roundtrip(
        "@g = dso_local unnamed_addr global i32 42, align 4
@h = private global i32 0, section \".data\", align 8
@c = constant [4 x i8] c\"abc\"
declare dso_local i32 @ext(i32 %x)
define dso_local i32 @f(i32 %a) {
  %entry:
    %r = call i32 @ext(i32 %a)
    ret i32 %r
}",
    );
}

#[test]
fn roundtrip_call_arg_attrs() {
    assert_roundtrip(
        "declare i32 @callee(i32, ptr, i8)
define i32 @f(i32 %a, ptr %p, i8 %b) {
  %entry:
    %r1 = call i32 @callee(i32 signext %a, ptr noalias %p, i8 %b)
    %r2 = call fastcc i32 @callee(i32 %a, ptr nonnull %p, i8 noundef %b)
    ret i32 %r1
}",
    );
}

#[test]
fn roundtrip_aggregate_operands_and_addrspace() {
    assert_roundtrip(
        "@g = private global ptr addrspace(1) null
define ptr addrspace(1) @f(ptr addrspace(1) %p, { i32, i64 } %agg) {
  %entry:
    %a = extractvalue { i32, i64 } zeroinitializer, 0
    %b = extractvalue { i32, i64 } %agg, 1
    %c = insertvalue { i32, i64 } %agg, i32 %a, 0
    %v = extractvalue { i32, i64 } %c, 0
    ret ptr addrspace(1) %p
}",
    );
}

#[test]
fn roundtrip_atomic_instructions() {
    assert_roundtrip(
        "define i32 @f(ptr %p, i32 %v, i32 %c, i32 %n) {
  %entry:
    %r1 = atomicrmw add ptr %p, i32 %v seq_cst
    %r2 = atomicrmw xchg ptr %p, i32 %v acq_rel
    %r3 = atomicrmw umax ptr %p, i32 %v monotonic
    %r4 = atomicrmw fadd ptr %p, i32 %v release
    %r5 = cmpxchg ptr %p, i32 %c, i32 %n acquire monotonic
    %r6 = cmpxchg weak ptr %p, i32 %c, i32 %n seq_cst acquire
    fence release
    ret i32 %r1
}",
    );
}

#[test]
fn roundtrip_arith_flags() {
    // LLVM 算术标志：nsw/nuw/exact（`add nsw i32 %a, i32 %b`）
    assert_roundtrip(
        "define i32 @f(i32 %a, i32 %b) {
  %entry:
    %x = add nsw i32 %a, i32 %b
    %y = sub nuw i32 %x, i32 1
    %z = udiv exact i32 %y, i32 2
    ret i32 %z
}",
    );
}

#[test]
fn roundtrip_param_attrs() {
    // 参数属性（LLVM：`i32 signext %a` / `ptr noalias %p`）
    assert_roundtrip(
        "define i32 @f(i32 signext %a, i32 zeroext %b, ptr noalias %p, ptr nonnull %q) nounwind {
  %entry:
    %x = add nsw i32 %a, i32 %b
    %v = load i32, ptr %p
    ret i32 %x
}",
    );
}

#[test]
fn unknown_param_attr_rejected() {
    // 未知参数属性严格报错
    let r = parse_module(
        "define i32 @f(i32 bogus %a) {
  %entry:
    ret i32 0
}",
    );
    assert!(r.is_err(), "unknown param attr should be rejected");
}

#[test]
fn roundtrip_float_hex_constants() {
    // LLVM 标准浮点 hex 常量（位精确）：f32 0x3FC00000 = 1.5f、f64 0x3FF8000000000000 = 1.5
    assert_roundtrip(
        "define double @f() {
  %entry:
    %x = fadd double 0x3FF8000000000000, double 0x4000000000000000
    %y = fadd float 0x3FC00000, float 0x40200000
    %z = fpext float %y to double
    %r = fadd double %x, double %z
    ret double %r
}",
    );
}

#[test]
fn roundtrip_select_freeze() {
    // select（3 操作数）与 freeze（LLVM 值语义指令）round-trip
    assert_roundtrip(
        "define i32 @f(i1 %c, i32 %a, i32 %b) {
  %entry:
    %s = select i1 %c, i32 %a, i32 %b
    %f = freeze i32 %s
    %x = add nsw i32 %f, i32 1
    ret i32 %x
}",
    );
}

#[test]
fn roundtrip_gep_inbounds() {
    // getelementptr inbounds（LLVM 常用：指针运算越界保证）
    assert_roundtrip(
        "define ptr @f(ptr %base, i64 %idx) {
  %entry:
    %p = getelementptr inbounds i32, ptr %base, i64 %idx
    %q = getelementptr inbounds [4 x i32], ptr %base, i64 0, i64 %idx
    ret ptr %q
}",
    );
}

#[test]
fn roundtrip_switch_i64() {
    // switch case 值类型跟随 discriminant（i64 而非硬编码 i32）
    assert_roundtrip(
        "define i32 @f(i64 %x) {
  %entry:
    switch i64 %x, label %def [ i64 1, label %one
                                i64 2, label %two ]
  %def:
    ret i32 0
  %one:
    ret i32 1
  %two:
    ret i32 2
}",
    );
}

#[test]
fn roundtrip_global_linkage_and_string() {
    // 全局 linkage（private/internal）+ LLVM 字符串常量（c"..."）
    assert_roundtrip(
        "@a = private global i32 42
@b = internal global [4 x i8] c\"abc\\00\"
@c = global [8 x i8] c\"\\01\\FF\\00\\10Hi!\"
define i32 @main() {
  %entry:
    %p = load i32, ptr @a
    ret i32 %p
}",
    );
}

#[test]
fn unknown_linkage_rejected() {
    // 未知 linkage 严格报错
    let r = parse_module("@g = boguslink global i32 42\n");
    assert!(r.is_err(), "unknown linkage should be rejected");
}

#[test]
fn roundtrip_fastmath_flags() {
    // fast-math 标志：fast 别名 + 多标志组合 + 多算术标志（nsw nuw）
    assert_roundtrip(
        "define double @f(double %a, double %b, i32 %x, i32 %y) {
  %entry:
    %m = fmul fast double %a, double %b
    %s = fadd nnan ninf double %m, double %a
    %r = fmul contract reassoc double %s, double %b
    %i = add nsw nuw i32 %x, i32 %y
    %z = shl nuw i32 %i, i32 2
    ret double %r
}",
    );
}

#[test]
fn test_verify_fastmath_misuse() {
    // fast-math 用在整数指令 → InvalidImmediate（builder 构造 + verify）
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
    let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
    let (entry, _) = fb.create_entry_block();
    fb.switch_to_block(entry);
    let a = fb.iconst(1, ctx.i32_ty());
    let b = fb.iconst(2, ctx.i32_ty());
    let v = fb.emit1(
        forge_ir::Opcode::Iadd,
        vec![a, b],
        vec![],
        ctx.i32_ty(),
        forge_ir::InstFlags::FMF_FAST, // fast 对整数 add 非法
    );
    fb.ret(&[v]);
    let func = fb.finish().expect("build");

    let mut verifier = Verifier::with_ctx(ctx.clone());
    let result = verifier.verify(&func);
    assert!(
        matches!(result, Err(ref e) if e.iter().any(|x| matches!(x, VerifyError::InvalidImmediate { .. }))),
        "fast on integer add should be reported, got: {result:?}"
    );
}

#[test]
fn roundtrip_call_attrs() {
    // call 属性：tail + fastcc（LLVM：`tail call fastcc i32 @f(...)`）
    assert_roundtrip(
        "declare i32 @add(i32 %a, i32 %b)
define i32 @f(i32 %x, i32 %y) {
  %entry:
    %r = tail call fastcc i32 @add(i32 %x, i32 %y)
    %s = call i32 @add(i32 %r, i32 1)
    ret i32 %s
}",
    );
}

#[test]
fn roundtrip_ret_attrs_zeroinit_dsolocal() {
    // 返回属性（signext）+ global zeroinitializer + dso_local 前缀
    assert_roundtrip(
        "@z = global [4 x i32] zeroinitializer
@f = dso_local private global i32 42
define signext i8 @g(i8 zeroext %b) {
  %entry:
    %p = load i32, ptr @f
    ret i8 %b
}",
    );
}

#[test]
fn roundtrip_aggregate_constants() {
    // 数组/结构体聚合常量（LLVM：`[i32 1, i32 2]` / `{ i32 1, i64 2 }`）
    assert_roundtrip(
        "@arr = global [4 x i32] [i32 1, i32 -2, i32 3, i32 42]
@pt = global { i32, i64 } { i32 1, i64 2 }
@mix = constant [3 x float] [float 1.5, float -2.5, float 0.0]
define i32 @main() {
  %entry:
    %p = load i32, ptr @arr
    ret i32 %p
}",
    );
}

#[test]
fn roundtrip_const_expr_globals() {
    // 3.3 常量表达式：ptrtoint 两例（i32/i64）+ inttoptr + bitcast + addrspacecast
    // + getelementptr + 嵌套。display 必须原样还原表达式文本（字节是占位）。
    // 注：裸全局引用 init（`@p = global ptr @h`）与 LALR(1) 模块级歧义冲突，
    // 本 parser 暂不支持（用 `ptrtoint (ptr @h to ptr)` 等价表达）。
    assert_roundtrip(
        "@h = global i32 0
@g32 = global i32 ptrtoint (ptr @h to i32)
@g64 = global i64 ptrtoint (ptr @h to i64)
@i2p = global ptr inttoptr (i64 42 to ptr)
@bc = global ptr bitcast (ptr @h to ptr)
@gp = global ptr getelementptr (i32, ptr @h, i64 2)
@gp2 = global ptr getelementptr (i32, ptr @h, i32 1, i64 3)
@nested = global i32 ptrtoint (bitcast (ptr @h to ptr) to i32)
define i32 @main() {
  %entry:
    ret i32 0
}",
    );
}

#[test]
fn roundtrip_const_expr_addrsize() {
    // addrspacecast 常量表达式（to_ty 为 ptr addrspace(1)）
    assert_roundtrip(
        "@h = global i32 0
@asc = global ptr addrspace(1) addrspacecast (ptr @h to ptr addrspace(1))
define i32 @main() {
  %entry:
    ret i32 0
}",
    );
}

#[test]
fn parse_rejects_unknown_const_expr_op() {
    // 未知常量表达式操作名 → 语义错误
    assert!(
        forge_ir::ir_parser::parse_module(
            "@h = global i32 0
@g = global i32 frobnicate (ptr @h to i32)
define i32 @main() {
  %entry:
    ret i32 0
}"
        )
        .is_err(),
        "unknown constant expression op must be rejected"
    );
}

#[test]
fn roundtrip_exception_invoke_landingpad() {
    // 3.2 P1.1 异常处理文本层：invoke/landingpad/resume/personality round-trip。
    // 文本层仅解析/展示；codegen 对含异常函数报 Unsupported。
    assert_roundtrip(
        "declare void @__gxx_personality_v0()
declare i32 @__cxa_throw(ptr, ptr, ptr)
declare void @may_throw(i32)
define void @f() personality ptr @__gxx_personality_v0 {
  %entry:
    invoke void @may_throw(i32 1) to label %ok unwind label %pad
  %ok:
    ret void
  %pad:
    %l = landingpad { ptr, i32 } cleanup
    resume { ptr, i32 } %l
}",
    );
}

#[test]
fn roundtrip_exception_invoke_retval() {
    // invoke 带返回值类型（LLVM：`invoke i32 @f() to label %ok unwind label %pad`；
    // 返回值文本层丢弃——normal 块参数传递为长期增强）
    assert_roundtrip(
        "declare void @__gxx_personality_v0()
declare i32 @may_throw(i32)
define i32 @f() personality ptr @__gxx_personality_v0 {
  %entry:
    invoke i32 @may_throw(i32 1) to label %ok unwind label %pad
  %ok:
    ret i32 0
  %pad:
    %l = landingpad { ptr, i32 } cleanup
    resume { ptr, i32 } %l
}",
    );
}

#[test]
fn roundtrip_exception_invoke_result_binding() {
    // 4.2 边界：`%r = invoke i32 ...` 返回值绑定——normal 块 phi 参数 0 接收
    // 返回值（LLVM：invoke 返回值即 normal 块第一个参数）；%r 后续可引用。
    assert_roundtrip(
        "declare void @__gxx_personality_v0()
declare i32 @may_throw(i32)
define i32 @f() personality ptr @__gxx_personality_v0 {
  %entry:
    %r = invoke i32 @may_throw(i32 1) to label %ok unwind label %pad
  %ok:
    %okv = phi i32 [ 0, %entry ]
    %s = add i32 %okv, i32 1
    ret i32 %s
  %pad:
    %l = landingpad { ptr, i32 } cleanup
    resume { ptr, i32 } %l
}",
    );
}

#[test]
fn parse_accepts_invoke_forward_reference() {
    // 第十一轮语义变更：invoke/call 到未声明函数（含前向 ifunc 等）——
    // LLVM 隐式声明允许，注册宽松占位（void() 签名）
    assert!(
        forge_ir::ir_parser::parse_module(
            "define void @f() {
  %entry:
    invoke void @missing() to label %ok unwind label %pad
  %ok:
    ret void
  %pad:
    unreachable
}"
        )
        .is_ok(),
        "invoke to forward-referenced function must be accepted (implicit declaration)"
    );
}

#[test]
fn roundtrip_agg_const_nested_extract() {
    // 3.1 聚合常量 Value：嵌套提取（结果是聚合）+ insertvalue 聚合字面量
    assert_roundtrip(
        "define i32 @f() {
  %entry:
    %c = extractvalue [2 x [2 x i32]] [[2 x i32] [i32 1, i32 2], [2 x i32] [i32 3, i32 4]], 1
    %d = extractvalue [2 x i32] %c, 0
    %n = insertvalue [4 x i32] [i32 1, i32 2, i32 3, i32 4], i32 7, 2
    ret i32 %d
}",
    );
}

#[test]
fn roundtrip_agg_const_struct_extract() {
    // 结构体聚合：{ i32, i64 } 提取标量元素
    assert_roundtrip(
        "define i64 @f() {
  %entry:
    %c = extractvalue { i32, i64 } { i32 1, i64 2 }, 1
    ret i64 %c
}",
    );
}

#[test]
fn roundtrip_exception_landingpad_clauses() {
    // 3.2 延伸：landingpad catch/filter 子句列表（clang -fexceptions C++ 典型输出）
    assert_roundtrip(
        "declare void @__gxx_personality_v0()
declare ptr @__cxa_begin_catch(ptr)
declare void @__cxa_call_unexpected(ptr)
declare void @may_throw(i32)
define void @f() personality ptr @__gxx_personality_v0 {
  %entry:
    invoke void @may_throw(i32 1) to label %ok unwind label %pad
  %ok:
    ret void
  %pad:
    %l = landingpad { ptr, i32 } cleanup, catch ptr @__cxa_begin_catch
    resume { ptr, i32 } %l
  %pad2:
    %l2 = landingpad { ptr, i32 } catch ptr @__cxa_begin_catch, filter ptr @__cxa_call_unexpected
    resume { ptr, i32 } %l2
}",
    );
}

#[test]
fn parse_rejects_unknown_landingpad_clause() {
    // landingpad 未知子句 → 语义错误
    assert!(
        forge_ir::ir_parser::parse_module(
            "define void @f() {
  %e:
    %l = landingpad { ptr, i32 } frobnicate
    resume { ptr, i32 } %l
}"
        )
        .is_err(),
        "unknown landingpad clause must be rejected"
    );
}

#[test]
fn roundtrip_global_alias() {
    // 4.3 补充：模块级别名 alias（TypeOp / GEP / addrspacecast aliasee）
    assert_roundtrip(
        "@global = global i32 0
@bar = internal alias i32, ptr @global
@ia = internal alias ptr addrspace(2), addrspacecast (ptr addrspace(1) @global to ptr addrspace(3))
@ref = alias i32, getelementptr inbounds (i32, ptr @bar, i64 1)
define void @f() {
  %entry:
    ret void
}",
    );
}

#[test]
fn roundtrip_uwtable_byref_attrs() {
    // 2.2 高频函数/参数属性（开放集合专用 token）
    assert_roundtrip(
        "define void @foo() uwtable nosync {
  %entry:
    ret void
}",
    );
    assert_roundtrip(
        "declare void @bar(ptr byref(ptr) %p)
define void @foo(ptr byref(ptr) %q) {
  %entry:
    ret void
}",
    );
}

#[test]
fn roundtrip_packed_struct() {
    // 2.1 packed struct 现代语法：`<{ i32, i32 }>`（类型/字面量/TypeDef）
    assert_roundtrip(
        "define void @foo(ptr %x) nounwind {
  %entry:
    store <{i32, i32}><{i32 7, i32 9}>, ptr %x
    ret void
}",
    );
    assert_roundtrip("@g = global <{i32, i32}> <{i32 1, i32 2}>\n");
    assert_roundtrip(
        "%X = type <{ i32, i64 }>
define ptr @f(ptr %s) {
  %entry:
    ret ptr %s
}",
    );
}

#[test]
fn roundtrip_agg_literal_store_ret() {
    // 3.1 剩余：store/ret 聚合常量字面量（AggConst 值表示）
    assert_roundtrip(
        "define void @foo(ptr %x) nounwind {
  %entry:
    store {i32, i32}{i32 7, i32 9}, ptr %x
    store [2 x i32][i32 7, i32 9], ptr %x
    ret void
}",
    );
    // ret 聚合字面量（单元素）
    assert_roundtrip(
        "define { i32 } @foob() nounwind {
  %entry:
    ret {i32}{ i32 0 }
}
define [1 x i32] @food() nounwind {
  %entry:
    ret [1 x i32][ i32 0 ]
}",
    );
}

#[test]
fn roundtrip_f32_constant_inline() {
    // f32 常量不再硬编码 f64（display 按值类型输出 f32/float 文本）
    assert_roundtrip(
        "define float @f() {
  %entry:
    %r = fadd float 1.5, float 2.5
    ret float %r
}",
    );
}

#[test]
fn roundtrip_vector_div() {
    // forge Vdiv 的 LLVM 文本名 "div"（曾读不回）
    assert_roundtrip(
        "define <4 x i32> @vd(<4 x i32> %a, <4 x i32> %b) {
  %entry:
    %r = div <4 x i32> %a, <4 x i32> %b
    ret <4 x i32> %r
}",
    );
}

// ── 输出格式断言（严格 LLVM）───────────────────────────────

#[test]
fn display_strict_llvm_format() {
    let m = parse_module(
        "define i32 @add(i32 %a, i32 %b) {
  %entry:
    %s = add i32 %a, i32 %b
    ret i32 %s
}",
    )
    .unwrap();
    let text = m.to_string();
    assert!(
        text.contains("define i32 @add(i32 %a, i32 %b)"),
        "define header: {text}"
    );
    assert!(text.contains("%entry:"), "block label: {text}");
    assert!(
        text.contains("%s = add i32 %a, i32 %b"),
        "typed operands: {text}"
    );
    assert!(text.contains("ret i32 %s"), "ret: {text}");
    assert!(!text.contains("fn "), "no old forge fn header: {text}");
    assert!(!text.contains("iadd"), "no forge mnemonic: {text}");
}

#[test]
fn display_constant_inline_format() {
    let m = parse_module(
        "define i32 @c() {
  %entry:
    %r = add i32 40, i32 2
    ret i32 %r
}",
    )
    .unwrap();
    let text = m.to_string();
    // iconst 内联为操作数常量，无独立 iconst 指令行
    assert!(text.contains("add i32 40, i32 2"), "inline const: {text}");
    assert!(!text.contains("iconst"), "no iconst instruction: {text}");
}

// ── 名字消歧 round-trip ────────────────────────────────────

#[test]
fn roundtrip_duplicate_names_disambiguated() {
    // 同名绑定 → display 消歧 %x / %x_1 → parse 成功且结构等价
    assert_roundtrip(
        "define i32 @f(i32 %x) {
  %entry:
    %x1 = add i32 %x, i32 1
    %y = add i32 %x1, i32 %x1
    ret i32 %y
}",
    );
}

// ── forge 扩展指令 round-trip ──────────────────────────────

#[test]
fn roundtrip_stack_addr() {
    // offset 现在真正打印并读回（断言比较 immediates）
    assert_roundtrip(
        "define ptr @f() {
  %entry:
    %p = stack_addr i64 -4
    ret ptr %p
}",
    );
}

#[test]
fn roundtrip_copy() {
    assert_roundtrip(
        "define i32 @f(i32 %x) {
  %entry:
    %y = copy i32 %x
    ret i32 %y
}",
    );
}

#[test]
fn roundtrip_call_indirect() {
    assert_roundtrip(
        "define i32 @main(i32 %x) {
  %entry:
    %p = stack_addr i64 -4
    %v = call i32 %p(i32 %x)
    ret i32 %v
}",
    );
}

#[test]
fn roundtrip_fconst_inline() {
    assert_roundtrip(
        "define double @f() {
  %entry:
    %r = fadd double 1.5, double 2.5
    ret double %r
}",
    );
}

// ── 全量 Assembler 用例 roundtrip（第二十九轮）─────────────────
// 对 llvm_assembler_cases 全部正向用例做 display→reparse 往返,
// 系统性验证 display 完整性（比随机 fuzz 更贴近真实 LLVM 输入）。
//
// 第二十九轮状态:首次运行暴露 74 个 display 保真度缺口(124 过/254 skip),
// 分批次修复(第二十九~三十轮:74 → 27 → 9 → 2 → 0);第三十轮收尾后
// **452 用例 display→reparse 全部通过**(见 docs/forge-ir-display-gaps.md),
// 本测试取消 ignore 作为常规回归。

#[test]
fn roundtrip_all_assembler_cases() {
    use forge_ir::ir_parser::parse_module;
    use std::panic::catch_unwind;
    // is_negative 判定与 llvm_assembler_compat.rs 保持一致（RUN 指令 + 文件名关键字）
    let is_neg = |name: &str, src: &str| -> bool {
        if src.lines().take(6).any(|l| l.contains("not llvm-as")) {
            return true;
        }
        let has_asm_run = src.lines().take(6).any(|l| l.contains("llvm-as"));
        let is_split = src.lines().take(6).any(|l| l.contains("split-file"));
        (name.contains("error")
            || name.contains("parse-error")
            || name.contains("invalid")
            || name.contains("redefinition"))
            && (has_asm_run || is_split)
    };
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/llvm_assembler_cases");
    let mut checked = 0usize;
    let mut skipped = 0usize;
    let mut failures = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("cases dir")
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ll") {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).expect("read");
        if is_neg(&name, &src) {
            skipped += 1;
            continue;
        }
        let src = src.split(";---").next().unwrap_or(&src).to_string();
        match parse_module(&src) {
            Ok(m1) => {
                let text = m1.to_string();
                match parse_module(&text) {
                    Ok(m2) => {
                        if let Err(e) = catch_unwind(|| assert_modules_eq(&m1, &m2, &text)) {
                            let msg = e
                                .downcast_ref::<String>()
                                .cloned()
                                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                                .unwrap_or_else(|| "panic (no message)".to_string());
                            failures.push(format!("{name}: {msg}"));
                        } else {
                            checked += 1;
                        }
                    }
                    Err(e) => failures.push(format!("{name}: reparse failed: {e}")),
                }
            }
            Err(_) => skipped += 1,
        }
    }
    eprintln!(
        "roundtrip_all_assembler_cases: checked={checked} skipped={skipped} failures={}",
        failures.len()
    );
    for f in &failures {
        eprintln!("ROUNDTRIP-FAIL: {f}");
    }
    if !failures.is_empty() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("roundtrip_failures.txt");
        let _ = std::fs::write(p, failures.join("\n"));
    }
    assert!(
        failures.is_empty(),
        "roundtrip failures ({}):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
