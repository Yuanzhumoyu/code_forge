# 通用汇编器/解码器设计方案（v2 — 架构无关重写）

> 状态：**已落地（v13）**。本提案的汇编器侧与解码器统一均已实现，见下方
> 「落地清单」。落地代码：
>
> - `crates/frontend/forge-dsl/src/assembler/`（token 模型 + 参考词法 + 参考操作数解析）
> - `crates/frontend/forge-dsl/src/v12/codegen/mod.rs`（token 化 assemble + 类型签名分发 +
>   前缀扫描声明表 + 定宽位 trie + VEX 内存形式）
> - `crates/frontend/forge-dsl/src/v12/model.rs`（多类型槽 `classes`、`[meta]` 语法键、
>   `[conventions.cond]`、`[conventions.prefix_scan]`、立即数约束 min/max/values/float）
> - `crates/frontend/forge-dsl/src/v12/codegen/integration.rs`（`parse_insts` 两遍布局：
>   标签/符号回填 + `.byte/.align/.global/.extern` 伪指令 + `Inst::Raw`）
> - `isa/demo_v12.toml` + `tests/demo_v12_tests.rs`（同助记符多宽度自动分发证明）
> - `isa/x86_v12.toml`（`gprx` 显式 classes；VEX_RR_MEMREF 内存形式；VMOVAPS_RM）
>
> **落地清单（与本提案的对应）**：
>
> 1. §3.1 操作数类型系统 → `OperandSlot.classes`（多类型集合）+ 槽约束过滤
>    （`__reg_f`）；§3.3 的"同助记符多 form 按类型签名分发" → 生成期特异性
>    排序（`form_specificity`，窄约束优先）+ 类型签名去重（`type_signature`）。
> 2. §3.2 类型数据 → 生成 `__reg_cls`/`__imm`/`__label`/`__cond`/`__mem`
>    返回类型化值；失败**回滚 token 位置**（多形状回退语义）。
> 3. §3.3 标签/符号 → `__label` 符号引用 + `parse_insts` 两遍布局（Block 索引
>    契约，与 encode `use_label_at` 一致；未定义 → `UndefinedLabel`）。
> 4. §4 指令元数据 → `[meta]` 语法键（comment_char/label_suffix/mnemonic_case/
>    imm_prefix/directive_prefix）+ `[conventions.cond]` 条件码表。
> 5. 词法 → `assembler::lex`（logos 参考）+ 生成模块内 std-only 手写 `__lex`
>    （token 集合逐 token 一致；`-` 独立 token、`0x`/`0b`/float/char/字符串、
>    标识符含 `.`/`$`）。
> 6. 解码器统一 → 变长前缀扫描由 `[conventions.prefix_scan]` 声明表生成（缺省
>    x86 集）；定宽 decode 重写为**位级决策树**（常量位段路径 + 叶节点补集零
>    guard，LLVM DecoderEmitter 形态，歧义边生成期硬报错）；VEX **内存形式**
>    （mod≠3 + base/disp + SIB）encode/decode 双向支持。
> 7. 伪指令 → `Inst::Raw(Vec<u8>)` 变体 + `parse_insts` 展开 `.byte`/`.align`
>    （偏移跟踪，填充字节 `[emit.align_pad]` 可配置，缺省 0x00）/`.global`/
>    `.extern`（符号登记，无消费方）。
> 8. SIB **索引寻址** → `MemRef` 增 `index: Option<Reg>`/`scale: u8`；组装
>    `[base+index*scale±disp]`、编码 SIB（scale 位 + REX.X）、解码提取
>    （MemReg 形式拒绝 index，避免与 MemRefOp 同 opcode 误吞）；big-endian
>    定宽 decode/encode 已有 `meta.endian` 驱动（`from_be/to_be_bytes`）。
> 9. x86 助记符合并 → `movrr`/`mov64rr` 合并为 `mov`；**12 条算术/存储宽度
>    变体（add32/sub32/imul32/xor32/and32/cmp32/test32/rol32/ror32/neg32/
>    mov_mem32/mov_sto32）合并为基助记符**——按操作数实际类型自动分发
>    （`add eax,ebx` → 32 位 01 D8、`add rax,rbx` → 48 01 D8）。配套**宽度
>    感知 lowering 候选解析**：`when = { eq = ["rs1_width", N] }` 规则优先
>    "单类且宽度匹配"的候选，多类（多态）槽保持声明序（槽类驱动
>    post-regalloc 的 Reg 视图 → opsize，是合并可行性的关键）。
>    `mov32` **保留**独立助记符（FIX32 语义承重）。
> 10. **8 位 mov**（opcode 8A，`mov al, bl` → 8A C3）——`gpr1b` 槽
> （byte_reg 强制 REX：spl/bpl/sil/dil）+ `MRR_BYTE` form；
> `mov` 全宽度（8/16/32/64）齐备。
> 11. **EVEX（AVX-512）最小集已落地**：`[reg.fpr32]`（ZMM0-31）、`fpr32`
> 槽、`EVEX_RRV` form（`evex` 数据键复用 VexSpec 的 map/pp/w/l，l=0/1/2
> → L'L=128/256/512）、`vaddps/vsubps/vmulps zmm` 指令（助记符与 VEX
> 版共享，按操作数类自动分发）。codegen：62 + P0/P1/P2 前缀发射
> （R'/X'/B'/R 与 V'/L'L 位）+ EVEX 解码 arm + 0x62 前缀键。
> `num_fp_regs` 与默认 FPR 类保持一致（优先 XMM/FPR(16)，ZMM 不改变
> 浮点寄存器计数）。后续增量：压缩位移（disp8×N）、opmask/broadcast、
> EVEX 内存形式。
> 12. **真重复合并**：`movrm`（MOV_RM8_R64 ≡ MOV_RM_R）、`movsxd_gpr`
> （MOVSXD_R_GPR ≡ MOVSXD_R_RM）合并进基助记符（编码字节相同）。
> 剩余多分支（cvtsi2sd/cvtss2si/_r64、movzx8/16、movsx8/16 的源宽度
> 分发）需多类槽 + `set_reg_field` 按 IR 宽度重建寄存器视图（更深层
> regalloc 升级），已文档化为后续迭代。
>
> 13. **512 位 EVEX 族**：`EVEX_RR`（2 操作数无源：vmovaps zmm,zmm）与
> `EVEX_RRV`/`EVEX_RR_MEMREF`（3 操作数/内存）齐备；`vaddpd/vsubpd/
>    vmulpd/vdivpd/vpaddd/vpsubd/vpaddq/vpsubq/vdivps zmm`（L'L=2）；
> VPADDQ/VPSUBQ 的 **evex_w=1**（EVEX 下 W 位区分 dword/qword，VEX
> 版 WIG 无碍）。编码 `62 + P0(R'X'B'R 00 mm) + P1(W vvvv 1 pp) +
>    P2(z L'L b V' aaa)`。
>
> 14. **opmask/broadcast/z（A2）**：`forge_ir::RegClass::KReg` 新类 +
> `[reg.kreg8]`（K0-K7）+ `kreg` 槽；EVEX P2 的 **aaa 位**（掩码操作数
> k 寄存器索引 & 7）与 **z 位**（零掩码，`evex_z` 键）encode/decode
> 双向；同助记符 3/4 操作数自动分发（`vaddps zmm0,zmm1,zmm2` vs
> `vaddps zmm0,zmm1,zmm2,k1`）；`vaddpsz`（z=1）独立助记符避签名去重。
>
> 15. **YMM/ZMM by-ref 传参（B1 被调方侧）**：`[abi.arg_class]` 的
> `strategy="by-ref" + limit`（位）→ `TargetABI::vector_by_ref_limit()`
> （阈值字节）；入口守卫按 ISA 声明放行宽向量参数（无声明仍拒绝）；
> `AllocResult.param_by_ref` 标记 >16 字节向量参数；`@move_args` 生成
> by-ref 收参——GPR 槽位是数据指针，**vmovups**（非对齐！vmovaps 对
> 未对齐指针 #GP）load 到 YMM/ZMM。JIT 测试 `v256_byref_param` 通过
> （`vmovaps zmm` 改 `vmovups` 是崩溃根因）。IR Call 宽向量实参的调用
> 方侧栈拷贝未落地——显式拒绝防静默截断。
> 16. **cvt 转换合并（C 族，指令级 opsize/rex_w 覆盖）**：`Instruction` 新增
> `opsize`/`rex_w` 覆盖键（优先于 form 级）；`cvtsi2sd/cvtsi2ss`（源宽度）
> 与 `cvtsd2si/cvttsd2si/cvtss2si`（目的宽度）的 32/64 位版本合并为单助记符
> ——`opsize = "s1"/"s0"`（源/目的操作数宽度驱动 REX.W）+ `rex_w = "auto"` +
> 源/目的槽改 `gprx`（多类），汇编器按实际操作数宽度自动分发
> （`cvtsi2sd xmm0, eax` → w=0、`cvtsi2sd xmm0, rax` → w=1）。decode 侧
> 多类槽宽度 guard 用 `__opsize == 8`（REX.W 判定）。6 条指令减为 3 条。
>
> 17. **movzx/movsx 合并（C 族）**：`movzx`/`movsx` 同助记符 + 源槽宽度分发
> （gpr1b → 0F B6/BE、gpr2 → 66 0F B7/BF）；新 `MRR_0F_NOOS` form（无
> opsize 一致性检查——dest/src 宽度不同合法）。**关键**：64 位 dest 的
> movzx/movsx 必须发 REX.W（`48 0F B6` 等），否则只写低 16/8 位保留高
> 位垃圾（JIT uextend_i16 根因）。lowering 宽度过滤改 role-aware：In/InOut
> 槽须恰为 w、Out 槽不参与（movzx dest gpr8 宽 8 ≠ w 合法；rol32 inout
> gpr32 == w 精确）。删除重复 MOVZX_B/MOVZX_W。
> 18. **YMM by-ref 调用方侧**：IR Call 宽向量实参的栈拷贝（VMOVUPS_MR store +
> LEA 指针）未落地——`arg_move_loop` 显式拒绝防静默截断（GPR/XMM 低位），
> 列为后续迭代（需 LowerCtx 栈槽 API + 帧布局 call 参数区）。
> 19. **Load/Store 32 位变体独立助记符（mini_c 除法根因修复）**：`MOV_R_MEM_32`
> （`mov_mem32`）与 `STORE_MEM_R_32`（`mov_sto32`）从同助记符拆分——32 位
> load/store 与 64 位变体同助记符（mov_mem/mov_sto）时，lowering 模板无宽度
> hint 按声明序选 64 位变体 → 32 位 store 写 8 字节覆盖相邻槽、32 位 load
> 读 8 字节含相邻垃圾 → Sdiv 除数高 32 位垃圾 → `idiv` 除错 → mini_c
> `v12_division`（100/7）got 0。Load/Store 规则 `when = { eq = ["rd"/
> "rs1_width", 32] }` 显式选 `mov_mem32`/`mov_sto32`。REX.W 由 dest 槽
> 宽度驱动（gpr32 → 0、gpr → 1），decode 靠 opsize（REX.W → __opsize）
> 区分。**mini_c v12 25/25 + JIT 184 全绿**。
> 20. **mini_c 递归调用修复**：AST 级内联对递归函数（`fib(n-1)`）无限展开（编译期
> 栈溢出）。Direct/V12 后端：`CodegenCtx.inlining` 栈含当前函数名（函数体内调用
> 自身 = 递归）+ `callee_is_self_calling`（递归函数任何调用点改真实 Call）——
> compiler.rs 预注册占位 Function 拿 FuncRef（`Module::replace_function` 后填
> 真实体），递归 Call 经 `builder.call(fib_ref)` 运行时递归（JIT 按 @N 符号
> 解析）。JIT 矩阵新增 `call_recursive_fib`/`call_recursive_fib_slot` 验证。
> HIR 后端（IrGraph 无 Call 原子）：递归明确报错（防栈溢出），后续补 Call 原子
> 后改真实调用。mini_c v12 28/28 + test_fibonacci（fib(5)=5）通过。
>
---

## 0. 为什么现有设计不合格（问题诊断）

1. **编码方式仍绑定 x86**：`v12/codegen` 里 `modrm = "rr"/"ext"/"rr_rev"/"rr_src2"`、
   VEX `C4`、`opcode_reg`（`+r`）、`force_disp_base`、REX 前缀扫描、SIB/disp 等，都是
   **x86 特有形状被硬编码进生成器分支**。换架构就得新增分支，`modrm`/`vex` 等键名本身
   就是 x86 词汇。
2. **寻址方式写死**：汇编器把内存参数硬编码成 `[base+disp]`（`__parse_mem_ref`），
   decode 把内存形式里 `MemReg`/`MemRefOp` 写成 `[基址+位移]`。但 MIPS/ARM/RISC-V 的
   内存形式未必是这样（可能是 `base+offset`、PC-relative、或 index 缩放），不该写死。
3. **汇编器参数类型太弱**：现有 `InterpolationType` 只有寄存器类/宽度集/寄存器名/立即数，
   且只用于“占位符模板”匹配，**没有把“合法参数”转成“带类型的数据”**交给后端。
4. **指令缺乏元数据**：指令没有“字节码地址”“读写副作用”“分支/调用/返回”等统一可查询
   数据，符号/标签解析时无法按操作数类型拿到其地址。
5. **解码器仍是“每指令 if 链”**：虽然已改成字节前缀树，但树的叶子上仍挂着 x86 专用的
   逐指令提取逻辑，不是“从数据生成、架构无关”的通用引擎。

**结论**：需要换一条“数据驱动”的道路——一切指令编码、操作数类型、寻址模式、指令元数据
都是**声明式数据**，由一份**通用引擎**（前缀树解码 + 类型驱动装配 + 编码器 + 后端分发）消费，
新架构只需补数据，不改引擎。

---

## 1. 设计目标

- **架构无关**：引擎里没有 `ModRM`/`REX`/`VEX`/`[reg+imm]` 等 x86 词；它们全部下沉到 ISA 数据。
- **一个前缀树同时服务解码与汇编**：编码的常量字节/位构成解码树；同一棵树按
  “助记符 → 操作数类型”反向遍历即装配器（“根据编码规则，各分支节点推断下一路径”）。
- **强操作数类型系统**：既**约束**（只能 gpr/fpr/vec、宽度、具体寄存器、立即数/寻址），
  又**产出**（寄存器索引、宽度、立即数、寻址信息、标签地址）——一切交给后端分发。
- **指令默认元数据**：字节码地址（标签定位）、读写/副作用、分支/调用/返回、谓词、编码信息。
- **寻址模式声明式**：`[reg+imm]` 只是 x86 的一个寻址模式；每种 ISA 各自声明，不写死。
- **新增架构零引擎改动**：只写 TOML + 一个小的“操作数读取/写入”适配（若该 ISA 有特殊尾部）。

---

## 2. 统一编码模型：`EncSpec`（一切指令的规范化描述）

每条指令 = 一个 `EncSpec`，由**声明式编码原子**构成，与架构无关：

```rust
enum EncAtom {
    /// 常量位/字节（含掩码，用于 +r/cond 等低位移折）：(bit, width, value)
    Const { bit: u32, width: u8, value: u64 },
    /// 从某操作数的**规范化字段**取值，放到若干 bit 位（可散布 pieces）。
    Field { field: FieldId, bit: u32, width: u8 },
    /// 一个寻址模式占位（后续按 ISA 的 AddressingMode 展开）。
    Addr { mode: AddrModeId },
    /// 尾部立即数字节数（字节流尾部）。
    TrailingImm { bytes: u8 },
    /// 固定长度的整字（定宽 ISA）。
    Word { width: u32, endian: Endian },
}

struct EncSpec {
    /// 常量前缀（解码树的 key）：`(bit, mask, value)` 序列。
    constant: Vec<(u32, u64, u64)>,     // (bit, mask, value)，沿字节/位展开
    atoms: Vec<EncAtom>,                 // 编码/解码的主体序列
    /// 尾长模型（变长 ISA：ModRM 类 + imm；定宽 ISA：Word）。
    layout: Layout,
    /// 操作数→字段映射（每个操作数占哪些 Field）。
    operand_fields: Vec<OperandFieldMap>,
}
```

关键：**编码形状完全由数据描述**。x86 的 ModRM 相当于一个名为 `modrm` 的寻址/寄存器组
`Layout` 变体，由 x86 的 TOML 声明；RISC-V 的 `opcode/funct3/funct7/rd/rs1/rs2` 就是散布
位字段，直接映射。引擎不关心“ModRM”“+r”“VEX”这些名字。

### 2.1 操作数字段（`Field`）

操作数在编码中占据的“字段”是规范化实体，与 ISA 无关：

```rust
enum FieldKind {
    Reg { class: RegRef, width: u16 },   // 寄存器（类 + 宽度；width=0 表示按值推导）
    Imm  { bytes: u8, signed: bool },
    AddrSegment { role: AddrRole },       // base/index/scale/disp 之一（见 §5）
    Label { width: u8, signed: bool },    // 符号/偏移（标签定位用）
    Cond { bits: u8 },
    Fixed(u64),                            // 常量字段（funct 等）
}
```

---

## 3. 操作数类型系统（汇编器“参数类型”的核心）

这是用户草稿 `assembler/InterpolationType` 的正规化扩展。每个操作数槽声明
**一类可接受类型 + 约束**；装配时把 token 解析成**带类型的数据**（`TypedOperand`）。

### 3.1 类型（`OperandType`，约束用）

```rust
/// 原子操作数类型——只有“寄存器”和“立即数”。其余一切（内存寻址、标签、条件）都是它们的
/// **组合或特例**，不作为独立类型出现。
enum OperandType {
    /// 一类寄存器（宽度即 `RegClass` payload，不再单独重复）。
    Reg(RegClass),
    /// 多类寄存器之一（多宽度视图）。
    RegAny(BTreeSet<RegClass>),
    /// 立即数（宽度 + 符号性 + 范围/枚举）。标签（带延迟符号）与条件码都是立即数。
    Imm(ImmSpec),
}

/// 内存寻址**没有类型/值/规范**——它只是 asm 模板语法（`[` `+` `]`）把若干**原子**操作数
/// 组合在一起。如 `[{{1:gpr}}+{{2:imm}}]` = 一个寄存器（base）+ 一个立即数（disp），
/// 各填编码的 base/disp 字段。没有任何 `Mem` 类型或值。
struct ImmSpec { bytes: u8, signed: bool, min: Option<i64>, max: Option<i64>,
                 values: Option<BTreeSet<i64>> }   // 枚举值
```

### 3.2 类型数据（合法参数解析后得到的值，交给后端）

```rust
enum TypedOperand {
    Reg { index: u32, class: RegClass },   // 宽度 = class payload
    /// 立即数；`sym = Some(name)` 表示是符号（标签）引用，布局期解析地址。
    Imm { value: i64, bytes: u8, signed: bool, sym: Option<String> },
}
```

**装配器职责**：按操作数槽的原子 `OperandType` 解析 token → `TypedOperand`（注册器/立即数）。
内存寻址是模板层对多个原子操作数（register + immediate）的分组，解析出的都是原子值，
分别填 base/disp 字段，无独立内存类型。解析失败回退到该指令下一“形态”。

### 3.3 类型约束与产出合一（例子）

```toml
asm = "add {{0:gpr<8>}}, {{1:gpr<8>}}"            # 两个 64 位 GPR
asm = "ldr {{0:gpr<8>}}, [{{1:gpr<8>}}+{{2:imm<4>}}]"  # 内存 = 模板把寄存器/立即数组合
asm = "jmp {{0:imm<4>}}"                            # 标签=带符号的立即数，布局期解析
asm = "bcond {{0:imm<1>}}, {{1:imm<4>}}"            # 条件码=小立即数
```

- `{{0:gpr<8>}}`：只接受 64 位 GPR；匹配则产出 `TypedOperand::Reg{index, class:gpr8}`。
- `[{{1:gpr}}+{{2:imm}}]`：两个原子操作数（base=寄存器、disp=立即数），由模板语法组合成内存；
  各填编码的 base/disp 字段。
- `{{0:imm<4>}}`：接受符号或数字；符号则 `sym=Some(name)`，**布局期**解析为该指令的地址。

---

## 4. 指令默认元数据（`InstructionMeta`）

每一条指令声明/携带以下元数据，装配与后端均可用：

```rust
struct InstructionMeta {
    /// 布局期分配字节偏移（0-based）；标签定义时记录“当前偏移”。
    byte_offset: Addr,
    /// 序列化后可查询：长度 / 起始地址。
    size_bytes: u8,
    /// 副作用：隐式读/写的物理寄存器（clobbers/reads/writes）。
    reads: Vec<PhysReg>,
    writes: Vec<PhysReg>,
    clobbers: Vec<PhysReg>,
    /// 控制流。
    is_branch: bool, is_call: bool, is_ret: bool, is_terminator: bool,
    /// 谓词（带条件执行的 ISA）。
    predicate: Option<PredRef>,
    /// 其它属性：is_move / has_side_effect / is_foldable / may_load / may_store。
    flags: MetaFlags,
    /// 编码信息（编码器/后端分发用）。
    enc: EncSpec,
}
```

**标签/符号解析**：汇编器维护一个符号表（label → 字节偏移）。指令中任一带 `Label`
类型的参数，在**布局完成**后按该类型解析：`s32` 表示其值是相对/绝对偏移，符号则替换成
“这条指令的地址”或“目标标签的地址”，交给编码器做重定位。这样“使用标签时，对应的指令
参数按类型获取该标签的地址信息”就自然成立。

**副作用**：`reads/writes/clobbers` 让寄存器分配在指令点避开隐式被改的寄存器（如 x86
`idiv` 改 RAX/RDX、`cqo` 改 RDX）；这是后端（regalloc）消费的元数据，与架构无关。

---

## 5. 寻址模式抽象（不写死 `[reg+imm]`）

内存/寻址不再硬编码成“`[base+disp]`”，而是**每种 ISA 声明自己的寻址模式**：

```rust
struct AddrModeSpec {
    name: &'static str,                       // "base_disp" / "base_index_scale_disp" / "pcrel"
    /// 语法的“模板”（分片），占位符引用 Base/Index/Scale/Disp 段。
    syntax: &'static str,                     // 如 "[{base}{+{index}*{scale}}{+{disp}}]"
    /// 各段用什么 FieldKind 编码（x86 用 ModRM/SIB；MIPS 用 imm+base；PC-rel 用即时偏移）。
    segs: Vec<AddrSeg>,
    /// 编码时如何把各段映射到 EncAtom（ISA-specific）。
    encode_segs: String,                      // 引用后端适配（见 §7）
    /// 解码时如何从 EncAtom 还原各段。
    decode_segs: String,
}

struct AddrSeg {
    kind: Base|Index|Scale|Disp,
    /// 段约束：base/index 必须是某寄存器类；disp 是某宽度的立即数/标签。
    ty: OperandType,
}
```

- x86：`base_disp` 段 = `base(gpr8) + disp(imm s8/s32)`，编码进 ModRM/SIB（x86 声明）。
- MIPS：`base + imm16`（内存立即数 + 基址寄存器）。
- ARM/RISC-V：`[base + imm]` 或 `base + index<shl>`。
- PC-relative：`disp(imm s32)`，无 base/index。

**新增 ISA 只需在 TOML 里声明新的 `AddrModeSpec`**，引擎不变。`[reg+imm]` 在 x86 里只是
一种 `AddrModeSpec`，不再是引擎的“特殊形状”。

### 5.1 寻址“编码适配”的两种方案（供确定）

寻址模式的结构（段、语法、约束）纯 TOML 即可；真正的**段→字节 编码/解码**差异很大
（x86 的 ModRM/SIB、MIPS 的 imm+base、PC-relative…）。这里给两种方案，选择后再定：

> **方案 A：纯 TOML 声明，引擎零代码**

- 用“声明式 bit 分配”表达段：每个段 = `(FieldKind, bitrange, mask)`，引擎按通用位放置/提取。
- 优点：引擎真正零适配，新增 ISA 完全靠数据；模型统一、易校验。
- 缺点：表达力受限——x86 ModRM 的“mod 位决定 reg/rm 还是 SIB+disp”这类**条件结构**，
  以及“寄存器的 REX 高位扩展”这些**跨段的隐式联动**，纯位表很难自然表达；需要引擎支持
  “条件/有依赖的位布局”，复杂度上升或需引入虚拟“宏段”。

> **方案 B：TOML 声明结构 + 每 ISA 一小段适配代码**

- TOML 描述段结构与约束；`AddrModeSpec` 只生成“数据骨架”，
  真正的 encode/decode 由每 ISA 注册一个小的 `AddrFn { encode(fields)->bytes, decode(bytes)->fields }`。
- 优点：表达力完整（x86/MIPS/任意特殊结构都能写），且寻址适配被**隔离**在 ISA 模块，
  引擎与其它 ISA 不受影响；“支持新架构”= 写 TOML + 写一个 ~30 行的寻址适配。
- 缺点：每 ISA 仍要写一点代码（比现状少很多——现状是引擎里上千行 x86 专用分支）。

**对比**：方案 A 更“纯数据”、更理想化，但要对条件位布局建模（复杂度/风险高）；
方案 B 务实、表达力完整，剩下的小适配最贴近 x86/MIPS 这类“结构特殊但不至于重写引擎”的
真实需求。**（建议方案 B 为主体；有需要可用方案 A 的“纯位段”覆盖简单 ISA。）**

---

## 6. 共享前缀树：解码 + 汇编

### 6.1 解码树（`DecodeTrie`）

- 以 `constant` 位/字节为 key（`(bit, mask, value)`，含 +r/cond 低位移折）。
- 节点 = 测试某位掩码；叶 = 指令（携带 `EncSpec` + 操作数字段映射 + 元数据）。
- **既处理变长又处理定长**：变长 ISA 的常量前缀做字节树；定长 ISA 的常量位做位树，
  同一棵树两种都可表达（对定宽，常量位就在 32 位字里，按位/字节展开）。
- 同一常量前缀的**多形态**（opcode family：寄存器 vs 内存、operand-size 变体）在一个叶里，
  按声明顺序尝试，直到某形态的操作数类型/尾部匹配。

### 6.2 汇编树（`AssembleTrie`）

由**同一份 `EncSpec`** 反向导出：

- 第一维：助记符（字符串树）。
- 第二维：该助记符的**形态**（按编码的常量部分 + 操作数类型签名去重）。
- 第三维：逐操作数按 `OperandType` 尝试解析；解析成功 → `TypedOperand` → 编码器。

“根据指令编码规则，通过各个分支节点推断下一路径”——`EncSpec.constant` 决定了形态；
匹配到形态后，`operand_fields` + `AddrModeSpec` 决定了每个操作数的类型与后缀编码。
装配即是解码树的反向遍历（从文本 → 常量位 → 编码输出）。

---

## 7. 后端分发（消费 `TypedOperand` + 元数据）

装配产物是一个 **`AssembledInst`**：

```rust
struct AssembledInst {
    inst_id: u32,
    operands: Vec<TypedOperand>,           // 强类型值
    meta: InstructionMeta,                 // 地址/副作用/控制流/编码
    byte_offset: Addr,                     // 布局期回填
}
```

编码器拿到 `EncSpec` + `TypedOperand[]` → 按 atoms/寻址模式发出字节；
若带标签/相对偏移 → 生成**重定位**（把符号解析延迟到布局结束）。
后端（JIT/汇编输出）拿到 `AssembledInst` 直接分发，不再做“猜测”。

---

## 8. ISA TOML 扩展（声明层）

在现有 `v12` 模型上扩展（`deny_unknown_fields` 继续严格）：

```toml
[[operand_slots]]
name = "gpr64"
kind = "reg"
type = "gpr"          # 更丰富的类型声明
width = 8
class = "gpr8"

[[operand_slots]]
name = "mem_base_disp"
kind = "mem"
addressing = "base_disp"          # 引用 §5 的一个 AddrModeSpec
base_class = "gpr8"
disp_width = 8

[[addressing_modes]]               # 新增：ISA 声明自己的寻址模式
name = "base_disp"
syntax = "[{base}{+{disp}}]"

[[instructions]]
name = "ADD"
enc = "R"                          # 引用 [[forms.enc]]（声明式编码原子）
operands = ["gpr64", "gpr64"]
meta = { writes = ["rd"], reads = ["rs1","rs2"], is_move = true }
asm = "add {{0:gpr<8>}}, {{1:gpr<8>}}"
```

`[[forms]]` 的 `modrm/vex/rex/opcode_reg` 等 x86 键**移除**，替换为声明式 `enc` 原子
（`constant`、`Field{rd/rs1/rs2}`、`Addr{...}`、`TrailingImm`...）。

---

## 9. 新增一个自定义架构（无引擎改动的证明）

以“MIPS-like”为例，仅靠 TOML：

```toml
[meta]
name = "mips32"; default_inst_width = 32; endian = "big"

[[addressing_modes]]
name = "off_base"                 # MIPS load/store：imm16(base)
syntax = "{disp}({base})"

[[forms]]
name = "R_3op"
enc = { constant = [{bit=26,width=6,value=0}],
        Field= { rd=15..20, rs1=21..25, rs2=16..20 } }

[[instructions]]
name = "ADD"
form = "R_3op"
opcode = 0x00; fields = { funct = 0x20 }
asm = "add {{0:gpr<8>}}, {{1:gpr<8>}}, {{2:gpr<8>}}"
meta = { writes=["rd"], reads=["rs1","rs2"] }
```

这条 ISA 的汇编/解码/编码无需改引擎；其寻址（`{disp}({base})`）与 x86 的 `[base+disp]`
完全不同，但都由 `AddrModeSpec` + 编码适配处理。

---

## 10. 重构路线（换血式，而非增删改查）

1. **新模型 + 引擎**：定义 `EncSpec`/`OperandType`/`AddrModeSpec`/`InstructionMeta`，
   实现通用 `DecodeTrie` + `AssembleTrie` + 编码器 + 后端分发（复用/扩展现有 `enc` 模块）。
2. **TOML 模型重构**：删除 x86 专属键（`modrm/vex/rex/opcode_reg/force_disp_base/...`），
   换成声明式 `enc` 原子；`operand_slots` 使用 `OperandType`；新增 `[[addressing_modes]]`
   与 `[[instructions.meta]]`。
3. **装配器重写**：类型驱动解析（`OperandType → TypedOperand`），经 `AssembleTrie` 反向遍历；
   符号/标签按 `Label.spec` 在布局期解析；内存按 `AddrModeSpec` 解析。
4. **解码器重写**：`DecodeTrie` 叶上不再是 x86 专用提取，而是通用 `Extractor`（按 `FieldKind`+ `AddrModeSpec.decode` 还原 `TypedOperand`）。
5. **后端**：`AssembledInst`（typed operands + metadata）供编码器/JIT 消费。
6. **验证**：x86 与 riscv 用新模型重写并对比现有 golden；再新增一个自定义 ISA 证明通用性。

---

## 11. 待确认的关键决策

1. **保留现有 `enc` 模块作为新引擎内核，还是重写**？（建议保留 `DecodeTrie`/`AssembleTrie`
   结构，重写其数据模型为 `EncSpec`/`OperandType`。）
2. `OperandType` 是否需要“用户在 asm 模板里写类型”还是“只由 TOML 操作数槽声明，
   模板引用槽名”（建议：模板引槽名，类型集中在槽声明，避免重复）。
3. 寻址模式的“编码适配”（`encode_segs`/`decode_segs`）最小形态：是纯 TOML 声明，
   还是允许每 ISA 一段小适配代码（推荐小适配，因 encode/decode 逻辑各 ISAA 差异大）。
4. 标签地址解析时机：布局结束统一回填（推荐），还是符号出现即解析。
5. 是否移除 `asm` 字符串模板中的“字面段”魔法，改为更结构化的 `syntax` 描述
   （推荐：保留模板字符串但用 `OperandType` 驱动解析，字面段仅作分隔/前缀）。

---

## 12. 落地清单（v13 实际实现）

> 本节记录 v13（v12 语法 + TargetMachine 集成 + 汇编器/解码器增强）的实际
> 落地成果，与上文"重构路线"的映射：不是换血式重写，而是在 v12 唯一语法
> 上逐步接入后端能力。

| # | 项 | 状态 | 位置 |
| --- | ---- | ------ | ------ |
| 1 | 定宽 encode/decode（位段 pieces 散布） | ✅ | `v12/codegen/mod.rs` gen_encode/gen_decode |
| 2 | 变长 x86 语义键（ModRM/REX/VEX/EVEX） | ✅ | 同上 gen_vlen_encode/decode |
| 3 | 类型化 Inst 字段 + Reg 枚举 | ✅ | gen_inst_enum |
| 4 | 自包含 asm（表驱动，首词=助记符） | ✅ | gen_assemble |
| 5 | `[abi]`/`[emit]`/`[spill]`/`[lowering]` 声明层 | ✅ | gen_abi/gen_frame_lowering/gen_lowering |
| 6 | `when` 谓词宽度分派（32/64） | ✅ | pred.rs + gen_lowering |
| 7 | Call/CallIndirect 专用 lowering（@N 符号 reloc） | ✅ | gen_call_lowering |
| 8 | GlobalAddr ABS8 重定位 | ✅ | gen_encoder |
| 9 | JIT 跨函数 Call（FuncRef 序符号注册） | ✅ | runtime/jit.rs |
| 10 | mini_c 递归（预注册占位 + replace_function） | ✅ | examples/mini_c |
| 11 | by-ref 宽向量 ABI（vmovups 收参） | ✅ | [abi.arg_class].by-ref |
| 12 | EVEX 族 + opmask（kreg） | ✅ | isa/x86_v12.toml |
| 13 | 512 位族（VPADDQ 等 evex_w=1） | ✅ | 同上 |
| 14 | cvt 合并（cvtsi2sd 单 form + opsize 覆盖） | ✅ | 同上 |
| 15 | movzx/movsx 合并（MRR_0F_NOOS） | ✅ | 同上 |
| 16 | mov_mem32/mov_sto32 独立助记符（32 位 load/store） | ✅ | 同上 |
| 17 | **B3**：demo_v12 接 TargetMachine（最小三件套模板） | ✅ | isa/demo_v12.toml + demo_v12_tm_tests |
| 18 | **B3**：`[abi].move_inst`/`ret_mov_inst`/`ret_regs`（角色解析） | ✅ | gen_call_lowering/@move_args |
| 19 | **B3**：`[emit].epilogue_label`（无 JMP 定宽 ISA 回退） | ✅ | gen_frame_lowering |
| 20 | **B2**：riscv64_v12 接 TargetMachine（SysV 子集 + W 变体） | ✅ | isa/riscv64_v12.toml + riscv64_v12_tm_tests |
| 21 | **B2**：`alloc_neg`/`min_frame_bytes`/`{frame_size_mN}` 占位符 | ✅ | [abi.frame]/[emit] |
| 22 | **B2**：RiscvRelocPatcher（JAL/B 型位段重排）+ 定宽 label fixup | ✅ | machine/reloc_patcher.rs |
| 23 | **B2**：QEMU system-mode 验证（sifive_test 退出码；const42 真执行） | ✅ | forge-tests exec/qemu.rs + isa/riscv64_v12 |
| 24 | **D**：汇编器增强（.equ/表达式/数据伪指令/.macro/行号） | ✅ | gen_assembler + asm_enhance_tests |
| 25 | **E**：decode 错误带部分匹配偏移 | ✅ | gen_decode + decoder_enhance_tests |
| 26 | **E**：`[meta].default_opsize` | ✅ | 同上 |
| 27 | **E**：大端变长 imm（imm_read_ts 按 endian） | ✅ | 同上 |
| 28 | **B2+**：riscv 矩阵恢复（值域过滤 16 位退出码 + 符号扩展） | ✅ | jit_matrix（expected_fits_u8 → ±32767；sign_extend_exit） |
| 29 | **B2+**：riscv 分支/跳转（BEQ+JAL 定宽 label fixup） | ✅ | gen_lowering terminator + [emit].epilogue_label=true |
| 30 | **B2+**：riscv 跨函数 Call/递归 QEMU 真执行（fib 等） | ✅ | exec_riscv64_module + RiscvRelocPatcher（占位 imm 清零修复） |
| 31 | **B2+**：`[abi].call_clobbers`/`reserved`（call 点 clobber 集 + 不可分配） | ✅ | gen_call_lowering/gen_reg_info |
| 32 | **B2+**：`[abi.frame].callee_saved_bytes_override`/`stack_slot_shift` | ✅ | frame_layout/lowering（spill 帧内 + 栈槽 fp 基准） |
| 33 | **B2+**：定宽 `@push_callee`/`@pop_callee`（SD/LD 帧槽） | ✅ | gen_emit_pseudo |
| 34 | **B2+**：regalloc callee-saved 优先（跨调用值安全） | ✅ | regalloc_bt pop_free |
| 35 | **B2+**：riscv 整数补全（除法/minmax/旋转/扩展/Abs/Select/空指针/Alloca/溢出/饱和） | ✅ | isa/riscv64_v12.toml lowering + CAPS |
| 36 | **B2+**：Zbb 编码修正（v10 迁移的 0x13/0x14 值错，LLVM 对齐规范） | ✅ | 同上（MIN funct7=0x05、CLZ 0x33 等） |
| 37 | **B2+**：`{iconst_hi20}` 双重右移修复（大常量 lui 字节规范） | ✅ | gen_lowering_attrs |
| 38 | **B2+**：riscv 标量浮点（Fconst 位模式 / Fcmp 条件 / Fptosi / Fptoui，f32+f64） | ✅ | isa/riscv64_v12.toml lowering + CAPS |
| 39 | **B2+**：浮点编码修正（LLVM 对齐）——FLE/FLT/FEQ funct3=0/1/2、fcvt D 变体 funct7=0x61、FCVT_S_D=0x20/FCVT_D_S=0x21 | ✅ | 同上（原值 funct3=6/4/5、funct7=0x60 错） |
| 40 | **B2+**：QEMU crt0 置 mstatus.FS=Dirty（`csrrs x0,mstatus,t0`，0x3002A073；原错码 0x3002F073 是 CSRRC 清位） | ✅ | exec/qemu.rs |
| 41 | **B2+**：Fcmp/Fconst 宽度谓词显式化（无宽度 `cond=N` 规则截胡 32 位分支）+ lo32 LUI 符号扩展截断（bit19=1 → slli32+srli32） | ✅ | isa/riscv64_v12.toml lowering |
| 42 | **C**：GlobalAddr（auipc+addi PC-relative；encoder 特判 AUIPC_GLOBAL/ADDI_GLOBAL imm<0 → "G{id}" reloc，patcher 按 opcode 0x17/0x13 分写 hi20/lo12） | ✅ | gen_encoder + RiscvRelocPatcher |
| 43 | **C**：QEMU module 打包布局数据段（globals init 字节 + "G{id}" 符号；**data_off 8 对齐**——AMO 自然对齐，代码长非 8 倍数时 misaligned_store 挂起） | ✅ | exec_riscv64_module + executor.exec_module 签名扩展 |
| 44 | **C**：原子族——AMOADD/AMOSWAP（funct5=0/1 修正；v10 迁移的 offset25 错把 aq/rl 包进 funct5 → offset27）+ AtomicRmw（imm0 分派 Xchg/Add/Sub=neg+amoadd） | ✅ | isa/riscv64_v12.toml + lowering |
| 45 | **C**：Cmpxchg **无分支** LR/SC 序列（掩码选择写入值；单线程 QEMU sc 不失败无需重试循环，模板层无函数内 label） | ✅ | lowering（lr.d/xor/sltu/slli/srai/xori/and/or/sc.d） |
| 46 | **D**：GetElementPtr（expand_geps 编译期展开为 mul+add，无需 TOML 规则）+ SaddSat/SsubSat（复用溢出检测 + 掩码饱和值选择） | ✅ | compiler.rs expand_geps + lowering |
| 47 | **D**：Iconst i64 完整 64 位路径（新增 {iconst_hi32_hi20/lo12}/{iconst_lo32_hi20/lo12}；原 lui+addi 只支持 32 位，大 i64 常量错编） | ✅ | gen_lowering_attrs + lowering |
| 48 | **F**：Clz/Ctz/Popcnt SWAR 软件序列（Hacker's Delight；QEMU rc 无 Zbb） | ✅ | lowering（lui/addi/slli/or 掩码 + 分治计数） |
| 49 | **F+**：Nop/Undef/Poison（xor 清零）+ Bitreverse（SWAR 分治 5 级 + 32 位交换） | ✅ | lowering |
| 50 | **G**：@push_callee **按需保存**（regalloc 的 callee_saved_to_save 只列实际分配的 s 系；SD/LD 从 13 个降到实际数；**class 通配**——i32 值以 GPR(4) 分配仍按 num 计入，否则 fib 递归 got -15） | ✅ | regalloc_bt + gen_emit_pseudo（运行时遍历） |

### 已知限制（诚实记录）

- riscv 矩阵 **126 passed / 60 skipped / 0 failed**（值域 ±32767 过滤大值；
  向量/Bswap/CallIndirect 未实现 → Skip）。
  执行链：`exec_riscv64(_module)` → 裸机 ELF + sifive_test（16 位退出码，
  `sign_extend_exit` 符号扩展回 i64）。
- **QEMU 11.1.0-rc2（v11.0.92）的 TCG 对特定 and/xori/and/or 寄存器序列有 bug**
  （Select 首版实测 s8=0；`-singlestep` 下结果不同）——**Select 用掩码法
  （slli/srai 全 0/全 1 掩码）规避**；Zbb 指令（MIN/CLZ 等）在该版本默认 CPU
  与显式属性（zbb=on/max/rva23u64）下均 illegal（LLVM 验证编码规范，
  QEMU rc TCG 未接）→ min/max/rot 用基础指令序列实现。
- riscv 跨函数 Call/递归（call_cross_function=42、call_recursive_fib=5、
  call_recursive_fib_slot=2、call_chain）QEMU 真执行全绿；`RelocPatcher`
  对 encoder 占位 imm（-(FuncRef+1)）先清零位段再写新值（OR 会恒跳 -1）。
- riscv 每函数 prologue **按需保存** callee-saved（阶段 G：regalloc 的
  `callee_saved_to_save` 只含实际分配的 s 系；帧仍 min_frame_bytes=104 保守）。
  跨调用存活值放 callee-saved 策略不变；递归（fib 等）真执行全绿。
- `call_clobbers` 全量列 caller-saved（ra/参数/临时 + 固定用途）——
  跨调用存活值在 call 点 spill；s 系跨调用安全（@push_callee）。
- clz/ctz/cpop 与 rol/ror 编码重叠（clz rd,rs = rol rd,rs,x0 别名）——
  decode 按声明序/叶优先返回 rol（roundtrip 语义歧义，测试不列入；
  编码正确性由 golden 覆盖）。
- **浮点 QEMU 调试实录**（供后续阶段参考）：QEMU 复位 mstatus.FS=Off →
  浮点指令非法且 TCG 表现为**挂起**（fault 循环），crt0 必须显式置 FS=Dirty；
  浮点比较/转换一律用 LLVM（clang+llvm-objdump）对照编码——funct3
  （FLE/FLT/FEQ=0/1/2，非 6/4/5）与 D 变体 funct7（S|1，fcvt.w.d=0x61）是
  两个易错点；低 32 位 LUI 立即数 bit19=1 时符号扩展污染高 32 位
  （-42.7 的 0x9999A → NaN），fconst 需 slli32+srli32 截断。
- **GlobalAddr/原子调试实录**：riscv psABI 的 %pcrel_hi/%pcrel_lo **同以
  auipc 指令地址为分母**——addi 的 site 比 auipc 大 4，patcher 需 +4 对齐
  （否则目标偏 4 字节）；AMO 的 funct5 位段是 **bits 31:27**（offset 27，
  v10 迁移写成 offset 25 会吞掉 aq/rl 位）；QEMU 对 AMO 未对齐地址报
  misaligned_store（挂起）→ 数据段必须 8 字节对齐；Cmpxchg 用无分支
  LR/SC（模板层无函数内 label；单线程 QEMU 下 sc 不失败）。
- **位操作调试实录**：TOML 里 LUI 字面量**必须写预移位值**（imm20<<12，
  如 0x55555 → 0x55555000）——imm20 piece 自带 shift=12，写裸 imm20 会被
  右移错编（0x55555 → 0x55000）；lo12 超出 ±2047 时 lui 需 +1 进位
  （0x0F0F0F0F 用 lui 0xF0F1 + addi -0xF1）；i64 常量需 64 位 Iconst 路径
  （lui/addi/slli/lui/addi/or，lo32 LUI bit19=1 符号扩展截断）。
- G/H 未开工（见任务清单：G = forge-rustc 向量、H = aarch64_v12 定宽 +
  demo_be 大端）。
- **forge-dsl 重构记录（2026-08）**：codegen 从 2 个巨型文件（mod.rs 4264 +
  integration.rs 4000）拆为 7 个职责模块（mod 927 / integration 712 /
  machine 1030 / frame 827 / vlen 2244 / lowering 1485 / asm 1141 行）；
  `group_names`/`parse_u64` 单点化（shared.rs）；GlobalAddr 从指令名特判
  → `global_reloc` 字段（abs8/pcrel_hi/pcrel_lo）；is_move 支持显式
  `move_inst` 声明；尾声跳转 `[emit].epilogue_jump_inst` 键；spill base
  缺省从 `[abi.frame].fp` 取。全部等价变换（golden 字节不变）。
