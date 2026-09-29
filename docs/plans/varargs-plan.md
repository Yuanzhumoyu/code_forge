# 变参（varargs）方案 [progress]

> 状态：**方案 + 决策点清单**（2026-09-30 起草）。**代码未动**——本仓库今天没有任何前端会产出
> 变参签名，发射侧对 `va_list` 零消费，所以先写清"有什么、缺什么、先做哪一段"，避免给一条
> 没人走的路写发射代码。相关背景：`docs/reference/calling-conventions.md`、
> `docs/plans/calling-convention-redesign-plan.md` 的 A6。

## 1. 现状：**规划（plan）已经能做，发射与前端是空的**

| 层 | 现状 | 证据（按符号名，行号随迭代漂移） |
| --- | --- | --- |
| 规则数据 | `hidden = { va_meta_pool, va_len_pool, va_list, va_list_size, va_list_align }`、`variadic_stack_only` 都在 | `forge-abi/src/rules.rs` 的 `Hidden` / `VaListKind` |
| 引擎 | 变参签名能规划：**未命名实参**按 `variadic_stack_only` 决定"只走栈"还是"继续用寄存器"；产出 `AbiPlan::va_area`（kind/size/align/stack_only）与 `hidden.va_meta`/`va_len` | `forge-abi/src/engine.rs`（`unnamed` 分支、`va_area` 构造） |
| 黄金快照 | 四份内置约定 × 变参语料已钉住（见 §2 表） | `forge-abi/tests/golden/*.plan.txt` 的 `## va_*` 段 |
| CLI | `forge-isa abi plan <谱> --conv <名> --sig "…" --variadic <命名数>` 能打印变参计划 | `forge-isa/src/abi.rs` |
| **发射** | **零消费**：`va_area` / `hidden.va_meta` / `hidden.va_len` 在 `crates/…/src` 里没有任何读者 | 全仓 grep：命中只在引擎、测试与黄金文件 |
| **前端** | **零产出**：`FunctionSignature::variadic` 在生产代码里没有设置者（测试与二进制格式往返测试除外） | `forge-rustc` / `mini_c` 里没有 `variadic` |
| IR | 签名能表达 `variadic`（二进制格式也往返） | `forge-ir/src/ir/types.rs`、`forge-ir/src/binary/types.rs` |

一句话：**"计划"这一半是完整的，"发射 + 前端"这一半完全没有。**

## 2. 四份内置约定的 `va_list` 形态（本仓库内置数据）

这张表是**代码里的数据**（`forge-abi/src/builtin.rs` 的四份规则），不是抄 psABI 手册——
与官方定本逐条核对列为 §4 的待办 D3。守卫 `invariants.rs::va_shapes_match_the_documented_table`
拿引擎实际算出的计划与这张表逐格对照。

| 约定 | `VaListKind` | size | align | 未命名实参 | `va_meta` 寄存器 |
| --- | --- | --- | --- | --- | --- |
| `win64` | `Win64Stack` | 8 | 8 | 只走栈 | — |
| `sysv64` | `SysvRegSave` | 24 | 8 | 继续用寄存器 | `RAX` |
| `aapcs64` | `Aapcs64Struct` | 32 | 8 | 只走栈 | — |
| `lp64d` | `RiscvSaveArea` | 24 | 8 | 只走栈 | — |

## 3. 缺口（按链路，越靠前越阻塞）

```text
前端（写 variadic 签名 / va_start / va_arg）      ← 缺
   │
IR（签名能表达；缺"取下一个实参"的 op 形态）        ← 缺
   │
引擎（✔ 已能算：va_area / va_meta / 未命名实参落点）
   │
发射：调用方（设 %al、按 plan 传未命名实参）        ← 缺
发射：被调方（物化寄存器保存区 / 建 va_list）        ← 缺
```

三个具体缺口：

1. **调用方**：调用一个**外部**变参函数（如 `printf`）时，要按约定设 `va_meta`（SysV 的 `%al`
   = 用到的向量寄存器个数）并把未命名实参按 plan 传出去。引擎已给出 `va_meta` 与落点，
   缺的只是发射。
2. **被调方**：函数自己是变参时，要在序言把"寄存器参数区"物化成 `va_area`
   （SysV 保存区 / riscv 保存区 / AAPCS64 结构；Win64 例外——`va_list` 就是栈指针）。
3. **`va_arg`**：从 `va_list` 取下一个实参，且要做**默认实参提升**（`f32`→`f64`、窄整型→
   `int`）。这是唯一需要"按类型推进游标"的部分，也是与 A6 的"按成员赋值"同源的难点。

## 4. 需要拍板的决策点

| # | 决策 | 选项 | 影响 |
| --- | --- | --- | --- |
| D1 | **先服务哪种场景** | ① 调用外部变参函数（调用方）② 自己被当变参调用（被调方） | ①只动发射、能立刻用 JIT 验（造一个自己的变参 callee）；②要 `va_start` 的 IR 形态，牵动前端 |
| D2 | **`va_arg` 怎么表达** | ① IR 内建 op（`va_arg(va_list, ty)`）② 前端展开成 load + 游标运算 ③ 提供库函数 | 决定要不要动 `forge-ir` 的 opcode 表与文本/二进制格式 |
| D3 | **以哪份 psABI 定本为准** | SysV / Win64 / AAPCS64 / RISC-V 各自官方定本 | §2 的 size/align 现在是"数据里写的"，需逐条核对（尤其 `sysv64` 的 24 字节保存区与 `%al` 语义） |
| D4 | **先做哪台机器** | 建议 **win64**（`va_list` = 栈指针，不需要寄存器保存区、不需要 `%al`） | 最小的可用切片；riscv/sysv 的保存区留到第二期 |
| D5 | **提升规则放哪** | 规则数据（`when = { kind = "float", size_le = 4 } → f64`）还是引擎内置 | 影响"足够通用"：数据化更好，但要设计键 |

## 5. 建议的分期（每期独立可验证）

- **V1（调用方，win64 优先）**：调用点按 plan 发未命名实参；`va_meta` 非空时按约定设寄存器
  （SysV 的 `%al`）；守卫 = 生成物形状 + 一个 JIT 用例（自造 win64 变参 callee，读栈上的
  未命名实参拼出结果）。**不依赖前端**：用例直接用 `FunctionSignature::variadic(n)` 构造。
- **V2（被调方，win64）**：序言物化 `va_area`（win64 下就是取栈参数的地址）。
- **V3（寄存器保存区，sysv64/lp64d）**：按 `va_area.size/align` 在帧内开槽并把参数寄存器存进去；
  校验 `hidden.va_meta` 的写入。
- **V4（`va_arg`）**：按 D2 的裁定实现取参 + D5 的提升规则；这是唯一必须动 IR/前端的一期。
- **V5（体检）**：`forge-isa abi check` 增一条"变参形状自洽"的检查（`va_list_size` 与
  保存区槽数一致等）。

验收口径沿用本仓库惯例：每期跑 `fmt` + `clippy -D warnings` + workspace 全量（含三条 JIT
矩阵），生成物形状/落点用守卫钉住，行为用 JIT 真跑。

## 6. 为什么现在**不做**（触发条件）

与 A6 的两条"评估后不做"同源：**没有消费者**。

- 没有任何前端会产出变参签名 ⇒ V2–V4 写完也没有真实输入；
- 发射侧零消费 ⇒ 现在写只是"为将来的路预埋"，而本仓库的纪律是"先有证据再动手"
  （参见 A6 ⑤ 的按成员赋值、`callee_pop` 两条）。

**触发条件**（出现任一即可开工）：① 有宿主需要在 JIT 里调用外部变参函数（例如 `printf`
一类运行时入口）；② 有前端（`mini_c` 或 forge-rustc 的外部声明）开始产出 `variadic` 签名；
③ 有人要接一份"被调方变参"的 ABI 兼容测试。

在此之前，本方案的 §1–§2 就是**当前事实的索引**，`invariants.rs` 的变参守卫保证它不漂移。
