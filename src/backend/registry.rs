//! 后端注册表。
//!
//! 提供全局后端注册和查找功能。用户通过 [`register_backend!`] 宏注册自定义 ISA，
//! 通过 [`Registry::global().lookup()`] 查找编译器。

use super::*;
use crate::{CompileError, CompiledFunction};
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// 类型擦除的编译器接口。
pub trait Compiler: Send + Sync {
    /// 编译器名称（对应 ISA 名称）。
    fn name(&self) -> &str;
    /// 编译一个 IR 函数。
    fn compile(&self, func: &Function) -> Result<CompiledFunction, CompileError>;
    /// 向下转型。
    fn as_any(&self) -> &dyn Any;
}

/// 泛型编译器包装。
///
/// 将实现了 [`InstructionSet`] 的类型包装为类型擦除的 [`Compiler`]。
pub struct IsaCompiler<I: InstructionSet> {
    _phantom: std::marker::PhantomData<I>,
}

impl<I: InstructionSet> IsaCompiler<I> {
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<I: InstructionSet> Default for IsaCompiler<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: InstructionSet> Compiler for IsaCompiler<I> {
    fn name(&self) -> &str {
        I::name()
    }

    fn compile(&self, func: &Function) -> Result<CompiledFunction, CompileError> {
        FunctionCompiler::<I>::compile(func)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 全局后端注册表。
pub struct Registry {
    backends: RwLock<HashMap<String, Arc<dyn Compiler>>>,
}

impl Registry {
    /// 获取全局注册表单例。
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<Registry> = OnceLock::new();
        INSTANCE.get_or_init(|| Registry {
            backends: RwLock::new(HashMap::new()),
        })
    }

    /// 注册一个后端编译器。
    pub fn register(&self, backend: Arc<dyn Compiler>) {
        let mut backends = self.backends.write().unwrap();
        backends.insert(backend.name().to_string(), backend);
    }

    /// 按名称查找编译器。
    pub fn lookup(&self, name: &str) -> Option<Arc<dyn Compiler>> {
        let backends = self.backends.read().unwrap();
        backends.get(name).cloned()
    }

    /// 列出所有已注册的后端名称。
    pub fn list(&self) -> Vec<String> {
        let backends = self.backends.read().unwrap();
        backends.keys().cloned().collect()
    }

    /// 后端是否已注册。
    pub fn contains(&self, name: &str) -> bool {
        let backends = self.backends.read().unwrap();
        backends.contains_key(name)
    }
}

/// 注册一个 ISA 后端。
///
/// # Example
/// ```ignore
/// use codegen_lib::prelude::*;
///
/// struct MyIsa;
/// impl InstructionSet for MyIsa {
///     type Inst = MyInst;
///     fn name() -> &'static str { "my-arch" }
///     // ...
/// }
///
/// register_backend!(MyIsa);
/// ```
#[macro_export]
macro_rules! register_backend {
    ($isa:ty) => {
        $crate::Registry::global()
            .register(std::sync::Arc::new($crate::IsaCompiler::<$isa>::new()));
    };
}

/// 便捷函数：使用已注册的 ISA 编译一个函数。
///
/// 自动运行默认优化管道（常量折叠、死代码消除、复制传播）。
///
/// ```ignore
/// let code = compile::<MyIsa>(
///     "my_func",
///     &Signature::new(&[(Type::I32, "x")], &[Type::I32]),
///     |b| {
///         let entry = b.create_block_with_params(&[(Type::I32, "x")]);
///         b.switch_to_block(entry.0);
///         b.return_(&[entry.1[0]]);
///     },
/// )?;
/// ```
pub fn compile<I: InstructionSet>(
    name: &str,
    signature: &Signature,
    build_fn: impl FnOnce(&mut FunctionBuilder),
) -> Result<CompiledFunction, CompileError> {
    let mut builder = FunctionBuilder::new(name, signature.clone());
    build_fn(&mut builder);
    let mut func = builder.finish();

    // 运行默认优化管道
    let pm = crate::optimize::PassManager::default();
    if !pm.is_empty() {
        let opt_result = pm.run_on_function(&mut func)?;
        log::info!(
            "Optimized '{}': {} insts removed, {} blocks removed",
            func.name,
            opt_result.instructions_removed,
            opt_result.blocks_removed
        );
    }

    FunctionCompiler::<I>::compile(&func)
}
