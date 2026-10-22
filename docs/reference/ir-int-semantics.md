# IR 整数与浮点的位宽语义（forge-ir）

> `[active]` 2026-10-18 起生效。**以代码与测试为准**：本文描述 forge-ir 的类型/运算语义，
> 是实现"任意位宽整数"与"任意浮点格式"的**唯一事实源**；与文档冲突时以 `forge-ir` 的
> verifier + 单测为准。

## 1. 为什么需要这一层

LLVM 的 `i3`/`i128` 与 Swift/GCC 的任意精度整数都遵循同一条分层：

```text
前端（rustc / 其它）──MIR→IR──► IR（只声明"这是 i<N> 的加法"）
                                   │
                                   ├─ legalize（目标无关：widen / split / libcall）
                                   └─ isel + ISA-DSL（只看目标机原生位宽）
```

如果**把拆分写在最上层**（例如在 forge-rustc 里手工把 128 位拆成两半），就会：
① 把目标相关知识塞进前端；② 非 2 的幂位宽（i3/i7）无法表达；③ 有原生宽运算的 ISA 用不上。
因此 IR 必须能**独立表达任意位宽**，拆分留给后端 legalize。

## 2. 整数：无符号位图 + 任意位宽

- **类型**：`TypeEntry::Int { bits: u32 }`。`bits == 0` 表示 `void`（既有约定），
  `bits == 1` 与内建 `bool` 是同一类型。类型空间上限是 `u32`（机制上无上限；
  实际编译预算由 `limb` 数决定，见 §5）。
- **值的含义**：`i<N>` 是**无符号位图**（低 `N` 位有效）。**符号性由 opcode 表达**——
  `Sdiv/Srem/Sshr/Sextend/Sitofp` 以及 `Icmp` 的 `IntCC` 才是"有符号"的那一面。
  这与 LLVM 一致（`i32` 本身无符号性）。
- **常量**：`ConstantPool::insert_int(value, bits)` 会先按 `bits` 规范化
  （`mask_to_bits`），因此 `-1 : i3` 与 `7 : i3` 是**同一个常量**；`bits > 128`
  走 `Big` 通道（`Builder::iconst_big`）。

### 2.1 逐运算语义

记值位宽为 `b`。

| 运算 | 语义 |
| --- | --- |
| `Iadd` / `Isub` / `Imul` / `Band` / `Bor` / `Bxor` / `Bnot` / `Neg` | 结果 = `(运算) mod 2^b`，回绕，无 UB |
| `Ishl` / `Ushr` / `Sshr` | 移位量按**无符号**比较：`amt >= b` ⇒ `Ishl`/`Ushr` 得 **0**，`Sshr` 得**全符号位**；否则按 `b` 位移 |
| `Icmp` (IntCC) | 按 `b` 位值比较；`IntCC::S*` 系列把 bit `b-1` 当符号位 |
| `Ireduce(dst)` | 要求 `dst_bits < src_bits`，保留低 `dst_bits` 位 |
| `Uextend(dst)` / `Sextend(dst)` | 要求 `dst_bits > src_bits`；`Sextend` 用 bit `src_bits-1` 填充 |
| `Bitcast` | 仅**等宽**互转；不等宽必须用 `zext/sext/ireduce` |
| `Udiv`/`Urem` | 除数为 0 ⇒ 目标机原生除零行为（op 上已有 `may_ub` 标记），不是 IR 层 UB |
| `Sdiv`/`Srem` | 二进制补码；除数为 0 同上 |
| `Select` | 逐位选值，结果与分支同类型 |

> 为什么把"移位量 `>= b`"也写死：非 2 的幂位宽（如 `i3`）没有硬件指令，
> legalize 之后用"扩位 + 掩码"实现时，若 IR 语义不明确，两条实现路径会给出不同结果。

### 2.2 布局

- `size_bytes = ceil(bits / 8)`；`alignment = min(natural_align(size_bytes), 16)`。
  最后一步的 16 字节上限避免"1024 位整数要求 128 字节对齐"这种离谱布局。

## 3. 浮点：按**格式**建模，而不是按总位宽

`TypeEntry::Float { fmt: FloatFmt }`，其中

```rust
pub struct FloatFmt { pub exp_bits: u8, pub mant_bits: u16 }
```

**符号位不入参**：所有被支持的浮点格式都是 **1 个符号位**（已知常量），只有阶码与尾数
是不确定的 ⇒ 总位宽 = `1 + exp_bits + mant_bits`（**不单独存 bits**，避免两份真相）。
这与 `util::big::FloatFormat { exp_bits, sig_bits }` 的口径一致。

只存总位宽无法区分格式：16 位既可能是 `f16`（1+5+10），也可能是 `bf16`（1+8+7）；
8 位有 e4m3/e5m2 等变体；还有大量厂商自定义格式。**按阶码/尾数建模**让任意格式都可表达
（符号位恒 1 位，无需入参）。

**预设**：`F16`/`F32`/`F64`/`F128`/`BF16`。旧的 `TypeEntry::BFloat { bits }` 变体已删除
（bf16 就是 `Float{1,8,7}`，无冗余）；`TypeStore::bfloat_ty(16)` 仍保留为预设入口。

**API**：

- `TypeStore::float_ty(bits)`：仅接受 16/32/64/128，映射到规范 IEEE 格式（其它位宽明确 panic 提示改用下一行）。
- `TypeStore::float_ty_fmt(fmt)`：任意格式（如 `FloatFmt::new(5, 2)` = 1+5+2 = 8 位）。
- `TypeStore::is_float`（store 版）对任意格式成立；`TypeId::is_float`（内建枚举表）只覆盖 f16/f32/f64/f128。

**文本名**：已知格式打印 `f16/f32/f64/f128/bf16`，其余打印 `f<exp>_<mant>`
（如 `f5_2` 表示 1 符号 + 5 阶码 + 2 尾数 = 8 位）。解析器接受这两种写法。

**常量折叠**：`util::big::FloatFormat` 与 `FloatFmt` 同为 (阶码, 尾数) 口径，可以一一映射，
因此任意格式都能参与常量折叠。

## 4. 补位规则（legalize 的优先级阶梯）

这是"映射层没有对应位宽运算时，用最近位宽补位"的**规范化定义**。目标机在 DSL 里声明
`[machine].int_widths = [...]`（原生支持的整数位宽，升序）。

以位宽 `b`、原生宽度集合 `W`、`W_max = max(W)` 记：

1. **R1 精确**：`b ∈ W` ⇒ 原样交给映射层。
2. **R2 就近升位**：否则取 `W` 中 `>= b` 的最小者 `W↑`；操作数 `zext|sext → iW↑`
   （有符号 op 用 `sext`，其余 `zext`）→ 运算 → `ireduce → i<b>`。
3. **R3 分字展开**：`b > W_max` ⇒ 拆成 `n = ceil(b / W_max)` 个字（末字按剩余位宽），
   每个字内部走 R1/R2，结果按字拼回。进位用 `icmp Ult` + `Iadd` 表达（不需要新 op）。
4. **R4 不可达**：谱里为**非原生位宽**写 `lower` 规则 ⇒ 生成期按"死规则"报错
   （legalize 之后映射层永远看不到非原生位宽）。

R1–R3 由 `forge-codegen` 的 `legalize::int_width` pass 统一实施（单一实现，无冗余）；
ISA-DSL 只声明能力，不重复描述展开方式。

## 5. 规模与边界

- 拆字号 `n = ceil(bits / W_max)`：线性族（add/sub/位运算/移位/比较/扩展截断）展开 **O(n)** 条指令，
  乘除是 **O(n²)**。**不设硬上限**；`n > 256` 时打印规模提示，但不失败。
- 位宽 > `2^32-1`：类型空间上界，明确报错（不是静默截断）。
- `Bitcast` 非等宽、`Ireduce` 方向错、`Sextend` 方向错：verifier 报错。
- `Ptrtoint/Inttoptr`：只允许指针宽度；窄整数转指针先 `zext` 到指针位宽。

## 6. 与 LLVM 的对应关系

| 本仓库 | LLVM |
| --- | --- |
| R1 精确 | `LegalizerInfo::legalFor` |
| R2 就近升位 | `widenScalarToNextPow2` / `clampScalar`（GlobalISel）、`PromoteIntegerResult`（SelectionDAG） |
| R3 分字展开 | `ExpandIntegerResult`（把 i128 拆成两个 i64） |
| `[machine].int_widths` | 目标机的 legal 类型集合 |
| `FloatFmt{exp,mant}`（符号恒 1 位） | LLVM 只有固定 `half/float/double/fp128` + `bfloat`；本仓库推广到任意 (阶码, 尾数) 组合 |

参考：[LLVM LegalizeIntegerTypes.cpp](https://llvm.org/doxygen/LegalizeIntegerTypes_8cpp_source.html#l01250)、
[AArch64LegalizerInfo.cpp（`legalFor` + `clampScalar` + `widenScalarToNextPow2`）](https://android.googlesource.com/toolchain/llvm-project/+/63f73545dd897f30846edfac5dc014f62efec6b5/llvm/lib/Target/AArch64/AArch64LegalizerInfo.cpp#2)。
