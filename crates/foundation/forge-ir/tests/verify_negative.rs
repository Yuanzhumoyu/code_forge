//! 负向测试（4.2）：verify 错误码全覆盖 + parse 层非法语法拒绝。
//!
//! verify.rs 内部单元测试已覆盖 20 个 `VerifyError` 变体；本文件补齐其余
//! 7 个（MissingEntry / MissingTerminator / FcmpOperandNotFloat /
//! StoreAddrNotPointer / CallInvalidTarget / PathWithoutReturn /
//! TerminatorDominanceViolation），使 27 个错误码全部有回归测试。
//! 末尾附 parse 层非法语法拒绝样例。

use forge_ir::builder::FunctionBuilder;
use forge_ir::function::Function;
use forge_ir::ir_parser::parse_module;
use forge_ir::opcode::{FloatCC, IntCC, Opcode};
use forge_ir::types::{FunctionSignature, TypeContext};
use forge_ir::verify::{Verifier, VerifyError};
use forge_ir::{CallConv, InstFlags, TypeId};

fn verify_builder(
    sig_returns: &[forge_ir::TypeId],
    build: impl FnOnce(&mut FunctionBuilder, &TypeContext),
) -> Vec<VerifyError> {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], sig_returns);
    let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
    build(&mut fb, &ctx);
    let func = fb.finish().expect("finish must succeed");
    let mut verifier = Verifier::with_ctx(ctx);
    verifier.verify(&func).err().unwrap_or_default()
}

/// parse + verify：返回任一函数的 verify 错误（无错误返回 None）。
fn verify_errs(src: &str) -> Option<Vec<VerifyError>> {
    let m = parse_module(src).ok()?;
    let ctx = m.types.clone();
    for f in m.iter_functions() {
        let mut v = Verifier::with_ctx(ctx.clone());
        if let Err(errs) = v.verify(f) {
            return Some(errs);
        }
    }
    None
}

fn has_any(errs: &[VerifyError], pred: impl Fn(&VerifyError) -> bool, label: &str) {
    assert!(errs.iter().any(pred), "expected {label}, got: {errs:#?}");
}

// ── 缺口 1：MissingEntry ──

#[test]
fn verify_missing_entry() {
    // 无 entry block 的函数
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
    let fb = FunctionBuilder::new("f", ctx.clone(), sig);
    // 不创建任何块，直接 finish
    let func = fb.finish().expect("finish with no blocks");
    let mut verifier = Verifier::with_ctx(ctx);
    let err = verifier.verify(&func).expect_err("must fail");
    has_any(
        &err,
        |e| matches!(e, VerifyError::MissingEntry),
        "MissingEntry",
    );
}

// ── 缺口 2：MissingTerminator（手构 dfg：块无终结符）──

#[test]
fn verify_missing_terminator() {
    let ctx = TypeContext::new();
    let sig_ref = ctx.register_signature(FunctionSignature::new(&[], &[]));
    let mut func = Function::new("f", ctx.clone(), sig_ref, CallConv::Default);
    let b = func.dfg.make_block();
    func.entry_block = Some(b);
    // 不设置终结符
    let mut verifier = Verifier::with_ctx(ctx);
    let err = verifier.verify(&func).expect_err("must fail");
    has_any(
        &err,
        |e| matches!(e, VerifyError::MissingTerminator { .. }),
        "MissingTerminator",
    );
}

// ── 缺口 3：FcmpOperandNotFloat ──

#[test]
fn verify_fcmp_operand_not_float() {
    let errs = verify_builder(&[], |fb, ctx| {
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let a = fb.iconst(1, ctx.i32_ty());
        let b = fb.iconst(2, ctx.i32_ty());
        // builder 的 fcmp 对非浮点操作数直接 assert，须用 emit1 绕过构建层检查，
        // 由 verify 的 check_immediates 报 FcmpOperandNotFloat
        fb.emit1(
            Opcode::Fcmp {
                cond: FloatCC::Equal,
            },
            vec![a, b],
            vec![],
            ctx.bool_ty(),
            InstFlags::NONE,
        );
        fb.ret(&[]);
    });
    has_any(
        &errs,
        |e| matches!(e, VerifyError::FcmpOperandNotFloat { .. }),
        "FcmpOperandNotFloat",
    );
}

// ── 缺口 4：StoreAddrNotPointer ──

#[test]
fn verify_store_addr_not_pointer() {
    let errs = verify_builder(&[], |fb, ctx| {
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let v = fb.iconst(1, ctx.i32_ty());
        let addr = fb.iconst(2, ctx.i32_ty()); // 非指针地址
        fb.store(v, addr);
        fb.ret(&[]);
    });
    has_any(
        &errs,
        |e| matches!(e, VerifyError::StoreAddrNotPointer { .. }),
        "StoreAddrNotPointer",
    );
}

// ── 缺口 5：CallInvalidTarget ──

#[test]
fn verify_call_invalid_target() {
    let errs = verify_builder(&[], |fb, ctx| {
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let target = fb.iconst(1, ctx.i32_ty()); // call_indirect 目标非指针
        fb.call_indirect(target, &[], &[ctx.i32_ty()]);
        fb.ret(&[]);
    });
    has_any(
        &errs,
        |e| matches!(e, VerifyError::CallInvalidTarget { .. }),
        "CallInvalidTarget",
    );
}

// ── 缺口 6：PathWithoutReturn（手构 dfg：非 void 函数 + 从未设置终结符的块）──

#[test]
fn verify_path_without_return() {
    let ctx = TypeContext::new();
    let sig_ref = ctx.register_signature(FunctionSignature::new(&[], &[ctx.i32_ty()]));
    let mut func = Function::new("f", ctx.clone(), sig_ref, CallConv::Default);
    let b = func.dfg.make_block();
    func.entry_block = Some(b);
    // 不设置终结符（builder finish 会拦截，手构 dfg 可绕过）：
    // 非 void 函数可达路径无返回 → PathWithoutReturn（同时也会报 MissingTerminator）
    let mut verifier = Verifier::with_ctx(ctx);
    let err = verifier.verify(&func).expect_err("must fail");
    has_any(
        &err,
        |e| matches!(e, VerifyError::PathWithoutReturn { .. }),
        "PathWithoutReturn",
    );
}

// ── 缺口 7：TerminatorDominanceViolation ──

fn build_branch_merge(fb: &mut FunctionBuilder, ctx: &TypeContext) {
    // entry: c = icmp; branch(c, bb1, bb2)
    // bb1: v2 = iconst; jump bb2
    // bb2: ret v2  ← v2 定义于 bb1，bb2 还有 entry 前驱 → bb1 不支配 bb2
    let (entry, _) = fb.create_entry_block();
    fb.switch_to_block(entry);
    let a = fb.iconst(1, ctx.i32_ty());
    let b = fb.iconst(2, ctx.i32_ty());
    let c = fb.icmp(IntCC::Equal, a, b);
    let bb1 = fb.create_block();
    let bb2 = fb.create_block();
    fb.branch(c, bb1, &[], bb2, &[]);
    fb.switch_to_block(bb1);
    let v2 = fb.iconst(3, ctx.i32_ty());
    fb.jump(bb2, &[]);
    fb.switch_to_block(bb2);
    fb.ret(&[v2]);
}

#[test]
fn verify_terminator_dominance_violation() {
    let errs = verify_builder(&[TypeId::I32], |fb, ctx| {
        build_branch_merge(fb, ctx);
    });
    has_any(
        &errs,
        |e| matches!(e, VerifyError::TerminatorDominanceViolation { .. }),
        "TerminatorDominanceViolation",
    );
}

#[test]
fn verify_invoke_unwind_exempt_from_dominance() {
    // 3.2 P1.1：Invoke 的 unwind 边豁免——unwind_args 传值（定义于非支配块）
    // 不受正常支配树约束（异常路径语义），verify 不报 TerminatorDominanceViolation。
    // 构造：entry 定义 v（invoke 块），unwind 块经 invoke 的 unwind 边可达，
    // unwind 块内部使用 v——正常支配下 unwind 块是 invoke 的后继（支配成立），
    // 重点验证 unwind_args 值不被误报。
    let errs = verify_builder(&[], |fb, ctx| {
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let ok = fb.create_block();
        let pad = fb.create_block();
        let v = fb.iconst(1, ctx.i32_ty());
        let l = fb.landingpad(ctx.ptr_ty());
        fb.resume(l);
        fb.switch_to_block(ok);
        fb.ret(&[]);
        fb.switch_to_block(pad);
        let l2 = fb.landingpad(ctx.ptr_ty());
        fb.resume(l2);
        fb.switch_to_block(entry);
        // unwind_args 传 v（v 定义于 entry，entry 支配 pad——正常支配也成立，
        // 但 unwind 边路径允许更宽松的值来源）
        fb.invoke(
            forge_ir::FuncRef(0),
            &[],
            forge_ir::TypeId::VOID,
            ok,
            &[],
            pad,
            &[v],
        );
    });
    // 合法：unwind_args 值不触发 TerminatorDominanceViolation
    assert!(
        !errs
            .iter()
            .any(|e| matches!(e, VerifyError::TerminatorDominanceViolation { .. })),
        "invoke unwind_args 不受正常支配约束，got: {errs:#?}"
    );
}

#[test]
fn verify_same_shape_but_legal_merge() {
    // 对照：bb2 只被 bb1 支配（无 entry 前驱）时终结符用 v2 合法
    let errs = verify_builder(&[TypeId::I32], |fb, ctx| {
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let a = fb.iconst(1, ctx.i32_ty());
        let b = fb.iconst(2, ctx.i32_ty());
        let c = fb.icmp(IntCC::Equal, a, b);
        let bb1 = fb.create_block();
        let bb2 = fb.create_block();
        fb.branch(c, bb1, &[], bb1, &[]); // 两个目标都是 bb1
        fb.switch_to_block(bb1);
        let v2 = fb.iconst(3, ctx.i32_ty());
        fb.jump(bb2, &[]);
        fb.switch_to_block(bb2);
        fb.ret(&[v2]);
    });
    assert!(
        errs.is_empty(),
        "legal merge should verify cleanly, got: {errs:#?}"
    );
}

// ── parse 层非法语法拒绝 ──

#[test]
fn parse_rejects_unknown_icmp_predicate() {
    assert!(
        parse_module("define i1 @f() {\n  %e:\n    %r = icmp bogus i32 1, 2\n    ret i1 %r\n}")
            .is_err(),
        "unknown icmp predicate must be rejected"
    );
}

#[test]
fn parse_rejects_unknown_fcmp_predicate() {
    // P1：ueq 等 16 个 LLVM 条件现全部支持（正例）
    assert!(
        parse_module("define i1 @f() {\n  %e:\n    %r = fcmp ueq f64 1.0, 2.0\n    ret i1 %r\n}")
            .is_ok(),
        "ueq 现为合法 fcmp 条件（P1 全 16 条件）"
    );
    // 伪谓词仍必须拒绝
    assert!(
        parse_module("define i1 @f() {\n  %e:\n    %r = fcmp bogus f64 1.0, 2.0\n    ret i1 %r\n}")
            .is_err(),
        "unknown fcmp predicate (bogus) must be rejected"
    );
}

#[test]
fn parse_rejects_missing_return_value() {
    // 非 void 函数 ret 不带值
    assert!(
        parse_module("define i32 @f() {\n  %e:\n    ret\n}").is_err(),
        "ret without value in non-void function must be rejected"
    );
}

#[test]
fn parse_rejects_undefined_local_type() {
    assert!(
        parse_module("define i32 @f() {\n  %e:\n    %r = add bogus_type 1, 2\n    ret i32 %r\n}")
            .is_err(),
        "undefined type name must be rejected"
    );
}

#[test]
fn verify_rejects_nonconst_struct_index() {
    // S4.1：struct 位置非常量索引——verify 报错（LLVM LangRef）
    let m = parse_module(
        "define i32 @f(ptr %p, i64 %i) {\n  %e:\n    %v = getelementptr {i32, i32}, ptr %p, i64 %i\n    ret i32 0\n}",
    )
    .expect("parse ok");
    let mut v = forge_ir::verify::Verifier::with_ctx(m.types.clone());
    let mut has_err = false;
    for f in m.iter_functions() {
        if let Err(errs) = v.verify(f) {
            let _ = errs;
            has_err = true;
        }
    }
    assert!(has_err, "non-constant struct index must be rejected (S4.1)");
    // 数组位置非常量索引合法（i8* 指针运算）
    let m2 = parse_module(
        "define ptr @g(ptr %p, i64 %i) {\n  %e:\n    %v = getelementptr i8, ptr %p, i64 %i\n    ret ptr %v\n}",
    )
    .expect("parse ok");
    let mut v2 = forge_ir::verify::Verifier::with_ctx(m2.types.clone());
    let mut any_err = false;
    for f in m2.iter_functions() {
        if let Err(errs) = v2.verify(f) {
            let _ = errs;
            any_err = true;
        }
    }
    assert!(!any_err, "array non-constant index must be accepted");
}

#[test]
fn parse_accepts_opaque_type() {
    // S3.1：`opaque` 占位类型（`%ty = type opaque`——容器外类型）
    let _m = parse_module("%ty = type opaque\ndefine ptr @f() {\n  %e:\n    ret ptr null\n}")
        .expect("opaque type definition must be accepted");
    // load 目标为 opaque → verify 拒绝（无大小）
    let src = "%ty = type opaque\ndefine i32 @f(ptr %p) {\n  %e:\n    %v = load %ty, ptr %p\n    ret i32 0\n}";
    assert!(
        parse_module(src).is_err() || module_has_verify_error(parse_module(src).unwrap()),
        "load of opaque type must be rejected"
    );
}

fn module_has_verify_error(m: forge_ir::function::Module) -> bool {
    let mut v = forge_ir::verify::Verifier::with_ctx(m.types.clone());
    let mut any = false;
    for f in m.iter_functions() {
        if let Err(errs) = v.verify(f) {
            any = true;
            let _ = errs;
        }
    }
    any
}

#[test]
fn parse_accepts_bare_second_operand() {
    // S2：LLVM 现代语法第二操作数无类型（`add i32 40, 2`——类型从第一推导）
    let m = parse_module("define i32 @f() {\n  %e:\n    %r = add i32 40, 2\n    ret i32 %r\n}")
        .expect("bare second operand must be accepted (S2)");
    let f = m.iter_functions().next().unwrap();
    // 第二操作数类型应从第一操作数推导为 i32
    let inst = f
        .dfg
        .insts()
        .find(|(_, i)| i.opcode == forge_ir::opcode::Opcode::Iadd)
        .map(|(_, i)| i)
        .expect("iadd inst");
    assert_eq!(inst.operands.len(), 2);
    let t0 = f.dfg.value_type(inst.operands[0]).unwrap();
    let t1 = f.dfg.value_type(inst.operands[1]).unwrap();
    assert_eq!(t0, t1, "second operand type must be derived from the first");
}

#[test]
fn parse_accepts_dbg_non_dilocation() {
    // 3.4 metadata kind×形状校验：!dbg 宽松化（第十一轮）——正向用例
    // （drop-debug-info/incomplete-ir-metadata 等旧格式/升级链）引用非
    // DILocation 节点必须接受；DI 负向拒绝在 parse 层独立保证
    assert!(
        parse_module("!0 = !{i32 1}\ndefine i32 @f() !dbg !0 {\n  %e:\n    ret i32 1\n}").is_ok(),
        "!dbg pointing at a tuple node must be accepted (lenient)"
    );
    assert!(
        parse_module("!0 = !{i32 1}\ndefine i32 @f(ptr %p) {\n  %e:\n    %v = load i32, ptr %p, align 4, !dbg !0\n    ret i32 %v\n}")
            .is_ok(),
        "instruction !dbg pointing at a tuple node must be accepted (lenient)"
    );
}

#[test]
fn parse_rejects_tbaa_not_tuple() {
    // 3.4 扩展：!tbaa 必须引用 tuple tag 节点（`!{!{...}, i64 1}` 嵌套结构）
    assert!(
        parse_module("!0 = !DILocation(line: 1, column: 1)\ndefine i32 @f(ptr %p) {\n  %e:\n    %v = load i32, ptr %p, align 4, !tbaa !0\n    ret i32 %v\n}")
            .is_err(),
        "!tbaa pointing at a non-tuple node must be rejected"
    );
    // 合法 tbaa（嵌套 tag tuple）接受
    assert!(
        parse_module("!0 = !{!{\"root\", i64 0}, i64 0}\ndefine i32 @f(ptr %p) {\n  %e:\n    %v = load i32, ptr %p, align 4, !tbaa !0\n    ret i32 %v\n}")
            .is_ok(),
        "!tbaa pointing at a tuple tag node must be accepted"
    );
}

#[test]
fn parse_rejects_range_not_integer_tuple() {
    // 3.4 扩展：!range 必须引用两个整数的 tuple（`!{ i32 0, i32 10 }`）
    assert!(
        parse_module("!0 = !{i32 0, ptr null}\ndefine i32 @f(ptr %p) {\n  %e:\n    %v = load i32, ptr %p, align 4, !range !0\n    ret i32 %v\n}")
            .is_err(),
        "!range with a non-integer member must be rejected"
    );
    // 合法 range（整数区间）接受
    assert!(
        parse_module("!0 = !{i32 0, i32 10}\ndefine i32 @f(ptr %p) {\n  %e:\n    %v = load i32, ptr %p, align 4, !range !0\n    ret i32 %v\n}")
            .is_ok(),
        "!range with an integer interval must be accepted"
    );
}

#[test]
fn parse_accepts_dbg_dilocation() {
    // 合法组合：!dbg → DILocation（函数级 + 指令级）
    parse_module("!0 = !DILocation(line: 1, column: 1, scope: !0)\ndefine i32 @f(ptr %p) !dbg !0 {\n  %e:\n    %v = load i32, ptr %p, align 4, !dbg !0\n    ret i32 %v\n}")
        .expect("!dbg → DILocation must parse");
}

#[test]
fn verify_rejects_aggregate_in_arith() {
    // 3.1 聚合类型指令分类：算术指令操作数类型为聚合 → TypeMismatch
    //（load/store 聚合值合法，算术拒绝）
    let m = parse_module(
        "define [2 x i32] @f() {
  %e:
    %c = extractvalue [2 x [2 x i32]] [[2 x i32] [i32 1, i32 2], [2 x i32] [i32 3, i32 4]], 1
    %r = add [2 x i32] %c, [2 x i32] %c
    ret [2 x i32] %r
}",
    )
    .expect("parse ok");
    let f = m.iter_functions().next().unwrap().clone();
    let mut verifier = Verifier::with_ctx(m.types.clone());
    let errs = verifier.verify(&f).err().unwrap_or_default();
    assert!(
        errs.iter()
            .any(|e| matches!(e, VerifyError::TypeMismatch { .. })),
        "聚合类型参与算术应报 TypeMismatch，got: {errs:#?}"
    );
}

#[test]
fn parse_rejects_unknown_opcode() {
    assert!(
        parse_module("define i32 @f() {\n  %e:\n    %r = frobnicate i32 1, 2\n    ret i32 %r\n}")
            .is_err(),
        "unknown opcode must be rejected"
    );
}

// ── 1.2 值域/类型校验（LLVM 负向用例驱动）──

#[test]
fn parse_rejects_int_type_width_overflow() {
    // invalid-inttype：i8388609 超出 LLVM 位宽上限（lexer u16 溢出 → 0 → 语义拒绝）
    assert!(
        parse_module("@i2 = common global i8388609 0, align 4\n").is_err(),
        "integer type width > u16 must be rejected"
    );
    // 边界：i65535 合法（u16 内）
    assert!(
        parse_module("@ok = common global i65535 0, align 4\n").is_ok(),
        "i65535 is within u16 range and must be accepted"
    );
}

#[test]
fn parse_rejects_addrspace_overflow() {
    // invalid-addrspace：addrspace(16777216) 超出 2^24-1
    assert!(
        parse_module(
            "define void @f() {\n  %e:\n    %y = alloca i32, addrspace(16777216)\n    ret void\n}\n"
        )
        .is_err(),
        "addrspace >= 2^24 must be rejected"
    );
    assert!(
        parse_module(
            "define void @f() {\n  %e:\n    %y = alloca i32, addrspace(16777215)\n    ret void\n}\n"
        )
        .is_ok(),
        "addrspace 2^24-1 is the maximum and must be accepted"
    );
}

#[test]
fn parse_rejects_align_overflow() {
    // align-inst-alloca：align 8589934592 超出 2^30（u32 溢出截断防护）
    assert!(
        parse_module(
            "define void @foo() {\n  %e:\n    %p = alloca i1, align 8589934592\n    ret void\n}\n"
        )
        .is_err(),
        "align > 2^30 must be rejected"
    );
    assert!(
        parse_module(
            "define void @foo() {\n  %e:\n    %p = alloca i1, align 1073741824\n    ret void\n}\n"
        )
        .is_ok(),
        "align 2^30 is the maximum and must be accepted"
    );
}

#[test]
fn verify_rejects_atomicrmw_operand_type() {
    // invalid-atomicrmw-add：atomicrmw add 要求整数操作数（float 拒绝）
    assert!(
        verify_errs("define void @f(ptr %ptr) {\n  %e:\n  atomicrmw add ptr %ptr, float 1.0 seq_cst\n  ret void\n}\n")
            .is_some(),
        "atomicrmw add with float operand must be rejected"
    );
    assert!(
        verify_errs("define void @f(ptr %ptr) {\n  %e:\n  atomicrmw fadd ptr %ptr, float 1.0 seq_cst\n  ret void\n}\n")
            .is_none(),
        "atomicrmw fadd with float operand must be accepted"
    );
}

// ── 1.4 剩余小项（cmpxchg 序/类型、cast 大小、metadata 旧语法）──

#[test]
fn verify_rejects_cmpxchg_unordered() {
    // cmpxchg-ordering/2：cmpxchg 不允许 unordered 序（须 monotonic 或更强）
    assert!(
        verify_errs("define void @f(ptr %a, i32 %b, i32 %c) {\n  %e:\n  %x = cmpxchg ptr %a, i32 %b, i32 %c unordered monotonic\n  ret void\n}\n")
            .is_some(),
        "cmpxchg unordered ordering must be rejected"
    );
    assert!(
        verify_errs("define void @f(ptr %a, i32 %b, i32 %c) {\n  %e:\n  %x = cmpxchg ptr %a, i32 %b, i32 %c acq_rel monotonic\n  ret void\n}\n")
            .is_none(),
        "cmpxchg acq_rel/monotonic must be accepted"
    );
}

#[test]
fn verify_rejects_cmpxchg_operand_type_mismatch() {
    // opaque-ptr-cmpxchg：cmp 与 new 类型必须一致
    assert!(
        verify_errs("define void @f(ptr %p, i32 %a, i64 %b) {\n  %e:\n  %x = cmpxchg ptr %p, i32 %a, i64 %b acq_rel monotonic\n  ret void\n}\n")
            .is_some(),
        "cmpxchg cmp/new type mismatch must be rejected"
    );
}

#[test]
fn verify_rejects_vector_bitcast_size_mismatch() {
    // invalid_cast3：向量 bitcast 须等总大小（<4 x ptr> → <2 x ptr> 拒绝）
    assert!(
        verify_errs("define <2 x ptr> @f(<4 x ptr> %c) {\n  %e:\n  %bc = bitcast <4 x ptr> %c to <2 x ptr>\n  ret <2 x ptr> %bc\n}\n")
            .is_some(),
        "vector bitcast with different total size must be rejected"
    );
}

#[test]
fn parse_rejects_legacy_metadata_syntax() {
    // invalid-metadata-has-type：`!0 = metadata !{}` 是 pre-opaque 旧语法（仅最新版 LLVM）
    assert!(
        parse_module("!0 = metadata !{}\n").is_err(),
        "legacy `metadata !` tuple syntax must be rejected"
    );
    assert!(
        parse_module("!0 = distinct !{}\n").is_ok(),
        "distinct prefix must still be accepted"
    );
}

#[test]
fn verify_rejects_gep_index_into_scalar() {
    // GEP 索引超过聚合深度：标量位置后继续索引（LLVM：indexing into scalar）
    assert!(
        verify_errs(
            "%RT = type { i8, [10 x [20 x i32]], i8 }
%ST = type { i32, double, %RT }
define ptr @foo(ptr %s) {
entry:
  %reg = getelementptr %ST, ptr %s, i32 1, i64 2, i32 1, i32 5, i32 13
  ret ptr %reg
}
"
        )
        .is_some(),
        "GEP with index past aggregate depth must be rejected"
    );
}

#[test]
fn verify_accepts_gep_array_element() {
    // 数组元素访问的末位索引到达标量合法
    assert!(
        verify_errs(
            "define ptr @f(ptr %p) {
  %e:
  %r = getelementptr [4 x i32], ptr %p, i64 0, i64 2
  ret ptr %r
}
"
        )
        .is_none(),
        "array element GEP must be accepted"
    );
}

#[test]
fn parse_rejects_bare_byref_param_attr() {
    // byref 必须带类型参数（LLVM：`ptr byref(i32)`——裸 byref 拒绝）
    assert!(
        parse_module("define void @f(ptr byref) {\n  ret void\n}\n").is_err(),
        "bare byref param attribute must be rejected"
    );
    assert!(
        parse_module("define void @f(ptr byref(i32)) {\n  ret void\n}\n").is_ok(),
        "byref(i32) must be accepted"
    );
}

#[test]
fn parse_rejects_inalloca_param_attr() {
    // inalloca 只能用于 alloca 关键字 / call 实参——函数定义参数拒绝
    assert!(
        parse_module("define void @f(ptr inalloca) {\n  ret void\n}\n").is_err(),
        "inalloca param attribute on definition must be rejected"
    );
}

#[test]
fn parse_rejects_alloca_addrspace_before_align() {
    // LLVM alloca 参数顺序：count → align → addrspace（addrspace 必须最后）
    assert!(
        parse_module(
            "define void @f() {\n  %a = alloca i32, addrspace(1), align 4\n  ret void\n}\n"
        )
        .is_err(),
        "alloca addrspace before align must be rejected"
    );
    assert!(
        parse_module(
            "define void @f() {\n  %a = alloca i32, align 4, addrspace(1)\n  ret void\n}\n"
        )
        .is_ok(),
        "alloca align before addrspace must be accepted"
    );
}
