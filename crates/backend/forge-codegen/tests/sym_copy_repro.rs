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
int = ["r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r11"]
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
    // 守卫：本用例必须**真的**溢出过（否则它不再覆盖"缺溢出模板"那类缺陷）。
    let loads = lines.iter().filter(|l| l.starts_with("ld ")).count();
    let stores = lines.iter().filter(|l| l.starts_with("st ")).count();
    assert!(
        loads >= 2 && stores >= 2,
        "回归形状变了：n={n} 的产物里没有溢出搬运（ld={loads}, st={stores}）——\
         本用例的价值就在「寄存器压力 ⇒ 溢出」这条路径。\n{lines:#?}"
    );

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
