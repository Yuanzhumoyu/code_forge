# forge-ir 二进制序列化执行方案（B1–B5）

> 状态：[progress]（2026-09-19 撰写并执行）。上游设计取舍见
> `docs/plans/forge-ir-s8-design.md` §2；本文件是它的**执行细化**（字节级格式、切片、
> 每片门禁与负向对照）。范围由用户 2026-09-19 拍板：**只做二进制序列化**——
> S8 的另两个候选（MemorySSA-lite、crate 边界拆分）**本轮不做**，也**不做**任何
> 兼容旧结构的迁移层。文中一切现状数字均为本机实测，**以代码与测试为准**。

## 1. 目标与非目标

**目标**：`Module` ⇄ 字节流**无损**往返（类型、常量、函数体、globals/aliases/comdats、
metadata、目标三元组与数据布局、符号信息），字节流**确定**（同输入同输出，同进程同
producer 版本下逐字节可比），解码**fail-closed**（截断/越界/未知 tag 一律 `Err`，绝不
panic、绝不静默丢数据）。

**非目标**（写死，避免范围蔓延）：

- 不兼容 LLVM bitcode（这是自家缓存格式，不是格式互操作）；
- 不做向后兼容/自动升级（仓库既有决策"无需兼容旧版本结构"：版本不符即 `Err`）；
- 不做惰性解析/延迟加载（先正确与确定，懒加载是另一个工程）；
- 不序列化**可重算**的东西（`use_lists`、去重表、分析缓存——落盘即双写源）；
- 不引入任何新依赖（见 §7 对 postcard/bincode/rkyv/serde 的比较与否决）；
- 不做压缩、不做 mmap、不做跨进程共享内存。

## 2. 格式 v1（字节级规范）

### 2.1 容器布局

```text
offset 0   ┌ magic：8 字节 ASCII "FORGEIR\0"
           ├ format_version：varint（= IR_FORMAT_VERSION，当前 1）
           ├ producer：varint 长度 + UTF-8 字节（**头部自包含**，不引用 STRINGS：
           │   头部诊断不依赖任何段）
           ├ section_count：varint
           ├ 段表：section_count × { id: u8 | offset: varint | len: varint }
           │   offset 为**从流起点算起的绝对偏移**；len 为段体字节数
           └ 段体：各段按 id 升序紧密排列（无对齐、无填充）
```

段表在段体之前完整写出 ⇒ 解码器可以先做**结构校验**（段是否越界、是否重叠、是否
缺段），再进入语义解码；这也是把错误定位做到"偏移 + 说明"的前提。

**实现细化（B1 已落）**：段表偏移是绝对偏移；段表条数在校验前先与"剩余字节数/3"
比对（每条至少 3 字节），拒绝对不可信长度分配；段体允许留空隙但**不得重叠**；
`COMPAT` 段体只有 1 字节 `flags`，本版本只认 `0`（未知位置位即 `Err`）。

### 2.2 段清单

| id | 名 | 内容 | 解码依赖 |
| --- | --- | --- | --- |
| `0x00` | COMPAT | 兼容位标记（见 §2.7） | 无（最先读） |
| `0x01` | STRINGS | 字符串表（`num` + 各串长度 + 串字节拼接；`num = 0` = 空池） | 无 |
| `0x02` | TYPES | `TypeStore`：内建类型表 + 命名类型 + `DataLayout` + 签名表 | STRINGS |
| `0x03` | CONSTS | 常量池 6 通道：int / float / big / vector / aggregate / 字符串 | STRINGS、TYPES |
| `0x04` | METADATA | `MetadataStore` 节点表 | STRINGS、TYPES |
| `0x05` | FUNCS | 每个函数的签名 + `dfg`（值/指令/块/块参数/附件/`loc`）+ `layout` + 属性 | STRINGS、TYPES、CONSTS、METADATA |
| `0x06` | GLOBALS | 全局变量、别名、comdat + 符号信息 | STRINGS、TYPES、CONSTS、METADATA |
| `0x07` | MODULE | 目标三元组、`source_filename`、模块级 asm、函数名表（`FuncRef` 顺序） | STRINGS、FUNCS |

**顺序是契约**：解码器按上表依赖顺序推进；写侧同一顺序。段的**内部**顺序也固定
（见各切片），因此"同输入同输出"不依赖任何哈希容器遍历。

### 2.3 编码原语

| 原语 | 编码 | 备注 |
| --- | --- | --- |
| `u8` / `bool` | 1 字节 | `bool` 只接受 `0`/`1`，其它值 → `Err` |
| `u32` / `u64` / `usize` | LEB128 varint | `usize` 按 `u64` 写；读侧超过平台 `usize::MAX` → `Err` |
| `i32` / `i64` | zigzag + LEB128 | 负值不膨胀 |
| `f32` / `f64` | 4/8 字节小端 | 写 `to_bits()`、读 `from_bits()`——**NaN 载荷与 `-0.0` 原样保留** |
| `Option<T>` | `0x00` 空 / `0x01` 有 + 载荷 | 无"默认值"捷径 |
| `Vec<T>` | varint 长度 + 元素 | 长度先与"剩余字节数"比对，超限即 `Err`（**不做 `with_capacity(恶意长度)`**） |
| 字符串 / `ImmStr` | varint 索引（指向 STRINGS 段） | 走 STRINGS 段，绝不裸写长度+字节 |
| 句柄（`Value`/`Inst`/`Block`/`TypeId`/…） | varint dense index | 顺序即 id：读侧按出现顺序重建，**不写"上一次解析的编号"** |
| 枚举（`Opcode`/`TypeEntry`/`Ordering`/`AtomicRmwOp`/`MetadataNode`/…） | u8/u16 判别值 + 载荷 | 读侧**穷举 match**：新增变体编译期报错，不会静默错位 |
| `Big` | 符号 1 字节 + varint 字节长 + LE 字节 | 规范形式：无前导零、零只有正号 |
| metadata 嵌套 | 递归 + **深度上限**（见 §2.6） | 恶意深嵌套不得把解码器打爆栈 |

### 2.4 函数段的两遍解码（关键设计）

`DataFlowGraph` 里值的 dense 顺序 = **块参数先建、指令结果后建**。为保证"顺序即 id"
在解码侧精确复现，FUNCS 段对每个函数写出：

1. `value_kinds`：varint 个数 + 每个值的 1 字节 kind（`0` = 块参数，`1` = 指令结果）；
2. 块表：块数，每块 `{ 名字, 参数个数, 各参数类型, 各参数的值索引 }`；
3. 指令表：指令数，每条 `{ opcode, 结果个数, 结果值索引, 操作数, immediates, flags, mem_flags, 附件, loc, isel_strategy }`；
4. `layout`：块顺序、入口块、函数参数表。

解码器据此**校验**：块参数与指令结果按 `value_kinds` 顺序恰好覆盖 `0..value_count`，
且每条指令的结果索引等于"轮到它的下一个索引"；任何不一致 → `Err`（而不是"猜一个"）。
这条显式表把"隐式假设构造顺序"变成**可校验的数据**，代价每值 1 字节。

### 2.5 字符串表

```text
num: varint
lengths: num × varint（与 bytes 分列）
bytes: 各串字节按顺序拼接（UTF-8）
```

去重按首次出现顺序（写侧用 `HashMap<ImmStr, u32>` **仅作写入期索引**，输出顺序由
首次出现决定 ⇒ 字节流仍确定）。读侧重建 `StringPool`，并校验：每段是合法 UTF-8、
**无重复**（重复即 `Err`）。

**实现细化（B1 已落，与早期草案的差异）**：表**不预留"索引 0 = 空串"槽**，
`num = 0`（空池）合法。理由：预留槽会让解码后的池比编码前多一条 `""`，且
`InternedStr(i)` 与表索引相差 1；不预留则**池逐条往返**（连空串也不多不少）、
解码期句柄与索引恒等、以后各段的深度比较不用为"句柄平移"开特例。空串需要时就是
普通一条（`intern("")`）。

### 2.6 解码器（`Cursor`）与 fail-closed 纪律

- 所有读取走 `Cursor`：`read_u8/read_varint/read_zigzag/read_bytes(n)/read_str`；
  每次读先查剩余长度，越界 → `IrError::BinaryDecode { offset, msg }`；
- **绝不 panic**：不使用 `unwrap()`/切片索引，不用 `debug_assert` 当校验；
- 长度字段（`Vec`/字符串/段）一律先与剩余字节比对，再分配；
- metadata 嵌套深度上限 64（超过 → `Err`），防递归爆栈；
- 未知段 id、未知枚举判别值、未知 `Opcode` 判别值 → `Err`（不是"跳过该段"）。

### 2.7 兼容检查与版本策略

```rust
/// 当前格式版本。与解码器同源（写侧常量、读侧比对）。
pub const IR_FORMAT_VERSION: u16 = 1;

/// 只读头部即可回答"这份字节流我能不能读"（不建任何 IR）。
pub struct BinaryCompat {
    pub format_version: u16,
    /// 开放集合字符串一律 `ImmStr`（v3 S5 边界规则，`open_set_boundary.rs` 钉住）。
    pub producer: ImmStr,
    pub sections: Vec<SectionId>,
}

/// 只解析头部；版本不符/未知段/头部截断一律 `Err`（带偏移）。
pub fn check_binary_compat(bytes: &[u8]) -> Result<BinaryCompat, IrError>;
```

- 版本不符 → `Err`（**不做兼容层**）；
- `COMPAT` 段：v1 只写一个 `flags` varint（当前为 `0`，无标志位），为将来"读侧可忽略
  的扩展"留位置；**未知 flag 位 → `Err`**（宁可拒绝，不静默降级）；
- 演进规则：只允许"加段 + 升版本"；已有段的字段顺序一旦发布不再改。

### 2.8 确定性

- 写侧**禁止**遍历 `HashMap`/`HashSet`（本仓已有守卫：以 `Value/Block/Inst` 为键的
  `HashMap` 全仓为 0，见 `crates/backend/forge-codegen/tests/entity_tables.rs`）；
- 所有集合按 dense index 或"首次出现顺序"输出；
- `producer` 串含 crate 版本：**跨版本不保证逐字节相同**，同版本内保证；往返幂等
  （`from_binary(to_binary(m))` 再编码与首次**逐字节相同**）是硬验收项。

## 3. 代码布局与公共 API

```text
crates/foundation/forge-ir/src/binary/
├── mod.rs      # 门面：IR_FORMAT_VERSION、SectionId、BinaryCompat、check_binary_compat、
│               #       Module::{to_binary, to_binary_into, from_binary}
├── format.rs   # 段 id/常量/magic/头部布局（唯一"格式事实源"）
├── writer.rs   # 写侧原语 + 字符串表 + 段装配（finish）
├── reader.rs   # 读侧 Cursor + 头部/段表解析 + STRINGS 段
└── types.rs    # TYPES 段（类型条目 / 命名类型 / 签名表 / DataLayout）
```

（后续切片按同一模式加 `consts.rs` / `metadata.rs` / `funcs.rs` / `globals.rs` /
`module.rs`，各段编码器在本地缓冲里写完再 `assign_section` 装回 writer——
这样"写段体"与"登记字符串（要 `&mut Writer`）"可以交错，不触借用冲突。）

- `lib.rs` 增 `pub mod binary;` + 扁平重导出 `pub use binary::{IR_FORMAT_VERSION, check_binary_compat, BinaryCompat};`；
- **不加 feature 门控**（决策变更，理由见 §7）：二进制层零依赖、不依赖文本层，
  与 `text` 正交；
- 新错误变体：`IrError::BinaryDecode { offset: usize, msg: String }`（带偏移，和文本层
  诊断口径一致），`Display` 文案含偏移与说明。

## 4. 切片（每片独立可验证、可单独叫停）

| 片 | 范围 | 产出 | 负向对照（必须有） | 估行 |
| --- | --- | --- | --- | --- |
| B1 | `format.rs` + `Cursor` 读写原语 + STRINGS 段 + 空 `Module` 往返 | `to_binary`/`from_binary`/`to_binary_into` + `check_binary_compat` | 截断 magic/版本/段表 ⇒ `Err`；未知段 id ⇒ `Err`；版本号改 1 位 ⇒ `Err`；两次编码逐字节相同 | 400–550 |
| B2 | TYPES 段（内建表 + 命名类型 + `DataLayout` + 签名） | 类型往返 + 去重收敛 | 坏类型 tag ⇒ `Err`；同名重复命名类型 ⇒ 定义侧拒绝/解码侧 `Err`；跨 `TypeContext` 句柄不混用 | 350–500 |
| B3 | CONSTS 段（int/float/big/vector/aggregate/字符串） | 常量往返（含 `Big` 符号+字节、NaN 载荷、向量端序） | 非法 `ConstId` tag ⇒ `Err`；`Big` 前导零与非规范零 ⇒ 写侧规范、读侧 `Err` | 300–450 |
| B4 | FUNCS 段（`value_kinds` + 块 + 指令 + `layout` + 属性/附件/`loc`） | 函数体往返（fuzz 10k + 语料） | 中间截断 ⇒ `Err` 且偏移落在该段内；`value_kinds` 与结果索引不一致 ⇒ `Err` | 450–650 |
| B5 | METADATA / GLOBALS / MODULE 段 + 全模块语料 + `docs/reference/binary-format.md` | 全模块往返 + 尺寸基线 + 参考文档 | 段重叠/越界 ⇒ `Err`；未知 flag 位 ⇒ `Err`；198 语料往返幂等 | 300–450 |

**每片完成即跑 §5 门禁并提交**（不跨片合提交），提交信息写清"本片新增/修了什么 +
负向对照的实测结论"。

## 5. 每片门禁（命令固定，逐条留证据）

```bash
cargo fmt --all && cargo fmt --all -- --check
cargo clippy --workspace --exclude forge-rustc --all-targets --all-features -- -D warnings
cargo clippy -p forge-ir --no-default-features --lib -- -D warnings
cargo test --workspace --exclude forge-rustc --exclude cargo-forge -j 1 -- --test-threads=1
cargo test -p forge-ir --test llvm_assembler_compat --test corpus_roundtrip --test display_llvm
cargo check --workspace --exclude forge-rustc --release --all-targets
RUSTDOCFLAGS=-D warnings cargo doc --no-deps --all-features
npx --yes markdownlint-cli2 <本次改动的 .md 文件>
```

外加一次性矩阵（三架构 JIT）：

```bash
cargo test -p forge-tests --lib isa::x86_v12::jit_matrix_x86_v12 -- --test-threads=1 --nocapture
cargo test -p forge-tests --lib isa::riscv64_v12::jit_matrix_riscv64_v12 -- --test-threads=1 --nocapture
cargo test -p forge-tests --lib isa::arm64_v12::jit_matrix_arm64_v12 -- --test-threads=1 --nocapture
```

**基线（2026-09-19 实测，作对照）**：工作区 1515 passed / 0 failed / 19 ignored；
LLVM 语料 198 正向 / 254 正确拒绝 / 0 误收；语料往返幂等 189/189；
JIT 矩阵 x86 195/3/0、riscv64 131/67/0、arm64 23/175/0。

## 6. 风险与对策

| 风险 | 对策 |
| --- | --- |
| 句柄顺序假设错（值/指令/块 dense id） | B4 显式写 `value_kinds` 并在解码侧校验；不一致即 `Err`（§2.4） |
| `TypeId` 稳定性 | 类型段内按 dense index 自洽重建；读侧只认段内顺序，不认外部编号 |
| `ImmStr` 的 SSO/堆/共享三态 | 一律走字符串表（索引），不写内联/堆的物理形态 |
| `Big` 非规范表示 | 写侧规范化（无前导零、零只有正号）；读侧校验并 `Err` |
| metadata 空洞占位 `Placeholder` ≠ 用户 `!{}` | 显式区分位（单独判别值），不靠"空串/零值"约定 |
| 恶意/损坏输入打爆内存或栈 | 长度先比剩余字节；嵌套深度上限；无 `with_capacity(不可信长度)` |
| 往返不收敛（编码非幂等） | 硬验收：`encode(decode(encode(m)))` 与 `encode(m)` 逐字节相同 |
| 新增 `IrError` 变体引发全仓 match 失配 | 编译期穷举报错逐个修（预期为 0–3 处），不写兜底臂 |

## 7. 与既有决策的关系

- **零新依赖**：手写编解码。已评估并否决 `postcard`/`bincode`/`rkyv`/`serde`——
  它们要么引入依赖与 derive 宏，要么无法把"偏移 + fail-closed 纪律 + 禁止 HashMap
  遍历的确定性"钉在同一层；本格式的错误定位与确定性是**验收项**，不是附加特性。
- **不加 feature 门控**（对 `forge-ir-s8-design.md` §2.4 的修订）：二进制层零依赖、
  不依赖文本层，门控只会制造"只有开 feature 才编到"的验证盲区；`text` 门控保留原样。
- **无兼容层**：与仓库既有决策一致（"无需兼容旧版本结构"），旧结构直接删除并同提交
  迁移全部调用点，不留别名/双入口。
- **与文本层正交**：`text` 与 `binary` 互不依赖，可各自独立编译与验证。

## 8. 参考

- MLIR Bytecode Format（段表/varint/字符串段/偏移段的设计来源）：
  <https://mlir.llvm.org/docs/BytecodeFormat/>
- LLVM BitCode Format（块结构、缩写与向后兼容取舍）：
  <https://llvm.org/docs/BitCodeFormat.html>
- wasmtime 序列化（容器头 + 版本前置检查 + 面向用户的错误信息）：
  <https://docs.rs/wasmtime/36.0.4/src/wasmtime/engine/serialization.rs.html>

## 9. 验收记录

> 每片完成后在此追加实测数字与命令（禁止只写"通过"）。空白表示尚未执行。
> B1/B2 合并提交（原因见 B1 行）：B1 的负向对照暴露了 `open_set_boundary` 违规，
> 修复与 B2 同批落地，两片的门禁在合并提交上一次性跑绿。

| 片 | 提交 | 关键证据 | 日期 |
| --- | --- | --- | --- |
| B1 | （与 B2 同提交） | 容器 + STRINGS：`binary` 单测（原语 varint/zigzag/Option/长度前缀的已知字节 + `Cursor` 截断/溢出/非法 UTF-8/越界四类 fail-closed）、`tests/binary_format.rs` 的空模块往返、池逐条往返、编码确定性 + `decode→encode` 幂等、`to_binary_into` 追加语义、`check_binary_compat`；负向：**任意截断前缀（0..len-1）全部 `Err` 且不 panic**、坏魔数（偏移 0）、版本不符、未知段 id、未知 COMPAT flag 位、重复字符串、段数炸弹（100 万段）。**负向对照实测**：`BinaryCompat.producer: String` ⇒ `open_set_boundary.rs::no_plain_string_fields_on_public_ir_surface` FAILED 并点名该行（v3 S5 守卫当场生效）→ 改 `ImmStr`；`to_binary_into` 追加语义下 `finish()` 的头部长度断言（按绝对长度比）误报 → 改为按增量比。 | 2026-09-19 |
| B2 | | TYPES：类型条目 12 种 tag 全覆盖（Int/Float/BFloat/Vector/ScalableVector/Array/Struct 含命名字段/Pointer/Function/Token/Metadata/Opaque）、命名类型表（按名排序 ⇒ 字节确定）、签名表（含 `CallConv::Custom(42)` 与 variadic）、`DataLayout`（三张对齐表 + 指针表按 key 排序）。往返断言：条目**逐字段相等**（含 `InternedStr` 句柄，因字符串表不预留槽 ⇒ 句柄与索引恒等）、命名类型查回一致、签名逐字段一致、`DataLayout` 逐字段一致（不用 `{:?}`——内部 `HashMap` 的 Debug 序随实例随机种子变化）。负向：未知类型 tag、TYPES 段体截断（读到一半即错）、悬空 `TypeId`（条目引用 / 命名类型引用，单测级）、预填充固定索引错位（单测级）、`num=0` 空池合法（不再是错误）。 | 2026-09-19 |
| B3 | | | |
| B4 | | | |
| B5 | | | |
