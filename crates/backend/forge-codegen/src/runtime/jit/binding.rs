//! v32：JIT 调用绑定（`bind` / `bind_source` / `bind_raw` / `JitFn`）。
//!
//! 从 `jit.rs` 拆出（file_size_ratchet：`runtime/jit.rs` 保持装载/符号/内存职责，
//! 绑定与错误面独立成文）。子模块可访问 `jit` 的私有 `entry`/`CompiledEntry`。

use super::JitLoader;
use super::signature::{JitCallSignature, JitValue};
use crate::IrError;
use crate::machine::target::TargetMachine;
use crate::pipeline::signature::{JitSigShape, JitType};

/// v32 绑定错误（使用错误归 `Semantic`，不进能力缺口台账）。
#[derive(Debug)]
pub enum JitBindError {
    Lookup(String),
    NotC {
        conv: forge_ir::CallConvId,
    },
    Variadic {
        name: String,
    },
    NotVariadic {
        name: String,
    },
    ParamCount {
        expected: usize,
        found: usize,
    },
    ParamType {
        kind: &'static str,
        index: usize,
        expected: JitType,
        found: JitType,
        hint: &'static str,
    },
    ReturnMismatch {
        kind: &'static str,
        expected: Vec<JitType>,
        found: Vec<JitType>,
    },
    UnsupportedAbi {
        reason: String,
    },
    Memory(String),
}

impl std::fmt::Display for JitBindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JitBindError::Lookup(name) => write!(f, "JIT 函数 `{name}` 未装载"),
            JitBindError::NotC { conv } => {
                write!(f, "JIT 绑定只接受平台 C 约定，产物是 {conv:?}")
            }
            JitBindError::Variadic { name } => write!(
                f,
                "JIT 函数 `{name}` 是变参：请用 bind_variadic 或 bind_raw，bind 不表达 `...`"
            ),
            JitBindError::NotVariadic { name } => {
                write!(f, "JIT 函数 `{name}` 不是变参，不能 bind_variadic")
            }
            JitBindError::ParamCount { expected, found } => {
                write!(f, "JIT 形参个数不匹配：请求 {expected}，产物 {found}")
            }
            JitBindError::ParamType {
                kind,
                index,
                expected,
                found,
                hint,
            } => write!(
                f,
                "JIT {kind}第 {index} 个形参不匹配：请求 {expected:?}，产物 {found:?}（{hint}）"
            ),
            JitBindError::ReturnMismatch {
                kind,
                expected,
                found,
            } => write!(f, "JIT {kind}返回不匹配：请求 {expected:?}，产物 {found:?}"),
            JitBindError::UnsupportedAbi { reason } => write!(f, "JIT ABI 暂不支持：{reason}"),
            JitBindError::Memory(msg) => write!(f, "JIT 装载失败：{msg}"),
        }
    }
}

impl std::error::Error for JitBindError {}

impl From<JitBindError> for IrError {
    fn from(e: JitBindError) -> Self {
        IrError::Semantic(e.to_string())
    }
}

/// 变参“长度怎么处理”的显式契约（只是绑定期元数据；callee 侧仍需实现）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariadicTermination {
    /// 调用者自行保证（最弱；`bind_variadic` 的默认）。
    Unspecified,
    /// 固定前缀第 `param` 个参数是计数；绑定期校验它是整数。
    Counted { param: usize },
    /// 末位哨兵；callee 扫描到哨兵停止。
    Sentinel,
    /// printf 风格格式串；由 callee 解析。
    Format,
}

#[allow(clippy::result_large_err)]
fn compare_shape(
    expected: &JitSigShape,
    actual: &JitSigShape,
    kind: &'static str,
) -> Result<(), JitBindError> {
    if expected.params.len() != actual.params.len() {
        return Err(JitBindError::ParamCount {
            expected: expected.params.len(),
            found: actual.params.len(),
        });
    }
    for (i, (e, a)) in expected.params.iter().zip(&actual.params).enumerate() {
        if e == a {
            continue;
        }
        if matches!(e, JitType::UnsupportedAbi(_)) || matches!(a, JitType::UnsupportedAbi(_)) {
            return Err(JitBindError::UnsupportedAbi {
                reason: format!("{kind} 第 {i} 个形参：请求 {e:?}，产物 {a:?}"),
            });
        }
        return Err(JitBindError::ParamType {
            kind,
            index: i,
            expected: e.clone(),
            found: a.clone(),
            hint: "JIT 产物按 ABI 收参；聚合/128 位需 M2 的 ABI planner",
        });
    }
    if expected.returns != actual.returns {
        if expected
            .returns
            .iter()
            .chain(actual.returns.iter())
            .any(|t| matches!(t, JitType::UnsupportedAbi(_)))
        {
            return Err(JitBindError::UnsupportedAbi {
                reason: format!(
                    "{kind} 返回：请求 {:?}，产物 {:?}",
                    expected.returns, actual.returns
                ),
            });
        }
        return Err(JitBindError::ReturnMismatch {
            kind,
            expected: expected.returns.clone(),
            found: actual.returns.clone(),
        });
    }
    Ok(())
}

/// v32 类型化函数句柄：绑定已校验，生命周期跟随 loader。
pub struct JitFn<'a, F> {
    ptr: F,
    abi: Option<JitSigShape>,
    source: Option<JitSigShape>,
    host: bool,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<F: Copy> JitFn<'_, F> {
    /// 取回原始函数指针（`bind` 已校验签名）。
    pub fn raw(&self) -> F {
        self.ptr
    }
}

impl<F> JitFn<'_, F> {
    /// 绑定时的 ABI 签名（`bind_raw` 可能没有）。
    pub fn abi(&self) -> Option<&JitSigShape> {
        self.abi.as_ref()
    }

    /// 绑定时的源码签名。
    pub fn source(&self) -> Option<&JitSigShape> {
        self.source.as_ref()
    }

    /// 产物目标是否与宿主一致（只影响 `call`；`raw()` 不受限）。
    pub fn is_host_compatible(&self) -> bool {
        self.host
    }
}

/// 带长度契约的变参句柄（`bind_variadic_with_termination`）。
pub struct JitVariadicFn<'a, F> {
    inner: JitFn<'a, F>,
    termination: VariadicTermination,
}

impl<F: Copy> JitVariadicFn<'_, F> {
    pub fn raw(&self) -> F {
        self.inner.raw()
    }
}

impl<F> JitVariadicFn<'_, F> {
    pub fn termination(&self) -> VariadicTermination {
        self.termination
    }

    pub fn is_host_compatible(&self) -> bool {
        self.inner.is_host_compatible()
    }

    pub fn abi(&self) -> Option<&JitSigShape> {
        self.inner.abi()
    }
}

macro_rules! jit_fn_call {
    ($($p:ident),*) => {
        impl<$($p,)* R> JitFn<'_, extern "C" fn($($p),*) -> R> {
            #[allow(non_snake_case, clippy::too_many_arguments)]
            #[inline]
            pub fn call(&self, $($p: $p),*) -> R {
                assert!(
                    self.host,
                    "跨目标 JIT 调用不能安全 call；请用 raw() 并自担 ABI 风险"
                );
                (self.ptr)($($p),*)
            }
        }
    };
}

jit_fn_call!();
jit_fn_call!(A);
jit_fn_call!(A, B);
jit_fn_call!(A, B, C);
jit_fn_call!(A, B, C, D);
jit_fn_call!(A, B, C, D, E);
jit_fn_call!(A, B, C, D, E, F);
jit_fn_call!(A, B, C, D, E, F, G);
jit_fn_call!(A, B, C, D, E, F, G, H);
jit_fn_call!(A, B, C, D, E, F, G, H, I);
jit_fn_call!(A, B, C, D, E, F, G, H, I, J);
jit_fn_call!(A, B, C, D, E, F, G, H, I, J, K);
jit_fn_call!(A, B, C, D, E, F, G, H, I, J, K, L);
jit_fn_call!(A, B, C, D, E, F, G, H, I, J, K, L, M);
jit_fn_call!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
jit_fn_call!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
jit_fn_call!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);

/// 安全版“变参”替代：**指针 + 长度** ABI（不是 C `...`）。
///
/// JIT 函数签名为 `(ptr: *const T, len: usize) -> R`；Rust 侧传 `&[T]`，
/// 长度由 `len` 明示，不依赖 ABI 自动计数，也没有外部 C 变参保存区问题。
pub struct JitSliceFn<'a, T, R> {
    inner: JitFn<'a, extern "C" fn(*const T, usize) -> R>,
}

impl<T, R> JitSliceFn<'_, T, R> {
    /// 调 JIT：`f(slice.as_ptr(), slice.len())`。
    pub fn call(&self, args: &[T]) -> R {
        self.inner.call(args.as_ptr(), args.len())
    }

    /// 产物目标是否与宿主一致（`call` 会检查）。
    pub fn is_host_compatible(&self) -> bool {
        self.inner.is_host_compatible()
    }

    /// 绑定时的 ABI 签名。
    pub fn abi(&self) -> Option<&JitSigShape> {
        self.inner.abi()
    }
}

/// 安全版“变参”替代（可变切片）：JIT 函数签名为 `(ptr: *mut T, len: usize) -> R`。
pub struct JitSliceMutFn<'a, T, R> {
    inner: JitFn<'a, extern "C" fn(*mut T, usize) -> R>,
}

impl<T, R> JitSliceMutFn<'_, T, R> {
    /// 调 JIT：`f(slice.as_mut_ptr(), slice.len())`。
    pub fn call(&self, args: &mut [T]) -> R {
        self.inner.call(args.as_mut_ptr(), args.len())
    }

    pub fn is_host_compatible(&self) -> bool {
        self.inner.is_host_compatible()
    }

    pub fn abi(&self) -> Option<&JitSigShape> {
        self.inner.abi()
    }
}

impl<M: TargetMachine> JitLoader<M> {
    /// v32：安全版变参——绑定 `(ptr, len)` ABI 的 JIT 函数，调用传 `&[T]`。
    ///
    /// 这是“用户决定长度怎么处理”的安全形态：长度显式、运行时可变，不需要 C `...`。
    #[allow(clippy::result_large_err)]
    pub fn bind_slice<T: 'static, R: JitValue>(
        &self,
        name: &str,
    ) -> Result<JitSliceFn<'_, T, R>, JitBindError> {
        let inner = self.bind::<extern "C" fn(*const T, usize) -> R>(name)?;
        Ok(JitSliceFn { inner })
    }

    /// v32：安全版变参（可变切片）——绑定 `(ptr: *mut T, len)` ABI。
    #[allow(clippy::result_large_err)]
    pub fn bind_slice_mut<T: 'static, R: JitValue>(
        &self,
        name: &str,
    ) -> Result<JitSliceMutFn<'_, T, R>, JitBindError> {
        let inner = self.bind::<extern "C" fn(*mut T, usize) -> R>(name)?;
        Ok(JitSliceMutFn { inner })
    }

    /// v32：按**产物 ABI 签名**绑定类型化函数句柄。
    ///
    /// 不比较宿主与目标（绑定只做签名校验）；跨目标时 `call` 会 panic，`raw()` 自担。
    #[allow(clippy::result_large_err)]
    pub fn bind<F: JitCallSignature>(&self, name: &str) -> Result<JitFn<'_, F>, JitBindError> {
        let entry = self
            .entry(name)
            .map_err(|e| JitBindError::Lookup(e.to_string()))?;
        if !entry.jit_sig.conv.is_c() {
            return Err(JitBindError::NotC {
                conv: entry.jit_sig.conv.clone(),
            });
        }
        if entry.jit_sig.variadic {
            return Err(JitBindError::Variadic {
                name: name.to_string(),
            });
        }
        let expected = F::abi(&entry.jit_sig.target);
        let abi = entry
            .jit_sig
            .abi
            .as_ref()
            .ok_or_else(|| JitBindError::UnsupportedAbi {
                reason: "产物没有可用的 ABI 签名（含 M2 未展开形态）".to_string(),
            })?;
        compare_shape(&expected, abi, "ABI")?;
        let host = entry.jit_sig.target.is_host_compatible();
        let ptr = unsafe { entry.mem.get_fn::<F>(0) }
            .map_err(|e| JitBindError::Memory(format!("{e:?}")))?;
        Ok(JitFn {
            ptr,
            abi: Some(abi.clone()),
            source: Some(entry.jit_sig.source.clone()),
            host,
            _marker: std::marker::PhantomData,
        })
    }

    /// v32：只校验**源码签名**（IR 语义契约），不返回可调用指针。
    #[allow(clippy::result_large_err)]
    pub fn bind_source<F: JitCallSignature>(
        &self,
        name: &str,
    ) -> Result<JitSigShape, JitBindError> {
        let entry = self
            .entry(name)
            .map_err(|e| JitBindError::Lookup(e.to_string()))?;
        if !entry.jit_sig.conv.is_c() {
            return Err(JitBindError::NotC {
                conv: entry.jit_sig.conv.clone(),
            });
        }
        if entry.jit_sig.variadic {
            return Err(JitBindError::Variadic {
                name: name.to_string(),
            });
        }
        let expected = F::source(&entry.jit_sig.target);
        compare_shape(&expected, &entry.jit_sig.source, "源码")?;
        Ok(expected)
    }

    /// v32：绑定 C 变参函数——只校验固定前缀与 `variadic`；调用用 `raw()`。
    ///
    /// Rust 1.99 的 `...` 类型让“固定前缀”可以类型化；`...` 部分由调用者与
    /// Rust 变参类型保证（语言边界，泛型 `call` 无法表达）。
    ///
    /// # Safety
    ///
    /// **当前后端只支持 JIT 生成 caller 的变参保存区契约**：callee 的 `va_start`
    /// 只初始化 `va_list` 对象，不保存外部调用者的参数寄存器。因此 **host 直接
    /// 调用一个会 `va_arg` 的变参 JIT 函数会读到垃圾**（模块路径 / JIT caller 正常）。
    /// 调用者必须保证目标函数不会从外部 C 变参调用中读取未命名实参，或使用
    /// `bind_raw` 并自行承担 ABI 责任。该缺口修复前不提供安全版 `bind_variadic`。
    #[allow(clippy::result_large_err)]
    pub unsafe fn bind_variadic<F: JitCallSignature>(
        &self,
        name: &str,
    ) -> Result<JitFn<'_, F>, JitBindError> {
        let entry = self
            .entry(name)
            .map_err(|e| JitBindError::Lookup(e.to_string()))?;
        if !entry.jit_sig.conv.is_c() {
            return Err(JitBindError::NotC {
                conv: entry.jit_sig.conv.clone(),
            });
        }
        if !entry.jit_sig.variadic {
            return Err(JitBindError::NotVariadic {
                name: name.to_string(),
            });
        }
        let expected = F::abi(&entry.jit_sig.target);
        if !expected.variadic {
            return Err(JitBindError::NotVariadic {
                name: format!("{name}（请求的 Rust 类型不是变参）"),
            });
        }
        let abi = entry
            .jit_sig
            .abi
            .as_ref()
            .ok_or_else(|| JitBindError::UnsupportedAbi {
                reason: "产物没有可用的 ABI 签名（含 M2 未展开形态）".to_string(),
            })?;
        compare_shape(&expected, abi, "ABI(变参固定前缀)")?;
        let host = entry.jit_sig.target.is_host_compatible();
        let ptr = unsafe { entry.mem.get_fn::<F>(0) }
            .map_err(|e| JitBindError::Memory(format!("{e:?}")))?;
        Ok(JitFn {
            ptr,
            abi: Some(abi.clone()),
            source: Some(entry.jit_sig.source.clone()),
            host,
            _marker: std::marker::PhantomData,
        })
    }

    /// v32：带显式长度契约的变参绑定（仍 unsafe，原因同 [`Self::bind_variadic`]）。
    ///
    /// `Counted { param }` 会校验固定前缀第 `param` 个形参是整数；`Sentinel`/`Format`
    /// 只记录契约，由 callee 实现。长度怎么处理由调用者选择，而不是 ABI 自动猜。
    #[allow(clippy::result_large_err)]
    pub unsafe fn bind_variadic_with_termination<F: JitCallSignature>(
        &self,
        name: &str,
        termination: VariadicTermination,
    ) -> Result<JitVariadicFn<'_, F>, JitBindError> {
        let inner = unsafe { self.bind_variadic::<F>(name)? };
        if let VariadicTermination::Counted { param } = termination {
            let Some(abi) = inner.abi() else {
                return Err(JitBindError::UnsupportedAbi {
                    reason: "产物没有可用的 ABI 签名，无法校验 Counted 计数参数".to_string(),
                });
            };
            let Some(ty) = abi.params.get(param) else {
                return Err(JitBindError::ParamCount {
                    expected: param + 1,
                    found: abi.params.len(),
                });
            };
            if !matches!(ty, JitType::Int { .. } | JitType::Bool) {
                return Err(JitBindError::ParamType {
                    kind: "变参计数",
                    index: param,
                    expected: JitType::Int { bits: 64 },
                    found: ty.clone(),
                    hint: "Counted 的 param 必须是整数形参",
                });
            }
        }
        Ok(JitVariadicFn { inner, termination })
    }

    /// v32 逃生口：不做签名校验；`call` 的宿主检查仍然生效。
    ///
    /// # Safety
    /// 调用者必须保证 `F` 与产物 ABI 完全一致。
    #[allow(clippy::result_large_err)]
    pub unsafe fn bind_raw<F>(&self, name: &str) -> Result<JitFn<'_, F>, JitBindError> {
        let entry = self
            .entry(name)
            .map_err(|e| JitBindError::Lookup(e.to_string()))?;
        let host = entry.jit_sig.target.is_host_compatible();
        let ptr = unsafe { entry.mem.get_fn::<F>(0) }
            .map_err(|e| JitBindError::Memory(format!("{e:?}")))?;
        Ok(JitFn {
            ptr,
            abi: entry.jit_sig.abi.clone(),
            source: Some(entry.jit_sig.source.clone()),
            host,
            _marker: std::marker::PhantomData,
        })
    }
}
