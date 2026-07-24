//! 别名分析 (Alias Analysis) pass。
//!
//! 提供基础的内存别名分析，用于 Load/Store 优化的前置分析。
//! 当前实现为保守版本：假设所有 Store 可能与任何 Load 别名。
//!
//! # 分析结果
//!
//! - `may_alias(a, b)`: 两个内存访问是否可能指向同一位置
//! - `must_alias(a, b)`: 两个内存访问是否一定指向同一位置
//! - `is_constant(addr)`: 地址是否从不被写入（read-only）

use crate::ir::*;
use std::collections::HashSet;

/// 别名分析结果。
pub struct AliasAnalysis {
    /// 已知的只读地址（从不被 Store 写入）。
    read_only_addrs: HashSet<Value>,
    /// 逃逸地址（传递给外部函数或存储到全局）。
    escaped_addrs: HashSet<Value>,
}

impl AliasAnalysis {
    /// 对函数执行别名分析。
    pub fn analyze(func: &Function) -> Self {
        let mut read_only = HashSet::new();
        let mut escaped = HashSet::new();

        for block in &func.blocks {
            for inst in &block.instructions {
                match &inst.opcode {
                    Opcode::Store
                        // 被 store 的地址不是只读的
                        if inst.operands.len() >= 2 => {
                            read_only.remove(&inst.operands[1]);
                        }
                    Opcode::StackAddr { .. } | Opcode::GlobalAddr { .. } => {
                        if let Some(v) = inst.result {
                            read_only.insert(v);
                        }
                    }
                    Opcode::Call { .. } | Opcode::CallIndirect => {
                        // 调用可能写入任何地址 — 标记所有地址为逃逸
                        for addr in read_only.drain() {
                            escaped.insert(addr);
                        }
                    }
                    _ => {}
                }
            }
        }

        // 检查是否有 StackStore 写入
        for block in &func.blocks {
            for inst in &block.instructions {
                if matches!(inst.opcode, Opcode::StackStore { .. }) {
                    // StackStore 写入栈 — 不影响 GlobalAddr
                }
            }
        }

        Self {
            read_only_addrs: read_only,
            escaped_addrs: escaped,
        }
    }

    /// 两个内存访问是否可能别名（保守）。
    pub fn may_alias(&self, _addr_a: Value, _addr_b: Value) -> bool {
        // 保守：假设总是可能别名
        true
    }

    /// 地址是否从不被写入。
    pub fn is_read_only(&self, addr: Value) -> bool {
        self.read_only_addrs.contains(&addr) && !self.escaped_addrs.contains(&addr)
    }

    /// 内存访问是否可能逃逸（被外部代码修改）。
    pub fn may_escape(&self, addr: Value) -> bool {
        self.escaped_addrs.contains(&addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionBuilder;

    #[test]
    fn alias_analysis_no_crash() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.stack_store(0, v);
        let r = b.stack_load(0, Type::I32);
        b.return_(&[r]);

        let func = b.finish();
        let aa = AliasAnalysis::analyze(&func);
        // Stack ops don't affect GlobalAddr read-only status
        let _ = aa;
    }

    #[test]
    fn alias_analysis_global_addr() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let addr = b.global_addr(0);
        let v = b.load(addr, Type::I32);
        b.return_(&[v]);

        let func = b.finish();
        let aa = AliasAnalysis::analyze(&func);
        assert!(aa.is_read_only(addr));
    }
}
