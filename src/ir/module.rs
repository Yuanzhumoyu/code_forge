//! IR 模块 — 多函数容器。
//!
//! `Module` 管理一组 IR 函数，提供：
//! - 按名称/索引查找函数
//! - 跨函数的常量函数表（用于 const fn 编译时求值）
//! - 模块级优化管道

use super::context::Context;
use super::function::Function;
use super::types::{FuncRef, Type};
use std::collections::HashMap;

/// 全局变量的链接类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Linkage {
    /// 外部可见（默认）。
    #[default]
    External,
    /// 仅当前模块可见。
    Internal,
    /// 可被合并（允许多个同名的 Weak 定义）。
    Weak,
    /// 外部引用（该变量在另一个模块中定义）。
    ExternalWeak,
    /// 不会在目标文件中生成（编译时常量）。
    Private,
}

/// 全局变量（或常量）。
///
/// 在 IR 中通过 `GlobalAddr { global: u32 }` 指令引用，
/// 其中 `global` 是 `Module::globals` 中的索引。
#[derive(Clone, Debug)]
pub struct GlobalVariable {
    /// 变量名（也是链接器符号名）。
    pub name: String,
    /// 变量类型。
    pub ty: Type,
    /// 初始化值（可选 — None 表示未初始化外部变量）。
    pub init: Option<Vec<u8>>,
    /// 链接类型。
    pub linkage: Linkage,
    /// 是否为常量（不可变）。
    pub is_constant: bool,
    /// 对齐要求（字节，0 = 默认）。
    pub alignment: u32,
}

impl GlobalVariable {
    /// 创建一个外部可见的全局常量。
    pub fn constant(name: &str, ty: Type) -> Self {
        Self {
            name: name.to_string(),
            ty,
            init: None,
            linkage: Linkage::External,
            is_constant: true,
            alignment: 0,
        }
    }

    /// 创建一个外部可见的全局变量（可变）。
    pub fn mutable(name: &str, ty: Type) -> Self {
        Self {
            name: name.to_string(),
            ty,
            init: None,
            linkage: Linkage::External,
            is_constant: false,
            alignment: 0,
        }
    }

    /// 设置初始化数据。
    pub fn with_init(mut self, data: Vec<u8>) -> Self {
        self.init = Some(data);
        self
    }

    /// 设置链接类型。
    pub fn with_linkage(mut self, linkage: Linkage) -> Self {
        self.linkage = linkage;
        self
    }

    /// 设置对齐。
    pub fn with_alignment(mut self, align: u32) -> Self {
        self.alignment = align;
        self
    }
}

/// IR 模块 — 一组相关函数的容器。
///
/// 每个添加的函数获得一个 `FuncRef`，可通过名称或索引查找。
/// 标记为 `is_const` 的函数自动注册到常量函数表，
/// 供 `ConstFnEvalPass` 在编译时求值使用。
///
/// # Example
///
/// ```ignore
/// let mut module = Module::new();
///
/// // 添加一个 const 函数
/// let mut builder = FunctionBuilder::new("add", sig.clone());
/// builder.set_const(true);
/// // ... build ...
/// let func_ref = module.add_function(builder.finish());
///
/// // 添加调用者
/// let mut builder2 = FunctionBuilder::new("main", sig2);
/// // ... build with calls to func_ref ...
/// let main_ref = module.add_function(builder2.finish());
///
/// // 获取 const fn 优化管道
/// let pm = module.optimization_pipeline();
/// ```
#[derive(Clone, Debug, Default)]
pub struct Module {
    /// 模块中的所有函数（按 FuncRef 索引）。
    functions: Vec<Function>,
    /// 函数名 → FuncRef 的映射。
    name_map: HashMap<String, FuncRef>,
    /// next FuncRef to assign.
    next_ref: u32,
    /// 模块级别的 IR 上下文 — 共享类型表、常量池和元数据。
    context: Context,
    /// 全局变量表。
    globals: Vec<GlobalVariable>,
    /// 全局变量名 → 索引的映射。
    global_names: HashMap<String, u32>,
}

impl Module {
    /// 创建一个空模块。
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            name_map: HashMap::new(),
            next_ref: 0,
            context: Context::new(),
            globals: Vec::new(),
            global_names: HashMap::new(),
        }
    }

    /// 返回模块的上下文引用。
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// 返回模块的上下文可变引用。
    pub fn context_mut(&mut self) -> &mut Context {
        &mut self.context
    }

    /// 用指定的上下文创建一个新模块。
    pub fn with_context(context: Context) -> Self {
        Self {
            functions: Vec::new(),
            name_map: HashMap::new(),
            next_ref: 0,
            context,
            globals: Vec::new(),
            global_names: HashMap::new(),
        }
    }

    // ============================================================
    // 全局变量管理
    // ============================================================

    /// 添加一个全局变量，返回其索引。
    /// 如果同名变量已存在，返回错误。
    pub fn add_global(&mut self, global: GlobalVariable) -> Result<u32, String> {
        if self.global_names.contains_key(&global.name) {
            return Err(format!("global '{}' already exists", global.name));
        }
        let idx = self.globals.len() as u32;
        self.global_names.insert(global.name.clone(), idx);
        self.globals.push(global);
        Ok(idx)
    }

    /// 按索引获取全局变量。
    pub fn get_global(&self, idx: u32) -> Option<&GlobalVariable> {
        self.globals.get(idx as usize)
    }

    /// 按索引获取全局变量的可变引用。
    pub fn get_global_mut(&mut self, idx: u32) -> Option<&mut GlobalVariable> {
        self.globals.get_mut(idx as usize)
    }

    /// 按名称查找全局变量索引。
    pub fn find_global(&self, name: &str) -> Option<u32> {
        self.global_names.get(name).copied()
    }

    /// 全局变量数量。
    pub fn global_count(&self) -> usize {
        self.globals.len()
    }

    /// 迭代所有全局变量。
    pub fn iter_globals(&self) -> impl Iterator<Item = &GlobalVariable> {
        self.globals.iter()
    }

    /// 添加一个函数到模块中，返回其 `FuncRef`。
    ///
    /// 函数名必须是唯一的。如果同名函数已存在，返回错误。
    pub fn add_function(&mut self, func: Function) -> Result<FuncRef, String> {
        if self.name_map.contains_key(&func.name) {
            return Err(format!("function '{}' already exists in module", func.name));
        }

        let func_ref = FuncRef(self.next_ref);
        self.next_ref += 1;
        self.name_map.insert(func.name.clone(), func_ref);
        self.functions.push(func);
        Ok(func_ref)
    }

    /// 按 FuncRef 获取函数。
    pub fn get(&self, func_ref: FuncRef) -> Option<&Function> {
        self.functions.get(func_ref.0 as usize)
    }

    /// 按 FuncRef 获取函数的可变引用。
    pub fn get_mut(&mut self, func_ref: FuncRef) -> Option<&mut Function> {
        self.functions.get_mut(func_ref.0 as usize)
    }

    /// 按名称查找函数。
    pub fn find_by_name(&self, name: &str) -> Option<FuncRef> {
        self.name_map.get(name).copied()
    }

    /// 按 FuncRef 查找函数名。
    pub fn name_of(&self, func_ref: FuncRef) -> Option<&str> {
        self.functions
            .get(func_ref.0 as usize)
            .map(|f| f.name.as_str())
    }

    /// 模块中的函数数量。
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// 模块是否为空。
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// 迭代所有函数（不可变）。
    pub fn iter(&self) -> impl Iterator<Item = &Function> {
        self.functions.iter()
    }

    /// 迭代所有函数（可变）。
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Function> {
        self.functions.iter_mut()
    }

    /// 获取所有函数的 FuncRef 列表。
    pub fn func_refs(&self) -> Vec<FuncRef> {
        (0..self.functions.len())
            .map(|i| FuncRef(i as u32))
            .collect()
    }

    // ============================================================
    // 常量函数表
    // ============================================================

    /// 构建常量函数表：`FuncRef → Function`。
    ///
    /// 仅包含标记为 `is_const == true` 的函数。
    /// 此表可直接传递给 `ConstFnEvalPass::new()`。
    pub fn const_function_table(&self) -> HashMap<FuncRef, Function> {
        self.functions
            .iter()
            .enumerate()
            .filter(|(_, f)| f.is_const)
            .map(|(i, f)| (FuncRef(i as u32), f.clone()))
            .collect()
    }

    /// 获取所有 const 函数的 FuncRef 列表。
    pub fn const_func_refs(&self) -> Vec<FuncRef> {
        self.functions
            .iter()
            .enumerate()
            .filter(|(_, f)| f.is_const)
            .map(|(i, _)| FuncRef(i as u32))
            .collect()
    }

    /// 检查指定函数是否为 const 函数。
    pub fn is_const_fn(&self, func_ref: FuncRef) -> bool {
        self.get(func_ref).map(|f| f.is_const).unwrap_or(false)
    }

    // ============================================================
    // 模块级优化管道
    // ============================================================

    /// 构建包含 const fn 求值和内联的优化管道。
    ///
    /// 管道顺序：
    /// 1. InlinePass（内联小函数）
    /// 2. ConstFnEvalPass（编译时求值 const 调用）
    /// 3. ConstFoldPass（常量折叠） → 不动点
    /// 4. CsePass（公共子表达式消除）
    /// 5. LicmPass（循环不变量外提） → 不动点
    /// 6. DeadCodeElimPass（死代码消除）
    /// 7. CopyPropPass（复制传播）
    pub fn optimization_pipeline(&self) -> crate::optimize::PassManager {
        let mut pm = crate::optimize::PassManager::new();

        // 1. Mem2Reg — 提升栈变量为 SSA（最早运行，最大化 SSA 覆盖）
        pm.add_pass(
            crate::optimize::Mem2RegPass::new(),
            crate::optimize::PassRunMode::Once,
        );

        // 2. 内联小函数（为后续优化创造机会）
        let func_table = self.function_table();
        pm.add_pass(
            crate::optimize::InlinePass::new(func_table.clone()),
            crate::optimize::PassRunMode::Once,
        );

        // 3. 如果有 const 函数，求值 const 调用
        let const_table = self.const_function_table();
        if !const_table.is_empty() {
            pm.add_pass(
                crate::optimize::ConstFnEvalPass::new(const_table),
                crate::optimize::PassRunMode::UntilFixedPoint,
            );
        }

        // 4. 代数简化 + 常量折叠
        pm.add_pass(
            crate::optimize::EGraphPass::new(),
            crate::optimize::PassRunMode::Once,
        );
        pm.add_pass(
            crate::optimize::ConstFoldPass::new(),
            crate::optimize::PassRunMode::UntilFixedPoint,
        );

        // 5. 稀疏条件常量传播
        pm.add_pass(
            crate::optimize::SccpPass::new(),
            crate::optimize::PassRunMode::Once,
        );

        // 6. Jump threading — 简化控制流
        pm.add_pass(
            crate::optimize::JumpThreadPass::new(),
            crate::optimize::PassRunMode::UntilFixedPoint,
        );

        // 7. 值编号 + CSE
        pm.add_pass(
            crate::optimize::GvnPass::new(),
            crate::optimize::PassRunMode::Once,
        );
        pm.add_pass(
            crate::optimize::CsePass::new(),
            crate::optimize::PassRunMode::Once,
        );

        // 8. 循环优化
        pm.add_pass(
            crate::optimize::LicmPass::new(),
            crate::optimize::PassRunMode::UntilFixedPoint,
        );
        pm.add_pass(
            crate::optimize::IndVarSimplifyPass::new(),
            crate::optimize::PassRunMode::Once,
        );
        pm.add_pass(
            crate::optimize::LoopUnrollPass::new(),
            crate::optimize::PassRunMode::Once,
        );

        // 9. 尾调用转换
        pm.add_pass(
            crate::optimize::TailCallPass::new(func_table),
            crate::optimize::PassRunMode::Once,
        );

        // 10. 清理
        pm.add_pass(
            crate::optimize::DeadCodeElimPass::new(),
            crate::optimize::PassRunMode::Once,
        );
        pm.add_pass(
            crate::optimize::CopyPropPass::new(),
            crate::optimize::PassRunMode::Once,
        );

        // 11. Phi 消除（SSA 解构，在寄存器分配前）
        pm.add_pass(
            crate::optimize::PhiElimPass::new(),
            crate::optimize::PassRunMode::Once,
        );

        pm
    }

    /// 构建完整的函数表：`FuncRef → Function`（所有函数）。
    pub fn function_table(&self) -> HashMap<FuncRef, Function> {
        self.functions
            .iter()
            .enumerate()
            .map(|(i, f)| (FuncRef(i as u32), f.clone()))
            .collect()
    }

    /// 在模块中的所有函数上运行给定的优化管道。
    ///
    /// 这是 `PassManager::run_on_module()` 的便捷包装。
    pub fn run_passes(
        &mut self,
        pm: &crate::optimize::PassManager,
    ) -> Result<crate::optimize::PassResult, crate::CompileError> {
        pm.run_on_module(&mut self.functions)
    }

    /// 在模块中的所有函数上运行默认优化管道。
    ///
    /// 等价于 `self.run_passes(&self.optimization_pipeline())`。
    pub fn optimize(&mut self) -> Result<crate::optimize::PassResult, crate::CompileError> {
        let pm = self.optimization_pipeline();
        self.run_passes(&pm)
    }

    /// 运行 LTO 跨模块优化。
    ///
    /// 将多个模块合并后执行跨函数内联和死函数删除。
    pub fn optimize_with_lto(modules: &[Module]) -> Result<Vec<Function>, crate::CompileError> {
        use crate::optimize::lto::{LtoContext, LtoPass};
        use crate::optimize::ModulePass;

        let mut ctx = LtoContext::new();
        for m in modules {
            ctx.add_module(m.clone());
        }
        let pass = LtoPass::new(ctx);
        // 合并所有函数并按 LTO pass 处理
        let mut all_funcs: Vec<Function> = modules
            .iter()
            .flat_map(|m| m.iter().cloned())
            .collect();
        pass.run_on_module(&mut all_funcs)?;
        Ok(all_funcs)
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;

    #[test]
    fn test_module_add_and_lookup() {
        let mut module = Module::new();
        let sig = Signature::new(&[], &[Type::I32]);

        let mut builder = FunctionBuilder::new("answer", sig.clone());
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        let v = builder.iconst_i32(42);
        builder.return_(&[v]);

        let func_ref = module.add_function(builder.finish()).unwrap();
        assert_eq!(func_ref, FuncRef(0));
        assert_eq!(module.len(), 1);
        assert_eq!(module.find_by_name("answer"), Some(FuncRef(0)));
        assert_eq!(module.get(func_ref).unwrap().name, "answer");
    }

    #[test]
    fn test_module_duplicate_name_error() {
        let mut module = Module::new();
        let sig = Signature::new(&[], &[]);

        let mut b1 = FunctionBuilder::new("f", sig.clone());
        b1.create_block();
        module.add_function(b1.finish()).unwrap();

        let mut b2 = FunctionBuilder::new("f", sig);
        b2.create_block();
        assert!(module.add_function(b2.finish()).is_err());
    }

    #[test]
    fn test_const_function_table() {
        let mut module = Module::new();
        let sig = Signature::new(&[], &[Type::I32]);

        // const function
        let mut builder = FunctionBuilder::new("const_fn", sig.clone());
        builder.set_const(true);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        let v = builder.iconst_i32(10);
        builder.return_(&[v]);
        module.add_function(builder.finish()).unwrap();

        // non-const function
        let mut builder2 = FunctionBuilder::new("runtime_fn", sig);
        let entry2 = builder2.create_block();
        builder2.switch_to_block(entry2);
        let v2 = builder2.iconst_i32(20);
        builder2.return_(&[v2]);
        module.add_function(builder2.finish()).unwrap();

        let const_table = module.const_function_table();
        assert_eq!(const_table.len(), 1);
        assert!(const_table.contains_key(&FuncRef(0)));
        assert!(module.is_const_fn(FuncRef(0)));
        assert!(!module.is_const_fn(FuncRef(1)));
    }

    #[test]
    fn test_module_optimization_pipeline() {
        let mut module = Module::new();
        let sig = Signature::new(&[], &[Type::I32]);

        // const fn answer() -> i32 { 42 }
        let mut builder = FunctionBuilder::new("answer", sig.clone());
        builder.set_const(true);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        let v = builder.iconst_i32(42);
        builder.return_(&[v]);
        module.add_function(builder.finish()).unwrap();

        // fn caller() -> i32 { answer() + 1 }
        let mut builder2 = FunctionBuilder::new("caller", sig);
        let entry2 = builder2.create_block();
        builder2.switch_to_block(entry2);
        let r = builder2.call(FuncRef(0), &[], &[Type::I32]);
        let one = builder2.iconst_i32(1);
        let result = builder2.iadd(r[0], one);
        builder2.return_(&[result]);
        module.add_function(builder2.finish()).unwrap();

        // Run optimization
        let result = module.optimize().unwrap();
        assert!(result.changed);

        // caller should now have answer() replaced with Iconst(42),
        // and 42 + 1 folded to Iconst(43)
        let caller = module.get(FuncRef(1)).unwrap();
        let has_optimized = caller.blocks[0].instructions.iter().any(|i| {
            if let Opcode::Iconst { index } = &i.opcode {
                caller
                    .constant_pool
                    .get(*index)
                    .and_then(|b| b.try_to_i64())
                    == Some(43)
            } else {
                false
            }
        });
        assert!(has_optimized, "Expected Iconst(43) after optimization");
    }

    #[test]
    fn test_global_variables() {
        let mut module = Module::new();

        let g1 =
            GlobalVariable::constant("PI", Type::F64).with_init(std::f64::consts::PI.to_le_bytes().to_vec());
        let g2 = GlobalVariable::mutable("counter", Type::I32).with_linkage(Linkage::Internal);

        let idx1 = module.add_global(g1).unwrap();
        let idx2 = module.add_global(g2).unwrap();

        assert_eq!(idx1, 0);
        assert_eq!(idx2, 1);
        assert_eq!(module.global_count(), 2);

        // 按名称查找
        assert_eq!(module.find_global("PI"), Some(0));
        assert_eq!(module.find_global("counter"), Some(1));
        assert_eq!(module.find_global("nonexistent"), None);

        // 按索引获取
        let pi = module.get_global(0).unwrap();
        assert!(pi.is_constant);
        assert_eq!(pi.name, "PI");
        assert_eq!(pi.ty, Type::F64);
        assert_eq!(pi.linkage, Linkage::External);

        let counter = module.get_global(1).unwrap();
        assert!(!counter.is_constant);
        assert_eq!(counter.linkage, Linkage::Internal);

        // 重复名称检查
        let dup = GlobalVariable::constant("PI", Type::I32);
        assert!(module.add_global(dup).is_err());
    }

    #[test]
    fn test_global_addr_with_global_variable() {
        let mut module = Module::new();
        let g =
            GlobalVariable::constant("my_const", Type::I32).with_init(42u32.to_le_bytes().to_vec());
        let g_idx = module.add_global(g).unwrap();

        // 构建使用 global_addr 的函数
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("main", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let addr = b.global_addr(g_idx);
        let val = b.load(addr, Type::I32);
        b.return_(&[val]);
        let func = b.finish();

        module.add_function(func).unwrap();

        // 验证全局变量和函数引用
        let global_ref = module.get_global(g_idx).unwrap();
        assert_eq!(global_ref.name, "my_const");
        assert_eq!(module.len(), 1);
    }
}
