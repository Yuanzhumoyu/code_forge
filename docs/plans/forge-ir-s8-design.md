# forge-ir S8 可选项设计方案（待拍板）

> 状态：[progress]（2026-09-17 撰写；本文件**只给设计与取舍**，不含实现）。
> 上游：`docs/plans/forge-ir-v3-plan.md` §4-I 与 §5-S8（原文："**I 可选项（S8，默认不做）**：
> 二进制序列化、MemorySSA-lite、crate 边界拆分"、"S8 | 可选（二进制/MemorySSA/crate 边界）| 需拍板"）。
> 文中所有现状数字均为 2026-09-17 本机实测（命令随文给出），**以代码为准**。

## 1. 范围、判据与现状基线

S8 是 v3 方案里唯一"默认不做"的一期：三个候选彼此独立，可以只做其中一个，也可以都不做。
本文按同一组判据逐个给设计，最后给建议与需要拍板的问题。

**判据**：①用户可见价值；②实现成本（行数/切片数）；③风险面（正确性/回归面）；
④可验证性（能不能钉成守卫）；⑤与既有非目标/决策是否冲突。

**现状基线**（实测）：

| 事实 | 数值 | 依据 |
| --- | --- | --- |
| `forge-ir` 生产代码 | **25,245 行** | `Get-ChildItem src -Recurse -Filter *.rs` 逐文件计行 |
| 其中文本层（`src/text/**`：parser + display） | **9,995 行（≈40%）** | 同上 |
| 最大单文件 | `text/parser/semantics.rs` 4,510、`text/display.rs` 2,906、`verify.rs` 2,641 | 同上 |
| `forge-ir` 测试代码 | **8,540 行**（`tests/` + 各文件 `#[cfg(test)]` 未计） | `Get-ChildItem tests -Recurse` |
| 运行期依赖 | `smallvec`、`thiserror`、`dashu`、`bitflags`；`logos`/`lalrpop-util` **optional**（`text`） | `crates/foundation/forge-ir/Cargo.toml` |
| 实体句柄 | **10 类**（`Value, Inst, Block, TypeId, FuncRef, ConstId, GlobalId, SigRef, AggId, VReg`） | `entity.rs` 的 `entity_ref_impls!` |
| 类型条目 | `TypeEntry` **9 个变体**（Int/Float/BFloat/Vector/ScalableVector/Array/Struct/Pointer/Function） | `types.rs` |
| 常量通道 | `ConstantPool` 6 类（int/float/big/vector/aggregate/字符串池）+ 去重表 | `constant.rs` |
| metadata | `MetadataNode`（Leaf/Tuple/…）+ `MetadataValue` 7 变体（含 `IntBig(ImmStr)`） | `metadata.rs` |
| 分析设施 | 支配树 + `predecessors()/successors()`（`analysis.rs` 471 行）；别名缓存 `alias.rs` 213 行 | 逐文件计行 |
| 门下移的现成状态 | `features=["text"]` 门控 2 个模块 + 静态边界守卫 + CI 无 feature 检查 | 计划 §6"文本层门控"切片 |

**本机编译时间基线**（有负载波动，仅作数量级参考）：`cargo check --workspace --exclude forge-rustc
--all-targets` ≈ **1 分 09 秒**（增量命中）；`--release --all-targets` ≈ 2 分；`cargo doc --no-deps
--all-features` ≈ 2 分。**当前没有任何一期因 forge-ir 的编译时间而受阻**。

## 2. 候选 A：二进制序列化（IR bitcode）

### 2.1 目标与非目标

**目标**：`Module` ⇄ 字节流**无损**往返（类型、常量、函数体、globals/aliases/comdats、metadata、
目标三元组与数据布局、符号信息），且字节流**确定**（同输入必同输出）。

**非目标**（明确写死，避免范围蔓延）：

- **不**兼容 LLVM bitcode（不是格式互操作，是自家缓存）；
- **不**做向后兼容/自动升级（仓库既有决策"无需兼容旧版本结构"）；
- **不**做惰性解析/延迟加载（先要正确与确定，懒加载是另一个工程）；
- **不**序列化可重算的东西（见 2.2 表）——落盘即双写源，违反单一事实源。

### 2.2 数据模型清单（什么落盘、什么必须重算）

| 数据 | 处理 | 理由 |
| --- | --- | --- |
| `TypeContext` / `TypeStore`（含命名类型与 `DataLayout`） | **落盘** | 类型是跨函数共享事实；`DataLayout` 是类型大小/对齐的唯一来源 |
| `ConstantPool` 6 通道（int/float/big/vector/aggregate/字符串池） | **落盘** | 常量是语义载荷；`Big` 走符号+字节长度+LE 字节，`ImmStr` 走字符串表 |
| `Function::{name, signature, calling_convention, attributes, extra_attrs, param_attrs, ret_attrs}` | **落盘** | 调用约定/属性影响 codegen 与 ABI |
| `Function::dfg`（值、指令、块、块参数、操作数、immediates、附件、`loc`） | **落盘** | 函数体本体；句柄按 dense index 写（顺序即 id，天然规范化） |
| `Function::layout`（块顺序、入口、参数表） | **落盘** | 顺序是语义（打印与 codegen 都依赖） |
| `Module::{globals, global_aliases, comdats, metadata_store, target_triple, source_filename}` | **落盘** | 模块级实体 |
| `SymbolInfo`（linkage/visibility/section/comdat/TLS） | **落盘** | 影响链接与 ABI |
| `Function::use_lists` | **重算** | 完全可由指令重算；落盘=双写，一旦不一致就是"两个真相" |
| `ConstantPool` 的 `*_dedup` 表、字符串池索引 | **重算** | 去重表是加速结构，insert 时自然重建 |
| `AnalysisManager` 缓存（支配树/循环森林/别名 memo） | **重算** | 已有修订号自校验，落盘无意义（S6 已删 `AnalysisCacheStale` 语义） |
| `DebugInfo.locations` | **落盘**（`loc` 随指令；file 走字符串表） | 调试信息是源码事实，不是派生量 |

### 2.3 格式设计

```text
┌─ 头部 ────────────────────────────────────────────────┐
│ magic "FORGEIR\0" (8B) | u16 版本 | u16 段数 | u32 保留 │
├─ 段表（每段：u8 tag | u32 长度 | u32 偏移）─────────────┤
│ 0x01 TYPES   0x02 CONSTS  0x03 METADATA  0x04 FUNCS    │
│ 0x05 GLOBALS 0x06 STRINGS 0x07 LAYOUT    0x08 SYMBOLS  │
└───────────────────────────────────────────────────────┘
```

编码原语（全部 fail-closed，越界/截断一律 `Err`，不 panic）：

- **varint**：LEB128（无符号）/ zigzag（有符号）；长度前缀一律 varint；
- **字符串表**：`STRINGS` 段顺序去重，引用写 u32 索引；空串用索引 0 固定；
- **句柄**：`Value/Inst/Block/TypeId/...` 一律写 dense index（`EntityRef::as_u32`），
  读侧按出现顺序 `push`/`insert` 重建——**顺序即 id**；
- **`Option<T>`**：1 字节 tag（0=空，1=有）；`Big`：符号 1 字节 + varint 字节长 + LE 字节；
- **枚举**（`Opcode`/`TypeEntry`/`AtomicRmwOp`/`Ordering`/`CondKind`/`MetadataNode` …）：
  写 u16/u8 判别值，读侧**穷举 match**（新增变体时编译期报错 → 不会静默错位）；
- **确定性**：所有遍历按 dense index 顺序，**禁止**遍历 `HashMap`（否则字节流不确定）。
  这一条现在有全仓守卫兜底：以 `Value/Block/Inst` 为键的 `HashMap` 全仓为 0（见
  `crates/backend/forge-codegen/tests/entity_tables.rs`）。

**版本策略**：`IR_FORMAT_VERSION: u16` 与常量同源；读侧版本 ≠ 常量 → `Err`（不做兼容）；
**未知段 tag → 硬错**（fail-closed，防"新写旧读"静默丢数据）；演进只允许"加段 + 升版本"。

### 2.4 公共 API 与 feature

```rust
impl Module {
    /// 序列化为确定性字节流（同输入必同输出）。
    pub fn to_binary(&self) -> Vec<u8>;
    /// 从字节流重建；截断/版本不符/未知段一律 `Err`（不 panic）。
    pub fn from_binary(bytes: &[u8]) -> Result<Module, IrError>;
}
```

- feature：`binary`（**建议默认开启**，与 `text` 对称；关闭时零成本、零依赖）；
- 错误：新增 `IrError::BinaryDecode { offset: usize, msg: String }`（带偏移，与文本层诊断口径一致）；
- 与文本层**正交**：`binary` 不依赖 `text`，反之亦然（可只开 `binary` 编出"无解析器"产物）。

### 2.5 验证方案（全部可执行、可钉成守卫）

1. **结构化往返**：`from_binary(to_binary(m))` 与 `m` 深比较——直接复用
   `tests/roundtrip_fuzz.rs` 的 `assert_modules_eq`/`assert_globals_eq` 思路并补 metadata；
2. **字节确定性**：`to_binary` 两次逐字节相同；`from_binary` 后再 `to_binary` 与首次
   **逐字节相同**（规范化收敛，等价于文本层的"打印幂等"）；
3. **真实语料**：198 个 LLVM 正向用例 → parse → `to_binary` → `from_binary` → 结构等价
   **且**文本打印等价（复用 `display_llvm` 的对照口径）；
4. **fuzz 扩面**：10k 随机模块（现成生成器）同样走二进制往返；
5. **负向守卫**：截断任意字节 → `Err` 不 panic；改一个段 tag → `Err`；改版本号 → `Err`；
6. **尺寸基线**：对 198 个正向用例记录合计字节数与最大值（作为后续演进的对照，不含压缩）。

### 2.6 分期（每期独立可验证、可单独叫停）

| 期 | 内容 | 产出 |
| --- | --- | --- |
| S8-A1 | 骨架：magic/版本/段表 + `TYPES` 段 + 字符串表 | 类型往返 + 确定性守卫 |
| S8-A2 | `CONSTS` 段（6 通道） | 常量往返（含 `Big`/向量字节/端序） |
| S8-A3 | `FUNCS` 段（含 `dfg`/`layout`/附件/`loc`） | 函数体往返（fuzz 覆盖） |
| S8-A4 | `GLOBALS`/`METADATA`/`SYMBOLS`/`LAYOUT` 段 | 全模块往返（语料覆盖） |
| S8-A5 | 负向守卫 + CI 步骤 + `binary` feature 门控 | 端到端门禁 |

### 2.7 成本、风险与收益

- **成本**：估 **1,200–1,800 行**（含测试），5 个切片；无新依赖（纯手写编解码）。
- **风险点**：`TypeId` 稳定性（必须在类型段内按 dense index 自洽重建，不能依赖"上次解析的
  编号"）；`ImmStr`（22 字节内联 + 堆/共享）必须走字符串表而非裸长度；`Big` 的规范形式
  （同值不同表示要收敛）；metadata 的**空洞占位**（`MetadataNode::Placeholder` ≠ 用户 `!{}`，
  需要显式区分位）；`loc.file` 的共享字符串。
- **收益**：①给 `features=["text"]` 一个**真实消费方**（现在该 feature 关掉后没有任何使用
  场景，只证明了"能编"）；②bench/CI 夹具可缓存已解析模块；③为 rustc e2e 的 IR 级缓存
  留接口（**需另评估**，不是本期的承诺）。
- **与既有决策的关系**：不冲突（不引入兼容层、不加依赖、不动文本层语义）。

## 3. 候选 B：MemorySSA-lite

### 3.1 现状

有支配树与 CFG 查询（`analysis.rs`，471 行）；有**保守**别名判定 + 记忆化缓存
（`alias.rs`，213 行；`gvn` 用它做 load 消重的 kill 精度）。**没有**后支配树、没有内存 SSA、
没有 DSE/store-to-load 前向 pass。

### 3.2 lite 设计（如果做）

- **表示**：不建完整 MemorySSA。只建"每块一个**进入 def**"的链：`MemDef{ inst, block }` +
  `per_block_entry: SecondaryMap<Block, MemDefId>`；**不做 MemoryPhi**（跨块取支配者链上最近的
  def，保守但简单）。粒度：**单一内存位置**（不区分字段/索引），精度由 alias 判定兜底。
- **构建**：RPO 遍历，块内顺序扫描；`Store/Call/AtomicRmw/Cmpxchg` 产生新 def；`Load` 记为 use
  并挂到当前 def 的 use 链（供后续 DSE/前向使用）。
- **接入**：作为 `AnalysisManager` 的惰性分析（修订号自校验，写 IR 自动失效）；
  消费方是**新 pass**（DSE-lite：store 之后无别名 use 且非 volatile/atomic → 删）。
- **验证**：①差分——DSE 开关两侧用 `Verifier` + 现有 pass 测试对齐；②负向——`volatile`/`atomic`
  载荷必须保留（构造用例必须 FAIL 若不保留）；③矩阵——三条 JIT 矩阵跑通（DSE 影响 codegen 输出）。

### 3.3 判据结论

**建议不做**。理由：①**当前没有消费方**——优化器现有 GVN/CSE + 保守别名已够用，为不存在的
pass 建基础设施违反"每期有可测收益"的既有纪律；②`alias.rs` 的精度**不足以**支撑激进 DSE
（它只是 memo + 保守判定），做浅了收益小、做深了要先补别名分析（另一期）；
③**触发条件**：一旦排期包含 DSE 或 store→load 前向，本设计的 3 步可以直接开工。

## 4. 候选 C：crate 边界拆分

### 4.1 现状

单 crate + `text` feature：`#[cfg(feature="text")]` 门控 `src/text/**`（parser + display）
（9,995 行，占 40%），可选依赖 `logos`/`lalrpop-util`；4 个下游 crate 依赖 forge-ir
（`forge-opt`、`forge-codegen`、`forge-hir`、`forge-object`），另有根伞 crate。

### 4.2 方案 C1：拆出 `forge-ir-text`

- **做法**：`text::parser` + `text::display` + `llvm_mapping` 移到新 crate；`text` feature 删除。
- **收益**：`cargo check` 只编核心时不编 9,995 行；依赖图显式（谁用文本层一眼可见）。
- **代价**：①`forge_ir::text::*` 路径全变——下游与
  forge-ir 自身 8,540 行测试里大量引用（`format!("{}", m)` 依赖 `Display for Module`）；
  ②文本层需要读核心**内部**面（`TypeStore`、`ConstantPool` 字段、`MetadataStore` 空洞语义），
  拆开后要么把这些内部面公开（扩大公共面），要么把文本层需要的操作变成 trait（成本更高）；
  ③原先的 `#[cfg]` 边界改为 crate 版本锁步（改核心就要同步改文本 crate 的版本与 API）。
- **实测支撑**：文本层占比 40%，但**没有**任何编译时间痛点（见 §1 基线）。

### 4.3 方案 C2：拆出 `forge-ir-types`

- **做法**：`types/entity/entity_map/constant/imm_str/string_pool` 独立。
- **问题**：`Module`/`Function`/`DataFlowGraph`/`Signature`/`SourceLocation` 全都持 `TypeId`，
  而 `TypeEntry` 需要 `FunctionSignature`（又需 `CallConv`/属性）——把类型拆出去会形成
  "types ↔ 核心"的双向依赖，只能用 trait 或把一半核心塞进 types crate。
- **结论**：收益（并行编译）远小于代价（公共面重构），**不建议**。

### 4.4 判据结论

**建议不做**：`text` feature + 静态守卫 + CI 无 feature 检查，已经提供了"可关文本层 + 分层
可验证"的全部**实际**收益；物理拆分的主要动机是编译时间与依赖卫生，前者本机未测出痛点，
后者已由守卫覆盖。**触发条件**：出现"必须把解析器从产物里彻底排除"的发布需求（不是编译
选项而是**产物形状**要求），或 forge-ir 编译时间成为日常瓶颈。

## 5. 建议汇总与需要拍板的问题

| 候选 | 建议 | 触发条件 | 预估 |
| --- | --- | --- | --- |
| A 二进制序列化 | **可做（推荐优先）** | 需要 IR 缓存/无文本层产物/夹具加速 | 1,200–1,800 行，5 切片，无新依赖 |
| B MemorySSA-lite | **不做** | 排期出现 DSE / store→load 前向 | 3 步（后支配树 → def 链 → DSE-lite） |
| C crate 拆分 | **不做** | 产物形状要求"无解析器"，或编译时间成瓶颈 | C1 中等（公共面 + 5,000 行引用迁移） |

**需要你拍板的四个问题**（答复后我按结果开工；也可以直接说"按建议办"）：

1. **有没有"无文本层"的真实消费方**（把 forge-ir 编进不带解析器的产物/缓存）？
   答"有" → A 的 `binary` feature 默认值按消费方定；答"没有" → A 降级为"先做 A1/A2 攒接口"。
2. **优化器排期里有没有 DSE 或 store→load 前向**？答"有" → B 开工（先补后支配树）。
3. **编译时间是否已是痛点**？答"是" → C1 进入评估（我会先给"拆分收益 = 文本层 9,995 行的
   编译时间"实测，再决定）；答"否" → C 关闭。
4. **rustc e2e 是否要用二进制 IR 缓存加速**？答"是" → A 提到最高优先（含缓存键与失效策略设计）。

## 6. 参考（外部）

- LLVM bitcode 自描述与 `Upgrade*` 层：<https://llvm.org/docs/BitCodeFormat.html>
- Cranelift 的 IR 与 CLIF 文本/二进制取舍：<https://github.com/bytecodealliance/wasmtime>
- MemorySSA（完整版，作为"lite 与完整的差距"参照）：
  <https://llvm.org/docs/MemorySSA.html>
- 别名分析（本仓 `alias.rs` 的保守口径对照）：
  <https://llvm.org/docs/AliasAnalysis.html>
