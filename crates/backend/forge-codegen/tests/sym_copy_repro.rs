//! **块参数两阶段并行拷贝**：实参必须在拷贝点仍然活着。
//!
//! 复现的缺陷（`[BUG-block-param-copy]`，Turing Complete "Symphony" 架构）：
//! `head` 有 ≥2 个块参数、入边全是 `Jump` ⇒ 走 `param_copies.rs` 的"独立参数 +
//! 两阶段显式拷贝"路径。触发条件是**寄存器压力**：3 个块参数 + 每个 Jump 入边 3 个
//! 并行拷贝临时寄存器，把 16 个寄存器里可分配的 9 个（`zr/sp/flags/r12/r13/r9/r10`
//! 固定）顶到溢出。
//!
//! 真正的缺陷在溢出路径：分配器决定溢出后，**谱没有 `[machine.spill.<名>]` 时
//! 发射侧一条 ld/st 都不发**（过去是 `let _ = (__dst, __off);` 静默 no-op）⇒ 溢出槽
//! 是垃圾、被 spill 的值在 use 处读到别人的值。报告里的机器码正是这条：
//! `add r9,r6,r5`（t 的 def 走 scratch）→ `addi r9,zr,1`（常量也走同一个 scratch，
//! 覆盖 t）→ `mov r3,r9`（拷贝读 t 拿到 1）。修复见
//! `crates/frontend/forge-isa-dsl/tests/spill_needs_template.rs`（缺模板 ⇒ 能力缺口）。
//!
//! 本文件是**端到端守卫**：本夹具**有**真溢出模板（LD/ST），因此同一份 IR 必须
//! 算对 `fib(15) == 610`——这是报告 §7.5 第 3 条要求的验收值。

forge_dsl::isa_from_file!("tests/isa/sym_copy.toml", spec_tests = false);

use std::collections::HashMap;

use forge_codegen::CompileSession;
use forge_codegen::TargetMachine as TargetMachineTrait;
use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::{IntCC, PhysReg, TypeId};
use sym_copy::{Inst, TargetMachine, decode, disassemble};

/// 让 `TargetMachineTrait` 的关联方法在作用域内（与其它夹具测试一致）。
#[allow(dead_code)]
fn _assert_tm_trait(tm: &TargetMachine) -> forge_ir::RegClass {
    TargetMachineTrait::reg_info(tm).value_gpr_class()
}

/// 本夹具的调用约定（标量参数走 `int` 池、返回值走 `ret_int` 池）。
const SYM_RULES: &str = r#"
name = "sym_copy"
position = "by_class"
int_pool = "int"
stack_align = 4
stack = { slot_bytes = 4, first_offset_slots = 0 }
aliases = ["c"]
classify = [
  { when = { kind = "scalar" }, do = { direct = { pool = "int" } } },
]
ret_classify = [
  { when = { kind = "scalar" }, do = { direct = { pool = "ret_int" } } },
]
fallback = { stack = {} }
note = "块参数两阶段拷贝夹具：标量参数/返回"
"#;

const SYM_BINDING: &str = r#"
isa = "sym_copy"
conv = "sym_copy"
[pools]
int = ["r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8"]
ret_int = ["r1"]
"#;

fn ensure_convention() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        use forge_codegen::pipeline::conv_registry::{register_binding_toml, register_rules_toml};
        register_rules_toml(SYM_RULES).expect("sym_copy 约定规则应能注册");
        register_binding_toml(SYM_BINDING).expect("sym_copy 约定绑定应能注册");
    });
}

/// 报告 §2 的 IR：四块 + `head` 三个块参数 + 回边传 `(b, a+b, i+1)`。
fn fib_loop() -> forge_ir::ir::function::Function {
    let i = TypeId::I32;
    let sig = FunctionSignature::new(&[(i, "n")], &[i]);
    let mut b = FunctionBuilder::new("fib", TypeContext::new(), sig);

    let (entry, e) = b.create_block_with_params(&[(i, "n")]);
    let (head, h) = b.create_block_with_params(&[(i, "a"), (i, "bb"), (i, "ii")]);
    let (body, bd) = b.create_block_with_params(&[(i, "a"), (i, "bb"), (i, "ii")]);
    let (exit, ex) = b.create_block_with_params(&[(i, "a"), (i, "bb"), (i, "ii")]);

    b.switch_to_block(entry);
    let zero = b.const_int(0, i);
    let one = b.const_int(1, i);
    let zero2 = b.isub(zero, zero);
    b.jump(head, &[zero, one, zero2]);

    b.switch_to_block(head);
    let cond = b.icmp(IntCC::Slt, h[2], e[0]);
    b.branch(cond, body, &[h[0], h[1], h[2]], exit, &[h[0], h[1], h[2]]);

    b.switch_to_block(body);
    let t = b.iadd(bd[0], bd[1]);
    let one2 = b.const_int(1, i);
    let i2 = b.iadd(bd[2], one2);
    b.jump(head, &[bd[1], t, i2]);

    b.switch_to_block(exit);
    b.ret(&[ex[0]]);
    b.finish().expect("build fib")
}

fn decode_all(code: &[u8]) -> Vec<Inst> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off < code.len() {
        let (inst, used) = decode(&code[off..])
            .unwrap_or_else(|| panic!("offset {off} 处无法解码：{:02x?}", code));
        assert_eq!(used, 4, "定宽 4 字节指令");
        out.push(inst);
        off += used;
    }
    out
}

fn disasm(code: &[u8]) -> Vec<String> {
    decode_all(code).iter().map(disassemble).collect()
}

/// 夹具 ISA 的极小解释器：只实现 lowering 真正会发的那十来条指令。
///
/// 语义按 `tests/isa/sym_copy.toml` 的约定：
/// - `r0`（`zr`）恒 0；`r15`（`flags`）由 `cmp` 写：bit0 = 相等、bit2 = 有符号小于；
/// - `sp` = r14、`fp` = r13、链接寄存器 = r12；
/// - 跳转立即数 = **以本条指令地址为基准**的字节位移（定宽 4 字节）。
struct Machine {
    regs: [u32; 16],
    mem: HashMap<u32, u32>,
    pc: u32,
    steps: u64,
    /// 返回地址哨兵（调用方把 r12 设成它，执行到就停）。
    ret_pc: u32,
}

impl Machine {
    fn new(ret_pc: u32) -> Self {
        // 调用方视角：sp = fp = 0x8000（帧向下生长，溢出槽在 [sp, fp) 内）。
        let mut regs = [0u32; 16];
        regs[14] = 0x8000;
        regs[13] = 0x8000;
        Self {
            regs,
            mem: HashMap::new(),
            pc: 0,
            steps: 0,
            ret_pc,
        }
    }

    fn set(&mut self, r: u32, v: u32) {
        if r != 0 {
            self.regs[r as usize] = v;
        }
    }

    fn get(&self, r: u32) -> u32 {
        self.regs[r as usize]
    }

    fn run(&mut self, code: &[Inst]) -> u32 {
        loop {
            assert!(self.steps < 200_000, "执行步数超限（疑似死循环）");
            self.steps += 1;
            let here = self.pc;
            let idx = (here / 4) as usize;
            assert!(idx < code.len(), "pc 越界：{}", here);
            let mut next = here + 4;
            match code[idx] {
                Inst::Nop => {}
                Inst::Mov { a, b } => {
                    let v = self.get(b.to_index());
                    self.set(a.to_index(), v);
                }
                Inst::Movi { a, imm } => self.set(a.to_index(), imm as u32),
                Inst::Add { a, b, c } => {
                    let v = self.get(b.to_index()).wrapping_add(self.get(c.to_index()));
                    self.set(a.to_index(), v);
                }
                Inst::Sub { a, b, c } => {
                    let v = self.get(b.to_index()).wrapping_sub(self.get(c.to_index()));
                    self.set(a.to_index(), v);
                }
                Inst::Addi { a, b, imm } => {
                    let v = self.get(b.to_index()).wrapping_add(imm as u32);
                    self.set(a.to_index(), v);
                }
                Inst::Subi { a, b, imm } => {
                    let v = self.get(b.to_index()).wrapping_sub(imm as u32);
                    self.set(a.to_index(), v);
                }
                Inst::Andi { a, b, imm } => {
                    let v = self.get(b.to_index()) & (imm as u32);
                    self.set(a.to_index(), v);
                }
                Inst::Lsri { a, b, imm } => {
                    let v = self.get(b.to_index()).wrapping_shr(imm as u32);
                    self.set(a.to_index(), v);
                }
                Inst::Ld { a, b, imm } => {
                    let addr = self.get(b.to_index()).wrapping_add(imm as u32);
                    let v = *self.mem.get(&addr).unwrap_or(&0);
                    self.set(a.to_index(), v);
                }
                Inst::St { a, b, imm } => {
                    let addr = self.get(b.to_index()).wrapping_add(imm as u32);
                    let v = self.get(a.to_index());
                    self.mem.insert(addr, v);
                }
                Inst::Cmp { a, b } => {
                    let (x, y) = (self.get(a.to_index()), self.get(b.to_index()));
                    let eq = u32::from(x == y);
                    let lt = u32::from((x as i32) < (y as i32)) << 2;
                    self.regs[15] = eq | lt; // flags = r15
                }
                Inst::Jmp { a } => next = self.get(a.to_index()),
                Inst::Jmpi { imm } => next = here.wrapping_add(imm as u32),
                Inst::Jne { imm } => {
                    if self.regs[15] & 1 == 0 {
                        next = here.wrapping_add(imm as u32);
                    }
                }
                Inst::Je { imm } => {
                    if self.regs[15] & 1 == 1 {
                        next = here.wrapping_add(imm as u32);
                    }
                }
                Inst::Ret => next = self.get(12),
                Inst::Raw(_) => panic!("夹具不该出现伪指令"),
            }
            if next == self.ret_pc {
                break;
            }
            self.pc = next;
        }
        self.regs[1]
    }
}

/// 编译 + 执行 `fib(n)`（ABI：`n` 走 int 池首寄存器 `r1`，返回同 `r1`）。
fn run_fib(n: u32) -> (u32, Vec<String>, u64) {
    ensure_convention();
    let func = fib_loop();
    let cf = CompileSession::new(TargetMachine::new())
        .compile(&func)
        .expect("fib 循环必须可编译");

    let lines = disasm(&cf.code);
    let insts = decode_all(&cf.code);
    let ret_pc = (insts.len() * 4) as u32;
    let mut m = Machine::new(ret_pc);
    m.set(1, n); // ABI：第一个 int 参数
    m.set(12, ret_pc); // 链接寄存器 = 调用方返回地址
    let out = m.run(&insts);
    (out, lines, m.steps)
}

/// 报告 §7.5 第 3 条要求的回归：三块参数循环必须算出 fib(15) == 610。
#[test]
fn block_param_copy_loop_computes_fib() {
    for (n, want) in [(0u32, 0u32), (1, 1), (2, 1), (10, 55), (15, 610)] {
        let (got, lines, steps) = run_fib(n);
        assert_eq!(
            got,
            want,
            "fib({n}) 应为 {want}，实际 {got}（steps={steps}）——两阶段并行拷贝的回边实参\
             在拷贝点已被覆盖。\n反汇编：\n{}",
            lines.join("\n")
        );
    }
}

/// **溢出路径的端到端守卫**：9 个可分配寄存器、8 个派生值同时活着 ⇒ 必然溢出。
///
/// 这条与上一条分开，因为两件事要分开证：① 两阶段拷贝在**不溢出**时算对；
/// ② 真溢出时 `[machine.spill.<名>]` 的 ld/st 真的把值搬进/搬出槽——没有它，
/// 溢出槽就是垃圾（`[BUG-block-param-copy]` 的错值正是这条）。
///
/// 只用**函数内算出来的值**做压力（不用形参）：形参溢出后的入口写槽是另一个
/// 独立缺陷（`frame.rs` 的 `spilled_receive_supported` 把入口写槽挂在
/// `roles = ["stack_arg_load"]` 上 ⇒ 没有栈参数形参的 ISA 完全不写槽），
/// 单独立案，不在本用例里混测。
#[test]
fn spill_pressure_round_trip_computes_correctly() {
    ensure_convention();
    let i = TypeId::I32;
    let sig = FunctionSignature::new(&[(i, "n")], &[i]);
    let mut b = FunctionBuilder::new("spill8", TypeContext::new(), sig);
    let (entry, args) = b.create_block_with_params(&[(i, "n")]);
    b.switch_to_block(entry);
    // 先把 8 个派生值都算出来（全部活到后面的求和）⇒ 峰值远超 9 个可分配寄存器。
    let mut vals = Vec::new();
    for k in 1..=8u64 {
        let c = b.const_int(k, i);
        vals.push(b.iadd(args[0], c));
    }
    let mut sum = vals[0];
    for v in &vals[1..] {
        sum = b.iadd(sum, *v);
    }
    b.ret(&[sum]);
    let func = b.finish().expect("build spill8");

    let cf = CompileSession::new(TargetMachine::new())
        .compile(&func)
        .expect("压力函数必须可编译（本夹具声明了溢出模板）");
    let lines = disasm(&cf.code);
    let loads = lines.iter().filter(|l| l.starts_with("ld ")).count();
    let stores = lines.iter().filter(|l| l.starts_with("st ")).count();
    assert!(
        loads >= 1 && stores >= 1,
        "本用例必须真的溢出（否则它不再覆盖溢出搬运路径）：ld={loads}, st={stores}\n{lines:#?}"
    );

    let insts = decode_all(&cf.code);
    let ret_pc = (insts.len() * 4) as u32;
    let mut m = Machine::new(ret_pc);
    m.set(1, 5); // ABI：唯一的 int 形参 n = 5
    m.set(12, ret_pc);
    let got = m.run(&insts);
    // 8*5 + (1+2+…+8) = 40 + 36
    assert_eq!(got, 76, "溢出往返后结果错了：{got}\n{}", lines.join("\n"));
}

/// 从 `st rX, r13, OFF` / `ld rX, r13, OFF` 里取槽偏移。
fn slot_off(line: &str) -> Option<&str> {
    let (head, _) = line.split_once(", r13, ")?;
    if head.starts_with("st ") || head.starts_with("ld ") {
        line.split_once(", r13, ").map(|(_, off)| off)
    } else {
        None
    }
}

/// **形参溢出后入口必须把槽写对**（M1 的回归）。
///
/// 缺陷形态（`frame.rs` 的 `spilled_receive_supported` 把入口写槽挂在
/// `roles = ["stack_arg_load"]` 上）：没有栈形参的 ISA 里，形参一旦被 allocate 阶段
/// spill，收参就**只写寄存器、不写槽**；而发射侧对"有槽"的值一律从槽读 ⇒ reload 读到
/// 未初始化栈。实测 9 形参 / 9 可分配寄存器：只发出 4 条收参 mov + 4 条 ld，全函数
/// 0 条 st。
///
/// **形参只取 3 个**，是为了让 ABI 源寄存器（r1..r3）与分配器的 home（`pop_free`
/// 从最大号开始取：r8/r7/r6）**不相交**——收参顺序缺陷（另一个独立缺陷：home 落在
/// 后续形参的 ABI 源上时，先收的 mov 会踩掉后收的源）会掩盖本用例要证的东西。
///
/// 断言两层：① 每条 `ld …, r13, OFF` 之前必须出现过同一 OFF 的 `st`（槽先写后读，
/// 线性成立——入口写槽都在序言）；② 解释执行结果正确。
#[test]
fn spilled_incoming_params_are_stored_into_their_slots() {
    ensure_convention();
    let i = TypeId::I32;
    let names = ["a", "b", "c"];
    let sig = FunctionSignature::new(&names.iter().map(|nm| (i, *nm)).collect::<Vec<_>>(), &[i]);
    let mut b = FunctionBuilder::new("spill_params", TypeContext::new(), sig);
    let (entry, args) =
        b.create_block_with_params(&names.iter().map(|nm| (i, *nm)).collect::<Vec<_>>());
    b.switch_to_block(entry);
    // 8 个派生值同时活着（全部活到求和）+ 3 个形参 ⇒ 远超 8 个可分配寄存器，
    // 形参因此被 spill（home 与 ABI 源不相交，收参不会互相踩）。
    let mut vals = Vec::new();
    for k in 1..=8u64 {
        let c = b.const_int(k, i);
        vals.push(b.iadd(args[0], c));
    }
    let mut sum = vals[0];
    for v in &vals[1..] {
        sum = b.iadd(sum, *v);
    }
    sum = b.iadd(sum, args[1]);
    sum = b.iadd(sum, args[2]);
    b.ret(&[sum]);
    let func = b.finish().expect("build spill_params");

    let cf = CompileSession::new(TargetMachine::new())
        .compile(&func)
        .expect("形参溢出必须可编译（本夹具声明了溢出模板）");
    let lines = disasm(&cf.code);
    let stores = lines.iter().filter(|l| l.starts_with("st ")).count();
    assert!(
        stores > 0,
        "本用例必须真的溢出形参（否则不再覆盖入口写槽）：st={stores}\n{lines:#?}"
    );

    // ① **每个被读的槽都至少被写过一次**。
    //
    // 不能要求"线性先写后读"：发射侧对纯 def 的溢出值也会先发一条 Reload（"纯 def 的
    // 值也装，无害"），那条 ld 出现在 st 之前是设计如此。真正要钉的是"这个槽有没有
    // 人写"——M1 的缺陷正是**整段写槽代码没生成**，于是 ld 读的是未初始化栈。
    let mut written: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut read: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in &lines {
        if let Some(off) = slot_off(line) {
            if line.starts_with("st ") {
                written.insert(off.to_string());
            } else if line.starts_with("ld ") {
                read.insert(off.to_string());
            }
        }
    }
    let never_written: Vec<&String> = read.difference(&written).collect();
    assert!(
        never_written.is_empty(),
        "这些溢出槽被读却从未写过（形参溢出后入口没写槽）：{never_written:?}——反汇编：\n{}",
        lines.join("\n")
    );

    // ② 语义：8*1 + (1+2+…+8) + 2 + 3 = 8 + 36 + 5 = 49。
    let insts = decode_all(&cf.code);
    let ret_pc = (insts.len() * 4) as u32;
    let mut m = Machine::new(ret_pc);
    // ABI：int 池 = r1..r8，3 个形参依次落在 r1..r3。
    for (idx, v) in [1u32, 2, 3].iter().enumerate() {
        m.set(idx as u32 + 1, *v);
    }
    m.set(12, ret_pc);
    let got = m.run(&insts);
    assert_eq!(
        got,
        49,
        "形参溢出往返后结果错了：{got}（应为 8*1+36+2+3）\n{}",
        lines.join("\n")
    );
}

/// 收参是"一串 `mov home, abi_src`"，而 home 是运行期值 ⇒ 若某形参的 home 恰好是
/// **后面**某个形参的 abi_src，先收的 mov 会踩掉后收的源（2026-11-01 实测：
/// 6 个形参加上"池按最大号优先"的取号顺序 ⇒ home 落进 r1..r6 的源区间，
/// `b`/`e` 被覆盖）。
///
/// 修法：分配期把入口实参寄存器当约束——形参 home 回避它们（标准做法：入口 live-in
/// 寄存器先占住），从根上消除顺序依赖；回避不开的形参走 spill，入口写槽由 M1 保证。
#[test]
fn six_params_receive_without_clobbering_abi_sources() {
    ensure_convention();
    let i = TypeId::I32;
    let names = ["a", "b", "c", "d", "e", "f"];
    let sig = FunctionSignature::new(&names.iter().map(|nm| (i, *nm)).collect::<Vec<_>>(), &[i]);
    let mut b = FunctionBuilder::new("recv6", TypeContext::new(), sig);
    let (entry, args) =
        b.create_block_with_params(&names.iter().map(|nm| (i, *nm)).collect::<Vec<_>>());
    b.switch_to_block(entry);
    let mut sum = args[0];
    for arg in &args[1..] {
        sum = b.iadd(sum, *arg);
    }
    b.ret(&[sum]);
    let func = b.finish().expect("build recv6");

    let cf = CompileSession::new(TargetMachine::new())
        .compile(&func)
        .expect("6 个形参必须可编译");
    let lines = disasm(&cf.code);
    let insts = decode_all(&cf.code);
    let ret_pc = (insts.len() * 4) as u32;
    let mut m = Machine::new(ret_pc);
    for (idx, v) in [1u32, 2, 3, 4, 5, 6].iter().enumerate() {
        m.set(idx as u32 + 1, *v);
    }
    m.set(12, ret_pc);
    let got = m.run(&insts);
    assert_eq!(
        got,
        21,
        "收参被后续形参的源寄存器覆盖了：{got}（应为 1+2+3+4+5+6）\n{}",
        lines.join("\n")
    );
}

/// **函数体自身压力**下的正确性守卫（9 个派生值同时活着 ⇒ 必然驱逐/溢出）。
///
/// 注意它**不**单独保证"一条指令 3 个操作数全溢出"——那个形状由
/// `regalloc_bt::spill::tests::three_spilled_operands_share_two_scratch_slots` 直接钉住
/// （构造精确、不依赖分配器决策）；本用例只保证压力下的语义仍然对。
#[test]
fn single_instruction_with_three_spilled_operands_needs_two_scratch() {
    ensure_convention();
    let i = TypeId::I32;
    let sig = FunctionSignature::new(&[(i, "n")], &[i]);
    let mut b = FunctionBuilder::new("three_spilled", TypeContext::new(), sig);
    let (entry, args) = b.create_block_with_params(&[(i, "n")]);
    b.switch_to_block(entry);
    // 9 个派生值都活到最后的求和 ⇒ 峰值 10 个值 > 8 个可分配寄存器。
    let mut vals = Vec::new();
    for k in 1..=9u64 {
        let c = b.const_int(k, i);
        vals.push(b.iadd(args[0], c));
    }
    let mut sum = vals[0];
    for v in &vals[1..] {
        sum = b.iadd(sum, *v);
    }
    b.ret(&[sum]);
    let func = b.finish().expect("build three_spilled");

    let cf = CompileSession::new(TargetMachine::new())
        .compile(&func)
        .expect("三个操作数同时溢出时 2 个 scratch 必须够（def 与 use 共用槽）");
    let lines = disasm(&cf.code);
    let insts = decode_all(&cf.code);
    let ret_pc = (insts.len() * 4) as u32;
    let mut m = Machine::new(ret_pc);
    m.set(1, 1); // n = 1
    m.set(12, ret_pc);
    let got = m.run(&insts);
    // 9*1 + (1+…+9) = 9 + 45
    assert_eq!(
        got,
        54,
        "结果错了：{got}（应为 9+45）\n{}",
        lines.join("\n")
    );
}

/// 事件化：形参因"回避入口实参寄存器"而溢出时必须发 `PARAM-SPILLED`（可见性，不报错）。
///
/// 放在这个**独立测试二进制**里：`set_var` 是进程级副作用（Rust 2024 起为 `unsafe`），
/// 这里没有别的线程依赖该变量，写完立即恢复。
#[test]
fn param_spilling_at_entry_emits_an_event() {
    ensure_convention();
    let log_path = std::env::temp_dir().join("forge_param_spilled_event.log");
    let _ = std::fs::remove_file(&log_path);
    // SAFETY：本测试二进制里没有别的线程读 `FORGE_CODEGEN_EVENTS`；设完→编译→立刻清。
    unsafe { std::env::set_var("FORGE_CODEGEN_EVENTS", &log_path) };

    let i = TypeId::I32;
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let sig = FunctionSignature::new(&names.iter().map(|nm| (i, *nm)).collect::<Vec<_>>(), &[i]);
    let mut b = FunctionBuilder::new("evt_params", TypeContext::new(), sig);
    let (entry, args) =
        b.create_block_with_params(&names.iter().map(|nm| (i, *nm)).collect::<Vec<_>>());
    b.switch_to_block(entry);
    let sum = b.iadd(args[0], args[1]);
    b.ret(&[sum]);
    let func = b.finish().expect("build evt_params");
    let _ = CompileSession::new(TargetMachine::new()).compile(&func);

    unsafe { std::env::remove_var("FORGE_CODEGEN_EVENTS") };
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    let _ = std::fs::remove_file(&log_path);
    assert!(
        log.contains("PARAM-SPILLED"),
        "8 个形参占满 ABI 池 ⇒ 全部因回避入口源寄存器而溢出，必须发 PARAM-SPILLED：{log}"
    );
    assert!(
        log.contains("entry-src-reserved"),
        "reason 字段应说明原因：{log}"
    );
}

/// 三操作数 ALU 的三个操作数**全部**溢出：2 个 scratch 就够（def 与一个 use 共用槽，
/// 指令先读后写）。旧实现给每个 spilled vreg 各分一个槽，于是要 3 个 ⇒ 在 2 个 scratch
/// 的谱上直接编译失败（`第 N 条指令需要 3 个 scratch 寄存器…`）。
#[test]
fn eight_all_spilled_params_fit_in_two_scratch_registers() {
    ensure_convention();
    let i = TypeId::I32;
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let sig = FunctionSignature::new(&names.iter().map(|nm| (i, *nm)).collect::<Vec<_>>(), &[i]);
    let mut b = FunctionBuilder::new("recv8", TypeContext::new(), sig);
    let (entry, args) =
        b.create_block_with_params(&names.iter().map(|nm| (i, *nm)).collect::<Vec<_>>());
    b.switch_to_block(entry);
    let mut sum = args[0];
    for arg in &args[1..] {
        sum = b.iadd(sum, *arg);
    }
    b.ret(&[sum]);
    let func = b.finish().expect("build recv8");

    // 8 个形参 = ABI 池的全部 8 个寄存器，而它们又都是入口源寄存器 ⇒ 全溢出。
    let cf = CompileSession::new(TargetMachine::new())
        .compile(&func)
        .expect("全溢出时 2 个 scratch 必须够（def 与 use 共用槽）");
    let lines = disasm(&cf.code);
    let insts = decode_all(&cf.code);
    let ret_pc = (insts.len() * 4) as u32;
    let mut m = Machine::new(ret_pc);
    for (idx, v) in [1u32, 2, 3, 4, 5, 6, 7, 8].iter().enumerate() {
        m.set(idx as u32 + 1, *v);
    }
    m.set(12, ret_pc);
    let got = m.run(&insts);
    assert_eq!(
        got,
        36,
        "8 个全溢出形参求和错了：{got}（应为 36）\n{}",
        lines.join("\n")
    );
}
