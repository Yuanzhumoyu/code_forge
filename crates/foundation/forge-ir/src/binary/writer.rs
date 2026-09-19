//! 二进制**写侧**：编码原语 + 字符串表 + 段装配。
//!
//! 两条纪律（与 [`super::reader`] 对偶）：
//!
//! 1. **确定性**：字符串按首次出现顺序去重，段按 [`SectionId`] 升序写；
//!    除写侧字符串表的 `HashMap`（只作索引、不参与输出顺序）外不遍历哈希容器。
//! 2. **段体先写、头部后拼**：字符串表要等其他段把遇到的 `ImmStr` 全部登记完
//!    才能落盘，因此 [`Writer::finish`] 才产出最终字节流（MLIR bytecode 同序）。

use std::collections::HashMap;

use crate::util::imm_str::ImmStr;
use crate::util::string_pool::InternedStr;

use super::format::{IR_FORMAT_VERSION, MAGIC, PRODUCER, SectionId};

// ============================================================
// 编码原语（小端字节序；长度一律 varint）
// ============================================================

/// 写 1 字节。
pub(crate) fn put_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}

/// 写 LEB128 无符号 varint。
pub(crate) fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// 写 zigzag + LEB128（负值不膨胀）。
///
/// B1 尚未有消费方（B2 起有符号常量用到），但原语与格式同批落地并自带单测，
/// 避免"用到才加"造成的格式漂移。
pub(crate) fn put_zigzag(out: &mut Vec<u8>, v: i64) {
    let zz = ((v << 1) ^ (v >> 63)) as u64;
    put_varint(out, zz);
}

/// 写 LEB128 无符号 varint（u128；最多 19 字节）。
pub(crate) fn put_varint_u128(out: &mut Vec<u8>, mut v: u128) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// 写 zigzag + LEB128（i128；常量池的整数常量用）。
pub(crate) fn put_zigzag_i128(out: &mut Vec<u8>, v: i128) {
    let zz = ((v << 1) ^ (v >> 127)) as u128;
    put_varint_u128(out, zz);
}

/// 写 varint 长度前缀 + 原始字节。
pub(crate) fn put_len_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// 写 varint 长度前缀 + UTF-8 字节。
pub(crate) fn put_str(out: &mut Vec<u8>, s: &str) {
    put_len_prefixed(out, s.as_bytes());
}

/// 写 `Option` 的显式判别位（`0` = 空，`1` = 有）。
///
/// 无"默认值"捷径：空与"零值"是两回事（例如 0 字节的向量 vs 没有向量）。
#[allow(dead_code)] // B1 尚未有消费方（B2+ 的 Option 字段用到），原语随骨架落地
pub(crate) fn put_option_tag(out: &mut Vec<u8>, present: bool) {
    put_u8(out, u8::from(present));
}

/// varint 编码长度（段表重算用）。
pub(crate) fn varint_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

// ============================================================
// 字符串表
// ============================================================

/// 写侧字符串表：按**首次出现顺序**去重（索引即顺序，无保留槽）。
///
/// 不预留"空串槽"：池里有空串就是普通一条，池为空则表长为 0——这样
/// `InternedStr(i)` 与表索引 `i` 在解码期恒等（池按表顺序重建），模块的池
/// **逐条不差**地往返（连空串都不多不少）。
///
/// `HashMap` 只用于"查是否已登记"，**不参与输出顺序**（输出顺序 = `order`）。
pub(crate) struct StringTableWriter {
    order: Vec<ImmStr>,
    index: HashMap<ImmStr, u32>,
}

impl StringTableWriter {
    fn new() -> Self {
        Self {
            order: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// 登记一个字符串，返回其索引。
    pub(crate) fn intern(&mut self, s: &ImmStr) -> u32 {
        if let Some(&idx) = self.index.get(s) {
            return idx;
        }
        let idx = self.order.len() as u32;
        self.order.push(s.clone());
        self.index.insert(s.clone(), idx);
        idx
    }

    /// 已登记字符串数（= 表长；池为空则为 0）。
    fn len(&self) -> usize {
        self.order.len()
    }

    /// 落盘：`num` + `num` 个长度 + 字节拼接（第 0 项长度恒为 0）。
    fn encode(&self, out: &mut Vec<u8>) {
        put_varint(out, self.order.len() as u64);
        for s in &self.order {
            put_varint(out, s.len() as u64);
        }
        for s in &self.order {
            out.extend_from_slice(s.as_str().as_bytes());
        }
    }
}

// ============================================================
// 段装配
// ============================================================

/// 模块编码器：各段体先写进自己的缓冲，[`Writer::finish`] 再拼头部 + 段表 + 段体。
pub(crate) struct Writer {
    strings: StringTableWriter,
    /// 类型库字符串池句柄 → 二进制字符串表索引（`pool_map[i]` = 池句柄 `i` 的索引）。
    pool_map: Vec<u32>,
    sections: Vec<(SectionId, Vec<u8>)>,
}

impl Writer {
    pub(crate) fn new() -> Self {
        Self {
            strings: StringTableWriter::new(),
            pool_map: Vec::new(),
            sections: Vec::new(),
        }
    }

    /// 取（必要时新建）某段的段体缓冲。
    pub(crate) fn section(&mut self, id: SectionId) -> &mut Vec<u8> {
        if let Some(pos) = self.sections.iter().position(|(sid, _)| *sid == id) {
            return &mut self.sections[pos].1;
        }
        self.sections.push((id, Vec::new()));
        &mut self.sections.last_mut().expect("刚 push").1
    }

    /// 装回某段的段体。
    ///
    /// 段编码器在**本地**缓冲里写完后调用：本地缓冲让"写段体字节"与"登记字符串
    /// （要 `&mut Writer`）"可以交错，避免同一时刻双重可变借用。
    pub(crate) fn assign_section(&mut self, id: SectionId, body: Vec<u8>) {
        *self.section(id) = body;
    }

    /// 登记字符串（所有 `ImmStr` 一律经这里落进 STRINGS 段）。
    pub(crate) fn intern(&mut self, s: &ImmStr) -> u32 {
        self.strings.intern(s)
    }

    /// 按类型库字符串池顺序整池登记，并记下"池句柄 → 表索引"的映射。
    ///
    /// 表不预留空串槽 ⇒ 新池（每个模块的池都从空开始）里 `pool_map[i] == i`，
    /// 解码侧同样恒等，模块的池逐条往返。各段写类型库句柄仍应经
    /// [`Writer::pool_string_index`] 换算（不假设恒等）。
    pub(crate) fn intern_pool(&mut self, pool: &[ImmStr]) {
        self.pool_map = pool.iter().map(|s| self.strings.intern(s)).collect();
        debug_assert!(
            self.pool_map.windows(2).all(|w| w[0] < w[1]),
            "池顺序登记后表索引必须严格递增"
        );
    }

    /// 类型库句柄 `InternedStr(i)` 在二进制字符串表里的索引。
    pub(crate) fn pool_string_index(&self, handle: InternedStr) -> Option<u32> {
        self.pool_map.get(handle.0 as usize).copied()
    }

    /// 已登记字符串数（含索引 0 的空串）。
    pub(crate) fn string_count(&self) -> usize {
        self.strings.len()
    }

    /// 组装完整字节流，**追加**到 `out`。
    pub(crate) fn finish(mut self, out: &mut Vec<u8>) {
        let start = out.len();
        // STRINGS 段由字符串表本身产生（即便只有空串也必须存在：读侧要求）。
        let mut strings_body = Vec::new();
        self.strings.encode(&mut strings_body);
        if let Some(pos) = self
            .sections
            .iter()
            .position(|(sid, _)| *sid == SectionId::Strings)
        {
            self.sections[pos].1 = strings_body;
        } else {
            self.sections.push((SectionId::Strings, strings_body));
        }
        self.sections.sort_by_key(|(id, _)| id.as_u8());

        // 段表里的 offset 是绝对偏移 = 头部长度 + 之前段体长度之和；而头部长度
        // 又取决于 offset 的 varint 长度 ⇒ 取不动点（偏移随头部长度单调，通常
        // 2 轮收敛；这里给 8 轮上限并在未收敛时 debug 断言）。
        let mut header_len = 0usize;
        let mut offsets = vec![0u64; self.sections.len()];
        for _ in 0..8 {
            let mut off = header_len as u64;
            let mut size = MAGIC.len()
                + varint_len(u64::from(IR_FORMAT_VERSION))
                + varint_len(PRODUCER.len() as u64)
                + PRODUCER.len()
                + varint_len(self.sections.len() as u64);
            for (i, (_, body)) in self.sections.iter().enumerate() {
                offsets[i] = off;
                size += 1 + varint_len(off) + varint_len(body.len() as u64);
                off += body.len() as u64;
            }
            if size == header_len {
                break;
            }
            header_len = size;
        }
        debug_assert_eq!(
            {
                let mut off = header_len as u64;
                let mut size = MAGIC.len()
                    + varint_len(u64::from(IR_FORMAT_VERSION))
                    + varint_len(PRODUCER.len() as u64)
                    + PRODUCER.len()
                    + varint_len(self.sections.len() as u64);
                for (i, (_, body)) in self.sections.iter().enumerate() {
                    debug_assert_eq!(offsets[i], off);
                    size += 1 + varint_len(off) + varint_len(body.len() as u64);
                    off += body.len() as u64;
                }
                size
            },
            header_len,
            "段表不动点未收敛"
        );

        // 头部（自包含）：魔数 + 版本 + producer + 段数 + 段表。
        out.extend_from_slice(&MAGIC);
        put_varint(out, u64::from(IR_FORMAT_VERSION));
        put_str(out, PRODUCER);
        put_varint(out, self.sections.len() as u64);
        for (i, (id, body)) in self.sections.iter().enumerate() {
            put_u8(out, id.as_u8());
            put_varint(out, offsets[i]);
            put_varint(out, body.len() as u64);
        }
        debug_assert_eq!(out.len() - start, header_len, "头部长度与段表推算不一致");

        for (_, body) in &self.sections {
            out.extend_from_slice(body);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip_bytes() {
        // 已知编码：0 → [0x00]、127 → [0x7f]、128 → [0x80,0x01]、300 → [0xac,0x02]。
        let cases: &[(u64, &[u8])] = &[
            (0, &[0x00]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (300, &[0xac, 0x02]),
        ];
        for (v, expect) in cases {
            let mut buf = Vec::new();
            put_varint(&mut buf, *v);
            assert_eq!(buf, *expect, "varint({v}) 编码不符");
            assert_eq!(varint_len(*v), expect.len(), "varint_len({v}) 不符");
        }
        // 边界：u64::MAX 是 10 字节（首字节低 7 位 + 9 个 0xff...）
        let mut buf = Vec::new();
        put_varint(&mut buf, u64::MAX);
        assert_eq!(buf.len(), 10, "u64::MAX 应为 10 字节 varint");
        assert_eq!(varint_len(u64::MAX), 10);
    }

    #[test]
    fn zigzag_is_compact_for_negatives() {
        // -1 → 1、1 → 2（zigzag 交织），因此 -1 只占 1 字节。
        let mut buf = Vec::new();
        put_zigzag(&mut buf, -1);
        assert_eq!(buf, vec![0x01]);
        let mut buf = Vec::new();
        put_zigzag(&mut buf, 1);
        assert_eq!(buf, vec![0x02]);
        // i64::MIN 是 10 字节
        let mut buf = Vec::new();
        put_zigzag(&mut buf, i64::MIN);
        assert_eq!(buf.len(), 10);
    }

    #[test]
    fn i128_zigzag_is_compact_and_exact() {
        // 小值 1 字节；i128::MIN 的 zigzag = u128::MAX（19 字节 varint）。
        let mut buf = Vec::new();
        put_zigzag_i128(&mut buf, -1);
        assert_eq!(buf, vec![0x01], "zigzag(-1) = 1");
        let mut buf = Vec::new();
        put_zigzag_i128(&mut buf, 5);
        assert_eq!(buf, vec![0x0a], "zigzag(5) = 10");
        let mut buf = Vec::new();
        put_zigzag_i128(&mut buf, i128::MIN);
        assert_eq!(buf.len(), 19, "i128::MIN 的 zigzag 是 u128::MAX（19 字节）");
        let mut buf = Vec::new();
        put_varint_u128(&mut buf, u128::MAX);
        assert_eq!(buf.len(), 19, "u128::MAX 是 19 字节 varint");
        // 已知边界：u128 的 18 字节上限（7*18 = 126 位）
        let mut buf = Vec::new();
        put_varint_u128(&mut buf, (1u128 << 126) - 1);
        assert_eq!(buf.len(), 18);
        let mut buf = Vec::new();
        put_varint_u128(&mut buf, 1u128 << 126);
        assert_eq!(buf.len(), 19);
    }

    #[test]
    fn option_tag_is_explicit() {
        let mut buf = Vec::new();
        put_option_tag(&mut buf, false);
        put_option_tag(&mut buf, true);
        assert_eq!(buf, vec![0x00, 0x01]);
    }

    #[test]
    fn len_prefixed_and_str_roundtrip() {
        let mut buf = Vec::new();
        put_len_prefixed(&mut buf, b"abc");
        put_str(&mut buf, "");
        put_str(&mut buf, "中文");
        assert_eq!(&buf[..4], &[0x03, b'a', b'b', b'c']);
        assert_eq!(buf[4], 0x00, "空串 = 长度 0");
        assert_eq!(buf[5], 0x06, "`中文` 是 6 字节 UTF-8");
    }

    #[test]
    fn string_table_dedups_by_first_appearance() {
        let mut table = StringTableWriter::new();
        assert_eq!(table.len(), 0, "不预留空串槽：新表为空");
        let b = table.intern(&ImmStr::from("b"));
        let a = table.intern(&ImmStr::from("a"));
        let b2 = table.intern(&ImmStr::from("b"));
        let empty = table.intern(&ImmStr::from(""));
        assert_eq!((b, a, b2), (0, 1, 0), "去重且按首次出现顺序编号");
        assert_eq!(empty, 2, "空串是普通一条（不占保留槽）");
        assert_eq!(table.len(), 3);
        let mut body = Vec::new();
        table.encode(&mut body);
        // num=3、长度表 [1,1,0]、字节 "ba"
        assert_eq!(&body[..4], &[0x03, 0x01, 0x01, 0x00]);
        assert_eq!(&body[4..], b"ba");
    }
}
