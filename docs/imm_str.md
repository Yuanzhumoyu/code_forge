# ImmStr — 不可变字符串类型设计清单

> 状态：已实现（forge-ir 0.1.0） · 配套迁移清单见文末

## 1. 背景与动机

项目中存在大量字符串克隆与重复分配：

- `forge-ir` 的 `Function.name`、`GlobalVariable.name`、`Comdat.name`、`GlobalAlias.name`、
  `SymbolInfo.section`、`TargetTriple` 四字段、`DebugInfo.file` 等均为 `String`，
  `Module` 维护三张 `HashMap<String, _>` 符号索引表，每次 `add_function` 都要
  `name.to_string()` 克隆一份堆字符串；
- `display.rs` 的 NameResolver 每次打印都重建 `HashMap<Value, String>`，是热路径；
- `ir_parser` / `verify` 中有大量 `"call".to_string()` 这类编译期字面量克隆；
- 解析器 lexer 对每个 token 做一次 `lex.slice().to_string()` 堆分配。

目标：设计一个**不可变字符串**类型，满足：

1. 枚举类型，变体覆盖**栈上内联（SSO）**与**堆上**两类存储；
2. **编译期静态字符串零拷贝借用**（`&'static str`，不产生任何分配）；
3. **克隆时直接引用共享**（O(1)，不深拷贝内容）；
4. `Send + Sync`（IR 可能跨线程共享，如 JIT 并行编译路径）；
5. 尺寸与 `String` 相当（64 位下 24 字节），可作为 `HashMap` 键并支持
   `get(&str)` 无克隆查询。

## 2. 现成库调研对比

| 库 | 内部表示 | 内联容量 | size_of | Clone 语义 | &'static str 零拷贝 | Send+Sync | 结论 |
|---|---|---|---|---|---|---|---|
| **compact_str** | 24B 全内联 / 独占堆缓冲（tag 藏指针低位） | 24 B | 24 B | **深拷贝 O(n)**（明确拒绝 COW） | 不支持 | ✅ | ❌ 排除：克隆深拷贝，违背核心目标 |
| **smol_str** | union{ Inline{len:u8,[u8;23]}, Arc\<str\> } | 23 B | 24 B | O(1)（Arc refcount++） | 长串会拷入 Arc | ✅ | ◑ 最接近，但**无 Static 借用变体** |
| **kstring** | Static \| Inline \| (Box\|Arc) | 15 B（max_inline 22 B） | ~24 B | Static/Arc O(1)；Box/Inline 拷贝 | ✅ 原生保留 | ✅ | ◑ 变体结构可借鉴；默认 Box 深拷贝、内联小 |
| **ecow (EcoString)** | 1B tag + 1B len + 22B inline，自研 refcount | 22 B | 24 B | O(1) 共享 | 未核实 | ✅ | ◑ 布局与自研一致 |
| **flexstr** | FlexStr：inline / Arc 共享 | 22 B | 24 B | FlexStr O(1)；FlexStrBase 深拷贝 | 有限 | ✅ | ◑ 双类型设计偏复杂 |
| **tendril** | rope，Rc\<Buffer\> 共享 | 小 | 24 B | O(1) | 部分 | ❌（Rc） | ❌ 排除：不 Send，久未活跃 |
| **im::RcStr** | `Rc<str>` 别名 | 无 | 16 B | O(1) | 部分 | ❌（Rc） | ❌ 排除：不 Send/Sync |

**结论**：没有比自研更贴合的组合——`compact_str` 克隆深拷贝；`smol_str` 缺
静态借用变体（长静态字符串会被拷入 Arc）；`kstring` 默认 `Box<str>` 深拷贝且
内联仅 15B；`tendril`/`im::RcStr` 因 `Rc` 不跨线程。自研方案是各库最佳实践的
组合：

- **内联编码**：借鉴 `smol_str`/`ecow` 的"1 字节长度 + 内容数组"（inline 22B）；
- **变体结构**：借鉴 `kstring` 的 `Static` 变体（`&'static str` 零拷贝借用）；
- **堆上共享**：`Arc<str>`（O(1) 克隆 + 原生 Send/Sync）。

**明确不采用的技巧**：arcstr 的 "refcount==0 即 static literal"（把 `&'static str`
塞进 Arc 的 data 指针槽）需要 unsafe 操作引用计数，v1 保持安全、简单、可审计，
用独立的 `Static` 变体表达同一语义。

## 3. 类型定义与布局

```rust
/// 不可变字符串 — SSO 栈上内联 + 静态借用 + Arc 堆上共享。
///
/// Clone 为 O(1)：Inline 固定 23 字节 memcpy；Static 指针拷贝；Shared 原子
/// 引用计数递增。三变体合计 24 字节（与 `String` 同尺寸）。
#[derive(Clone)]
#[repr(u8)]
pub enum ImmStr {
    /// ≤22 字节字符串内联在栈上，零堆分配。
    Inline(InlineStr),
    /// 编译期字面量的零拷贝借用（'static）。
    Static(&'static str),
    /// 堆上引用计数共享（Arc<str>）。
    Shared(Arc<str>),
}

/// 栈上内联存储：22 字节内容 + 1 字节长度，共 23 字节无 padding。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct InlineStr {
    buf: [u8; 22],
    len: u8,
}

pub const INLINE_CAP: usize = 22;
```

布局推导（64 位）：

```
InlineStr = [u8;22] + u8        = 23 B，对齐 1
Arc<str>                        = 16 B
枚举 = max(23, 16, 16) + 判别 1 = 24 B，对齐 8 → 24 B
```

对比：`String` 24 B（ptr+len+cap）、`&str` 16 B、`InternedStr` 4 B。

### 变体选择规则

| 输入 | 短（≤22 B） | 长（>22 B） |
|---|---|---|
| `&'static str`（经 `from_static`，const） | `Inline`（memcpy 进栈） | `Static`（零拷贝借用） |
| `String` | `Inline`（搬移字节） | `Shared`（`Arc::from(s)` 零拷贝 move） |
| `&str`（通用借用，含非 static） | `Inline` | `Shared`（必须拷贝——借用无法区分 'static） |
| `Cow<'static, str>::Borrowed` | `Inline` | `Static` |
| `Cow<'static, str>::Owned` | `Inline` | `Shared`（零拷贝 move） |
| `Arc<str>` | `Inline`（拷贝出内容后释放 Arc） | `Shared`（直接共享） |

> **关于 `Cow<'static, str>` 的说明**：需求方曾提议用 `Cow<'static, str>` 表达
> "静态借用 + 堆上拥有"。`Cow::Borrowed` 直接对应 `Static` 变体；但
> `Cow::Owned(String)` 的 `Clone` 是**深拷贝**，与"克隆直接引用"目标冲突，
> 故堆上变体用 `Arc<str>` 承载（`Arc::from(String)` 同样零拷贝 move，克隆仅
> refcount+1）。语义完全等价、代价更优。类型提供 `From<Cow<'static, str>>`
> 入口以吸收该提议。

## 4. 克隆语义与复杂度

| 变体 | Clone 行为 | 复杂度 |
|---|---|---|
| `Inline` | 23 字节 memcpy（常数） | O(1) |
| `Static` | 复制胖指针 | O(1) |
| `Shared` | `Arc` 原子 refcount+1，共享同一堆 buffer | O(1) |

任何路径都不拷贝堆上字符串内容。注意：`Inline` 的 O(1) 指固定字节数的
memcpy，属常数时间；真正的零拷贝只对 `Static`/`Shared` 成立。

## 5. API 契约

```rust
impl ImmStr {
    pub const fn from_static(s: &'static str) -> Self; // const 友好，恒为 Static（零分配）
    pub fn as_str(&self) -> &str;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn is_inline(&self) -> bool;
    pub fn is_static(&self) -> bool;
    pub fn is_shared(&self) -> bool;
}

impl From<&str> for ImmStr;         // 通用借用；长串拷贝进 Arc
impl From<String> for ImmStr;
impl From<Cow<'static, str>> for ImmStr;
impl From<Arc<str>> for ImmStr;
impl From<ImmStr> for String;        // 按需深拷贝（如错误消息拼接）
```

trait 实现：

- `Clone` — O(1) 共享（见上表）
- `PartialEq`/`Eq` — 按内容比较；`Shared` 对同指针走 `Arc::ptr_eq` 快速路径
- `PartialOrd`/`Ord` — 按内容字典序（`str::cmp`）
- `Hash` — 按内容 hash（与 Eq 一致，可用作 HashMap/HashSet 键）
- `Borrow<str>` — **关键**：`HashMap<ImmStr, V>` 支持 `get(name: &str)` 无克隆查询
- `AsRef<str>`、`Deref<Target = str>` — 透明使用 `str` 方法
- `Display`/`Debug` — 输出内容（Debug 带引号）
- `Default` — 空串（Inline 空缓冲）

不变式：短于等于 22 字节的字符串**总是**落在 `Inline`（`from_static` 对短串同样内联）。
注意：`From<&str>` 在类型层面无法区分 `'static` 与非 `'static` 借用（Rust 禁止同时
实现 `From<&'static str>` 与 `From<&str>`），故长串统一拷贝进 Arc；需要零拷贝静态
借用时显式使用 `from_static` 或 `From<Cow<'static, str>>::Borrowed`。

## 6. 与 InternedStr / StringPool 的关系

| | `InternedStr` | `ImmStr` |
|---|---|---|
| 存储 | 全局池句柄（u32），内容在 `StringPool` 内 | 自包含值，内容随值走 |
| Clone | Copy（零开销） | O(1)（memcpy / refcount） |
| 生命周期 | 依赖 pool 存活（借用语义） | 自持所有权，无外部依赖 |
| 去重 | 按内容全局去重 | 不去重（相同内容各自持有） |
| 典型用途 | 模块内大量重复名字（value_names、metadata） | 跨模块传播的名字、HashMap 键、公开字段 |

两者**互补共存**：

- 公开结构字段（`Function.name` 等）与 Module 索引表 → `ImmStr`；
- 模块内高频去重句柄（`value_names`/`block_names`/metadata/Immediate）→
  保持 `InternedStr`；
- 桥接：`StringPool::intern(&imm_str)` 可直接以 `&ImmStr`（Deref 到 str）入池，
  `lookup(id)` 结果可 `ImmStr::from(&str)` 转回自持值。

选择指引：需要跨函数/跨模块传播、作为 `HashMap` 键、生命周期简单 → `ImmStr`；
模块内大量去重、需要 Copy 句柄 → `InternedStr`。

## 7. 边界与取舍

- **Inline 容量 22 B**：24 字节枚举布局约束下三变体的最优解（`smol_str` 的
  23 B 内联需 union + tag 编码，无法同时容纳 `Static` 变体）。
- **`From<&str>` 长串必须拷贝**：借用无法脱离原生命周期，这是不可变借用的
  固有约束；调用方应优先传 `String` 或 `&'static str` 走零拷贝路径。
- **`From<Arc<str>>` 短串会拷贝出内容**：Arc 可能被共享，无法 move 出 buffer；
  为维持"短串恒 Inline"不变量做一次小拷贝（代价可忽略，短串场景罕见）。
- **不做 hash 缓存**：`Shared` 变体未缓存内容 hash（v1 简化；`Arc<str>` 无法
  直接缓存，需自定义 header，收益有限，留给后续）。
- **错误类型不迁移**：`CompileError`/`ParseError`/`VerifyError` 的 String 字段为
  冷路径（构造一次、消费一次），迁移需引入生命周期依赖，收益≈0，明确不做。

## 8. 迁移清单（forge-ir 及下游）

优先级 P0 > P1 > P2。括号内为收益说明。

### P0 — 符号名 + Module 索引表

- [x] `Function.name: String → ImmStr`（function.rs:200）
- [x] `GlobalVariable.name: String → ImmStr`（function.rs:645）
- [x] `Comdat.name: String → ImmStr`（symbol.rs:174）
- [x] `GlobalAlias.name: String → ImmStr`（function.rs:668——原 `Alias` 已并入 `GlobalAlias`）
- [x] `SymbolInfo.section: Option<String> → Option<ImmStr>`（symbol.rs:194）
- [x] `Module` 三张索引表 `HashMap<String,_> → HashMap<ImmStr,_>`
  （`func_names`/`global_names`/`comdat_names`，function.rs:741-754；
  `Borrow<str>` 令 `find_function(&str)` 等查询零克隆）
- [x] `Function::new` 签名 `name: String → name: impl Into<ImmStr>`
  （builder.rs:133、semantics.rs、jit.rs 等调用方同步）

### P0 — 显示热路径

- [x] `display.rs` NameResolver：`values/blocks: HashMap<_, String>` →
  `HashMap<_, ImmStr>`（每次 Display 打印少 N 次堆分配，`disambiguate` 克隆 O(1)）
- [x] `display.rs:533-534` `"undef"/"poison"` 字面量改 `ImmStr::from_static`

### P1 — 结构数据

- [x] `TargetTriple.{arch,vendor,os,environment}: String → ImmStr`
  （data_layout.rs:472-480，parse 切片直转，短字段零分配）
- [x] `SourceLocation.file: Option<String> → Option<ImmStr>`（debug_info.rs:13）
- [x] `DebugInfo.function_name: Option<String> → Option<ImmStr>`（debug_info.rs:57）
- [x] `FunctionSignature.params: Vec<(TypeId, String)> → Vec<(TypeId, ImmStr)>`
  （types.rs:678）

### P1 — 字面量 + 解析器

- [x] `ir_parser/semantics.rs` 函数名构造改 `ImmStr::from(trimmed)`（短函数名
  内联零分配；长名单次 Arc 分配，不再 to_string + 二次拷贝）
- [ ] `verify.rs` 字面量 — **评估后不迁移**：所有字面量都在错误构造路径
  （`errors.push(...)` 分支内，冷路径），且字段属 `VerifyError(String)`，迁移需
  连带改错误类型（明确不迁移），收益≈0
- [ ] `ir_parser` lexer（lexer.rs:123-142）— **评估后不迁移**：token 生命周期短
  （lexer→parser→semantics 立即消费），grammar.lalrpop 生成代码改动面大、风险高；
  最终函数名已通过 `FunctionBuilder::new(impl Into<ImmStr>)` 受益
- [ ] `ir_parser` AST（ast_items.rs）String 字段 → `ImmStr`（临时 AST，收益中等，
  可选）

### 不迁移

- `CompileError`/`ParseError`/`VerifyError` 错误 String 字段（冷路径）
- `forge-dsl` 代码生成器的模板字符串（一次性构建）
- `InternedStr` 相关（value_names/block_names/metadata/Immediate）保持现状

### 下游调用方同步

- [x] forge-codegen `jit.rs`：`compiled/symbols/pending_relocs` 键改 `ImmStr`
  （`Arc<str>` 为 Send+Sync，无碍并行编译路径）；`func.name.clone()`/`global.name.clone()`
  变 O(1)；`register_external`/`add_compiled`/符号注册路径改 `ImmStr::from`
- [x] forge-codegen `emit.rs`：`Relocation.symbol` 改 `ImmStr`（add_reloc
  `ImmStr::from`、空符号 `ImmStr::default()`）；`output_types.rs` 同步
- [x] forge-opt `lto.rs`：`build_ref_table` 返回 `HashMap<ImmStr, FuncRef>`
- [x] forge-object：`written_symbols`/`declared_externs` 键改 `ImmStr`
- [x] forge-hir / examples / 集成测试：适配 `FunctionBuilder::new(impl Into<ImmStr>)`
  （`&String` → `.as_str()`、测试断言 `.name.as_str()`）；forge-hir 内部
  `Block.name` 等 HIR 层字段保持 `String`（与 forge-ir API 无关，未迁移）

## 9. 验证

- [x] `cargo test -p forge-ir` 全绿（含 imm_str 单元测试）
- [x] `cargo test --workspace --exclude forge-rustc` 全绿（1287 passed / 0 failed）
- [x] `cargo check --workspace` 通过（含 forge-rustc）
