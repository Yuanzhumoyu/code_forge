//! **空闲池**：取号（含"回避入口实参寄存器"的变体）与归还。
//!
//! 从 `state.rs` 拆出（那里撞规模棘轮，见 `tests/file_size_ratchet.rs`）。归属：
//! 取号策略是分配器自己的记账，与"活区间怎么算"无关；三张表（`free_regs` /
//! `reg_owner` / `assignments`）的一致性约定写在 `state.rs` 的字段注释里。

use forge_ir::*;

use super::state::BtState;

impl<'a> BtState<'a> {
    pub(super) fn pop_free(&mut self, class: RegClass) -> Option<PReg> {
        self.pop_free_avoiding(class, &[])
    }

    /// 同 [`Self::pop_free`]，但**跳过 `avoid` 里的物理寄存器**。
    ///
    /// 唯一用途：形参预分配时回避**入口实参寄存器**（`RegAllocConfig::entry_arg_srcs`）
    /// ——收参是 `mov home, abi_src` 一串直线 mov，home 是运行期值，若某 home 恰好是
    /// 后面某个形参的 abi_src，先收的那条 mov 会踩掉后收的源（生成期排不出安全顺序）。
    ///
    /// **判定用 `RegClass::overlaps`（同号 = 同一物理寄存器文件），不用 `is_fp()` 这种
    /// 粗划分**：GPR(4)/GPR(8)/GPR(16) 同号是同一个物理寄存器，FPR(32)/VEC(128)/FPR(64)
    /// 同号也是同一寄存器文件，KReg 又要与 GPR 区分开。漏判（把占用者当成可用）会踩掉
    /// 入口源；多判只会多回避几个号（压力略升、绝不会错值）。
    pub(super) fn pop_free_avoiding(&mut self, class: RegClass, avoid: &[PReg]) -> Option<PReg> {
        // 某个候选寄存器是否"落在入口实参寄存器上"（号相同 + 类重叠）。
        let avoided = |p: PReg| {
            avoid
                .iter()
                .any(|a| a.num == p.num && a.class.overlaps(p.class))
        };
        loop {
            // 先取号（结束 free_regs 的可变借用），再检查物理占用/回避。
            let preg = {
                let pool = self.free_regs.get_mut(&class)?;
                // **确定性取号**（2026-08 修）：入池路径（`expire_dead` 的
                // `HashMap::retain` 随机序、`spill_vreg`）会让池序跨进程不同 ⇒
                // 直接 `pop()` 会让分配/溢出决策连锁非确定（每次编译产物不同）。
                //
                // **callee-saved 优先**：跨调用存活的值必须落在被保存的寄存器上，
                // 否则 call 点被 clobber 会强制 spill（riscv 递归 fib：参数先分到
                // caller-saved 的 t 系 ⇒ 与 `@move_args` 写的寄存器分叉 ⇒ 结果错）。
                // x86 的 callee-saved 恰在高编号，纯"取最大号"天然命中；riscv 的
                // callee-saved（X9/X18-X27）与 caller-saved 交错，需显式优先。
                if let Some(max_cs) = pool
                    .iter()
                    // 入口实参寄存器不能给形参当 home（见本函数文档）。
                    .filter(|p| !avoided(**p))
                    .filter(|p| {
                        self.config
                            .callee_saved
                            .iter()
                            .any(|(c, n)| *n == p.num && c.is_fp() == p.class.is_fp())
                    })
                    .max_by_key(|p| p.num)
                    .copied()
                {
                    pool.retain(|p| *p != max_cs);
                    Some(max_cs)
                } else if let Some((max_idx, _)) = pool
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| !avoided(**p))
                    .max_by_key(|(_, p)| p.num)
                {
                    Some(pool.swap_remove(max_idx))
                } else {
                    None
                }
            }?;
            // 跳过已被占用的寄存器（spill/驱逐可能把已占寄存器残留进空闲池；跨宽度类
            // 重叠 GPR(4)/GPR(8) 同编号也视为占用）。入池路径都会先 `clear_reg_owner`，
            // 这里只是防御历史/边角污染——被丢弃的寄存器会在占用者释放时自然回池。
            if !avoided(preg) && !self.phys_conflicts(preg) {
                return Some(preg);
            }
        }
    }

    pub(super) fn remove_from_free(&mut self, preg: PReg) {
        if let Some(pool) = self.free_regs.get_mut(&preg.class) {
            pool.retain(|p| *p != preg);
        }
    }
}
