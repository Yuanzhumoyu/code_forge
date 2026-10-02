//! **变参元信息寄存器**（v20 V7，SysV 的 `%al`）：调用变参函数时，调用方要把"用了几个向量
//! 寄存器"写进约定的元信息寄存器——glibc 的 `printf` 一族靠它决定从寄存器保存区里读几个 XMM
//! （`%al = 0` = 一个都没用）。不写就是**静默错值**（外部变参函数的浮点实参会读错）。
//!
//! 这条以前是"零消费"：引擎算得出 `hidden.va_meta`，但没有任何发射侧读者。现在调用点按
//! **布局**算出这个数（运行期才知道），用"立即数 → 整数寄存器"（`roles = ["gpr_mov_imm"]`）
//! 写进去；本 ISA 没申报这条能力而约定又要写 ⇒ **fail-closed**。

use forge_codegen::FunctionCompiler;
use forge_codegen::arch::x86::TargetMachine;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::function::Module;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::{CallConvId, ConvName, TypeId};

/// `mov rax, imm64` = `48 B8` + 8 字节小端立即数（`%al` 就是这条的低字节）。
fn has_mov_rax_imm(code: &[u8], imm: u64) -> bool {
    let mut want = vec![0x48u8, 0xB8];
    want.extend_from_slice(&imm.to_le_bytes());
    code.windows(want.len()).any(|w| w == want.as_slice())
}

/// 建一个"被调方 + 调用方"的模块：调用方用 sysv64 调一个带 `(i64, f64)` 实参的函数。
/// `callee_variadic` 决定被调方是不是变参（只有变参才要求写 `%al`）。
fn caller_calling(callee_variadic: bool) -> Vec<u8> {
    let mut m = Module::new();
    let sig_c = FunctionSignature::new(&[(TypeId::I64, "fmt")], &[TypeId::I64])
        .with_variadic(callee_variadic);
    let mut bc = FunctionBuilder::new("callee", TypeContext::new(), sig_c);
    let (blk, p) = bc.create_block_with_params(&[(TypeId::I64, "fmt")]);
    bc.switch_to_block(blk);
    bc.ret(&[p[0]]);
    let callee = m.add_function(bc.finish().expect("callee"));

    // 调用方（sysv64）：`callee(0, 1.5)` —— 一个整数 + 一个浮点（用掉 XMM0）。
    let sig_m = FunctionSignature::new(&[], &[TypeId::I64])
        .with_calling_convention(CallConvId::builtin(ConvName::SysV64));
    let mut bm = FunctionBuilder::new("caller", TypeContext::new(), sig_m);
    bm.create_block_here();
    let fmt = bm.iconst_i64(0);
    let x = bm.fconst(1.5f64.to_bits(), TypeId::F64);
    let r = bm.call(callee, &[fmt, x], &[TypeId::I64]);
    bm.ret(&[r[0]]);
    let caller = m.add_function(bm.finish().expect("caller"));

    FunctionCompiler::for_module(TargetMachine::new(), &m)
        .compile_raw(m.get_function(caller))
        .expect("调用点应当能编")
        .code
}

/// ① 调**变参**函数且用了一个 XMM ⇒ 必须写 `%al = 1`（`mov rax, 1`）。
#[test]
fn variadic_call_reports_the_vector_register_count() {
    let code = caller_calling(true);
    assert!(
        has_mov_rax_imm(&code, 1),
        "变参调用点要写 `%al = 用掉的向量寄存器数`（这里 1）：{:02x?}",
        code
    );
}

/// ② 被调方**不是**变参 ⇒ 不写（非变参函数不读 `%al`）；同一段代码因此在两种情形下分叉，
/// 证明这个数来自"被调方是变参"这条运行期信息，而不是无条件的常量。
#[test]
fn non_variadic_call_leaves_the_meta_register_alone() {
    let code = caller_calling(false);
    assert!(
        !has_mov_rax_imm(&code, 1),
        "非变参调用点不该写 `%al`：{:02x?}",
        code
    );
}
