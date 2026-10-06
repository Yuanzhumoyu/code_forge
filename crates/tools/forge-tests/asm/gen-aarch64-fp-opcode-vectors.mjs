// FP unscaled 访存（LDUR/STUR 的 D/S 形态）的字节向量。
// 字节来源：语料 CHECK 行的上游期望值——`ldur d8, [sp, #8]` = e8 83 40 fc、`stur d8` = e8 83 00 fc、
// `ldur s7, [sp, #4]` = e7 43 40 bc、`stur s7` = e7 43 00 bc。
// 我们的 FP 槽用 **V 名**（语料写 `d8`/`s7`），所以向量用同名寄存器的 V 写法——钉住的是**编码**。
import fs from 'node:fs';

const p = 'isa/arm64.toml';
let t = fs.readFileSync(p, 'utf8');
const eol = t.includes('\r\n') ? '\r\n' : '\n';
const J = (a) => a.join(eol);
const anchor = 'bytes = [0x41, 0xC0, 0x23, 0x8B]';
if (!t.includes(anchor)) throw new Error('找不到向量锚点');
const vecs = [
  ['ldur v8, [sp, #8]', [0xe8, 0x83, 0x40, 0xfc]],
  ['stur v8, [sp, #8]', [0xe8, 0x83, 0x00, 0xfc]],
  ['ldur v7, [sp, #4]', [0xe7, 0x43, 0x40, 0xbc]],
  ['stur v7, [sp, #4]', [0xe7, 0x43, 0x00, 0xbc]],
];
const block = ['', '# FP unscaled 访存（LDUR/STUR 的 D/S 形态）：字节取自语料的上游期望值（`d8`/`s7` 对应的编码）。'];
for (const [asm, bytes] of vecs) {
  block.push('[[vectors]]', `asm = "${asm}"`, `bytes = [${bytes.map((b) => '0x' + b.toString(16).toUpperCase().padStart(2, '0')).join(', ')}]`, '');
}
block.pop();
fs.writeFileSync(p, t.replace(anchor, anchor + J(block)));
console.log(`追加 ${vecs.length} 条向量`);
