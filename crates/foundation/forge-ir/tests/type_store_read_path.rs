//! 类型读路径纪律守卫（v3 S3："`TypeStore` 显式传参 / 读一次锁"）。
//!
//! 背景：`TypeContext::borrow()` 每次都是一次 `RwLock` 读锁获取，而 `RwLock`
//! **不可重入**——读锁存活期间再调 `borrow_mut()`（intern 新类型）会自锁死。
//! 所以多查询路径（display/verify/lowering）的正确形态是"**入口取一次锁，把
//! `&TypeStore` 显式传下去**"：既避免重复取锁，也让"这个作用域只读"落在类型上
//! （拿不到 `&TypeStore` 就写不出 intern 调用）。
//!
//! 改前实测：`display.rs` 有 51 处 `borrow()`（`value_as_literal` 每个值一次、
//! `NameResolver::new` 每个块/值各一次）——打印一个模块的取锁次数与函数体大小
//! 成正比。改后非测试路径只剩 2 处入口取锁。
//!
//! 本文件用 `TypeContext::debug_read_count`（仅 `debug_assertions`）把这条纪律
//! 钉成可测不变量：**打印一个模块 / 一个函数各只取 1 次读锁**；**校验一个函数
//! 的取锁次数不随 IR 规模增长**（改前 `verify.rs` 的 `check_conversion`/
//! `check_immediates`/`check_gep_indices` 在每条指令上各取一次锁）。
#![cfg(debug_assertions)]

use forge_ir::builder::FunctionBuilder;
use forge_ir::display::function_to_string;
use forge_ir::types::{FunctionSignature, TypeContext};
use forge_ir::verify::Verifier;
use forge_ir::{Function, Module, TypeId};

/// 一个内容较"重"的模块：参数化入口块 + 第二个块 + 整型/浮点/向量常量——
/// 覆盖 display 里会查类型的那些路径（值字面量、块参数、签名、函数头）。
fn rich_module() -> (Module, TypeContext) {
    let mut m = Module::new();
    let ctx = m.types.clone();
    let i32_ty = ctx.i32_ty();
    let v4f32 = ctx.borrow_mut().vector_ty(ctx.f32_ty(), 4);

    let sig = FunctionSignature::new(&[(i32_ty, "x")], &[i32_ty]);
    let mut fb = FunctionBuilder::new("rich", ctx.clone(), sig);
    let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "p")]);
    fb.switch_to_block(entry);
    let a = fb.iadd(params[0], params[0]);
    let f = fb.fconst_f32(1.5);
    let v = fb.vconst_bytes(vec![0u8; 16], v4f32);
    let cond = fb.iconst_bool(true);
    let body = fb.create_block();
    fb.branch(cond, body, &[], entry, &[]);
    fb.switch_to_block(body);
    let r = fb.iadd(a, a);
    fb.ret(&[r]);
    let func = fb.finish().expect("build");
    m.add_function(func);
    // 让浮点/向量常量都被引用上（display 会走它们的字面量路径）
    let _ = (f, v);
    (m, ctx)
}

/// 打印整个模块只取 1 次读锁（改前与函数体大小成正比）。
#[test]
fn module_display_takes_one_read_lock() {
    let (m, ctx) = rich_module();
    ctx.debug_reset_read_count();
    let text = format!("{m}");
    let reads = ctx.debug_read_count();
    assert!(
        text.contains("rich"),
        "模块文本应包含函数（实际输出：{text}）"
    );
    assert_eq!(
        reads,
        1,
        "打印模块必须只取 1 次类型读锁（实际 {reads} 次；文本 {} 字节）",
        text.len()
    );
}

/// 单函数 dump（`function_to_string`）同样只取 1 次。
#[test]
fn function_display_takes_one_read_lock() {
    let (m, ctx) = rich_module();
    let func = m.iter_functions().next().expect("一个函数");
    ctx.debug_reset_read_count();
    let text = function_to_string(func);
    let reads = ctx.debug_read_count();
    assert!(text.contains("define"), "单函数文本应含 define");
    assert_eq!(reads, 1, "function_to_string 必须只取 1 次类型读锁");
}

/// 重复打印：每次打印各取 1 次（不共享、不泄漏、不累积）。
#[test]
fn repeated_display_keeps_the_budget() {
    let (m, ctx) = rich_module();
    ctx.debug_reset_read_count();
    for _ in 0..3 {
        let _ = format!("{m}");
    }
    assert_eq!(
        ctx.debug_read_count(),
        3,
        "3 次打印 = 3 次取锁（每次打印独立取一次）"
    );
}

/// 单块函数：入口带一个参数，随后 `extra` 条 `iadd`，最后 `ret`。
fn func_with_insts(ctx: &TypeContext, extra: usize) -> Function {
    let i32_ty = ctx.i32_ty();
    let sig = FunctionSignature::new(&[(i32_ty, "x")], &[i32_ty]);
    let mut fb = FunctionBuilder::new("sized", ctx.clone(), sig);
    let (entry, params) = fb.create_block_with_params(&[(i32_ty, "p")]);
    fb.switch_to_block(entry);
    let mut acc = params[0];
    for _ in 0..extra {
        acc = fb.iadd(acc, params[0]);
    }
    fb.ret(&[acc]);
    fb.finish().expect("build")
}

/// 校验一个函数的取锁次数**不随指令数增长**（v3 S3 读路径纪律在 verify.rs 的落地）。
///
/// 改前：`check_conversion`/`check_immediates`/`check_gep_indices` 在每条指令上
/// 各 `borrow()` 一次 ⇒ 次数 ∝ 指令数（13 处 `borrow()`）。改后 `verify()` 入口
/// 取一次，`&TypeStore` 贯穿全部检查。
#[test]
fn verifier_read_locks_do_not_scale_with_ir_size() {
    let ctx = TypeContext::new();
    let reads_for = |extra: usize| -> usize {
        let func = func_with_insts(&ctx, extra);
        let mut verifier = Verifier::with_ctx(ctx.clone());
        ctx.debug_reset_read_count();
        let outcome = verifier.verify(&func);
        assert!(outcome.is_ok(), "{extra} 条指令的 IR 应合法：{outcome:?}");
        ctx.debug_read_count()
    };

    let small = reads_for(1);
    let large = reads_for(80);
    assert_eq!(
        small, large,
        "取锁次数不得随 IR 规模增长（1 条指令 {small} 次 vs 80 条 {large} 次）"
    );
    assert_eq!(
        small, 3,
        "校验一次函数的取锁次数应是个小常数（实测 {small} 次；其中 verify.rs 入口 1 次）"
    );
}

/// `TypeContext` 的**一次性查询封装**每次调用恰好 1 次读锁（不多、不少）。
///
/// 这些封装（`ctx.size_bytes(ty)` 这类）是多查询调用方的"顺手口"；多查询路径
/// 必须改用 `&TypeStore` 显式传参（见本文件其余用例）。这里钉住"每次调用 1 次"，
/// 防止某个封装内部退化成多次取锁（例如递归里每层取一次）。
#[test]
fn one_shot_type_context_accessors_take_one_lock() {
    let ctx = TypeContext::new();
    let i32_ty = ctx.i32_ty();
    let vec_ty = ctx.vector_ty(i32_ty, 4);
    let ptr_ty = ctx.ptr_ty();

    let one = |what: &str, f: &dyn Fn()| {
        ctx.debug_reset_read_count();
        f();
        assert_eq!(
            ctx.debug_read_count(),
            1,
            "TypeContext::{what} 应恰好取 1 次读锁"
        );
    };

    one("is_int", &|| {
        let _ = ctx.is_int(i32_ty);
    });
    one("is_float", &|| {
        let _ = ctx.is_float(i32_ty);
    });
    one("is_ptr", &|| {
        let _ = ctx.is_ptr(ptr_ty);
    });
    one("is_void", &|| {
        let _ = ctx.is_void(i32_ty);
    });
    one("is_vector", &|| {
        let _ = ctx.is_vector(vec_ty);
    });
    one("element_type", &|| {
        let _ = ctx.element_type(vec_ty);
    });
    one("scalar_bits", &|| {
        let _ = ctx.scalar_bits(i32_ty);
    });
    one("size_bytes", &|| {
        let _ = ctx.size_bytes(vec_ty);
    });
    one("alignment", &|| {
        let _ = ctx.alignment(vec_ty);
    });
    one("fmt_type", &|| {
        let _ = ctx.fmt_type(vec_ty);
    });
    one("get_signature", &|| {
        let _ = ctx.get_signature(sig_ref(&ctx, i32_ty));
    });

    // intern 路径只取写锁，不取读锁
    ctx.debug_reset_read_count();
    let _ = ctx.vector_ty(i32_ty, 8);
    let _ = ctx.int_ty(24);
    assert_eq!(
        ctx.debug_read_count(),
        0,
        "intern（写路径）不应取读锁（否则读写锁序会自锁死）"
    );
}

fn sig_ref(ctx: &TypeContext, ty: TypeId) -> forge_ir::SigRef {
    ctx.register_signature(FunctionSignature::new(&[(ty, "x")], &[ty]))
}

// ── 读写交错函数的"先写后读"结构（v3 S3 余项②）──

fn src_locals(n: usize) -> String {
    let mut s = String::from("define i32 @f(i32 %a) {\nentry:\n");
    let mut acc = "%a".to_string();
    for i in 0..n {
        s.push_str(&format!("  %v{i} = add i32 {acc}, 1\n"));
        acc = format!("%v{i}");
    }
    s.push_str(&format!("  ret i32 {acc}\n}}\n"));
    s
}

fn src_floatbits(n: usize) -> String {
    let mut s = String::from("define float @f(float %a) {\nentry:\n");
    let mut acc = "%a".to_string();
    for i in 0..n {
        s.push_str(&format!("  %v{i} = fadd float {acc}, 0x3F800000\n"));
        acc = format!("%v{i}");
    }
    s.push_str(&format!("  ret float {acc}\n}}\n"));
    s
}

fn src_vec(n: usize) -> String {
    let mut s = String::from("define void @f(ptr %p) {\nentry:\n");
    for i in 0..n {
        s.push_str(&format!(
            "  store <4 x i32> <i32 1, i32 2, i32 3, i32 4>, ptr %p\n    ; {i}\n"
        ));
    }
    s.push_str("  ret void\n}\n");
    s
}

fn snapshots(src: &str) -> usize {
    forge_ir::ir_parser::parse_module(src)
        .unwrap_or_else(|e| panic!("parse 失败：{e}"))
        .types
        .debug_read_count()
}

/// 同一**读段**只取一次快照：位模式/向量字面量操作数不再比局部值多取锁。
///
/// "先写后读"结构（v3 S3 余项②）：`operand_to_value` 在 `to_type`（可能 intern 新类型，
/// 写）之后取**一次**快照，把各 arm 需要的类型事实读成 `Copy` 局部量；快照不跨越会
/// intern 字符串的 arm（跨越会触发整表 COW 克隆）。
///
/// **A/B 实测**（同夹具同解析器，仅差这次结构改造）：
///
/// | 形态（32 条指令） | 改前 | 改后 |
/// | --- | --- | --- |
/// | 局部值（公共路径） | 129 | 129（无回归） |
/// | 浮点位模式 `fadd float … 0x3F800000` | 161 | **129**（每条 −1） |
/// | 向量字面量 `store <4 x i32> <…>` | 224 | **192**（每条 −1） |
#[test]
fn operand_reads_share_one_snapshot() {
    assert_eq!(
        snapshots(&src_locals(32)),
        129,
        "公共路径基线（改前同为 129）"
    );
    assert_eq!(
        snapshots(&src_floatbits(32)),
        129,
        "位模式操作数应与局部值取锁数相同（改前 161：多一次逐操作数取锁）"
    );
    assert_eq!(
        snapshots(&src_vec(32)),
        192,
        "向量字面量 = 基线 + 每指令 1 次（lane 编码 helper）；改前 224"
    );
}

// ── 锁 → 快照（v3 S3）：新语义的守卫 ──

/// 快照是**不可变视图**：拿快照后 intern 新类型，旧快照看不到、新快照看得到。
///
/// 这条同时是"读快照与写**可以并存**"的证据——改前（`RwLockReadGuard` 直接借用
/// `self.store`）在同一线程里"先 `borrow()` 再 `borrow_mut()`"会**自锁死**，
/// 本用例根本写不出来。
#[test]
fn snapshot_is_isolated_from_later_interning() {
    let ctx = TypeContext::new();
    ctx.debug_reset_cow_clone_count();
    let old = ctx.borrow();
    assert!(old.lookup_named("Later").is_none(), "初始没有该命名类型");

    // 快照 `old` 存活期间写入：写侧必须整表克隆（COW），次数被计数
    let anon = {
        let mut guard = ctx.borrow_mut();
        let a = guard.struct_anon(vec![], false);
        guard.define_named("Later", a);
        a
    };
    assert_eq!(
        ctx.debug_cow_clone_count(),
        1,
        "快照存活期间的写入必须触发**恰好一次**整表克隆"
    );
    assert!(
        old.lookup_named("Later").is_none(),
        "旧快照必须看不到写入后的类型（隔离）"
    );
    assert_eq!(
        ctx.borrow().lookup_named("Later"),
        Some(anon),
        "新快照必须看到写入后的类型"
    );
}

/// **常态零克隆**：真实工作负载（建 IR + intern + 打印 + 校验 + 解析文本）里
/// 不应有任何"拿着快照去 intern"的地方——否则每次写都要克隆整表（O(表大小)）。
///
/// 这是"读快照不跨 intern"纪律的可测量形式：改前那条纪律靠**死锁**（不可重入）
/// 强制，现在靠本用例（`debug_cow_clone_count == 0`）强制。
#[test]
fn write_path_never_clones_in_real_workload() {
    let (m, ctx) = rich_module();
    // 可校验的函数单独造（`rich_module` 只保证可打印）
    let func = func_with_insts(&ctx, 8);
    let mut verifier = Verifier::with_ctx(ctx.clone());
    ctx.debug_reset_cow_clone_count();
    ctx.debug_reset_read_count();

    // 1) 打印（入口取一次快照）+ 校验 + 再 intern 新类型（同一 ctx 上混合读写）
    let _ = format!("{m}");
    assert!(verifier.verify(&func).is_ok(), "IR 应合法");
    let _ = ctx.vector_ty(ctx.i32_ty(), 8);
    let _ = ctx.int_ty(24);
    // 2) 文本层（解析器内部大量 interning）
    let parsed = forge_ir::ir_parser::parse_module(
        "define i32 @f(i32 %a) {\nentry:\n  %x = add i32 %a, 1\n  ret i32 %x\n}\n",
    )
    .expect("parse");
    let _ = format!("{parsed}");

    assert_eq!(
        ctx.debug_cow_clone_count(),
        0,
        "正常路径不得出现「快照跨 intern」（每次都会克隆整表）"
    );
    assert!(
        ctx.debug_read_count() > 0,
        "本用例应确实走过读快照路径（实测 0 次 ⇒ 用例没测到东西）"
    );
}
