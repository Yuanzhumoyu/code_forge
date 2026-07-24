"""Migrate ISA TOML files to v11 asm format.

Changes:
1. Add `asm = "..."` field to each [inst.X] after encoding line
2. Convert lowering insts from [{inst="X", args={k="v"}}] to ["X v1, v2, ..."]
"""
import re
import sys
from collections import OrderedDict

# ── asm templates per instruction ──
# x86_64
X86_ASM = {
    "MOV_R8_RM": "mov {dest}, {src}",
    "MOV_RM_R8": "mov {dest}, {src}",
    "MOV_REG_IMM64": "mov {reg}, 0x{imm:x}",
    "ADD_RM8_R8": "add {dest}, {src}",
    "SUB_RM8_R8": "sub {dest}, {src}",
    "IMUL_R64_RM64": "imul {dest}, {src}",
    "NOT_RM": "not {dest}",
    "NEG_RM": "neg {dest}",
    "DIV_RM": "div {src}",
    "IDIV_RM": "idiv {src}",
    "XOR_RM8_R8": "xor {dest}, {src}",
    "AND_RM8_R8": "and {dest}, {src}",
    "OR_RM8_R8": "or {dest}, {src}",
    "SHL_RM_CL": "shl {dest}, cl",
    "SHR_RM_CL": "shr {dest}, cl",
    "SAR_RM_CL": "sar {dest}, cl",
    "CMP_RM8_R8": "cmp {src1}, {src2}",
    "TEST_RM8_R8": "test {dest}, {src}",
    "SETCC_RM8": "set{cond} {dest}",
    "RET": "ret",
    "NOP": "nop",
    "UD2": "ud2",
    "JMP_REL32": "jmp .L{rel}",
    "JCC_REL32": "j{cond} .L{rel}",
    "PUSH_REG": "push {reg}",
    "POP_REG": "pop {reg}",
    "LEA_R64_SIB": "lea {dest}, [{base}+{index}*{scale}+{disp}]",
    "MOVSD_R_R": "movsd {dest}, {src}",
    "SD_FMOV": "movsd {dest}, {src}",
    "ADDSD": "addsd {dest}, {src}",
    "SUBSD": "subsd {dest}, {src}",
    "MULSD": "mulsd {dest}, {src}",
    "DIVSD": "divsd {dest}, {src}",
    "SQRTSD": "sqrtsd {dest}, {src}",
    "CVTSI2SD": "cvtsi2sd {dest}, {src}",
    "CVTSD2SI": "cvtsd2si {dest}, {src}",
    "ANDPD": "andpd {dest}, {src}",
    "XORPD": "xorpd {dest}, {src}",
    "MOVQ_XMM_R64": "movq {dest}, {src}",
    "MOVQ_R64_XMM": "movq {dest}, {src}",
    "COMISD": "comisd {src1}, {src2}",
    "CQO": "cqo",
    "MOVSXD_R64_RM32": "movsxd {dest}, {src}",
}

# AArch64
A64_ASM = {
    "SD_MOV": "mov {dest}, {src}",
    "SD_MOV_IMM": "mov {dest}, #{imm}",
    "SD_ADD": "add {dest}, {src1}, {src2}",
    "SD_SUB": "sub {dest}, {src1}, {src2}",
    "SD_MUL": "mul {dest}, {src1}, {src2}",
    "SD_SDIV": "sdiv {dest}, {src1}, {src2}",
    "SD_UDIV": "udiv {dest}, {src1}, {src2}",
    "SD_AND": "and {dest}, {src1}, {src2}",
    "SD_OR": "orr {dest}, {src1}, {src2}",
    "SD_XOR": "eor {dest}, {src1}, {src2}",
    "SD_SHL": "lsl {dest}, {src1}, {src2}",
    "SD_SHR": "lsr {dest}, {src1}, {src2}",
    "SD_SAR": "asr {dest}, {src1}, {src2}",
    "SD_CMP": "cmp {src1}, {src2}",
    "SD_SETCC": "cset {dest}, {cond}",
    "SD_JMP": "b {rel}",
    "SD_JCC": "b.{cond} {rel}",
    "SD_CALL": "blr {target}",
    "SD_RET": "ret",
    "SD_NOP": "nop",
    "SD_UD2": "brk #0",
    "SD_LOAD": "ldr {dest}, [{src}]",
    "SD_STORE": "str {val}, [{addr}]",
}

# Wasm32
WASM_ASM = {
    "SD_MOV": "local.get {dest} ;; via {src}",
    "SD_MOV_IMM": "i64.const {imm} ;; → {dest}",
    "SD_ADD": "i64.add {dest}, {src}",
    "SD_SUB": "i64.sub {dest}, {src}",
    "SD_MUL": "i64.mul {dest}, {src}",
    "SD_UDIV": "i64.div_u {dest}, {src}",
    "SD_SDIV": "i64.div_s {dest}, {src}",
    "SD_UREM": "i64.rem_u {dest}, {src}",
    "SD_SREM": "i64.rem_s {dest}, {src}",
    "SD_AND": "i64.and {dest}, {src}",
    "SD_OR": "i64.or {dest}, {src}",
    "SD_XOR": "i64.xor {dest}, {src}",
    "SD_NOT": "i64.not {dest}",
    "SD_NEG": "i64.neg {dest}",
    "SD_SHL": "i64.shl {dest}, {src}",
    "SD_SHR": "i64.shr_u {dest}, {src}",
    "SD_SAR": "i64.shr_s {dest}, {src}",
    "SD_CMP": "i64.cmp {src1}, {src2}",
    "SD_SETCC": "i64.set{cond} {dest}",
    "SD_FMOV": "f64.mov {dest}, {src}",
    "SD_FMOV_BITS": "f64.reinterpret_i64 {dest}, {src}",
    "SD_FADD": "f64.add {dest}, {src}",
    "SD_FSUB": "f64.sub {dest}, {src}",
    "SD_FMUL": "f64.mul {dest}, {src}",
    "SD_FDIV": "f64.div {dest}, {src}",
    "SD_FSQRT": "f64.sqrt {dest}, {src}",
    "SD_FNEG": "f64.neg {dest}",
    "SD_FABS": "f64.abs {dest}",
    "SD_FCMP": "f64.cmp {src1}, {src2}",
    "SD_I2F": "f64.convert_i64_s {dest}, {src}",
    "SD_F2I": "i64.trunc_f64_s {dest}, {src}",
    "SD_SEXT": "i64.extend_i32_s {dest}, {src}",
    "SD_LOAD": "i64.load {dest}, [{src}]",
    "SD_STORE": "i64.store {val}, [{addr}]",
    "SD_JMP": "br {rel}",
    "SD_JCC": "br_if {rel}, {cond}",
    "SD_CALL": "call {target}",
    "SD_RET": "return",
    "SD_NOP": "nop",
    "SD_UD2": "unreachable",
    "SD_PUSH": "push {reg} ;; wasm stack",
    "SD_POP": "pop {reg} ;; wasm stack",
    "SD_STACK_ADDR": "stack.addr {dest}, {offset}",
    "WASM_GET_LOCAL": "local.get {dest}",
    "WASM_SET_LOCAL": "local.set {dest}",
    "WASM_TEE_LOCAL": "local.tee {dest}",
}

# RISC-V
RV_ASM = {
    "ADD": "add {dest}, {src1}, {src2}",
    "SUB": "sub {dest}, {src1}, {src2}",
    "MUL": "mul {dest}, {src1}, {src2}",
    "DIVU": "divu {dest}, {src1}, {src2}",
    "DIV": "div {dest}, {src1}, {src2}",
    "REMU": "remu {dest}, {src1}, {src2}",
    "REM": "rem {dest}, {src1}, {src2}",
    "AND": "and {dest}, {src1}, {src2}",
    "OR": "or {dest}, {src1}, {src2}",
    "XOR": "xor {dest}, {src1}, {src2}",
    "SLL": "sll {dest}, {src1}, {src2}",
    "SRL": "srl {dest}, {src1}, {src2}",
    "SRA": "sra {dest}, {src1}, {src2}",
    "SLT": "slt {dest}, {src1}, {src2}",
    "SLTU": "sltu {dest}, {src1}, {src2}",
    "ADDI": "addi {dest}, {src1}, {imm}",
    "ANDI": "andi {dest}, {src1}, {imm}",
    "ORI": "ori {dest}, {src1}, {imm}",
    "XORI": "xori {dest}, {src1}, {imm}",
    "SLTI": "slti {dest}, {src1}, {imm}",
    "SLTIU": "sltiu {dest}, {src1}, {imm}",
    "SLLI": "slli {dest}, {src1}, {shamt}",
    "SRLI": "srli {dest}, {src1}, {shamt}",
    "SRAI": "srai {dest}, {src1}, {shamt}",
    "LUI": "lui {dest}, {imm}",
    "LD": "ld {dest}, {imm}({base})",
    "SD": "sd {src2}, {imm}({base})",
    "JAL": "jal {dest}, .L{offset}",
    "JALR": "jalr {dest}, {imm}({base})",
    "BEQ": "beq {src1}, {src2}, .L{offset}",
    "BNE": "bne {src1}, {src2}, .L{offset}",
    "BLT": "blt {src1}, {src2}, .L{offset}",
    "BGE": "bge {src1}, {src2}, .L{offset}",
    "BLTU": "bltu {src1}, {src2}, .L{offset}",
    "BGEU": "bgeu {src1}, {src2}, .L{offset}",
    "NOP_RV": "nop",
    "ECALL": "ecall",
}


def parse_fields(inst_block_text):
    """Extract field names in BTreeMap order from an [inst.X] block."""
    m = re.search(r'fields\s*=\s*\{\s*([^}]+)\}', inst_block_text)
    if not m:
        return []
    pairs = m.group(1)
    fields = []
    for pair in pairs.split(','):
        pair = pair.strip()
        if '=' in pair:
            name = pair.split('=')[0].strip().strip('"')
            fields.append(name)
    return sorted(fields)  # BTreeMap order


def extract_inst_blocks(content):
    """Split TOML content by [inst.X] sections, returning dict name→block_text."""
    blocks = {}
    pattern = re.compile(r'\[inst\.(\w+)\]\n(.*?)(?=\n\[|\Z)', re.DOTALL)
    for m in pattern.finditer(content):
        name = m.group(1)
        text = m.group(0)
        blocks[name] = text
    return blocks


def get_field_order(content):
    """Get field order (BTreeMap sorted) for each instruction."""
    orders = {}
    pattern = re.compile(r'\[inst\.(\w+)\]\n(.*?)(?=\n\[|\Z)', re.DOTALL)
    for m in pattern.finditer(content):
        name = m.group(1)
        text = m.group(2)
        orders[name] = parse_fields(text)
    return orders


def convert_lowering_args(inst_name, args_dict, field_order):
    """Convert {k1:v1, k2:v2} → ordered values string."""
    if inst_name not in field_order:
        return None
    fields = field_order[inst_name]
    values = [args_dict.get(f, "???") for f in fields]
    return ", ".join(values)


def convert_insts_line(line, field_order_lookup):
    """Convert a line containing insts = [{...}] to insts = [\"...\"] format."""
    # Match insts = [{ inst = "NAME", args = { k = "v", ... } }, ...]
    result = []
    # Find all { inst = "X", args = { k="v",... } } blocks
    pattern = re.compile(r'\{\s*inst\s*=\s*"([^"]+)"\s*,\s*args\s*=\s*\{([^}]+)\}\s*\}')

    def replace_one(m):
        inst_name = m.group(1)
        args_str = m.group(2)
        # Parse args
        args = {}
        for pair in re.finditer(r'(\w+)\s*=\s*"([^"]*)"', args_str):
            args[pair.group(1)] = pair.group(2)
        # Convert to ordered values
        if inst_name in field_order_lookup:
            fields = field_order_lookup[inst_name]
            values = []
            for f in fields:
                v = args.get(f, "")
                if not v:
                    # Check if it's hidden/remapped
                    pass
                values.append(v)
            ops = ", ".join(values)
            return f'"{inst_name} {ops}"'
        else:
            # Keep as-is if we don't know the field order
            return m.group(0)

    return pattern.sub(replace_one, line)


def add_asm_to_insts(content, asm_map):
    """Add asm = \"...\" after each encoding line in [inst.X] blocks."""
    lines = content.split('\n')
    current_inst = None
    result = []
    for line in lines:
        m = re.match(r'\[inst\.(\w+)\]', line)
        if m:
            current_inst = m.group(1)
        result.append(line)
        if current_inst and re.match(r'\s*encoding\s*=', line):
            asm = asm_map.get(current_inst)
            if asm:
                indent = line[:len(line) - len(line.lstrip())]
                result.append(f'{indent}asm = "{asm}"')
    return '\n'.join(result)


def convert_lowering_to_asm(content, field_order):
    """Convert all lowering insts from structured to asm string format."""
    # Pattern matches insts = [{ inst = "...", args = { ... } }, ...]
    def replace_insts(m):
        inner = m.group(1)
        # Parse each { inst = "X", args = { k="v",... } }
        items = []
        for item_m in re.finditer(r'\{\s*inst\s*=\s*"([^"]+)"\s*,\s*args\s*=\s*\{([^}]+)\}\s*\}', inner):
            inst_name = item_m.group(1)
            args_str = item_m.group(2)
            args = {}
            for pair_m in re.finditer(r'(\w+)\s*=\s*"([^"]*)"', args_str):
                args[pair_m.group(1)] = pair_m.group(2)

            if inst_name in field_order:
                fields = field_order[inst_name]
                values = [args.get(f, "") for f in fields]
                ops = ", ".join(values)
                items.append(f'"{inst_name} {ops}"')
            else:
                items.append(item_m.group(0))

        if items:
            return f'insts = [{", ".join(items)}]'
        return m.group(0)

    # Match insts = [ ... ] blocks (multi-line aware)
    pattern = re.compile(r'insts\s*=\s*\[([^\]]+)\]', re.DOTALL)
    return pattern.sub(replace_insts, content)


def migrate_file(filepath, asm_map, is_riscv=False):
    """Migrate a single ISA TOML file."""
    with open(filepath, 'r') as f:
        content = f.read()

    # Step 1: Add asm fields to inst definitions
    content = add_asm_to_insts(content, asm_map)

    # Step 2: Get field order for all instructions
    field_order = get_field_order(content)

    # Step 3: Convert lowering insts to asm strings
    content = convert_lowering_to_asm(content, field_order)

    with open(filepath, 'w') as f:
        f.write(content)

    print(f"  Migrated: {filepath} ({len(field_order)} instructions)")


if __name__ == "__main__":
    base = "examples/isa"

    # x86_64
    migrate_file(f"{base}/x86_64_v10.toml", X86_ASM)

    # aarch64
    migrate_file(f"{base}/aarch64_v10.toml", A64_ASM)

    # wasm32
    migrate_file(f"{base}/wasm32_v10.toml", WASM_ASM)

    # riscv64
    migrate_file(f"{base}/riscv64_v10.toml", RV_ASM, is_riscv=True)

    print("\nAll files migrated.")
