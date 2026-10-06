// aarch64 SIMD&FP 访存的**宽度视图**族：B/H/S/D/Q × {无符号偏移, unscaled, 前索引, 后索引} × {ldr, str}，
// 外加 S/D/Q 的成对 LDP/STP × {有符号偏移, 前索引, 后索引}。
//
// **为什么必须按宽度分寄存器组**：FP 访存的助记符都是 `ldr`/`str`，宽度只体现在**寄存器名**上
// （`ldr s0,…` / `ldr d0,…`）。若所有名字同属一个组（都算 FPR(8)），两条变体的**文本形状完全相同**
// ⇒ 分派提交到先声明的那条（实测：`ldur s7, [sp, #4]` 被编成 D 形态）。所以保留既有
// `[reg.fpr8]`（V0..V31 = D 视图 + `D0..D31` 别名），另开 `fpr1/fpr2/fpr4/fpr16`（B/H/S/Q）。
// `[meta].default_fpr_width = 8` ⇒ ABI/角色的"主 FPR 类"仍是 FPR(8)，不受新增组影响。
//
// 编码（SIMD&FP；与 GPR 族同形，多一个 V=1 位）：
//   单寄存器 `size 111 V 00 opc …`；size：00=B、01=H、10=S、11=D；Q 是 size=00 + opc 位取 10。
//   [25:24]：01 = 无符号偏移（imm12）、00 = unscaled/前/后（imm9 + mode[11:10]：00/11/01）。
//   成对 `opc 101 V idx imm7 …`：opc 00=S、01=D、10=Q；idx：010 有符号 / 011 前 / 001 后。
import fs from 'node:fs';

const SPEC = 'isa/arm64.toml';
let t = fs.readFileSync(SPEC, 'utf8');
const eol = t.includes('\r\n') ? '\r\n' : '\n';
const J = (a) => a.join(eol);

// ── ① 宽度视图的寄存器组 + D 别名 ─────────────────────────────────────────
const anchorFpr = J(['[[operand_slots]]', 'name = "fpr"']);
if (!t.includes(anchorFpr)) throw new Error('找不到 fpr 槽锚点');
const dAliases = Array.from({ length: 32 }, (_, i) => `D${i} = ${i}`).join(', ');
t = t.replace(
  anchorFpr,
  [
    '# ── FP 的**宽度视图**：宽度必须由寄存器组承载 ──',
    '# 既有 `[reg.fpr8]`（V0..V31）就是 **D 视图**，下面给它挂 `D0..D31` 别名；',
    '# B/H/S/Q 各开一组。若共用一个组，`ldr s0,…` 与 `ldr d0,…` 的文本形状相同、分派只能取',
    '# 先声明的那条（实测把 S 编成了 D）——所以宽度不能只靠"名字在同一个池里"。',
    '[reg.fpr1]',
    'prefix = "B"',
    'count = 32',
    'aliases = {}',
    '',
    '[reg.fpr2]',
    'prefix = "H"',
    'count = 32',
    'aliases = {}',
    '',
    '[reg.fpr4]',
    'prefix = "S"',
    'count = 32',
    'aliases = {}',
    '',
    '[reg.fpr16]',
    'prefix = "Q"',
    'count = 32',
    'aliases = {}',
    '',
    '[[operand_slots]]',
    'name = "fprb"',
    'kind = "reg"',
    'class = "fpr1"',
    '',
    '[[operand_slots]]',
    'name = "fprh"',
    'kind = "reg"',
    'class = "fpr2"',
    '',
    '[[operand_slots]]',
    'name = "fprs"',
    'kind = "reg"',
    'class = "fpr4"',
    '',
    '[[operand_slots]]',
    'name = "fprq"',
    'kind = "reg"',
    'class = "fpr16"',
    '',
    anchorFpr,
  ].join(eol),
);
t = t.replace(
  /(\[reg\.fpr8\]\r?\n(?:.*\r?\n)*?)(\r?\n\[\[operand_slots\]\])/,
  (m, head, tail) => `${head}aliases = { ${dAliases} }${tail}`,
);

// ── ② 按宽度分单位的 imm 槽 ───────────────────────────────────────────────
const anchorImm = J(['[[operand_slots]]', 'name = "imm12b"']);
if (!t.includes(anchorImm)) throw new Error('找不到 imm12b 锚点');
const immSlots = [];
for (const [n, u, doc] of [
  ['imm12fp1', 1, 'B'],
  ['imm12fp2', 2, 'H'],
  ['imm12fp4', 4, 'S'],
  ['imm12fp8', 8, 'D'],
  ['imm12fp16', 16, 'Q'],
]) {
  immSlots.push('[[operand_slots]]', `name = "${n}"`, 'kind = "imm"', 'signed = false', 'width = 12', `unit = ${u}`, `# FP 访存（${doc} 视图）`, '');
}
immSlots.pop();
t = t.replace(anchorImm, immSlots.join(eol) + eol + eol + anchorImm);

// ── ③ 两个形式 ────────────────────────────────────────────────────────────
const anchorForm = J(['[[forms]]', 'name = "LSUIR"   # LDR/STR 寄存器偏移：rt rn rm', 'opcode_field = "op8"', 'operand_fields = ["rt", "rn", "rm"]']);
if (!t.includes(anchorForm)) throw new Error('找不到 LSUIR 锚点');
t = t.replace(
  anchorForm,
  [anchorForm, '', '[[forms]]', 'name = "FPLSUI"   # SIMD&FP 单寄存器无符号偏移：rt rn imm12', 'opcode_field = "op8"', 'operand_fields = ["rt", "rn", "imm12"]', '', '[[forms]]', 'name = "FPPAIR"   # SIMD&FP 成对：rt rn rt2 imm7', 'opcode_field = "op8"', 'operand_fields = ["rt", "rn", "rt2", "imm7"]'].join(eol),
);

// ── ④ 指令 ────────────────────────────────────────────────────────────────
const VIEWS = [
  ['B', 'fprb', 'imm12fp1', 0b00],
  ['H', 'fprh', 'imm12fp2', 0b01],
  ['S', 'fprs', 'imm12fp4', 0b10],
  ['D', 'fpr', 'imm12fp8', 0b11],
  // Q 与 B 同为 size 00（128 位靠 opc 位型区分）——槽的类不同，靠寄存器名分派。
  ['Q', 'fprq', 'imm12fp16', 0b00],
];
const PAIRS = [
  ['S', 'fprs', 'imm12fp4', 0b00],
  ['D', 'fpr', 'imm12fp8', 0b01],
  ['Q', 'fprq', 'imm12fp16', 0b10],
];
const base8 = (size, v24) => ((size << 6) | (0b111 << 3) | (1 << 2) | v24) & 0xff; // size 111 V [25:24]

const blocks = [];
blocks.push(`# ── SIMD&FP 访存（\`ldr d8, [sp, #8]\` / \`str s7, [sp, #4]\` / \`stp d8, d9, [sp, #-64]!\`）──
#
# 宽度只体现在寄存器名上（助记符都是 \`ldr\`/\`str\`/\`ldp\`/\`stp\`），所以按**视图**分槽
# （见 [reg.fpr1..fpr16] 的说明）；imm12/imm7 的单位 = 访问宽度。`);
for (const [view, slot, immSlot, size] of VIEWS) {
  for (const [dir, opc2] of [['ldr', 1], ['str', 0]]) {
    // 128 位（Q）的 opc 是 11/10，而 B/H/S/D 是 01/00。
    const isQ = view === 'Q';
    const opcEff = isQ ? (opc2 === 1 ? 3 : 2) : opc2;
    const reg = dir === 'ldr' ? 'dst' : 'src';
    const inout = dir === 'ldr' ? ':out' : '';
    blocks.push(`[[instructions]]
name = "${dir.toUpperCase()}${view}_FP"
form = "FPLSUI"
opcode = 0x${base8(size, 0b01).toString(16).toUpperCase()}
fields = { opc2 = ${opcEff} }
ops = ["${reg}:${slot}${inout}", "base:r64", "imm:${immSlot}"]
asm = "${dir} {${reg}}, [{base}, #{imm}]"`);
    const unscaled = base8(size, 0b00);
    blocks.push(`[[instructions]]
name = "${dir === 'ldr' ? 'LDUR' : 'STUR'}${view}_FP"
form = "LSUN"
opcode = 0x${unscaled.toString(16).toUpperCase()}
fields = { opc2 = ${opcEff}, mode = 0, zero21 = 0 }
ops = ["${reg}:${slot}${inout}", "base:r64", "off:imm9s"]
asm = "${dir === 'ldr' ? 'ldur' : 'stur'} {${reg}}, [{base}, #{off}]"`);
    for (const [sfx, mode, text] of [['_PRE', 3, `[{base}, #{off}]!`], ['_POST', 1, `[{base}], #{off}`]]) {
      blocks.push(`[[instructions]]
name = "${dir.toUpperCase()}${view}_FP${sfx}"
form = "LSUN"
opcode = 0x${unscaled.toString(16).toUpperCase()}
fields = { opc2 = ${opcEff}, mode = ${mode}, zero21 = 0 }
ops = ["${reg}:${slot}${inout}", "base:r64", "off:imm9s"]
asm = "${dir} {${reg}}, ${text}"`);
    }
  }
}
blocks.push(`# 成对 LDP/STP（SIMD&FP）：\`opc 101 V idx imm7 …\`；opc 00=S、01=D、10=Q；idx 010/011/001。`);
for (const [view, slot, immSlot, opc] of PAIRS) {
  for (const [dir, l] of [['stp', 0], ['ldp', 1]]) {
    const rt = dir === 'ldp' ? `${slot}:out` : slot;
    for (const [sfx, v24, idxBase, text] of [
      ['', 0b01, 0, `[{base}, #{imm}]`],
      ['_PRE', 0b01, 2, `[{base}, #{imm}]!`],
      ['_POST', 0b00, 2, `[{base}], #{imm}`],
    ]) {
      const op8 = (opc << 6) | (0b101 << 3) | (1 << 2) | v24;
      blocks.push(`[[instructions]]
name = "${dir.toUpperCase()}${view}_FP${sfx}"
form = "FPPAIR"
opcode = 0x${op8.toString(16).toUpperCase()}
fields = { idx2 = ${idxBase | l} }
ops = ["t1:${rt}", "base:r64", "t2:${rt}", "imm:${immSlot}"]
asm = "${dir} {t1}, {t2}, ${text}"`);
    }
  }
}

const lines = t.split(/\r?\n/);
const anchorIns = 'asm = "subs {dst}, {src}, {src2}, lsl #{sh}"';
const at = lines.findIndex((l) => l.trim() === anchorIns);
if (at < 0) throw new Error('找不到指令段尾锚点');
lines.splice(at + 1, 0, '', ...blocks.join('\n\n').split('\n'));
fs.writeFileSync(SPEC, lines.join(eol));
console.log(`生成 ${blocks.length - 2} 个指令块`);
