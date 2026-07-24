//! VEX / EVEX 前缀编码辅助函数（v8.3 新增）。
//!
//! 处理 VEX2 (0xC5)、VEX3 (0xC4) 和 EVEX (0x62) 前缀的字节编码。

use crate::backend::emit::CodeSink;

/// VEX 前缀字段配置。
#[derive(Clone, Copy, Debug)]
pub struct VexFields {
    /// ~R (inverted)
    pub r: bool,
    /// ~X (inverted)
    pub x: bool,
    /// ~B (inverted)
    pub b: bool,
    /// VEX.m-mmmm — 映射选择 (0=0F, 1=0F38, 2=0F3A, 3=0F5A...)
    pub mmmmm: u8,
    /// W 位 (操作数大小)
    pub w: bool,
    /// vvvv — 第 3 操作数寄存器 (ISA 反转存储)
    pub vvvv: u8,
    /// L 位 (向量长度: 0=128, 1=256)
    pub l: bool,
    /// pp — 隐式前缀 (0=none, 1=66, 2=F3, 3=F2)
    pub pp: u8,
}

impl VexFields {
    /// 创建默认 VEX 字段（VEX2 兼容）。
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            r: false,
            x: false,
            b: false,
            mmmmm: 1, // 0F 前缀映射
            w: false,
            vvvv: 0,
            l: false,
            pp: 0,
        }
    }

    /// 发射 VEX2 前缀 (2 字节, 0xC5 形式)。
    /// 限制: m-mmmm=1 (0F), R 位必须可用, vvvv≤7, pp 有效。
    /// 格式: [11000101] [~R vvvv L pp]
    pub fn emit_vex2(&self, sink: &mut CodeSink) {
        let byte1 = 0xC5u8;
        let byte2 = ((!self.r as u8) << 7)
            | ((self.vvvv & 0x7) << 3)
            | ((self.l as u8) << 2)
            | (self.pp & 0x3);
        sink.put_bytes(&[byte1, byte2]);
    }

    /// 发射 VEX3 前缀 (3 字节, 0xC4 形式)。
    /// 格式: [11000100] [~R ~X ~B m-mmmm] [W vvvv L pp]
    pub fn emit_vex3(&self, sink: &mut CodeSink) {
        let byte1 = 0xC4u8;
        let byte2 = ((!self.r as u8) << 7)
            | ((!self.x as u8) << 6)
            | ((!self.b as u8) << 5)
            | (self.mmmmm & 0x1F);
        let byte3 = ((self.w as u8) << 7)
            | ((self.vvvv & 0xF) << 3)
            | ((self.l as u8) << 2)
            | (self.pp & 0x3);
        sink.put_bytes(&[byte1, byte2, byte3]);
    }

    /// 自动选择 VEX2 或 VEX3。
    pub fn emit(&self, sink: &mut CodeSink) {
        if self.mmmmm == 1 && !self.r && !self.x && !self.b && !self.w && self.vvvv <= 7 {
            self.emit_vex2(sink);
        } else {
            self.emit_vex3(sink);
        }
    }
}

/// EVEX 前缀字段配置。
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub struct EvexFields {
    /// ~R (P[7])
    pub r: bool,
    /// ~X (P[6])
    pub x: bool,
    /// ~B (P[5])
    pub b: bool,
    /// ~R' (P[4])
    pub r2: bool,
    /// mm (映射: 01=0F, 10=0F38, 11=0F3A)
    pub mm: u8,
    /// W (P[15])
    pub w: bool,
    /// vvvv (P[14:11])
    pub vvvv: u8,
    /// pp (P[10:9])
    pub pp: u8,
    /// z (P[8]) — 零掩码
    pub z: bool,
    /// b (P[23]) — 广播
    pub bcast: bool,
    /// VL (P[22:21]) — 向量长度: 0=128, 1=256, 2=512, 3=reserved
    pub vl: u8,
    /// aaa (P[20:18]) — 掩码寄存器 (k0-k7)
    pub aaa: u8,
    /// V' (P[17]) — vvvv 扩展位
    pub v2: bool,
    /// D (P[16]) — 是否从内存广播
    pub d: bool,
}

impl EvexFields {
    /// 创建默认 EVEX 字段。
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            r: false,
            x: false,
            b: false,
            r2: false,
            mm: 1,
            w: false,
            vvvv: 0,
            pp: 0,
            z: false,
            bcast: false,
            vl: 0,
            aaa: 0,
            v2: false,
            d: false,
        }
    }

    /// 发射 EVEX 前缀 (4 字节, 0x62 形式)。
    /// 格式:
    ///   P0: 01100010 (0x62)
    ///   P1: [~R ~X ~B ~R' 0 mm]
    ///   P2: [W vvvv pp]
    ///   P3: [z b VL aaa V' D]
    pub fn emit(&self, sink: &mut CodeSink) {
        let p0 = 0x62u8;
        let p1 = ((!self.r as u8) << 7)
            | ((!self.x as u8) << 6)
            | ((!self.b as u8) << 5)
            | ((!self.r2 as u8) << 4)
            | (self.mm & 0x3);
        let p2 = ((self.w as u8) << 7) | ((self.vvvv & 0xF) << 3) | (self.pp & 0x3);
        let p3 = ((self.z as u8) << 7)
            | ((self.bcast as u8) << 6)
            | ((self.vl & 0x3) << 4)
            | ((self.aaa & 0x7) << 1);
        sink.put_bytes(&[p0, p1, p2, p3]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vex2_encoding() {
        let mut sink = CodeSink::new();
        let v = VexFields {
            r: false,
            x: false,
            b: false,
            mmmmm: 1,
            w: false,
            vvvv: 0,
            l: false,
            pp: 0,
        };
        v.emit_vex2(&mut sink);
        // 0xC5, 0b_0_0000_0_00 = 0x00
        assert_eq!(sink.bytes(), &[0xC5, 0x80]); // ~R=1<<7
    }

    #[test]
    fn test_vex3_encoding() {
        let mut sink = CodeSink::new();
        let v = VexFields {
            r: true,
            x: true,
            b: true,
            mmmmm: 2,
            w: true,
            vvvv: 0,
            l: true,
            pp: 1,
        };
        v.emit_vex3(&mut sink);
        // 0xC4, 0b_0_0_0_00010 = 0x02, 0b1_0000_1_01 = 0x85
        // r=true,x=true,b=true → ~R=0,~X=0,~B=0, mmmmm=2 → byte2=0x02
        // w=true,l=true,pp=1 → 1<<7 | 1<<2 | 1 = 0x85
        assert_eq!(sink.bytes(), &[0xC4, 0x02, 0x85]);
    }

    #[test]
    fn test_evex_encoding() {
        let mut sink = CodeSink::new();
        let e = EvexFields {
            r: false,
            x: false,
            b: false,
            r2: false,
            mm: 1,
            w: false,
            vvvv: 0,
            pp: 0,
            z: true,
            bcast: true,
            vl: 2,
            aaa: 5,
            v2: false,
            d: false,
        };
        e.emit(&mut sink);
        // P0: 0x62
        // P1: 0b1_1_1_1_0_01 = 0xF1
        // P2: 0b0_0000_00 = 0x00
        // P3: 0b1_1_10_101_0_0 = 0xEA
        // P1: ~R=1,~X=1,~B=1,~R'=1,mm=1 → 0xF1
        // P2: W=0,vvvv=0,pp=0 → 0x00
        // P3: z=1,bcast=1,vl=2,aaa=5 → 128+64+32+10 = 234 = 0xEA
        assert_eq!(sink.bytes(), &[0x62, 0xF1, 0x00, 0xEA]);
    }
}
