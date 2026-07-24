//! 调用约定（ABI）抽象。
//!
//! 提供参数/返回值位置计算和栈帧布局。
//! 遵循 System V AMD64 ABI 的八分法（eightfold classification）。

use crate::ir::{PReg, RegClass, Type};

/// 八分法分类 — System V AMD64 ABI 的基本分类单位。
///
/// 每个八字节（8-byte）被分类为以下之一。
/// 参考 AMD64 ABI §3.2.3。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgClass {
    /// 整数类型（通用寄存器）。
    Integer,
    /// 单精度/双精度浮点或 SSE 向量类型。
    Sse,
    /// SSE 向量的高半部分。
    SseUp,
    /// 整体通过栈传递（MEMORY）。
    Memory,
    /// 无分类（用于填充）。
    NoClass,
    /// x87 浮点。
    X87,
    /// x87 浮点的高半部分。
    X87Up,
    /// 复数 x87 类型。
    ComplexX87,
}

/// 将单个 IR 类型分类为八分法类别列表。
///
/// 每种类型产生 1 个或多个八字节的分类结果。
/// 结构体等复合类型会被递归拆解。
pub fn classify_type(ty: &Type) -> Vec<ArgClass> {
    match ty {
        Type::Void => vec![ArgClass::NoClass],
        Type::Bool
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::I128
        | Type::Ptr
        | Type::Pointer(_)
        | Type::Function(_) => {
            let sz = ty.size_bytes();
            let num_8bytes = sz.div_ceil(8) as usize;
            vec![ArgClass::Integer; num_8bytes]
        }
        Type::F16 | Type::F32 => vec![ArgClass::Sse],
        Type::F64 => vec![ArgClass::Sse],
        Type::F128 => vec![ArgClass::Sse, ArgClass::SseUp],
        Type::V64 => vec![ArgClass::Sse],
        Type::V128 => vec![ArgClass::Sse, ArgClass::SseUp],
        Type::V256 => {
            vec![
                ArgClass::Sse,
                ArgClass::SseUp,
                ArgClass::SseUp,
                ArgClass::SseUp,
            ]
        }
        // 复合类型（无 Context 时保守处理）
        Type::StructNamed(_) | Type::StructAnon(_) | Type::Array(_) | Type::Vector(_) => {
            let sz = ty.size_bytes();
            if sz > 16 {
                vec![ArgClass::Memory]
            } else {
                let num_8bytes = sz.div_ceil(8) as usize;
                vec![ArgClass::Integer; num_8bytes]
            }
        }
    }
}

/// 合并相邻的八分法分类（参照 AMD64 ABI §3.2.3 合并规则）。
pub fn merge_classes(classes: &mut Vec<ArgClass>) {
    let mut i = 0;
    while i + 1 < classes.len() {
        let (a, b) = (classes[i], classes[i + 1]);
        let merged = match (a, b) {
            // (MEMORY, ANY) → MEMORY
            (ArgClass::Memory, _) | (_, ArgClass::Memory) => Some(ArgClass::Memory),
            // (INTEGER, INTEGER) → INTEGER
            (ArgClass::Integer, ArgClass::Integer) => Some(ArgClass::Integer),
            // (SSE, SSEUP) → SSE (SSEUP 是前一个 SSE 的高半部分)
            (ArgClass::Sse, ArgClass::SseUp) => Some(ArgClass::Sse),
            // (SSEUP, SSEUP) → SSEUP
            (ArgClass::SseUp, ArgClass::SseUp) => Some(ArgClass::SseUp),
            // (X87, X87UP) → X87
            (ArgClass::X87, ArgClass::X87Up) => Some(ArgClass::X87),
            // (NO_CLASS, ANY) → ANY
            (ArgClass::NoClass, other) => Some(other),
            (other, ArgClass::NoClass) => Some(other),
            // 其他组合 → MEMORY（不兼容的合并）
            _ => Some(ArgClass::Memory),
        };
        if let Some(m) = merged {
            classes[i] = m;
            classes.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

/// 判断类型是否需要通过隐式指针（sret）返回。
pub fn requires_sret(ty: &Type) -> bool {
    let classes = classify_type(ty);
    classes.contains(&ArgClass::Memory) || ty.size_bytes() > 16
}

/// 参数或返回值的位置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgLocation {
    /// 位于寄存器中。
    Reg { reg: PReg },
    /// 位于栈上。
    Stack { offset: i32 },
}

/// 栈帧布局。
#[derive(Clone, Debug)]
pub struct StackFrame {
    /// 帧总大小。
    pub frame_size: u32,
    /// 参数区域大小。
    pub arg_area_size: u32,
    /// 局部变量区域大小。
    pub local_area_size: u32,
    /// 被调用者保存寄存器区域大小。
    pub callee_save_area_size: u32,
}

impl StackFrame {
    pub fn new() -> Self {
        Self {
            frame_size: 0,
            arg_area_size: 0,
            local_area_size: 0,
            callee_save_area_size: 0,
        }
    }

    /// 计算对齐后的帧大小。
    pub fn compute(&mut self, align: u32) -> u32 {
        self.frame_size = self.arg_area_size + self.local_area_size + self.callee_save_area_size;
        self.frame_size = (self.frame_size + align - 1) & !(align - 1);
        self.frame_size
    }
}

impl Default for StackFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// 调用约定 trait。
pub trait CallingConvention: Send + Sync {
    /// 调用约定名称。
    fn name(&self) -> &str;

    /// 计算参数位置。
    fn compute_arg_locs(&self, args: &[Type]) -> Vec<ArgLocation>;

    /// 计算返回值位置。
    fn compute_ret_locs(&self, rets: &[Type]) -> Vec<ArgLocation>;

    /// 栈对齐要求。
    fn stack_align(&self) -> u32 {
        16
    }
}

// ============================================================
// System V AMD64 ABI
// ============================================================

/// System V AMD64 调用约定。
///
/// 前 6 个整数/指针参数通过寄存器传递（rdi, rsi, rdx, rcx, r8, r9），
/// 前 8 个浮点参数通过 xmm0-xmm7 传递，
/// 其余通过栈传递。返回值在 rax（整数）或 xmm0（浮点）。
pub struct SysVAbi;

impl CallingConvention for SysVAbi {
    fn name(&self) -> &str {
        "SystemV"
    }

    fn compute_arg_locs(&self, args: &[Type]) -> Vec<ArgLocation> {
        let int_arg_regs = [0u8, 1, 2, 3, 4, 5]; // rdi, rsi, rdx, rcx, r8, r9
        let float_arg_regs = [16u8, 17, 18, 19, 20, 21, 22, 23]; // xmm0-xmm7
        let mut int_idx = 0;
        let mut float_idx = 0;
        let mut stack_offset = 0i32;
        let mut locs = Vec::new();

        for ty in args {
            // Sret 参数：通过隐式指针传递
            if requires_sret(ty) {
                // Sret 指针占用一个 INTEGER 寄存器（rdi）
                if int_idx < int_arg_regs.len() {
                    locs.push(ArgLocation::Reg {
                        reg: PReg::new(int_arg_regs[int_idx], RegClass::Int),
                    });
                    int_idx += 1;
                } else {
                    locs.push(ArgLocation::Stack {
                        offset: stack_offset,
                    });
                    stack_offset += 8;
                }
                continue;
            }

            let classes = classify_type(ty);
            for &class in &classes {
                match class {
                    ArgClass::Integer => {
                        if int_idx < int_arg_regs.len() {
                            locs.push(ArgLocation::Reg {
                                reg: PReg::new(int_arg_regs[int_idx], RegClass::Int),
                            });
                            int_idx += 1;
                        } else {
                            locs.push(ArgLocation::Stack {
                                offset: stack_offset,
                            });
                            stack_offset += 8;
                        }
                    }
                    ArgClass::Sse | ArgClass::SseUp => {
                        if float_idx < float_arg_regs.len() {
                            locs.push(ArgLocation::Reg {
                                reg: PReg::new(float_arg_regs[float_idx], RegClass::Float),
                            });
                            float_idx += 1;
                        } else {
                            locs.push(ArgLocation::Stack {
                                offset: stack_offset,
                            });
                            stack_offset += 8;
                        }
                    }
                    ArgClass::Memory => {
                        // MEMORY 类：整个参数通过栈传递
                        locs.push(ArgLocation::Stack {
                            offset: stack_offset,
                        });
                        stack_offset += align_to(ty.size_bytes() as i32, 8);
                    }
                    ArgClass::NoClass => { /* 跳过填充 */ }
                    _ => {
                        // X87/X87UP/ComplexX87：当前未实现，回退到栈
                        locs.push(ArgLocation::Stack {
                            offset: stack_offset,
                        });
                        stack_offset += 8;
                    }
                }
            }
        }

        locs
    }

    fn compute_ret_locs(&self, rets: &[Type]) -> Vec<ArgLocation> {
        let mut locs = Vec::new();
        let int_regs = [0u8, 1]; // rax, rdx
        let mut int_idx = 0;
        let mut float_idx = 0u8; // xmm0, xmm1

        for ty in rets {
            // 大返回值通过隐式 sret 指针传递
            if requires_sret(ty) {
                // Sret 指针本身在 rax 中返回
                locs.push(ArgLocation::Reg {
                    reg: PReg::new(0, RegClass::Int),
                });
                continue;
            }

            if ty.is_float() && float_idx < 2 {
                locs.push(ArgLocation::Reg {
                    reg: PReg::new(16 + float_idx, RegClass::Float),
                });
                float_idx += 1;
            } else if int_idx < int_regs.len() {
                locs.push(ArgLocation::Reg {
                    reg: PReg::new(int_regs[int_idx], RegClass::Int),
                });
                int_idx += 1;
            }
        }

        locs
    }
}

/// 对齐到指定边界。
fn align_to(value: i32, align: i32) -> i32 {
    (value + align - 1) & !(align - 1)
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sysv_arg_locs() {
        let abi = SysVAbi;

        let locs = abi.compute_arg_locs(&[Type::I32, Type::I32, Type::I64]);
        assert_eq!(locs.len(), 3);
        assert_eq!(
            locs[0],
            ArgLocation::Reg {
                reg: PReg::new(0, RegClass::Int)
            }
        );
        assert_eq!(
            locs[1],
            ArgLocation::Reg {
                reg: PReg::new(1, RegClass::Int)
            }
        );
        assert_eq!(
            locs[2],
            ArgLocation::Reg {
                reg: PReg::new(2, RegClass::Int)
            }
        );
    }

    #[test]
    fn test_sysv_float_args() {
        let abi = SysVAbi;

        let locs = abi.compute_arg_locs(&[Type::F32, Type::F64]);
        assert_eq!(locs.len(), 2);
        assert_eq!(
            locs[0],
            ArgLocation::Reg {
                reg: PReg::new(16, RegClass::Float)
            }
        );
        assert_eq!(
            locs[1],
            ArgLocation::Reg {
                reg: PReg::new(17, RegClass::Float)
            }
        );
    }

    #[test]
    fn test_sysv_many_args_spill() {
        let abi = SysVAbi;
        // 8 个整数参数，前 6 个在寄存器，后 2 个溢出到栈
        let args = vec![Type::I32; 8];
        let locs = abi.compute_arg_locs(&args);
        assert_eq!(locs.len(), 8);
        for loc in locs.iter().take(6) {
            assert!(matches!(loc, ArgLocation::Reg { .. }));
        }
        for loc in locs.iter().skip(6) {
            assert!(matches!(loc, ArgLocation::Stack { .. }));
        }
    }

    #[test]
    fn test_classify_basic_types() {
        assert_eq!(classify_type(&Type::I32), vec![ArgClass::Integer]);
        assert_eq!(classify_type(&Type::I64), vec![ArgClass::Integer]);
        assert_eq!(classify_type(&Type::F32), vec![ArgClass::Sse]);
        assert_eq!(classify_type(&Type::F64), vec![ArgClass::Sse]);
        assert_eq!(
            classify_type(&Type::V128),
            vec![ArgClass::Sse, ArgClass::SseUp]
        );
        assert_eq!(classify_type(&Type::Ptr), vec![ArgClass::Integer]);
    }

    #[test]
    fn test_requires_sret() {
        // 基本类型不需要 sret
        assert!(!requires_sret(&Type::I32));
        assert!(!requires_sret(&Type::F64));
        // 大类型（>16 字节）需要 sret
        // V256 = 32 字节，需要 sret
        assert!(requires_sret(&Type::V256));
        // V128 = 16 字节，不需要 sret
        assert!(!requires_sret(&Type::V128));
    }

    #[test]
    fn test_sret_parameter() {
        let abi = SysVAbi;
        // 一个需要 sret 返回的大类型参数 (V256 = 32 字节)
        let locs = abi.compute_arg_locs(&[Type::V256]);
        // Sret 指针应该在第一个整数寄存器（rdi）
        assert_eq!(locs.len(), 1);
        if let ArgLocation::Reg { reg } = locs[0] {
            assert_eq!(reg.num, 0); // rdi
            assert_eq!(reg.class, RegClass::Int);
        } else {
            panic!("Expected register location for sret");
        }
    }
}
