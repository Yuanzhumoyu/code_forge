# 常量层（`ConstId` / `BitValue` / `ConstantPool` / `AsConst`）

> `[active]` v28（2026-10-18）：不兼容旧池的重新设计。**以代码为准**：
> `crates/foundation/forge-ir/src/ir/constant/`。
> 二进制落盘格式见 [`binary-format.md`](binary-format.md) §7.2 与版本历史 v6。

## 1. 三条不变量

1. **标量只有一种形态**：`BitValue { bits: u128, width: u32 }`（`1..=128`，更宽走 `Big`）。
   "这是整数还是浮点"由**使用点的类型/opcode** 决定，池**不记录种类**——
   于是 `i32 0x3FC0_0000` 与 `f32 1.5` 是同一条目，这正是"整数与浮点合并为比特值"的意图。
2. **一个句柄类型管全部常量**（含聚合）：`ConstId` 是四元枚举，2 bit tag 恰好铺满 ⇒
   `raw()`/`from_raw()` 是**全域**函数，任何 `u32` 都能解码，没有非法 tag、没有 panic 分支。
3. **句柄索引 = 通道内插入序号**：四条通道各自按值去重（interner），二进制按同序重建即可
   保证句柄逐位不变；解码期"第 i 条拿到的索引 ≠ i"即判重复条目并报错。

## 2. `ConstId`

```rust
pub enum ConstId { Bits(u32), Big(u32), Vector(u32), Agg(u32) }

ConstId::TAG_BITS   // 0
ConstId::TAG_BIG    // 1
ConstId::TAG_VECTOR // 2
ConstId::TAG_AGG    // 3

id.raw()             // tag<<30 | index
ConstId::from_raw(x) // 全域：不会失败
id.tag() / id.index() // 高位 tag / 低 30 位池内索引
ConstId::default()   // Bits(0)
```

`ConstId` **不实现** `EntityRef`（全仓没有以它为键的密集表）；句柄字段私有化守卫因此仍然成立。
旧版的独立聚合句柄 `AggId` 已删除。

## 3. 通道与创建入口

| 通道 | 载荷 | 创建 |
| --- | --- | --- |
| 位值 | `BitValue`（整数/浮点合并） | `intern_bits(BitValue)`、`intern_int(i128, width)`、`intern_float(bits, width)` |
| 任意精度 | `Big` | `intern_big(Big)` |
| 向量 | 字节 + 端序 | `intern_vector(&[u8], Endianness)` |
| 聚合 | `AggConst { ty, children: Vec<ConstId> }` | `intern_agg(ty, children)` |

要点：

- `intern_int` 入池前套**符号扩展规范形**（`canonical_int`）：`-1` 在 64 位里是全 1 位图，
  `i1` 例外（bool 取值 0/1，不扩展）。这样 `const_int(-1, i64)` 仍是 `-1`，文本打印
  （有符号字面量）与回读也稳定。
- `intern_vector` 的**端序进去重键**：同字节不同端序是两个常量（旧实现会误合并）。
- 聚合的子节点就是 `ConstId`——"子节点是不是聚合"由池判定，不需要额外枚举；**环不可能**
  （子句柄必先于父存在）。
- 池**没有预置槽**：`new()` 即空池，`bool` 只是普通常量（去重后自然两条）。

## 4. 读取与视图

```rust
pool.get(id) -> Option<ConstEntry<'_>>      // Bits / Big / Vector / Agg 借用视图
pool.bits(id) -> Option<BitValue>           // 分型便捷入口
pool.big(id) / pool.vector(id) / pool.vector_endian(id) / pool.aggregate(id)

pool.as_i64(id) -> Option<i64>              // 位值按【原位宽】取补码值；超 i64 范围返回 None
pool.as_big(id) -> Option<Big>              // 同上，不丢高位语义
pool.as_f64_bits(id) -> Option<u64>         // 低 64 位；Big 转浮点

pool.import(&mut dst, src, id) -> ConstId   // 跨池重建（聚合递归）
```

**`as_i64`/`as_big` 的位宽语义**（曾经的缺陷源）：池里存的是符号扩展规范形，但调用方要的是
"这个位宽下的补码值"。因此两者内部都先按 `width` 截断再符号扩展
（`signed_of`）⇒ `i32::MIN` 得到 `-2147483648`，而不是一个巨大的正 `u128`。
直接 `i64::try_from(bits)` 会让 isel 静默回落成 0（v28 实测：JIT `boundary_constants` 红）。

## 5. `AsConst` —— 前端唯一的常量糖

```rust
fb.constant(42i32);          // 等价于 42i32.into_const(&mut fb)
fb.const_int(v, ty);         // 统一整数（i8..=i64/u8..=u64/i128 都能直接传）
fb.const_float(bits, ty);    // 统一浮点（f16/bf16/f32/f64/f128 共用）
fb.const_bool(v);            // i1
fb.const_big(v, ty);         // 任意精度
fb.const_vector(lanes) / const_vector_endian / const_vector_array / const_vector_bytes[_endian]
```

- `AsConst` 覆盖 `bool`、`i8..i128`、`u8..u128`、`f32`、`f64`；**不提供** `usize/isize`
  （宽度随目标，容易在跨目标时变味）。
- 历史的按类型平铺函数（`iconst_i8/i16/i32/i64`、`fconst_f32/f64`、`iconst_i128`、
  `fconst128`、`vconst*`）**已全部删除**，没有兼容别名。

## 6. 边界与失败模式

| 情形 | 处置 |
| --- | --- |
| `>128` 位整数 | 只能 `intern_big`；`bits(id)` 返回 `None` |
| `width` 越界 | `intern_bits` 有 `debug_assert!(1..=128)`；二进制解码仍按 `1..=128` 校验 |
| 同 `(bits,width)` 被 int 与 float 各用一次 | 合法且期望：同一 `ConstId`，两次解释分别是位图 / IEEE 模式 |
| 悬空句柄交给 `import` | **panic**（旧实现静默返回源池句柄，会让跨池句柄泄漏到本池） |
| 解析期浮点宽度未知 | 记 128（"未定"），文本打印优先用**值类型**定宽（half/bf16 只能靠类型区分） |
| 聚合子越界/未解码 | 二进制解码报 `BinaryDecode`；verifier 报"池外句柄" |

## 7. 与文本/二进制的关系

- **文本**：`Iconst`/`Fconst` 打印时按 opcode + 值类型解释同一条位值；聚合常量按
  `ConstId::Agg` 递归还原成 `{…}` 字面量；解析侧 `Operand::Int/UInt` → `intern_int`、
  `Operand::Float` → `intern_float(…, 128)`。
- **二进制**：v6 起 CONSTS 是**带 tag 单流**（bits / big / vector / aggregate），
  无兼容读取；详见 [`binary-format.md`](binary-format.md)。

## 8. 回归网（改这里必须跑）

| 检查 | 覆盖 |
| --- | --- |
| `cargo test -p forge-ir` | 合并键 / 无预置槽 / 端序进键 / 跨池导入 / 句柄编码全域 / `AsConst` / v6 往返 / 文本语料 |
| `cargo test --workspace --exclude forge-rustc` | 下游 opt/object/dsl/tests 全量 |
| `cargo test -p forge-rustc --test e2e` | 真实 rustc 后端（`u128_*`/`i128_*` 在内的 164 条） |
| `cargo forge gate --guards` | 结构守卫（含 `entity_tables`、规模棘轮、能力缺口单点） |
