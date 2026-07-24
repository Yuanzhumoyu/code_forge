//! Assembly integration tests — roundtrip, multi-instruction encoding + disassembly,
//! and end-to-end compiler output inspection.

use codegen_lib::FunctionCompiler;
use codegen_lib::backend::x86_64::{X86Inst as Inst, X86Isa as Isa, ensure_registered};
use codegen_lib::ir::*;
use codegen_lib::prelude::*;

fn r(n: u32) -> VReg {
    VReg(n)
}

// ═══════════════════════════════════════════════════
// Roundtrip: Encode → Disassemble consistency
// ═══════════════════════════════════════════════════

#[test]
fn test_roundtrip_encode_disasm_ret() {
    let inst = Inst::Ret;
    let bytes = Isa::encode(&inst).unwrap();
    let disasm = Isa::disassemble(&inst);
    assert!(!bytes.is_empty());
    assert_eq!(disasm, "ret");
}

#[test]
fn test_roundtrip_all_push_regs() {
    let expected_names = [
        "RAX","RCX","RDX","RBX","RSP","RBP","RSI","RDI",
        "R8","R9","R10","R11","R12","R13","R14","R15",
    ];
    for (i, &name) in expected_names.iter().enumerate() {
        let inst = Inst::PushReg { reg: r(i as u32) };
        let bytes = Isa::encode(&inst).unwrap();
        let disasm = Isa::disassemble(&inst);
        assert_eq!(disasm, format!("push {}", name), "push reg {} mismatch", i);
        // Low regs: 0x50-0x57; high regs: REX 0x41 prefix
        let first_byte_ok = (0x50..=0x57).contains(&bytes[0]) || bytes[0] == 0x41;
        assert!(first_byte_ok, "push reg {} unexpected first byte: {:02x}", i, bytes[0]);
    }
}

#[test]
fn test_roundtrip_all_arith_instructions() {
    let cases: Vec<(&str, Inst)> = vec![
        ("add RAX, RCX", Inst::AddRmR { opsize: 64u8, dest: r(0), src: r(1) }),
        ("sub RDX, RBX", Inst::SubRmR { opsize: 64u8, dest: r(2), src: r(3) }),
        ("xor RSI, RDI", Inst::XorRmR { opsize: 64u8, dest: r(6), src: r(7) }),
        ("cmp RSP, RBP", Inst::CmpRmR { opsize: 64u8,  src1: r(4), src2: r(5) }),
        ("test RAX, RDX", Inst::TestRmR { opsize: 64u8,  dest: r(0), src: r(2) }),
    ];
    for (expected_asm, inst) in &cases {
        let bytes = Isa::encode(inst).unwrap();
        let disasm = Isa::disassemble(inst);
        assert!(!bytes.is_empty());
        assert_eq!(&disasm, expected_asm, "disasm mismatch");
    }
}

#[test]
fn test_roundtrip_all_jcc_conds() {
    let conds: &[u8] = &[0x84, 0x85, 0x8C, 0x8E, 0x8F, 0x8D, 0x82, 0x86, 0x87, 0x83];
    let names: &[&str] = &["e","ne","l","le","g","ge","b","be","a","ae"];
    for (&cond, &name) in conds.iter().zip(names.iter()) {
        let inst = Inst::JccRel32 { cond, rel: 42 };
        let bytes = Isa::encode(&inst).unwrap();
        let disasm = Isa::disassemble(&inst);
        assert!(!bytes.is_empty());
        assert_eq!(disasm, format!("j{} .L42", name));
    }
}

// ═══════════════════════════════════════════════════
// Multi-instruction sequences
// ═══════════════════════════════════════════════════

#[test]
fn test_multi_inst_prologue_pattern() {
    let seq = &[
        Inst::PushReg { reg: r(5) },                    // push RBP
        Inst::MovRRm { opsize: 64u8, dest: r(5), src: r(4) },        // mov RBP, RSP
        Inst::PushReg { reg: r(3) },                     // push RBX
        Inst::PushReg { reg: r(12) },                    // push R12
    ];
    let mut all_bytes = Vec::new();
    let mut all_disasm = Vec::new();
    for inst in seq {
        all_bytes.extend(Isa::encode(inst).unwrap());
        all_disasm.push(Isa::disassemble(inst));
    }
    assert_eq!(all_disasm[0], "push RBP");
    assert_eq!(all_disasm[1], "mov RBP, RSP");
    assert_eq!(all_disasm[2], "push RBX");
    assert_eq!(all_disasm[3], "push R12");
    assert_eq!(all_bytes[0], 0x55); // push rbp
}

#[test]
fn test_multi_inst_epilogue_pattern() {
    let seq = &[
        Inst::PopReg { reg: r(15) },   // pop R15
        Inst::PopReg { reg: r(14) },   // pop R14
        Inst::PopReg { reg: r(5) },    // pop RBP
        Inst::Ret,
    ];
    let mut all_bytes = Vec::new();
    for inst in seq {
        all_bytes.extend(Isa::encode(inst).unwrap());
    }
    // R15→0x41,0x5F; R14→0x41,0x5E; RBP→0x5D; RET→0xC3
    assert_eq!(&all_bytes, &[0x41, 0x5F, 0x41, 0x5E, 0x5D, 0xC3]);
}

#[test]
fn test_multi_inst_compare_and_branch() {
    let seq = &[
        Inst::CmpRmR { opsize: 64u8,  src1: r(0), src2: r(2) },       // cmp RAX, RDX
        Inst::JccRel32 { cond: 0x94, rel: 0 },             // je .L0
    ];
    let bytes0 = Isa::encode(&seq[0]).unwrap();
    let bytes1 = Isa::encode(&seq[1]).unwrap();
    // cmp rax, rdx: REX.W + 0x39 0xD0 (opsize=64)
    assert_eq!(&bytes0, &[0x48, 0x39, 0xD0]);
    // je: 0x0F 0x94 + 4 bytes rel32
    assert_eq!(bytes1[0], 0x0F);
    assert_eq!(bytes1[1], 0x94);
    assert_eq!(bytes1.len(), 6);
}

// ═══════════════════════════════════════════════════
// JIT + code inspection tests
// ═══════════════════════════════════════════════════

#[test]
fn test_compile_simple_add_inspect_code() {
    ensure_registered();
    let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
    let mut b = FunctionBuilder::new("add", sig);
    let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
    b.switch_to_block(entry);
    let sum = b.iadd(params[0], params[1]);
    b.return_(&[sum]);
    let func = b.finish();

    let compiled = FunctionCompiler::<Isa>::compile_raw(&func).unwrap();
    let code = &compiled.code;

    eprintln!("simple_add code ({} bytes): {:02x?}", code.len(), code);
    assert!(code.len() > 5, "too short: {} bytes", code.len());
    assert_eq!(code[code.len() - 1], 0xC3, "function should end with RET");
}

#[test]
fn test_compile_return_constant_inspect_code() {
    ensure_registered();
    let sig = Signature::new(&[], &[Type::I32]);
    let mut b = FunctionBuilder::new("ret42", sig);
    b.create_block_here();
    let v = b.iconst_i32(42);
    b.return_(&[v]);
    let func = b.finish();

    let compiled = FunctionCompiler::<Isa>::compile_raw(&func).unwrap();
    let code = &compiled.code;

    eprintln!("return_constant code ({} bytes): {:02x?}", code.len(), code);
    assert!(!code.is_empty());
    assert_eq!(code[code.len() - 1], 0xC3, "function should end with RET");
}

#[test]
fn test_compile_conditional_inspect_code() {
    ensure_registered();
    let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
    let mut b = FunctionBuilder::new("abs_like", sig);
    let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
    b.switch_to_block(entry);
    let zero = b.iconst_i32(0);
    let cond = b.icmp(IntCC::SignedGreaterThanOrEqual, params[0], zero);
    let then_b = b.create_block();
    let else_b = b.create_block();
    b.branch(cond, then_b, else_b, &[], &[]);
    // then: return x
    b.switch_to_block(then_b);
    b.return_(&[params[0]]);
    // else: return -x
    b.switch_to_block(else_b);
    let neg = b.isub(zero, params[0]);
    b.return_(&[neg]);
    let func = b.finish();

    let compiled = FunctionCompiler::<Isa>::compile_raw(&func).unwrap();
    let code = &compiled.code;

    eprintln!("abs_like code ({} bytes): {:02x?}", code.len(), code);
    assert!(code.len() > 10, "conditional function too short");
    assert_eq!(code[code.len() - 1], 0xC3, "function should end with RET");
    // Should contain TEST or CMP opcode somewhere
    let has_test_or_cmp = code.windows(2).any(|w| w[0] == 0x85 || w[0] == 0x39);
    assert!(has_test_or_cmp, "conditional should have TEST or CMP: {:02x?}", code);
}

// ═══════════════════════════════════════════════════
// Deterministic encoding + stress
// ═══════════════════════════════════════════════════

#[test]
fn test_deterministic_encoding_large_sequence() {
    // 200 MOV instructions — verify deterministic encoding
    let mut seq = Vec::new();
    for i in 0..100u32 {
        seq.push(Inst::MovRRm { opsize: 64u8, dest: r(i % 16), src: r((i + 1) % 16) });
    }
    let bytes1: Vec<u8> = seq.iter()
        .flat_map(|inst| Isa::encode(inst).unwrap())
        .collect();
    let bytes2: Vec<u8> = seq.iter()
        .flat_map(|inst| Isa::encode(inst).unwrap())
        .collect();
    assert_eq!(bytes1, bytes2, "encoding should be deterministic");
    assert!(bytes1.len() > 200, "should produce substantial output");
}

#[test]
fn test_stress_encode_disasm_200_instructions() {
    for i in 0..100u32 {
        let inst = Inst::PushReg { reg: r(i % 16) };
        assert!(Isa::encode(&inst).is_ok(), "encode push failed at {}", i);
        let d = Isa::disassemble(&inst);
        assert!(!d.is_empty(), "disasm push failed at {}", i);

        let inst = Inst::PopReg { reg: r(i % 16) };
        assert!(Isa::encode(&inst).is_ok(), "encode pop failed at {}", i);
        let d = Isa::disassemble(&inst);
        assert!(!d.is_empty(), "disasm pop failed at {}", i);
    }
}

#[test]
fn test_disassemble_with_bytes_all_formats() {
    let inst = Inst::Ret;
    let output = Isa::disassemble_with_bytes(&inst, &[0xC3]);
    assert!(output.contains("C3") && output.contains("ret"),
        "disassemble_with_bytes: {}", output);
}

#[test]
fn test_disassemble_full_all_formats() {
    let inst = Inst::AddRmR { opsize: 64u8, dest: r(0), src: r(1) };
    let bytes = Isa::encode(&inst).unwrap();
    let output = Isa::disassemble_full(&inst, 0x4000, &bytes);
    assert!(output.contains("4000"), "should contain address: {}", output);
    assert!(output.contains("add"), "should contain mnemonic: {}", output);
    assert!(output.contains("RAX"), "should contain reg name: {}", output);
}
