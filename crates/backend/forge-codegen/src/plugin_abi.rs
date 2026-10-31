//! 插件 ABI 的**可序列化编译产物**（v28 M3+）。
//!
//! 机器码本身用原始字节传；重定位含 `ImmStr`，不能直接跨 dylib 返回 Rust 容器。
//! 这里给插件/宿主共用一套紧凑的、与架构无关的编码：
//!
//! ```text
//! u32 count
//! 每项：
//!   u64 offset
//!   u32 kind_tag (0=Absolute, 1=Relative)
//!   u32 width_bytes
//!   i32 adjustment
//!   u64 addend
//!   u32 reloc_len + bytes
//!   u32 symbol_len + bytes
//! ```
//!
//! 版本变化必须同时升 [`crate::PLUGIN_ABI_VERSION`]；加载器只接受精确相等。

use crate::{CompiledFunction, RelocKind, Relocation};
use forge_ir::ImmStr;

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

/// 编码一个函数的全部重定位（不含 line/CFI；它们不是链接正确性前提）。
pub fn encode_relocations(cf: &CompiledFunction) -> Vec<u8> {
    let mut out = Vec::new();
    put_u32(&mut out, cf.relocations.len() as u32);
    for r in &cf.relocations {
        put_u64(&mut out, r.offset as u64);
        match r.kind {
            RelocKind::Absolute(w) => {
                put_u32(&mut out, 0);
                put_u32(&mut out, w);
                put_u32(&mut out, 0);
            }
            RelocKind::Relative(w, adj) => {
                put_u32(&mut out, 1);
                put_u32(&mut out, w);
                put_u32(&mut out, (adj as i32) as u32);
            }
        }
        put_u64(&mut out, r.addend as u64);
        put_str(&mut out, r.reloc.as_str());
        put_str(&mut out, r.symbol.as_str());
    }
    out
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn u32(&mut self) -> Result<u32, String> {
        let s = self
            .b
            .get(self.p..self.p + 4)
            .ok_or_else(|| "重定位流截断（u32）".to_string())?;
        self.p += 4;
        Ok(u32::from_le_bytes(s.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let s = self
            .b
            .get(self.p..self.p + 8)
            .ok_or_else(|| "重定位流截断（u64）".to_string())?;
        self.p += 8;
        Ok(u64::from_le_bytes(s.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        let s = self
            .b
            .get(self.p..self.p + n)
            .ok_or_else(|| "重定位流截断（string）".to_string())?;
        self.p += n;
        String::from_utf8(s.to_vec()).map_err(|e| format!("重定位字符串非 UTF-8：{e}"))
    }
}

/// 解码插件回传的重定位流。
pub fn decode_relocations(bytes: &[u8]) -> Result<Vec<Relocation>, String> {
    let mut r = Reader { b: bytes, p: 0 };
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let offset = r.u64()? as usize;
        let tag = r.u32()?;
        let width = r.u32()?;
        let adj = r.u32()? as i32 as i8;
        let addend = r.u64()? as i64;
        let reloc = r.string()?;
        let symbol = r.string()?;
        let kind = match tag {
            0 => RelocKind::Absolute(width),
            1 => RelocKind::Relative(width, adj),
            _ => return Err(format!("未知 RelocKind tag {tag}")),
        };
        out.push(Relocation {
            offset,
            kind,
            reloc: ImmStr::from(reloc),
            symbol: ImmStr::from(symbol),
            addend,
        });
    }
    if r.p != bytes.len() {
        return Err(format!("重定位流尾部多出 {} 字节", bytes.len() - r.p));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_relocations() {
        let cf = CompiledFunction {
            code: vec![0; 8],
            relocations: vec![
                Relocation {
                    offset: 4,
                    kind: RelocKind::Relative(4, -4),
                    reloc: ImmStr::from("call32"),
                    symbol: ImmStr::from("foo"),
                    addend: -4,
                },
                Relocation {
                    offset: 0,
                    kind: RelocKind::Absolute(8),
                    reloc: ImmStr::from(""),
                    symbol: ImmStr::from("bar"),
                    addend: 0,
                },
            ],
            code_size: 8,
            line_entries: Vec::new(),
            cfi: None,
        };
        let bytes = encode_relocations(&cf);
        let back = decode_relocations(&bytes).expect("decode");
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].kind, cf.relocations[0].kind);
        assert_eq!(back[0].symbol.as_str(), "foo");
        assert_eq!(back[1].kind, cf.relocations[1].kind);
    }
}
