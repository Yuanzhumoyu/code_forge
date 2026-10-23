//! 从 `compiler.rs` 拆出（v25 M4：逐字搬移，不改行为）。

use super::*;
use crate::LowerCtx;
use crate::arch::x86;
// 落点探针与它的类别枚举住在 `legalize::common`（v23 M6 批次 4 从本文件迁出）——
// 只有这些集成用例还从"编译侧"看它们，所以 import 留在测试模块里。

/// 生成期 AVX-512 可行性开关的作用域守卫（Drop 时清除，**panic 安全**）。
/// 必须与 `crate::AVX512_ENV_LOCK` 配对使用——env 只放开"能否发射 EVEX"
/// 的生成期门；**执行** EVEX 的测试用 `avx512_hardware_available()` 判 skip
/// （env 泄漏到运行时测试会真的执行 EVEX → STATUS_ILLEGAL_INSTRUCTION，
/// 2026-09-12 实测 0xC000001D）。
struct AssumeAvx512;

impl AssumeAvx512 {
    fn set() -> Self {
        unsafe { std::env::set_var("FORGE_ASSUME_AVX512", "1") };
        Self
    }
}

impl Drop for AssumeAvx512 {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("FORGE_ASSUME_AVX512") };
    }
}

fn build_sig(params: &[TypeId], ret: TypeId) -> FunctionSignature {
    FunctionSignature::new(&params.iter().map(|t| (*t, "")).collect::<Vec<_>>(), &[ret])
}

/// **C1 守卫**：聚合落点探针必须带**成员形状**（`TyView::from_ir_type`），否则
/// AAPCS64/LP64D 的 HFA 会被当成"普通聚合"分流成 by-ref，而被调方按浮点寄存器收参
/// ⇒ 静默错值（v23 M2 修复）。
///
/// 判据用**同尺寸、不同成员**的一对类型对照——这是唯一能证明"成员真的进了探针"的
/// 方式（只比 size 的实现对两者给同一答案）：
///
/// - `{f64,f64,f64}`（24 B）是 HFA（aapcs64 `hfa_max = 4`）⇒ 浮点寄存器档；
/// - `{i64,i64,i64}`（24 B）不是 HFA、且 > 16 B ⇒ 按引用（内存）档。
#[test]
fn aggregate_place_probe_carries_member_shapes() {
    crate::pipeline_hooks::ensure_registered();
    let tm = crate::arch::arm64::TargetMachine::new();
    let mut store = TypeStore::new();
    let f64ty = store.float_ty(64);
    let i64ty = store.int_ty(64);
    let hfa = store.struct_anon(vec![f64ty, f64ty, f64ty], false);
    let plain = store.struct_anon(vec![i64ty, i64ty, i64ty], false);
    assert_eq!(store.size_bytes(hfa), 24);
    assert_eq!(store.size_bytes(plain), 24);

    // v25 M1：分类只有一处入口（`legalize/classify.rs`），本测试的断言意图不变——
    // **两者答案必须不同**，才说明形状带了成员（C1 修复的不变量）。
    let mut clf = crate::pipeline::legalize::classify::Classifier::new(&tm, "aapcs64", &store);
    let hfa_place = clf.arg_place(hfa).expect("HFA 分类");
    assert!(
        !crate::pipeline::legalize::classify::is_by_ref(&hfa_place),
        "aapcs64 的 24 字节 HFA 必须走寄存器档（形状丢了成员就会错判成 by-ref）：{hfa_place:?}"
    );
    let plain_place = clf.arg_place(plain).expect("普通聚合分类");
    assert!(
        crate::pipeline::legalize::classify::is_by_ref(&plain_place),
        "同尺寸但非 HFA 的聚合按引用（内存）：{plain_place:?}"
    );
}

/// Phase 4.2：builder 构造 IR 走完整管线，断言 AllocResult.assignments 非空且合法。
#[test]
fn test_alloc_result_assignments() {
    x86::ensure_registered();
    let mut b = FunctionBuilder::new(
        "alloc_test",
        TypeContext::new(),
        build_sig(&[], TypeId::I64),
    );
    b.create_block_here();
    let a = b.constant(21i64);
    let c = b.constant(2i64);
    let s = b.imul(a, c);
    b.ret(&[s]);
    let func = b.finish().expect("build");
    let machine = x86::TargetMachine::new();
    let (cf, alloc) = CompileSession::new(machine)
        .compile_with_alloc(&func)
        .expect("compile_with_alloc");
    assert!(!cf.code.is_empty(), "编译产物非空");
    assert!(
        !alloc.assignments.is_empty(),
        "assignments 非空（有 XReg 被分配）"
    );
    for (i, preg) in alloc.assignments.iter().enumerate() {
        let Some(preg) = preg else { continue };
        let xreg = XReg::new(i as u32, preg.class);
        assert!(
            preg.class == RegClass::GPR64 || preg.class.is_fp(),
            "XReg {xreg:?} 分配类非法: {:?}",
            preg.class
        );
    }
}

/// Phase 4.3：参数 ABI 分配——f(i64×4) 的前 4 参数 XReg 分配到 RCX/RDX/R8/R9。
#[test]
fn test_param_abi_allocation() {
    x86::ensure_registered();
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(
        &[
            (ctx.i64_ty(), "a"),
            (ctx.i64_ty(), "b"),
            (ctx.i64_ty(), "c"),
            (ctx.i64_ty(), "d"),
        ],
        &[ctx.i64_ty()],
    );
    let mut b = FunctionBuilder::new("param_abi", ctx, sig);
    let (_entry, params) = b.create_entry_block();
    // 四个参数求和（强制全部活跃跨函数体）
    let s0 = b.iadd(params[0], params[1]);
    let s1 = b.iadd(s0, params[2]);
    let s2 = b.iadd(s1, params[3]);
    b.ret(&[s2]);
    let func = b.finish().expect("build");
    let machine = x86::TargetMachine::new();
    let (_cf, alloc) = CompileSession::new(machine)
        .compile_with_alloc(&func)
        .expect("compile_with_alloc");
    // 参数 XReg（入口收参）由分配器自由分配（非 precolor arg_regs）：
    // entry 收参 mov 把 [abi.arg_regs]（RCX/RDX/R8/R9）的值搬给参数 XReg，
    // 随后 XReg 可任意分配——分配器从 GPR 高编号起（r15 先），天然避开
    // div 的 RAX/RDX 等低编号 clobber 寄存器。ABI 正确性由收参 mov +
    // 执行测试保证；这里断言：所有参数 XReg 都有合法 GPR 分配。
    let param_pregs: Vec<u32> = alloc
        .param_vregs
        .iter()
        .map(|x| alloc.preg(*x).map(|p| p.num).unwrap_or(u32::MAX))
        .collect();
    assert_eq!(
        alloc.param_vregs.len(),
        4,
        "应有 4 个参数 XReg（收参），实际 {}",
        alloc.param_vregs.len()
    );
    assert!(
        param_pregs.iter().all(|&n| n < 16),
        "参数 XReg 应全部有合法 GPR 分配，实际 {param_pregs:?}"
    );
    // 参数分配不得与 div clobber 寄存器（RAX=0/RDX=2）冲突——若参数落在
    // 其上，div 序列的 clobber 会强制溢出（见写死保护测试）。自由分配
    // 从高编号起，低压力下参数集中在 r15-r12。
    assert!(
        !param_pregs.contains(&0) && !param_pregs.contains(&2),
        "参数不得分配到 RAX/RDX（div clobber 区），实际 {param_pregs:?}"
    );
}

/// Phase 4.4：写死保护——div 序列（clobber RAX/RDX）+ 16 值活跃压力下，
/// 分配器必须产出完整分配（无 RegAlloc 错误、无未处理冲突），写死值不被覆盖。
/// 执行语义正确性由 forge-tests 的 test_udiv_pressure（672）验证；此处断言
/// 分配层完整性：每个分配/溢出的 XReg 都有合法去向。
#[test]
fn test_div_clobber_alloc_integrity() {
    x86::ensure_registered();
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[ctx.i64_ty()]);
    let mut b = FunctionBuilder::new("div_clobber_alloc", ctx, sig);
    b.create_block_here();
    let mut acc = b.constant(0i64);
    for i in 1..=16i64 {
        let a = b.constant(i * 42);
        let c = b.constant(i);
        let d = b.udiv(a, c);
        acc = b.iadd(acc, d);
    }
    b.ret(&[acc]);
    let func = b.finish().expect("build");
    let machine = x86::TargetMachine::new();
    let (cf, alloc) = CompileSession::new(machine)
        .compile_with_alloc(&func)
        .expect("div + clobber 压力下编译必须成功（无 RegAlloc 错误）");
    assert!(!cf.code.is_empty(), "编译产物非空");
    assert!(!alloc.assignments.is_empty(), "必须有分配");
    // 写死保护：clobber 寄存器（RAX=0/RDX=2）上的分配不可能是"跨 div 序列
    // 活跃的中间值"——clobber 机制在序列点把它们溢出（spill_slots 记录）
    // 或分配器避开。断言：每个 spill 槽的 XReg 大小合法（spill 数据完整）。
    for (i, slot) in alloc.spill_slots.iter().enumerate() {
        let Some(slot) = slot else { continue };
        let xreg = XReg::new(i as u32, forge_ir::RegClass::GPR64);
        assert!(
            slot.size >= 4 && slot.size <= 16,
            "spill 槽大小非法: {xreg:?} -> {slot:?}"
        );
    }
}

/// P0-1 集成：`with_opt_level` 编译常量折叠函数——O1 管线把
/// `iconst(21)*iconst(2)` 折叠为常量，编译产物仍正确（此前优化管线
/// 在生产路径是死代码）。
#[test]
fn p0_compile_with_opt_level() {
    x86::ensure_registered();
    let mut b = FunctionBuilder::new(
        "opt_const_fold",
        TypeContext::new(),
        build_sig(&[], TypeId::I64),
    );
    b.create_block_here();
    let a = b.constant(21i64);
    let c = b.constant(2i64);
    let s = b.imul(a, c);
    b.ret(&[s]);
    let func = b.finish().expect("build");

    let machine = x86::TargetMachine::new();
    // 无优化编译（对照）
    let (cf0, _) = CompileSession::new(machine.clone())
        .compile_with_alloc(&func)
        .expect("compile 无优化");
    // O1 优化编译（常量折叠 21*2 → 42）
    let (cf1, _) = CompileSession::new(machine.clone())
        .with_opt_level(forge_opt::OptimizationLevel::O1)
        .compile_with_alloc(&func)
        .expect("compile 带 O1");
    assert!(!cf0.code.is_empty(), "无优化产物非空");
    assert!(!cf1.code.is_empty(), "O1 产物非空");
    // O1 常量折叠后指令更少（movabs 42 而非 movabs 21 + movabs 2 + imul）
    assert!(
        cf1.code.len() <= cf0.code.len(),
        "O1 常量折叠应不增大代码（P0-1 回归：优化管线未生效）"
    );
}

/// WA-37 D5 守卫：V512（64 字节）by-ref 参数的**被调方**收参，load 变体必须
/// 按参数 IR 字节宽分派到 **64B（EVEX zmm）**。旧实现按参数 XReg 的寄存器类
/// 宽分派（`reg_class_for` 对 >128 位恒给 `VEC(32)`）→ 64B 分支永不可达，
/// 只从 [ptr] 拷 32B（lane8..15 丢失；原测试只断言 lane0 故未暴露）。
///
/// 本机无 AVX-512F 时用 `FORGE_ASSUME_AVX512=1` 放开**可行性守卫**以验证
/// 编码分派——本测试只编译不执行（EVEX 真执行需硬件）。
#[test]
fn test_v512_byref_callee_load_is_64b() {
    x86::ensure_registered();
    // 安全：仅本测试设置该 env（进程内其他测试只读；只影响能力判断）
    let _env_guard = crate::AVX512_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _assume = AssumeAvx512::set();
    // 可见性事件：本用例恒跑（env 放开可行性门）——落盘以便 CI 日志佐证。
    crate::jit_event("V512-GEN", "test_v512_byref_callee_load_is_64b");
    // 类型必须建立在**同一个** TypeContext 上（跨上下文 TypeId 会越界：
    // 参考 jit.rs 的 V512 用例在本机因无 AVX-512 直接 return，其体从未跑到）。
    let tc = TypeContext::new();
    let vt = tc.vector_ty(TypeId::F32, 16); // 16 × f32 = 64 字节 = V512
    let sig = FunctionSignature::new(&[(vt, "v")], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("callee_v512", tc, sig);
    let (blk, p) = b.create_block_with_params(&[(vt, "v")]);
    b.switch_to_block(blk);
    let idx = b.constant(0i32);
    let lane = b.vextract(p[0], idx);
    let wide = b.fpext(lane, TypeId::F64);
    let int = b.fptosi(wide, TypeId::I32);
    b.ret(&[int]);
    let func = b.finish().expect("build v512 callee");
    let cf = CompileSession::new(x86::TargetMachine::new())
        .compile(&func)
        .expect("compile v512 callee（FORGE_ASSUME_AVX512 下应通过守卫）");
    // EVEX 前缀（62）+ EVEX 编码的 0F 10 家族；32B 变体是 3 字节 VEX
    // （C4 P0 P1 10，P1 = W0/vvvv=1111/L=1/pp=00 → 0x7C）——本编码器恒发
    // 3 字节 VEX，故负向断言按 C4 形态匹配（旧的 `C5 FC 10` 恒不命中 =
    // 空断言，2026-09-12 W3 修正）。
    let has_evex_load = cf.code.windows(5).any(|w| w[0] == 0x62 && w[4] == 0x10);
    let has_vex_ymm_load = cf
        .code
        .windows(4)
        .any(|w| w[0] == 0xC4 && w[2] == 0x7C && w[3] == 0x10);
    assert!(
        has_evex_load,
        "V512 收参应含 EVEX zmm load（62 ... 10）：D5 回归（按类宽分派 → 只拷 32B）。\
             prologue bytes = {:02x?}",
        &cf.code[..cf.code.len().min(48)]
    );
    assert!(
        !has_vex_ymm_load,
        "V512 收参不得退回 32B VEX.256 ymm load（C4 .. 7C 10，lane8..15 丢失）"
    );
}

/// V512（64 字节）`Vconst` 的**生成级**守卫（不需要 AVX-512 硬件：只编译不执行）。
///
/// 背景：`isa/x86.toml` 的 `Vconst` 规则原只覆盖 `rd = 64/128/256`，V512
/// 常量落到「no matching rule」→ `Unsupported`。该缺口只在**有 AVX-512F 的机器**
/// 上暴露（`runtime::jit` 的 `test_jit_v512_byref_param` 无 AVX-512F 时提前
/// return，本机即如此），历史上因此在 CI 上偶发红（Test (Windows) 命中
/// AVX-512 runner 的 run）。本测试用「**无宽向量参数/返回** + 函数内局部 V512
/// 常量」构造——不触发宽向量 ABI 的 AVX-512 入口守卫，任何机器都能编译验证。
///
/// 断言：①编译成功（修复前 `crate::IrError::capability("VCONST_NO_RULE", "DSL lowering: Vconst no matching rule")`）；
/// ②机器码含 **4 条 EVEX `VINSERTF32X4`**（`62 P0 P1 P2 18 /r ib`）且 imm 恰为
/// 0/1/2/3 各一次 ⇒ 512 位的 4 个 128 位 lane 都被显式写入（这也是 Vconst 规则
/// 「`{out}` 自身当累加器、初始值不影响结果」这一构造前提的可执行证据）。
///
/// W3（2026-09-12）起：64B 向量值（含 Vconst）统一由编译入口的字节门守卫
/// 「>32B 需 AVX-512F」——无硬件时用 `FORGE_ASSUME_AVX512=1` 放开可行性门
/// （本例只编译不执行，EVEX 真执行由有硬件的 runner 守护）。
#[test]
fn test_v512_vconst_generates_four_evex_inserts() {
    x86::ensure_registered();
    // 安全：仅本测试设置该 env（AVX512_ENV_LOCK 串行化，见 lib.rs 说明）。
    let _env_guard = crate::AVX512_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _assume = AssumeAvx512::set();
    // 可见性事件：本用例恒跑（env 放开生成期门）——落盘以便 CI 日志佐证。
    crate::jit_event("V512-GEN", "test_v512_vconst_generates_four_evex_inserts");
    let tc = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("v512_vconst", tc, sig);
    b.create_block_here();
    // 16 × f32 = 64 字节 = V512（类型由 T+len 推导）。
    let v = b.const_vector(vec![
        1.5f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0,
    ]);
    // 取 lane15（=16.0）——与运行级 V512 用例同形，便于两处对照。
    let idx = b.constant(15i32);
    let lane = b.vextract(v, idx);
    let wide = b.fpext(lane, TypeId::F64);
    let int = b.fptosi(wide, TypeId::I32);
    b.ret(&[int]);
    let func = b.finish().expect("build v512 vconst");
    let cf = CompileSession::new(x86::TargetMachine::new())
        .compile(&func)
        .expect("V512 常量应可降级（修复前：Vconst no matching rule）");
    // EVEX = 62 + P0 P1 P2 + opcode：窗口 [62, P0, P1, P2, opcode, modrm, imm]。
    let mut imms: Vec<u8> = Vec::new();
    for w in cf.code.windows(7) {
        if w[0] == 0x62 && w[4] == 0x18 {
            imms.push(w[6]);
        }
    }
    imms.sort_unstable();
    assert_eq!(
        imms,
        vec![0u8, 1, 2, 3],
        "V512 常量应由 4 条 EVEX VINSERTF32X4 拼成、imm = 0/1/2/3 各一次（4 个 lane 全写）。\
             实际 imm = {imms:?}；code = {:02x?}",
        &cf.code[..cf.code.len().min(64)]
    );
}

/// 构造并编译 `${lanes}×f32` 向量的**栈槽往返**函数（W3 宽向量 Load/Store
/// 用例共用）：vconst → alloca → store → load → vextract lane0 → i32。
fn compile_wide_slot_roundtrip(lanes: usize) -> Result<CompiledFunction, IrError> {
    let tc = TypeContext::new();
    let vt = tc.vector_ty(TypeId::F32, lanes as u32);
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("wide_vec_slot", tc, sig);
    let (blk, _p) = b.create_block_with_params(&[]);
    b.switch_to_block(blk);
    let v = b.const_vector(vec![1.0f32; lanes]);
    let slot = b.alloca(vt, 1);
    b.store(v, slot);
    let back = b.load(slot, vt);
    let idx = b.constant(0i32);
    let lane = b.vextract(back, idx);
    let wide = b.fpext(lane, TypeId::F64);
    let int = b.fptosi(wide, TypeId::I32);
    b.ret(&[int]);
    let func = b.finish().expect("build wide vector slot roundtrip");
    CompileSession::new(x86::TargetMachine::new()).compile(&func)
}

/// **栈探测**（v27 M6-78）：帧超过阈值时序言必须先 `mov rax, 帧大小` + `call __chkstk`
/// （带 `__chkstk` 重定位），否则 Windows 上会跳过 guard page ⇒ 随机 AV。
///
/// 正反两面都钉：大帧 ⇒ 有探测；小帧 ⇒ 无探测（不能给所有函数都加一条 call）。
#[cfg(target_arch = "x86_64")]
#[test]
fn stack_probe_emitted_only_for_big_frames() {
    // 大帧：64KB 的栈槽
    let tc = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b = FunctionBuilder::new("big_frame", tc.clone(), sig);
    let (blk, _p) = b.create_block_with_params(&[]);
    b.switch_to_block(blk);
    let slot = b.alloca(TypeId::I8, 65536);
    let z = b.constant(0i8);
    b.store(z, slot);
    let back = b.load(slot, TypeId::I8);
    let v = b.uextend(back, TypeId::I32);
    b.ret(&[v]);
    let func = b.finish().expect("build big_frame");
    let cf = CompileSession::new(x86::TargetMachine::new())
        .compile(&func)
        .expect("compile big_frame");
    assert!(
        cf.code.len() > 10,
        "大帧函数应有可观代码量，实测 {}",
        cf.code.len()
    );
    // 序言 = `49 BB <imm64>`（mov r11, <绝对地址>）+ `48 B8 <imm64>`（mov rax, size）
    // + `41 FF D3`（call r11）——绝对地址 thunk，`WA-51` 的距离问题不再存在。
    assert_eq!(cf.code[0], 0x49, "序言首字节应为 mov r11, imm64 的 REX.WB");
    assert_eq!(cf.code[1], 0xBB, "序言应为 mov r11, imm64（49 BB）");
    assert_eq!(
        cf.code[10], 0x48,
        "符号装载之后应是 mov rax, imm64 的 REX.W"
    );
    assert_eq!(cf.code[11], 0xB8, "第二条应为 mov rax, imm64（48 B8）");
    assert_eq!(
        &cf.code[20..23],
        &[0x41, 0xFF, 0xD3],
        "第三条应为 call r11（41 FF D3）"
    );
    let probe_reloc = cf
        .relocations
        .iter()
        .find(|r| r.symbol.as_str() == "__chkstk");
    assert!(
        probe_reloc.is_some(),
        "大帧序言必须带 `__chkstk` 重定位，实测重定位：{:?}",
        cf.relocations
    );

    // 小帧：同一形状但不该触发探测
    let tc2 = TypeContext::new();
    let sig2 = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut b2 = FunctionBuilder::new("small_frame", tc2, sig2);
    let (blk2, _p2) = b2.create_block_with_params(&[]);
    b2.switch_to_block(blk2);
    let z2 = b2.constant(7i32);
    b2.ret(&[z2]);
    let func2 = b2.finish().expect("build small_frame");
    let cf2 = CompileSession::new(x86::TargetMachine::new())
        .compile(&func2)
        .expect("compile small_frame");
    assert!(
        !cf2.relocations
            .iter()
            .any(|r| r.symbol.as_str() == "__chkstk"),
        "小帧不得发栈探测，实测重定位：{:?}",
        cf2.relocations
    );
}

/// W3（2026-09-12）：V256（32B）向量的 **IR 层 Load/Store 已可降级**——
/// ISA 规则 `rd_vec/rs1_vec = 32` → `VMOVUPS_RM/MR`（VEX.256，`vex_l=1`；
/// 槽类 `fpr16` 的寄存器编号即 ymm 号）。旧实现把 >16B 的 Load/Store 一律
/// fail-closed 拒绝（"ISA 类模型缺 YMM 槽类"）——本轮由规则 + 编译入口字节门
/// （>32B 才要 AVX-512F）共同承载。
///
/// 只编译不执行（运行级 lane 往返见 `runtime::jit::test_jit_v256_slot_roundtrip`）。
/// 断言机器码含 **VEX.256** 的 0F 10/11（vmovups ymm）：本编码器恒发 3 字节
/// VEX（`C4 P0 P1`），P1 = W=0/vvvv=1111/L=1/pp=00 → `0x7C`（仅 R̄/X̄/B̄ 在 P0）。
#[test]
fn test_v256_slot_load_store_is_lowered() {
    x86::ensure_registered();
    let cf = compile_wide_slot_roundtrip(8).expect("32B 向量槽往返应可降级（W3）");
    let is_vex256 = |w: &[u8], op: u8| w[0] == 0xC4 && w[2] == 0x7C && w[3] == op;
    assert!(
        cf.code.windows(4).any(|w| is_vex256(w, 0x10)),
        "32B load 应为 VEX.256 vmovups ymm, [base]（C4 .. 7C 10）：code = {:02x?}",
        &cf.code[..cf.code.len().min(64)]
    );
    assert!(
        cf.code.windows(4).any(|w| is_vex256(w, 0x11)),
        "32B store 应为 VEX.256 vmovups [base], ymm（C4 .. 7C 11）：code = {:02x?}",
        &cf.code[..cf.code.len().min(64)]
    );
    // 反向守卫：32B 槽往返**不得**只搬 8 字节（旧默认规则只会发 movsd
    // F3 0F 10/11 → lane1..7 静默截断）。
    assert!(
        !cf.code
            .windows(3)
            .any(|w| w == [0xF3, 0x0F, 0x10] || w == [0xF3, 0x0F, 0x11]),
        "32B 槽往返不得退回 movsd（8 字节静默截断）：code = {:02x?}",
        &cf.code[..cf.code.len().min(64)]
    );
}

/// W3（2026-09-12）：V512（64B）向量的 IR 层 Load/Store——**需 AVX-512F**
/// （EVEX vmovups zmm）。分两半：①无 AVX-512F 的机器上必须**编译期拒绝**
/// （与 ABI 守卫同一判据，不发射本机无法执行的 EVEX）；②`FORGE_ASSUME_AVX512=1`
/// 放开可行性门后成功降级且含 EVEX 0F 10/11（只编译不执行）。
///
/// 本机（无 AVX-512F）两半都跑到；有 AVX-512F 的机器跳过①（无从构造"无
/// 能力"场景）——②由生成级断言守护编码。
#[test]
fn test_v512_slot_load_store_requires_avx512() {
    x86::ensure_registered();
    // 安全：仅本测试设置该 env（与 D5 用例共用 AVX512_ENV_LOCK 串行化）。
    let _env_guard = crate::AVX512_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if crate::avx512_hardware_available() {
        // 可见性事件：本机是否有 AVX-512F 决定这一半是否跑到（libtest 吞掉
        // 通过测试的输出 → 只能靠 `FORGE_JIT_EVENTS` 落盘判断）。
        crate::jit_event("AVX512-HW=1", "test_v512_slot_load_store_requires_avx512");
        eprintln!("[w3] 本机有 AVX-512F——跳过「无能力必须拒绝」一半（无从构造）");
    } else {
        crate::jit_event("AVX512-HW=0", "test_v512_slot_load_store_requires_avx512");
        let err = match compile_wide_slot_roundtrip(16) {
            Ok(_) => panic!("无 AVX-512F 时 64B 向量必须编译期拒绝（不得发射 EVEX）"),
            Err(e) => e,
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("64 字节宽向量") && msg.contains("宽向量档位"),
            "64B：错误信息应说明字节数与「本机宽向量档位」这一判据：{msg}"
        );
    }
    let _assume = AssumeAvx512::set();
    let cf = compile_wide_slot_roundtrip(16)
        .expect("FORGE_ASSUME_AVX512 下 64B 向量槽往返应可降级（W3）");
    // EVEX：62 + P0 P1 P2 + opcode（窗口 [62, P0, P1, P2, opcode, modrm]）；
    // P2 的 L'L = 10（bits 6:5）= 512 位宽。
    let has_evex = |op: u8| {
        cf.code
            .windows(5)
            .any(|w| w[0] == 0x62 && w[4] == op && (w[3] & 0x60) == 0x40)
    };
    assert!(
        has_evex(0x10),
        "64B load 应为 EVEX vmovups zmm, [mem]（62 ... 10）：code = {:02x?}",
        &cf.code[..cf.code.len().min(64)]
    );
    assert!(
        has_evex(0x11),
        "64B store 应为 EVEX vmovups [mem], zmm（62 ... 11）：code = {:02x?}",
        &cf.code[..cf.code.len().min(64)]
    );
}

/// **WA-46 宽度分档守卫（类映射）**：向量值的寄存器类必须按**值宽**分档——
/// ≤16B → `VEC(128)`、≤32B → `VEC(256)`、>32B → **`VEC(512)`**。旧实现 >128 位
/// 一律 `VEC(256)`，而类的 `reg_width` 决定 spill 槽大小与 `xreg.width()`
/// ⇒ 64B 的 V512 值只有 32B 槽、搬运宽度也只报 32（溢出后高半区静默截断）。
/// 这里同时锁住链路：`VEC(512).bytes() == 64`（类表 fallback 用
/// `class.bytes()` 作 `reg_width`，见上方 classes 构建）。
#[test]
fn test_vector_reg_class_widths() {
    let tc = TypeContext::new();
    let v64 = tc.vector_ty(TypeId::F32, 2);
    let v128 = tc.vector_ty(TypeId::F32, 4);
    let v256 = tc.vector_ty(TypeId::F32, 8);
    let v512 = tc.vector_ty(TypeId::F32, 16);
    let mut ctx = LowerCtx::new();
    ctx.type_store = Some(tc.borrow());
    assert_eq!(ctx.reg_class_for(&v64), RegClass::VEC(128));
    assert_eq!(ctx.reg_class_for(&v128), RegClass::VEC(128));
    assert_eq!(ctx.reg_class_for(&v256), RegClass::VEC(256));
    assert_eq!(
        ctx.reg_class_for(&v512),
        RegClass::VEC(512),
        "64 字节向量必须归 VEC(512)（槽宽/搬运宽度按值宽；WA-46）"
    );
    assert_eq!(
        RegClass::VEC(512).bytes(),
        64,
        "VEC(512) 的 reg_width 必须是 64 字节（spill 槽 = 64 字节）"
    );
}

/// **WA-46 溢出宽度守卫（生成级，直接驱动 FrameLowering）**：FPR 溢出
/// 必须按 `width` 搬运——旧实现只有一份 8 字节 `MOVSD` 模板且生成的
/// `emit_spill_load/store` 忽略 width ⇒ 向量值 spill 只搬低 8 字节
/// （高半区静默截断）。用例绕过分配器（高压力下分配器会因 3-scratch 限制
/// 拒绝，测不到该路径），直接对每个宽度检查发射的机器码形状。
#[test]
fn test_fpr_spill_width_dispatch() {
    use crate::machine::frame::TargetFrameLowering;
    x86::ensure_registered();
    let fl = x86::FrameLowering;
    let emit = |width: u16, is_load: bool| -> Result<Vec<u8>, IrError> {
        let mut sink = crate::pipeline::emit::CodeSink::new();
        if is_load {
            fl.emit_spill_load(0, -32, width, RegClass::FPR64, &mut sink)?;
        } else {
            fl.emit_spill_store(0, -32, width, RegClass::FPR64, &mut sink)?;
        }
        Ok(sink.bytes().to_vec())
    };
    // 8 字节标量：movsd（F2 0F 10/11）
    let s8 = emit(8, false).expect("8B store");
    assert_eq!(&s8[..3], &[0xF2, 0x0F, 0x11], "8B spill store 应为 movsd");
    let l8 = emit(8, true).expect("8B load");
    assert_eq!(&l8[..3], &[0xF2, 0x0F, 0x10], "8B spill load 应为 movsd");
    // 16 字节：movups（0F 10/11，无 F2/F3 前缀）
    let s16 = emit(16, false).expect("16B store");
    assert_eq!(
        &s16[..2],
        &[0x0F, 0x11],
        "16B spill store 应为 movups（0F 11）"
    );
    let l16 = emit(16, true).expect("16B load");
    assert_eq!(
        &l16[..2],
        &[0x0F, 0x10],
        "16B spill load 应为 movups（0F 10）"
    );
    // 32 字节：VEX.256 vmovups（C4 .. 7C 10/11）
    let s32 = emit(32, false).expect("32B store");
    assert!(
        s32.len() >= 4 && s32[0] == 0xC4 && s32[2] == 0x7C && s32[3] == 0x11,
        "32B spill store 应为 VEX.256 vmovups（C4 .. 7C 11）：{s32:02x?}"
    );
    let l32 = emit(32, true).expect("32B load");
    assert!(
        l32.len() >= 4 && l32[0] == 0xC4 && l32[2] == 0x7C && l32[3] == 0x10,
        "32B spill load 应为 VEX.256 vmovups（C4 .. 7C 10）：{l32:02x?}"
    );
    // 64 字节：EVEX.512 vmovups（62 .. 10/11，P2 的 L'L = 10）
    let s64 = emit(64, false).expect("64B store");
    assert!(
        s64.len() >= 5 && s64[0] == 0x62 && s64[4] == 0x11 && (s64[3] & 0x60) == 0x40,
        "64B spill store 应为 EVEX zmm vmovups（62 .. 11）：{s64:02x?}"
    );
    let l64 = emit(64, true).expect("64B load");
    assert!(
        l64.len() >= 5 && l64[0] == 0x62 && l64[4] == 0x10 && (l64[3] & 0x60) == 0x40,
        "64B spill load 应为 EVEX zmm vmovups（62 .. 10）：{l64:02x?}"
    );
    // 其它宽度 → fail-closed（不静默退回窄搬运）。
    for w in [24u16, 48] {
        let e = emit(w, false).expect_err("未声明宽度的 store 必须报错");
        let msg = format!("{e}");
        assert!(
            msg.contains("spill"),
            "{w}B：错误信息应说明 spill 宽度缺失：{msg}"
        );
        assert!(emit(w, true).is_err(), "{w}B：load 同样必须 fail-closed");
    }
}

/// **掩码类（KReg）溢出必须 fail-closed**（v23 M5：spill 搬运由 `is_fp: bool`
/// 改成按**寄存器类**分派之后新增的那条守卫）。
///
/// 布尔时代：KReg 与 GPR 同为 `is_fp == false` ⇒ 掩码值会**静默**按通用寄存器
/// 搬运（发一条 mov，读回来的东西与掩码寄存器无关）。现在必须在编译期明确报错，
/// 而不是发一条错的指令。
#[test]
fn test_mask_class_spill_is_fail_closed() {
    use crate::machine::frame::TargetFrameLowering;
    x86::ensure_registered();
    let fl = x86::FrameLowering;
    for class in [RegClass::KReg(8), RegClass::KReg(16)] {
        let mut sink = crate::pipeline::emit::CodeSink::new();
        let err = fl
            .emit_spill_load(0, -32, 8, class, &mut sink)
            .expect_err("掩码类 spill load 必须报错（本仓没有掩码溢出模板）");
        let msg = format!("{err}");
        assert!(
            msg.contains("掩码") && msg.contains("spill"),
            "{class:?}：错误信息应点名掩码类与 spill，实际：{msg}"
        );
        assert!(
            fl.emit_spill_store(0, -32, 8, class, &mut sink).is_err(),
            "{class:?}：store 同样必须 fail-closed"
        );
    }
}

/// `class` 与 `width` **不可互相替代**：同一个 `FPR(8)` 池的寄存器既能装 4 字节
/// （f32）也能装 8 字节（f64）——宽度档由 `width` 决定，指令族由 `class` 决定。
/// 这条用例把两种宽度都过一遍同一个类，钉住"宽度确实参与了分派"。
#[test]
fn spill_width_and_class_are_independent() {
    use crate::machine::frame::TargetFrameLowering;
    x86::ensure_registered();
    let fl = x86::FrameLowering;
    let emit = |width: u16| -> Vec<u8> {
        let mut sink = crate::pipeline::emit::CodeSink::new();
        fl.emit_spill_store(0, -32, width, RegClass::FPR64, &mut sink)
            .expect("FPR 溢出");
        sink.bytes().to_vec()
    };
    // 4 字节（f32 值）与 8 字节（f64 值）走**同一个类**、不同宽度 ⇒ 允许走同一档
    // 标量模板（≤ 浮点值池），但都必须是"浮点族"的编码（F2 前缀的 movsd），
    // 绝不能因为"宽度只有 4"就退成通用寄存器搬运。
    let s4 = emit(4);
    let s8 = emit(8);
    assert_eq!(&s4[..3], &[0xF2, 0x0F, 0x11], "4B FPR 溢出仍走浮点族");
    assert_eq!(&s8[..3], &[0xF2, 0x0F, 0x11], "8B FPR 溢出走浮点族");
    // 而 16 字节同族值走另一档（movups）——宽度变了，模板就变。
    let s16 = emit(16);
    assert_eq!(&s16[..2], &[0x0F, 0x11], "16B 同族值换宽度档");
}
