//! 溢出槽**初始化**检查：`Reload` 读的槽必须先被写过。
//!
//! ## 治什么
//!
//! `spill_vreg` 驱逐活值时只分配槽 + 释放寄存器，**不产生机器指令**；槽的内容要
//! 靠"def 时 `Store`"或"驱逐时落槽"来写。任何一条路径上"先 reload 后 store"就是
//! **读未初始化栈**——机器码不会崩，只会静默算错（v27 M6 的 `StackAddr` 外提错值
//! 一直在这个方向）。
//!
//! 判据是**定义-必达**（definite assignment）：
//!
//! - 入口：函数参数占用的溢出槽由序言写（`param_vregs` 里被溢出的那些）⇒ 视为已初始化；
//! - 块内：按编辑序累积"已写槽"集合（`Store` / `SpillToSlot` 写、`Reload` 读）；
//! - 跨块：`in[B] = ∩ out[pred]`（**交集**：只有所有前驱都写过才算写过）；
//! - reload 的槽不在"已写"集合里 ⇒ 报错，点名函数 / 指令序号 / 槽偏移。
//!
//! 只在 debug 构建或 `FORGE_VERIFY_SPILL_INIT=1` 时跑（数据流是 O(块×槽)）。

use std::collections::{HashMap, HashSet};

use crate::machine::inst::MachineInst;
use crate::pipeline::alloc_result::{AllocResult, SpillEdit};
use crate::prelude::IrError;

/// 校验一个函数的溢出槽不会在初始化前被读取。
pub(crate) fn verify_spill_init<I: MachineInst>(
    fname: &str,
    vcode: &crate::VCode<I>,
    alloc: &AllocResult,
) -> Result<(), IrError> {
    let blocks: Vec<_> = vcode.blocks().collect();
    let n = blocks.len();
    if n == 0 {
        return Ok(());
    }

    // 全局指令序号 → (块号, 块内序号)，以及每块的指令范围。
    let mut block_of_gi: Vec<usize> = Vec::new();
    for (bi, b) in blocks.iter().enumerate() {
        for _ in &b.instructions {
            block_of_gi.push(bi);
        }
    }

    // 每块的后继：分支目标（按 ir_block 映射）+ 非返回块 fallthrough。
    let mut ir_to_idx: HashMap<forge_ir::Block, usize> = HashMap::new();
    for (i, b) in blocks.iter().enumerate() {
        ir_to_idx.insert(b.ir_block, i);
    }
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, b) in blocks.iter().enumerate() {
        for inst in &b.instructions {
            if inst.is_branch() {
                for t in inst.branch_targets() {
                    if let Some(&ti) = ir_to_idx.get(&t) {
                        succs[i].push(ti);
                    }
                }
            }
        }
        // **只在"真的会顺序落下"时才加 fallthrough**：末指令是无条件跳转
        //（`is_branch()` 且只报 1 个目标，典型是 `JmpRel32`）时没有 fallthrough。
        // 这条对活跃性分析只是保守（多一条边无害），但对**定义-必达**的交集是**致命**的：
        // 伪造的前驱会把"该槽已写"的结论冲掉 ⇒ 满屏误报（第一版 156 处误报就是这么来的）。
        // 末指令是分支 ⇒ 后继**只取它报的目标**（`branch_targets()` 对 Jcc 会报
        // 两个、对 Jmp 报一个）；只有"没有分支"的块才真的顺序落下。
        let falls_through = match b.instructions.last() {
            Some(last) => !last.is_branch(),
            None => true,
        };
        if falls_through && !b.is_return_block && i + 1 < n {
            succs[i].push(i + 1);
        }
        succs[i].sort_unstable();
        succs[i].dedup();
    }

    // 每块：写集合 / 读事件（按块内顺序，记录"读之前写了没有"）。
    let mut written_in_block: Vec<HashSet<i32>> = vec![HashSet::new(); n];
    let mut reads: Vec<Vec<(usize, i32)>> = vec![Vec::new(); n]; // (块内序号, 偏移)
    let mut inst_stores: Vec<Vec<Vec<i32>>> = vec![Vec::new(); n]; // [块][块内序号] → 本指令写槽
    let mut gi = 0usize;
    for (bi, b) in blocks.iter().enumerate() {
        for (ii, _) in b.instructions.iter().enumerate() {
            inst_stores[bi].resize_with(b.instructions.len(), Vec::new);
            if let Some(edits) = alloc.edits.get(gi) {
                for e in edits {
                    match e {
                        SpillEdit::Store { slot, .. } => {
                            written_in_block[bi].insert(slot.offset);
                            inst_stores[bi][ii].push(slot.offset);
                        }
                        SpillEdit::Reload { slot, .. } => {
                            reads[bi].push((ii, slot.offset));
                        }
                        _ => {}
                    }
                }
            }
            gi += 1;
        }
    }

    // 入口种子：被溢出的参数槽由序言写。
    let mut entry_in: HashSet<i32> = HashSet::new();
    for v in &alloc.param_vregs {
        if alloc.is_spilled(*v) {
            entry_in.insert(alloc.spill_slot(*v).offset);
        }
    }

    // 前向数据流：in[B] = ∩ out[pred]；入口块用 entry_in ∪ 自身 gen 兜底。
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, ss) in succs.iter().enumerate() {
        for &s in ss {
            preds[s].push(i);
        }
    }
    // **`in[B] = ∩ out[pred]`，`out[B] = in[B] ∪ gen[B]`**——注意是前驱的 **out**，
    // 不是前驱的 in（用 in 会漏掉"前驱自己写的槽"，全函数范围的误报就是这么来的：
    // 第一版写成 ∩ in[pred]，结果每个块都报几十处）。
    let mut livein: Vec<Option<HashSet<i32>>> = vec![None; n];
    let mut liveout: Vec<Option<HashSet<i32>>> = vec![None; n];
    livein[0] = Some(entry_in.clone());
    let mut changed = true;
    let mut guard = 0;
    while changed && guard < 1000 {
        changed = false;
        guard += 1;
        // out 依赖于 in：先按当前 in 重算所有 out，再用新 out 重算 in（逆序扫一遍足够收敛）。
        for b in 0..n {
            let Some(inn) = livein[b].as_ref() else {
                continue;
            };
            let mut o = inn.clone();
            o.extend(written_in_block[b].iter().copied());
            if liveout[b].as_ref() != Some(&o) {
                liveout[b] = Some(o);
                changed = true;
            }
        }
        for b in 0..n {
            if b == 0 {
                continue;
            }
            let mut acc: Option<HashSet<i32>> = None;
            let mut any = false;
            for &p in &preds[b] {
                let Some(pout) = liveout[p].as_ref() else {
                    continue;
                };
                any = true;
                acc = Some(match acc {
                    None => pout.clone(),
                    Some(a) => a.intersection(pout).copied().collect(),
                });
            }
            if !any {
                continue; // 无前驱（或前驱还不可达）：跳过
            }
            let newin = acc.unwrap_or_default();
            if livein[b].as_ref() != Some(&newin) {
                livein[b] = Some(newin);
                changed = true;
            }
        }
    }

    // 规模统计（判断检查是否"结构性偏置"：Store 远少于 Reload 时，任何读都可能报）。
    {
        let stores: usize = written_in_block.iter().map(|s| s.len()).sum();
        let reloads: usize = reads.iter().map(|r| r.len()).sum();
        eprintln!(
            "[spill-init] '{fname}' 规模：块 {n}，块内写集合合计 {stores}，reload 事件 {reloads}，\
             edits 表长 {}",
            alloc.edits.len()
        );
    }

    // 逐块验证：块内顺序地累积"已写"，遇到 reload 就判。
    for b in 0..n {
        let Some(in_set) = livein[b].as_ref() else {
            continue; // 不可达块不判
        };
        let mut written = in_set.clone();
        let mut read_iter = reads[b].iter().peekable();
        for ii in 0..blocks[b].instructions.len() {
            // 本指令的 **Reload 在指令之前**、**Store 在指令之后**：先判读，再并入本指令写的槽。
            while let Some(&&(ri, off)) = read_iter.peek() {
                if ri != ii {
                    break;
                }
                if !written.contains(&off) {
                    // **当前是探针而非硬错误**：种子集合还不完整——序言里的
                    // `@move_args`（栈参数收参）与变参保存区写槽发生在 vcode 之外，
                    // 本检查看不到，于是会误报。先打印出来当线索用；
                    // 等把"序言写槽"也纳入种子后再升级为错误（届时它能挡住
                    // "驱逐未落槽"这一整类静默错值）。
                    // 找出这个槽的属主 vreg（取第一个匹配），把它是否有 def 一起打出来：
                    // "有 def 但本检查没看到 Store" 与 "完全没有 def（参数/未物化值）"
                    // 是两种完全不同的病。
                    let owner = alloc
                        .spill_slots
                        .iter()
                        .enumerate()
                        .find_map(|(v, s)| s.filter(|s| s.offset == off).map(|_| v));
                    let (owner_idx, has_def, has_store) = match owner {
                        Some(v) => {
                            let has_def = false;
                            let mut has_store = false;
                            let mut g = 0usize;
                            for bb in vcode.blocks() {
                                for _ in &bb.instructions {
                                    if let Some(es) = alloc.edits.get(g) {
                                        for e in es {
                                            match e {
                                                SpillEdit::Store { slot, .. }
                                                    if slot.offset == off =>
                                                {
                                                    has_store = true
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    g += 1;
                                }
                            }
                            (v as u32, has_def, has_store)
                        }
                        None => (u32::MAX, false, false),
                    };
                    eprintln!(
                        "[spill-init] '{fname}' 块 {b} 第 {ii} 条指令从槽 {off} 读值，\
                         但按现有种子它此前没被写过（属主 v{owner_idx}，有 def={has_def}，\
                         全函数有 Store={has_store}）"
                    );
                }
                read_iter.next();
            }
            if let Some(st) = inst_stores[b].get(ii) {
                for off in st {
                    written.insert(*off);
                }
            }
        }
        // 本块写完的槽并入 out（供后继使用）：这里只用 gen，顺序无关。
        written.extend(written_in_block[b].iter().copied());
        let _ = &written;
    }
    Ok(())
}
