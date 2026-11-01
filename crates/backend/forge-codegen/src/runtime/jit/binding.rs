//! v32：JIT 调用绑定（`bind` / `bind_source` / `bind_raw` / `JitFn`）。
//!
//! 从 `jit.rs` 拆出（file_size_ratchet：`runtime/jit.rs` 保持装载/符号/内存职责，
//! 绑定与错误面独立成文）。子模块可访问 `jit` 的私有 `entry`/`CompiledEntry`。

use super::JitLoader;
use super::signature::JitCallSignature;
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

impl<M: TargetMachine> JitLoader<M> {
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

    /// v32：绑定 C 变参函数——校验固定前缀与 `variadic`，调用用 `raw()`。
    ///
    /// Rust 1.99 的 `...` 类型让“固定前缀”可以类型化；`...` 部分由调用者与
    /// Rust 变参类型保证（语言边界，泛型 `call` 无法表达）。
    #[allow(clippy::result_large_err)]
    pub fn bind_variadic<F: JitCallSignature>(
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
