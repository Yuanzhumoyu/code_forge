//! 编译前置能力门（v30 M2：从 `compiler::compile_stages` 外提）。
//!
//! 这些检查都在**任何 IR 改写 / lowering 之前**、对原始函数做判断，失败一律
//! fail-closed（`IrError::capability`，进 `backend-gaps.md` 台账）：
//!
//! - 值池门：函数里出现的每个值类型都必须能被该 ISA 的寄存器类承载；
//! - 宽向量门：`>16B` 的参数/返回要有 ABI 落点与宿主档位，IR 层只放行 32/64B；
//! - 异常门：`landingpad` / `invoke` / `resume` 没有机器指令映射。
//!
//! 只读、无副作用；错误文本与阶段判定（`Stage::Legalize`）跟外提前逐字一致。

use crate::IrError;
use crate::machine::target::TargetMachine;
use forge_ir::{Function, Opcode, TermKind, TypeId};

/// 跑完所有前置门；第一条失败即返回。
pub(crate) fn run<M: TargetMachine>(machine: &M, func: &Function) -> Result<(), IrError> {
    check_value_pools(machine, func)?;

    // 宽向量参数/返回（>16 字节）ABI：
    // - 若 ISA 声明 `vector by-ref limit`（如 x86 limit=128 位）→ 宽向量按
    //   引用传参（调用方栈拷贝 + 传指针 GPR；被调方入口从 [ptr] 加载）。
    // - 否则拒绝（ABI 仅寄存器传值，宽向量会静默截断）。
    let by_ref_limit = machine.abi().vector_by_ref_limit();
    // **一次快照**（v3 S3：`TypeStoreRef` 是 owned，不与下面 `func` 的借用冲突）——
    // 原来每个参数/返回值/每个指令类型各 `borrow()` 一次（取锁次数 ∝ IR 规模）。
    let types = func.types.borrow();
    let param_tys = func.param_types();
    let return_tys = func.return_types();
    let wide = param_tys
        .iter()
        .chain(&return_tys)
        .find(|t| {
            (types.is_vector(**t) || types.is_scalable_vector(**t)) && types.size_bytes(**t) > 16
        })
        .copied();
    if let Some(ty) = wide {
        let bytes = types.size_bytes(ty);
        // S4：超过 16 字节的定宽向量要**本机能编能跑**才放行（V512 = EVEX 前提）——
        // 判据来自 ISA 后端（`TargetRegInfo::wide_vector_bytes` = 谱申报档位 ∩ 宿主上界），
        // 通用管线**不认识** `avx512` 这类能力名（v23 M3）。
        let wide_caps = machine.reg_info().wide_vector_bytes();
        if bytes > 16 && !wide_caps.contains(&(bytes as u16)) {
            return Err(IrError::capability(
                "SIMD_WIDE_VECTOR_TIER",
                format!(
                    "SIMD 参数/返回 {bytes} 字节宽向量：本 ISA/宿主的宽向量档位是 {wide_caps:?}\
                     （type {ty:?}）"
                ),
            ));
        }
        match by_ref_limit {
            Some(lim) if bytes <= lim => {
                // 在寄存器传值范围内（≤16 字节）：走现有 XMM/GPR 传参路径。
            }
            Some(lim) => {
                // 超过阈值：by-ref 传参（调用方栈拷贝 + 传指针；被调方入口
                // 从 [ptr] 加载）。入口守卫通过——move_args/call lowering
                // 的 by-ref 路径处理。V512（64 字节）等超宽向量若 ISA 未
                // 声明足够 limit 时由 encode 侧能力决定（avx512 检测）。
                let _ = lim;
            }
            None => {
                return Err(IrError::capability(
                    "SIMD_VECTOR_BYREF",
                    format!(
                        "SIMD 参数/返回 {bytes} 字节暂不支持（ABI 未声明 vector by-ref；type {ty:?}）"
                    ),
                ));
            }
        }
    }
    // IR 层宽向量（>16B）能力门（W3，2026-09-12）：
    //  - 32B（V256）：VEX.256（如 vmovups ymm）——ISA 已有算术/常量/槽往返
    //    全套规则；与既有 V256 算术路径一致，不在此加硬件门（运行级用例
    //    自行按 AVX 能力 skip）。
    //  - 64B（V512）：EVEX（zmm）——**需 AVX-512F**；无则编译期拒绝
    //    （与上方 ABI 守卫同一判据），避免发射本机无法执行的 EVEX。
    //  - 其它 >16B 宽度（非 32/64）：无 lowering 规则 → 显式拒绝。
    // 旧行为：>16B 向量 Load/Store 一律拒绝（ISA 缺宽槽类 → 曾落到默认
    // 8 字节 MOV 静默截断）；现由 isa/x86.toml 的 rd_vec/rs1_vec
    // = 32/64 规则 + 本条硬件门共同把住。
    let wide_caps = machine.reg_info().wide_vector_bytes();
    for (_, inst) in func.dfg.insts() {
        let tys: Vec<TypeId> = inst
            .results
            .iter()
            .chain(inst.operands.iter())
            .filter_map(|v| func.dfg.value_type(*v))
            .collect();
        for ty in tys {
            if !(types.is_vector(ty) || types.is_scalable_vector(ty)) {
                continue;
            }
            let bytes = types.size_bytes(ty);
            if bytes <= 16 {
                continue;
            }
            if bytes > 16 && !wide_caps.contains(&(bytes as u16)) {
                return Err(IrError::capability(
                    "VECTOR_TIER_UNSUPPORTED",
                    format!(
                        "IR 层 {bytes} 字节宽向量：本 ISA/宿主的宽向量档位是 {wide_caps:?}\
                         （type {ty:?}）"
                    ),
                ));
            }
            if bytes != 32 && bytes != 64 {
                return Err(IrError::capability(
                    "VECTOR_BYTES_LOWERING",
                    format!(
                        "IR {bytes} 字节向量暂不支持（仅 V256 32B / V512 64B 有 lowering \
                         规则；type {ty:?}）"
                    ),
                ));
            }
        }
    }
    // 聚合参数/返回 >8 字节：由 expand_agg_call_args / expand_large_agg_params /
    // expand_large_agg_ret 处理（≤16 字节拆段；>16 字节在 pass 内拒绝）。
    // 异常处理（P1.1）：invoke/landingpad/resume 无机器指令映射，编译期拒绝
    for (_, inst) in func.dfg.insts() {
        if matches!(inst.opcode, Opcode::LandingPad) {
            return Err(IrError::capability(
                "EXCEPTION_LANDINGPAD",
                "异常处理 landingpad 无机器指令映射（P1.1 仅文本层解析/展示）".to_string(),
            ));
        }
    }
    for (b, _bd) in func.dfg.blocks() {
        if matches!(
            func.dfg.term_kind(b),
            Some(TermKind::Invoke | TermKind::Resume)
        ) {
            return Err(IrError::capability(
                "EXCEPTION_INVOKE_RESUME",
                "异常处理 invoke/resume 无机器指令映射（P1.1 仅文本层解析/展示）".to_string(),
            ));
        }
    }
    Ok(())
}

/// 值池门：函数内每个值类型都必须能被 ISA 承载
/// （`TargetRegInfo::class_for_type` 返回 `Some`）。否则报 `Unsupported`
/// 并点名类型与值池宽度——**绝不**静默按某个宽度缺省生成寄存器类。
fn check_value_pools<M: TargetMachine>(machine: &M, func: &Function) -> Result<(), IrError> {
    let ri = machine.reg_info();
    let gpr = ri.value_gpr_class();
    let fpr = ri.value_fpr_class();
    for (v, _) in func.dfg.values() {
        let Some(ty) = func.dfg.value_type(v) else {
            continue;
        };
        // 已删除/墓碑值：DCE 之后 `ValueData.ty` 被置为 `TypeId::VOID`
        // （见 `dfg.rs` 的 replace/tombstone 路径），它不承载任何寄存器——
        // 必须跳过，否则 1 字节值池的 ISA 上**任何**被优化掉的死值都会让
        // 整个函数编译失败（`from_type_id(VOID)` 兜底是 GPR(8)）。
        if matches!(ty, TypeId::VOID) {
            continue;
        }
        if ri.class_for_type(ty).is_none() {
            return Err(IrError::capability(
                "VALUE_POOL_TYPE",
                format!(
                    "ISA 值池无法承载类型 {}（{} 位；值池 GPR {} 字节 / FPR {} 字节。\
                     需要 [meta].value_gpr_width / addr_width / value_fpr_width 声明更宽的\
                     寄存器组，或改用更窄的 IR 类型）",
                    crate::machine::reg_info::type_label(ty),
                    ty.builtin_scalar_bits()
                        .map(|b| b.to_string())
                        .unwrap_or_else(|| "非标量".to_string()),
                    gpr.bytes(),
                    fpr.bytes()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_ir::FunctionBuilder;
    use forge_ir::FunctionSignature;
    use forge_ir::InstFlags;
    use forge_ir::TypeContext;

    fn x86() -> crate::arch::x86::TargetMachine {
        crate::arch::x86::TargetMachine::new()
    }

    fn scalar_fn() -> Function {
        let sig = FunctionSignature::new(&[(TypeId::I64, "a")], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("scalar", TypeContext::new(), sig);
        let (blk, p) = b.create_block_with_params(&[(TypeId::I64, "a")]);
        b.switch_to_block(blk);
        let one = b.const_int(1i64, TypeId::I64);
        let sum = b.iadd(p[0], one);
        b.ret(&[sum]);
        b.finish().expect("build scalar")
    }

    #[test]
    fn scalar_function_passes_all_prechecks() {
        run(&x86(), &scalar_fn()).expect("标量函数不应被前置门拒绝");
    }

    #[test]
    fn landingpad_is_reported_as_capability_gap() {
        let sig = FunctionSignature::new(&[], &[TypeId::I64]);
        let mut b = FunctionBuilder::new("lp", TypeContext::new(), sig);
        b.create_block_here();
        let _ = b.emit1(
            Opcode::LandingPad,
            vec![],
            vec![],
            TypeId::I64,
            InstFlags::NONE,
        );
        let zero = b.const_int(0i64, TypeId::I64);
        b.ret(&[zero]);
        let func = b.finish().expect("build landingpad");

        let err = run(&x86(), &func).expect_err("landingpad 必须 fail-closed");
        assert_eq!(err.gap_id().as_deref(), Some("EXCEPTION_LANDINGPAD"));
    }
}
