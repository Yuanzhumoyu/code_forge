//! **类间位搬移**（v20 V7）：值的寄存器类与 ABI 落点的类**不同**时不允许静默错值。
//!
//! 这个形状不是假想的——psABI 的**整数约定收浮点**就是它：RISC-V 的变参实参一律按整数约定传
//! （`riscv-cc.adoc` 的浮点调用约定一节：*"The remainder of this section applies only to named
//! arguments. Variadic arguments are passed according to the integer calling convention."*），
//! Zfinx/软浮点约定同理（浮点标量进整数寄存器）。
//!
//! 旧实现只看**落点的类**决定用哪条搬移指令：浮点值落在整数寄存器时会把 FPR 的号当 GPR 号用
//! （`Reg::from_index(10, GPR)` = `x10`），**静默错值**。本文件用一份**合成约定**（浮点判给
//! `int` 池）走真实管线，钉住两件事：
//!
//! ① 两侧都发**类间位搬移**（riscv：`fmv.d.x`/`fmv.x.d`；x86：`MOVQ`）；
//! ② 与"正常分类"（浮点进 FPR 池）走的仍是同类搬移（`fsgnj.d`）——两路互不串味。

use forge_codegen::FunctionCompiler;
use forge_codegen::arch::riscv64::TargetMachine;
use forge_codegen::pipeline::conv_registry;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::{CallConvId, ConvName, TypeId};

/// 合成约定：**浮点标量判给 `int` 池**（"整数约定收浮点"）。
///
/// 只覆写 `classify`（去掉浮点/HFA 两条 ⇒ 浮点落到最后的 `scalar` ⇒ `int` 池），
/// 其余（栈参数、callee-saved、返回分类）继承 `c`。
const ZFINX_LIKE: &str = r#"
name = "zfinx_like"
parent = "c"
classify = [
  { when = { kind = "vector", size_le = 16 },    do = { direct = { pool = "int" } } },
  { when = { kind = "aggregate", hfa_max = 2 },  do = { direct = { pool = "int", slots = "hfa" } } },
  { when = { kind = "aggregate", size_le = 16 }, do = { direct = { pool = "int", slots = 2 } } },
  { when = { kind = "aggregate", size_gt = 16 }, do = { indirect = { via = "caller_stack_copy" } } },
  { when = { kind = "scalar" },                  do = { direct = { pool = "int" } } },
]
"#;

/// 绑定：与内置 lp64d 同一组池，只是挂在约定名 `zfinx_like` 上。
const ZFINX_BINDING: &str = r#"
isa = "riscv64"
conv = "zfinx_like"
[pools]
int = ["X10", "X11", "X12", "X13", "X14", "X15", "X16", "X17"]
float = ["F10", "F11", "F12", "F13", "F14", "F15", "F16", "F17"]
cs_gpr = ["X9", "X18", "X19", "X20", "X21", "X22", "X23", "X24", "X25", "X26", "X27"]
ret_int = ["X10", "X11"]
ret_float = ["F10", "F11"]
"#;

/// `fn f(x: f64) -> i64 { fptosi(x) }` —— 被调方收一个浮点标量。
fn f64_param_ret_i64(conv: CallConvId) -> forge_ir::Function {
    let ctx = TypeContext::new();
    let sig =
        FunctionSignature::new(&[(TypeId::F64, "x")], &[TypeId::I64]).with_calling_convention(conv);
    let mut b = FunctionBuilder::new("bank_probe", ctx, sig);
    let (entry, params) = b.create_block_with_params(&[(TypeId::F64, "x")]);
    b.switch_to_block(entry);
    let v = b.fptosi(params[0], TypeId::I64);
    b.ret(&[v]);
    b.finish().expect("build")
}

/// RISC-V 的编码判据（`fmv.d.x rd, rs1`：opcode 0x53、funct3 = 0、funct7 = 0x79）。
fn has_fmv_d_x(code: &[u8]) -> bool {
    code.windows(4).any(|c| {
        let w = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        w & 0xFE00_707F == 0xF200_0053
    })
}

/// `fsgnj.d rd, rs, rs`（`fpr_mov` 的 64 位档：opcode 0x53、funct3 = 0、funct7 = 0x11）。
fn has_fsgnj_d(code: &[u8]) -> bool {
    code.windows(4).any(|c| {
        let w = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        w & 0xFE00_707F == 0x2200_0053
    })
}

/// `fmv.x.d rd, rs1`（`fpr_to_gpr_mov` 的 64 位档：opcode 0x53、funct3 = 0、funct7 = 0x71）。
fn has_fmv_x_d(code: &[u8]) -> bool {
    code.windows(4).any(|c| {
        let w = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        w & 0xFE00_707F == 0xE200_0053
    })
}

/// ① 落点是整数寄存器、值是浮点 ⇒ **类间位搬移**（`fmv.d.x`），不是把 FPR 的号当 GPR 用。
#[test]
fn float_value_into_an_int_place_uses_the_cross_bank_move() {
    conv_registry::register_rules_toml(ZFINX_LIKE).expect("注册规则");
    conv_registry::register_binding_toml(ZFINX_BINDING).expect("注册绑定");

    let cf = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&f64_param_ret_i64(CallConvId::named("zfinx_like")))
        .expect("整数约定收浮点应当能编");
    assert!(
        has_fmv_d_x(&cf.code),
        "落点整数 + 值浮点 ⇒ 必须发类间位搬移 `fmv.d.x`（旧实现把 FPR 的号当 GPR 号用）：{:02x?}",
        cf.code
    );
}

/// ② 对照组：**正常分类**（浮点进 FPR 池，内置 lp64d）走的仍是同类搬移 `fsgnj.d`——
/// 说明四路分派没有把两条路串起来。
#[test]
fn float_value_into_an_fpr_place_stays_a_same_bank_move() {
    let cf = FunctionCompiler::new(TargetMachine::new())
        .compile_raw(&f64_param_ret_i64(CallConvId::builtin(ConvName::Lp64d)))
        .expect("lp64d 下浮点参数照常收参");
    assert!(
        has_fsgnj_d(&cf.code),
        "落点 FPR + 值浮点 ⇒ 同类搬移 `fsgnj.d`：{:02x?}",
        cf.code
    );
    assert!(
        !has_fmv_d_x(&cf.code),
        "不该发类间位搬移（落点就是 FPR 池）：{:02x?}",
        cf.code
    );
}

/// ③ **调用点**（数据流相反）：值在 FPR、落点是整数寄存器 ⇒ 发 `fmv.x.d`（`fpr_to_gpr_mov`）。
///
/// 这条覆盖 `arg_move_loop` 的四路分派：被调方是变参/整数约定时，调用方要把浮点值的**位模式**
/// 放进 a0-a7——RISC-V 变参正是这个形状（定本：变参实参按整数约定传）。
#[test]
fn call_site_float_arg_into_an_int_place_uses_the_cross_bank_move() {
    use forge_ir::ir::function::Module;

    conv_registry::register_rules_toml(ZFINX_LIKE).expect("注册规则");
    conv_registry::register_binding_toml(ZFINX_BINDING).expect("注册绑定");

    let mut m = Module::new();
    // callee: (f64) -> i64 —— 只作为调用目标（本用例只编 caller）。
    let sig_c = FunctionSignature::new(&[(TypeId::F64, "x")], &[TypeId::I64]);
    let mut bc = FunctionBuilder::new("callee", TypeContext::new(), sig_c);
    let (blk, p) = bc.create_block_with_params(&[(TypeId::F64, "x")]);
    bc.switch_to_block(blk);
    let v = bc.fptosi(p[0], TypeId::I64);
    bc.ret(&[v]);
    let callee = m.add_function(bc.finish().expect("callee"));

    // caller: () -> i64 { callee(1.5) }，**自己**用 zfinx_like（调用点的落点按本函数的约定算）。
    let sig_m = FunctionSignature::new(&[], &[TypeId::I64])
        .with_calling_convention(CallConvId::named("zfinx_like"));
    let mut bm = FunctionBuilder::new("caller", TypeContext::new(), sig_m);
    bm.create_block_here();
    let x = bm.fconst(1.5f64.to_bits(), TypeId::F64);
    let r = bm.call(callee, &[x], &[TypeId::I64]);
    // 单值返回：`call` 返回 Vec，取第 0 个。
    bm.ret(&[r[0]]);
    let caller = m.add_function(bm.finish().expect("caller"));

    let cf = FunctionCompiler::for_module(TargetMachine::new(), &m)
        .compile_raw(m.get_function(caller))
        .expect("整数约定下的调用点应当能编");
    assert!(
        has_fmv_x_d(&cf.code),
        "调用点：落点整数 + 值浮点 ⇒ 必须发 `fmv.x.d`（位模式进参数寄存器）：{:02x?}",
        cf.code
    );
}
