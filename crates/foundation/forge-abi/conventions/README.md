# 约定绑定（conventions）

本目录是 **A1 的参考数据**：把 `builtin` 里的**平台无关规则**绑到具体 ISA 的寄存器上。
它们是 `include_str!` 进 `forge_abi::builtin::bindings()` 的**随 crate 数据**，
CLI（`forge-isa abi …`）与集成测试都用同一份，避免"文档一套、测试一套"。

## 文件与覆盖关系

| 文件 | ISA 名 | 约定名 | 参数寄存器（int / float） | 间接结果指针 | callee-saved |
| --- | --- | --- | --- | --- | --- |
| `win64-x86_64.toml` | `x86_v12` | `win64` | RCX-R9 / XMM0-3（按位置共用游标） | RCX（`int` 池 0 槽） | RBX,RDI,RSI,R12-R15 |
| `sysv64-x86_64.toml` | `x86_v12` | `sysv64` | RDI,R9 序 / XMM0-7（按类各计数） | RDI（`int` 池 0 槽） | RBX,R12-R15 |
| `aapcs64-arm64.toml` | `arm64_v12` | `aapcs64` | X0-X7 / **无浮点池（缺口）** | **X8（独立池）** | X19-X28 |
| `lp64d-riscv64.toml` | `riscv64_v12` | `lp64d` | X10-X17 / F10-F17（按类各计数） | X10（`int` 池 0 槽） | X9,X18-X27 |

三条与"从谱里抄 `[abi]`"不同的约定值得写下来：

1. **帧指针不列进 `cs_gpr`**：x86 的 RBP、riscv 的 X8、arm64 的 X29 都由帧件保存，
   规则侧用 `callee_saved.includes_fp` / `includes_link` 表达"它也被保存"。
   列进池里会让"保存谁"被算两遍。
2. **`sret` 有独立池时才独立**：AAPCS64 的 x8 不占用户参数序列，所以单开 `sret` 池；
   x86/riscv 的间接结果指针**就是第 0 个整数参数槽**（RCX / RDI / X10），走 `int` 池 + `sret_slot = 0`。
   这一条正是旧实现"永远取首 int 槽"能碰对 x86、碰上 arm64 必错的原因。
3. **池是懒解析的**：没被任何签名用到的池可以不写，也不会有错；一旦规则要求它，
   引擎立刻报 `MissingPool`。arm64 缺浮点池就是这种"显式缺口"。

## 已知缺口（A5/A6 关闭）

- **arm64 没有 FPR/VEC 寄存器组**：`float` 池欠奉 → 浮点/HFA 参数规划时报 `MissingPool`
  （`forge-isa abi check isa/arm64_v12.toml` 会如实列出这批 GAP）。
  补齐 = 谱里加 `[reg.fpr8]`（`V0..V31`）+ `fpr_mov`/`vec_mov` 角色的指令声明。
- **SysV 的 eightbyte 分类未建模**：≤16B 聚合统一按两个 `int` 槽（真实 SysV 会按成员
  拆到 SSE），见 `builtin::SYSV64` 的 `note`。
- **变参的 LEN 类元信息寄存器未启用**：模型里有 `hidden.va_len_pool` 这一格，
  但**没有**内置约定打开它（psABI 现状以官方定本为准，A6 逐条核对后再定），
  因此也没有哪个绑定给 `va_len` 池。引擎支持它（`tests/invariants.rs` 用一份
  自定义约定覆盖这条路径）。

## 加自己的约定

```toml
# my-conv.toml —— 规则（平台无关）
name = "myconv"
parent = "c"
position = "by_class"
stack = { slot_bytes = 8, first_offset_slots = 1 }
classify = [ { when = { kind = "scalar" }, do = { direct = { pool = "int" } } } ]
```

```toml
# my-binding.toml —— (ISA, 约定) 绑定
isa = "my_isa"
conv = "myconv"
[pools]
int = ["R10", "R11"]   # 名字或该 ISA 的寄存器号（整数）都行
```

命令行用法与"注册表未注册即报错"的语义见
[`docs/reference/calling-conventions.md`](../../../../docs/reference/calling-conventions.md)。
