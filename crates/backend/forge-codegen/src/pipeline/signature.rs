//! 编译产物携带的签名（v30 源码签名 + v32 目标感知描述符）。
//!
//! - [`JitSignatureInfo`]：v30 的源码签名（`TypeId` 列表），供旧的 `get` 使用；
//! - [`JitSig`]：v32 的**自包含**签名面——`source`（IR 语义）+ `abi`（合法化后的
//!   真实接口）+ `target`（产物自己的指针位宽/端序/ISA）。`JitLoader::bind` 只读它，
//!   所以 `usize`/指针/`bool`/函数指针都不再依赖宿主 `cfg`。

use forge_ir::ir::types::TypeEntry;
use forge_ir::{CallConvId, Endianness, Function, FunctionSignature, TypeId, TypeStore};

/// 一个产物函数的源码签名（v30 旧接口；JIT 的 `get` 用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JitSignatureInfo {
    /// 形参类型（顺序即 ABI 位置序）。
    pub params: Vec<TypeId>,
    /// 返回类型（0 个 = void，1 个 = 单值）。
    pub returns: Vec<TypeId>,
    /// 调用约定（IR 侧标识；`c` 表示平台 C 约定）。
    pub conv: CallConvId,
    /// 是否变参。
    pub variadic: bool,
}

impl JitSignatureInfo {
    pub fn from_function(func: &Function, variadic: bool) -> Self {
        Self {
            params: func.param_types(),
            returns: func.return_types(),
            conv: func.calling_convention.clone(),
            variadic,
        }
    }

    pub fn from_signature(sig: &FunctionSignature) -> Self {
        Self {
            params: sig.params.iter().map(|(ty, _)| *ty).collect(),
            returns: sig.returns.clone(),
            conv: sig.calling_convention.clone(),
            variadic: sig.variadic,
        }
    }
}

/// 产物自己的目标事实（来自 `TypeStore.data_layout` + `TargetMachine`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JitTarget {
    /// ISA 名（`x86_64`/`aarch64`/`riscv64` 等）。
    pub arch: String,
    /// 指针位宽（32/64）。
    pub pointer_bits: u16,
    /// 原生整数位宽（严格升序）。
    pub int_widths: Vec<u16>,
    /// 端序。
    pub endian: Endianness,
}

impl JitTarget {
    /// 当前产物是否与宿主执行环境一致（只影响 `call` 的安全性，不影响 `bind`）。
    pub fn is_host_compatible(&self) -> bool {
        self.pointer_bits == usize::BITS as u16 && arch_matches_host(&self.arch)
    }
}

fn arch_matches_host(arch: &str) -> bool {
    let host = std::env::consts::ARCH;
    arch == host
        || (host == "x86_64" && (arch == "x86" || arch == "x86_64"))
        || (host == "aarch64" && (arch == "arm64" || arch == "aarch64"))
}

/// 自包含类型描述（不携带 `TypeId`，可跨类型库比较）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JitType {
    Void,
    Bool,
    Int {
        bits: u16,
    },
    Float {
        bits: u16,
    },
    Ptr {
        addr_space: u32,
    },
    Function {
        params: Vec<JitType>,
        rets: Vec<JitType>,
        variadic: bool,
    },
    Struct {
        size: u32,
        align: u16,
        packed: bool,
        fields: Vec<JitType>,
    },
    /// 枚举：C 无字段枚举（`#[repr(u8)]` 等）的 ABI 就是 repr 整数；
    /// 带字段的 Rust 枚举需要 `jit_rust_abi` 的反射桥（M3）。
    Enum {
        repr: Box<JitType>,
        variants: Vec<JitEnumVariant>,
    },
    /// 共用体：C ABI 的按值分类依赖成员（SysV 的 INTEGER/SSE 合并），
    /// 由 `plan_jit_abi`（M2）处理；M1 只允许按引用使用。
    Union {
        size: u32,
        align: u16,
        fields: Vec<JitType>,
    },
    Array {
        elem: Box<JitType>,
        len: u64,
    },
    Vector {
        elem: Box<JitType>,
        lanes: u32,
    },
    /// 当前阶段无法表达的形态（128 位/聚合 ABI 展开等），携带原因。
    UnsupportedAbi(&'static str),
}

/// 枚举变体描述（反射桥生成；C 无字段枚举只需 discriminant）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JitEnumVariant {
    pub name: String,
    pub discriminant: i128,
    pub fields: Vec<JitType>,
}

impl JitType {
    /// ABI 槽投影：`bool` → 8 位整数，函数指针 → 指针，其余标量/指针不变。
    /// 聚合/向量/128 位在 M2 由 ABI planner 展开；M1 明确报 Unsupported。
    pub fn to_abi(self) -> JitType {
        match self {
            JitType::Bool => JitType::Int { bits: 8 },
            JitType::Function { .. } => JitType::Ptr { addr_space: 0 },
            JitType::Enum { repr, variants } => {
                if variants.iter().all(|v| v.fields.is_empty()) {
                    repr.to_abi()
                } else {
                    JitType::UnsupportedAbi("带字段枚举需 jit_rust_abi 反射桥（M3）")
                }
            }
            JitType::Union { .. } => {
                JitType::UnsupportedAbi("union 按值需 plan_jit_abi（M2）；按引用用 ByRef")
            }
            JitType::Struct { .. } | JitType::Array { .. } | JitType::Vector { .. } => {
                JitType::UnsupportedAbi("聚合/向量的 ABI 展开需 plan_jit_abi（M2）")
            }
            JitType::Int { bits } if bits > 64 => {
                JitType::UnsupportedAbi("128 位入口 ABI 需 M1b（wide_split 拆分/重组）")
            }
            other => other,
        }
    }

    /// 从产物 `TypeStore` 构造（递归；深度上限防递归类型）。
    pub fn from_ir(store: &TypeStore, ty: TypeId) -> JitType {
        Self::from_ir_depth(store, ty, 0)
    }

    fn from_ir_depth(store: &TypeStore, ty: TypeId, depth: u32) -> JitType {
        if depth > 64 {
            return JitType::UnsupportedAbi("类型嵌套过深");
        }
        if ty == TypeId::BOOL {
            return JitType::Bool;
        }
        if ty == TypeId::VOID {
            return JitType::Void;
        }
        match store.get(ty) {
            TypeEntry::Int { bits } => {
                if *bits == 1 {
                    JitType::Bool
                } else {
                    JitType::Int {
                        bits: u16::try_from(*bits).unwrap_or(u16::MAX),
                    }
                }
            }
            TypeEntry::Float { .. } => JitType::Float {
                bits: store.scalar_bits(ty).unwrap_or(64) as u16,
            },
            TypeEntry::Pointer { addr_space } => JitType::Ptr {
                addr_space: *addr_space,
            },
            TypeEntry::Function {
                params,
                rets,
                is_vararg,
            } => JitType::Function {
                params: params
                    .iter()
                    .map(|p| Self::from_ir_depth(store, *p, depth + 1))
                    .collect(),
                rets: rets
                    .iter()
                    .map(|r| Self::from_ir_depth(store, *r, depth + 1))
                    .collect(),
                variadic: *is_vararg,
            },
            TypeEntry::Struct {
                fields, is_packed, ..
            } => JitType::Struct {
                size: store.size_bytes(ty),
                align: u16::try_from(store.alignment(ty)).unwrap_or(u16::MAX),
                packed: *is_packed,
                fields: fields
                    .iter()
                    .map(|f| Self::from_ir_depth(store, f.ty, depth + 1))
                    .collect(),
            },
            TypeEntry::Array { elem, len } => JitType::Array {
                elem: Box::new(Self::from_ir_depth(store, *elem, depth + 1)),
                len: *len,
            },
            TypeEntry::Vector { elem, len } => JitType::Vector {
                elem: Box::new(Self::from_ir_depth(store, *elem, depth + 1)),
                lanes: *len,
            },
            TypeEntry::ScalableVector { .. } => JitType::UnsupportedAbi("可扩展向量"),
            TypeEntry::Token | TypeEntry::Metadata | TypeEntry::Opaque => JitType::Void,
        }
    }
}

/// 一段签名面：形参、返回、变参标志。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JitSigShape {
    pub params: Vec<JitType>,
    pub returns: Vec<JitType>,
    pub variadic: bool,
}

impl JitSigShape {
    pub fn from_function(func: &Function, variadic: bool, abi: bool) -> Option<Self> {
        let store = func.types.borrow();
        Self::from_store(&store, func, variadic, abi)
    }

    /// 用**已取好的**类型快照构造（编译入口只取一次，读路径预算 1 次）。
    pub fn from_store(
        store: &TypeStore,
        func: &Function,
        variadic: bool,
        abi: bool,
    ) -> Option<Self> {
        let map = |ty: TypeId| {
            let t = JitType::from_ir(store, ty);
            if abi { t.to_abi() } else { t }
        };
        let params: Vec<JitType> = func.param_types().into_iter().map(map).collect();
        let returns: Vec<JitType> = func.return_types().into_iter().map(map).collect();
        if abi
            && params
                .iter()
                .chain(returns.iter())
                .any(|t| matches!(t, JitType::UnsupportedAbi(_)))
        {
            return None;
        }
        Some(Self {
            params,
            returns,
            variadic,
        })
    }
}

/// v32 的完整签名：源码 + ABI + 目标事实。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JitSig {
    pub source: JitSigShape,
    /// 合法化后 ABI 签名；含当前阶段无法表达的形态时为 `None`。
    pub abi: Option<JitSigShape>,
    pub conv: CallConvId,
    pub variadic: bool,
    pub target: JitTarget,
}

impl JitSig {
    /// 从原始函数（源码）与合法化后函数（ABI）构造。
    ///
    /// **只取一次类型快照**：源/ABI 共用同一个 `TypeContext`（合法化是克隆后改类型），
    /// 因此 source/ABI/target 都在这一份 `&TypeStore` 上解析（读路径预算 1 次）。
    pub fn from_functions<M: crate::machine::target::TargetMachine>(
        source: &Function,
        abi: &Function,
        machine: &M,
    ) -> Self {
        let store = abi.types.borrow();
        let variadic = store.get_signature(source.signature).variadic;
        let target = JitTarget {
            arch: machine.isa_info().name().to_string(),
            pointer_bits: (store.data_layout.pointer_size(0) * 8) as u16,
            int_widths: machine.int_widths().to_vec(),
            endian: store.data_layout.endianness,
        };
        let source_shape =
            JitSigShape::from_store(&store, source, variadic, false).unwrap_or_else(|| {
                JitSigShape {
                    params: Vec::new(),
                    returns: Vec::new(),
                    variadic,
                }
            });
        let abi_shape = JitSigShape::from_store(&store, abi, variadic, true);
        Self {
            source: source_shape,
            abi: abi_shape,
            conv: source.calling_convention.clone(),
            variadic,
            target,
        }
    }
}
