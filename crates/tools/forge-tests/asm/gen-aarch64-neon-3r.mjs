// NEON ??????add/sub/and/orr/eor/???**?????????**?????????
// ?? `0 Q U 01110 size 1 Rm 1 0000 1 Rn Rd` ? ?????? vec_a[28:24]/vec_c[21]/
// vec_d[15:10]/vq[30]/vu[29]/vsize[23:22]???? 3 ??? form?rt/rn/rm??
import fs from "node:fs";
const SPEC = "isa/arm64.toml";
const CORPUS = "crates/tools/forge-tests/asm/parse/aarch64/llvm-mc/arm64-advsimd.s";
const WANT = ["add", "sub", "and", "orr", "eor", "bic", "orn", "eon", "smax", "smin", "umax", "umin", "mul", "mla"];
const ARR6 = ["8b", "16b", "4h", "8h", "2s", "4s"];

const pairs = [];
for (const raw of fs.readFileSync(CORPUS, "utf8").split(/\r?\n/)) {
  const m = raw.trim().match(/CHECK[^:]*:\s*([a-z][a-z0-9.]*)\s+(.*?)encoding:\s*\[([^\]]*)\]/);
  if (!m) continue;
  const toks = m[3].split(",").map((t) => t.trim());
  if (toks.length !== 4 || toks.some((t) => !/^0[xX][0-9a-fA-F]+$/.test(t))) continue;
  const b = toks.map((t) => parseInt(t, 16));
  const asm = (m[1] + " " + m[2]).replace(/[;#].*$/, "").replace(/\s+/g, " ").trim();
  const ops = asm.split(/\s+/).slice(1).join(" ").split(",").map((s) => s.trim()).filter(Boolean);
  if (ops.length !== 3) continue;                       // three-register form only
  if (ops.some((o) => o.includes("["))) continue;       // lane forms (`v0[1]`) are a different family
  const [mnem, arr] = m[1].split(".");
  if (!WANT.includes(mnem) || !ARR6.includes(arr)) continue;
  pairs.push({ asm, bytes: b, mnem, arr, word: b[3] * 0x1000000 + b[2] * 0x10000 + b[1] * 0x100 + b[0] });
}
// ? (mnem, arr) ???????????
const seen = new Map();
for (const p of pairs) {
  const k = p.mnem + "." + p.arr;
  if (!seen.has(k)) seen.set(k, p);
}
const ins = [], vec = [];
const fams = [...new Set([...seen.values()].map((p) => p.mnem))];
for (const fam of fams) {
  ins.push(`# ${fam}????????????????`);
  for (const arr of ARR6) {
    const p = seen.get(fam + "." + arr);
    if (!p) continue;
    const q = (p.word >> 30) & 1, u = (p.word >> 29) & 1, size = (p.word >> 22) & 3;
    const c = (p.word >> 21) & 1, d = (p.word >> 10) & 0x3f;
    const name = ("V" + fam.toUpperCase() + arr.toUpperCase()).replace(/[^A-Z0-9]/g, "");
    ins.push(`[[instructions]]`, `name = "${name}"`, `form = "VEC3R"`, `opcode = 0x0E`,
      `fields = { vq = ${q}, vu = ${u}, vsize = ${size}, vec_c = ${c}, vec_d = ${d} }`,
      `ops = ["dst:fpr:out", "src:fpr", "src2:fpr"]`, `asm = "${fam}.${arr} {dst}, {src}, {src2}"`, "");
    // ? (?,??) ?**??**????????
    for (const q2 of pairs.filter((x) => x.mnem === fam && x.arr === arr)) {
      vec.push(`[[vectors]]`, `asm = "${q2.asm}"`, `bytes = [${q2.bytes.map((x) => "0x" + x.toString(16).toUpperCase().padStart(2, "0")).join(", ")}]`, "");
    }
  }
}
let t = fs.readFileSync(SPEC, "utf8");
const eol = t.includes("\r\n") ? "\r\n" : "\n";
const fAnchor = 'operand_fields = ["rt", "rn"]';
if (!t.includes(fAnchor)) throw new Error("form ????");
t = t.replace(fAnchor, [fAnchor, "", "[[forms]]", 'name = "VEC3R"   # NEON ?????Rd, Rn, Rm?', 'opcode_field = "vec_a"', 'operand_fields = ["rt", "rn", "rm"]'].join(eol));
const iAnchor = 'asm = "neg.4s {dst}, {src}"';
if (!t.includes(iAnchor)) throw new Error("??????");
t = t.replace(iAnchor, iAnchor + eol + eol + ins.join(eol).trimEnd());
const vAnchor = "bytes = [0x00, 0xB8, 0xA0, 0x6E]";
if (!t.includes(vAnchor)) throw new Error("??????");
t = t.replace(vAnchor, vAnchor + eol + eol + vec.join(eol).trimEnd());
fs.writeFileSync(SPEC, t);
console.log(`fams=${fams.length} instructions=${ins.filter((l) => l.startsWith('name = "')).length} vectors=${vec.filter((l) => l.startsWith("asm = ")).length}`);
console.log("fams: " + fams.join(" "));
