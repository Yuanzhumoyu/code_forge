//! LLVM IR Display round-trip：parse → display → parse 结构等价。
//! forge IR 的 Display 输出严格 LLVM 文本，可被新解析器完整读回。

use forge_ir::function::Module;
use forge_ir::ir_parser::parse_module;

/// parse₁ → display → parse₂，断言两次 forge IR 结构等价
/// （函数数/块数/指令 opcode+cond/操作数索引/终结符；类型索引跨 store 不比较）。
fn assert_roundtrip(src: &str) {
    let m1 = parse_module(src).unwrap_or_else(|e| panic!("parse1 failed for:\n{src}\n{e}"));
    let text = m1.to_string();
    let m2 = parse_module(&text).unwrap_or_else(|e| panic!("reparse failed for:\n{text}\n{e}"));
    assert_modules_eq(&m1, &m2, &text);
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
            assert_eq!(b1.inst_order.len(), b2.inst_order.len(), "inst count blk");
            for (i1, i2) in b1.inst_order.iter().zip(b2.inst_order.iter()) {
                let a = &f1.dfg.insts[i1.0 as usize];
                let b = &f2.dfg.insts[i2.0 as usize];
                assert_eq!(a.opcode, b.opcode, "opcode:\n{text}");
                assert_eq!(
                    a.operands, b.operands,
                    "operands of {:?}:\n{text}",
                    a.opcode
                );
            }
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
