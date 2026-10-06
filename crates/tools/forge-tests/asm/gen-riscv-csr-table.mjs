#!/usr/bin/env node
//! riscv64 **命名 CSR 表**的生成器（`isa/riscv64.toml` 的 `[conventions.imm_names.csr]`）。
//!
//! 表是从上游**机械生成**的，不是手抄的——判据与出处都在这份脚本里：
//!
//! 1. 取上游 LLVM 的 `llvm/lib/Target/RISCV/RISCVSystemOperands.td`（钉死 ref `llvmorg-19.1.0`），
//!    用**恰好够用的 TableGen 子集**解析：匿名 `def : SysReg<…>`、`foreach i = A...B in`
//!    区间（`hpmcounter3..31`、`pmpaddr0..63`、`mstateen0..3`…）、`let AltName/DeprecatedName`
//!    别名、`!add`/`!sub`/`!and` 与 `"前缀"#i#"后缀"` 拼接；
//! 2. **用语料自带的期望字节逐条对账**：`asm/parse/riscv64/llvm-mc/**` 里
//!    `# CHECK-ENC: encoding: [...]`（上游自己写的 oracle）反解出 name → 值，与第 1 步的表
//!    比：缺名 / 值不符 / 语料自相矛盾都会打印出来（**有输出就是错**，不是"参考信息"）；
//! 3. 把结果**原地重写**进 `isa/riscv64.toml`（保留原文件行尾；幂等——重复跑不产生 diff）。
//!
//! 用法：`node crates/tools/forge-tests/asm/gen-riscv-csr-table.mjs`
//!
//! 为什么不是 fetch.mjs 的一部分：那支管的是**语料**（拉 `.s` + 取舍 + 出处），
//! 这支管的是**谱里的数据表**（上游 .td → TOML）；两者的输入、输出、失败模式都不同。
//! 刷新 spec（换 ref）时跑这支，再跑 fetch.mjs 看取舍账。

import fs from 'node:fs';
import path from 'node:path';

const REF = 'llvmorg-19.1.0';
const URLS = [
  `https://raw.githubusercontent.com/llvm/llvm-project/${REF}/llvm/lib/Target/RISCV/RISCVSystemOperands.td`,
  `https://cdn.jsdelivr.net/gh/llvm/llvm-project@${REF}/llvm/lib/Target/RISCV/RISCVSystemOperands.td`,
];
const SPEC = path.resolve('isa/riscv64.toml');
const CORPUS = path.resolve('crates/tools/forge-tests/asm/parse/riscv64/llvm-mc');
const TABLE = '[conventions.imm_names.csr]';

// ── ① 取上游 .td ────────────────────────────────────────────────────────────
let td = null;
for (let a = 0; a < 6 && td === null; a++) {
  const url = URLS[a % URLS.length];
  try {
    const r = await fetch(url, { headers: { 'user-agent': 'forge-tests-fetch' } });
    const body = await r.text();
    if (r.ok && body.includes('class SysReg')) td = body;
    else console.log(`# ${r.status} ${url}（${body.length} 字节，不合用）`);
  } catch (e) {
    console.log(`# 失败 ${url}: ${e.message}`);
  }
  if (td === null) await new Promise((r) => setTimeout(r, 800 * (a + 1)));
}
if (td === null) throw new Error('上游取不到（raw / jsDelivr 都试过）');

// ── TableGen 子集求值 ───────────────────────────────────────────────────────
/** `!add(a,b)` / `!sub(a,b)` / `!and(a,b)` / `!or(a,b)` / 字面量 / 循环变量 → 数值。 */
function evalExpr(src, vars) {
  const s = src.trim();
  const call = s.match(/^!(add|sub|and|or)\((.*)\)$/);
  if (call) {
    const args = [];
    let depth = 0;
    let cur = '';
    for (const ch of call[2]) {
      if (ch === '(') depth++;
      if (ch === ')') depth--;
      if (ch === ',' && depth === 0) {
        args.push(cur);
        cur = '';
      } else cur += ch;
    }
    args.push(cur);
    const [x, y] = args.map((a) => evalExpr(a, vars));
    return { add: x + y, sub: x - y, and: x & y, or: x | y }[call[1]];
  }
  if (/^0x[0-9a-fA-F]+$/.test(s)) return Number(s);
  if (/^\d+$/.test(s)) return Number(s);
  if (s in vars) return vars[s];
  throw new Error(`不会求值：${s}`);
}

/** `"hpmcounter"#i#"h"` → 字符串。 */
function evalName(src, vars) {
  return src
    .split('#')
    .map((p) => {
      const t = p.trim();
      if (t.startsWith('"') && t.endsWith('"')) return t.slice(1, -1);
      if (t in vars) return String(vars[t]);
      throw new Error(`不会拼名字：${src}（片段 ${t}）`);
    })
    .join('');
}

const rows = new Map(); // name → value
const stack = []; // 花括号帧：{ loop } 或 null
let pendingLoop = null; // `foreach … in` 后直接跟单条语句（无花括号）
let pendingAlt = null; // `let AltName = "x" in` —— 给紧随其后的 def 加别名

for (const line of td.split('\n')) {
  const fe = line.match(/foreach\s+(\w+)\s*=\s*(\d+)\s*\.\.\.\s*(\d+)\s+in\s*(\{?)/);
  if (fe) {
    const loop = { var: fe[1], from: Number(fe[2]), to: Number(fe[3]) };
    if (fe[4] === '{') stack.push(loop);
    else pendingLoop = loop;
    continue;
  }
  const opens = (line.match(/\{/g) ?? []).length;
  const closes = (line.match(/\}/g) ?? []).length;

  // `let AltName = "x" in`（行内前置）或 def 行尾 `{ let AltName = "x"; }`：两种都收。
  const preAlt = line.match(/let\s+(?:AltName|DeprecatedName)\s*=\s*"([^"]+)"\s*in\b/);
  if (preAlt) pendingAlt = preAlt[1];

  const defs = [...line.matchAll(/def\s*(?:\w+\s*)?:\s*SysReg<([^>]*)>/g)];
  if (defs.length > 0) {
    // 环境 = 栈里所有循环的**每一种组合**（上游只用到单层，这里照样支持多层）。
    let envs = [{}];
    for (const f of stack) {
      if (!f) continue;
      envs = envs.flatMap((e) =>
        Array.from({ length: f.to - f.from + 1 }, (_, k) => ({ ...e, [f.var]: f.from + k })),
      );
    }
    if (pendingLoop) {
      const f = pendingLoop;
      envs = Array.from({ length: f.to - f.from + 1 }, (_, k) => ({ [f.var]: f.from + k }));
      pendingLoop = null;
    }
    for (const env of envs) {
      for (const d of defs) {
        const inner = d[1];
        const comma = inner.indexOf(','); // 名字里没有逗号，第一个就是分隔
        const name = evalName(inner.slice(0, comma), env);
        const value = evalExpr(inner.slice(comma + 1).replace(/;$/, ''), env);
        const aliases = [...line.matchAll(/(?:AltName|DeprecatedName)\s*=\s*"([^"]+)"/g)].map(
          (m) => m[1],
        );
        if (pendingAlt) {
          aliases.push(pendingAlt);
          pendingAlt = null;
        }
        for (const n of [name, ...aliases]) {
          if (rows.has(n) && rows.get(n) !== value) {
            throw new Error(`上游自相矛盾：${n} 既是 ${rows.get(n)} 又是 ${value}`);
          }
          rows.set(n, value);
        }
      }
    }
  }
  for (let k = 0; k < opens; k++) if (!fe || k > 0) stack.push(null);
  for (let k = 0; k < closes; k++) stack.pop();
}

if (rows.size < 100) throw new Error(`只解析出 ${rows.size} 个 CSR——解析器与上游格式不符`);

// ── ② 用语料自带的期望字节对账 ──────────────────────────────────────────────
const fromCorpus = new Map();
const conflicts = [];
for (const f of fs.readdirSync(CORPUS).filter((n) => n.endsWith('.s'))) {
  const text = fs.readFileSync(path.join(CORPUS, f), 'utf8');
  let enc = null;
  for (const line of text.split('\n')) {
    const m = line.match(/encoding:\s*\[([^\]]*)\]/);
    if (m) {
      const bytes = [...m[1].matchAll(/0x([0-9a-fA-F]{2})/g)].map((x) => parseInt(x[1], 16));
      if (bytes.length === 4) {
        enc = (bytes[0] | (bytes[1] << 8) | (bytes[2] << 16) | (bytes[3] << 24)) >>> 0;
      }
      continue;
    }
    const ins = line.match(/^\s*csr(?:rw|rs|rc|rwi|rsi|rci)\s+\S+\s*,\s*([A-Za-z_]\w*)\s*,/);
    if (ins && enc !== null) {
      const name = ins[1];
      const value = (enc >>> 20) & 0xfff;
      if (fromCorpus.has(name) && fromCorpus.get(name) !== value) {
        conflicts.push(`${name}: 语料里既有 ${fromCorpus.get(name)} 又有 ${value}（${f}）`);
      }
      fromCorpus.set(name, value);
    }
  }
}
const missing = [...fromCorpus.keys()].filter((n) => !rows.has(n)).sort();
const mismatch = [...fromCorpus.entries()]
  .filter(([n, v]) => rows.has(n) && rows.get(n) !== v)
  .map(([n, v]) => `${n}: 表 ${rows.get(n)} vs 语料 ${v}`);
console.log(`上游表 ${rows.size} 个名字；语料反解 ${fromCorpus.size} 个名字`);
console.log(`  语料里表里没有的（${missing.length}）：${missing.join(' ') || '（无）'}`);
console.log(`  值不符（${mismatch.length}）：${mismatch.join('; ') || '（无）'}`);
console.log(`  语料自相矛盾（${conflicts.length}）：${conflicts.join('; ') || '（无）'}`);
if (missing.length || mismatch.length || conflicts.length) {
  throw new Error('对账不过——先查清楚再写进谱');
}

// ── ③ 原地重写 `[conventions.imm_names.csr]`（保留行尾；幂等）────────────────
const sorted = [...rows.entries()].sort((a, b) => a[1] - b[1] || a[0].localeCompare(b[0]));
const sameValue = new Map();
for (const [n, v] of sorted) sameValue.set(v, [...(sameValue.get(v) ?? []), n]);
const dups = [...sameValue.entries()].filter(([, n]) => n.length > 1);

const header = `# 命名立即数表（\`kind = "imm"\` + \`names\`）：**一个名字 = 一个值**（区别于上面的位集合表——
# 那是"若干名字拼接、按位或"）。CSR 名就是这张表：\`csrrs t1, mstatus, zero\` 与
# \`csrrs t2, 0x300, zero\` 同义；浮点舍入模式（\`rtz\`/\`dyn\`…）将来也走同一机制。
#
# 数据来源：上游 LLVM \`llvm/lib/Target/RISCV/RISCVSystemOperands.td\`（ref \`${REF}\`），
# **机械生成**——生成器 = \`crates/tools/forge-tests/asm/gen-riscv-csr-table.mjs\`，别手改这张表。
# 展开含 \`foreach\` 区间（\`hpmcounter3..31\`、\`pmpaddr0..63\`…）与 \`AltName\`/\`DeprecatedName\` 别名，
# 并用**语料自带的期望字节**逐条对账：语料里出现的 ${fromCorpus.size} 个名字全部命中、值零不符。
# **同值多名**（${dups.map(([, n]) => n.join('/')).join('、')}）渲染取**字典序最小者**，
# 与 \`[conventions.cond]\` 的"同码取字母序最小名"同口径。
#
# 运行时**双向**：解析认名字也认数字；渲染时值在表里就写名字（\`disassemble → assemble\` 闭合）。
# 改这张表＝改数据，不动生成器；名字非法/值超出槽的接受值域在 \`validate\` 期报错。
${TABLE}`;

const raw = fs.readFileSync(SPEC, 'utf8');
const eol = raw.includes('\r\n') ? '\r\n' : '\n';
const lines = raw.split(/\r?\n/);
const body = sorted.map(([n, v]) => `${n} = 0x${v.toString(16).toUpperCase().padStart(3, '0')}`);

const start = lines.findIndex((l) => l.trim() === TABLE);
let next;
if (start >= 0) {
  // 已有这张表：整块换掉（表体 = 表头之后直到下一个 `[` 开头的节）。
  next = lines.findIndex((l, i) => i > start && l.startsWith('['));
  if (next < 0) next = lines.length;
} else {
  const at = lines.findIndex((l) => l.trim() === '[[operand_slots]]');
  if (at < 0) throw new Error('找不到插入点（`[[operand_slots]]`）');
  start = at;
  next = at;
}
const out = [...lines.slice(0, start), ...header.split('\n'), ...body, '', ...lines.slice(next)];
fs.writeFileSync(SPEC, out.join(eol));
console.log(`已写入 isa/riscv64.toml：${header.split('\n').length + body.length} 行表（共 ${out.length} 行）`);
