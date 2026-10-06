// aarch64 字节/半字 GPR 访存（ldrb/ldrh/ldrsb/ldrsh/ldrsw/strb/strh）的生成器。
// 编码：`size 111 V 00 opc imm12 Rn Rt`——与本谱既有的 LDR/STR 同一族，只是 size/opc 不同：
//   size 00 → 0x38（byte）、01 → 0x78（halfword）、10 → 0xB8（word/ldrsw）、11 → 0xF8（ldr/str X）
//   opc 00 = store、01 = 无符号载、10 = 符号载到 X、11 = 符号载到 W
// imm12 的单位 = 访问宽度（byte 1 / halfword 2 / word 4），LDRSW 是 word 4。
// 无位移形态 = imm12 = 0 那一格（与既有 LDRX_ND 同款：声明序在后、解码仍走带位移那条）。
import fs from 'node:fs';
import path from 'node:path';

const SPEC = 'isa/arm64.toml';
const DIR = 'crates/tools/forge-tests/asm/parse/aarch64/llvm-mc';

// (助记符, size 位, opc, 目的/源 槽, imm12 单位, 是否无位移)
// **无符号偏移族**（imm12）的 [25:24] = 01 ⇒ op8 是 0x39/0x79/0xB9（**不是** unscaled 族的
// 0x38/0x78/0xB8——这一处写错由全集语料的字节对拍档当场抓到，见 README 的登记）。
const DEFS = [
  ['ldrb', 0x39, 1, 'r32', 1],
  ['strb', 0x39, 0, 'r32', 1],
  ['ldrh', 0x79, 1, 'r32', 2],
  ['strh', 0x79, 0, 'r32', 2],
  ['ldrsb', 0x39, 2, 'r64', 1],
  ['ldrsb', 0x39, 3, 'r32', 1],
  ['ldrsh', 0x79, 2, 'r64', 2],
  ['ldrsh', 0x79, 3, 'r32', 2],
  ['ldrsw', 0xb9, 2, 'r64', 4],
];

let t = fs.readFileSync(SPEC, 'utf8');
const eol = t.includes('\r\n') ? '\r\n' : '\n';

// ① 新槽：按单位分（1/2/4）
const anchorSlot = ['[[operand_slots]]', 'name = "imm12w"'].join(eol);
if (!t.includes(anchorSlot)) throw new Error('找不到 imm12w 锚点');
const newSlots = [
  '# 字节/半字访存的 imm12：单位同样是"字节 ÷ 访问宽度"（`ldrb w5, [x4, #20]` ⇒ 字段 20；',
  '# `ldrh w2, [sp, #32]` ⇒ 字段 16）。LDRSW 是 word 访问 ⇒ 复用 `imm12w`。',
  '[[operand_slots]]',
  'name = "imm12b"',
  'kind = "imm"',
  'signed = false',
  'width = 12',
  'unit = 1',
  '',
  '[[operand_slots]]',
  'name = "imm12h"',
  'kind = "imm"',
  'signed = false',
  'width = 12',
  'unit = 2',
  '',
].join(eol);
t = t.replace(anchorSlot, newSlots + anchorSlot);

// ② 指令
const blocks = [];
blocks.push(`# ── 字节/半字 GPR 访存（\`ldrb w4, [x3]\`、\`ldrsh x5, [x9, #24]\`、\`strb w5, [x4, #20]\`）──
#
# 与 LDR/STR 同族（\`size 111 V 00 opc imm12 Rn Rt\`），只是 size/opc 不同：
# size 00 → \`0x38\`（byte）、01 → \`0x78\`（halfword）；opc 00 = store、01 = 无符号载、
# 10 = 符号载到 X、11 = 符号载到 W；\`ldrsw\` 是 size 10 + opc 10。
# imm12 的单位 = 访问宽度（byte 1 / halfword 2 / word 4，见槽上的 \`unit\`）。
# 无位移形态（\`[Xn]\`）= imm12 = 0 那一格，与 \`LDRX_ND\` 同款：声明序在后、解码走带位移那条。`);
for (const [mn, op8, opc2, slot, unit] of DEFS) {
  const imm = unit === 1 ? 'imm12b' : unit === 2 ? 'imm12h' : 'imm12w';
  const isLoad = opc2 !== 0;
  const reg = isLoad ? 'dst' : 'src';
  const nameBase = `${mn.toUpperCase()}${slot === 'r64' ? 'X' : 'W'}`;
  blocks.push(`[[instructions]]
name = "${nameBase}"
form = "LSUI"
opcode = 0x${op8.toString(16).toUpperCase()}
fields = { opc2 = ${opc2} }
ops = ["${reg}:${slot}${isLoad ? ':out' : ''}", "base:r64", "imm:${imm}"]
asm = "${mn} {${reg}}, [{base}, #{imm}]"`);
  // `ldrsw` 也在做：它的无符号偏移族 op8 是 `0xB9`（不是 unscaled 家族 `LDURSW` 的 `0xB8`），
  // 因此不再与 `LDURSW` 的 `mode`（bit10 起 2 位）在位 trie 里重叠。
  blocks.push(`[[instructions]]
name = "${nameBase}_ND"
form = "LSUIND"
opcode = 0x${op8.toString(16).toUpperCase()}
fields = { opc2 = ${opc2}, imm12s = 0 }
ops = ["${reg}:${slot}${isLoad ? ':out' : ''}", "base:r64"]
asm = "${mn} {${reg}}, [{base}]"`);
}
const lines = t.split(/\r?\n/);
const anchorIns = 'asm = "subs {dst}, {src}, {src2}, lsl #{sh}"';
const at = lines.findIndex((l) => l.trim() === anchorIns);
if (at < 0) throw new Error('找不到指令段尾锚点');
lines.splice(at + 1, 0, '', ...blocks.join('\n\n').split('\n'));
t = lines.join(eol);

// ③ 向量：从语料 CHECK 行里挑**具体字节**的样本（每种助记符一条）
const CHECK = /^(?:[;\s]|\/\/)*CHECK[^:]*:\s*(.+?)\s*(?:;|\/\/)\s*encoding:\s*\[([^\]]*)\]/;
const want = new Set(['ldrb', 'strb', 'ldrh', 'strh', 'ldrsb', 'ldrsh', 'ldrsw']);
const picked = new Map();
for (const f of fs.readdirSync(DIR).filter((n) => n.endsWith('.s'))) {
  for (const line of fs.readFileSync(path.join(DIR, f), 'utf8').split(/\r?\n/)) {
    const c = line.match(CHECK);
    if (!c) continue;
    const mn = c[1].trim().split(/\s+/)[0];
    if (!want.has(mn) || picked.has(mn + c[1].trim())) continue;
    const bytes = [...c[2].matchAll(/0x([0-9a-fA-F]{2})/g)].map((x) => parseInt(x[1], 16));
    if (bytes.length !== 4 || !/^(ldrb|strb|ldrh|strh|ldrsb|ldrsh|ldrsw)\s+[xw]\d+,\s*\[[xw]\d+,\s*#\d+\]$/.test(c[1].trim())) continue;
    if (!picked.has(mn)) picked.set(mn, [c[1].trim(), bytes]);
  }
}
const vAnchor = 'bytes = [0x41, 0xC0, 0x23, 0x8B]';
if (!t.includes(vAnchor)) throw new Error('找不到向量锚点');
const vBlock = ['', '# 字节/半字访存：字节取自语料 CHECK 行的上游期望值（每种助记符一条）。'];
for (const [mn, [asm, bytes]] of picked) {
  vBlock.push(
    '[[vectors]]',
    `asm = "${asm}"`,
    `bytes = [${bytes.map((b) => '0x' + b.toString(16).toUpperCase().padStart(2, '0')).join(', ')}]`,
    '',
  );
}
vBlock.pop();
t = t.replace(vAnchor, vAnchor + eol + vBlock.join(eol));
fs.writeFileSync(SPEC, t);
console.log(`生成 ${blocks.length - 1} 块（${DEFS.length} 组 × 2 形态）；向量 ${picked.size} 条：${[...picked.keys()].join(' ')}`);
