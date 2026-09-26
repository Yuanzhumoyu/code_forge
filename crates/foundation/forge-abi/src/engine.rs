//! 通用 ABI 引擎：**规则（约定） + 绑定（ISA 寄存器） + 目标能力** → [`AbiPlan`]。
//!
//! 调用方与被调方共用同一个引擎与同一份 plan：不再有两处手写序列靠约定"对齐"
//! （这正是旧设计里 sret 位置、栈参数偏移、callee-saved 序列各自为政的根源）。

use std::collections::BTreeMap;

use crate::binding::AbiBinding;
use crate::error::AbiError;
use crate::plan::{
    AbiPlan, ArgLoc, CalleeSavedPlan, DeclAttrs, Extension, HiddenSlots, Placement, Purpose,
    RegRef, RetLoc, StackLayout, VaArea,
};
use crate::registry::AbiHooks;
use crate::rules::{AbiRules, ClassAction, IndirectVia, PositionRule};
use crate::ty::TyView;

/// ISA 需要向引擎申报的**能力**（"能不能用某类指令"，而不是"约定了什么"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    /// 寄存器 ← 寄存器 的整数移动（收参/返参搬运）。
    GprMov,
    /// 浮点寄存器移动（按宽度）。
    FprMov,
    /// 向量寄存器移动（按宽度）。
    VecMov,
    /// 调整栈指针（帧分配/释放、被叫方弹栈）。
    SpAdjust,
    /// 栈参数装载（被叫方从调用方栈区取参）。
    StackArgLoad,
    /// 栈参数存放（调用方写栈参数）。
    StackArgStore,
    /// 帧内地址计算（byval 副本 / sret 缓冲）。
    FrameAddr,
    /// 宽向量片段搬运（>16B 的 byval/sret 拷贝）。
    WideVecMove,
    /// 直接调用（函数符号）。
    Call,
    /// 间接调用。
    CallIndirect,
    /// 返回。
    Ret,
}

impl Capability {
    pub fn name(self) -> &'static str {
        match self {
            Capability::GprMov => "gpr_mov",
            Capability::FprMov => "fpr_mov",
            Capability::VecMov => "vec_mov",
            Capability::SpAdjust => "sp_adjust",
            Capability::StackArgLoad => "stack_arg_load",
            Capability::StackArgStore => "stack_arg_store",
            Capability::FrameAddr => "frame_addr",
            Capability::WideVecMove => "wide_vec_move",
            Capability::Call => "call",
            Capability::CallIndirect => "call_indirect",
            Capability::Ret => "ret",
        }
    }
}

/// 目标 ISA 的能力视图（由 ISA 侧的薄适配器实现；**只答能力，不答约定**）。
pub trait AbiTarget {
    /// ISA 名。
    fn isa_name(&self) -> &str;
    /// 物理寄存器总数。
    fn reg_count(&self) -> u32;
    /// 寄存器名（诊断/快照用）。
    fn reg_name(&self, index: u32) -> Option<String>;
    /// 寄存器类名（诊断/快照用，如 `"GPR(8)"`）。
    fn reg_class_name(&self, index: u32) -> String;
    /// 按名字查号。
    fn reg_index(&self, name: &str) -> Option<u32>;
    /// 该类寄存器的字节宽度。
    fn reg_width(&self, index: u32) -> u8;
    /// 该寄存器是否**固定用途/不可分配**（零寄存器、平台寄存器、sp/fp/ra…）。
    fn pinned(&self, index: u32) -> bool;
    /// 可分配的通用寄存器（用于推导"被调方可能破坏谁"）。
    fn allocatable(&self) -> Vec<u32>;
    /// spill 搬运可借用的 scratch。
    fn spill_scratch(&self) -> Vec<u32> {
        Vec::new()
    }
    /// 链接寄存器（ra/lr）。
    fn link_reg(&self) -> Option<u32> {
        None
    }
    /// 能力 → 可编码的位宽（`None` = 该 ISA 没有这个能力）。
    fn cap(&self, cap: Capability) -> Option<u16>;
}

/// 一个函数的 ABI 视图（调用方/被调方共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// `(名字, 类型)`；名字只用于诊断与快照。
    pub params: Vec<(String, TyView)>,
    /// 每条形参的**声明属性**（LLVM `byval`/`sret`/`inreg`/`zeroext`/`signext`/`align`）。
    ///
    /// 可以比 `params` 短（缺的按默认处理，见 [`Signature::attr`]）——前端只有"部分参数
    /// 带属性"时不必补齐一长串默认值。
    pub attrs: Vec<DeclAttrs>,
    pub ret: Option<TyView>,
    /// 返回值的声明属性（`byval`/`sret` 在返回位没有意义，引擎忽略它们）。
    pub ret_attrs: DeclAttrs,
    pub variadic: bool,
    /// **命名参数个数**（变参函数里 `i >= fixed_count` 的是"未命名实参"；
    /// 多数约定把它们强制走栈，SysV 则继续用寄存器）。
    pub fixed_count: usize,
}

impl Signature {
    pub fn new(params: Vec<(String, TyView)>, ret: Option<TyView>) -> Self {
        let n = params.len();
        Self {
            params,
            attrs: Vec::new(),
            ret,
            ret_attrs: DeclAttrs::default(),
            variadic: false,
            fixed_count: n,
        }
    }

    /// 带**声明属性**的签名（可与 `params` 等长，也可更短）。
    pub fn with_attrs(mut self, attrs: Vec<DeclAttrs>) -> Self {
        self.attrs = attrs;
        self
    }

    /// 返回值属性。
    pub fn with_ret_attrs(mut self, attrs: DeclAttrs) -> Self {
        self.ret_attrs = attrs;
        self
    }

    /// 第 `i` 个形参的声明属性（缺省 = 无属性）。
    pub fn attr(&self, i: usize) -> DeclAttrs {
        self.attrs.get(i).copied().unwrap_or_default()
    }

    /// 变参：`fixed` 是命名参数个数，其余为未命名实参。
    pub fn variadic(mut self, fixed: usize) -> Self {
        self.variadic = true;
        self.fixed_count = fixed;
        self
    }
}

/// 池的游标状态（按 `position` 规则推进）。
struct Pools<'a> {
    binding: &'a AbiBinding,
    target: &'a dyn AbiTarget,
    rules: &'a AbiRules,
    cache: BTreeMap<String, Vec<RegRef>>,
    /// 按类计数时的游标（池名 → 下一个可用下标）。
    cursors: BTreeMap<String, usize>,
    /// 按位置计数时**共享**的位置游标（int/float/vector 共用）。
    shared_cursor: usize,
}

impl<'a> Pools<'a> {
    fn get(&mut self, pool: &str) -> Result<&Vec<RegRef>, AbiError> {
        if !self.cache.contains_key(pool) {
            let regs = self.binding.resolve_pool(pool, self.target)?;
            self.cache.insert(pool.to_string(), regs);
        }
        Ok(&self.cache[pool])
    }

    /// 当前游标（按位置规则时 int/float/vector 共用）。
    fn cursor(&self, pool: &str) -> usize {
        match self.rules.position {
            PositionRule::ByClass => *self.cursors.get(pool).unwrap_or(&0),
            PositionRule::ByPosition => self.shared_cursor,
        }
    }

    fn set_cursor(&mut self, pool: &str, at: usize) {
        match self.rules.position {
            PositionRule::ByClass => {
                self.cursors.insert(pool.to_string(), at);
            }
            PositionRule::ByPosition => {
                self.shared_cursor = at;
            }
        }
    }

    /// 连续取 `n` 个可用槽；**不够就一个都不取**（不推进游标）。
    ///
    /// 固定用途（pinned）与被 hidden 占用的寄存器会被**占位跳过**——位置计数里
    /// 它们同样消耗一个位置，这一点是 Windows x64/AAPCS64 都对得上的。
    fn take_n(
        &mut self,
        pool: &str,
        n: usize,
        skip_used: &[u32],
    ) -> Result<Option<Vec<RegRef>>, AbiError> {
        let regs = self.get(pool)?.clone();
        let mut k = self.cursor(pool);
        let mut picked: Vec<RegRef> = Vec::with_capacity(n);
        while picked.len() < n {
            let Some(r) = regs.get(k) else {
                return Ok(None); // 池不够：整组落到栈
            };
            k += 1;
            if self.target.pinned(r.index) || skip_used.contains(&r.index) {
                continue;
            }
            picked.push(r.clone());
        }
        self.set_cursor(pool, k);
        Ok(Some(picked))
    }

    /// 取一个槽。
    fn take(&mut self, pool: &str, skip_used: &[u32]) -> Result<Option<RegRef>, AbiError> {
        Ok(self.take_n(pool, 1, skip_used)?.map(|mut v| v.remove(0)))
    }

    /// 预占一个池的槽位（hidden 指针占位：把游标推到 slot 之后）。
    fn reserve_slot(&mut self, pool: &str, slot: u32) {
        let at = self.cursor(pool).max(slot as usize + 1);
        self.set_cursor(pool, at);
    }
}

/// 栈分配器（被叫方视角：偏移从 `first_offset_slots × slot_bytes` 起）。
struct StackAlloc {
    cursor: u32,
    max: u32,
}

impl StackAlloc {
    fn new(start: u32) -> Self {
        Self {
            cursor: start,
            max: start,
        }
    }

    /// 分配一段（按 align 与槽单位向上取整），返回偏移。
    fn alloc(&mut self, size: u32, align: u32, slot: u32) -> u32 {
        let step = align.max(slot).max(1);
        self.cursor = self.cursor.div_ceil(step) * step;
        let off = self.cursor;
        self.cursor += size.div_ceil(slot.max(1)) * slot.max(1);
        self.max = self.max.max(self.cursor);
        off
    }
}

/// 引擎入口：算出一个签名的完整调用计划。
///
/// **两侧对称**：同一份 plan 既描述调用方怎么写（[`StackLayout::caller_offset`]、
/// 实参落点），也描述被调方怎么收（栈参数偏移、hidden 槽、callee-saved 集合）。
pub fn plan_fn(
    target: &dyn AbiTarget,
    rules: &AbiRules,
    binding: &AbiBinding,
    sig: &Signature,
    hooks: Option<&dyn AbiHooks>,
) -> Result<AbiPlan, AbiError> {
    rules.validate()?;
    binding.validate()?;
    if binding.conv != rules.name {
        return Err(AbiError::BadRules {
            name: binding.isa.clone(),
            why: format!(
                "绑定声称约定 `{}`，但规则是 `{}`——两者必须一致",
                binding.conv, rules.name
            ),
        });
    }

    let mut pools = Pools {
        binding,
        target,
        rules,
        cache: BTreeMap::new(),
        cursors: BTreeMap::new(),
        shared_cursor: 0,
    };
    // 返回寄存器与参数寄存器是**两套位置序列**（x86：返回在 RAX/XMM0，参数从 RCX/RDI 起；
    // 按位置计数的约定若共用游标，一个 2 槽返回会把第一个参数挤到第 3 个位置）。
    let mut ret_pools = Pools {
        binding,
        target,
        rules,
        cache: BTreeMap::new(),
        cursors: BTreeMap::new(),
        shared_cursor: 0,
    };
    let mut hidden = HiddenSlots::default();
    let mut skip_used: Vec<u32> = Vec::new();

    // ① 返回值先分类：`Indirect{HiddenSret}` 要预占 hidden 槽（x86 RCX / AAPCS64 x8 /
    //    riscv a0），并且**用户实参要从它之后开始**（这正是旧实现"首 int 槽"的
    //    硬编码想表达、但换个 ISA 就错的那件事）。
    let ret_view = sig.ret.clone();
    let ret_action = match &ret_view {
        Some(ty) => classify(rules, target, ty, sig.variadic, hooks, ClassDir::Ret)?,
        None => ClassAction::Ignore,
    };
    let mut ret = RetLoc::Void;
    {
        let pools = &mut ret_pools;
        match (&ret_view, &ret_action) {
            (None, _) => {}
            (Some(ty), ClassAction::Direct { pool, slots }) => {
                let n = slots.resolve(ty).ok_or_else(|| AbiError::Unsupported {
                    conv: rules.name.clone(),
                    what: format!(
                        "返回值 `{}` 不适用槽数规则 `{slots:?}`（`hfa` 只对同质浮点聚合成立）",
                        crate::plan::ty_text(ty)
                    ),
                })?;
                if n == 1 {
                    match pools.take(pool, &skip_used)? {
                        Some(r) => ret = RetLoc::Reg { reg: r },
                        None => {
                            return Err(AbiError::PoolExhausted {
                                conv: rules.name.clone(),
                                arg: 0,
                                pool: pool.clone(),
                            });
                        }
                    }
                } else if n == 2 {
                    match pools.take_n(pool, 2, &skip_used)? {
                        Some(mut regs) if regs.len() == 2 => {
                            ret = RetLoc::RegPair {
                                lo: regs.remove(0),
                                hi: regs.remove(0),
                            }
                        }
                        _ => {
                            return Err(AbiError::Unsupported {
                                conv: rules.name.clone(),
                                what: format!(
                                    "返回值 `{}` 需要 2 个寄存器槽，池 `{pool}` 不够",
                                    crate::plan::ty_text(ty)
                                ),
                            });
                        }
                    }
                } else {
                    // ≥3 槽的返回（如 AAPCS64 的 4×f32 HFA 返回）：模型能表达
                    // （`Placement::RegGroup` 同形），但返回路径的搬运留到 A6——
                    // **明确拒绝**，不降级、不静默只回第一个寄存器。
                    return Err(AbiError::Unsupported {
                        conv: rules.name.clone(),
                        what: format!(
                            "返回值 `{}` 需要 {n} 个连续寄存器槽（≥3 的返回搬运见 A6）",
                            crate::plan::ty_text(ty)
                        ),
                    });
                }
            }
            (Some(ty), ClassAction::Pair { lo, hi }) => {
                let l = pools.take(lo, &skip_used)?;
                let h = pools.take(hi, &skip_used)?;
                match (l, h) {
                    (Some(l), Some(h)) => ret = RetLoc::RegPair { lo: l, hi: h },
                    _ => {
                        return Err(AbiError::Unsupported {
                            conv: rules.name.clone(),
                            what: format!(
                                "返回值 `{}` 的 (lo,hi) 拆分取不到寄存器（两个池里至少有一个不够）",
                                crate::plan::ty_text(ty)
                            ),
                        });
                    }
                }
            }
            (Some(ty), ClassAction::Indirect { via }) => {
                match via {
                    IndirectVia::HiddenSret => {
                        let pool = rules.hidden.sret_pool.clone().ok_or_else(|| {
                            AbiError::Unsupported {
                                conv: rules.name.clone(),
                                what: "宽返回需要 sret 槽，但约定没有声明 hidden.sret_pool".into(),
                            }
                        })?;
                        pools.reserve_slot(&pool, rules.hidden.sret_slot);
                        let regs = pools.get(&pool)?.clone();
                        let slot = rules.hidden.sret_slot as usize;
                        let rr = regs
                            .get(slot)
                            .cloned()
                            .ok_or_else(|| AbiError::Unsupported {
                                conv: rules.name.clone(),
                                what: format!("hidden.sret_pool `{pool}` 没有第 {slot} 个槽"),
                            })?;
                        skip_used.push(rr.index);
                        hidden.sret = Some(rr);
                        ret = RetLoc::Indirect {
                            size: ty.size,
                            align: ty.align as u16,
                        };
                    }
                    IndirectVia::CallerStackCopy => {
                        // 调用方传指针 → 被调方通过指针写回；指针本身占一个 int 槽。
                        let pool = rules.int_pool.clone();
                        match pools.take(&pool, &skip_used)? {
                            Some(r) => {
                                ret = RetLoc::Reg { reg: r };
                            }
                            None => {
                                return Err(AbiError::Unsupported {
                                    conv: rules.name.clone(),
                                    what: "间接返回需要 int 池里的指针槽，池已耗尽".into(),
                                });
                            }
                        }
                    }
                }
            }
            (Some(_), ClassAction::Stack { align }) => {
                // 返回值放栈：AAPCS64 之外少见；需要 hidden 指针告诉被调方写哪儿。
                let pool = rules
                    .hidden
                    .sret_pool
                    .clone()
                    .ok_or_else(|| AbiError::Unsupported {
                        conv: rules.name.clone(),
                        what: "栈上返回需要约定声明 hidden.sret_pool（间接结果指针）".into(),
                    })?;
                pools.reserve_slot(&pool, rules.hidden.sret_slot);
                let regs = pools.get(&pool)?.clone();
                let slot = rules.hidden.sret_slot as usize;
                let rr = regs
                    .get(slot)
                    .cloned()
                    .ok_or_else(|| AbiError::Unsupported {
                        conv: rules.name.clone(),
                        what: format!("hidden.sret_pool `{pool}` 没有第 {slot} 个槽"),
                    })?;
                skip_used.push(rr.index);
                hidden.sret = Some(rr);
                ret = RetLoc::Indirect {
                    size: ret_view.as_ref().map(|t| t.size).unwrap_or(0),
                    align: align.unwrap_or(1) as u16,
                };
            }
            (Some(_), ClassAction::Ignore) => {}
        }
    }

    // ② context（Swift self / Go context / 闭包 env）：占掉约定声明的槽。
    if let Some(pool) = &rules.hidden.context_pool {
        pools.reserve_slot(pool, rules.hidden.context_slot);
        let regs = pools.get(pool)?.clone();
        let slot = rules.hidden.context_slot as usize;
        if let Some(rr) = regs.get(slot).cloned() {
            skip_used.push(rr.index);
            hidden.context = Some(rr);
        }
    }
    // ③ 变参元信息寄存器（SysV `%al` / RISC-V `LEN`）。
    if sig.variadic {
        for (pool, slot_ref) in [
            (&rules.hidden.va_meta_pool, &mut hidden.va_meta),
            (&rules.hidden.va_len_pool, &mut hidden.va_len),
        ] {
            if let Some(pool) = pool {
                let regs = pools.get(pool)?.clone();
                if let Some(rr) = regs.first().cloned() {
                    skip_used.push(rr.index);
                    *slot_ref = Some(rr);
                }
            }
        }
    }

    // ④ 参数。
    let stack_start = rules.stack.first_offset_slots * rules.stack.slot_bytes;
    let mut stack = StackAlloc::new(stack_start);
    // byval（调用方栈上副本）走**独立**的临时区：坐标系从 0 起，与传出参数区无关。
    let mut byval = StackAlloc::new(0);
    let mut args: Vec<ArgLoc> = Vec::with_capacity(sig.params.len());
    for (i, (name, ty)) in sig.params.iter().enumerate() {
        let action = classify(rules, target, ty, sig.variadic, hooks, ClassDir::Param)?;
        // 变参的**未命名实参**（`i >= fixed_count`）：`variadic_stack_only` 的约定
        // （Win64/AAPCS64/RISC-V）强制走栈；SysV 继续按普通规则落寄存器。
        let unnamed = sig.variadic && i >= sig.fixed_count;
        let attrs = sig.attr(i);
        let place = if attrs.sret {
            // 声明了 `sret`：这就是**间接结果指针**，占约定声明的 hidden 槽
            //（x86 RCX/RDI、AAPCS64 **x8**、riscv a0）——不再走普通参数分类。
            let slot = sret_slot(&mut pools, &mut hidden, rules, &mut skip_used)?;
            Placement::Indirect {
                ptr: Some(slot),
                at: None,
                on_stack: false,
            }
        } else if let Some(size) = attrs.byval {
            // 声明了 `byval(N)`：调用方在自己栈上做 N 字节副本、传指针（指针按 int 池传）。
            byval_place(&mut pools, &mut byval, rules, ty, size, &skip_used)?
        } else if unnamed && rules.variadic_stack_only {
            stack_place(&mut stack, rules, ty, &action)
        } else {
            let mut p = place_arg(
                &mut pools, &mut stack, &mut byval, rules, ty, &action, &skip_used,
            )?;
            // `inreg`：分类说走栈时再试一次寄存器（LLVM 的语义是"优先寄存器"，不是硬要求；
            // 池真空了就仍走栈——这一点写在 plan 里是可见的，不会静默换寄存器）。
            if attrs.inreg
                && matches!(p, Placement::Stack { .. })
                && let Some(reg) = pools.take(&rules.int_pool, &skip_used)?
            {
                p = Placement::Reg {
                    reg,
                    ext: attrs.extension(),
                    purpose: Purpose::Normal,
                };
            }
            // 声明属性折进落点：符号扩展 + 声明的对齐。
            apply_decl_attrs(p, ty, &attrs, rules)
        };
        args.push(ArgLoc {
            what: name.clone(),
            index: Some(i),
            size: ty.size,
            place,
        });
    }

    let stack_args_bytes = stack.max.saturating_sub(stack_start);
    let arg_area_bytes = stack_args_bytes + rules.shadow_bytes;
    let layout = StackLayout {
        align: rules.stack_align,
        slot_bytes: rules.stack.slot_bytes,
        shadow_bytes: rules.shadow_bytes,
        first_arg_offset: stack_start as i32,
        arg_area_bytes,
        byval_area_bytes: byval.max,
        red_zone: rules.red_zone,
        frame_padding: rules.frame_padding,
    };

    // ⑤ callee-saved：池里列出的寄存器（按序）。
    let mut cs_regs: Vec<RegRef> = Vec::new();
    for pool in &rules.callee_saved.pools {
        cs_regs.extend(pools.get(pool)?.clone());
    }
    let callee_saved = CalleeSavedPlan {
        mechanism: rules.callee_saved.mechanism,
        regs: cs_regs,
        includes_fp: rules.callee_saved.includes_fp,
        includes_link: rules.callee_saved.includes_link,
    };

    // ⑥ clobbers：可分配寄存器 − callee-saved − pinned（调用方要假设被破坏的部分）。
    let cs_idx: Vec<u32> = callee_saved.regs.iter().map(|r| r.index).collect();
    let clobbers: Vec<RegRef> = target
        .allocatable()
        .into_iter()
        .filter(|i| !cs_idx.contains(i) && !target.pinned(*i))
        .map(|i| {
            RegRef::new(
                i,
                target.reg_class_name(i),
                target.reg_name(i).unwrap_or_else(|| format!("p{i}")),
            )
        })
        .collect();

    // ⑦ 变参区域。
    let va_area = if sig.variadic {
        match rules.hidden.va_list {
            crate::rules::VaListKind::None => {
                return Err(AbiError::Unsupported {
                    conv: rules.name.clone(),
                    what: "签名是变参，但约定没有声明 hidden.va_list（变参形态）".into(),
                });
            }
            kind => Some(VaArea {
                kind,
                size: rules.hidden.va_list_size,
                align: rules.hidden.va_list_align,
                stack_only: rules.variadic_stack_only,
            }),
        }
    } else {
        None
    };

    // ⑧ 能力核对：约定要求的能力，ISA 必须声明（缺口在这里报，而不是生成期才炸）。
    if rules.callee_saved.mechanism == crate::plan::CalleeSaveMechanism::Push
        && target.cap(Capability::GprMov).is_none()
    {
        return Err(AbiError::CapabilityGap {
            conv: rules.name.clone(),
            cap: Capability::GprMov.name().into(),
            bits: 64,
        });
    }
    if matches!(ret, RetLoc::Indirect { .. }) && target.cap(Capability::FrameAddr).is_none() {
        // sret 缓冲需要帧内地址（不强制：调用方也可能直接在寄存器里给出指针）。
        // 这里只做"两者都没有"的兜底判断。
        if hidden.sret.is_none() {
            return Err(AbiError::CapabilityGap {
                conv: rules.name.clone(),
                cap: Capability::FrameAddr.name().into(),
                bits: 64,
            });
        }
    }

    let callee_pop_bytes =
        AbiPlan::resolve_callee_pop(rules.callee_pop, stack_args_bytes + rules.shadow_bytes);

    Ok(AbiPlan {
        conv: rules.name.clone(),
        variadic: sig.variadic,
        args,
        ret,
        stack: layout,
        callee_saved,
        hidden,
        clobbers,
        callee_pop_bytes,
        va_area,
        widen_to_bits: rules.extensions.widen_to_bits,
        note: rules.note.clone(),
    })
}

/// 分类的**方向**：参数位 / 返回位（两者的规则可以不同，见 `AbiRules::ret_classify`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassDir {
    Param,
    Ret,
}

/// 分类：返回位用 `ret_classify`（**非空即独占**，不再落回参数位规则——返回寄存器与
/// 参数寄存器不是同一批，混用会把返回值放进参数池）；规则顺序即优先级，未命中走 `fallback`。
fn classify(
    rules: &AbiRules,
    _target: &dyn AbiTarget,
    ty: &TyView,
    _variadic: bool,
    hooks: Option<&dyn AbiHooks>,
    dir: ClassDir,
) -> Result<ClassAction, AbiError> {
    if let Some(h) = hooks
        && let Some(a) = h.classify(rules, ty, dir)
    {
        return Ok(a);
    }
    let list = if dir == ClassDir::Ret && !rules.ret_classify.is_empty() {
        &rules.ret_classify
    } else {
        &rules.classify
    };
    for r in list {
        if r.when.matches(ty) {
            return Ok(r.do_.clone());
        }
    }
    Ok(rules.fallback.clone())
}

/// 把一个参数落到寄存器/栈/间接。
fn place_arg(
    pools: &mut Pools<'_>,
    stack: &mut StackAlloc,
    byval: &mut StackAlloc,
    rules: &AbiRules,
    ty: &TyView,
    action: &ClassAction,
    skip_used: &[u32],
) -> Result<Placement, AbiError> {
    match action {
        ClassAction::Ignore => Ok(Placement::Ignore),
        ClassAction::Stack { align } => Ok(stack_place(
            stack,
            rules,
            ty,
            &ClassAction::Stack { align: *align },
        )),
        ClassAction::Direct { pool, slots } => {
            let n = slots.resolve(ty).ok_or_else(|| AbiError::Unsupported {
                conv: rules.name.clone(),
                what: format!(
                    "参数 `{}` 不适用槽数规则 `{slots:?}`（`hfa` 只对同质浮点聚合成立）",
                    crate::plan::ty_text(ty)
                ),
            })?;
            match n {
                0 => Ok(Placement::Ignore),
                1 => match pools.take(pool, skip_used)? {
                    Some(reg) => Ok(Placement::Reg {
                        reg,
                        ext: Extension::None,
                        purpose: Purpose::Normal,
                    }),
                    None => Ok(stack_place(stack, rules, ty, action)),
                },
                // 连续 N 槽：不够就整块走栈（`take_n` 保证不会"取一半"）。
                n => match pools.take_n(pool, n as usize, skip_used)? {
                    Some(mut regs) if regs.len() == 2 => Ok(Placement::RegPair {
                        lo: regs.remove(0),
                        hi: regs.remove(0),
                    }),
                    Some(regs) if regs.len() > 2 => Ok(Placement::RegGroup { regs }),
                    Some(_) => Ok(stack_place(stack, rules, ty, action)),
                    None => Ok(stack_place(stack, rules, ty, action)),
                },
            }
        }
        ClassAction::Pair { lo, hi } => {
            let l = pools.take(lo, skip_used)?;
            let h = pools.take(hi, skip_used)?;
            match (l, h) {
                (Some(l), Some(h)) => Ok(Placement::RegPair { lo: l, hi: h }),
                // A1 的简化：拆不开就整块走栈（AAPCS64 规范允许"部分在寄存器"，
                // 那需要按成员赋值的规则语言，属 A6）。
                _ => Ok(stack_place(stack, rules, ty, action)),
            }
        }
        ClassAction::Indirect { via } => match via {
            IndirectVia::HiddenSret => {
                let Some(pool) = &rules.hidden.sret_pool else {
                    return Err(AbiError::Unsupported {
                        conv: rules.name.clone(),
                        what: "参数需要 sret 指针但约定没有 hidden.sret_pool".into(),
                    });
                };
                pools.reserve_slot(pool, rules.hidden.sret_slot);
                let regs = pools.get(pool)?.clone();
                let rr = regs
                    .get(rules.hidden.sret_slot as usize)
                    .cloned()
                    .ok_or_else(|| AbiError::Unsupported {
                        conv: rules.name.clone(),
                        what: format!("hidden.sret_pool `{pool}` 槽位不足"),
                    })?;
                Ok(Placement::Indirect {
                    ptr: Some(rr),
                    at: None,
                    on_stack: false,
                })
            }
            IndirectVia::CallerStackCopy => {
                // 调用方在自己栈上做副本；指针按 int 池传（池空则指针也在栈上）。
                // 副本落在**独立的 byval 临时区**（不是传出参数区，见 `StackLayout`）。
                let copy_off = byval.alloc(ty.size, ty.align, rules.stack.slot_bytes);
                let pool = rules.int_pool.clone();
                match pools.take(&pool, skip_used)? {
                    Some(ptr) => Ok(Placement::Indirect {
                        ptr: Some(ptr),
                        at: Some(copy_off as i32),
                        on_stack: true,
                    }),
                    None => Ok(Placement::Indirect {
                        ptr: None,
                        at: Some(copy_off as i32),
                        on_stack: true,
                    }),
                }
            }
        },
    }
}

/// 走栈的落点（含对齐与 `byval` 语义的体积计算）。
fn stack_place(
    stack: &mut StackAlloc,
    rules: &AbiRules,
    ty: &TyView,
    action: &ClassAction,
) -> Placement {
    let align = match action {
        ClassAction::Stack { align: Some(a), .. } => (*a).max(1),
        _ => ty.align.max(1),
    };
    let off = stack.alloc(ty.size.max(1), align, rules.stack.slot_bytes);
    // **被调方视角**的栈偏移 = `first_offset_slots × slot`（返回地址 + 被调方保存的帧指针）
    // **+ shadow**（调用方在返回地址之上预留的 scratch 区）`+` 该参数在参数区里的位置。
    //
    // 为什么要加 shadow：调用方把第 k 个栈参数写在 `[sp + shadow + k×slot]`（见
    // `Placement::Stack` 与 `CallLayout::caller_offset`），而**被调方**的 `sp` 比调用点的
    // `sp` 低一个返回地址、再低一个保存的帧指针——所以被调方读同一个实参时的偏移
    // 正好是"调用方偏移 + first_offset_slots×slot"。不加 shadow 会读到 shadow 区里的垃圾
    // （实测 win64 第 5 个参数：布局给 16，现有发射算式给 48 —— 差的就是这 32 字节）。
    Placement::Stack {
        offset: (off + rules.shadow_bytes) as i32,
        size: ty.size.max(1) as u16,
        align: align as u16,
    }
}

/// 声明了 `sret` 的形参：在约定声明的 hidden sret 槽里取指针（x86 RCX/RDI、AAPCS64 x8、
/// riscv a0），并记进 [`HiddenSlots::sret`]。
fn sret_slot(
    pools: &mut Pools<'_>,
    hidden: &mut HiddenSlots,
    rules: &AbiRules,
    skip_used: &mut Vec<u32>,
) -> Result<RegRef, AbiError> {
    let pool = rules
        .hidden
        .sret_pool
        .clone()
        .ok_or_else(|| AbiError::Unsupported {
            conv: rules.name.clone(),
            what: "形参声明了 `sret`，但约定没有声明 hidden.sret_pool".into(),
        })?;
    pools.reserve_slot(&pool, rules.hidden.sret_slot);
    let regs = pools.get(&pool)?.clone();
    let slot = rules.hidden.sret_slot as usize;
    let rr = regs
        .get(slot)
        .cloned()
        .ok_or_else(|| AbiError::Unsupported {
            conv: rules.name.clone(),
            what: format!("hidden.sret_pool `{pool}` 没有第 {slot} 个槽"),
        })?;
    if !skip_used.contains(&rr.index) {
        skip_used.push(rr.index);
    }
    hidden.sret = Some(rr.clone());
    Ok(rr)
}

/// 声明了 `byval(N)` 的形参：调用方栈上 N 字节副本 + 指针（指针按 int 池传）。
fn byval_place(
    pools: &mut Pools<'_>,
    byval: &mut StackAlloc,
    rules: &AbiRules,
    ty: &TyView,
    size: u32,
    skip_used: &[u32],
) -> Result<Placement, AbiError> {
    let n = size.max(1);
    let off = byval.alloc(n, ty.align.max(1), rules.stack.slot_bytes);
    let pool = rules.int_pool.clone();
    Ok(match pools.take(&pool, skip_used)? {
        Some(ptr) => Placement::Indirect {
            ptr: Some(ptr),
            at: Some(off as i32),
            on_stack: true,
        },
        None => Placement::Indirect {
            ptr: None,
            at: Some(off as i32),
            on_stack: true,
        },
    })
}

/// 把**声明属性**折进落点：符号扩展写进 `ext`、声明的对齐写进栈落点。
///
/// 只对"能承载它"的落点生效：`Reg`/`RegPair` 带 `ext`，`Stack` 带 `align`。
/// 其余落点（`Indirect`/`Ignore`）忽略这两个属性——不假装它们生效。
fn apply_decl_attrs(p: Placement, _ty: &TyView, attrs: &DeclAttrs, _rules: &AbiRules) -> Placement {
    let ext = attrs.extension();
    let align = attrs.declared_align();
    match p {
        Placement::Reg { reg, purpose, .. } => Placement::Reg { reg, ext, purpose },
        Placement::RegPair { lo, hi } => Placement::RegPair { lo, hi },
        Placement::RegGroup { regs } => Placement::RegGroup { regs },
        Placement::Stack {
            offset,
            size,
            align: a,
        } => Placement::Stack {
            offset,
            size,
            align: align.map_or(a, |d| d.max(a as u32) as u16),
        },
        other => other,
    }
}
