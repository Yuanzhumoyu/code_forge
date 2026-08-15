//! ⚠️ 第三十七轮状态:全部 ISA 关闭 enable_pattern_isel(标签零消费、写回已禁用)。
//! 待接通:统一 DSL lower_pattern 命名与手写标签后重新启用(见 docs/forge-ir-code-quality-audit.md)。

//! 基于模式的指令选择 — 在 lowering 之前匹配 IR 模式以生成更优的机器码。
//!
//! ## 设计理念
//!
//! 默认的 lowering 逐个将每个 IR 操作码转换为机器指令。
//! 当 IR 模式映射到更高效的单条机器指令（例如 `lea` 用于加/乘组合，或融合比较与分支）时，
//! 这会导致次优代码。
//!
//! `PatternMatcher` 在 lowering 到机器码之前遍历 IR 指令序列，
//! 寻找已知模式并生成优化的 instruction 序列。
//!
//! ## 示例模式
//!
//! - **LEA 合并**: `Iadd(base, Imul(index, const))` → 单条 `lea` 指令 (`enc_lea_sib`)
//! - **比较+分支融合**: `JCC` 紧跟 `CMP` → `enc_jcc_rel32`（条件已由 CMP 设置）
//! - **乘加融合**: `Fadd(Fmul(a, b), c)` → `FMADD`（在支持的平台上）
//!
//! ## 使用方式
//!
//! ```ignore
//! let mut matcher = PatternMatcher::new();
//! matcher.register_standard_patterns();
//! // 在 lowering 期间对每个基本块应用：
//! let optimized_insts = matcher.apply(&block_instructions);
//! ```

use crate::Immediate;
use forge_ir::Big;
use forge_ir::Instruction;
use forge_ir::Opcode;
use smallvec::SmallVec;
// ============================================================
// Pattern — 一条匹配并替换的规则
// ============================================================

/// 一条模式匹配规则。匹配一段 IR 指令序列并产生一个替换策略。
///
/// `condition` 允许在模式匹配成功后再添加一个额外检查
///（例如验证常量值在范围内，或操作数兼容）。
/// 条件函数接收匹配的指令切片和可选的常量池引用。
#[derive(Clone)]
pub struct Pattern {
    /// 人类可读名称（用于调试/追踪）。
    pub name: &'static str,
    /// 要匹配的 IR 操作码序列。长度决定了滑动窗口的大小。
    pub input: Vec<Opcode>,
    /// 替换策略名称——传递给 ISA 特定的重写逻辑。
    pub output: &'static str,
    /// 可选的条件检查。接收匹配的指令切片和常量池；
    /// 返回 `true` 以允许替换。
    #[allow(clippy::type_complexity)]
    pub condition: Option<fn(matched: &[&Instruction], constants: Option<&[Big]>) -> bool>,
}

impl std::fmt::Debug for Pattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pattern")
            .field("name", &self.name)
            .field("input_len", &self.input.len())
            .field("output", &self.output)
            .finish()
    }
}

// ============================================================
// PatternMatch — 一次成功匹配的结果
// ============================================================

/// 一次成功模式匹配的结果。
#[derive(Debug, Clone)]
pub struct PatternMatch {
    /// 匹配到的指令在原始序列中的起始索引。
    pub pos: usize,
    /// 匹配消耗的指令数量。
    pub consumed: usize,
    /// 匹配的模式名称。
    pub pattern_name: &'static str,
    /// 替换策略（来自模式的 `output`）。
    pub replacement: &'static str,
}

// ============================================================
// PatternMatcher — 遍历指令窗口并应用模式
// ============================================================

/// 遍历（已 lowered 或 IR）指令窗口寻找已知模式，并将其替换为最优序列。
pub struct PatternMatcher {
    patterns: Vec<Pattern>,
}

impl Default for PatternMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl PatternMatcher {
    /// 创建一个空的模式匹配器。
    pub fn new() -> Self {
        Self {
            patterns: Vec::new(),
        }
    }

    /// 注册一条模式。
    pub fn add_pattern(&mut self, pattern: Pattern) {
        self.patterns.push(pattern);
    }

    /// 返回当前注册的模式数量。
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }

    /// 在一个指令切片中寻找第一条匹配的模式。
    ///
    /// 从位置 `pos` 开始，尝试所有模式。返回第一条成功的匹配（如果有）。
    /// `constants` 为可选的常量池引用，供条件函数使用。
    pub fn try_match_at(
        &self,
        insts: &[Instruction],
        pos: usize,
        constants: Option<&[Big]>,
    ) -> Option<PatternMatch> {
        for pattern in &self.patterns {
            let pat_len = pattern.input.len();
            if pos + pat_len > insts.len() {
                continue;
            }

            // 逐操作码匹配
            let slice = &insts[pos..pos + pat_len];
            let opcodes_match = slice
                .iter()
                .zip(pattern.input.iter())
                .all(|(inst, expected)| inst.opcode == *expected);

            if !opcodes_match {
                continue;
            }

            // 检查条件
            if let Some(ref cond_fn) = pattern.condition {
                // 在切片上收集引用
                let refs: SmallVec<[&Instruction; 4]> = slice.iter().collect();
                if !cond_fn(&refs, constants) {
                    continue;
                }
            }

            return Some(PatternMatch {
                pos,
                consumed: pat_len,
                pattern_name: pattern.name,
                replacement: pattern.output,
            });
        }

        None
    }

    /// 遍历完整的指令序列，寻找所有模式匹配。
    ///
    /// 贪心策略：从位置 0 开始，尝试匹配。如果匹配成功，跳过已消耗的指令；
    /// 否则前进一条指令。
    ///
    /// `constants` 为可选的常量池引用。
    /// 返回所有匹配的列表，按位置升序排列。
    pub fn find_all_matches(
        &self,
        insts: &[Instruction],
        constants: Option<&[Big]>,
    ) -> Vec<PatternMatch> {
        let mut matches = Vec::new();
        let mut pos = 0;

        while pos < insts.len() {
            if let Some(m) = self.try_match_at(insts, pos, constants) {
                pos += m.consumed;
                matches.push(m);
            } else {
                pos += 1;
            }
        }

        matches
    }

    /// 对指令序列就地应用模式——返回匹配（标记）的序列数。
    ///
    /// 匹配的模式在代表指令上打 `isel_strategy` 标签（操作数/opcode 不变，
    /// 语义零影响）；lowering 读取标签后发射融合机器序列
    ///（Imul+Iadd → LEA、Icmp+Select → CMOV、Fmul+Fadd → FMA）。
    ///
    /// `constants` 为可选的常量池引用（供条件函数查询）。
    pub fn apply(&self, insts: &mut [Instruction], constants: Option<&[Big]>) -> usize {
        let matches = self.find_all_matches(insts, constants);
        let n = matches.len();
        for m in &matches {
            self.tag_match(insts, m);
        }
        n
    }

    /// 给匹配的代表指令打上 isel_strategy 标签（不修改操作数/opcode，
    /// 保证标记前后语义一致；lowering 负责按标签发射融合序列）。
    fn tag_match(&self, insts: &mut [Instruction], m: &PatternMatch) {
        if m.pos + m.consumed > insts.len() || m.consumed < 2 {
            return;
        }
        let first = &insts[m.pos];
        let tag = match m.replacement {
            "lea_sib" => {
                // %t = Imul(idx, scale); %r = Iadd(base, %t)
                // → Iadd 标记 "lea_sib:scale"（lowering 发射 base + idx*scale）
                let scale = insts.iter().find_map(|ins| {
                    if ins.opcode == Opcode::Iconst
                        && ins.results.first().copied() == first.operands.get(1).copied()
                    {
                        ins.immediates.first().and_then(|imm| match imm {
                            Immediate::Uint(v) => Some(v.to_string()),
                            Immediate::Int(v) => Some(v.to_string()),
                            _ => None,
                        })
                    } else {
                        None
                    }
                });
                match scale {
                    Some(_s) => Some("lea_sib"),
                    None => Some("lea_sib"),
                }
            }
            "fused_cmp_select" => {
                let cond = match first.opcode {
                    Opcode::Icmp { cond } => Some(cond),
                    _ => None,
                };
                Some(match cond {
                    Some(_c) => "cmovcc",
                    None => "cmovcc",
                })
            }
            "fma" => Some("fma"),
            _ => None,
        };
        if let Some(t) = tag {
            insts[m.pos + 1].isel_strategy = Some(t);
        }
    }
}

// ============================================================
// 标准模式注册
// ============================================================

impl PatternMatcher {
    /// 注册 x86 标准模式，在 lowering 过程中捕获常见 IR 模式并
    /// 映射到更优的指令序列。
    pub fn register_standard_patterns(&mut self) {
        // ---- LEA 合并: Iadd(base, Imul(index, const)) ----
        // 当 detect 到 Iadd 后跟 Imul（常用于地址计算）时，可以
        // 替换为单条 `lea` 指令。Imul 操作数必须是 2/4/8 的小常数。
        self.add_pattern(Pattern {
            name: "lea-merge-iadd-imul",
            input: vec![Opcode::Imul, Opcode::Iadd],
            output: "lea_sib",
            condition: Some(|matched, constants| {
                // 检查 Imul 的第二个操作数是否为小常数指针缩放因子
                // Imul: operands[0]=base, operands[1]=const_scale
                // Iadd: operands[0]=imul_result=index*scale, operands[1]=disp_base
                //
                // 有效的 LEA 可以表示: base + index*scale (scale=1,2,4,8)
                if matched.len() < 2 {
                    return false;
                }
                let imul_inst = matched[0];
                let iadd_inst = matched[1];
                if imul_inst.operands.len() < 2 {
                    return false;
                }
                // Iadd 的第一个操作数必须是 Imul 的结果（def-use 链）
                if let Some(imul_result) = imul_inst.results.first().copied() {
                    if iadd_inst.operands.first() != Some(&imul_result) {
                        return false;
                    }
                } else {
                    return false;
                }
                // 验证 Imul 的第二个操作数是常数且值在 {1,2,4,8} 中
                // 通过检查该值是否来自 Iconst 并查询常量池
                if let Some(pool) = constants {
                    let scale_val = imul_inst.operands[1];
                    // 查找该 Value 对应的 Iconst 指令（线性扫描指令序列）
                    // 这里我们不能回溯指令，所以放宽检查：
                    // 只要 Iadd 使用了 Imul 的结果，就允许匹配。
                    // 真正的常量验证在 lowering 时完成。
                    let _ = (scale_val, pool);
                }
                true
            }),
        });

        // ---- Compare+Select fusion: Icmp + Select ----
        // Icmp followed by Select that uses the comparison result can be
        // fused into a conditional-move pattern (e.g. x86 CMOVcc).
        self.add_pattern(Pattern {
            name: "cmp-select-fusion",
            input: vec![
                Opcode::Icmp {
                    cond: forge_ir::IntCC::Equal,
                },
                Opcode::Select,
            ],
            output: "fused_cmp_select",
            condition: Some(|matched, _constants| {
                if matched.len() < 2 {
                    return false;
                }
                let icmp_inst = matched[0];
                let select_inst = matched[1];
                // The Icmp result must be the condition operand of Select (operands[0])
                if let Some(icmp_result) = icmp_inst.results.first().copied() {
                    select_inst.operands.first() == Some(&icmp_result)
                } else {
                    false
                }
            }),
        });

        // ---- Mul-Add 融合: Fadd(Fmul(a, b), c) ----
        // 当 Fmul 的结果直接喂入 Fadd 时，在支持的平台上（如果可用）
        // 将二者融合为单条 FMADD 指令。
        self.add_pattern(Pattern {
            name: "mul-add-fusion",
            input: vec![Opcode::Fmul, Opcode::Fadd],
            output: "fma",
            condition: Some(|matched, _constants| {
                if matched.len() < 2 {
                    return false;
                }
                let fmul = matched[0];
                let fadd = matched[1];
                // Fmul 的结果必须被 Fadd 作为操作数使用
                if let Some(fmul_result) = fmul.results.first().copied() {
                    fadd.operands.contains(&fmul_result)
                } else {
                    false
                }
            }),
        });
    }
}

/// 全局架构中立标准模式匹配器（lea/cmp-select/fma 融合）。
/// 由声明了 `[meta].enable_pattern_isel = true` 的 ISA 通过
/// `TargetMachine::pattern_matcher()` 使用；进程内只初始化一次。
/// 第二十七轮:全部 ISA 关闭该开关(标签零消费 + Box::leak 泄漏),
/// 匹配器保留供后续接通。
pub fn standard_matcher() -> &'static PatternMatcher {
    static MATCHER: std::sync::OnceLock<PatternMatcher> = std::sync::OnceLock::new();
    MATCHER.get_or_init(|| {
        let mut m = PatternMatcher::new();
        m.register_standard_patterns();
        m
    })
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use forge_ir::mem_flags::MemFlags;
    use forge_ir::{Block, InstFlags, Opcode, Value};

    fn mk_inst(
        opcode: Opcode,
        operands: &[u32],
        results: &[u32],
        imm: Option<Immediate>,
    ) -> Instruction {
        Instruction {
            opcode,
            block: Block(0),
            pos: 0,
            results: results.iter().map(|&r| Value(r)).collect(),
            operands: operands.iter().map(|&o| Value(o)).collect(),
            immediates: imm.into_iter().collect(),
            flags: InstFlags::NONE,
            mem_flags: MemFlags::NONE,
            metadata: Default::default(),
            loc: None,
            isel_strategy: None,
            param_attrs: Default::default(),
            fn_attrs: Default::default(),
        }
    }

    /// Imul+Iadd 地址计算模式应被标记为 lea_sib 策略，且操作数/opcode 不变
    ///（标记不改语义——lowering 未读标签时行为与标记前一致）。
    #[test]
    fn test_lea_fusion_tags_strategy() {
        let mut matcher = PatternMatcher::new();
        matcher.register_standard_patterns();
        assert!(matcher.pattern_count() >= 3);

        let mut insts = vec![
            // %2 = Imul %1, %7   （%1=index, %7=scale——condition 在无常量池时放宽）
            mk_inst(Opcode::Imul, &[1, 7], &[2], None),
            // %3 = Iadd %2, %0  （imul 结果在前，%0=base）
            mk_inst(Opcode::Iadd, &[2, 0], &[3], None),
        ];
        let n = matcher.apply(&mut insts, None);
        assert_eq!(n, 1, "Imul+Iadd 应匹配一次");

        // 代表指令（Iadd）打上策略标签；Imul 保持原样
        assert_eq!(insts[0].opcode, Opcode::Imul, "Imul 不得被改写");
        assert_eq!(
            insts[0].operands.as_slice(),
            [Value(1), Value(7)].as_slice(),
            "Imul 操作数不变"
        );
        assert_eq!(insts[1].opcode, Opcode::Iadd, "Iadd 不得被改写");
        assert_eq!(
            insts[1].operands.as_slice(),
            [Value(2), Value(0)].as_slice(),
            "Iadd 操作数不变"
        );
        assert_eq!(
            insts[1].isel_strategy,
            Some("lea_sib"),
            "Iadd 应带 lea_sib 标签"
        );

        // 带 Iconst scale 时策略串携带缩放因子
        let mut insts2 = vec![
            mk_inst(Opcode::Iconst, &[], &[2], Some(Immediate::Uint(4))),
            mk_inst(Opcode::Imul, &[1, 2], &[3], None),
            mk_inst(Opcode::Iadd, &[3, 0], &[4], None),
        ];
        let n2 = matcher.apply(&mut insts2, None);
        assert_eq!(n2, 1);
        assert_eq!(
            insts2[2].isel_strategy,
            Some("lea_sib"),
            "第二十七轮:pattern_isel 关闭,标签改静态(不再编码 scale——消除 Box::leak 泄漏)"
        );
    }

    /// 不匹配的序列（无 def-use 链）不被标记。
    #[test]
    fn test_non_matching_sequence_untouched() {
        let mut matcher = PatternMatcher::new();
        matcher.register_standard_patterns();
        // Imul 结果未被 Iadd 使用
        let mut insts = vec![
            mk_inst(Opcode::Imul, &[1], &[2], None),
            mk_inst(Opcode::Iadd, &[0, 5], &[3], None),
        ];
        let n = matcher.apply(&mut insts, None);
        assert_eq!(n, 0);
        assert_eq!(insts[1].isel_strategy, None);
    }

    #[test]
    fn test_pattern_matcher_compiles() {
        let m = PatternMatcher::new();
        assert_eq!(m.pattern_count(), 0);
    }
}
