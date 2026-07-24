"""Add asm fields to minimal_sd.toml instruction definitions."""
import re

with open("examples/isa/minimal_sd.toml", "r") as f:
    content = f.read()

asm_map = {
    "SD_MOV": "mov {dest}, {src}",
    "SD_MOV_IMM": "mov {dest}, {imm}",
    "SD_ADD": "add {dest}, {src}",
    "SD_SUB": "sub {dest}, {src}",
    "SD_MUL": "mul {dest}, {src}",
    "SD_UDIV": "udiv {dest}, {src}",
    "SD_SDIV": "sdiv {dest}, {src}",
    "SD_UREM": "urem {dest}, {src}",
    "SD_SREM": "srem {dest}, {src}",
    "SD_AND": "and {dest}, {src}",
    "SD_OR": "or {dest}, {src}",
    "SD_XOR": "xor {dest}, {src}",
    "SD_NOT": "not {dest}",
    "SD_NEG": "neg {dest}",
    "SD_SHL": "shl {dest}, {src}",
    "SD_SHR": "shr {dest}, {src}",
    "SD_SAR": "sar {dest}, {src}",
    "SD_CMP": "cmp {src1}, {src2}",
    "SD_SETCC": "set{cond} {dest}",
    "SD_FMOV": "fmov {dest}, {src}",
    "SD_FMOV_BITS": "fmov.bits {dest}, {src}",
    "SD_FADD": "fadd {dest}, {src}",
    "SD_FSUB": "fsub {dest}, {src}",
    "SD_FMUL": "fmul {dest}, {src}",
    "SD_FDIV": "fdiv {dest}, {src}",
    "SD_FSQRT": "fsqrt {dest}, {src}",
    "SD_FNEG": "fneg {dest}",
    "SD_FABS": "fabs {dest}",
    "SD_FCMP": "fcmp {src1}, {src2}",
    "SD_I2F": "i2f {dest}, {src}",
    "SD_F2I": "f2i {dest}, {src}",
    "SD_SEXT": "sext {dest}, {src}",
    "SD_LOAD": "ld {dest}, [{src}]",
    "SD_STORE": "st [{addr}], {val}",
    "SD_JMP": "jmp {rel}",
    "SD_JCC": "j{cond} {rel}",
    "SD_CALL": "call {target}",
    "SD_RET": "ret",
    "SD_NOP": "nop",
    "SD_UD2": "ud2",
    "SD_PUSH": "push {reg}",
    "SD_POP": "pop {reg}",
    "SD_STACK_ADDR": "stackaddr {dest}, {offset}",
}

lines = content.split("\n")
current_inst = None
result = []
for line in lines:
    m = re.match(r"\[inst\.(\w+)\]", line)
    if m:
        current_inst = m.group(1)
    result.append(line)
    if current_inst and re.match(r"\s*encoding\s*=", line):
        asm = asm_map.get(current_inst, "???")
        indent = line[: len(line) - len(line.lstrip())]
        result.append(f'{indent}asm = "{asm}"')

with open("examples/isa/minimal_sd.toml", "w") as f:
    f.write("\n".join(result))

print(f"Done: updated minimal_sd.toml")
