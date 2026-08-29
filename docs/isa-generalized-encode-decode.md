# Generalizing the Assembler & Decoder (Architecture-Agnostic Encode/Decode)

## TL;DR

The current `forge-dsl` v12 generator works but is **architecture-specialized**:

- **Decoder** is a **linear if-chain** in declaration order (O(N) over all instructions,
  worst-case `16` x86 worst case), with x86-specific shapes (`ModRM` mod=11 / ext / rr_rev /
  rr_src2, VEX `C4`, `+r` opcodes, prefix scanning `66/F2/F3/67/REX`, SIB/disp, `force_disp_base`)
  hard-coded in the generator (`v12/codegen/mod.rs`).
- **Assembler** is a per-mnemonic `if` block that matches operands by **length-delimited
  literal scanning** of the asm template — a string-splitting heuristic, not a grammar or a
  trie; and the existing `crates/frontend/forge-dsl/src/assembler` lexer is **dead code**,
  never wired into the generator.

The redesign replaces these with **data-driven tables + a shared byte-prefix trie** used by
BOTH the assembler and the decoder. The encoding of every instruction is a declarative
`InstEnc` record; from that record we derive:

1. a **decode decision tree** (a trie over the constant prefix bytes, with bit-masks for
   `+r`/condition-code low bits — this is the industry-standard shape),
2. an **assemble trie** (triee over the mnemonic, then over operand shapes),
3. a **disassemble** template + a **symmetric encoder**.

This is the direction taken by LLVM TableGen (`DecoderEmitter`), Ghidra SLEIGH, and
Capstone/iced-x86: a small number of **architecture-neutral structures** plus a per-ISA
"operand reader" describing how to place/extract register/immediate/memory fields. The
design is grounded in:

- [LLVM TableGen `DecoderEmitter.cpp` — decode decision trees](https://github.com/jurahul/llvm-project/blob/35313d0a40da0be55ddb35c092794268aa3320c6/llvm/utils/TableGen/DecoderEmitter.cpp)
- [Ghidra SLEIGH language spec — pattern-based instruction encoding](https://ghidra.re/ghidra_docs/languages/html/sleigh.html)
- [Capstone x86 disassembler decoder](https://android.googlesource.com/platform/external/capstone/+/refs/tags/android-s-beta-3/arch/X86/X86Disassembler.h)
- [mesa ISASPEC — XML ISA specification & codegen](https://docs.mesa3d.org/isaspec.html)

---

## 1. Diagnosis of the current model

### 1.1 Decoder: linear if-chain

`v12/codegen/mod.rs` `gen_vlen_decode` emits one `if` arm per instruction:

```rust
pub fn decode(bytes: &[u8]) -> Option<(Inst, usize)> {
    // prefix scan (66/F0/F2/F3/67/REX) ...
    if __o + N <= bytes.len() && bytes[__o] == 0x0F && bytes[__o+1] == 0xC1 { ... }
    if __o + M <= bytes.len() && bytes[__o] == 0x8B { ... }
    // → O(N) worst case, declaration-order first-match
}
```

Problems:

- **O(N)**: a linear scan over every instruction; no shared-prefix shortening. Decoding a
  stream of N instructions is O(N·|ISA|).
- **Hard-coded x86**: `ModrmKind`, VEX, `+r`, prefix scanning, SIB/disp, `force_disp_base`.
  Any other ISA (or a custom architecture) requires new generator arms.
- **First-match by declaration order**: correct but fragile — correctness depends on the TOML
  author ordering aliases correctly. A trie makes this a tree property.

### 1.2 Assembler: literal-scan heuristics

`gen_assemble` groups by mnemonic, then for each form splits the operand text by the *next
literal segment* of the asm template:

```rust
if __rest[__pos..].starts_with("mov ") ...
let __text0 = &__rest[__pos..__end];   // split at next literal
```

Problems:

- **String-splitting, not lexing**: `add RAX, 42` is matched by finding the literal `", "`.
  It cannot handle whitespace-insensitive operands, mnemonic aliases shared across forms, or
  expressions without brittle delimiter tricks.
- **Per-mnemonic `if`**: O(mnemonics) again.
- **Dead lexer**: `crates/frontend/forge-dsl/src/assembler` (a Logos tokenizer for the
  asm-template interpolation) is never called from codegen.

### 1.3 The good parts to keep

- **Fixpoint**: `encode` is correct and byte-exact (golden tests green after the validation
  bug fix). We keep the encode semantics; only its *structure* is re-derived from data.
- **Declarative TOML** already describes forms via semantic keys (`modrm`, `vex`, `prefix`,
  `opsize`, `imm`, `escape`, `opcode_reg`). The generalization is to treat these as **data** a
  trie can be built from, not as cases in a generator `match`.

---

## 2. The architecture we move to

### 2.1 The universal `InstEnc` record

Every instruction is normalized into:

```rust
struct InstEnc {
    id: u32,
    /// The constant prefix that keys the decode trie. The *last* entry may carry a
    /// mask (condition-code low nibble: 0xF0; `+r` low 3 bits: 0xF8). Entries are
    /// byte-positional but the trie only needs them in order.
    const_prefix: Vec<ByteKey>,          // (byte_pos, mask, value)
    /// Per-operand field description, in decode order.
    operands: Vec<OpField>,
    /// Length model: fixed, or variable (ModRM tail + trailing immediate).
    len: LenModel,
    /// Disassemble / assemble template (mnemonic + operand shape).
    asm: AsmTemplate,
}

enum ByteKey { Exact(usize, u8), Masked(usize, u8, u8) }   // pos, value[, mask]

enum OpField {
    Reg { width: u8, class: RegClass },         // register index
    Imm { width: u8, signed: bool },            // immediate / label
    Cond { width: u8 },                          // condition code
    Mem { base_width: u8, indexable: bool, scale: u8, disp_width: u8 }, // base+index*scale+disp
}

enum LenModel {
    Fixed { bytes: u8 },
    Vlen { modrm: Option<ModrmModel>, imm_bytes: u8, has_sib: bool, disp: DispModel },
}
```

The key generalization: `const_prefix` is **any ordered list of constant bytes** — an `InstEnc`
is agnostic to whether the ISA is fixed-width (RISC-V: constant opcode/funct bytes within a
32-bit word) or variable-length (x86: prefix/escape/opcode bytes). Only the *operand readers*
& *length model* differ per ISA.

### 2.2 The shared decode/assemble trie

Build **one trie** over `const_prefix` (the constant bytes). Walking it consumes those bytes
and lands on a leaf that carries the instruction's `operands`/`len`/`asm`. Everything after the
constant prefix is handled by the per-ISA **operand reader** (a single strategy, not per-instruction
arms).

```text
root ── 0x0F ── 0x38 ── 0xC0 ──(leaf: PMULLD)     // multi-byte opcode
     ├─ 0x0F ── 0x51 ──(leaf: SQRTSD/SQRTSS)      // prefix decided later
     ├─ 0x8B ──(leaf: MOV_R_RM, subset of ModRM)
     ├─ 0x40..0x4F (mask 0xF0) ──(cond leaves)    // +r / cond-code
     ...
```

This is exactly the byte-trie the request describes ("前缀树... 变长/定长"), and it is the same
structure LLVM's `DecoderEmitter` emits (a decision tree on `(bitpos, mask)`). Because the tree
stores `(mask, value)` at edges, **one trie handles fixed-width (all bytes constant except
operand fields) and variable-length (constant prefix + tail) uniformly**.

**Assembler side**: the *same* byte-trie serves the assembler, because async the request notes,
"根据指令编码规则，通过各个分支节点便可以推断下一路径的结果" — the fixed parts of the
encoding tell you which mnemonic/operand shapes are still live as you parse. Concretely, the
assembler uses a **separate trie keyed on the mnemonic string**, then refines by operand shape.
Both tries are generated from the same `InstEnc` table, so they never disagree.

### 2.3 Per-ISA operand reader (the only arch-specific part)

Variable-length x86/MIPS need tail logic; fixed-width RISC-V does not. Rather than scatter this
across generator arms, isolate it behind a small trait:

```rust
trait IsaEncoding {
    /// From `prefix_bytes` (the already-matched constant prefix) + `raw_rest`,
    /// extract the operand values and the consumed length.
    fn read_tail(&self, enc: &InstEnc, raw: &[u8]) -> Option<(Vec<OpValue>, usize)>;
    /// Inverse: given operand values, emit the tail bytes.
    fn write_tail(&self, enc: &InstEnc, vals: &[OpValue]) -> Vec<u8>;
}
```

For x86, one implementation handles ModRM/SIB/disp/REX/VEX (the current logic, re-homed as a
`write_tail`/`read_tail` pair). For RISC-V, a trivial 32-bit field layout implementation. New
custom architectures implement only this trait — the trie, assembler, and disassembler are
shared and unchanged. That is the "更通用" (more general) goal, achieved by **removing x86
knowledge from the generic machinery**, not by writing another x86 generator.

---

## 3. Migration plan (phased, with test gates)

Repos are green at: `cargo test -p forge-dsl` and `cargo test -p forge-codegen --test x86_v12_tests`.
Each phase must keep those green plus `decoder_smoke`/`v12_integration`.

- **Phase A (done): fix the inverted `validate_operand_slots` reg-class check** — a real
  bug that prevented `x86_v12.toml`/`riscv64_v12.toml` from compiling at HEAD.
- **Phase B (done): introduce the `enc` core** — `InstEnc` IR + generic `build_decode_trie` /
  `decode` + `build_assemble_trie` / `assemble` + symmetric `encode`, unit-tested on a
  fixed-width ISA, a variable-length opcode-family+memory ISA, and a `+r`/cond-in-opcode ISA
  — all through the shared trie (`encode`/`decode`/`assemble` round-trips).
- **Phase C (done for the variable-length decoder): `gen_vlen_decode` now emits a byte-prefix
  decision tree** (a trie over the constant escape/opcode/modrm_fixed bytes, with masks for
  `+r`/condition-code low bits, and opcode-family leaves resolved by the per-arm tail guard in
  declaration order) instead of a linear `if`-chain. Verified byte-identical against the golden
  spec/decode/round-trip tests (`x86_v12_tests`, `decoder_smoke`).
- **Phase D (next): replace `gen_assemble`'s literal-scan with a mnemonic trie + a real lexer**
  (re-homing the `assembler` lexer, or a table-driven one). Gate: `assemble_disassemble_roundtrip`.
- **Phase E (next): extract x86 tail logic into `IsaEncoding` (read_tail/write_tail)** so the
  generic machinery has no x86 names; add RISC-V as a second `IsaEncoding`. Gate: `riscv64_v12_tests`.
- **Phase F (next): a new custom-ISA smoke ISA** to prove a third architecture needs no
  generator change.

---

## 4. Acceptance criteria

- One code path decodes & assembles fixed-width and variable-length instructions.
- No x86 identifiers (ModRM/REX/VEX/+r) appear in the generic engine; they live in the ISA's
  `IsaEncoding` impl.
- Decode is trie-guided (sub-linear in the common case, shared-prefix shortened).
- Byte-exact parity preserved: all existing golden/spec/roundtrip tests pass unchanged,
  and the architecture-agnostic JIT matrix (182 cases) stays green.
