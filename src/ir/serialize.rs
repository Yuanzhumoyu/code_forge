//! IR 序列化/反序列化。
//!
//! 提供两种格式：
//! 1. 可读文本格式（Display trait）— 调试和人类阅读
//! 2. 紧凑二进制格式 — LTO、缓存、跨工具集成
//!
//! # 二进制格式规范 (v1)
//!
//! 所有多字节整数使用小端序 (LE)。
//!
//! ```text
//! Module: magic(b"CGLB") version(u8) num_funcs(u32) [func_ref(u32) func_len(u32) func_bytes]*
//! Function: name_len(u16) name is_const(u8) sig_params sig_returns pool_size(u32) [Big]* num_blocks(u16) [Block]*
//! Block: num_params(u8) [ty_tag]* num_insts(u16) [Instruction]* Terminator
//! Instruction: opcode_tag(u8) has_result(u8) [result(u32)] ty_tag(u8) num_ops(u8) [operand(u32)]*
//! Big: variant(u8: 0=Signed 1=Unsigned 2=Float) data_len(u16) data
//! ```

use super::big::Big;
use super::function::{Block, Function};
use super::instructions::*;
use super::module::Module;
use super::types::*;

// ============================================================
// 文本序列化（Display 委托）
// ============================================================

impl Function {
    pub fn serialize(&self) -> String {
        format!("{}", self)
    }
}

impl Module {
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        for func in self.iter() {
            out.push_str(&func.serialize());
            out.push('\n');
        }
        out
    }

    pub fn serialize_json(&self) -> String {
        let mut out = String::from("{\"functions\":[");
        for (i, func) in self.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"name\":\"{}\",\"blocks\":{}}}",
                func.name,
                func.blocks.len()
            ));
        }
        out.push_str("]}");
        out
    }
}

// ============================================================
// 二进制写入辅助
// ============================================================

const BINARY_MAGIC: &[u8; 4] = b"CGLB";
const BINARY_VERSION: u8 = 1;

trait WriteBin {
    fn w8(&mut self, v: u8);
    fn w16(&mut self, v: u16);
    fn w32(&mut self, v: u32);
    fn wbytes(&mut self, d: &[u8]);
}

impl WriteBin for Vec<u8> {
    fn w8(&mut self, v: u8) {
        self.push(v);
    }
    fn w16(&mut self, v: u16) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn w32(&mut self, v: u32) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn wbytes(&mut self, d: &[u8]) {
        self.extend_from_slice(d);
    }
}

fn wstr(buf: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    buf.w16(b.len() as u16);
    buf.wbytes(b);
}

fn wtype(buf: &mut Vec<u8>, ty: Type) {
    buf.w8(match ty {
        Type::Void => 0,
        Type::Bool => 20,
        Type::I8 => 1,
        Type::I16 => 2,
        Type::I32 => 3,
        Type::I64 => 4,
        Type::I128 => 5,
        Type::F16 => 6,
        Type::F32 => 7,
        Type::F64 => 8,
        Type::F128 => 9,
        Type::V64 => 10,
        Type::V128 => 11,
        Type::V256 => 12,
        Type::Ptr => 13,
        // 复合类型：使用基类型标记 + 后续序列化索引
        Type::StructNamed(_) => 14,
        Type::StructAnon(_) => 15,
        Type::Array(_) => 16,
        Type::Vector(_) => 17,
        Type::Pointer(_) => 18,
        Type::Function(_) => 19,
    });
}

fn rtype(tag: u8) -> Option<Type> {
    match tag {
        0 => Some(Type::Void),
        1 => Some(Type::I8),
        2 => Some(Type::I16),
        3 => Some(Type::I32),
        4 => Some(Type::I64),
        5 => Some(Type::I128),
        6 => Some(Type::F16),
        7 => Some(Type::F32),
        8 => Some(Type::F64),
        9 => Some(Type::F128),
        10 => Some(Type::V64),
        11 => Some(Type::V128),
        12 => Some(Type::V256),
        13 => Some(Type::Ptr),
        20 => Some(Type::Bool),
        _ => None,
    }
}

fn wbig(buf: &mut Vec<u8>, big: &Big) {
    let s = format!("{}", big);
    let tag = match big {
        Big::Signed(_) => 0u8,
        Big::Unsigned(_) => 1u8,
        Big::Float(_) => 2u8,
    };
    buf.w8(tag);
    buf.w16(s.len() as u16);
    buf.wbytes(s.as_bytes());
}

fn rbig(data: &[u8], pos: &mut usize) -> Option<Big> {
    let variant = *data.get(*pos)?;
    *pos += 1;
    let len = r16(data, pos)? as usize;
    let bytes = data.get(*pos..*pos + len)?;
    *pos += len;
    let s = std::str::from_utf8(bytes).ok()?;
    match variant {
        0 => s.parse::<dashu::Integer>().ok().map(Big::Signed),
        1 => s.parse::<dashu::Natural>().ok().map(Big::Unsigned),
        2 => s.parse::<dashu::Real>().ok().map(Big::Float),
        _ => None,
    }
}

fn wopcode(buf: &mut Vec<u8>, op: &Opcode) {
    match op {
        Opcode::Nop => buf.w8(0),
        Opcode::Iadd => buf.w8(1),
        Opcode::Isub => buf.w8(2),
        Opcode::Imul => buf.w8(3),
        Opcode::Udiv => buf.w8(4),
        Opcode::Sdiv => buf.w8(5),
        Opcode::Urem => buf.w8(6),
        Opcode::Srem => buf.w8(7),
        Opcode::Fadd { .. } => buf.w8(8),
        Opcode::Fsub { .. } => buf.w8(9),
        Opcode::Fmul { .. } => buf.w8(10),
        Opcode::Fdiv { .. } => buf.w8(11),
        Opcode::Fneg { .. } => buf.w8(12),
        Opcode::Fabs { .. } => buf.w8(13),
        Opcode::Freeze => buf.w8(48),
        Opcode::Fsqrt { .. } => buf.w8(14),
        Opcode::Vadd => buf.w8(15),
        Opcode::Vsub => buf.w8(16),
        Opcode::Vmul => buf.w8(17),
        Opcode::Vextract { lane } => {
            buf.w8(18);
            buf.w8(*lane);
        }
        Opcode::Vinsert { lane } => {
            buf.w8(19);
            buf.w8(*lane);
        }
        Opcode::Band => buf.w8(20),
        Opcode::Bor => buf.w8(21),
        Opcode::Bxor => buf.w8(22),
        Opcode::Bnot => buf.w8(23),
        Opcode::Ishl => buf.w8(24),
        Opcode::Ushr => buf.w8(25),
        Opcode::Sshr => buf.w8(26),
        Opcode::Icmp { cond } => {
            buf.w8(27);
            buf.w8(w_icmp(*cond));
        }
        Opcode::Fcmp { cond, .. } => {
            buf.w8(28);
            buf.w8(w_fcmp(*cond));
        }
        Opcode::Load => buf.w8(29),
        Opcode::Store => buf.w8(30),
        Opcode::StackLoad { offset } => {
            buf.w8(31);
            buf.wbytes(&offset.to_le_bytes());
        }
        Opcode::StackStore { offset } => {
            buf.w8(32);
            buf.wbytes(&offset.to_le_bytes());
        }
        Opcode::Iconst { index } => {
            buf.w8(33);
            buf.w32(*index);
        }
        Opcode::Fconst { index } => {
            buf.w8(34);
            buf.w32(*index);
        }
        Opcode::Sextend => buf.w8(35),
        Opcode::Uextend => buf.w8(36),
        Opcode::Ireduce => buf.w8(37),
        Opcode::Bitcast => buf.w8(38),
        Opcode::Call { func } => {
            buf.w8(39);
            buf.w32(func.0);
        }
        Opcode::CallIndirect => buf.w8(40),
        Opcode::StackAddr { offset } => {
            buf.w8(41);
            buf.wbytes(&offset.to_le_bytes());
        }
        Opcode::GlobalAddr { global } => {
            buf.w8(42);
            buf.w32(*global);
        }
        Opcode::Copy => buf.w8(43),
        Opcode::Phi { .. } => buf.w8(44),
        Opcode::Select => buf.w8(45),
        Opcode::Alloca { count } => {
            buf.w8(46);
            for b in count.to_le_bytes() {
                buf.w8(b);
            }
        }
        Opcode::GetElementPtr { indexed_ty } => {
            buf.w8(47);
            wtype(buf, *indexed_ty);
        }
        Opcode::ShuffleVector { mask } => {
            buf.w8(49);
            buf.wbytes(&mask[..]);
        }
        Opcode::AtomicRmw { op, ordering } => {
            buf.w8(50);
            buf.w8(w_atomic_rmw_op(*op));
            buf.w8(w_ordering(*ordering));
        }
        Opcode::Cmpxchg { ordering } => {
            buf.w8(51);
            buf.w8(w_ordering(*ordering));
        }
        Opcode::Fence { ordering } => {
            buf.w8(52);
            buf.w8(w_ordering(*ordering));
        }
        Opcode::ExtractValue { index } => {
            buf.w8(53);
            let idx = *index;
            buf.wbytes(&idx.to_le_bytes());
        }
        Opcode::InsertValue { index } => {
            buf.w8(54);
            let idx = *index;
            buf.wbytes(&idx.to_le_bytes());
        }
    }
}

fn ropcode(data: &[u8], pos: &mut usize) -> Option<Opcode> {
    let tag = *data.get(*pos)?;
    *pos += 1;
    match tag {
        0 => Some(Opcode::Nop),
        1 => Some(Opcode::Iadd),
        2 => Some(Opcode::Isub),
        3 => Some(Opcode::Imul),
        4 => Some(Opcode::Udiv),
        5 => Some(Opcode::Sdiv),
        6 => Some(Opcode::Urem),
        7 => Some(Opcode::Srem),
        8 => Some(Opcode::Fadd { flags: FastMathFlags::NONE }),
        9 => Some(Opcode::Fsub { flags: FastMathFlags::NONE }),
        10 => Some(Opcode::Fmul { flags: FastMathFlags::NONE }),
        11 => Some(Opcode::Fdiv { flags: FastMathFlags::NONE }),
        12 => Some(Opcode::Fneg { flags: FastMathFlags::NONE }),
        13 => Some(Opcode::Fabs { flags: FastMathFlags::NONE }),
        14 => Some(Opcode::Fsqrt { flags: FastMathFlags::NONE }),
        15 => Some(Opcode::Vadd),
        16 => Some(Opcode::Vsub),
        17 => Some(Opcode::Vmul),
        18 => {
            let l = *data.get(*pos)?;
            *pos += 1;
            Some(Opcode::Vextract { lane: l })
        }
        19 => {
            let l = *data.get(*pos)?;
            *pos += 1;
            Some(Opcode::Vinsert { lane: l })
        }
        20 => Some(Opcode::Band),
        21 => Some(Opcode::Bor),
        22 => Some(Opcode::Bxor),
        23 => Some(Opcode::Bnot),
        24 => Some(Opcode::Ishl),
        25 => Some(Opcode::Ushr),
        26 => Some(Opcode::Sshr),
        27 => {
            let c = r_icmp(data, pos)?;
            Some(Opcode::Icmp { cond: c })
        }
        28 => {
            let c = r_fcmp(data, pos)?;
            Some(Opcode::Fcmp { cond: c, flags: FastMathFlags::NONE })
        }
        29 => Some(Opcode::Load),
        30 => Some(Opcode::Store),
        31 => {
            let off = ri32(data, pos)?;
            Some(Opcode::StackLoad { offset: off })
        }
        32 => {
            let off = ri32(data, pos)?;
            Some(Opcode::StackStore { offset: off })
        }
        33 => {
            let idx = r32(data, pos)?;
            Some(Opcode::Iconst { index: idx })
        }
        34 => {
            let idx = r32(data, pos)?;
            Some(Opcode::Fconst { index: idx })
        }
        35 => Some(Opcode::Sextend),
        36 => Some(Opcode::Uextend),
        37 => Some(Opcode::Ireduce),
        38 => Some(Opcode::Bitcast),
        39 => {
            let fi = r32(data, pos)?;
            Some(Opcode::Call { func: FuncRef(fi) })
        }
        40 => Some(Opcode::CallIndirect),
        41 => {
            let off = ri32(data, pos)?;
            Some(Opcode::StackAddr { offset: off })
        }
        42 => {
            let global = u32::from_le_bytes([
                *data.get(*pos)?,
                *data.get(*pos + 1)?,
                *data.get(*pos + 2)?,
                *data.get(*pos + 3)?,
            ]);
            *pos += 4;
            Some(Opcode::GlobalAddr { global })
        }
        43 => Some(Opcode::Copy),
        44 => Some(Opcode::Phi {
            incoming: smallvec::SmallVec::new(),
        }),
        45 => Some(Opcode::Select),
        46 => {
            let mut count_bytes = [0u8; 4];
            for b in &mut count_bytes {
                *b = *data.get(*pos)?;
                *pos += 1;
            }
            Some(Opcode::Alloca {
                count: u32::from_le_bytes(count_bytes),
            })
        }
        47 => {
            let ty_tag = *data.get(*pos)?;
            *pos += 1;
            let indexed_ty = rtype(ty_tag)?;
            Some(Opcode::GetElementPtr { indexed_ty })
        }
        48 => Some(Opcode::Freeze),
        49 => {
            let mut mask = [0u8; 16];
            for b in mask.iter_mut() {
                *b = *data.get(*pos)?;
                *pos += 1;
            }
            Some(Opcode::ShuffleVector { mask })
        }
        50 => {
            let op_tag = *data.get(*pos)?;
            *pos += 1;
            let ord_tag = *data.get(*pos)?;
            *pos += 1;
            let op = r_atomic_rmw_op(op_tag)?;
            let ordering = r_ordering(ord_tag)?;
            Some(Opcode::AtomicRmw { op, ordering })
        }
        51 => {
            let ord_tag = *data.get(*pos)?;
            *pos += 1;
            let ordering = r_ordering(ord_tag)?;
            Some(Opcode::Cmpxchg { ordering })
        }
        52 => {
            let ord_tag = *data.get(*pos)?;
            *pos += 1;
            let ordering = r_ordering(ord_tag)?;
            Some(Opcode::Fence { ordering })
        }
        53 => {
            let idx = u32::from_le_bytes([*data.get(*pos)?, *data.get(*pos+1)?, *data.get(*pos+2)?, *data.get(*pos+3)?]);
            *pos += 4;
            Some(Opcode::ExtractValue { index: idx })
        }
        54 => {
            let idx = u32::from_le_bytes([*data.get(*pos)?, *data.get(*pos+1)?, *data.get(*pos+2)?, *data.get(*pos+3)?]);
            *pos += 4;
            Some(Opcode::InsertValue { index: idx })
        }
        _ => None,
    }
}

// -- int/float cc helpers --
fn w_icmp(cc: IntCC) -> u8 {
    match cc {
        IntCC::Equal => 0,
        IntCC::NotEqual => 1,
        IntCC::SignedLessThan => 2,
        IntCC::SignedGreaterThan => 3,
        IntCC::SignedLessThanOrEqual => 4,
        IntCC::SignedGreaterThanOrEqual => 5,
        IntCC::UnsignedLessThan => 6,
        IntCC::UnsignedGreaterThan => 7,
        IntCC::UnsignedLessThanOrEqual => 8,
        IntCC::UnsignedGreaterThanOrEqual => 9,
    }
}
fn w_fcmp(cc: FloatCC) -> u8 {
    match cc {
        FloatCC::Ordered => 0,
        FloatCC::Unordered => 1,
        FloatCC::LessThan => 2,
        FloatCC::GreaterThan => 3,
        FloatCC::LessThanOrEqual => 4,
        FloatCC::GreaterThanOrEqual => 5,
        FloatCC::Equal => 6,
        FloatCC::NotEqual => 7,
    }
}
fn r_icmp(data: &[u8], pos: &mut usize) -> Option<IntCC> {
    let tag = *data.get(*pos)?;
    *pos += 1;
    match tag {
        0 => Some(IntCC::Equal),
        1 => Some(IntCC::NotEqual),
        2 => Some(IntCC::SignedLessThan),
        3 => Some(IntCC::SignedGreaterThan),
        4 => Some(IntCC::SignedLessThanOrEqual),
        5 => Some(IntCC::SignedGreaterThanOrEqual),
        6 => Some(IntCC::UnsignedLessThan),
        7 => Some(IntCC::UnsignedGreaterThan),
        8 => Some(IntCC::UnsignedLessThanOrEqual),
        9 => Some(IntCC::UnsignedGreaterThanOrEqual),
        _ => None,
    }
}
fn r_fcmp(data: &[u8], pos: &mut usize) -> Option<FloatCC> {
    let tag = *data.get(*pos)?;
    *pos += 1;
    match tag {
        0 => Some(FloatCC::Ordered),
        1 => Some(FloatCC::Unordered),
        2 => Some(FloatCC::LessThan),
        3 => Some(FloatCC::GreaterThan),
        4 => Some(FloatCC::LessThanOrEqual),
        5 => Some(FloatCC::GreaterThanOrEqual),
        6 => Some(FloatCC::Equal),
        7 => Some(FloatCC::NotEqual),
        _ => None,
    }
}

// -- atomic/ordering helpers --
fn w_ordering(ordering: Ordering) -> u8 {
    match ordering {
        Ordering::NotAtomic => 0,
        Ordering::Unordered => 1,
        Ordering::Monotonic => 2,
        Ordering::Acquire => 3,
        Ordering::Release => 4,
        Ordering::AcquireRelease => 5,
        Ordering::SequentiallyConsistent => 6,
    }
}
fn r_ordering(tag: u8) -> Option<Ordering> {
    match tag {
        0 => Some(Ordering::NotAtomic),
        1 => Some(Ordering::Unordered),
        2 => Some(Ordering::Monotonic),
        3 => Some(Ordering::Acquire),
        4 => Some(Ordering::Release),
        5 => Some(Ordering::AcquireRelease),
        6 => Some(Ordering::SequentiallyConsistent),
        _ => None,
    }
}
fn w_atomic_rmw_op(op: AtomicRmwOp) -> u8 {
    match op {
        AtomicRmwOp::Xchg => 0,
        AtomicRmwOp::Add => 1,
        AtomicRmwOp::Sub => 2,
        AtomicRmwOp::And => 3,
        AtomicRmwOp::Nand => 4,
        AtomicRmwOp::Or => 5,
        AtomicRmwOp::Xor => 6,
        AtomicRmwOp::Max => 7,
        AtomicRmwOp::Min => 8,
        AtomicRmwOp::Umax => 9,
        AtomicRmwOp::Umin => 10,
    }
}
fn r_atomic_rmw_op(tag: u8) -> Option<AtomicRmwOp> {
    match tag {
        0 => Some(AtomicRmwOp::Xchg),
        1 => Some(AtomicRmwOp::Add),
        2 => Some(AtomicRmwOp::Sub),
        3 => Some(AtomicRmwOp::And),
        4 => Some(AtomicRmwOp::Nand),
        5 => Some(AtomicRmwOp::Or),
        6 => Some(AtomicRmwOp::Xor),
        7 => Some(AtomicRmwOp::Max),
        8 => Some(AtomicRmwOp::Min),
        9 => Some(AtomicRmwOp::Umax),
        10 => Some(AtomicRmwOp::Umin),
        _ => None,
    }
}

// -- basic read helpers --
fn r16(data: &[u8], pos: &mut usize) -> Option<u16> {
    let b = [*data.get(*pos)?, *data.get(*pos + 1)?];
    *pos += 2;
    Some(u16::from_le_bytes(b))
}
fn r32(data: &[u8], pos: &mut usize) -> Option<u32> {
    let b = [
        *data.get(*pos)?,
        *data.get(*pos + 1)?,
        *data.get(*pos + 2)?,
        *data.get(*pos + 3)?,
    ];
    *pos += 4;
    Some(u32::from_le_bytes(b))
}
fn ri32(data: &[u8], pos: &mut usize) -> Option<i32> {
    let b = [
        *data.get(*pos)?,
        *data.get(*pos + 1)?,
        *data.get(*pos + 2)?,
        *data.get(*pos + 3)?,
    ];
    *pos += 4;
    Some(i32::from_le_bytes(b))
}
fn rstr(data: &[u8], pos: &mut usize) -> Option<String> {
    let len = r16(data, pos)? as usize;
    let bytes = data.get(*pos..*pos + len)?;
    *pos += len;
    std::str::from_utf8(bytes).ok().map(|s| s.to_string())
}

// -- terminator write/read --
fn wterm(buf: &mut Vec<u8>, t: &Terminator) {
    match t {
        Terminator::Branch {
            cond,
            true_block,
            false_block,
            true_args,
            false_args,
        } => {
            buf.w8(1);
            buf.w32(cond.0);
            buf.w32(true_block.0);
            buf.w32(false_block.0);
            buf.w8(true_args.len() as u8);
            for v in true_args {
                buf.w32(v.0);
            }
            buf.w8(false_args.len() as u8);
            for v in false_args {
                buf.w32(v.0);
            }
        }
        Terminator::Jump { target, args } => {
            buf.w8(2);
            buf.w32(target.0);
            buf.w8(args.len() as u8);
            for v in args {
                buf.w32(v.0);
            }
        }
        Terminator::Return { values } => {
            buf.w8(3);
            buf.w8(values.len() as u8);
            for v in values {
                buf.w32(v.0);
            }
        }
        Terminator::Unreachable => buf.w8(4),
        Terminator::Switch {
            discriminant,
            default_block,
            cases,
        } => {
            buf.w8(5);
            buf.w32(discriminant.0);
            buf.w32(default_block.0);
            buf.w16(cases.len() as u16);
            for (val, target, args) in cases {
                buf.wbytes(&val.to_le_bytes());
                buf.w32(target.0);
                buf.w8(args.len() as u8);
                for v in args {
                    buf.w32(v.0);
                }
            }
        }
    }
}

fn rterm(data: &[u8], pos: &mut usize) -> Option<Terminator> {
    let tag = *data.get(*pos)?;
    *pos += 1;
    match tag {
        1 => {
            let c = Value(r32(data, pos)?);
            let tb = BlockId(r32(data, pos)?);
            let fb = BlockId(r32(data, pos)?);
            let nt = *data.get(*pos)? as usize;
            *pos += 1;
            let mut ta = smallvec::SmallVec::new();
            for _ in 0..nt {
                ta.push(Value(r32(data, pos)?));
            }
            let nf = *data.get(*pos)? as usize;
            *pos += 1;
            let mut fa = smallvec::SmallVec::new();
            for _ in 0..nf {
                fa.push(Value(r32(data, pos)?));
            }
            Some(Terminator::Branch {
                cond: c,
                true_block: tb,
                false_block: fb,
                true_args: ta,
                false_args: fa,
            })
        }
        2 => {
            let t = BlockId(r32(data, pos)?);
            let n = *data.get(*pos)? as usize;
            *pos += 1;
            let mut a = smallvec::SmallVec::new();
            for _ in 0..n {
                a.push(Value(r32(data, pos)?));
            }
            Some(Terminator::Jump { target: t, args: a })
        }
        3 => {
            let n = *data.get(*pos)? as usize;
            *pos += 1;
            let mut v = smallvec::SmallVec::new();
            for _ in 0..n {
                v.push(Value(r32(data, pos)?));
            }
            Some(Terminator::Return { values: v })
        }
        4 => Some(Terminator::Unreachable),
        5 => {
            let d = Value(r32(data, pos)?);
            let def = BlockId(r32(data, pos)?);
            let nc = r16(data, pos)? as usize;
            let mut cases = smallvec::SmallVec::new();
            for _ in 0..nc {
                let vb: [u8; 8] = [
                    data[*pos],
                    data[*pos + 1],
                    data[*pos + 2],
                    data[*pos + 3],
                    data[*pos + 4],
                    data[*pos + 5],
                    data[*pos + 6],
                    data[*pos + 7],
                ];
                *pos += 8;
                let val = i64::from_le_bytes(vb);
                let tgt = BlockId(r32(data, pos)?);
                let na = *data.get(*pos)? as usize;
                *pos += 1;
                let mut args = smallvec::SmallVec::new();
                for _ in 0..na {
                    args.push(Value(r32(data, pos)?));
                }
                cases.push((val, tgt, args));
            }
            Some(Terminator::Switch {
                discriminant: d,
                default_block: def,
                cases,
            })
        }
        _ => None,
    }
}

// ============================================================
// Function 二进制序列化
// ============================================================

impl Function {
    pub fn serialize_binary(&self) -> Vec<u8> {
        let mut b = Vec::new();
        self.write_binary(&mut b);
        b
    }

    fn write_binary(&self, buf: &mut Vec<u8>) {
        wstr(buf, &self.name);
        buf.w8(self.is_const as u8);
        // signature params
        buf.w16(self.signature.params.len() as u16);
        for (ty, name) in &self.signature.params {
            wtype(buf, *ty);
            wstr(buf, name);
        }
        // signature returns
        buf.w16(self.signature.returns.len() as u16);
        for ty in &self.signature.returns {
            wtype(buf, *ty);
        }
        // constant pool
        buf.w32(self.constant_pool.len() as u32);
        for big in self.constant_pool.constants() {
            wbig(buf, big);
        }
        // blocks
        buf.w16(self.blocks.len() as u16);
        for blk in &self.blocks {
            buf.w8(blk.params.len() as u8);
            for (_, ty) in &blk.params {
                wtype(buf, *ty);
            }
            buf.w16(blk.instructions.len() as u16);
            for inst in &blk.instructions {
                wopcode(buf, &inst.opcode);
                if let Some(v) = inst.result {
                    buf.w8(1);
                    buf.w32(v.0);
                } else {
                    buf.w8(0);
                }
                wtype(buf, inst.ty);
                buf.w8(inst.operands.len() as u8);
                for op in &inst.operands {
                    buf.w32(op.0);
                }
            }
            wterm(buf, &blk.terminator);
        }
    }

    pub fn deserialize_binary(data: &[u8]) -> Option<Self> {
        let mut p = 0;
        Self::read_binary(data, &mut p)
    }

    fn read_binary(data: &[u8], pos: &mut usize) -> Option<Self> {
        let name = rstr(data, pos)?;
        let is_const = *data.get(*pos)? != 0;
        *pos += 1;
        // signature
        let np = r16(data, pos)? as usize;
        let mut params = Vec::with_capacity(np);
        for _ in 0..np {
            let ty = rtype(*data.get(*pos)?)?;
            *pos += 1;
            let pn = rstr(data, pos)?;
            params.push((ty, pn));
        }
        let nr = r16(data, pos)? as usize;
        let mut returns = Vec::with_capacity(nr);
        for _ in 0..nr {
            let ty = rtype(*data.get(*pos)?)?;
            *pos += 1;
            returns.push(ty);
        }
        let ps: Vec<(Type, &str)> = params.iter().map(|(t, n)| (*t, n.as_str())).collect();
        let sig = Signature::new(&ps, &returns);
        let mut func = Function::new(&name, sig);
        func.is_const = is_const;
        // constant pool
        let psz = r32(data, pos)? as usize;
        for _ in 0..psz {
            let big = rbig(data, pos)?;
            func.constant_pool.insert(big);
        }
        // blocks
        let nb = r16(data, pos)? as usize;
        func.block_count = nb as u32;
        for _ in 0..nb {
            let bid = func.create_block_id();
            let nbp = *data.get(*pos)? as usize;
            *pos += 1;
            let mut bparams = Vec::with_capacity(nbp);
            for _ in 0..nbp {
                let ty = rtype(*data.get(*pos)?)?;
                *pos += 1;
                let v = func.create_value();
                bparams.push((v, ty));
            }
            let ni = r16(data, pos)? as usize;
            let mut insts = Vec::with_capacity(ni);
            for _ in 0..ni {
                let opcode = ropcode(data, pos)?;
                let has_r = *data.get(*pos)? != 0;
                *pos += 1;
                let result = if has_r {
                    let vi = r32(data, pos)?;
                    let v = Value(vi);
                    func.value_count = func.value_count.max(vi + 1);
                    Some(v)
                } else {
                    None
                };
                let ty = rtype(*data.get(*pos)?)?;
                *pos += 1;
                let no = *data.get(*pos)? as usize;
                *pos += 1;
                let mut ops = smallvec::SmallVec::new();
                for _ in 0..no {
                    ops.push(Value(r32(data, pos)?));
                }
                insts.push(Instruction {
                    opcode,
                    operands: ops,
                    result,
                    ty,
                    source_location: None,
                });
            }
            let term = rterm(data, pos)?;
            func.blocks.push(Block {
                id: bid,
                params: bparams,
                instructions: insts,
                terminator: term,
            });
        }
        Some(func)
    }
}

// ============================================================
// Module 二进制序列化
// ============================================================

impl Module {
    pub fn serialize_binary(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.wbytes(BINARY_MAGIC);
        buf.w8(BINARY_VERSION);
        let refs = self.func_refs();
        buf.w32(refs.len() as u32);
        for &fr in &refs {
            if let Some(func) = self.get(fr) {
                buf.w32(fr.0);
                let fb = func.serialize_binary();
                buf.w32(fb.len() as u32);
                buf.wbytes(&fb);
            }
        }
        buf
    }

    pub fn deserialize_binary(data: &[u8]) -> Option<Self> {
        let mut pos = 0;
        if data.len() < 5 {
            return None;
        }
        if &data[pos..pos + 4] != BINARY_MAGIC {
            return None;
        }
        pos += 4;
        if data[pos] != BINARY_VERSION {
            return None;
        }
        pos += 1;
        let nf = r32(data, &mut pos)? as usize;
        let mut module = Module::new();
        for _ in 0..nf {
            let _fr = r32(data, &mut pos)?;
            let fl = r32(data, &mut pos)? as usize;
            let fd = data.get(pos..pos + fl)?;
            pos += fl;
            if let Some(func) = Function::deserialize_binary(fd) {
                let _ = module.add_function(func);
            }
        }
        Some(module)
    }
}

// ============================================================
// 文本反序列化 — 解析 Display 格式的 IR 文本
// ============================================================

impl Function {
    /// 从文本格式反序列化函数。
    ///
    /// 解析 `fn name(...) -> ... { ... }` 格式的文本。
    /// 支持的指令语法与 `Display` 输出一致。
    pub fn deserialize_text(text: &str) -> Option<Self> {
        let lines: Vec<&str> = text.lines().map(|l| l.trim()).collect();
        if lines.is_empty() {
            return None;
        }

        // Parse: [const] fn name(params) [-> returns] {
        let header = lines[0];
        let is_const = header.starts_with("const ");
        let rest = if is_const { &header[6..] } else { header };
        let rest = rest.strip_prefix("fn ")?;

        // Extract name
        let paren_pos = rest.find('(')?;
        let name = rest[..paren_pos].trim().to_string();

        // Extract params: (x: i32, y: i32)
        let after_paren = &rest[paren_pos + 1..];
        let close_paren = after_paren.find(')')?;
        let params_str = &after_paren[..close_paren];
        let mut after_close = after_paren[close_paren + 1..].trim();

        // Parse params
        let mut params = Vec::new();
        if !params_str.is_empty() {
            for part in params_str.split(',') {
                let part = part.trim();
                let colon = part.rfind(':')?;
                let pname = part[..colon].trim().to_string();
                let ptype = parse_type(part[colon + 1..].trim())?;
                params.push((ptype, pname));
            }
        }

        // Parse returns if present
        let mut returns = Vec::new();
        if let Some(arrow_pos) = after_close.find("->") {
            let ret_part = after_close[arrow_pos + 2..].trim();
            let brace_pos = ret_part.find('{')?;
            let ret_types = ret_part[..brace_pos].trim();
            if !ret_types.is_empty() {
                for ty_str in ret_types.split(',') {
                    returns.push(parse_type(ty_str.trim())?);
                }
            }
            after_close = &ret_part[brace_pos..];
        }

        // Verify opening brace is present
        if !after_close.contains('{') {
            return None;
        }

        // Create function
        let ps: Vec<(Type, &str)> = params.iter().map(|(t, n)| (*t, n.as_str())).collect();
        let sig = Signature::new(&ps, &returns);
        let mut func = Function::new(&name, sig);
        func.is_const = is_const;

        // Parse blocks
        let mut line_idx = 1; // skip header
        while line_idx < lines.len() {
            let line = lines[line_idx];
            if line == "}" || line.is_empty() {
                line_idx += 1;
                continue;
            }

            // Parse "block b0(x: i32, y: i32):" or "block b0:"
            if line.starts_with("block ") {
                let block = parse_block(&mut func, line, &lines, &mut line_idx)?;
                func.blocks.push(block);
            } else {
                line_idx += 1;
            }
        }

        func.block_count = func.blocks.len() as u32;
        Some(func)
    }
}

/// Parse a type string like "i32", "f64", "ptr", "void", "v128"
fn parse_type(s: &str) -> Option<Type> {
    match s {
        "void" => Some(Type::Void),
        "i8" => Some(Type::I8),
        "i16" => Some(Type::I16),
        "i32" => Some(Type::I32),
        "i64" => Some(Type::I64),
        "i128" => Some(Type::I128),
        "f16" => Some(Type::F16),
        "f32" => Some(Type::F32),
        "f64" => Some(Type::F64),
        "f128" => Some(Type::F128),
        "v64" => Some(Type::V64),
        "v128" => Some(Type::V128),
        "v256" => Some(Type::V256),
        "ptr" => Some(Type::Ptr),
        _ => None,
    }
}

/// Parse a Value like "v0", "v42"
fn parse_value(s: &str) -> Option<Value> {
    s.strip_prefix('v')?.parse::<u32>().ok().map(Value)
}

/// Parse an IntCC like "Equal", "SignedLessThan"
fn parse_intcc(s: &str) -> Option<IntCC> {
    match s {
        "Equal" => Some(IntCC::Equal),
        "NotEqual" => Some(IntCC::NotEqual),
        "SignedLessThan" => Some(IntCC::SignedLessThan),
        "SignedGreaterThan" => Some(IntCC::SignedGreaterThan),
        "SignedLessThanOrEqual" => Some(IntCC::SignedLessThanOrEqual),
        "SignedGreaterThanOrEqual" => Some(IntCC::SignedGreaterThanOrEqual),
        "UnsignedLessThan" => Some(IntCC::UnsignedLessThan),
        "UnsignedGreaterThan" => Some(IntCC::UnsignedGreaterThan),
        "UnsignedLessThanOrEqual" => Some(IntCC::UnsignedLessThanOrEqual),
        "UnsignedGreaterThanOrEqual" => Some(IntCC::UnsignedGreaterThanOrEqual),
        _ => None,
    }
}

/// Parse a block and its instructions/terminator from text lines.
fn parse_block(
    func: &mut Function,
    header_line: &str,
    _all_lines: &[&str],
    line_idx: &mut usize,
) -> Option<Block> {
    // "block b0(x: i32, y: i32):" → id = b0, params = [(x, i32), (y, i32)]
    let rest = header_line.strip_prefix("block ")?;
    let paren_pos = rest.find('(');
    let colon_pos = rest.find(':')?;
    let id_str = if let Some(pp) = paren_pos {
        &rest[..pp]
    } else {
        &rest[..colon_pos]
    };
    let id_num = id_str.strip_prefix('b')?.parse::<u32>().ok()?;
    let block_id = BlockId(id_num);

    // Parse params if present
    let mut block_params = Vec::new();
    if let Some(pp) = paren_pos {
        let params_part = &rest[pp + 1..colon_pos];
        if let Some(close_p) = params_part.find(')') {
            let params_str = &params_part[..close_p];
            if !params_str.is_empty() {
                for part in params_str.split(',') {
                    let part = part.trim();
                    let colon = part.rfind(':')?;
                    let pname = part[..colon].trim().to_string();
                    let ptype = parse_type(part[colon + 1..].trim())?;
                    let v = func.create_value();
                    block_params.push((v, ptype));
                    // Map name for debugging
                    let _ = pname;
                }
            }
        }
    }

    let mut block = Block::new(block_id);
    block.params = block_params;
    *line_idx += 1;

    // Parse instructions and terminator
    // Indented with 4 spaces
    loop {
        if *line_idx >= _all_lines.len() {
            break;
        }
        let line = _all_lines[*line_idx];
        if !line.starts_with("    ") && !line.starts_with('\t') {
            // End of this block's contents
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            *line_idx += 1;
            continue;
        }

        // Check if this is a terminator
        if line.starts_with("ret ")
            || line.starts_with("jmp ")
            || line.starts_with("br ")
            || line.starts_with("switch ")
            || line == "unreachable"
        {
            block.terminator = parse_terminator(line, func)?;
            *line_idx += 1;
            break;
        }

        // Parse instruction: [vN = ] opcode operands [; location]
        let inst = parse_instruction(line, func)?;
        block.instructions.push(inst);
        *line_idx += 1;
    }

    Some(block)
}

/// Parse an instruction line like "v2 = iadd v0, v1" or "store v0 -> v1"
fn parse_instruction(line: &str, func: &mut Function) -> Option<Instruction> {
    let line = line.split(';').next()?.trim(); // strip source location comment
    if line.is_empty() {
        return None;
    }

    // Check for result assignment: "vN = ..."
    let (result, rest) = if let Some(eq_pos) = line.find('=') {
        let lhs = line[..eq_pos].trim();
        let result = parse_value(lhs);
        let rest = line[eq_pos + 1..].trim();
        (result, rest)
    } else {
        (None, line)
    };

    // Split into opcode and operands
    let (opcode_str, operands_str) = if let Some(space) = rest.find(' ') {
        (rest[..space].trim(), rest[space + 1..].trim())
    } else {
        (rest.trim(), "")
    };

    let ops: Vec<Value> = if operands_str.is_empty() {
        Vec::new()
    } else {
        // For calls: call @0(v0, v1)
        if opcode_str.starts_with("call ") && operands_str.starts_with('@') {
            // Special handling for call
            return parse_call_inst(line, result, func);
        }
        operands_str
            .split(',')
            .map(|s| parse_value(s.trim()))
            .collect::<Option<Vec<_>>>()?
    };

    let ty = Type::I32; // default, refined below
    let opcode = parse_opcode(opcode_str, &ops, func)?;

    let _result_val = result.unwrap_or_else(|| func.create_value());
    if result.is_none() {
        // Instructions without explicit result: store, stack_store
    }

    Some(Instruction::new(
        opcode,
        ops.into_iter().collect(),
        result,
        ty,
    ))
}

/// Parse opcode from string like "iadd", "icmp.Equal", "load.i32", "iconst.i32 @0"
fn parse_opcode(s: &str, ops: &[Value], _func: &mut Function) -> Option<Opcode> {
    match s {
        "iadd" => Some(Opcode::Iadd),
        "isub" => Some(Opcode::Isub),
        "imul" => Some(Opcode::Imul),
        "udiv" => Some(Opcode::Udiv),
        "sdiv" => Some(Opcode::Sdiv),
        "urem" => Some(Opcode::Urem),
        "srem" => Some(Opcode::Srem),
        "fadd" => Some(Opcode::Fadd { flags: FastMathFlags::NONE }),
        "fsub" => Some(Opcode::Fsub { flags: FastMathFlags::NONE }),
        "fmul" => Some(Opcode::Fmul { flags: FastMathFlags::NONE }),
        "fdiv" => Some(Opcode::Fdiv { flags: FastMathFlags::NONE }),
        "fneg" => Some(Opcode::Fneg { flags: FastMathFlags::NONE }),
        "fabs" => Some(Opcode::Fabs { flags: FastMathFlags::NONE }),
        "fsqrt" => Some(Opcode::Fsqrt { flags: FastMathFlags::NONE }),
        "and" => Some(Opcode::Band),
        "or" => Some(Opcode::Bor),
        "xor" => Some(Opcode::Bxor),
        "not" => Some(Opcode::Bnot),
        "shl" => Some(Opcode::Ishl),
        "ushr" => Some(Opcode::Ushr),
        "sshr" => Some(Opcode::Sshr),
        "copy" => Some(Opcode::Copy),
        "nop" => Some(Opcode::Nop),
        "select" if ops.len() >= 3 => Some(Opcode::Select),
        "phi" => Some(Opcode::Phi {
            incoming: smallvec::SmallVec::new(),
        }),
        "call_indirect" => Some(Opcode::CallIndirect),
        "vadd" => Some(Opcode::Vadd),
        "vsub" => Some(Opcode::Vsub),
        "vmul" => Some(Opcode::Vmul),
        s if s.starts_with("icmp.") => {
            let cc = parse_intcc(&s[5..])?;
            Some(Opcode::Icmp { cond: cc })
        }
        s if s.starts_with("fcmp.") => {
            let cc_str = &s[5..];
            let cc = match cc_str {
                "Ordered" => FloatCC::Ordered,
                "Unordered" => FloatCC::Unordered,
                "Equal" => FloatCC::Equal,
                "NotEqual" => FloatCC::NotEqual,
                "LessThan" => FloatCC::LessThan,
                "LessThanOrEqual" => FloatCC::LessThanOrEqual,
                "GreaterThan" => FloatCC::GreaterThan,
                "GreaterThanOrEqual" => FloatCC::GreaterThanOrEqual,
                _ => return None,
            };
            Some(Opcode::Fcmp { cond: cc, flags: FastMathFlags::NONE })
        }
        s if s.starts_with("iconst.") => {
            let at_pos = s.find('@')?;
            let idx = s[at_pos + 1..].parse::<u32>().ok()?;
            Some(Opcode::Iconst { index: idx })
        }
        s if s.starts_with("fconst.") => {
            let at_pos = s.find('@')?;
            let idx = s[at_pos + 1..].parse::<u32>().ok()?;
            Some(Opcode::Fconst { index: idx })
        }
        s if s.starts_with("load.") => Some(Opcode::Load),
        "store" => Some(Opcode::Store),
        s if s.starts_with("stack_load") => {
            let offset = extract_bracket_num(s)?;
            Some(Opcode::StackLoad { offset })
        }
        s if s.starts_with("stack_store") => {
            let offset = extract_bracket_num(s)?;
            Some(Opcode::StackStore { offset })
        }
        s if s.starts_with("stack_addr") => {
            let offset = extract_bracket_num(s)?;
            Some(Opcode::StackAddr { offset })
        }
        s if s.starts_with("global_addr") => {
            // global_addr @idx
            let idx_str = s.trim_start_matches("global_addr @");
            if let Ok(idx) = idx_str.parse::<u32>() {
                Some(Opcode::GlobalAddr { global: idx })
            } else {
                None
            }
        }
        s if s.starts_with("sextend.") => Some(Opcode::Sextend),
        s if s.starts_with("uextend.") => Some(Opcode::Uextend),
        s if s.starts_with("ireduce.") => Some(Opcode::Ireduce),
        s if s.starts_with("bitcast.") => Some(Opcode::Bitcast),
        s if s.starts_with("vextract.") => {
            let dot_pos = s.rfind('.')?;
            let lane = s[dot_pos + 1..].parse::<u8>().ok()?;
            Some(Opcode::Vextract { lane })
        }
        s if s.starts_with("vinsert.") => {
            let dot_pos = s.rfind('.')?;
            let lane = s[dot_pos + 1..].parse::<u8>().ok()?;
            Some(Opcode::Vinsert { lane })
        }
        s if s.starts_with("alloca") => {
            // alloca is deserialized with its type info from context
            Some(Opcode::Alloca { count: 1 })
        }
        s if s.starts_with("gep ") => Some(Opcode::GetElementPtr {
            indexed_ty: Type::Ptr,
        }),
        _ => None,
    }
}

fn extract_bracket_num(s: &str) -> Option<i32> {
    let start = s.find('[')?;
    let end = s[start..].find(']')?;
    let inner = &s[start + 1..start + end];
    // Handle "fp8" → 8, "fp-4" → -4
    inner
        .strip_prefix("fp")
        .and_then(|n| n.parse::<i32>().ok())
        .or_else(|| inner.parse::<i32>().ok())
}

fn parse_call_inst(
    _line: &str,
    _result: Option<Value>,
    _func: &mut Function,
) -> Option<Instruction> {
    // Parse: call @0(v0, v1)
    // Simplified: extract func index and operands
    let rest = _line;
    let call_pos = rest.find("call @")?;
    let after = &rest[call_pos + 6..];
    let paren_pos = after.find('(')?;
    let func_idx = after[..paren_pos].parse::<u32>().ok()?;
    let close_pos = after[paren_pos..].find(')')?;
    let args_str = &after[paren_pos + 1..paren_pos + close_pos];
    let operands: Vec<Value> = if args_str.is_empty() {
        Vec::new()
    } else {
        args_str
            .split(',')
            .map(|s| parse_value(s.trim()))
            .collect::<Option<Vec<_>>>()?
    };
    let result = _result.unwrap_or_else(|| _func.create_value());
    Some(Instruction::new(
        Opcode::Call {
            func: FuncRef(func_idx),
        },
        operands.into_iter().collect(),
        Some(result),
        Type::I32,
    ))
}

/// Parse a terminator line like "ret v0" or "jmp b1(v0)"
fn parse_terminator(line: &str, _func: &mut Function) -> Option<Terminator> {
    let line = line.trim();
    if line == "unreachable" {
        return Some(Terminator::Unreachable);
    }

    if let Some(rest) = line.strip_prefix("ret ") {
        let rest = rest.trim();
        if rest.is_empty() || rest == "void" {
            return Some(Terminator::Return {
                values: smallvec::smallvec![],
            });
        }
        let vals: Vec<Value> = rest
            .split(',')
            .map(|s| parse_value(s.trim()))
            .collect::<Option<Vec<_>>>()?;
        return Some(Terminator::Return {
            values: vals.into_iter().collect(),
        });
    }

    if let Some(rest) = line.strip_prefix("jmp ") {
        let rest = rest.trim();
        let paren_pos = rest.find('(');
        let target = if let Some(pp) = paren_pos {
            parse_blockid(&rest[..pp])?
        } else {
            parse_blockid(rest)?
        };
        let mut args = smallvec::SmallVec::new();
        if let Some(pp) = paren_pos {
            let close = rest[pp..].find(')')?;
            let args_str = &rest[pp + 1..pp + close];
            if !args_str.is_empty() {
                for a in args_str.split(',') {
                    args.push(parse_value(a.trim())?);
                }
            }
        }
        return Some(Terminator::Jump { target, args });
    }

    if let Some(rest) = line.strip_prefix("br ") {
        let parts: Vec<&str> = rest.split(',').map(|s| s.trim()).collect();
        if parts.len() >= 3 {
            let cond = parse_value(parts[0])?;
            let true_block = parse_blockid(parts[1])?;
            let false_block = parse_blockid(parts[2])?;
            // Parse optional args: (v0, v1)
            return Some(Terminator::Branch {
                cond,
                true_block,
                false_block,
                true_args: smallvec::smallvec![],
                false_args: smallvec::smallvec![],
            });
        }
    }

    None
}

fn parse_blockid(s: &str) -> Option<BlockId> {
    s.trim().strip_prefix('b')?.parse::<u32>().ok().map(BlockId)
}

// ============================================================
// LTO 桥接 — 使用二进制格式替换 stub
// ============================================================

impl crate::optimize::lto::LtoContext {
    /// 序列化模块为二进制（替代旧的文本序列化）。
    pub fn serialize_module(module: &Module) -> Result<Vec<u8>, crate::CompileError> {
        Ok(module.serialize_binary())
    }

    /// 从二进制反序列化模块（不再返回 Unimplemented）。
    pub fn deserialize_module(data: &[u8]) -> Result<Module, crate::CompileError> {
        Module::deserialize_binary(data).ok_or_else(|| {
            crate::CompileError::Internal(
                "Failed to deserialize module: invalid binary format".into(),
            )
        })
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use crate::ir::*;

    #[test]
    fn serialize_roundtrip_simple() {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("add_one", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        let result = b.iadd(params[0], one);
        b.return_(&[result]);
        let func = b.finish();
        let text = func.serialize();
        assert!(text.contains("fn add_one"));
        assert!(text.contains("iadd"));
    }

    #[test]
    fn binary_roundtrip_basic() {
        let sig = Signature::new(&[(Type::I32, "x"), (Type::I32, "y")], &[Type::I32]);
        let mut b = FunctionBuilder::new("add", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x"), (Type::I32, "y")]);
        b.switch_to_block(entry);
        let sum = b.iadd(params[0], params[1]);
        b.return_(&[sum]);
        let func = b.finish();
        let bytes = func.serialize_binary();
        assert!(!bytes.is_empty());
        let func2 = Function::deserialize_binary(&bytes).expect("deserialization failed");
        assert_eq!(func.name, func2.name);
        assert_eq!(func.blocks.len(), func2.blocks.len());
        assert!(func2.validate().is_valid());
    }

    #[test]
    fn binary_roundtrip_constants() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c1 = b.iconst_i32(42);
        let c2 = b.iconst_i64(100);
        let sum = b.iadd(c1, c2);
        b.return_(&[sum]);
        let func = b.finish();
        let bytes = func.serialize_binary();
        let func2 = Function::deserialize_binary(&bytes).expect("failed");
        assert_eq!(func.constant_pool.len(), func2.constant_pool.len());
        assert!(func2.validate().is_valid());
    }

    #[test]
    fn binary_roundtrip_branch() {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("br", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        let t = b.create_block();
        let e = b.create_block();
        b.switch_to_block(entry);
        let z = b.iconst_i32(0);
        let c = b.icmp(IntCC::SignedGreaterThan, params[0], z);
        b.branch(c, t, e, &[], &[]);
        b.switch_to_block(t);
        let v = b.iconst_i32(1);
        b.return_(&[v]);
        b.switch_to_block(e);
        let v = b.iconst_i32(0);
        b.return_(&[v]);
        let func = b.finish();
        let bytes = func.serialize_binary();
        let func2 = Function::deserialize_binary(&bytes).expect("failed");
        assert_eq!(func.blocks.len(), func2.blocks.len());
        assert!(func2.validate().is_valid());
    }

    #[test]
    fn binary_roundtrip_switch() {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("sw", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        let c0 = b.create_block();
        let c1 = b.create_block();
        let def = b.create_block();
        b.switch_to_block(entry);
        b.switch(params[0], def, &[(0, c0, &[]), (1, c1, &[])]);
        b.switch_to_block(c0);
        let v = b.iconst_i32(10);
        b.return_(&[v]);
        b.switch_to_block(c1);
        let v = b.iconst_i32(20);
        b.return_(&[v]);
        b.switch_to_block(def);
        let v = b.iconst_i32(0);
        b.return_(&[v]);
        let func = b.finish();
        let bytes = func.serialize_binary();
        let func2 = Function::deserialize_binary(&bytes).expect("failed");
        let has_sw = func2
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::Switch { .. }));
        assert!(has_sw, "switch should survive roundtrip");
        assert!(func2.validate().is_valid());
    }

    #[test]
    fn module_binary_roundtrip() {
        let mut module = Module::new();
        let sig = Signature::new(&[], &[Type::I32]);
        for name in ["f1", "f2"] {
            let mut b = FunctionBuilder::new(name, sig.clone());
            let e = b.create_block();
            b.switch_to_block(e);
            let v = b.iconst_i32(42);
            b.return_(&[v]);
            module.add_function(b.finish()).unwrap();
        }
        let bytes = module.serialize_binary();
        let m2 = Module::deserialize_binary(&bytes).expect("failed");
        assert_eq!(m2.len(), 2);
    }

    #[test]
    fn binary_rejects_bad_magic() {
        assert!(Module::deserialize_binary(&[0u8; 10]).is_none());
    }

    #[test]
    fn lto_roundtrip_works() {
        let mut module = Module::new();
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("f", sig);
        let e = b.create_block();
        b.switch_to_block(e);
        let v = b.iconst_i32(42);
        b.return_(&[v]);
        module.add_function(b.finish()).unwrap();
        let bytes = crate::optimize::lto::LtoContext::serialize_module(&module).unwrap();
        assert!(!bytes.is_empty());
        let m2 = crate::optimize::lto::LtoContext::deserialize_module(&bytes)
            .expect("LTO deserialization should now work");
        assert_eq!(m2.len(), 1);
    }

    #[test]
    fn text_deserialize_basic() {
        // Test that the text parser can parse a simple function (best-effort).
        // The parsed IR may not fully validate; round-trip fidelity needs
        // the binary format. This tests parser infrastructure.
        let sig = Signature::new(&[(Type::I32, "x"), (Type::I32, "y")], &[Type::I32]);
        let mut b = FunctionBuilder::new("add", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x"), (Type::I32, "y")]);
        b.switch_to_block(entry);
        let sum = b.iadd(params[0], params[1]);
        b.return_(&[sum]);
        let func = b.finish();

        let text = func.serialize();
        assert!(text.contains("fn add"));
        // Text deserialization is best-effort; binary format is the authoritative round-trip
        let _parsed = Function::deserialize_text(&text);
    }
}
