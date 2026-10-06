// aarch64 `mov <Rd>, #imm`（立即数别名 = `movz` hw=0 那一格）：
// 与寄存器形态 `mov {dst}, {src}` **形状不同**（`#` 前缀），所以可以共存而互不遮蔽。
// 编码照 [[templates]] 的 rows：MOVZX = 0x1A5、MOVZW = 0xA5（hw=0）。
import fs from 'node:fs';

const SPEC = 'isa/arm64.toml';
let t = fs.readFileSync(SPEC, 'utf8');
const eol = t.includes('\r\n') ? '\r\n' : '\n';

const blocks = [`# ── \`mov <Rd>, #imm\`（立即数别名：\`movz #imm, lsl #0\`）──
#
# 与寄存器形态 \`mov {dst}, {src}\` 形状不同（带 \`#\`）⇒ 共存不遮蔽。
# 上游把 \`mov x0, #0\` 编成 \`movz x0, #0\`（0xD2800000），正是这两条的编码。`];
for (const [nm, mnem, slot, opcode] of [
  ['MOVXIMM', 'mov', 'r64', '0x1A5'],
  ['MOVWIMM', 'mov', 'r32', '0xA5'],
]) {
  blocks.push(`[[instructions]]
name = "${nm}"
form = "MOVW16"
opcode = ${opcode}
fields = { hw = 0 }
ops = ["dst:${slot}:out", "imm:imm16u"]
asm = "${mnem} {dst}, #{imm}"`);
}

const lines = t.split(/\r?\n/);
const anchor = 'asm = "subs {dst}, {src}, {src2}, lsl #{sh}"';
const at = lines.findIndex((l) => l.trim() === anchor);
if (at < 0) throw new Error('找不到指令段尾锚点');
lines.splice(at + 1, 0, '', ...blocks.join('\n\n').split('\n'));
fs.writeFileSync(SPEC, lines.join(eol));
console.log('生成 2 条（mov 立即数别名）');
