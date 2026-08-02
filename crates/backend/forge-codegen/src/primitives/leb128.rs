//! LEB128 encoding utilities — ISA-agnostic, works on any `Vec<u8>`.
//!
//! Used by WASM and other ISAs that need variable-length integer encoding.

/// Encode an unsigned integer as unsigned LEB128.
pub fn encode_uleb128(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Encode a signed integer as signed LEB128.
pub fn encode_sleb128(buf: &mut Vec<u8>, mut value: i64) {
    loop {
        let mut byte = (value as u8) & 0x7F;
        value >>= 7;
        if (value == 0 && (byte & 0x40) == 0) || (value == -1 && (byte & 0x40) != 0) {
            buf.push(byte);
            break;
        }
        byte |= 0x80;
        buf.push(byte);
    }
}

/// Decode an unsigned LEB128 value from bytes. Returns (value, bytes_consumed).
pub fn decode_uleb128(bytes: &[u8]) -> Result<(u64, usize), &'static str> {
    let mut value: u64 = 0;
    let mut shift = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
        shift += 7;
        if shift >= 64 {
            return Err("LEB128 too long for u64");
        }
    }
    Err("truncated LEB128 sequence")
}

/// Decode a signed LEB128 value from bytes. Returns (value, bytes_consumed).
pub fn decode_sleb128(bytes: &[u8]) -> Result<(i64, usize), &'static str> {
    let mut value: i64 = 0;
    let mut shift = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        value |= ((byte & 0x7F) as i64) << shift;
        if byte & 0x80 == 0 {
            // Sign-extend if the last byte's sign bit is set
            if shift < 64 && (byte & 0x40) != 0 {
                value |= !0i64 << (shift + 7);
            }
            return Ok((value, i + 1));
        }
        shift += 7;
        if shift >= 64 {
            return Err("LEB128 too long for i64");
        }
    }
    Err("truncated LEB128 sequence")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uleb128_zero() {
        let mut buf = Vec::new();
        encode_uleb128(&mut buf, 0);
        assert_eq!(buf, &[0x00]);
        assert_eq!(decode_uleb128(&buf).unwrap(), (0, 1));
    }

    #[test]
    fn test_uleb128_small() {
        let mut buf = Vec::new();
        encode_uleb128(&mut buf, 127);
        assert_eq!(buf, &[0x7F]);
    }

    #[test]
    fn test_uleb128_two_bytes() {
        let mut buf = Vec::new();
        encode_uleb128(&mut buf, 128);
        assert_eq!(buf, &[0x80, 0x01]);
        assert_eq!(decode_uleb128(&buf).unwrap(), (128, 2));
    }

    #[test]
    fn test_sleb128_negative() {
        let mut buf = Vec::new();
        encode_sleb128(&mut buf, -1);
        assert_eq!(buf, &[0x7F]); // -1 in SLEB128 is 0x7F
        assert_eq!(decode_sleb128(&buf).unwrap(), (-1, 1));
    }

    #[test]
    fn test_sleb128_positive() {
        let mut buf = Vec::new();
        encode_sleb128(&mut buf, 64);
        assert_eq!(buf, &[0xC0, 0x00]);
        assert_eq!(decode_sleb128(&buf).unwrap(), (64, 2));
    }
}
