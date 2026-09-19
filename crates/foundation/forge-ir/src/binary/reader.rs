//! 二进制**读侧**：fail-closed 游标 + 头部/段表解析 + STRINGS 段解码。
//!
//! 纪律（与 [`super::writer`] 对偶）：
//!
//! - **绝不 panic**：所有越界/截断/非法取值一律返回 `IrError::BinaryDecode`（带偏移），
//!   不使用裸切片索引、不 `unwrap`、不把 `debug_assert` 当校验；
//! - **不做不可信分配**：长度字段先与"剩余字节数"比对，再分配（拒绝"声明 4G 长度"）；
//! - **未知即错**：未知段 id、未知 flag 位、重复段、段越界/重叠都返回 `Err`
//!   （不是"跳过该段"）。

use std::collections::HashMap;

use crate::error::IrError;
use crate::util::imm_str::ImmStr;

use super::format::{IR_FORMAT_VERSION, MAGIC, SectionId};
use super::pack;

// ============================================================
// 错误构造
// ============================================================

/// 构造带偏移的解码错误。
fn decode_err<T>(offset: usize, msg: impl Into<String>) -> Result<T, IrError> {
    Err(IrError::BinaryDecode {
        offset,
        msg: msg.into(),
    })
}

// ============================================================
// Cursor
// ============================================================

/// 只读游标：每次读先查剩余长度，越界即错（fail-closed）。
#[derive(Debug, Clone)]
pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// 从头开始。
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// 从指定绝对偏移开始（段体）。
    pub(crate) fn at(bytes: &'a [u8], offset: usize) -> Self {
        Self { bytes, pos: offset }
    }

    pub(crate) fn offset(&self) -> usize {
        self.pos
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    /// 读 1 字节。
    pub(crate) fn read_u8(&mut self) -> Result<u8, IrError> {
        if self.pos >= self.bytes.len() {
            return decode_err(self.pos, "读取 1 字节越界（文件已结束）");
        }
        let b = self.bytes[self.pos];
        self.pos += 1;
        Ok(b)
    }

    /// 读 LEB128 无符号 varint（最多 10 字节；溢出即错）。
    pub(crate) fn read_varint(&mut self) -> Result<u64, IrError> {
        let start = self.pos;
        let mut result = 0u64;
        let mut shift = 0u32;
        loop {
            if shift > 63 {
                return decode_err(start, "varint 超过 64 位");
            }
            let byte = self.read_u8()?;
            let low = u64::from(byte & 0x7f);
            if shift == 63 && low > 1 {
                return decode_err(start, "varint 溢出 u64");
            }
            result |= low << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
    }

    /// 读 varint 并转 `usize`（超过平台 `usize::MAX` 即错）。
    pub(crate) fn read_usize(&mut self) -> Result<usize, IrError> {
        let start = self.pos;
        let v = self.read_varint()?;
        usize::try_from(v).map_err(|_| IrError::BinaryDecode {
            offset: start,
            msg: format!("长度 {v} 超过本平台 usize 上限"),
        })
    }

    /// 读 zigzag + LEB128 的有符号数（i64 版；i128 见 [`Cursor::read_zigzag_i128`]）。
    ///
    /// B1 尚未有消费方（B3 起有符号常量用到），但原语与格式同批落地并自带单测，
    /// 避免"用到才加"造成的格式漂移。
    pub(crate) fn read_zigzag(&mut self) -> Result<i64, IrError> {
        let zz = self.read_varint()?;
        Ok(((zz >> 1) as i64) ^ -((zz & 1) as i64))
    }

    /// 读 LEB128 无符号 varint（u128；最多 19 字节，溢出即错）。
    #[allow(dead_code)] // B3 起使用（i128 常量/`Big` 的指数）
    pub(crate) fn read_varint_u128(&mut self) -> Result<u128, IrError> {
        let start = self.pos;
        let mut result = 0u128;
        let mut shift = 0u32;
        loop {
            if shift > 127 {
                return decode_err(start, "varint 超过 128 位");
            }
            let byte = self.read_u8()?;
            let low = u128::from(byte & 0x7f);
            if shift == 126 && low > 3 {
                return decode_err(start, "varint 溢出 u128");
            }
            result |= low << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
    }

    /// 读 zigzag + LEB128 的 i128（常量池整数）。
    #[allow(dead_code)] // B3 起使用
    pub(crate) fn read_zigzag_i128(&mut self) -> Result<i128, IrError> {
        let zz = self.read_varint_u128()?;
        Ok(((zz >> 1) as i128) ^ -((zz & 1) as i128))
    }

    /// 读 `n` 字节（越界即错）。
    pub(crate) fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], IrError> {
        if n > self.remaining() {
            return decode_err(
                self.pos,
                format!("读取 {n} 字节越界（只剩 {}）", self.remaining()),
            );
        }
        let out = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    /// 读 varint 长度前缀 + 原始字节（长度先与剩余比对，再切片——不预分配）。
    pub(crate) fn read_len_prefixed(&mut self) -> Result<&'a [u8], IrError> {
        let n = self.read_usize()?;
        self.read_bytes(n)
    }

    /// 读 varint 长度前缀 + UTF-8 文本（非法 UTF-8 即错，偏移指向载荷起点）。
    pub(crate) fn read_utf8(&mut self) -> Result<&'a str, IrError> {
        let bytes = self.read_len_prefixed()?;
        let at = self.pos - bytes.len();
        std::str::from_utf8(bytes).map_err(|e| IrError::BinaryDecode {
            offset: at,
            msg: format!("字符串不是合法 UTF-8：{e}"),
        })
    }

    /// 读**已知长度**的 UTF-8 文本（字符串表用：长度表与字节分离）。
    pub(crate) fn read_utf8_sized(&mut self, n: usize) -> Result<&'a str, IrError> {
        let at = self.pos;
        let bytes = self.read_bytes(n)?;
        std::str::from_utf8(bytes).map_err(|e| IrError::BinaryDecode {
            offset: at,
            msg: format!("字符串不是合法 UTF-8：{e}"),
        })
    }
}

// ============================================================
// 头部与段表
// ============================================================

/// 段表条目（`offset` 为绝对偏移）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SectionEntry {
    pub(crate) id: SectionId,
    pub(crate) offset: usize,
    /// 段体在流里的字节数（压缩体也算这个长度）。
    pub(crate) len: usize,
    /// `0` = 段体未压缩；`> 0` = 段体经 `pack` 压缩、解压后为该字节数（v2）。
    pub(crate) raw_len: usize,
}

/// 头部解析结果。
#[derive(Debug, Clone)]
pub(crate) struct Header {
    pub(crate) version: u16,
    pub(crate) producer: String,
    pub(crate) sections: Vec<SectionEntry>,
}

impl Header {
    pub(crate) fn section(&self, id: SectionId) -> Option<SectionEntry> {
        self.sections.iter().copied().find(|e| e.id == id)
    }
}

/// 解析头部 + 段表，并做**结构校验**（越界/重叠/重复/未知 id）。
///
/// 不做语义解码（不进 `Module`），因此 `check_binary_compat` 与 `from_binary`
/// 共用这一份实现。
pub(crate) fn parse_header(bytes: &[u8]) -> Result<Header, IrError> {
    if bytes.len() < MAGIC.len() {
        return decode_err(
            0,
            format!("文件长度 {} 小于 {}-字节魔数", bytes.len(), MAGIC.len()),
        );
    }
    let mut c = Cursor::new(bytes);
    let magic = c.read_bytes(MAGIC.len())?;
    if magic != MAGIC {
        return decode_err(0, format!("魔数不匹配（期望 {MAGIC:?}，实测 {magic:?}）"));
    }
    let version_at = c.offset();
    let version_raw = c.read_varint()?;
    let version = u16::try_from(version_raw).map_err(|_| IrError::BinaryDecode {
        offset: version_at,
        msg: format!("格式版本 {version_raw} 超出 u16"),
    })?;
    if version != IR_FORMAT_VERSION {
        return decode_err(
            version_at,
            format!("格式版本 {version} 与当前 {IR_FORMAT_VERSION} 不符（不做兼容升级）"),
        );
    }
    let producer = c.read_utf8()?.to_string();

    let count_at = c.offset();
    let count = c.read_usize()?;
    // 每条段表条目至少 4 字节（id + offset + len + raw_len）⇒ 条数不得超过剩余字节数的 1/4。
    if count > c.remaining() / 4 {
        return decode_err(
            count_at,
            format!(
                "段表声明 {count} 段，但剩余字节只够 {} 段",
                c.remaining() / 4
            ),
        );
    }

    let mut sections: Vec<SectionEntry> = Vec::with_capacity(count);
    for _ in 0..count {
        let id_at = c.offset();
        let id_byte = c.read_u8()?;
        let id = SectionId::from_u8(id_byte).ok_or(IrError::BinaryDecode {
            offset: id_at,
            msg: format!("未知段 id 0x{id_byte:02x}"),
        })?;
        let offset_at = c.offset();
        let offset = c.read_usize()?;
        let len_at = c.offset();
        let len = c.read_usize()?;
        let raw_len_at = c.offset();
        let raw_len = c.read_usize()?;
        // 解压后长度只允许"未压缩（0）"或"合理范围内"（防解压炸弹；见 pack 的常量）。
        if raw_len > 0 {
            pack::check_ratio(len, raw_len).map_err(|msg| IrError::BinaryDecode {
                offset: raw_len_at,
                msg: format!("段 {} 的 raw_len 不可信：{msg}", id.name()),
            })?;
        }
        let end = offset.checked_add(len).ok_or(IrError::BinaryDecode {
            offset: offset_at,
            msg: format!("段 {} 的 offset+len 溢出", id.name()),
        })?;
        if end > bytes.len() {
            return decode_err(
                len_at,
                format!(
                    "段 {} 越界：offset {offset} + len {len} > 文件长度 {}",
                    id.name(),
                    bytes.len()
                ),
            );
        }
        if sections.iter().any(|e| e.id == id) {
            return decode_err(id_at, format!("段 {} 重复出现", id.name()));
        }
        sections.push(SectionEntry {
            id,
            offset,
            len,
            raw_len,
        });
    }
    let header_end = c.offset();
    for e in &sections {
        if e.offset < header_end {
            return decode_err(
                e.offset,
                format!(
                    "段 {} 的 offset {} 落在头部/段表内（头部结束于 {header_end}）",
                    e.id.name(),
                    e.offset
                ),
            );
        }
    }
    // 段体不得重叠：按 offset 排序后逐对检查。
    let mut sorted = sections.clone();
    sorted.sort_by_key(|e| e.offset);
    for pair in sorted.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a.offset + a.len > b.offset {
            return decode_err(
                b.offset,
                format!(
                    "段 {} 与段 {} 重叠（前者 {}+{}，后者起点 {}）",
                    a.id.name(),
                    b.id.name(),
                    a.offset,
                    a.len,
                    b.offset
                ),
            );
        }
    }
    Ok(Header {
        version,
        producer,
        sections,
    })
}

// ============================================================
// STRINGS 段
// ============================================================

/// 解码字符串表：`num` + `num` 个长度 + 字节拼接。
///
/// 校验：每段合法 UTF-8、**无重复**（表是"内容 → 索引"的单射，重复说明文件被改过
/// 或写侧有 bug）。`num == 0`（空池）合法——表不预留空串槽。
///
/// `body` 是**段体本身**（调用方已按需解压），因此错误里的 `offset` 是段体内偏移。
pub(crate) fn decode_strings(body: &[u8]) -> Result<Vec<ImmStr>, IrError> {
    let mut c = Cursor::new(body);
    let num_at = c.offset();
    let num = c.read_usize()?;
    // 长度表：每项至少 1 字节 ⇒ 数量不得超过剩余字节数。
    if num > c.remaining() {
        return decode_err(
            num_at,
            format!("字符串表声明 {num} 项，但剩余字节只够 {} 项", c.remaining()),
        );
    }
    let mut lens = Vec::with_capacity(num);
    for _ in 0..num {
        lens.push(c.read_usize()?);
    }
    let mut out: Vec<ImmStr> = Vec::with_capacity(num);
    let mut seen: HashMap<ImmStr, usize> = HashMap::with_capacity(num);
    for (i, len) in lens.into_iter().enumerate() {
        let at = c.offset();
        let s = c.read_utf8_sized(len)?;
        let imm = ImmStr::from(s);
        if let Some(prev) = seen.insert(imm.clone(), i) {
            return decode_err(at, format!("字符串表第 {i} 项与第 {prev} 项重复：{s:?}"));
        }
        out.push(imm);
    }
    Ok(out)
}

// ============================================================
// Reader
// ============================================================

/// 模块读取器：头部 + 段表 + 字符串表（段体按需切片；压缩段体在 `parse` 时**一次性**
/// 解压到 `decoded`，因此 `section()` 对调用方仍是"给我一个游标"）。
///
/// `header`/`bytes` 只被段访问器读（B2 起使用，B1 只有单测覆盖）——整结构体加
/// `allow(dead_code)` 而不是逐个字段，避免"字段被 dead 方法读"的连锁告警。
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    header: Header,
    strings: Vec<ImmStr>,
    /// 解压后的段体（v2 压缩段；`raw_len == 0` 的段不在此表里）。**含 STRINGS**：
    /// 否则同一段"压了"与"没压"两种形态下 `section()` 的行为会不一致。
    decoded: Vec<(SectionId, Vec<u8>)>,
}

impl<'a> Reader<'a> {
    /// 解析头部与字符串表，校验 COMPAT 段（未知 flag 位即错），并**解压全部压缩段体**
    /// （解压失败即在此处报错——fail-closed，绝不把坏段体交给下游）。
    pub(crate) fn parse(bytes: &'a [u8]) -> Result<Self, IrError> {
        let header = parse_header(bytes)?;
        if let Some(entry) = header.section(SectionId::Compat) {
            let mut c = Cursor::at(bytes, entry.offset);
            let flags_at = c.offset();
            let flags = c.read_varint()?;
            if flags != 0 {
                return decode_err(
                    flags_at,
                    format!("COMPAT 段含未知 flag 位 0x{flags:x}（本版本只认 0）"),
                );
            }
        }
        let strings_entry = header
            .section(SectionId::Strings)
            .ok_or(IrError::BinaryDecode {
                offset: 0,
                msg: "缺 STRINGS 段".to_string(),
            })?;
        // STRINGS 段自身也可能被压缩（字符串表通常很值得压）。
        let strings_raw = decompress_entry(bytes, &strings_entry)?;
        let strings = decode_strings(&strings_raw)?;

        // 其余压缩段体一次性解压（数量 ≤ 7，代价可忽略；换来 `section()` 的简单签名）。
        let mut decoded = Vec::new();
        if strings_entry.raw_len > 0 {
            decoded.push((SectionId::Strings, strings_raw));
        }
        for entry in &header.sections {
            if entry.raw_len > 0 && entry.id != SectionId::Strings {
                let raw = decompress_entry(bytes, entry)?;
                decoded.push((entry.id, raw));
            }
        }
        Ok(Self {
            bytes,
            header,
            strings,
            decoded,
        })
    }

    #[allow(dead_code)] // B2+ 各段 reader 使用；B1 只有单测覆盖
    pub(crate) fn header(&self) -> &Header {
        &self.header
    }

    pub(crate) fn strings(&self) -> &[ImmStr] {
        &self.strings
    }

    /// 取某段体的游标；段不存在返回 `None`（缺失 = 空，不属于损坏）。
    ///
    /// 压缩段返回的是**解压后**的字节（`decoded` 里的副本），因此返回的游标借的是
    /// `&self` 而不是 `&'a [u8]`。
    #[allow(dead_code)] // B2+ 各段 reader 使用；B1 只有单测覆盖
    pub(crate) fn section(&self, id: SectionId) -> Option<Cursor<'_>> {
        let entry = self.header.section(id)?;
        if entry.raw_len > 0 {
            let raw = self
                .decoded
                .iter()
                .find(|(sid, _)| *sid == id)
                .map(|(_, body)| body.as_slice())?;
            Some(Cursor::new(raw))
        } else {
            Some(Cursor::at(self.bytes, entry.offset))
        }
    }
}

/// 取段体原始字节（压缩则解压，返回**拥有**的缓冲）。
fn decompress_entry(bytes: &[u8], entry: &SectionEntry) -> Result<Vec<u8>, IrError> {
    let body = &bytes[entry.offset..entry.offset + entry.len];
    if entry.raw_len == 0 {
        return Ok(body.to_vec());
    }
    pack::unpack(body, entry.raw_len).map_err(|msg| IrError::BinaryDecode {
        offset: entry.offset,
        msg: format!("段 {} 解压失败：{msg}", entry.id.name()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::writer::{put_str, put_u8, put_varint, varint_len};

    /// 独立实现的头部/段表装配（**不调用被测的 `writer::finish`**，避免"用被测代码
    /// 造夹具"）：算法相同，但这里手写，可用它交叉验证布局。段体一律未压缩
    /// （`raw_len = 0`）；要造压缩段用 [`build_stream_raw`]。
    fn build_stream(version: u64, producer: &str, sections: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let sections: Vec<(u8, Vec<u8>, usize)> =
            sections.iter().map(|(id, b)| (*id, b.clone(), 0)).collect();
        build_stream_raw(version, producer, &sections)
    }

    /// 同上，但每条段表条目可携带 `raw_len`（`> 0` = 段体是压缩体）。
    fn build_stream_raw(
        version: u64,
        producer: &str,
        sections: &[(u8, Vec<u8>, usize)],
    ) -> Vec<u8> {
        let mut header_len = 0usize;
        let mut offsets = vec![0u64; sections.len()];
        loop {
            let mut off = header_len as u64;
            let mut size = MAGIC.len()
                + varint_len(version)
                + varint_len(producer.len() as u64)
                + producer.len()
                + varint_len(sections.len() as u64);
            for (i, (_, body, raw_len)) in sections.iter().enumerate() {
                offsets[i] = off;
                // 条目 = id(1) + offset + len + raw_len
                size += 1
                    + varint_len(off)
                    + varint_len(body.len() as u64)
                    + varint_len(*raw_len as u64);
                off += body.len() as u64;
            }
            if size == header_len {
                break;
            }
            header_len = size;
        }
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        put_varint(&mut out, version);
        put_str(&mut out, producer);
        put_varint(&mut out, sections.len() as u64);
        for (i, (id, body, raw_len)) in sections.iter().enumerate() {
            put_u8(&mut out, *id);
            put_varint(&mut out, offsets[i]);
            put_varint(&mut out, body.len() as u64);
            put_varint(&mut out, *raw_len as u64);
        }
        assert_eq!(out.len(), header_len, "夹具头部长度推算");
        for (_, body, _) in sections {
            out.extend_from_slice(body);
        }
        out
    }

    /// 字符串段体：`num` + 长度表 + 字节拼接。
    fn strings_body(items: &[&str]) -> Vec<u8> {
        let mut body = Vec::new();
        put_varint(&mut body, items.len() as u64);
        for s in items {
            put_varint(&mut body, s.len() as u64);
        }
        for s in items {
            body.extend_from_slice(s.as_bytes());
        }
        body
    }

    fn ok_stream() -> Vec<u8> {
        build_stream(
            u64::from(IR_FORMAT_VERSION),
            "test 0.0.0",
            &[
                (SectionId::Compat.as_u8(), vec![0x00]),
                (SectionId::Strings.as_u8(), strings_body(&["", "alpha"])),
            ],
        )
    }

    #[test]
    fn cursor_reads_varint_zigzag_and_utf8() {
        // varint(300) + zigzag(-1) + len-prefixed("ch中" = 5 字节)
        let mut c = Cursor::new(&[0xac, 0x02, 0x01, 0x05, b'c', b'h', 0xe4, 0xb8, 0xad]);
        assert_eq!(c.read_varint().expect("varint"), 300);
        assert_eq!(c.read_zigzag().expect("zigzag"), -1);
        assert_eq!(c.read_utf8().expect("utf8"), "ch中");
        assert_eq!(c.remaining(), 0);
    }

    #[test]
    fn cursor_fails_closed() {
        // 空读
        let e = Cursor::new(&[]).read_u8().expect_err("空文件必须错");
        assert!(matches!(e, IrError::BinaryDecode { offset: 0, .. }));
        // 截断的 varint（续位未结束）：偏移指向"读不到的那个字节"
        let e = Cursor::new(&[0x80])
            .read_varint()
            .expect_err("截断 varint 必须错");
        assert!(
            matches!(e, IrError::BinaryDecode { offset: 1, .. }),
            "{e:?}"
        );
        // 溢出的 varint（11 字节全续位，shift 超过 63）
        let e = Cursor::new(&[0xff; 11])
            .read_varint()
            .expect_err("溢出 varint 必须错");
        assert!(matches!(e, IrError::BinaryDecode { .. }));
        // 长度前缀超过剩余字节：偏移指向载荷起点（长度 varint 之后）
        let e = Cursor::new(&[0x05, 1, 2, 3])
            .read_len_prefixed()
            .expect_err("长度越界必须错");
        assert!(
            matches!(e, IrError::BinaryDecode { offset: 1, .. }),
            "{e:?}"
        );
        // 非法 UTF-8：偏移指向载荷起点
        let e = Cursor::new(&[0x02, 0xff, 0xfe])
            .read_utf8()
            .expect_err("非法 UTF-8 必须错");
        assert!(
            matches!(e, IrError::BinaryDecode { offset: 1, .. }),
            "{e:?}"
        );
    }

    #[test]
    fn i128_varint_roundtrips_and_rejects_overflow() {
        for v in [
            0i128,
            1,
            -1,
            i64::MAX as i128,
            i64::MIN as i128,
            i128::MAX,
            i128::MIN,
            123456789012345678901234567890,
        ] {
            let mut buf = Vec::new();
            crate::binary::writer::put_zigzag_i128(&mut buf, v);
            let mut c = Cursor::new(&buf);
            assert_eq!(c.read_zigzag_i128().expect("回读"), v, "i128 zigzag({v})");
        }
        // 20 字节全续位 ⇒ 溢出（shift 超过 127）
        let e = Cursor::new(&[0xff; 20])
            .read_varint_u128()
            .expect_err("超长 varint 必须错");
        assert!(matches!(e, IrError::BinaryDecode { .. }), "{e:?}");
        // 第 19 字节（shift=126）只允许低 2 位
        let mut bytes = vec![0xff; 18];
        bytes.push(0x7f); // shift=126、低 7 位 = 0x7f > 3 ⇒ 溢出
        let e = Cursor::new(&bytes)
            .read_varint_u128()
            .expect_err("第 19 字节越界位必须错");
        assert!(matches!(e, IrError::BinaryDecode { .. }), "{e:?}");
    }

    #[test]
    fn reader_parses_header_and_strings() {
        let stream = ok_stream();
        let reader = Reader::parse(&stream).expect("合法流");
        assert_eq!(reader.header().version, IR_FORMAT_VERSION);
        assert_eq!(reader.header().producer, "test 0.0.0");
        assert_eq!(reader.strings().len(), 2);
        assert_eq!(reader.strings()[0].as_str(), "");
        assert_eq!(reader.strings()[1].as_str(), "alpha");
        assert!(reader.section(SectionId::Compat).is_some());
        assert!(
            reader.section(SectionId::Types).is_none(),
            "缺段 = 空，不是损坏"
        );
    }

    #[test]
    fn reader_rejects_version_mismatch() {
        let stream = build_stream(
            u64::from(IR_FORMAT_VERSION) + 1,
            "test",
            &[(SectionId::Strings.as_u8(), strings_body(&[""]))],
        );
        let e = Reader::parse(&stream).expect_err("版本不符必须错");
        match e {
            IrError::BinaryDecode { msg, .. } => assert!(msg.contains("格式版本"), "消息：{msg}"),
            other => panic!("错误类型不对：{other:?}"),
        }
    }

    #[test]
    fn reader_rejects_unknown_section_id() {
        let stream = build_stream(
            u64::from(IR_FORMAT_VERSION),
            "test",
            &[
                (SectionId::Strings.as_u8(), strings_body(&[""])),
                (0x7f, vec![0x00]),
            ],
        );
        let e = Reader::parse(&stream).expect_err("未知段 id 必须错");
        match e {
            IrError::BinaryDecode { msg, .. } => assert!(msg.contains("未知段 id"), "消息：{msg}"),
            other => panic!("错误类型错误：{other:?}"),
        }
    }

    #[test]
    fn reader_rejects_missing_strings_section() {
        let stream = build_stream(
            u64::from(IR_FORMAT_VERSION),
            "test",
            &[(SectionId::Compat.as_u8(), vec![0x00])],
        );
        let e = Reader::parse(&stream).expect_err("缺 STRINGS 必须错");
        match e {
            IrError::BinaryDecode { msg, .. } => assert!(msg.contains("STRINGS"), "消息：{msg}"),
            other => panic!("错误类型错误：{other:?}"),
        }
    }

    #[test]
    fn reader_rejects_compat_flag_bits() {
        // COMPAT 段带未知 flag 位（0x02）。
        let stream = build_stream(
            u64::from(IR_FORMAT_VERSION),
            "test",
            &[
                (SectionId::Compat.as_u8(), vec![0x02]),
                (SectionId::Strings.as_u8(), strings_body(&[""])),
            ],
        );
        let e = Reader::parse(&stream).expect_err("未知 flag 位必须错");
        match e {
            IrError::BinaryDecode { msg, .. } => assert!(msg.contains("flag"), "消息：{msg}"),
            other => panic!("错误类型错误：{other:?}"),
        }
    }

    #[test]
    fn decode_strings_rejects_duplicates() {
        let stream = build_stream(
            u64::from(IR_FORMAT_VERSION),
            "test",
            &[(
                SectionId::Strings.as_u8(),
                strings_body(&["", "dup", "dup"]),
            )],
        );
        let e = Reader::parse(&stream).expect_err("重复字符串必须错");
        match e {
            IrError::BinaryDecode { msg, .. } => assert!(msg.contains("重复"), "消息：{msg}"),
            other => panic!("错误类型错误：{other:?}"),
        }
    }

    #[test]
    fn empty_string_table_is_accepted() {
        // 表不预留空串槽 ⇒ num = 0 合法（空池）。
        let stream = build_stream(
            u64::from(IR_FORMAT_VERSION),
            "test",
            &[
                (SectionId::Compat.as_u8(), vec![0x00]),
                (SectionId::Strings.as_u8(), strings_body(&[])),
            ],
        );
        let reader = Reader::parse(&stream).expect("空字符串表合法");
        assert!(reader.strings().is_empty());
    }

    /// 显式指定段偏移的装配（用于构造"越界/重叠"这类损坏流）；段体一律未压缩。
    fn raw_stream(
        version: u64,
        producer: &str,
        entries: &[(u8, u64, u64)],
        bodies: &[&[u8]],
    ) -> Vec<u8> {
        let header_len = MAGIC.len()
            + varint_len(version)
            + varint_len(producer.len() as u64)
            + producer.len()
            + varint_len(entries.len() as u64)
            + entries
                .iter()
                .map(|(_, off, len)| 1 + varint_len(*off) + varint_len(*len) + 1) // + raw_len = 0
                .sum::<usize>();
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        put_varint(&mut out, version);
        put_str(&mut out, producer);
        put_varint(&mut out, entries.len() as u64);
        for (id, off, len) in entries {
            put_u8(&mut out, *id);
            put_varint(&mut out, *off);
            put_varint(&mut out, *len);
            put_varint(&mut out, 0); // raw_len = 0：未压缩
        }
        assert_eq!(out.len(), header_len, "夹具头部长度推算");
        let _ = header_len;
        for body in bodies {
            out.extend_from_slice(body);
        }
        out
    }

    #[test]
    fn reader_decompresses_packed_section() {
        // 够长且重复度高（短于 `MIN_PACK_INPUT` 的段体不建表、压不小 ⇒ 测不到这条路径）
        let raw = strings_body(&["", "alpha", &"alpha".repeat(20)]);
        let packed = pack::pack(&raw);
        assert!(packed.len() < raw.len(), "夹具的前提：这个段体真的更小");
        let stream = build_stream_raw(
            u64::from(IR_FORMAT_VERSION),
            "test 0.0.0",
            &[
                (SectionId::Compat.as_u8(), vec![0x00], 0),
                (SectionId::Strings.as_u8(), packed, raw.len()),
            ],
        );
        let reader = Reader::parse(&stream).expect("压缩段必须可解");
        assert_eq!(reader.strings().len(), 3);
        assert_eq!(reader.strings()[1].as_str(), "alpha");
        assert_eq!(reader.strings()[2].as_str(), "alpha".repeat(20));
        // `section()` 对压缩段返回的是**解压后**的游标（调用方不必感知压缩）
        let mut c = reader.section(SectionId::Strings).expect("STRINGS 段");
        let n = c.remaining();
        assert_eq!(
            c.read_bytes(n).expect("读完"),
            raw.as_slice(),
            "解压结果必须与未压缩段体逐字节相同"
        );
    }

    #[test]
    fn reader_rejects_untrusted_raw_len() {
        let raw = strings_body(&["", "alpha", &"alpha".repeat(20)]);
        let packed = pack::pack(&raw);
        // (a) 声明比实际少 1 字节：解压产出与声明不符（或尾随数据）⇒ 拒
        let stream = build_stream_raw(
            u64::from(IR_FORMAT_VERSION),
            "test 0.0.0",
            &[
                (SectionId::Compat.as_u8(), vec![0x00], 0),
                (SectionId::Strings.as_u8(), packed.clone(), raw.len() - 1),
            ],
        );
        let e = Reader::parse(&stream).expect_err("raw_len 不符必须错");
        assert!(
            matches!(e, IrError::BinaryDecode { .. }),
            "必须是有偏移的解码错误：{e:?}"
        );
        // (b) 声明 4 GiB：超过单段上限，连解压都不该开始
        let stream = build_stream_raw(
            u64::from(IR_FORMAT_VERSION),
            "test 0.0.0",
            &[
                (SectionId::Compat.as_u8(), vec![0x00], 0),
                (SectionId::Strings.as_u8(), packed, 4 << 30),
            ],
        );
        let e = Reader::parse(&stream).expect_err("解压炸弹必须错");
        match e {
            IrError::BinaryDecode { msg, .. } => assert!(msg.contains("raw_len"), "消息：{msg}"),
            other => panic!("错误类型错误：{other:?}"),
        }
    }

    #[test]
    fn parse_header_rejects_overlapping_sections() {
        let strings = strings_body(&[""]);
        let len = strings.len() as u64;
        // 先按"两段各占自己的区间"算出头部长度，再让两段共用同一 offset。
        let probe = raw_stream(
            u64::from(IR_FORMAT_VERSION),
            "test",
            &[
                (SectionId::Strings.as_u8(), 0, len),
                (SectionId::Types.as_u8(), 0, 0),
            ],
            &[&strings],
        );
        let header_len = (probe.len() - strings.len()) as u64;
        let stream = raw_stream(
            u64::from(IR_FORMAT_VERSION),
            "test",
            &[
                (SectionId::Strings.as_u8(), header_len, len),
                (SectionId::Types.as_u8(), header_len, 0),
            ],
            &[&strings],
        );
        let e = parse_header(&stream).expect_err("段重叠必须错");
        match e {
            IrError::BinaryDecode { msg, .. } => assert!(msg.contains("重叠"), "消息：{msg}"),
            other => panic!("错误类型错误：{other:?}"),
        }
    }

    #[test]
    fn parse_header_rejects_out_of_range_section() {
        let stream = raw_stream(
            u64::from(IR_FORMAT_VERSION),
            "test",
            &[(SectionId::Strings.as_u8(), 64, 1024)],
            &[&strings_body(&[""])],
        );
        let e = parse_header(&stream).expect_err("段越界必须错");
        match e {
            IrError::BinaryDecode { msg, .. } => assert!(msg.contains("越界"), "消息：{msg}"),
            other => panic!("错误类型错误：{other:?}"),
        }
    }
}
