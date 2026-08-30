//! P1-18：编码器/解码器 fuzz——确定性伪随机字节流鲁棒性 + 截断鲁棒性 +
//! 结构往返（decode(encode(decode(x))) == decode(x)）+ 定宽字节级回环。
//!
//! # 性质
//!
//! 1. **鲁棒性**：任意字节流（含空/过短/超长）decode 不 panic，
//!    且 `Ok` 的 `consumed ≤ bytes.len()`（不得越界消费）；
//! 2. **截断鲁棒性**：真实编码的任意前缀 decode 不 panic
//!    （变长 ISA 的 imm/ModRM 分片落在缓冲边界外必须优雅失败）；
//! 3. **结构往返**：`decode(encode(decode(x))) == decode(x)`——编码器
//!    确定性 + 解码器无损的硬契约（生成期注释同款契约）；
//! 4. **字节级回环**（定宽）：`encode(decode(x)) == x` 字节精确。
//!
//! 种子固定（可复现）；迭代量级 10^4，位决策树/前缀扫描均为 O(1)-O(4)。

use forge_codegen::machine::decoder::TargetDecoder;

// ─────────────────────────── 确定性 PRNG ───────────────────────────

/// xorshift64——固定种子 → 失败可复现。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// 随机字节缓冲（长度 0..=max_len）。
    fn random_bytes(&mut self, max_len: usize) -> Vec<u8> {
        let len = (self.next() as usize) % (max_len + 1);
        (0..len).map(|_| self.next() as u8).collect()
    }
}

// ─────────────────────────── 鲁棒性断言 ───────────────────────────

/// 断言单次 decode 鲁棒：不 panic、Ok 的 consumed ≤ len。
fn assert_decode_robust<D: TargetDecoder>(dec: &D, bytes: &[u8]) {
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dec.decode(bytes)));
    match res {
        Ok(Ok((_, consumed))) => assert!(
            consumed <= bytes.len(),
            "consumed {} > len {} for {:02x?}",
            consumed,
            bytes.len(),
            bytes
        ),
        Ok(Err(_)) => {}
        Err(_) => panic!("decode panicked on {:02x?}", bytes),
    }
}

/// 对一束编码逐字节前缀断言截断鲁棒。
fn assert_truncation_robust<D: TargetDecoder>(dec: &D, samples: &[&[u8]]) {
    for sample in samples {
        for len in 0..=sample.len() {
            let prefix = &sample[..len];
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                dec.decode(prefix)
            }));
            match res {
                Ok(Ok((_, consumed))) => {
                    assert!(consumed <= prefix.len(), "truncated consumed overrun: {prefix:02x?}")
                }
                Ok(Err(_)) => {}
                Err(_) => panic!("decode panicked on truncated prefix {prefix:02x?}"),
            }
        }
    }
}

// ─────────────────────────── 语料（已知编码）───────────────────────────

/// x86_v12 已知编码语料（REX/前缀/imm/ModRM 各形态；错误猜测无害——
/// decode Err 直接跳过，只对能解码的样本做往返）。
const X86_SAMPLES: &[&[u8]] = &[
    &[0x90],                         // nop
    &[0xC3],                         // ret
    &[0x50],                         // push rax
    &[0x58],                         // pop rax
    &[0x31, 0xC0],                   // xor eax, eax
    &[0x48, 0x31, 0xC0],             // xor rax, rax (REX.W)
    &[0x48, 0x89, 0xC0],             // mov rax, rax (REX.W)
    &[0x48, 0x89, 0xD8],             // mov rax, rbx
    &[0x48, 0x83, 0xC0, 0x01],       // add rax, 1 (imm8)
    &[0x48, 0x05, 0x78, 0x56, 0x34, 0x12], // add rax, 0x12345678 (imm32)
    &[0x48, 0x2B, 0x04, 0x25, 0x78, 0x56, 0x34, 0x12], // sub rax, [0x12345678]
    &[0xEB, 0x00],                   // jmp +0
    &[0xE8, 0x00, 0x00, 0x00, 0x00], // call +0
    &[0x66, 0x66, 0x90],             // 66 66 nop
    &[0x48, 0x8B, 0x44, 0x24, 0x08], // mov rax, [rsp+8]
    &[0x0F, 0x1F, 0x00],             // nop dword ptr [rax]
    &[0x48, 0x0F, 0xAF, 0xC1],       // imul rax, rcx
    &[0x48, 0x39, 0xC8],             // cmp rax, rcx
    &[0x48, 0x01, 0xC8],             // add rax, rcx
    &[0x48, 0x29, 0xC8],             // sub rax, rcx
];

/// riscv64_v12（定宽 32 位）已知编码语料。
const RISCV_SAMPLES: &[&[u8]] = &[
    &[0x13, 0x00, 0x00, 0x00],       // addi x0, x0, 0 (nop)
    &[0x93, 0x80, 0xA0, 0x02],       // addi x1, x0, 42
    &[0x33, 0x00, 0x00, 0x00],       // add x0, x0, x0
    &[0x33, 0x84, 0x00, 0x00],       // add x8, x0, x0
    &[0x83, 0x80, 0x00, 0x00],       // lw x1, 0(x0)
    &[0x23, 0x80, 0x00, 0x00],       // sw x0, 0(x0)
    &[0x63, 0x00, 0x00, 0x00],       // beq x0, x0, 0
    &[0x67, 0x80, 0x00, 0x00],       // jalr x1, 0(x0)
    &[0x6F, 0x00, 0x00, 0x00],       // jal x0, 0
    &[0x37, 0x00, 0x00, 0x00],       // lui x0, 0
];

/// demo_v12（定宽 32 位）已知编码语料。
const DEMO_SAMPLES: &[&[u8]] = &[&[0x10, 0, 0, 0]]; // ADD16

// ─────────────────────────── 测试 ───────────────────────────

/// 性质 1：随机字节流 decode 不 panic、无越界消费。
#[test]
fn fuzz_decode_random_bytes_no_panic() {
    let mut rng = Rng::new(0xF00D_2026);
    let x86 = forge_codegen::x86_v12::Decoder;
    let riscv = forge_codegen::riscv64_v12::Decoder;
    let demo = forge_codegen::demo_v12::Decoder;
    for _ in 0..100_000 {
        let bytes = rng.random_bytes(23);
        assert_decode_robust(&x86, &bytes);
        assert_decode_robust(&riscv, &bytes);
        assert_decode_robust(&demo, &bytes);
    }
    // 长缓冲（跨多条指令）与空缓冲同样鲁棒
    for _ in 0..10_000 {
        let bytes = rng.random_bytes(64);
        assert_decode_robust(&x86, &bytes);
    }
    assert_decode_robust(&x86, &[]);
    assert_decode_robust(&riscv, &[]);
    assert_decode_robust(&demo, &[]);
}

/// 性质 2：真实编码的任意前缀截断 decode 不 panic、无越界消费。
#[test]
fn fuzz_decode_truncated_prefixes_no_panic() {
    let x86 = forge_codegen::x86_v12::Decoder;
    let riscv = forge_codegen::riscv64_v12::Decoder;
    let demo = forge_codegen::demo_v12::Decoder;
    assert_truncation_robust(&x86, X86_SAMPLES);
    assert_truncation_robust(&riscv, RISCV_SAMPLES);
    assert_truncation_robust(&demo, DEMO_SAMPLES);
}

/// 性质 3+4：结构往返 decode(encode(decode(x))) == decode(x)；
/// riscv 定宽进一步断言 encode(decode(x)) == x 字节精确。
#[test]
fn fuzz_roundtrip_decode_encode_decode() {
    let x86 = forge_codegen::x86_v12::Decoder;
    let x86_enc = forge_codegen::x86_v12::Encoder;
    let riscv = forge_codegen::riscv64_v12::Decoder;
    let riscv_enc = forge_codegen::riscv64_v12::Encoder;
    let rm = forge_codegen::AllocResult::new();

    use forge_codegen::machine::encoder::TargetEncoder;

    for sample in X86_SAMPLES {
        let Ok((inst, n)) = x86.decode(sample) else { continue };
        assert_eq!(n, sample.len(), "x86 语料应整段消费: {sample:02x?}");
        let bytes = x86_enc
            .encode_to_bytes(&inst, &rm)
            .unwrap_or_else(|e| panic!("encode(decode(x)) 失败: {e:?} for {sample:02x?}"));
        let (inst2, _) = x86
            .decode(&bytes)
            .unwrap_or_else(|e| panic!("二次 decode 失败: {e:?} for {bytes:02x?}"));
        assert_eq!(
            inst, inst2,
            "decode(encode(decode(x))) != decode(x) for {sample:02x?} (re-encoded {bytes:02x?})"
        );
    }

    for sample in RISCV_SAMPLES {
        let Ok((inst, n)) = riscv.decode(sample) else { continue };
        assert_eq!(n, 4, "riscv 定宽应消费 4 字节: {sample:02x?}");
        let bytes = riscv_enc
            .encode_to_bytes(&inst, &rm)
            .unwrap_or_else(|e| panic!("riscv encode(decode(x)) 失败: {e:?} for {sample:02x?}"));
        assert_eq!(
            &bytes[..],
            *sample,
            "riscv 定宽 encode(decode(x)) 应字节精确回环"
        );
        let (inst2, _) = riscv.decode(&bytes).expect("riscv 二次 decode");
        assert_eq!(inst, inst2, "riscv decode(encode(decode(x))) != decode(x)");
    }
}
