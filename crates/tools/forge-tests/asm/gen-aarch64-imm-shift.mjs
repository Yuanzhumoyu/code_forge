// aarch64 移位立即数（`add w3, w4, #1024, lsl #12`）的生成器。
//
// A64 的 Add/subtract (immediate) 的 bit22 = sh（0 = 无移位、1 = 左移 12 位）；**文本里的立即数
// 始终是未移位的值**（`#1024, lsl #12` ⇒ 字段 1024），所以 `[[operand_slots]]` 的 `unit` 不受影响。
// 同一族还有**重定位 + 移位**的写法（`add x2, x3, #:lo12:sym, lsl #12`），符号槽是 `imm12sym`
// （unit 1 + symbols + require_symbol）——因为模板形状与数值变体不同，两者不互相遮蔽。
import fs from 'node:fs';

const SPEC = 'isa/arm64.toml';

let t = fs.readFileSync(SPEC, 'utf8');
const eol = t.includes('\r\n') ? '\r\n' : '\n';

const MN = [
  ['ADD', 'add', { X: 0x91, W: 0x11 }],
  ['ADDS', 'adds', { X: 0xb1, W: 0x31 }],
  ['SUB', 'sub', { X: 0xd1, W: 0x51 }],
  ['SUBS', 'subs', { X: 0xf1, W: 0x71 }],
];

const blocks = [];
blocks.push(`# ── 移位立即数（\`add w3, w4, #1024, lsl #12\`）──
#
# bit22 = sh（0 = 无移位 / 1 = 左移 12 位）；文本里的立即数**始终是未移位的值**（字段 = 1024）。
# 数值形态用 \`imm12u\`（不缩放），重定位形态用 \`imm12sym\`（unit 1 + symbols + require_symbol）——
# 后者的模板形状（\`#:lo12:sym, lsl #12\`）与数值形态不同，不会互相遮蔽。`);

for (const [nm, mnem, ops8] of MN) {
  for (const w of ['X', 'W']) {
    const slot = w === 'X' ? 'r64' : 'r32';
    blocks.push(`[[instructions]]
name = "${nm}_${w}_LSL12"
form = "ALUIMM"
opcode = 0x${ops8[w].toString(16).toUpperCase()}
fields = { shift = 1 }
ops = ["dst:${slot}:out", "src:${slot}", "imm:imm12u"]
asm = "${mnem} {dst}, {src}, #{imm}, lsl #12"`);
  }
}
blocks.push(`# 重定位 + 移位：\`add x2, x3, #:lo12:sym, lsl #12\`（\`elf-reloc-addsubimm.s\` 就这一条）。`);
for (const [nm, mnem, ops8] of MN) {
  for (const w of ['X', 'W']) {
    const slot = w === 'X' ? 'r64' : 'r32';
    blocks.push(`[[instructions]]
name = "${nm}_${w}_RELO_LSL12"
form = "ALUIMM"
opcode = 0x${ops8[w].toString(16).toUpperCase()}
fields = { shift = 1 }
ops = ["dst:${slot}:out", "src:${slot}", "imm:imm12sym"]
asm = "${mnem} {dst}, {src}, {imm}, lsl #12"`);
  }
}

const lines = t.split(/\r?\n/);
const anchorIns = 'asm = "subs {dst}, {src}, {src2}, lsl #{sh}"';
const at = lines.findIndex((l) => l.trim() === anchorIns);
if (at < 0) throw new Error('找不到指令段尾锚点');
lines.splice(at + 1, 0, '', ...blocks.join('\n\n').split('\n'));
fs.writeFileSync(SPEC, lines.join(eol));
console.log(`生成 ${blocks.length - 2} 块（数值 8 + 重定位 8）`);
