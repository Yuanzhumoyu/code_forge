//! 段体压缩：**零依赖、确定性**的 LZ77 变体（自研，不进任何第三方 crate）。
//!
//! # 为什么自研而不是引 zstd/flate2
//!
//! 二进制层的既有承诺是"零新依赖"（`docs/plans/forge-ir-binary-serialization-plan.md` §7），
//! 而压缩不是这条格式的语义要求——它只是**省字节**。因此这里的取舍是：自己写一个
//! 小而可审计的字节级压缩器，把"确定性 + fail-closed + 零依赖"三条都保住；
//! 若将来需要更高压缩率，可以**按段替换**为外部实现（段表已带 `raw_len`，
//! 格式不需要改），届时再单独评估依赖。
//!
//! # 编码（token 流）
//!
//! ```text
//! 重复直到输入耗尽：
//!   header = varint
//!     header 为偶：字面量段，长度 = header / 2，随后跟同样多的原始字节
//!     header 为奇：回引段，长度 = header / 2 + MIN_MATCH，随后跟 varint(dist - 1)
//! ```
//!
//! 长度用 `header >> 1` 存**增量**（匹配段减去 `MIN_MATCH`），因此小 token 只占 1 字节。
//! 窗口 64 KiB、`MIN_MATCH = 4`、单 token 最长 [`MAX_MATCH`]；匹配用固定哈希表 +
//! 链式前驱、贪心取最长、**候选链长上限**（防退化输入退化成 O(n²)）。
//! 全程无随机、无哈希迭代序依赖 ⇒ 同输入必同输出。
//!
//! # 成本（为什么有 [`Packer`] 与自适应表长）
//!
//! 压缩在**编码热路径**上，所以表不能"每段都新建一张 256 KiB 的哈希表再清零"：
//! 段有 8 个，而中等模块的段体大多只有几十到几百字节。两条对策，都不改变格式、
//! 也不改变"同输入同输出"：哈希表长按输入规模自适应（`1 << 12` 起，至多 `1 << 16`），
//! 且哈希表与链缓冲由 [`Packer`] 跨段复用（只清哈希表，链缓冲只在变长时 `resize`）。
//! 太短的段体（< [`MIN_PACK_INPUT`]）只发一个字面量 token——不建表。
//!
//! 实测（2026-09-19，`cargo bench -p forge-ir --bench ir_binary`，同一中等模块）：
//! 每段新建一张 256 KiB 表的版本 encode = **184.1 µs**；改为 scratch 跨段复用 + 表长随
//! 输入自适应后回到"不压缩基线"量级（v1 基线 21.9 µs，v2 三次运行 22.1 / 30.5 / 22.5 µs）
//! ⇒ **压缩的编码成本落在计时噪声内**。这条实测也说明本机单点波动可达 ~1.7×（同一版
//! 代码曾测得 45.2 µs），所以性能结论要看多次运行的方向，不看单点数字。
//!
//! # 解码纪律（fail-closed）
//!
//! - 距离必须 `1..=已产出字节数`（越界 = 坏数据，不"猜"）；
//! - 每个 token 的长度不得超过"还差多少字节到 `raw_len`"（防越写）；
//! - 字面量段的字节必须在输入内（越界即错）；
//! - 结束时必须**恰好**产出 `raw_len` 字节，且**输入恰好耗尽**（尾随字节 = 坏数据）。

/// 匹配段的最小长度（短于它的重复不值得一个 token）。
pub(crate) const MIN_MATCH: usize = 4;
/// 单个匹配段的最大长度（超出就再发一个 token，保证 span 可装进 varint）。
pub(crate) const MAX_MATCH: usize = 1 << 16;
/// 回看窗口（字节）。距离上限 = 本值。
pub(crate) const WINDOW: usize = 1 << 16;
/// 哈希位数下限（表长 = 1 << bits）：小段体不该付 256 KiB 的建表代价。
///
/// 表长随输入长度增长（见 [`hash_bits_for`]）。实测（2026-09-19，`--bench ir_binary`，
/// 同条件前后两轮）把下限从 12 降到 10 **没有可测收益**（encode 45.2 µs → 45.2 µs，
/// 语料体积 121,257 B → 121,249 B），说明自适应建表之后剩下的开销是**搜索本身**而不是
/// 清零；因此这里保守取 12（表长与输入量级相称），不再往下调。
const HASH_BITS_MIN: u32 = 12;
/// 哈希位数上限（与 [`WINDOW`] 同量级即可，再大只是浪费清零时间）。
const HASH_BITS_MAX: u32 = 16;
/// 短于该长度的段体**不压**：token 头 + 距离字段的最小开销注定压不小。
const MIN_PACK_INPUT: usize = 32;
/// 同哈希值最多回溯多少个候选（防最坏情况退化；确定性：只看前 N 个）。
const MAX_CHAIN: usize = 64;
/// 解码侧的**单段解压上限**（资源上限，不是格式语义限制）。
///
/// 段体解压后不得超过 256 MiB：写侧要产出这么大的段体，本身就得先在内存里持有一份
/// 同样大的 IR 段体，所以合法数据远在界内（语料最大模块的字节流 31 KB）。这条
/// 上限存在的唯一理由是**不解压炸弹**：35 字节的输入不能要求解码器造出 4 GiB。
pub(crate) const MAX_SECTION_RAW_BYTES: usize = 256 << 20;
/// 解码侧的**压缩比上限**（早退用）：`raw_len > packed_len * RATIO + SLACK` 即拒绝。
///
/// 合法的最大比值约 `MAX_MATCH / 3 ≈ 2.2 万`（整段单字节重复：1 字节字面量 + 每个
/// 65536 字节匹配只花 2–3 字节），所以 65536× 不会误伤任何写侧产物，却能立刻挡掉
/// "几字节输入声称几百 MiB" 的流。
pub(crate) const MAX_PACK_RATIO: usize = 1 << 16;
/// 见 [`MAX_PACK_RATIO`] 的常数项（小段体也不至于被误判）。
pub(crate) const PACK_RATIO_SLACK: usize = 4096;

// ============================================================
// 压缩
// ============================================================

#[inline]
fn hash3(data: &[u8], i: usize, bits: u32) -> usize {
    // 3 字节滚动哈希（乘法混合，确定性）；`bits` 由输入规模决定 ⇒ 同输入同表长。
    let a = u32::from(data[i]);
    let b = u32::from(data[i + 1]);
    let c = u32::from(data[i + 2]);
    let h = (a << 16) ^ (b << 8) ^ c;
    (h.wrapping_mul(0x9E37_79B1) >> (32 - bits)) as usize
}

/// 输入规模 → 哈希位数（单调，且只依赖长度 ⇒ 确定性）。
fn hash_bits_for(n: usize) -> u32 {
    let mut bits = HASH_BITS_MIN;
    while (1usize << bits) < n && bits < HASH_BITS_MAX {
        bits += 1;
    }
    bits
}

/// 压缩器 scratch：哈希表 + 链缓冲跨段复用（模块编码器一个实例压全部段）。
pub(crate) struct Packer {
    /// `head[hash]` = 最近一次出现该 3 字节前缀的位置。
    head: Vec<u32>,
    /// `prev[i]` = 上一个同哈希位置。
    prev: Vec<u32>,
    /// 当前表长对应的位数（`0` = 还没建表）。
    bits: u32,
}

impl Packer {
    pub(crate) fn new() -> Self {
        Self {
            head: Vec::new(),
            prev: Vec::new(),
            bits: 0,
        }
    }

    /// 压缩一段（无随机、无时间/内存依赖 ⇒ 同输入同输出）。
    ///
    /// **输出恒为合法的 packed 流**（`unpack(pack(x), x.len()) == x`）；短于
    /// [`MIN_PACK_INPUT`] 的输入不建表，只发一个字面量 token——它的体积必然大于原文，
    /// 于是调用方的"严格更小才用"决策自然把这类段体原样存。
    pub(crate) fn pack(&mut self, input: &[u8]) -> Vec<u8> {
        let n = input.len();
        if n < MIN_PACK_INPUT {
            let mut out = Vec::with_capacity(n + 8);
            flush_literals(&mut out, input);
            return out;
        }
        let bits = hash_bits_for(n);
        if self.bits == bits {
            self.head.fill(u32::MAX);
        } else {
            self.head.clear();
            self.head.resize(1usize << bits, u32::MAX);
            self.bits = bits;
        }
        // 链缓冲复用：只保证长度够；`prev` 的下标只在本轮写入后才会被读到
        // （候选位置一律来自本轮清过的 `head`），故无需清零。
        self.prev.clear();
        self.prev.resize(n, u32::MAX);

        let head = &mut self.head;
        let prev = &mut self.prev;
        let mut out = Vec::with_capacity(n / 2 + 16);
        let mut lit_start = 0usize; // 待输出的字面量段起点
        let mut i = 0usize;
        while i < n {
            let mut best_len = 0usize;
            let mut best_dist = 0usize;
            if i + MIN_MATCH <= n {
                let h = hash3(input, i, bits);
                let mut cand = head[h];
                let mut chain = 0usize;
                while cand != u32::MAX && chain < MAX_CHAIN {
                    let c = cand as usize;
                    let dist = i - c;
                    if dist == 0 || dist > WINDOW {
                        break;
                    }
                    // 贪心取最长匹配
                    let max_len = (n - i).min(MAX_MATCH);
                    let mut len = 0usize;
                    while len < max_len && input[c + len] == input[i + len] {
                        len += 1;
                    }
                    if len > best_len {
                        best_len = len;
                        best_dist = dist;
                        if len == max_len {
                            break; // 已到本位置上限，再找也不会更好
                        }
                    }
                    cand = prev[c];
                    chain += 1;
                }
            }
            if best_len >= MIN_MATCH {
                // 先冲掉待输出的字面量
                flush_literals(&mut out, &input[lit_start..i]);
                let header = ((best_len - MIN_MATCH) << 1) | 1;
                put_varint(&mut out, header as u64);
                put_varint(&mut out, (best_dist - 1) as u64);
                // 登记沿途位置的哈希（保证后续匹配能看见它们）
                let end = i + best_len;
                while i < end {
                    if i + MIN_MATCH <= n {
                        let h = hash3(input, i, bits);
                        prev[i] = head[h];
                        head[h] = i as u32;
                    }
                    i += 1;
                }
                lit_start = i;
            } else {
                // 未命中：登记该位置并前进 1 字节
                if i + MIN_MATCH <= n {
                    let h = hash3(input, i, bits);
                    prev[i] = head[h];
                    head[h] = i as u32;
                }
                i += 1;
            }
        }
        flush_literals(&mut out, &input[lit_start..n]);
        out
    }
}

/// 一次性压缩（单测与"只压一段"的场景用；模块编码器走 `Packer` 复用 scratch）。
#[cfg(test)]
pub(crate) fn pack(input: &[u8]) -> Vec<u8> {
    Packer::new().pack(input)
}

fn flush_literals(out: &mut Vec<u8>, lits: &[u8]) {
    if lits.is_empty() {
        return;
    }
    put_varint(out, (lits.len() as u64) << 1); // 偶数 = 字面量段
    out.extend_from_slice(lits);
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

// ============================================================
// 解压
// ============================================================

/// 解压的**只读游标**（与 `reader::Cursor` 同纪律：越界即错，不 panic、不预分配不可信长度）。
struct In<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> In<'a> {
    fn varint(&mut self) -> Result<u64, String> {
        let mut result = 0u64;
        let mut shift = 0u32;
        loop {
            if shift > 63 {
                return Err("packed: varint 超过 64 位".to_string());
            }
            let b = *self
                .bytes
                .get(self.pos)
                .ok_or_else(|| "packed: 读取越界（文件已结束）".to_string())?;
            self.pos += 1;
            result |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if n > self.bytes.len() - self.pos {
            return Err(format!(
                "packed: 字面量段需要 {n} 字节，只剩 {}",
                self.bytes.len() - self.pos
            ));
        }
        let s = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
}

/// 解压到**恰好** `raw_len` 字节；任何不一致都是 `Err`（带原因，不 panic）。
///
/// 调用方负责先把 `raw_len` 过一遍 [`check_ratio`]（防解压炸弹）。
pub(crate) fn unpack(packed: &[u8], raw_len: usize) -> Result<Vec<u8>, String> {
    check_ratio(packed.len(), raw_len)?;
    // 按需增长（至多预分配 64 KiB）：声明长度已在 check_ratio 里过了上限，
    // 但仍然不按它一次性分配。
    let mut out = Vec::with_capacity(raw_len.clamp(16, 1 << 16));
    let mut input = In {
        bytes: packed,
        pos: 0,
    };
    while out.len() < raw_len {
        let header = input.varint()?;
        let remaining = raw_len - out.len();
        if header & 1 == 0 {
            // 字面量段
            let len = (header >> 1) as usize;
            if len == 0 {
                return Err("packed: 字面量段长度为 0（空 token 非法）".to_string());
            }
            if len > remaining {
                return Err(format!(
                    "packed: 字面量段 {len} 字节超过还差的 {remaining} 字节"
                ));
            }
            out.extend_from_slice(input.take(len)?);
        } else {
            // 回引段
            let len = ((header >> 1) as usize) + MIN_MATCH;
            if len > remaining {
                return Err(format!(
                    "packed: 回引段 {len} 字节超过还差的 {remaining} 字节"
                ));
            }
            let dist = input.varint()? as usize + 1;
            if dist == 0 || dist > out.len() {
                return Err(format!(
                    "packed: 回引距离 {dist} 越界（已产出 {} 字节）",
                    out.len()
                ));
            }
            // 逐字节拷贝（允许重叠：`dist < len` 时这是 LZ77 的游程语义）
            let start = out.len() - dist;
            for src in start..start + len {
                let b = out[src];
                out.push(b);
            }
        }
    }
    if out.len() != raw_len {
        return Err(format!(
            "packed: 解压得到 {} 字节，声明 {raw_len}",
            out.len()
        ));
    }
    if input.pos != packed.len() {
        return Err(format!(
            "packed: 解压后有 {} 字节尾随数据（坏流）",
            packed.len() - input.pos
        ));
    }
    Ok(out)
}

/// 解压前的大小检查（防解压炸弹）：见 [`MAX_SECTION_RAW_BYTES`] / [`MAX_PACK_RATIO`]。
pub(crate) fn check_ratio(packed_len: usize, raw_len: usize) -> Result<(), String> {
    if raw_len > MAX_SECTION_RAW_BYTES {
        return Err(format!(
            "packed: 声明解压后 {raw_len} 字节，超过单段上限 {MAX_SECTION_RAW_BYTES} 字节\
             （资源上限，拒绝解压）"
        ));
    }
    if raw_len > packed_len.saturating_mul(MAX_PACK_RATIO) + PACK_RATIO_SLACK {
        return Err(format!(
            "packed: 声明解压后 {raw_len} 字节，但压缩体只有 {packed_len} 字节\
             （超过 {MAX_PACK_RATIO}× 安全上限，拒绝解压）"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(data: &[u8]) {
        let packed = pack(data);
        let back = unpack(&packed, data.len()).unwrap_or_else(|e| {
            panic!(
                "解压失败（raw {} B / packed {} B）：{e}",
                data.len(),
                packed.len()
            )
        });
        assert_eq!(back, data, "往返不一致（raw {} B）", data.len());
    }

    #[test]
    fn roundtrips_pathological_inputs() {
        roundtrip(b"");
        roundtrip(b"a");
        roundtrip(b"ab");
        roundtrip(b"abc"); // 短于 MIN_MATCH：必须走字面量
        roundtrip(b"abcd");
        roundtrip(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // 单字节长游程
        roundtrip(&[0u8; 4096]);
        roundtrip(&[0xffu8; 100_000]); // 超过窗口
        // 重复模式（跨窗口边界）
        let pattern: Vec<u8> = (0..=255u8).cycle().take(200_000).collect();
        roundtrip(&pattern);
        // 伪随机（几乎不可压缩；写侧会因此放弃压缩，但解压必须仍正确）
        let mut s = 0x1234_5678_9abc_def0u64;
        let rnd: Vec<u8> = (0..50_000)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 24) as u8
            })
            .collect();
        roundtrip(&rnd);
        // 长匹配（触及 MAX_MATCH 上限，需要多个 token）
        let mut long = vec![b'x'; MAX_MATCH + 64];
        long.extend_from_slice(&[b'y'; 8]);
        long.extend_from_slice(&vec![b'x'; MAX_MATCH + 64]);
        roundtrip(&long);
    }

    #[test]
    fn packed_really_shrinks_repetitive_data() {
        // 重复结构（IR 字节流的典型形态：小整数 + 重复的操作码名索引）
        let mut data = Vec::new();
        for i in 0..2000u32 {
            data.extend_from_slice(&(i % 7).to_le_bytes()[..2]);
            data.extend_from_slice(b"iadd\0");
        }
        let packed = pack(&data);
        assert!(
            packed.len() * 3 < data.len(),
            "重复数据至少应压到 1/3（实测 {} → {}）",
            data.len(),
            packed.len()
        );
        roundtrip(&data);
    }

    #[test]
    fn decoder_rejects_bad_input() {
        // 距离越界（回引到尚未产出的字节）
        let mut bad = Vec::new();
        put_varint(&mut bad, 1); // header 奇 = 回引，len = MIN_MATCH
        put_varint(&mut bad, 5); // dist = 6 > 已产出 0
        let e = unpack(&bad, 8).expect_err("距离越界必须错");
        assert!(e.contains("回引距离"), "{e}");

        // 字面量段越过声明长度
        let mut bad = Vec::new();
        put_varint(&mut bad, 10u64 << 1); // 字面量 10 字节
        bad.extend_from_slice(b"1234567890");
        let e = unpack(&bad, 4).expect_err("字面量超过 raw_len 必须错");
        assert!(e.contains("超过还差"), "{e}");

        // 截断的字面量
        let mut bad = Vec::new();
        put_varint(&mut bad, 8u64 << 1);
        bad.extend_from_slice(b"1234");
        let e = unpack(&bad, 8).expect_err("截断必须错");
        assert!(e.contains("只剩"), "{e}");

        // 尾随数据（`pack` 的输出恒为合法 packed 流，故可直接在末尾塞脏字节）
        let mut bad = pack(&[b'a'; 64]);
        bad.push(0);
        let e = unpack(&bad, 64).expect_err("尾随数据必须错");
        assert!(e.contains("尾随"), "{e}");

        // 空 token（长度 0 的字面量）
        let bad = vec![0u8];
        let e = unpack(&bad, 1).expect_err("空字面量必须错");
        assert!(e.contains("字面量段长度为 0"), "{e}");

        // 未产出足量字节就结束
        let e = unpack(&[], 4).expect_err("空输入配非零 raw_len 必须错");
        assert!(e.contains("越界"), "{e}");

        // 解压炸弹：声称 1 MiB 但只有 3 字节
        let e = unpack(&[0u8, 1, 2], 1 << 20).expect_err("超过安全上限必须错");
        assert!(e.contains("安全上限"), "{e}");
    }

    #[test]
    fn varint_helpers_are_leb128() {
        let mut out = Vec::new();
        put_varint(&mut out, 300);
        assert_eq!(out, vec![0xac, 0x02]);
    }
}
