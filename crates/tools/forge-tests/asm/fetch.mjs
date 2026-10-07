#!/usr/bin/env node
//! 真实汇编语料的**拉取 + 取舍 + 出处登记**（`asm/parse/**`、`asm/PROVENANCE.md`）。
//!
//! 三件事，一条命令（默认档）：
//!
//! 1. **拉候选全集**：按钉死的 ref 拉上游 LLVM MC 三套目录的 `.s`（x86 只取 Intel 语法
//!    文件名那批——本汇编器只吃 Intel 语法）；
//! 2. **算取舍**：跑解析档与编码对拍档拿**逐文件**记分板（`target/asm-suite/*.json`：
//!    `<isa>.json` 的 `file_rows`、`encoding-files.json`），只留「本谱整条接得住
//!    （`parsed >= 1`）、不产生红桶（`tail_mismatch` ≤ 已登记缺口）、且编码对拍**没有字节
//!    差异**」的文件——**判据是算出来的，不是手抄清单**。`parsed = 0` 的文件（本谱完全
//!    不认识的上游扩展）、出红的文件、字节对不上的文件（上游用压缩编码/我们发基础编码那类）
//!    都删掉：红桶是门禁，不能被语料本身喂成"永久红"；
//! 3. **重写 `asm/PROVENANCE.md`**：逐文件 字节/行/sha256（**本仓库落盘件**的摘要）。
//!
//! 用法：
//!
//! ```text
//! node crates/tools/forge-tests/asm/fetch.mjs                 # 默认：拉全集 → 取舍 → 写出处
//! node crates/tools/forge-tests/asm/fetch.mjs --isa riscv64    # 只处理一套
//! node crates/tools/forge-tests/asm/fetch.mjs --all            # 连子目录一起拉（SVE/SME/AMX/apx/rvv）
//! node crates/tools/forge-tests/asm/fetch.mjs --keep-all       # 拉全集但不裁剪（评估模式）
//! node crates/tools/forge-tests/asm/fetch.mjs --list           # 只打印计划
//! node crates/tools/forge-tests/asm/fetch.mjs --no-fetch       # 跳过下载（只用本地文件重算）
//! ```
//!
//! 拉完**必须**刷计数棘轮并看 diff（脚本会打印命令）：`$env:FORGE_ASM_WRITE_RATCHET = "1"`。
//!
//! 为什么用 Node 而不是 `pwsh`/`curl`：Node 自带根证书（本机 `curl`/`git` 走系统证书库会报
//! `SEC_E_NO_CREDENTIALS`），且 `node` 在任何装了前端工具链的机器上都有。

import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { spawn, spawnSync } from 'node:child_process';

const REF = 'llvmorg-19.1.0';
const DLLLM = 'llvm/llvm-project';

// ── 语料来源（一处声明；`dir` 相对 `asm/parse/`）──────────────────────────────
const SOURCES = [
  {
    isa: 'riscv64',
    key: 'llvm-mc',
    dir: 'riscv64/llvm-mc',
    upstream: 'llvm/test/MC/RISCV',
    license: 'Apache-2.0 WITH LLVM-exception',
    pick: () => true,
  },
  {
    isa: 'aarch64',
    key: 'llvm-mc',
    dir: 'aarch64/llvm-mc',
    upstream: 'llvm/test/MC/AArch64',
    license: 'Apache-2.0 WITH LLVM-exception',
    pick: () => true,
  },
  {
    isa: 'x86',
    key: 'llvm-mc',
    dir: 'x86/llvm-mc',
    upstream: 'llvm/test/MC/X86',
    license: 'Apache-2.0 WITH LLVM-exception',
    // Intel 语法文件的上游命名惯例（`*-intel.s` / `intel-syntax*.s`）；AT&T 那批不进。
    pick: (name) => /intel/i.test(name),
  },
];

// ── 手工来源（不按目录拉，内容有节选/改写；`sha256` 由本文件核对，漂了就报错）────
const MANUAL = [
  {
    local: 'x86/gnu-gas-intel/intel.excerpt.s',
    repo: 'ahjragaas/binutils-gdb',
    ref: '19cc5b7efb3e6440ff76103994d4dcf0d23cb4f4',
    upstreamPath: 'gas/testsuite/gas/i386/intel.s',
    license: 'GPL-3.0-or-later',
    takeLines: 64,
    note: '**节选**：上游文件**前 64 行**，逐字未改（GAS 的 Intel 方言用例；后段是 32 位模式语法，本谱有意不做）；上游规范库 = sourceware.org binutils-gdb，commit 是内容寻址，任一镜像同 sha',
    sha256: '319eb18610a8e9e1947c6634a8700365d9e8cb007b6f6fc7f4340e3617d9554b',
  },
];

// ── 子目录来源的手工条目（`--all` 之外的默认档看不到子目录；这里登记它们的真实上游路径，
//    落盘名按"目录段折进文件名"的惯例——`apx/rex2-format-intel.s` → `apx-rex2-format-intel.s`）──
const SUBDIR_OVERRIDES = {
  'x86/apx-rex2-format-intel.s': 'llvm/test/MC/X86/apx/rex2-format-intel.s',
};

// ── 已登记的方言缺口（逐文件允许的红桶数；与 `asm/ratchet/<isa>.txt` 的 mismatch 行一一对应）──
// 键 = `<isa>/<文件名>`。新缺口不要往这里加：先看棘轮里的样例，能修谱就修谱。
const ALLOWANCES = {
  // `jal a0, a0`——上游 `rv32i-valid.s` 第 88 行的自跳写法，见 ratchet/riscv64.txt。
  'riscv64/rv32i-valid.s': 1,
};

const args = process.argv.slice(2);
const has = (f) => args.includes(f);
const opt = (f, d) => {
  const i = args.indexOf(f);
  return i >= 0 && args[i + 1] ? args[i + 1] : d;
};
const RECURSIVE = has('--all');
const KEEP_ALL = has('--keep-all') || RECURSIVE;
const NO_FETCH = has('--no-fetch');
const LIST = has('--list');
const ONLY = opt('--isa', null);

const ASM = path.dirname(fileURLToPath(import.meta.url));
const PARSE = path.join(ASM, 'parse');
const REPO = path.resolve(ASM, '../../../..');
const SCORES = path.join(REPO, 'target/asm-suite');

const api = (u) => fetch(u, { headers: { 'user-agent': 'forge-tests-fetch' } });
const manualUrl = (m) => `https://raw.githubusercontent.com/${m.repo}/${m.ref}/${m.upstreamPath}`;

/** 上游目录里的候选文件（顶层；`--all` 递归，子目录段折进文件名）。 */
async function candidates(s) {
  const base = `https://api.github.com/repos/${DLLLM}/git/trees/${REF}:${s.upstream}`;
  const r = await api(base);
  if (!r.ok) throw new Error(`列目录失败 ${base} → HTTP ${r.status}`);
  const top = await r.json();
  const entries = RECURSIVE
    ? (await (await api(`https://api.github.com/repos/${DLLLM}/git/trees/${top.sha}?recursive=1`)).json()).tree
    : top.tree.map((e) => ({ ...e, path: e.path }));
  const out = [];
  for (const e of entries) {
    if (e.type !== 'blob' || !e.path.endsWith('.s')) continue;
    // 子目录段折进文件名（`apx/rex2-format-intel.s` → `apx-rex2-format-intel.s`）：
    // 本目录不建子目录，且**只改落盘名，不改文件内容**。
    const local = e.path.replace(/\//g, '-');
    if (!s.pick(local)) continue;
    out.push({ isa: s.isa, key: s.key, dir: s.dir, rel: e.path, local, size: e.size, upstream: s.upstream + '/' + e.path, license: s.license });
  }
  return out;
}

async function download(f) {
  const dest = path.join(PARSE, f.dir, f.local);
  if (f.size !== undefined && fs.existsSync(dest) && fs.statSync(dest).size === f.size) return 'skip';
  for (let a = 0; a < 4; a++) {
    const r = await api(`https://raw.githubusercontent.com/${DLLLM}/${REF}/${f.upstream}`).catch(() => null);
    if (r && r.ok) {
      const buf = Buffer.from(await r.arrayBuffer());
      if (f.size !== undefined && buf.length !== f.size) throw new Error(`${f.upstream}: 大小不符 ${buf.length} != ${f.size}`);
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      fs.writeFileSync(dest, buf);
      return 'fetch';
    }
    await new Promise((res) => setTimeout(res, 300 * (a + 1)));
  }
  throw new Error(`下载失败 ${f.upstream}`);
}

/** 跑解析档 + 编码对拍档：写 `target/asm-suite/*.json`（**棘轮不一致会 panic，但记分板先写**，忽略退出码）。 */
async function score() {
  console.log('\n== 打分（两个档**并发**跑：asm_parse 与 asm_encoding 各起一个 cargo 子进程；棘轮此刻多半不一致，非零退出属正常）');
  // `--no-fail-fast` 是**必需**的：cargo 默认在第一个失败的测试目标就停，而这两个档的棘轮几乎
  // 必然同时不一致（换了谱/换了语料）——少了它，排在后面的 `asm_parse` 根本不跑，解析档记分板
  // 就停在**上一轮**的旧内容上；于是"这一轮才下载、还没打过分"的候选被记成 `not-scored` 砍掉
  // （判据被静默绕过，2026-10-07 实测踩到：364 个候选被误记）。
  // **并发**：两个档各起一个 cargo 子进程（此前一条命令串行跑两个目标）。顺带保证两档都真跑
  // ——即使某一档的棘轮不一致，也不影响另一档的记分板落盘（`--no-fail-fast` 的双保险）。
  const run = (t) =>
    new Promise((res, rej) => {
      const c = spawn(
        'cargo',
        ['test', '-p', 'forge-tests', '--test', t, '--no-fail-fast', '--', '--nocapture'],
        { cwd: REPO, stdio: 'inherit', shell: process.platform === 'win32' },
      );
      c.on('error', rej);
      c.on('close', () => res());
    });
  await Promise.all([run('asm_parse'), run('asm_encoding')]);
}

/** 逐文件计数（记分板 → Map<"<isa>/<文件>", row>）。 */
function readScores() {
  const out = new Map();
  for (const isa of ['x86', 'riscv64', 'aarch64']) {
    const p = path.join(SCORES, `${isa}.json`);
    if (!fs.existsSync(p)) continue;
    const j = JSON.parse(fs.readFileSync(p, 'utf8'));
    for (const s of j.suites) for (const row of s.file_rows ?? []) out.set(`${j.isa}/${path.basename(row.file)}`, row);
  }
  return out;
}

/** 编码对拍逐文件计数（Map<"<isa>/<文件>", {cases,checked,unparsed,variants,known}>）。 */
function readEncodingScores() {
  const p = path.join(SCORES, 'encoding-files.json');
  return fs.existsSync(p) ? new Map(Object.entries(JSON.parse(fs.readFileSync(p, 'utf8')))) : new Map();
}

const kb = (n) => (n / 1024).toFixed(0) + 'KB';
const sha = (b) => crypto.createHash('sha256').update(b).digest('hex');
const table = (rows) => rows.map((r) => '| ' + r.join(' | ') + ' |').join('\n');
const countLines = (b) => {
  // 与 Rust 的 `str::lines().count()` 同口径：数换行，尾随换行不算多一行，
  // 末尾没换行的那半行算一行。
  const t = b.toString('utf8');
  return t.split('\n').length - (t.endsWith('\n') ? 1 : 0);
};

function provenance(rows, stats) {
  const [llvm, manual] = [rows.filter((r) => r.kind === 'llvm'), rows.filter((r) => r.kind === 'manual')];
  return `# 语料出处（\`asm/\`）

本文件由 \`crates/tools/forge-tests/asm/fetch.mjs\` **生成**（采集日期：${new Date().toISOString().slice(0, 10)}，
ref \`${REF}\`）——请勿手改；重跑脚本即可刷新。方法、取舍判据与已知缺口见
[\`README.md\`](README.md)。

三条必须知道的：

1. **落盘文件 = 上游逐字原文**（唯一例外见下表「处理」列的节选标注）；sha256 是
   **本仓库落盘件**的摘要（用来发现后续改动，不是上游文件的摘要）。仓库根
   \`.gitattributes\` 把 \`crates/tools/forge-tests/asm/**\` 钉成 \`text eol=lf\`，
   否则 \`core.autocrlf\` 在 Windows 上会改工作区、摘要立刻对不上。
2. **取舍是算出来的**：本次拉了上游 **${stats.candidates}** 个候选（顶层 \`.s\`；x86 只取
   Intel 语法的文件名那批），按判据留下 **${stats.kept}** 个、砍掉 **${stats.dropped}** 个
   （判据 = 解析档「整条接得住且不产生红桶」+ 编码对拍档「没有字节差异」，两份逐文件记分板
   在 \`target/asm-suite/\`）。被砍掉的那些不是"忘了"，而是**本谱现在还不认识**（SVE/SME/
   AVX-512/压缩指令/带符号 CSR 名/重定位表达式…）——它们会让红桶变成"永久红"，门禁就废了。
   谱长本事之后重跑脚本，会自动补回来。目录里另有 ${rows.length - stats.kept - manual.length} 个
   不在本次候选里的 LLVM 文件（子目录手工取的 \`apx/\` 那份）与 ${manual.length} 个手工来源，
   一并登记在下面。
3. \`asm/exec/**\`、\`asm/ratchet/**\` 是本仓库自己写的，不在下表。

## LLVM MC（${llvm.length} 份）

| 路径 | 上游 | 固定 ref | 许可 | 字节 | 行 | 摘要（sha256） | 处理 |
| --- | --- | --- | --- | --- | --- | --- | --- |
${table(llvm.map((r) => [
    '`parse/' + r.local + '`',
    `[${DLLLM} \`${r.upstream}\`](https://github.com/${DLLLM}/blob/${REF}/${r.upstream})`,
    `\`${REF}\``,
    `\`${r.license}\``,
    r.bytes,
    r.lines,
    '`' + r.sha256 + '`',
    '**整文件**，逐字未改',
  ]))}

## 手工来源（${manual.length} 份）

| 路径 | 上游 | 固定 ref | 许可 | 字节 | 行 | 摘要（sha256） | 处理 |
| --- | --- | --- | --- | --- | --- | --- | --- |
${table(manual.map((r) => [
    '`parse/' + r.local + '`',
    `[${r.repo} \`${r.upstreamPath}\`](https://sourceware.org/git/?p=binutils-gdb.git;a=blob;f=${r.upstreamPath})（镜像 [\`${r.repo}\`](https://github.com/${r.repo})）`,
    `\`${r.ref.slice(0, 12)}\``,
    `\`${r.license}\``,
    r.bytes,
    r.lines,
    '`' + r.sha256 + '`',
    r.note ?? '**整文件**，逐字未改',
  ]))}

## 许可与再分发

各份沿用上游许可：LLVM MC = \`Apache-2.0 WITH LLVM-exception\`，GAS = \`GPL-3.0-or-later\`。
仓库根**没有**统一 \`LICENSE\`/\`COPYING\`，所以 GAS 那份单独放在
\`parse/x86/gnu-gas-intel/\` 下，与本文一起构成出处与许可记录；若对外发行，请连同上游许可
正文一并带出，或把该目录换成 \`Apache-2.0\`/\`BSD-2-Clause\` 的语料。

## 有意排除的语料

\`riscv-tests\` / \`riscv-arch-test\` 的 \`.S\` **不进解析档**：它们靠 C 预处理器
（\`encoding.h\`）、链接脚本与 \`tohost\`/spike 自检协议，正文还是 git 子模块；本汇编器
没有预处理器，执行通道也不是 \`tohost\`（见 \`README.md\`「边界」）。XED/NASM/YASM 的
字节期望型语料（\`.reference\`/\`.hex\`/golden）留给**编码对拍档**，解析档只吃 \`.s\`。
`;
}

// ── 主流程 ──────────────────────────────────────────────────────────────────
const sources = SOURCES.filter((s) => !ONLY || s.isa === ONLY);
const jobs = [];
for (const s of sources) jobs.push(...(await candidates(s)));
const manualJobs = MANUAL.filter((m) => !ONLY || m.local.startsWith(ONLY + '/'));

console.log(`计划：${jobs.length} 个上游文件（${RECURSIVE ? '含子目录' : '只顶层'}）+ ${manualJobs.length} 个手工来源`);
for (const s of sources) {
  const mine = jobs.filter((j) => j.isa === s.isa);
  console.log(`  ${s.isa}/${s.key}: ${mine.length} 个候选 ${kb(mine.reduce((a, b) => a + b.size, 0))} ← ${s.upstream}${s.isa === 'x86' ? '（只取 Intel 语法文件名）' : ''}`);
}
if (LIST) process.exit(0);

if (!NO_FETCH) {
  let i = 0;
  const stats = { fetch: 0, skip: 0 };
  const worker = async () => {
    while (i < jobs.length) {
      const f = jobs[i++];
      stats[await download(f)]++;
    }
  };
  await Promise.all(Array.from({ length: 12 }, worker));
  console.log(`下载完成：新取 ${stats.fetch}，已有同大小 ${stats.skip}`);
  for (const m of manualJobs) {
    const dest = path.join(PARSE, m.local);
    const body = await (await api(manualUrl(m))).text();
    const buf = Buffer.from(body.split('\n').slice(0, m.takeLines).join('\n') + '\n');
    if (sha(buf) !== m.sha256) throw new Error(`${m.local}: 与登记的 sha256 不符（上游 ${m.ref.slice(0, 12)} 变了？）`);
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.writeFileSync(dest, buf);
    console.log(`手工来源已刷新：${m.local}（前 ${m.takeLines} 行，sha256 与登记一致）`);
  }
}

let stats = { candidates: jobs.length, kept: jobs.length, dropped: 0 };
if (!KEEP_ALL) {
  await score();
  const scores = readScores();
  const encScores = readEncodingScores();
  // 记分板缺档 ⇒ **拒绝裁剪**（否则会把语料删空——这条由 2026-10-06 的一次真实事故换来）。
  if (scores.size === 0) throw new Error(`记分板为空（${SCORES} 缺 file_rows）——拒绝裁剪`);
  const plan = [];
  for (const j of jobs) {
    const key = `${j.isa}/${j.local}`;
    const row = scores.get(key);
    const enc = encScores.get(key);
    const allow = ALLOWANCES[key] ?? 0;
    const why = !row
      ? 'not-scored'
      : row.parsed < 1
        ? 'parsed=0'
        : row.tail_mismatch > allow
          ? `tail=${row.tail_mismatch}`
          : enc && enc.cases > 0 && enc.known > 0
            ? `enc-known=${enc.known}`
            : null;
    if (why) plan.push({ key, why, dest: path.join(PARSE, j.dir, j.local) });
  }
  const kept = jobs.length - plan.length;
  if (jobs.length > 0 && kept === 0) throw new Error('取舍结果为空（一个文件都没留下）——拒绝裁剪');
  for (const d of plan) if (fs.existsSync(d.dest)) fs.unlinkSync(d.dest);
  const byIsa = {};
  for (const d of plan) byIsa[d.key.split('/')[0]] = (byIsa[d.key.split('/')[0]] ?? 0) + 1;
  const encBad = plan.filter((d) => d.why.startsWith('enc-'));
  const reds = plan.filter((d) => d.why.startsWith('tail='));
  console.log(`\n取舍：留 ${kept}，砍 ${plan.length}（${Object.entries(byIsa).map(([k, v]) => `${k} ${v}`).join('，')}）`);
  console.log(`  其中 parsed=0（本谱完全不认识）: ${plan.filter((d) => d.why === 'parsed=0').length}`);
  console.log(`  其中 有字节差异（上游是压缩/别的编码形式）: ${encBad.length}`);
  console.log(`  出红（未登记的解析缺口，要人看）: ${reds.length}${reds.length ? ' → ' + reds.slice(0, 6).map((d) => d.key + '(' + d.why + ')').join(', ') : ''}`);
  fs.writeFileSync(path.join(REPO, 'target/asm-dropped.json'), JSON.stringify(plan, null, 1));
  console.log('  逐条清单：target/asm-dropped.json');
  stats = { candidates: jobs.length, kept, dropped: plan.length };
}

// 出处登记：扫**落盘后**的实际文件（唯一事实源 = 磁盘）。上游路径由候选表/覆盖表给出——
// 目录里出现**来路不明**的语料文件就报错（不许静默登记一份不知道出处的文件）。
const upstreamOf = new Map(jobs.map((j) => [`${j.isa}/${j.local}`, j.upstream]));
for (const [k, v] of Object.entries(SUBDIR_OVERRIDES)) upstreamOf.set(k, v);
const rows = [];
for (const s of SOURCES) {
  const dir = path.join(PARSE, s.dir);
  if (!fs.existsSync(dir)) continue;
  for (const name of fs.readdirSync(dir).sort()) {
    if (!name.endsWith('.s')) continue;
    const key = s.dir + '/' + name;
    const up = upstreamOf.get(`${s.isa}/${name}`);
    if (!up) throw new Error(`${key}: 不在候选表里，也没有 SUBDIR_OVERRIDES 条目——出处不明，拒绝登记`);
    const buf = fs.readFileSync(path.join(dir, name));
    rows.push({ kind: 'llvm', local: key, upstream: up, license: s.license, bytes: buf.length, lines: countLines(buf), sha256: sha(buf) });
  }
}
for (const m of MANUAL) {
  const p = path.join(PARSE, m.local);
  if (!fs.existsSync(p)) continue;
  const buf = fs.readFileSync(p);
  rows.push({ kind: 'manual', ...m, bytes: buf.length, lines: countLines(buf), sha256: sha(buf) });
}
fs.writeFileSync(path.join(ASM, 'PROVENANCE.md'), provenance(rows, stats));
console.log(`\nPROVENANCE.md 已刷新：${rows.length} 行（LLVM ${rows.filter((r) => r.kind === 'llvm').length} + 手工 ${rows.filter((r) => r.kind === 'manual').length}）`);
console.log('\n下一步（**必须看 diff**）：');
console.log('  $env:FORGE_ASM_WRITE_RATCHET = "1"; cargo test -p forge-tests --test asm_parse');
console.log('  然后：cargo test -p forge-tests --test asm_parse --test asm_encoding');
