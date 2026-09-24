//! x86-64 v12 验证（迭代 3+：变长语义键）。v11 后端已删除，golden 全部为
//! 硬编码 x86 规范 oracle 字节（v12 组内寄存器索引，无 v11 的 `16+i` 偏差）。
//!
//! **规范字节已迁进谱内 `[[vectors]]`**（v19 V3b）：`isa/x86_v12.toml` 末尾 102 条，
//! 由生成物 `__spec_tests::spec_vector_*` 执行（`cargo test -p forge-codegen --lib`），
//! 也可单跑 `cargo run -p forge-isa -- test isa/x86_v12.toml`。本文件只留集成/往返/
//! ABI 类断言。
//!
//! - **字节级往返**：`decode(encode(X))` 再 encode 字节一致（含编码撞车的别名
//!   指令，声明序首匹配）——**全指令那份在生成物里**（v19 V3d：谱内派生枚举器 +
//!   `crate::isa_roundtrip_guard`），本文件只留"规范指令 decode 回自身"的小清单。
//! - **assemble/disassemble** 往返（opsize 操作数非文本，默认 64）。

use forge_codegen::x86_v12::{Inst, Reg, assemble, decode, disassemble, encode};
use forge_ir::{PhysReg, RegClass};

// ─────────────────── 规范字节：GPR @modrm/@modrm_imm32/内存 ───────────────────

#[test]
fn opsize_prefix_and_rex_w() {
    // v12.1：opsize 由寄存器宽度视图推导——AX→0x66 前缀、EAX→无、RAX→REX.W。
    // from_index_grp 按组名构造视图（gpr16/gpr32/gpr64 共享物理编号 0-15）。
    let cases: &[(RegClass, u32, u32, &[u8])] = &[
        (RegClass::GPR(2), 0, 1, &[0x66, 0x8B, 0xC1]), // movrr ax, cx
        (RegClass::GPR(4), 0, 1, &[0x8B, 0xC1]),       // movrr eax, ecx
        (RegClass::GPR(8), 0, 1, &[0x48, 0x8B, 0xC1]), // movrr RAX, rcx
        (RegClass::GPR(8), 8, 9, &[0x4D, 0x8B, 0xC1]), // movrr R8, r9（REX.W+R+B）
        (RegClass::GPR(8), 1, 8, &[0x49, 0x8B, 0xC8]), // movrr rcx, r8（REX.W+B）
    ];
    for (gpr, dest, src, expected) in cases {
        let v12b = encode(&Inst::MovRRm {
            dst: Reg::from_index(*dest, *gpr),
            src: Reg::from_index(*src, *gpr),
        })
        .unwrap();
        assert_eq!(
            v12b.as_slice(),
            *expected,
            "grp: {gpr} dest: {dest} src: {src}"
        );
    }
}

// ─────────────────── `+r` 形式（push/pop/bswap/mov_imm64，迭代 5）───────────────────

// ─────────────────── 控制流（JMP/CALL/RET，迭代 6）───────────────────

#[test]
fn control_flow_roundtrip() {
    use Inst::*;
    let insts = vec![
        Ret,
        JmpRel32 { target: 0 },
        JmpRel32 { target: 42 },
        JmpRel32 { target: -1 },
        CallRipRel { target: 0 },
        CallRipRel { target: 0x1234 },
    ];
    for inst in insts {
        let b = encode(&inst).unwrap();
        let (dec, n) = decode(&b).unwrap();
        assert_eq!(n, b.len(), "{inst:?}: 解码消费 {n} != {} 字节", b.len());
        let b2 = encode(&dec).unwrap();
        assert_eq!(b2, b, "{inst:?}: decode→encode 往返字节不一致");
    }
}

// ─────────────────── SSE 规范字节（v11 的 16+i 索引不合规）───────────────────

// ─────────────────── 内存寻址规范字节（@modrm_mem，迭代 3b）───────────────────

// ─────────────────── 家族（SSE）与 VEX 规范字节（迭代 4）───────────────────

// ─────────────────── 字节级 decode 往返 ───────────────────

// **全指令往返已迁进生成物**（v19 V3d）：谱内派生枚举器
// `x86_v12::__spec_tests::all_insts()`（每条指令 × 宽度视图 + 立即数边界 + 内存风味）
// 由 `crate::isa_roundtrip_guard::derived_insts_roundtrip_byte_stable` 跑
// `encode → decode → encode` 字节闭环——**不要再在这里手抄 `all_insts()`**：
// 手抄清单在谱加指令时不会自动跟上（这一版曾有 650 行）。本文件只留
// "规范指令 decode 回自身"那部分（别名撞车时本就不成立，需要显式小清单）。

#[test]
fn decode_identity_canonical() {
    // 规范指令（非别名）：decode(encode(X)) == X
    let canonical = [
        Inst::MovRRm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::MovRRm {
            dst: Reg::from_index(8, forge_ir::RegClass::GPR64),
            src: Reg::from_index(9, forge_ir::RegClass::GPR64),
        },
        Inst::MovsxdRRm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::AddRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            dst: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::ImulRRm {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::CmpRmR {
            src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            src2: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::AddRImm32 {
            dst: Reg::from_index(0, forge_ir::RegClass::GPR64),
            imm: 42,
        },
        Inst::CmpRImm32 {
            src: Reg::from_index(8, forge_ir::RegClass::GPR64),
            imm: -1,
        },
        Inst::Sqrtsd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Inst::Cvtsi2sd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::GPR64),
        },
        Inst::Andpd {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Inst::Comiss {
            src: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src2: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Inst::Movss {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
        Inst::Punpcklqdq {
            dst: Reg::from_index(0, forge_ir::RegClass::FPR(16)),
            src: Reg::from_index(1, forge_ir::RegClass::FPR(16)),
        },
    ];
    for inst in canonical {
        let b = encode(&inst).unwrap();
        let (dec, _) = decode(&b).unwrap();
        assert_eq!(dec, inst, "{inst:?}: 解码未还原（{b:02x?}）");
    }
}

// ─────────────────── assemble/disassemble 往返 ───────────────────

#[test]
fn disassemble_known_texts() {
    assert_eq!(
        disassemble(&assemble("mov RAX, RBX").unwrap()),
        "mov RAX, RBX"
    );
    assert_eq!(
        disassemble(&assemble("add RAX, 42").unwrap()),
        "add RAX, 42"
    );
    assert_eq!(
        disassemble(&assemble("sqrtsd XMM0, XMM1").unwrap()),
        "sqrtsd XMM0, XMM1"
    );
}

/// 类表 = **ISA 声明**（2026-09-13 去「编造 fallback 类表」）：
/// x86 的类表必须只含"已声明族/类型宽度/tier"，且**不得**出现未声明的类
/// （历史 `fallback_classes` 会凭空造出 GPR(2)/FPR(4)/VEC(16)… 并借池）。
#[test]
fn class_table_is_declared_isa_data() {
    let tm = forge_codegen::x86_v12::TargetMachine::new();
    let classes: Vec<RegClass> = forge_codegen::TargetMachine::reg_info(&tm)
        .register_classes()
        .iter()
        .map(|c| c.reg_class)
        .collect();
    for want in [
        RegClass::GPR(1),
        RegClass::GPR(2),
        RegClass::GPR(4),
        RegClass::GPR(8),
        RegClass::FPR(8),
        RegClass::FPR(16),
        RegClass::FPR(32),
        RegClass::VEC(16),
        RegClass::VEC(32),
        RegClass::VEC(64),
    ] {
        assert!(classes.contains(&want), "类表缺少 {want:?}：{classes:?}");
    }
    // `fpr4` 未声明（x86 只有 fpr8/fpr16/fpr32）→ 不得凭空出现。
    assert!(
        !classes.contains(&RegClass::FPR(4)),
        "未声明的 FPR(4) 不得出现：{classes:?}"
    );
    // 每个类都必须有非空可分配池（否则 regalloc 会在该类的值上死循环/报错）。
    for c in forge_codegen::TargetMachine::reg_info(&tm).register_classes() {
        assert!(
            !c.allocatable.is_empty(),
            "类 {}（{:?}）可分配池为空",
            c.name,
            c.reg_class
        );
    }
    // VEC(64)（V512）池必须来自浮点文件（x86 的 ZMM 与 XMM 同编号空间）。
    let v512 = forge_codegen::TargetMachine::reg_info(&tm)
        .register_classes()
        .into_iter()
        .find(|c| c.reg_class == RegClass::VEC(64))
        .expect("VEC(64)");
    assert_eq!(v512.width, 64, "VEC(64).width 必须是 64（spill 槽按值宽）");
    assert!(v512.allocatable.contains(&15), "池应含 XMM15：{v512:?}");
}
