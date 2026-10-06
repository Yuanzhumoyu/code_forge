// aarch64 重定位修饰形态（`add x0, x0, #:lo12:sym` / `ldr w0, [sp, #:lo12:...]`）的生成器。
//
// 机制全是既有的 + 本轮新加的 `require_symbol`：
//   · `[[conventions.imm_fn]]`：修饰名 → 值语义。**文本模板不带 `#`**（`:#{mod}:{0}` 里的 `#`
//     由指令模板消费）——这样**同一张表**同时覆盖 `#:lo12:sym` 与 `:lo12:sym` 两种写法；
//   · `symbols = true`：未定义 ident 记成符号引用、按 0 参与算术（与上游"未重定位字段 = 0"一致，
//     证据：`elf-reloc-addsubimm.s` 的 OBJ 期望写着 `91000062 add x2, x3, #0`）；
//   · `require_symbol = true`（**本轮新增的槽能力**）：本槽只在"记到了符号"时匹配——
//     否则数值写法会被它抢走（unit-1 槽的接受域比 unit-8 窄 ⇒ 特异性裁决里优先命中，
//     实测把 `ldr x0, [x1, #8]` 编成字段 8 而不是 1）。
//
// 修饰名清单是 **ISA 事实**（A64 的重定位修饰集合），不依赖语料状态；清单来源 = 语料普查
// （47 个名字，见 asm/README 的登记）。
import fs from 'node:fs';

const SPEC = 'isa/arm64.toml';

const MODS = [
  'abs_g0', 'abs_g0_nc', 'abs_g0_s', 'abs_g1', 'abs_g1_nc', 'abs_g1_s',
  'abs_g2', 'abs_g2_nc', 'abs_g2_s', 'abs_g3',
  'prel_g0', 'prel_g0_nc', 'prel_g1', 'prel_g1_nc', 'prel_g2', 'prel_g2_nc', 'prel_g3',
  'pg_hi21_nc', 'got', 'got_lo12', 'gotpage_lo15',
  'gottprel', 'gottprel_g0_nc', 'gottprel_g1', 'gottprel_lo12',
  'tlsdesc', 'tlsdesc_lo12',
  'tprel_g0', 'tprel_g0_nc', 'tprel_g1', 'tprel_g1_nc', 'tprel_g2', 'tprel_g2_nc', 'tprel_g3',
  'tprel_hi12', 'tprel_lo12', 'tprel_lo12_nc',
  'dtprel_g0', 'dtprel_g0_nc', 'dtprel_g1', 'dtprel_g1_nc', 'dtprel_g2', 'dtprel_g2_nc',
  'dtprel_hi12', 'dtprel_lo12', 'dtprel_lo12_nc',
  'secrel_hi12', 'secrel_lo12', 'lo12',
];

let t = fs.readFileSync(SPEC, 'utf8');
const eol = t.includes('\r\n') ? '\r\n' : '\n';
const J = (a) => a.join(eol);

// ── ① imm_fn 表 ───────────────────────────────────────────────────────────
const anchorConv = '[conventions.cond]';
if (!t.includes(anchorConv)) throw new Error('找不到 conventions 锚点');
const fnEntries = [
  '# ── 重定位修饰（A64 的 `#:lo12:sym` / `:lo12:sym`）──',
  '#',
  '# 修饰**只选重定位种类**，不改算术值：源文本的值 = 符号引用的占位（0），与上游',
  '# "未重定位字段 = 0"一致（`elf-reloc-addsubimm.s` 的 OBJ 期望写着 `add x2, x3, #0`）。',
  '# `text` **含 `#`**（`#:lo12:{0}` 与 `:lo12:{0}` 各一条）——于是指令模板**不带 `#`**，',
  '# 重定位变体的"文本形状"与数值变体**不同**：同一形状的多候选里分派会提交到排第一的那条',
  '# （不按操作数失败回退），形状相同就会互相遮蔽（实测：加了重定位变体后',
  '# `add W0, W1, #0` 反而装配不出来）。一张表覆盖带 `#` 与不带 `#` 两种写法。',
  '# 名字清单 = A64 的重定位修饰集合（语料普查 47 个名字）。',
];
for (const m of MODS) {
  fnEntries.push(
    '[[conventions.imm_fn]]',
    `name = "${m}"`,
    `text = "#:${m}:{0}"`,
    'expr = "{0}"',
    '',
    '[[conventions.imm_fn]]',
    `name = "${m}_n"`,
    `text = ":${m}:{0}"`,
    'expr = "{0}"',
    '',
  );
}
fnEntries.pop();
t = t.replace(anchorConv, fnEntries.join(eol) + eol + eol + anchorConv);

// ── ② 符号槽（unit = 1 + symbols + require_symbol）────────────────────────
const anchorSlot = J(['[[operand_slots]]', 'name = "amt3"', 'kind = "imm"', 'signed = false', 'width = 3']);
if (!t.includes(anchorSlot)) throw new Error('找不到 amt3 槽锚点');
t = t.replace(
  anchorSlot,
  [
    anchorSlot,
    '',
    '# ── 重定位修饰的**符号位置**：unit == 1（校验硬要求）+ symbols + require_symbol ──',
    '# `require_symbol` 把本槽收窄成"必须记到符号"——否则数值写法会被它抢走',
    '# （unit 更细 ⇒ 接受域更窄 ⇒ 特异性裁决里优先命中），见 model.rs 的字段注释。',
    '[[operand_slots]]',
    'name = "imm12sym"',
    'kind = "imm"',
    'signed = false',
    'width = 12',
    'symbols = true',
    'require_symbol = true',
  ].join(eol),
);

// ── ③ 变体指令 ────────────────────────────────────────────────────────────
const blocks = [];
blocks.push(`# ── 重定位修饰形态（\`add x0, x0, #:lo12:sym\` / \`ldr w0, [sp, #:lo12:...]\`）──
#
# 与数值形态**同编码、同 opcode**，差别只在立即数槽是**符号槽**（unit 1 + symbols +
# require_symbol）；修饰名由全局 \`[[conventions.imm_fn]]\` 匹配，所以**一个变体收全部修饰**。
# 声明序放在数值形态之后；即便特异性裁决把它排在前面，\`require_symbol\` 也会让它在
# "没记到符号"时直接不匹配，数值写法照旧落回数值槽。`);
for (const [nm, mnem, ops8] of [
  ['ADD', 'add', { X: 0x91, W: 0x11 }],
  ['ADDS', 'adds', { X: 0xb1, W: 0x31 }],
  ['SUB', 'sub', { X: 0xd1, W: 0x51 }],
  ['SUBS', 'subs', { X: 0xf1, W: 0x71 }],
]) {
  for (const w of ['X', 'W']) {
    const slot = w === 'X' ? 'r64' : 'r32';
    blocks.push(`[[instructions]]
name = "${nm}_${w}_RELO"
form = "ALUIMM"
opcode = 0x${ops8[w].toString(16).toUpperCase()}
fields = { shift = 0 }
ops = ["dst:${slot}:out", "src:${slot}", "imm:imm12sym"]
asm = "${mnem} {dst}, {src}, {imm}"`);
  }
}
for (const [dir, opc2] of [['ldr', 1], ['str', 0]]) {
  for (const w of ['X', 'W']) {
    const slot = w === 'X' ? 'r64' : 'r32';
    const op8 = w === 'X' ? 0xf9 : 0xb9;
    const reg = dir === 'ldr' ? 'dst' : 'src';
    blocks.push(`[[instructions]]
name = "${dir.toUpperCase()}${w}_RELO"
form = "LSUI"
opcode = 0x${op8.toString(16).toUpperCase()}
fields = { opc2 = ${opc2} }
ops = ["${reg}:${slot}${dir === 'ldr' ? ':out' : ''}", "base:r64", "imm:imm12sym"]
asm = "${dir} {${reg}}, [{base}, {imm}]"`);
  }
}

const lines = t.split(/\r?\n/);
const anchorIns = 'asm = "subs {dst}, {src}, {src2}, lsl #{sh}"';
const at = lines.findIndex((l) => l.trim() === anchorIns);
if (at < 0) throw new Error('找不到指令段尾锚点');
lines.splice(at + 1, 0, '', ...blocks.join('\n\n').split('\n'));
fs.writeFileSync(SPEC, lines.join(eol));
console.log(`生成 ${blocks.length - 1} 个变体块 + ${MODS.length} 条 imm_fn`);
