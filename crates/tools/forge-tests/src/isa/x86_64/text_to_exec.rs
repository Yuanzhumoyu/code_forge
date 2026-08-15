//! 4.4 端到端执行对照：`text → parse → compile_raw → run` 全链验证。
//! 文本层 parse 我们支持的 LLVM IR 文本，编译并 JIT 执行，断言数值结果
//!（与 clang -O0 语义一致：整数/浮点/分支/调用）。

#![cfg(test)]

use code_forge::backend::FunctionCompiler;
use code_forge::backend::arch::x86_64::{TargetMachine, ensure_registered};
use code_forge::ir::ir_parser::parse_module;
use code_forge::mem::ExecutableMemory;

/// parse 文本模块 → 编译首个函数 → JIT 执行 → 返回 i32。
fn exec_i32(src: &str) -> i32 {
    ensure_registered();
    let module = parse_module(src).expect("parse");
    let func = module.iter_functions().next().unwrap().clone();
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .expect("compile");
    assert!(!compiled.code.is_empty(), "empty code");
    let mem = ExecutableMemory::new(&compiled.code).expect("alloc");
    let f: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
    f()
}

/// 模块级执行（含 global 数据段）：JitCompiler::compile_module + get_fn。
/// 单函数 compile_raw 的 GlobalAddr 是 `lea_rip rd, global`（reloc 指向模块
/// 数据段符号 "G{id}"）——没有模块级数据段的裸执行会写地址 0 → SEGV。
fn exec_module_i32(src: &str) -> i32 {
    ensure_registered();
    let module = parse_module(src).expect("parse");
    let mut jit = code_forge::backend::jit::JitCompiler::new(TargetMachine::new());
    jit.compile_module(&module).expect("compile module");
    let f: extern "C" fn() -> i32 = jit.get_fn("main").expect("get main");
    f()
}

#[test]
fn text_parse_compile_run_arith() {
    // 整数算术（对照 clang：20 + 22 == 42）
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %a = add i32 20, i32 22
    ret i32 %a
}"
        ),
        42
    );
}

#[test]
fn text_parse_compile_run_branch() {
    // 分支 + phi 回填（对照 clang：x > 10 ? x*2 : x）
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %c = icmp sgt i32 15, i32 10
    br i1 %c, label %then, label %else
  %then:
    ret i32 30
  %else:
    ret i32 5
}"
        ),
        30
    );
}

#[test]
fn text_parse_compile_run_call() {
    // 函数调用链
    let src = "define i32 @add2(i32 %a, i32 %b) {
  %entry:
    %r = add i32 %a, i32 %b
    ret i32 %r
}
define i32 @main() {
  %entry:
    %v = call i32 @add2(i32 40, i32 2)
    ret i32 %v
}";
    let module = parse_module(src).expect("parse");
    // 编译 main（含 call 到 add2——需两个函数都编译进同一内存？简化：单函数编译
    // 要求 main 不依赖外部——用 self-contained main）
    let func = module.iter_functions().nth(1).unwrap().clone();
    ensure_registered();
    let compiled = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .expect("compile main");
    let mem = ExecutableMemory::new(&compiled.code).expect("alloc");
    let f: extern "C" fn() -> i32 = unsafe { mem.get_fn(0).unwrap() };
    let _ = f();
    // call 需要被调函数符号解析——JIT 内存无 add2 → 仅验证编译成功不 panic
}

#[test]
fn text_parse_compile_agg_store_ret() {
    // 3.2 聚合常量 store/ret（AggConst 值表示）：编译成功验证。
    // 注：聚合 store 的 JIT 执行在 x86_64 codegen 有机器码 bug（SEGV——store
    // 8 字节聚合的降级路径），执行对照留 codegen 修复后（已知差距）。
    for src in [
        r#"
@g = global {i32, i32} zeroinitializer
define i32 @main() {
  %entry:
    store {i32, i32} {i32 7, i32 9}, ptr @g
    %a = load i32, ptr @g
    ret i32 %a
}
"#,
        "define { i32 } @f() {\n  %entry:\n    ret {i32} { i32 42 }\n}\n",
        "define void @f(ptr %x) {\n  %entry:\n    store {} zeroinitializer, ptr %x\n    ret void\n}\n",
    ] {
        ensure_registered();
        let module = parse_module(src).expect("parse");
        let func = module.iter_functions().next().unwrap().clone();
        let compiled = FunctionCompiler::new(TargetMachine::new())
            .compile_raw(&func)
            .expect("compile");
        assert!(!compiled.code.is_empty());
    }
}

#[test]
fn text_parse_compile_eh_unsupported() {
    // 异常处理（invoke/landingpad/resume）：P1.1 仅文本层——codegen 报 Unsupported
    let src = r#"
declare void @__gxx_personality_v0()
declare i32 @may_throw(i32)
define i32 @main() personality ptr @__gxx_personality_v0 {
  %entry:
    invoke i32 @may_throw(i32 1) to label %ok unwind label %pad
  %ok:
    ret i32 0
  %pad:
    %l = landingpad { ptr, i32 } cleanup
    resume { ptr, i32 } %l
}
"#;
    ensure_registered();
    let module = parse_module(src).expect("parse");
    let func = module
        .iter_functions()
        .find(|f| f.name.as_str() == "main")
        .unwrap()
        .clone();
    let err = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&func)
        .expect_err("exception function must be Unsupported at codegen");
    let msg = format!("{err}");
    assert!(
        msg.contains("Unsupported"),
        "expected Unsupported error, got: {msg}"
    );
}

#[test]
fn text_parse_compile_run_agg_store() {
    // 聚合常量 store 执行（修复前：聚合类型 bits()=0 → store 宽度 0 机器码
    // 缺陷 → SEGV；修复：compile 期打包展开 + mem_opsize_for 真实宽度）。
    // 对照 clang：struct S { int a, b; } s = {7, 9}; return s.a; == 7
    assert_eq!(
        exec_module_i32(
            "@g = global {i32, i32} zeroinitializer
define i32 @main() {
  %entry:
    store {i32, i32} {i32 7, i32 9}, ptr @g
    %x = load i32, ptr @g
    ret i32 %x
}"
        ),
        7
    );
}

#[test]
fn text_parse_compile_run_plain_store_i64() {
    // 基础路径对照：非聚合 store i64 到 global
    assert_eq!(
        exec_module_i32(
            "@g = global i64 zeroinitializer
define i32 @main() {
  %entry:
    store i64 12345, ptr @g
    %x = load i32, ptr @g
    ret i32 %x
}"
        ),
        12345
    );
}

#[test]
fn text_parse_compile_run_agg_store_i64() {
    // 8 字节聚合（i64 单字段 + i32 对）：打包 64 位 store
    assert_eq!(
        exec_module_i32(
            "@g = global {i32, i32} zeroinitializer
define i32 @main() {
  %entry:
    store {i32, i32} {i32 42, i32 1}, ptr @g
    %x = load i32, ptr @g
    ret i32 %x
}"
        ),
        42
    );
}

#[test]
fn text_parse_compile_run_agg_store_i8_fields() {
    // 窄字段聚合 {i8, i8, i8, i8}：打包低 32 位
    assert_eq!(
        exec_module_i32(
            "@g = global {i8, i8, i8, i8} zeroinitializer
define i32 @main() {
  %entry:
    store {i8, i8, i8, i8} {i8 1, i8 2, i8 3, i8 4}, ptr @g
    %x = load i32, ptr @g
    ret i32 %x
}"
        ),
        // 小端：0x04030201
        0x04030201
    );
}

#[test]
fn text_parse_compile_run_nested_extractvalue_param() {
    // S1：嵌套聚合字段提取——参数路径（{{i32,i32},i32} 12 字节 >8 → 拆段 +
    // 内存化：外层提取登记槽地址，内层 GEP+load）
    assert_eq!(
        exec_module_i32(
            "define i32 @f({{i32, i32}, i32} %a) {
  %entry:
    %inner = extractvalue {{i32, i32}, i32} %a, 0
    %x = extractvalue {i32, i32} %inner, 1
    ret i32 %x
}
define i32 @main() {
  %entry:
    %r = call i32 @f({{i32, i32}, i32} {{i32 7, i32 9}, i32 11})
    ret i32 %r
}"
        ),
        9
    );
}

#[test]
fn text_parse_compile_run_nested_extractvalue_load() {
    // S1：嵌套聚合字段提取——load 路径（load 源地址链式登记）
    assert_eq!(
        exec_module_i32(
            "@g = global {{i32, i32}, i32} {{i32 7, i32 9}, i32 11}
define i32 @main() {
  %entry:
    %p = load {{i32, i32}, i32}, ptr @g
    %inner = extractvalue {{i32, i32}, i32} %p, 0
    %a = extractvalue {i32, i32} %inner, 1
    ret i32 %a
}"
        ),
        9
    );
}

#[test]
fn text_parse_compile_run_extractvalue() {
    // extractvalue 字段 0/1（聚合 load → GPR 移位提取）
    assert_eq!(
        exec_module_i32(
            "@g = global {i32, i32} {i32 7, i32 9}
define i32 @main() {
  %entry:
    %p = load {i32, i32}, ptr @g
    %a = extractvalue {i32, i32} %p, 1
    ret i32 %a
}"
        ),
        9
    );
}

#[test]
fn text_parse_compile_run_extractvalue_field0() {
    assert_eq!(
        exec_module_i32(
            "@g = global {i32, i32} {i32 7, i32 9}
define i32 @main() {
  %entry:
    %p = load {i32, i32}, ptr @g
    %a = extractvalue {i32, i32} %p, 0
    ret i32 %a
}"
        ),
        7
    );
}

#[test]
fn text_parse_compile_run_insertvalue() {
    // insertvalue 字段写回 → store 回 global → 读字段验证
    assert_eq!(
        exec_module_i32(
            "@g = global {i32, i32} {i32 7, i32 9}
define i32 @main() {
  %entry:
    %p = load {i32, i32}, ptr @g
    %q = insertvalue {i32, i32} %p, i32 42, 1
    store {i32, i32} %q, ptr @g
    %x = load i32, ptr @g
    ret i32 %x
}"
        ),
        7 // 字段 0 保持 7（insertvalue 只改字段 1）
    );
}

#[test]
fn text_parse_compile_run_insertvalue_then_load_field1() {
    // insertvalue 后字段 1 = 42（通过提取验证）
    assert_eq!(
        exec_module_i32(
            "@g = global {i32, i32} {i32 7, i32 9}
define i32 @main() {
  %entry:
    %p = load {i32, i32}, ptr @g
    %q = insertvalue {i32, i32} %p, i32 42, 1
    %a = extractvalue {i32, i32} %q, 1
    ret i32 %a
}"
        ),
        42
    );
}

#[test]
fn text_parse_compile_run_extractvalue_array() {
    // 数组元素提取（[2 x i32] 聚合 load → GPR 移位）
    assert_eq!(
        exec_module_i32(
            "@g = global [2 x i32] [i32 5, i32 6]
define i32 @main() {
  %entry:
    %p = load [2 x i32], ptr @g
    %a = extractvalue [2 x i32] %p, 1
    ret i32 %a
}"
        ),
        6
    );
}

#[test]
fn text_parse_compile_run_extractvalue_array_field0() {
    assert_eq!(
        exec_module_i32(
            "@g = global [2 x i32] [i32 5, i32 6]
define i32 @main() {
  %entry:
    %p = load [2 x i32], ptr @g
    %a = extractvalue [2 x i32] %p, 0
    ret i32 %a
}"
        ),
        5
    );
}

#[test]
fn text_parse_compile_run_ret_single_elem_agg() {
    // 单元素聚合返回（aggregate-return-single-value：{i32} 按 i32 ABI 返回
    // 在 RAX——低 32 位即字段 0）
    assert_eq!(
        exec_module_i32(
            "@g = global {i32} {i32 42}
define {i32} @main() {
  %entry:
    %p = load {i32}, ptr @g
    ret {i32} %p
}"
        ),
        42
    );
}

#[test]
fn text_parse_compile_run_ret_two_elem_agg_field0() {
    // 两元素聚合返回：RAX 低 32 位 = 字段 0（ABI 上 8 字节 struct 按整数返回）
    assert_eq!(
        exec_module_i32(
            "@g = global {i32, i32} {i32 7, i32 9}
define {i32, i32} @main() {
  %entry:
    %p = load {i32, i32}, ptr @g
    ret {i32, i32} %p
}"
        ),
        7
    );
}

#[test]
fn text_parse_compile_run_agg_param() {
    // 聚合参数传递（SysV：{i32,i32} 8 字节 → 整数寄存器）+ extractvalue 拆字段
    assert_eq!(
        exec_module_i32(
            "define i32 @sum({i32, i32} %p) {
  %entry:
    %a = extractvalue {i32, i32} %p, 0
    %b = extractvalue {i32, i32} %p, 1
    %r = add i32 %a, i32 %b
    ret i32 %r
}
define i32 @main() {
  %entry:
    %r = call i32 @sum({i32, i32} {i32 3, i32 4})
    ret i32 %r
}"
        ),
        7
    );
}

#[test]
fn text_parse_compile_run_agg_ret_param_chain() {
    // 完整链路：聚合返回 → 聚合值传参 → extractvalue 拆字段（对照 clang）
    assert_eq!(
        exec_module_i32(
            "@g = global {i32, i32} {i32 3, i32 4}
define {i32, i32} @make() {
  %entry:
    %p = load {i32, i32}, ptr @g
    ret {i32, i32} %p
}
define i32 @sum({i32, i32} %p) {
  %entry:
    %a = extractvalue {i32, i32} %p, 0
    %b = extractvalue {i32, i32} %p, 1
    %r = add i32 %a, i32 %b
    ret i32 %r
}
define i32 @main() {
  %entry:
    %q = call {i32, i32} @make()
    %r = call i32 @sum({i32, i32} %q)
    ret i32 %r
}"
        ),
        7
    );
}

#[test]
fn text_parse_compile_run_alloca_agg() {
    // alloca 聚合（修复前：Alloca 无操作数 lowering 用垃圾寄存器 → SEGV；
    // 修复：预扫描分配帧槽 + lea_off [rbp+disp]）
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %p = alloca {i32, i32}
    store {i32, i32} {i32 7, i32 9}, ptr %p
    %x = load i32, ptr %p
    ret i32 %x
}"
        ),
        7
    );
}

#[test]
fn text_parse_compile_run_alloca_multi() {
    // 多 alloca 共存（独立槽不重叠）
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %a = alloca i32
    %b = alloca i64
    store i32 5, ptr %a
    store i64 6, ptr %b
    %x = load i32, ptr %a
    ret i32 %x
}"
        ),
        5
    );
}

#[test]
fn text_parse_compile_run_alloca_arr_slot() {
    // alloca 数组槽（16 字节）——store 到槽内元素地址（常量偏移验证槽大小）
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %p = alloca [2 x i64]
    %p0 = getelementptr [2 x i64], ptr %p, i32 0, i32 1
    store i64 12345, ptr %p0
    %x = load i64, ptr %p0
    %r = trunc i64 %x to i32
    ret i32 %r
}"
        ),
        12345
    );
}

#[test]
fn text_parse_compile_run_gep_array_const_idx() {
    // GEP 常量索引：全局数组 @arr[2]（对照 clang：arr[2] == 30）
    assert_eq!(
        exec_module_i32(
            "@arr = global [4 x i32] [i32 10, i32 20, i32 30, i32 40]
define i32 @main() {
  %entry:
    %p = getelementptr [4 x i32], ptr @arr, i32 0, i32 2
    %v = load i32, ptr %p
    ret i32 %v
}"
        ),
        30
    );
}

#[test]
fn text_parse_compile_run_gep_array_dyn_idx() {
    // GEP 动态索引（数组下标变量）：arr[3] == 40
    assert_eq!(
        exec_module_i32(
            "@arr = global [4 x i32] [i32 10, i32 20, i32 30, i32 40]
define i32 @main() {
  %entry:
    %i = add i32 2, i32 1
    %p = getelementptr [4 x i32], ptr @arr, i32 0, i32 %i
    %v = load i32, ptr %p
    ret i32 %v
}"
        ),
        40
    );
}

#[test]
fn text_parse_compile_run_gep_struct_field() {
    // GEP struct 字段（field_offset 折叠）：@s.f1 == 200
    assert_eq!(
        exec_module_i32(
            "@s = global {i32, i32, i32} {i32 1, i32 200, i32 3}
define i32 @main() {
  %entry:
    %p = getelementptr {i32, i32, i32}, ptr @s, i32 0, i32 1
    %v = load i32, ptr %p
    ret i32 %v
}"
        ),
        200
    );
}

#[test]
fn text_parse_compile_run_gep_array_store() {
    // GEP 元素 store：arr[1] = 99 后读回
    assert_eq!(
        exec_module_i32(
            "@arr = global [4 x i32] [i32 10, i32 20, i32 30, i32 40]
define i32 @main() {
  %entry:
    %p = getelementptr [4 x i32], ptr @arr, i32 0, i32 1
    store i32 99, ptr %p
    %v = load i32, ptr %p
    ret i32 %v
}"
        ),
        99
    );
}

#[test]
fn text_parse_compile_run_local_arr_loop_sum() {
    // 局部数组 + 动态索引 GEP 循环求和（对照 clang：1+2+3+4 == 10）
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %p = alloca [4 x i32]
    store i32 1, ptr %p
    %p1 = getelementptr [4 x i32], ptr %p, i32 0, i32 1
    store i32 2, ptr %p1
    %p2 = getelementptr [4 x i32], ptr %p, i32 0, i32 2
    store i32 3, ptr %p2
    %p3 = getelementptr [4 x i32], ptr %p, i32 0, i32 3
    store i32 4, ptr %p3
    br label %loop
  %loop:
    %i = phi i32 [0, %entry], [%i2, %loop]
    %acc = phi i32 [0, %entry], [%acc2, %loop]
    %gep = getelementptr [4 x i32], ptr %p, i32 0, i32 %i
    %v = load i32, ptr %gep
    %acc2 = add i32 %acc, i32 %v
    %i2 = add i32 %i, i32 1
    %cmp = icmp slt i32 %i2, i32 4
    br i1 %cmp, label %loop, label %done
  %done:
    ret i32 %acc2
}"
        ),
        10
    );
}

#[test]
fn text_parse_compile_run_gep_nested_array() {
    // 嵌套数组两级 GEP：m[1][0] == 3（两级展开链：第一索引 + 数组×2）
    assert_eq!(
        exec_module_i32(
            "@m = global [2 x [2 x i32]] [[i32 1, i32 2], [i32 3, i32 4]]
define i32 @main() {
  %entry:
    %p = getelementptr [2 x [2 x i32]], ptr @m, i32 0, i32 1, i32 0
    %v = load i32, ptr %p
    ret i32 %v
}"
        ),
        3
    );
}

#[test]
fn text_parse_compile_run_gep_struct_arr_loop() {
    // 结构体数组循环累加 pts[i].0（对照 clang：1+3+5 == 9）——
    // GEP 混合链：第一索引 0（常量）→ 数组动态索引 %i（mul 8）→ struct 字段 0
    assert_eq!(
        exec_module_i32(
            "@pts = global [3 x {i32, i32}] [{i32 1, i32 2}, {i32 3, i32 4}, {i32 5, i32 6}]
define i32 @main() {
  %entry:
    br label %loop
  %loop:
    %i = phi i32 [0, %entry], [%i2, %loop]
    %acc = phi i32 [0, %entry], [%acc2, %loop]
    %gep = getelementptr [3 x {i32, i32}], ptr @pts, i32 0, i32 %i, i32 0
    %v = load i32, ptr %gep
    %acc2 = add i32 %acc, i32 %v
    %i2 = add i32 %i, i32 1
    %cmp = icmp slt i32 %i2, i32 3
    br i1 %cmp, label %loop, label %done
  %done:
    ret i32 %acc2
}"
        ),
        9
    );
}

#[test]
fn text_parse_compile_run_large_agg_store_i64x2() {
    // 大聚合常量 store（{i64,i64} 16 字节 → 逐段 store i64 ×2）：
    // 字段 0 = 7（段 0）、字段 1 = 9（段 1，GEP 偏移 8）
    assert_eq!(
        exec_module_i32(
            "@g = global {i64, i64} zeroinitializer
define i32 @main() {
  %entry:
    store {i64, i64} {i64 7, i64 9}, ptr @g
    %p1 = getelementptr {i64, i64}, ptr @g, i32 0, i32 1
    %v = load i64, ptr %p1
    %r = trunc i64 %v to i32
    ret i32 %r
}"
        ),
        9
    );
}

#[test]
fn text_parse_compile_run_large_agg_store_arr4() {
    // [4 x i32] 16 字节 store → 元素 3 = 40（GEP 偏移 12）
    assert_eq!(
        exec_module_i32(
            "@g = global [4 x i32] zeroinitializer
define i32 @main() {
  %entry:
    store [4 x i32] [i32 10, i32 20, i32 30, i32 40], ptr @g
    %p = getelementptr [4 x i32], ptr @g, i32 0, i32 3
    %v = load i32, ptr %p
    ret i32 %v
}"
        ),
        40
    );
}

#[test]
fn text_parse_compile_run_large_agg_store_4xi32() {
    // {i32,i32,i32,i32} 16 字节 store → 字段 0/3 读回（首尾段）
    assert_eq!(
        exec_module_i32(
            "@g = global {i32, i32, i32, i32} zeroinitializer
define i32 @main() {
  %entry:
    store {i32, i32, i32, i32} {i32 1, i32 2, i32 3, i32 4}, ptr @g
    %p = getelementptr {i32, i32, i32, i32}, ptr @g, i32 0, i32 3
    %v = load i32, ptr %p
    ret i32 %v
}"
        ),
        4
    );
}

#[test]
fn text_parse_compile_run_large_agg_store_odd_size() {
    // 12 字节聚合（{i64, i32}）：段 0 8 字节 + 段 1 4 字节（窄段不越界）
    assert_eq!(
        exec_module_i32(
            "@g = global {i64, i32} zeroinitializer
define i32 @main() {
  %entry:
    store {i64, i32} {i64 111, i32 222}, ptr @g
    %p = getelementptr {i64, i32}, ptr @g, i32 0, i32 1
    %v = load i32, ptr %p
    ret i32 %v
}"
        ),
        222
    );
}

#[test]
fn text_parse_compile_run_large_agg_copy() {
    // 大聚合 load → store 拷贝（@g → @h 逐段复制）→ 读 @h 字段 1
    assert_eq!(
        exec_module_i32(
            "@g = global {i64, i64} {i64 7, i64 9}
@h = global {i64, i64} zeroinitializer
define i32 @main() {
  %entry:
    %v = load {i64, i64}, ptr @g
    store {i64, i64} %v, ptr @h
    %p = getelementptr {i64, i64}, ptr @h, i32 0, i32 1
    %x = load i64, ptr %p
    %r = trunc i64 %x to i32
    ret i32 %r
}"
        ),
        9
    );
}

#[test]
fn text_parse_compile_run_large_agg_extractvalue() {
    // 大聚合 load → extractvalue 字段 1（值传播：GEP 偏移 + 标量 load）
    assert_eq!(
        exec_module_i32(
            "@g = global {i64, i64} {i64 7, i64 9}
define i32 @main() {
  %entry:
    %v = load {i64, i64}, ptr @g
    %a = extractvalue {i64, i64} %v, 1
    %r = trunc i64 %a to i32
    ret i32 %r
}"
        ),
        9
    );
}

#[test]
fn text_parse_compile_run_large_agg_extractvalue_arr() {
    // [4 x i64] 元素提取（24 字节？不——[4 x i64] 32 字节）——元素 2 = 30
    assert_eq!(
        exec_module_i32(
            "@g = global [4 x i64] [i64 10, i64 20, i64 30, i64 40]
define i32 @main() {
  %entry:
    %v = load [4 x i64], ptr @g
    %a = extractvalue [4 x i64] %v, 2
    %r = trunc i64 %a to i32
    ret i32 %r
}"
        ),
        30
    );
}

#[test]
fn text_parse_compile_run_large_agg_arr_elem_store() {
    // [2 x {i64,i64}] 数组元素大聚合 store（GEP 地址 + 分段）→ 读元素字段
    assert_eq!(
        exec_module_i32(
            "@arr = global [2 x {i64, i64}] zeroinitializer
define i32 @main() {
  %entry:
    %p = getelementptr [2 x {i64, i64}], ptr @arr, i32 0, i32 1
    store {i64, i64} {i64 5, i64 6}, ptr %p
    %p2 = getelementptr [2 x {i64, i64}], ptr @arr, i32 0, i32 1, i32 1
    %v = load i64, ptr %p2
    %r = trunc i64 %v to i32
    ret i32 %r
}"
        ),
        6
    );
}

#[test]
fn text_parse_compile_run_large_agg_arr_elem_load_copy() {
    // 数组元素大聚合 load → 拷贝到另一元素
    assert_eq!(
        exec_module_i32(
            "@arr = global [2 x {i64, i64}] [{i64 1, i64 2}, {i64 3, i64 4}]
define i32 @main() {
  %entry:
    %p1 = getelementptr [2 x {i64, i64}], ptr @arr, i32 0, i32 0
    %v = load {i64, i64}, ptr %p1
    %p2 = getelementptr [2 x {i64, i64}], ptr @arr, i32 0, i32 1
    store {i64, i64} %v, ptr %p2
    %p3 = getelementptr [2 x {i64, i64}], ptr @arr, i32 0, i32 1, i32 0
    %x = load i64, ptr %p3
    %r = trunc i64 %x to i32
    ret i32 %r
}"
        ),
        1
    );
}

#[test]
fn text_parse_compile_run_large_agg_field_loop() {
    // 大聚合数组字段累加循环（对照 clang：pts[0].1 + pts[1].1 = 2+4 = 6）
    assert_eq!(
        exec_module_i32(
            "@pts = global [2 x {i64, i64}] [{i64 1, i64 2}, {i64 3, i64 4}]
define i32 @main() {
  %entry:
    br label %loop
  %loop:
    %i = phi i32 [0, %entry], [%i2, %loop]
    %acc = phi i32 [0, %entry], [%acc2, %loop]
    %gep = getelementptr [2 x {i64, i64}], ptr @pts, i32 0, i32 %i, i32 1
    %v = load i64, ptr %gep
    %t = trunc i64 %v to i32
    %acc2 = add i32 %acc, i32 %t
    %i2 = add i32 %i, i32 1
    %cmp = icmp slt i32 %i2, i32 2
    br i1 %cmp, label %loop, label %done
  %done:
    ret i32 %acc2
}"
        ),
        6
    );
}

#[test]
fn text_parse_compile_run_large_agg_init_copy_loop() {
    // 全链路：大聚合常量初始化 → load → 拷贝到 alloca 槽 → 字段累加
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %dst = alloca [2 x {i64, i64}]
    store [2 x {i64, i64}] [{i64 1, i64 2}, {i64 3, i64 4}], ptr %dst
    br label %loop
  %loop:
    %i = phi i32 [0, %entry], [%i2, %loop]
    %acc = phi i32 [0, %entry], [%acc2, %loop]
    %gep = getelementptr [2 x {i64, i64}], ptr %dst, i32 0, i32 %i, i32 1
    %v = load i64, ptr %gep
    %t = trunc i64 %v to i32
    %acc2 = add i32 %acc, i32 %t
    %i2 = add i32 %i, i32 1
    %cmp = icmp slt i32 %i2, i32 2
    br i1 %cmp, label %loop, label %done
  %done:
    ret i32 %acc2
}"
        ),
        6
    );
}

#[test]
fn text_parse_compile_run_large_agg_param_abi() {
    // 大聚合 ABI 传参：调用方拆段（load 结果）+ 被调方签名拆开收参 +
    // extractvalue 段值提取（对照 clang：3+4=7）
    assert_eq!(
        exec_module_i32(
            "@g = global {i64, i64} {i64 3, i64 4}
define i32 @sum({i64, i64} %p) {
  %entry:
    %a = extractvalue {i64, i64} %p, 0
    %b = extractvalue {i64, i64} %p, 1
    %r = add i64 %a, i64 %b
    %t = trunc i64 %r to i32
    ret i32 %t
}
define i32 @main() {
  %entry:
    %v = load {i64, i64}, ptr @g
    %r = call i32 @sum({i64, i64} %v)
    ret i32 %r
}"
        ),
        7
    );
}

#[test]
fn text_parse_compile_run_large_agg_param_abi_const() {
    // 聚合常量参数拆段（AggConst 段来源）
    assert_eq!(
        exec_module_i32(
            "define i32 @sum({i64, i64} %p) {
  %entry:
    %a = extractvalue {i64, i64} %p, 0
    %b = extractvalue {i64, i64} %p, 1
    %r = add i64 %a, i64 %b
    %t = trunc i64 %r to i32
    ret i32 %t
}
define i32 @main() {
  %entry:
    %r = call i32 @sum({i64, i64} {i64 10, i64 32})
    ret i32 %r
}"
        ),
        42
    );
}

#[test]
fn text_parse_compile_run_large_agg_ret_abi() {
    // 大聚合返回：@make 返回 {i64,i64}（RAX/RDX 双槽）→ 调用方结果拆 2 →
    // extractvalue 字段 1（对照 clang：6）
    assert_eq!(
        exec_module_i32(
            "@g = global {i64, i64} {i64 5, i64 6}
define {i64, i64} @make() {
  %entry:
    %v = load {i64, i64}, ptr @g
    ret {i64, i64} %v
}
define i32 @main() {
  %entry:
    %q = call {i64, i64} @make()
    %a = extractvalue {i64, i64} %q, 1
    %r = trunc i64 %a to i32
    ret i32 %r
}"
        ),
        6
    );
}

#[test]
fn text_parse_compile_run_large_agg_ret_chain() {
    // 完整链路：make 返回大聚合 → 传参给 sum → 字段累加（对照 clang：5+6=11）
    assert_eq!(
        exec_module_i32(
            "@g = global {i64, i64} {i64 5, i64 6}
define {i64, i64} @make() {
  %entry:
    %v = load {i64, i64}, ptr @g
    ret {i64, i64} %v
}
define i32 @sum({i64, i64} %p) {
  %entry:
    %a = extractvalue {i64, i64} %p, 0
    %b = extractvalue {i64, i64} %p, 1
    %r = add i64 %a, i64 %b
    %t = trunc i64 %r to i32
    ret i32 %t
}
define i32 @main() {
  %entry:
    %q = call {i64, i64} @make()
    %r = call i32 @sum({i64, i64} %q)
    ret i32 %r
}"
        ),
        11
    );
}

#[test]
fn text_parse_compile_run_float_field_abi() {
    // 浮点字段 ABI：{f64,f64} 参数拆段 + extractvalue 浮点字段 movq_to_xmm
    // 位模式转换 + fadd 消费（对照 clang：1.5+2.5=4.0）
    assert_eq!(
        exec_module_i32(
            "define double @g({f64, f64} %p) {
  %entry:
    %a = extractvalue {f64, f64} %p, 0
    %b = extractvalue {f64, f64} %p, 1
    %r = fadd double %a, double %b
    ret double %r
}
define i32 @main() {
  %entry:
    %r = call double @g({f64, f64} {f64 1.5, f64 2.5})
    %c = fcmp oeq double %r, double 4.0
    %t = select i1 %c, i32 4, i32 0
    ret i32 %t
}"
        ),
        4
    );
}

#[test]
fn text_parse_compile_run_float_field_f32_abi() {
    // {f32,f32,f32,f32} 字段 1（段内偏移 4——shr 32 后 movq_to_xmm）
    assert_eq!(
        exec_module_i32(
            "define float @g({f32, f32, f32, f32} %p) {
  %entry:
    %b = extractvalue {f32, f32, f32, f32} %p, 1
    %d = extractvalue {f32, f32, f32, f32} %p, 3
    %r = fadd float %b, float %d
    ret float %r
}
define i32 @main() {
  %entry:
    %r = call float @g({f32, f32, f32, f32} {f32 1.0, f32 2.5, f32 0.0, f32 1.5})
    %b = bitcast float %r to i32
    ; 4.0f 位模式 0x40800000（fcmp float 需 comiss——既有限制，见附录）
    %c = icmp eq i32 %b, i32 1082130432
    %t = select i1 %c, i32 4, i32 0
    ret i32 %t
}"
        ),
        4
    );
}

#[test]
fn text_parse_compile_run_fcmp_f32_direct() {
    // fcmp float 直接比较（comiss——F32 变体分派）：1.5 < 2.5 → 1
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %c = fcmp olt float 1.5, 2.5
    %t = select i1 %c, i32 7, i32 0
    ret i32 %t
}"
        ),
        7
    );
    // F32 结果 bitcast+zext 路径（无 select）
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %c = fcmp olt float 1.5, 2.5
    %b = bitcast i1 %c to i8
    %z = zext i8 %b to i32
    ret i32 %z
}"
        ),
        1
    );
    // F64 仍走 comisd（回归）
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %c = fcmp olt double 2.5, 1.5
    %t = select i1 %c, i32 7, i32 0
    ret i32 %t
}"
        ),
        0
    );
}

#[test]
fn text_parse_compile_run_f32_unary() {
    // fsqrt float（sqrtss）：sqrt(2.25f) = 1.5f → 位模式 0x3FC00000
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %r = fsqrt float 2.25
    %b = bitcast float %r to i32
    ret i32 %b
}"
        ),
        0x3FC00000
    );
    // fabs float（andps 掩码）：|-2.5f| = 2.5f → 位模式 0x40200000
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %r = fabs float -2.5
    %b = bitcast float %r to i32
    ret i32 %b
}"
        ),
        0x40200000
    );
    // fneg float（xorps 掩码）：-1.5f → 位模式 0xBFC00000
    assert_eq!(
        exec_i32(
            "define i32 @main() {
  %entry:
    %r = fneg float 1.5
    %b = bitcast float %r to i32
    ret i32 %b
}"
        ),
        0xBFC00000u32 as i32
    );
}
