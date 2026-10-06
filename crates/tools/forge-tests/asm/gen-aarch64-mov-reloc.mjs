// aarch64 `movz`/`movk`/`movn` 的 **`abs_g*`/`prel_g*`/`tprel_g*`/`dtprel_g*`/`gottprel_g*` 重定位形态**。
//
// 与数值形态**同编码**，差别只在立即数槽是符号槽；关键点是 `hw`（半字选择）：**修饰名决定 hw**
// （`abs_g0` ⇒ 0、`abs_g1` ⇒ 1、`abs_g2` ⇒ 2、`abs_g3` ⇒ 3），所以四个槽各自用
// `imm_fns = [...]` **过滤**修饰表（新槽键，前一提交落地）——否则 `movz … #:abs_g2:foo` 会被
// `hw = 0` 的变体接走、静默写错 hw。
//
// 修饰名带 `#` / 不带 `#` 两种写法各有一个 imm_fn（`abs_g0` 与 `abs_g0_n`），过滤表里两种都要列。
// opcode/form 从谱里的**数值形态**读，不手抄。
import fs from 'node:fs';

const SPEC = 'isa/arm64.toml';
let t = fs.readFileSync(SPEC, 'utf8');
const eol = t.includes('\r\n') ? '\r\n' : '\n';
const J = (a) => a.join(eol);

// ── ① 从谱里读数值形态：mnemonic × 宽度 → (form, opcode)，以及 hw 的固定值 ──
const read = (name) => {
  const m = t.match(new RegExp(`\\[\\[instructions\\]\\]\\r?\\n(?:#[^\\n]*\\r?\\n)*name = "${name}"\\r?\\n([\\s\\S]*?)(?:\\r?\\n\\r?\\n|$)`));
  if (!m) throw new Error(`找不到 ${name}`);
  const op = m[1].match(/opcode = (0x[0-9A-Fa-f]+)/);
  const form = m[1].match(/form = "(\w+)"/);
  return { form: form ? form[1] : 'MOVW16', opcode: op ? op[1] : null };
};
// 只做 **X 宽度**：谱里现有的数值形态是 `MOVZ`/`MOVK`/`MOVN`（外加 `MOVZX1..3`/`MOVKX1..2`
// 等 hw 变体），**没有** `MOVZW`/`MOVKW`/`MOVNW` ⇒ W 的 opcode 无从抄起，留到补 W 数值形态那批。
const NAMES = [
  ['MOVZ', 'movz', 'X'],
  ['MOVK', 'movk', 'X'],
  ['MOVN', 'movn', 'X'],
];
const base = {};
for (const [n, mnem, w] of NAMES) {
  const r = read(n);
  if (!r.opcode) throw new Error(`${n} 没读到 opcode`);
  base[`${mnem}${w}`] = { ...r, mnem, w };
}

// ── ② 按 hw 分组的修饰名（两种写法都进过滤表）——直接从谱里的 imm_fn 名单读 ──
const GROUPS = { 0: [], 1: [], 2: [], 3: [] };
const reloc = [...t.matchAll(/\[\[conventions\.imm_fn\]\]\r?\nname = "([a-z0-9_]+)"/g)].map((m) => m[1]);
for (const name of reloc) {
  const m = name.match(/^(?:abs|prel|tprel|dtprel|gottprel)_g([0-3])/);
  if (!m) continue;
  GROUPS[Number(m[1])].push(name);
}
for (const k of Object.keys(GROUPS)) {
  if (GROUPS[k].length === 0) throw new Error(`hw=${k} 组没有修饰名`);
}

// ── ③ 四个符号槽（各自过滤）──────────────────────────────────────────────
const anchorSlot = J(['[[operand_slots]]', 'name = "imm12sym"']);
if (!t.includes(anchorSlot)) throw new Error('找不到 imm12sym 锚点');
const slotBlocks = [
  '# `movz`/`movk`/`movn` 的符号立即数：**`hw` 由修饰名决定**，所以按 hw 分四个槽、',
  '# 每个槽用 `imm_fns` 过滤修饰表（否则会把 `abs_g2` 的写法编成 `hw = 0`）。',
];
for (const hw of [0, 1, 2, 3]) {
  slotBlocks.push(
    '[[operand_slots]]',
    `name = "imm16g${hw}"`,
    'kind = "imm"',
    'signed = false',
    'width = 16',
    'symbols = true',
    'require_symbol = true',
    `imm_fns = [${GROUPS[hw].map((n) => `"${n}"`).join(', ')}]`,
    '',
  );
}
slotBlocks.pop();
t = t.replace(anchorSlot, slotBlocks.join(eol) + eol + eol + anchorSlot);

// ── ④ 24 条变体（数值形态之后声明）────────────────────────────────────────
const blocks = [];
blocks.push(`# ── \`movz\`/\`movk\`/\`movn\` 的 \`abs_g*\` 类重定位（\`movz x2, #:abs_g0:sym\`）──
#
# 与数值形态同编码；\`hw\` 由**修饰名**决定（\`abs_g0..g3\` ⇒ \`hw\` 0..3），所以槽按 hw 分并
# 用 \`imm_fns\` 过滤。模板**不带 \`#\`**（修饰文本自带），与数值形态形状不同、不互相遮蔽。`);
for (const [, mnem, w] of NAMES) {
  const b = base[`${mnem}${w}`];
  for (const hw of [0, 1, 2, 3]) {
    const slot = w === 'X' ? 'r64' : 'r32';
    blocks.push(`[[instructions]]
name = "${mnem.toUpperCase()}${w}_G${hw}_RELO"
form = "${b.form}"
opcode = ${b.opcode}
fields = { hw = ${hw} }
ops = ["dst:${slot}:out", "imm:imm16g${hw}"]
asm = "${mnem} {dst}, {imm}"`);
  }
}

const lines = t.split(/\r?\n/);
const anchorIns = 'asm = "subs {dst}, {src}, {src2}, lsl #{sh}"';
const at = lines.findIndex((l) => l.trim() === anchorIns);
if (at < 0) throw new Error('找不到指令段尾锚点');
lines.splice(at + 1, 0, '', ...blocks.join('\n\n').split('\n'));
fs.writeFileSync(SPEC, lines.join(eol));
console.log(`生成 ${blocks.length - 1} 条 + 4 个槽；各 hw 组的修饰名数：${[0, 1, 2, 3].map((h) => GROUPS[h].length).join('/')}`);
