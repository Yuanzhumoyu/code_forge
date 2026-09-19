# forge-ir 二进制格式（IR bitcode v2）

> 状态：[active]（2026-09-19 起；`IR_FORMAT_VERSION = 2`——v2 增段体压缩）。
> 实现：`crates/foundation/forge-ir/src/binary/`；执行方案与逐切片证据见
> `docs/plans/forge-ir-binary-serialization-plan.md`。**本文件是格式的规范文本**，
> 与代码不一致时以代码为准（`IR_FORMAT_VERSION` 常量即版本号）。

## 1. 范围与目标

`Module` ⇄ 字节流**无损**往返：字节流确定（同输入同输出，同 producer 版本下逐字节
可比）、解码 fail-closed（截断/越界/未知 tag 一律 `Err`，绝不 panic、绝不静默丢数据）。

非目标：不兼容 LLVM bitcode（自家缓存格式）；不做向后兼容与自动升级；不做惰性解析；
不序列化可重算的数据（use-lists、去重表、分析缓存）。

## 2. 公共 API

```rust
impl Module {
    /// 序列化为确定性字节流。
    pub fn to_binary(&self) -> Vec<u8>;
    /// 序列化并**追加**到 `out`。
    pub fn to_binary_into(&self, out: &mut Vec<u8>);
    /// 从字节流重建；任何损坏一律 `Err`（不 panic）。
    pub fn from_binary(bytes: &[u8]) -> Result<Module, IrError>;
}

/// 只解析头部 + 段表（含结构校验），不建 IR。
pub fn check_binary_compat(bytes: &[u8]) -> Result<BinaryCompat, IrError>;

pub struct BinaryCompat {
    pub format_version: u16,
    pub producer: ImmStr,
    pub sections: Vec<SectionId>,
}

pub enum IrError { /* … */ BinaryDecode { offset: usize, msg: String } }
```

**不加 feature 门控**：二进制层零依赖、不依赖文本层（`text` 与 `binary` 正交）。

## 3. 容器布局

```text
offset 0   magic "FORGEIR\0"（8 字节）
           format_version：varint（当前 2；不等于当前版本即 Err，无兼容升级）
           producer：varint 长度 + UTF-8（头部自包含，仅诊断用）
           section_count：varint
           段表：section_count × { id: u8 | offset: varint | len: varint | raw_len: varint }
                  offset  = 从流起点算起的**绝对偏移**（指向落盘的段体）
                  len     = 段体在流里的字节数
                  raw_len = 解压后长度；`0` = 段体原样存放，`> 0` = 段体是压缩体
           段体：按 id 升序紧密排列（无对齐、无填充）
```

结构校验（读侧，全部 fail-closed 且带偏移）：魔数、版本、头部/段表截断、段表条数
（先与"剩余字节数 / 4"比对，拒绝对不可信长度分配）、未知段 id、重复段、段越界
（`offset + len > 文件长度`）、段体落在头部/段表内、段体互相重叠；`raw_len > 0` 时
还要过压缩比与单段上限（见 §10）。

## 4. 段

| id | 名 | 内容 | 解码依赖 |
| --- | --- | --- | --- |
| `0x00` | COMPAT | `flags` varint（当前只认 `0`；未知位置位即 `Err`） | 无 |
| `0x01` | STRINGS | `num` + `num` 个长度 + 字节拼接（UTF-8）；`num = 0` = 空池 | 无 |
| `0x02` | TYPES | 类型条目表 + 命名类型表 + 签名表 + `DataLayout` | STRINGS |
| `0x03` | CONSTS | 常量池五通道：int / float / big / vector / aggregate | STRINGS、TYPES |
| `0x04` | METADATA | `MetadataStore` 节点表 + 命名表 | STRINGS |
| `0x05` | FUNCS | 每个函数：头部 + 每函数常量池 + `dfg` + `layout` | STRINGS、TYPES、CONSTS、METADATA |
| `0x06` | GLOBALS | 全局变量、别名、comdat | STRINGS、TYPES、CONSTS、METADATA |
| `0x07` | MODULE | `target_triple`、`source_filename`、模块 asm | STRINGS |

八个段**恒定写出**（空模块也写）：读侧据此把"段为空"与"段不存在"分开对待
（实现上缺段 = 跳过，因此新增段不会破坏旧读侧）。

## 5. 编码原语

| 原语 | 编码 |
| --- | --- |
| `u8` / `bool` | 1 字节（`bool` 只接受 `0`/`1`） |
| `u32` / `u64` / `usize` / `u128` | LEB128 varint（u128 最多 19 字节；溢出即 `Err`） |
| `i64` / `i128` | zigzag + LEB128 |
| `Option<T>` | `0x00` 空 / `0x01` 有 + 载荷 |
| `Vec<T>` | varint 长度 + 元素（长度先与剩余字节比对） |
| `ImmStr` / 字符串 | varint 索引 → STRINGS 段（**绝不裸写长度+字节**） |
| 句柄（`Value`/`Inst`/`Block`/`TypeId`/`ConstId`/`AggId`/`GlobalId`/`SigRef`/`MetadataId`/`ComdatId`/`FuncRef`） | varint dense index（顺序即 id） |
| `Big` | 变体 tag + payload（见下） |
| metadata 嵌套（`MetadataValue::Field` 递归） | 递归 + **深度上限 64**（`MAX_METADATA_DEPTH`，超过即 `Err`）——不可信输入不得把解码器栈打爆（进程 abort，不是可捕获错误） |
| 位集合（`InstFlags`/`MemFlags`/`FunctionAttributes`） | 底层整数原样（未知位保留） |
| 枚举（`Linkage`/`Visibility`/`DllStorageClass`/`TlsModel`/`MetadataKind`/`ComdatKind`/`CallConv`/`TypeEntry`/`Immediate`/`ValueDef`/`MetadataNode`/`MetadataValue`/`IntCC`/`FloatCC`） | 显式判别值 + 载荷，读侧**穷举 match**（新增变体编译期报错） |
| `Opcode` | **名字**的 STRINGS 索引（`ops.toml` 是单一事实源；读侧 `from_name`，未知名即 `Err`） |

`Big`：`Signed`/`Unsigned` 写 varint 字节长 + dashu 小端补码字节；`Float` 写
归一化有效数 + zigzag 指数 + **`Context::precision`**（精度是值的一部分，
`Repr::into_parts()` 会归一化有效数、`from_parts` 会把精度重置为"有效数位数"）。

## 6. 字符串表

不预留"索引 0 = 空串"槽、`num = 0` 合法：解码后的 `StringPool` 与编码前**逐条相同**
（连空串也不多不少），且解码期 `InternedStr(i)` 与表索引恒等。写侧去重按**首次出现
顺序**（`HashMap` 只作写入期索引，不参与输出顺序）；读侧拒绝重复项与非法 UTF-8。

## 7. 各段细节

### 7.1 TYPES

条目表（12 种 tag：Int/Float/BFloat/Vector/ScalableVector/Array/Struct/Pointer/
Function/Token/Metadata/Opaque，按 `TypeId` 顺序）→ 命名类型表（按名字节序排序）→
签名表（含 `CallConv::Custom`）→ `DataLayout`（指针表与三张对齐表**按 key 排序**）。

解码：清空到"无类型"→ 按原样插入条目（去重键"先出现者胜"，保留空洞
`TypeId(9)` 就是 `Int{0}` 的重复条目）→ 命名类型 → 签名 → `DataLayout` →
`finish_binary_decode`（与同布局的新建 store 逐条比对前 16 条预填充 + 悬空
`TypeId`/命名类型引用检查）。

### 7.2 CONSTS

`int`（值 zigzag + 位宽）→ `float`（位模式 u128 + 值宽）→ `big` → `vector`
（字节 + 端序）→ `aggregate`（类型 + 子节点：`Scalar(ConstId)` / `Agg(AggId)`，
只允许引用**已解码**的聚合 ⇒ 防环）。解码按同序 `insert_*` 重建：返回索引与该条
位置不符（文件含重复条目）即 `Err`。函数级常量池复用同一编码。

### 7.3 METADATA

节点表（`Leaf` / `Tuple` / `Named{name, ops, distinct}` / `Placeholder`，按 id 顺序）
→ 命名表（`name → id`，**多对一**，按名字节序排序）。

节点**按原样落位**（不用 `intern` 回放）：显式 `!N` 编号经 `insert_at` 预分配槽位，
arena 里允许出现内容相同的两条（各自都有引用者），`intern` 会折叠 ⇒ id 错位。
节点内 `Node(id)` 只校验 `id < 节点总数`（**前向引用合法**）。

### 7.4 FUNCS

每个函数：

```text
name | signature | call_conv | attributes | extra_attrs
| symbol(linkage/visibility/dll/section/comdat/TLS/3×bool)
| is_const | personality | param_attrs | ret_attrs
| metadata(附件) | debug_info(locations + function_name)
| value_names | block_names
| constants(五通道) | values | insts | blocks | layout | entry_block
```

- `values`：每个值写**显式 `ValueDef`**（`Inst(i,k)` / `Param(b,k)` / `AggConst` /
  `UndefNamed(str)`）+ 类型；
- `insts`：opcode 名、所属块、结果、操作数、immediates（11 变体）、
  `InstFlags`/`MemFlags`、调用点属性、metadata、`loc`、`isel_strategy`、墓碑位；
- `blocks`：参数类型、参数值、块内指令顺序、终结符指令。

**use-lists 不落盘**：解码后按每条指令的操作数重建（终结符指令同在 `insts` 里）。

解码后由 `validate_dfg` 校验："顺序即索引"的每条 `ValueDef` 回指一致（指令第 k 个
结果 / 块第 k 个参数）+ 指令结果反向回指 + 值类型/操作数/结果/块参数类型/块顺序表/
终结符/布局/入口块全部在界内——任一处不一致即 `Err`。

### 7.5 GLOBALS

全局变量（名字、类型、字节 init、表达式文本、符号、`is_constant`、对齐、地址空间、
附件、ifunc 参数与 resolver）→ 别名（名字、类型、linkage、`dso_local`、
`unnamed_addr`、aliasee 文本、附件）→ comdat（名字、kind）。
回放一律经 `Module::{add_global, add_global_alias, add_comdat}`：名字索引表随之重建
（这三张表**不落盘**），重名由这些入口 fail-closed 拒绝。

### 7.6 MODULE

`target_triple`（`arch`/`vendor`/`os`/`environment` 四段，`Option`）→
`source_filename`（`Option`）→ 模块 asm（多条，按序）。

## 8. 段体压缩（v2）

**逐段**压缩，段边界与解压后的字节流**完全不变**（解码器看到的永远是与 v1 逐字节
相同的段体）——压缩只是"落盘形态"。

### 8.1 决策规则（无旋钮）

写侧对每个段体跑一次压缩，**只在压缩体严格更小时**才存压缩体并把 `raw_len` 写成
原长；否则 `raw_len = 0`、段体原样落盘。没有开关、没有级别参数 ⇒ 同输入同输出，
且**永不"越压越大"**（`tests/binary_format.rs::packed_entries_are_always_strictly_smaller`
钉住这条不变量）。

### 8.2 压缩算法（自研、零依赖）

token 流，`header` 为 varint：

```text
header 为偶：字面量段，长度 = header / 2，随后跟同样多的原始字节
header 为奇：回引段，长度 = header / 2 + 4，随后跟 varint(距离 - 1)
```

窗口 64 KiB、最小匹配 4、单 token ≤ 64 KiB；匹配用固定哈希表 + 链式前驱、贪心取
最长、候选链长上限 64（防退化输入退化成 O(n²)）。哈希表长随输入规模自适应
（`1 << 12`…`1 << 16`），且由 `Packer` 跨段复用。全程无随机、无哈希迭代序依赖 ⇒
确定性。短于 32 字节的段体不建表（只发一个字面量 token ⇒ 必然"不更小" ⇒ 原样存）。

**为什么自研**：二进制层的既有承诺是"零新依赖"。算法只在段边界上起作用，因此
将来若需更高压缩率，可**按段替换**成外部实现（例如 zstd）而**不需要改格式**——
段表已经有 `raw_len`。届时再单独评估依赖。

### 8.3 解码侧的 fail-closed

解压前先做大小检查（防解压炸弹）：`raw_len` 超过单段上限 **256 MiB**，或超过
`len × 65536 + 4096` 的压缩比上限，一律 `Err`（不进入解压循环）。解压中：距离必须
落在已产出字节内、token 长度不得超过"还差多少到 `raw_len`"、字面量必须在输入内、
结束时必须**恰好**产出 `raw_len` 字节且**输入恰好耗尽**（多一字节 = 坏流）。
解压后的段体走与未压缩段体**完全相同**的段解码路径。

错误偏移的语义：段结构错误指向**流内绝对偏移**；解压失败指向该段的 `offset`；而
解压成功之后的段内解码错误（`decode_strings` 等）偏移是**段体相对**的——排错时先看
消息里的段名。

## 9. 确定性与版本

- 写侧不遍历 `HashMap` 决定输出顺序：所有集合按 dense index 或"首次出现顺序"输出，
  少数按内容排序（命名类型表、metadata 命名表、`DataLayout` 的三张对齐表与指针表）。
- 编码**幂等**：`encode(decode(encode(m)))` 与 `encode(m)` 逐字节相同（压缩决策同理，
  见 §8.1）。
- 版本策略：版本号不符即 `Err`；演进只允许"加段 + 升版本"，已发布段的字段顺序不再改。
  版本历史：

  | 版本 | 变更 |
  | --- | --- |
  | 1 | 首版容器：8 段 + 段表 `{id, offset, len}` |
  | 2 | 段表加第 4 字段 `raw_len` + 段体压缩（§8）；**无兼容读取**，v1 字节流在 v2 读侧直接报版本不符 |

## 10. 验证基线

| 证据 | 结果（2026-09-19 本机实测） |
| --- | --- |
| LLVM `test/Assembler` 正向语料二进制往返 | **198/198** 文本打印逐字符一致 + 编码幂等 |
| 尺寸基线（v2 压缩） | 源码文本合计 **408,810 B** → 字节流合计 **121,257 B**（**0.30×**）、最大 **8,482 B**（`auto_upgrade_nvvm_intrinsics.ll`）、平均 **612 B**；v1 未压缩为 241,440 B（0.59×）——一次性省掉 **×0.50** |
| fuzz 往返 | 随机模块 **10,500**（默认 10,000 + 固定种子 500）结构等价 + 文本一致 + 字节幂等，0 失败 |
| 吞吐（`cargo bench -p forge-ir --bench ir_binary`） | v2：encode ~22.5 µs、decode ~42.9 µs（三次运行中位数；中等模块 958 B）；v1：21.9 µs / 77.2 µs（1,335 B）⇒ 压缩的编码成本落在计时噪声内（+3%），解码因字节变少而快 ~44%。本机单点波动可达 ~1.7×，故只作方向判据 |
| 压缩专项 | `repetitive_strings_section_is_packed_and_roundtrips`（`raw_len` 必须等于**测试侧独立核算**的未压缩段体长度 + 压缩率 ≥ 4×）、`corrupt_packed_section_is_rejected`（坏首字节 / `raw_len` 不符 / 截断三路）、`decompression_bomb_is_rejected`（超单段上限、超压缩比） |
| 空模块 | 恒定写 8 个段；往返后池/常量槽逐条相同 |
| 负向对照 | 截断前缀（逐字节扫描）、坏魔数、版本不符、未知段 id/COMPAT 位、重复字符串/常量/metadata、悬空 `TypeId`/`ConstId`/`AggId`/操作数/值类型/metadata 附件、未知 opcode 名/枚举 tag/端序、value kind 不一致、聚合前向引用、metadata 嵌套 65/5000 层、坏压缩段/坏 `raw_len` —— 全部 `Err` 且不 panic |
| 回归 | workspace 1692 passed / 0 failed / 19 ignored；语料 198 正向 / 254 正确拒绝 / 0 误收；矩阵 x86 195/3/0、riscv64 131/67/0、arm64 23/175/0 |

## 11. 消费者与工具

- **示例 CLI**：`cargo run -p forge-ir --example ir_binary -- [--check] [--write <dir>] [--cache <dir>] <file.ll>…`
  只用公开 API（示例是独立 crate ⇒ 也是"公开面够不够用"的编译期检验）：
  默认模式做 parse → encode → decode → 逐函数 `Verifier` → 文本一致 → 字节幂等，
  并打印每文件的源码/字节流尺寸与 parse/encode/decode 耗时；`--check` 只读头部报
  `check_binary_compat`（版本/producer/段表）；`--write <dir>` 把字节流落盘为 `.fir`；
  `--cache <dir>` 走文件级 IR 缓存（见下）。
- **文件级 IR 缓存** `forge_ir::IrCache`：把"源码文本 → `Module`"这一步按**内容键**
  落盘（键 = 128 位指纹，含格式版本与 producer ⇒ 升版本自动失效）。语义是
  `load` / `store` / `get_or_insert_with`；**任何**读失败、损坏、版本不符都当 miss
  （自愈：miss 时重算并覆盖），落盘走"临时文件 + `rename`"以免读到半截条目。定位是
  **可随时删除的加速层，不是事实源**。
- **缓存的盈亏要看尺度**（本机实测，三次运行全值在 `docs/performance/bench_baseline.md`）：
  884 B 文本的中等模块命中 114–195 µs vs 直接解析 77–94 µs（**亏 1.2–2.3×**，文件读取的
  固定成本 75–204 µs 占主导）；43 KB 文本的大模块命中 723–1146 µs vs 解析 1835–2320 µs
  （**赚 2.0–2.5×**）。在 452 个 `.ll` 的完整夹具目录上跑示例，缓存不带来可测收益
  （332 个文件本就解析失败，可解析的 119 个都太小）。→ 调用方按实测盈亏决定是否缓存。
- **不在正确性测试里用缓存**：测试的职责包含覆盖解析器本身，缓存会把"文本 → 模块"
  这一步短路掉，等于让测试少测一层。基准与夹具运行器按需显式开启。
- **基准**：`cargo bench -p forge-ir --bench ir_binary`（含 `ir_binary_cache` /
  `ir_binary_cache_large` 两组：`parse_text` vs `cache_hit`，并拆出 `read_entry` /
  `decode_entry` 便于归因；文本层解析吞吐另见 `--bench ir_parse`）。

## 12. 已知限制

- 压缩算法是自研的字节级 LZ77：压缩率低于 zstd 一档；换来的是零依赖 + 完全确定性 +
  可审计（约 200 行）。替换为外部实现**不需要改格式**（§8.2）。
- 压缩在编码热路径上有实测代价（§10 的吞吐行），换来的是体积减半。
- 文本打印的**多别名 metadata**：2026-09-19 起已修——display 对每个名字各打印一行
  （名字按字节序 ⇒ 输出确定；`MetadataStore::names_of` 给出全部别名），
  `tests/display_llvm.rs::named_metadata_prints_every_alias` 钉住。
