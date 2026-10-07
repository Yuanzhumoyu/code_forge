// ??? CHECK ????? NEON ???????????????????? VEC2R
// ??? abs/neg ???????? `0 Q U 01110 size 1 0000 c [15:10] Rn Rd`?
// ?????? `.8b` ?????? (U, c, d)??????? (Q, size)?
import fs from "node:fs";
const SPEC = "isa/arm64.toml";
const CORPUS = "crates/tools/forge-tests/asm/parse/aarch64/llvm-mc/arm64-advsimd.s";
const FAMILIES = ["cnt", "mvn", "cls", "clz", "rev16", "rev32", "rev64"];
const ARR = [["8b",0,0],["16b",1,0],["4h",0,1],["8h",1,1],["2s",0,2],["4s",1,2]];

// 1) ???asm, bytes???????????
const pairs = [];
for (const raw of fs.readFileSync(CORPUS, "utf8").split(/\r?\n/)) {
  const m = raw.trim().match(/CHECK[^:]*:\s*([a-z][a-z0-9.]*)\s+(.*?)encoding:\s*\[([^\]]*)\]/);
  if (!m) continue;
  const toks = m[3].split(",").map((t) => t.trim());
  if (toks.length !== 4 || toks.some((t) => !/^0[xX][0-9a-fA-F]+$/.test(t))) continue;
  const bytes = toks.map((t) => parseInt(t, 16));
  const word = bytes[3] * 0x1000000 + bytes[2] * 0x10000 + bytes[1] * 0x100 + bytes[0];
  pairs.push({ asm: (m[1] + " " + m[2]).trim().replace(/\s+/g, " "), word });
}
// 2) ??? (U, c, d)??? `.8b` ???
const fam = {};
for (const f of FAMILIES) {
  const hit = pairs.find((p) => p.asm.startsWith(f + ".8b "));
  if (!hit) throw new Error("?? " + f + ".8b ????");
  fam[f] = { u: (hit.word >> 29) & 1, c: (hit.word >> 21) & 1, d: (hit.word >> 10) & 0x3f };
}
// 3) ???? + ????????????????????
const ins = [], vec = [];
for (const f of FAMILIES) {
  const { u, c, d } = fam[f];
  const lines = pairs.filter((p) => p.asm.startsWith(f + ".")).map((p) => p.asm.split(" ")[0]);
  ins.push(`# ${f}: (U=${u}, c=${c}, [15:10]=${d}) ?? ??? .8b ?????????? Q/size`);
  for (const [arr, q, size] of ARR) {
    const name = f.toUpperCase().replace(/[^A-Z0-9]/g, "") + arr.toUpperCase();
    const word = 0x0e000000 | (q << 30) | (u << 29) | (size << 22) | (c << 21) | (d << 10);
    const bytes = [word & 0xff, (word >> 8) & 0xff, (word >> 16) & 0xff, (word >> 24) & 0xff]
      .map((b) => "0x" + b.toString(16).toUpperCase().padStart(2, "0")).join(", ");
    ins.push(`[[instructions]]`, `name = "${name}"`, `form = "VEC2R"`, `opcode = 0x0E`,
      `fields = { vq = ${q}, vu = ${u}, vsize = ${size}, vec_c = ${c}, vec_d = ${d}, rm = 0 }`,
      `ops = ["dst:fpr:out", "src:fpr"]`, `asm = "${f}.${arr} {dst}, {src}"`, "");
    if (lines.includes(`${f}.${arr}`)) {
      vec.push(`[[vectors]]`, `asm = "${f}.${arr} v0, v0"`, `bytes = [${bytes}]`, "");
    }
  }
}
// 4) ???ASCII ?????????? ASCII ?????? ?????
let t = fs.readFileSync(SPEC, "utf8");
const eol = t.includes("\r\n") ? "\r\n" : "\n";
const insAnchor = 'asm = "neg.4s {dst}, {src}"';
if (!t.includes(insAnchor)) throw new Error("??????");
t = t.replace(insAnchor, insAnchor + eol + eol + ins.join(eol).trimEnd());
const vAnchor = "bytes = [0x00, 0xB8, 0xA0, 0x6E]";
if (!t.includes(vAnchor)) throw new Error("??????");
t = t.replace(vAnchor, vAnchor + eol + eol + vec.join(eol).trimEnd());
fs.writeFileSync(SPEC, t);
const n = ins.filter((l) => l.startsWith('name = "')).length;
console.log(`families=${FAMILIES.length} instructions=${n} vectors=${vec.filter((l) => l.startsWith("asm = ")).length}`);
console.log("derived: " + FAMILIES.map((f) => `${f}(U=${fam[f].u},c=${fam[f].c},d=${fam[f].d})`).join(" "));
