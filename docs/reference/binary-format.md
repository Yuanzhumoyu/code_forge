# forge-ir 二进制格式（IR bitcode v1）

> 状态：[active]（2026-09-19 起；`IR_FORMAT_VERSION = 1`）。
> 实现：`crates/foundation/forge-ir/src/binary/`；执行方案与逐切片证据见
> `docs/plans/forge-ir-binary-serialization-plan.md`。**本文件是格式的规范文本**，
> 与代码不一致时以代码为准（`IR_FORMAT_VERSION` 常量即版本号）。

## 1. 范围与目标

`Module` ⇄ 字节流**无损**往返：字节流确定（同输入同输出，同 producer 版本下逐字节
可比）、解码 fail-closed（截断/越界/未知 tag 一律 `Err`，绝不 panic、绝不静默丢数据）。

非目标：不兼容 LLVM bitcode（自家缓存格式）；不做向后兼容与自动升级；
不做惰性解析；不序列化可重算的数据（use-lists、去重表、分析缓存）。

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
           format_version：varint（当前 1；不等于当前版本即 Err，无兼容升级）
           producer：varint 长度 + UTF-8（头部自包含，仅诊断用）
           section_count：varint
           段表：section_count × { id: u8 | offset: varint | len: varint }
                  offset = 从流起点算起的**绝对偏移**
           段体：按 id 升序紧密排列（无对齐、无填充）
```

结构校验（读侧，全部 fail-closed 且带偏移）：魔数、版本、头部/段表截断、段表条数
（先与"剩余字节数 / 3"比对，拒绝对不可信长度分配）、未知段 id、重复段、段越界
（`offset + len > 文件长度`）、段体落在头部/段表内、段体互相重叠。

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

## 8. 确定性与版本

- 写侧不遍历 `HashMap` 决定输出顺序：所有集合按 dense index 或"首次出现顺序"输出，
  少数按内容排序（命名类型表、metadata 命名表、`DataLayout` 的三张对齐表与指针表）。
- 编码**幂等**：`encode(decode(encode(m)))` 与 `encode(m)` 逐字节相同。
- 版本策略：版本号不符即 `Err`；演进只允许"加段 + 升版本"，已发布段的字段顺序不再改。

## 9. 验证基线

| 证据 | 结果（2026-09-19 本机实测） |
| --- | --- |
| LLVM `test/Assembler` 正向语料二进制往返 | **198/198** 文本打印逐字符一致 + 编码幂等 |
| 尺寸基线 | 合计 **241,440 B**、最大 **31,038 B**（`auto_upgrade_nvvm_intrinsics.ll`）、平均 **1,219 B** |
| 空模块 | 恒定写 8 个段；往返后池/常量槽逐条相同 |
| 负向对照 | 截断前缀（逐字节扫描）、坏魔数、版本不符、未知段 id/COMPAT 位、重复字符串/常量/metadata、悬空 `TypeId`/`ConstId`/`AggId`/操作数/值类型/metadata 附件、未知 opcode 名/枚举 tag/端序、value kind 不一致、聚合前向引用 —— 全部 `Err` 且不 panic |
| 回归 | workspace 1689 passed / 0 failed / 19 ignored；语料 198 正向 / 254 正确拒绝 / 0 误收；矩阵 x86 195/3/0、riscv64 131/67/0、arm64 23/175/0 |

## 10. 已知限制

- 不压缩：字节数是"正确性优先"的基线，压缩属未来工作（有基线可对照）。
- 文本打印的**多别名 metadata**：2026-09-19 起已修——display 对每个名字各打印一行
  （名字按字节序 ⇒ 输出确定；`MetadataStore::names_of` 给出全部别名），
  `tests/display_llvm.rs::named_metadata_prints_every_alias` 钉住。
