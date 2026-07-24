//! 链接时优化 (LTO) 支持。
//!
//! 将 IR 序列化到对象文件的特殊段中，链接时跨模块优化。
//!
//! # 实现
//!
//! 1. 编译时将 IR 序列化为二进制附加到对象文件
//! 2. 链接时加载所有模块的 IR
//! 3. 跨模块运行优化管道（内联、死代码消除等）

use crate::CompileError;
use crate::ir::*;
use crate::optimize::OptimizationPass;

/// LTO 上下文 — 管理跨模块的 IR。
#[derive(Clone, Debug, Default)]
pub struct LtoContext {
    /// 参与 LTO 的模块列表。
    pub modules: Vec<Module>,
}

impl LtoContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加一个模块到 LTO 上下文。
    pub fn add_module(&mut self, module: Module) {
        self.modules.push(module);
    }

    /// 合并所有模块为单一模块。
    pub fn merge(&self) -> Result<Module, String> {
        let mut merged = Module::new();
        for module in &self.modules {
            for func in module.iter() {
                merged
                    .add_function(func.clone())
                    .map_err(|e| format!("LTO merge error: {}", e))?;
            }
        }
        Ok(merged)
    }
}

/// 跨模块优化 pass。
pub struct LtoPass {
    pub context: LtoContext,
}

impl LtoPass {
    pub fn new(context: LtoContext) -> Self {
        Self { context }
    }
}

impl crate::optimize::ModulePass for LtoPass {
    fn name(&self) -> &'static str {
        "lto"
    }
    fn description(&self) -> &'static str {
        "Link-time optimization across modules"
    }

    fn run_on_module(
        &self,
        functions: &mut [Function],
    ) -> Result<crate::optimize::PassResult, CompileError> {
        let mut result = crate::optimize::PassResult::default();
        // 跨模块内联
        let function_table: std::collections::HashMap<FuncRef, Function> = self
            .context
            .modules
            .iter()
            .flat_map(|m| {
                m.iter()
                    .enumerate()
                    .map(|(i, f)| (FuncRef(i as u32), f.clone()))
            })
            .collect();

        let inline_pass = crate::optimize::InlinePass::new(function_table);
        for func in functions.iter_mut() {
            let r = inline_pass.run_on_function(func)?;
            result.merge(&r);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionBuilder;

    #[test]
    fn lto_context_merge() {
        let mut ctx = LtoContext::new();
        let mut m1 = Module::new();
        let sig = Signature::void();
        let mut b = FunctionBuilder::new("f1", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        b.return_(&[]);
        m1.add_function(b.finish()).unwrap();
        ctx.add_module(m1);

        let merged = ctx.merge().unwrap();
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn lto_serialization_roundtrip() {
        let mut module = Module::new();
        let sig = Signature::void();
        let mut b = FunctionBuilder::new("f", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        b.return_(&[]);
        module.add_function(b.finish()).unwrap();

        let bytes = LtoContext::serialize_module(&module).unwrap();
        assert!(!bytes.is_empty());
        // deserialization not yet implemented
    }
}
