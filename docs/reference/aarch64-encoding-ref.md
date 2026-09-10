# A64 整数核心指令编码参考（为 isa 表 / golden 测试收集）

<!-- markdownlint-configure-file { "MD013": { "line_length": 300, "code_block_line_length": 300, "heading_line_length": 300, "tables": false } } -->
<!-- 文件级豁免原因：本参考以位级编码文本（位域/条件码/指令十六进制）与官方指令页长 URL 为主，
     此类行不可断行（现有最长 267 列）；行宽放宽至 300 仅为容纳此类内容，
     一般叙述文字仍按 120 列折行。 -->

> 抓取方法说明（来源可用性）：developer.arm.com / support.arm.com 的 DDI0602/DDI0596 网页现为 JS 渲染 SPA，
> web_fetch 只能拿到壳（正文需浏览器执行）；官方 PDF 与 documentation-service 亦为 application/pdf（抓取器不支持）。
> 因此正文位图以 **等价值来源** 交叉验证：
> ① [armgen](http://www.scs.stanford.edu/~zyedidia/arm64/)＝由 **Arm 官方 ISA XML 2026-03_rel（2026-03-26 发布）** 生成的逐指令 HTML
> （页脚 "Copyright © 2010-2026 Arm Limited… Non-Confidential"，与 DDI0602/DDI0597 同源）——本表主位图依据，已逐族抓取核对；
> ② Linux 内核 `arch/arm64/include/asm/insn.h`（其注释直接引述 ARM ARM §C3.1 "AArch64 main encoding table" 及全族基址掩码）；
> ③ Ruby YJIT 编码器 `yjit/src/asm/arm64/inst/*.rs`（每个文件注释链到官方 DDI0602/DDI0596 指令页，单测里的常量在真实 CPU 上验证过）；
> ④ 官方 AAPCS64（ARM-software/abi-aa，2025Q4 / 2026-01-23）。
> 抓不到的官方页（DDI0602 2025-03~2025-09）仍列为引用 URL 并标注日期差异。
> 位段在 ARMv8.0→2026 稳定不变（整数核心），表内若无标注即为 64 位与 32 位共用位图（sf 控制）。
> 记号：`[a:b]=xxx`＝固定值/字段；基址对写作 `32位基/64位基`；imm 均为**逻辑值**（字段落位时按起始位左移）。

## 1. 总体格式与编码空间主分类

| 项 | 内容 |
| --- | --- |
| 定宽 | 每指令 32 位；小端存取；PC 相对偏移一律 = 立即数字段 × 4（跳转粒度 4B） |
| 主分类（官方 C3.1 主表，按 bits[28:25] 的 6 行，被 Linux insn.h 原样引述） | `00xx`→Unallocated；`100-`→Data processing-immediate；`101-`→Branch, exception & system；`-10-`→Loads and stores；`-101`→Data processing-register；`0111`/`1111`→Data processing-SIMD&FP。实际裁决还叠加 bits[31:29]（sf/op/S）与族内细分位，下表逐族给出完整固定模式 |
| DP-immediate 子组（bits[28:24]） | `10001` 加/减(imm12+sh)；`10000`+[30:29]=immlo ADR/ADRP；`100100`(23:23=0) 逻辑位掩码；`100101` MOV 宽；`100110` 位域(BFM/SBFM/UBFM)；`100111` EXTR |
| DP-register 子组 | `01011` 加/减(shifted reg)；`01010` 逻辑(shifted reg)；`110101`+[22:21]=00 条件选择(CSEL 族)；`111010010`+op[30] 条件比较(CCMP/CCMN)；`11010000` ADC/SBC；`11010110` 双源(UDIV/SDIV/LSLV…)；`11011000` 三源(MADD/MSUB) |
| 访存子组（bits[29:27]=111） | [26:24]=`001` 无符号 12 位缩放偏移；`000`+idx[11:10] 无缩放 imm9(pre/post/UR)；`011`+S 寄存器偏移寻址；`010` literal；成对 LDP/STP 为 [29:27]=`101`+index[24:22]；`000` 尾又分 LDX/STX/原子 |
| 分支子组（前 7-8 位定族） | B/BL `000101x`/`100101x`(bit31=BL)；B.cond `01010100`；CBZ/CBNZ `(sf)0110100/1`(bit24)；TBZ/TBNZ `(b5)011011(op)`(bit24)；BR/BLR/RET `1101011_00xx_11111_000000` |

## 2. 立即数数据处理（DP-immediate）

| 指令 | 位布局（bit: 字段=值/含义） | 语义一句话 | 基址(32/64) |
| --- | --- | --- | --- |
| ADD/ADDS (imm) | `[31]=sf 0=W/1=X; [30]=op=0; [29]=S(0/1); [28:24]=10001; [23:22]=sh: 00 无 / 01 LSL12; [21:10]=imm12(无符号0..4095); [9:5]=Rn; [4:0]=Rd` | Rd = Rn + (imm12<<(sh?12:0))；S=1 置 NZCV | 0x11000000 / 0x91000000 (S=1: 0x31000000/0xB1000000) |
| SUB/SUBS (imm) | 同上但 `[30]=op=1` | Rd = Rn − … | 0x51000000/0xD1000000 (S: 0x71000000/0xF1000000) |
| CMP/CMN (imm) | SUBS/ADDS 且 `[4:0]=Rd=31`（XZR 语义） | 只置标志：CMP=SUBS XZR,Rn,#imm；CMN=ADDS XZR,Rn,#imm | 同上（S=1 基址） |
| NEG (imm) 别名 | SUB Rd, XZR, #imm | Rd = 0 − imm | 0x51000000/0xD1000000 |
| MOVZ/MOVN/MOVK | `[31]=sf; [30:29]=opc: 00=MOVN 10=MOVZ 11=MOVK; [28:23]=100101; [22:21]=hw: 00/01/10/11=LSL 0/16/32/48; [20:5]=imm16; [4:0]=Rd` | 目标位段 ← imm16<<(hw*16)：MOVZ 其余清零、MOVN 取反、MOVK 保留其余 | MOVN 0x12800000/0x92800000；MOVZ 0x52800000/0xD2800000；MOVK 0x72800000/0xF2800000 |
| ADR | `[31]=op=0; [30:29]=immlo; [28:24]=10000; [23:5]=immhi(19位, 含符号位在 bit23); [4:0]=Rd` | 地址=PC+SignExtend(immlo:immhi,21)（±1MB，字节粒度） | 0x10000000 |
| ADRP | 同上 `[31]=op=1` | 地址=Page(PC)+SignExtend(immlo:immhi,21)<<12（±4GB，4KB 页粒度） | 0x90000000 |
| AND/ANDS/ORR/EOR (bitmask imm) | `[31]=sf; [30:29]=opc: 00=AND 01=ORR 10=EOR 11=ANDS; [28:23]=100100; [22]=N; [21:16]=immr; [15:10]=imms; [9:5]=Rn; [4:0]=Rd` | Rd = Rn op <位掩码>。N:immr:imms 解码：先找 (N:imms) 前导连续位得元素宽 W∈{2..64}(N=1→64；imms 全 1 未分配)，连续 1 数由低位决定，再整体 ROR immr，W<64 时按 W 复制铺满寄存器 | AND 0x12000000/0x92000000；ORR 0x32000000/0xB2000000；EOR 0x52000000/0xD2000000；ANDS 0x72000000/0xF2000000 |
| TST 别名 | ANDS Rd=XZR | Rn AND mask 置标志 | 0x72000000/0xF2000000 |
| LSL/LSR/ASR (imm) 别名 | UBFM/SBFM（见 §3 位域）：`LSL Rd,Rn,#s = UBFM immr=(-s mod D), imms=D-1-s`（s=0→MOV）；`LSR = UBFM immr=s, imms=D-1`；`ASR = SBFM immr=s, imms=D-1`（D=32/64，s=0..D-1；s=D 有特殊编码） | 立即数移位 | UBFM: sf,opc[30:29]=10,[28:23]=100110,N[22](64位=1),immr[21:16],imms[15:10]；SBFM opc=00；32 位基 0x13000000/0x53000000，64 位含 N=1：0x93400000/0xD3400000 |
| ROR (imm) 别名 | EXTR Rd,Rn,Rn,#s；EXTR：sf 00 [28:23]=100111 N Rm imm6/`imms[15:10]` Rn Rd（字段与 BFM 同排） | 循环右移 | 32/64 基 0x13800000/0x93C00000（EXTR 基址见 insn.h extr 0x13800000） |

## 3. 寄存器数据处理（DP-register）

| 指令 | 位布局 | 语义 | 基址(32/64) |
| --- | --- | --- | --- |
| ADD/ADDS/SUB/SUBS (shifted reg) | `[31]=sf; [30]=op(0 加/1 减); [29]=S; [28:24]=01011; [23:22]=sh: 00=LSL 01=LSR 10=ASR; (11 未分配); [20:16]=Rm; [15:10]=imm6(移位量 0..63); [9:5]=Rn; [4:0]=Rd` | Rd=Rn ± Shift(Rm,sh,imm6) | ADD 0x0B000000/0x8B000000；ADDS 0x2B/0xAB；SUB 0x4B/0xCB；SUBS 0x6B/0xEB |
| AND/BIC/ORR/ORN/EOR/EON/ANDS/BICS (shifted reg) | `[31]=sf; [30:29]=opc 00/01/10/11; [28:24]=01010; [21]=N(1=先取反 Rm → BIC/ORN/EON/BICS); [20:16]=Rm; [15:10]=imm6; [9:5]=Rn; [4:0]=Rd` | Rd = Rn op (N? ~Rm:Rm)；逻辑操作 | AND 0x0A000000/0x8A000000；BIC 0x0A200000/0x8A200000；ORR 0x2A000000/0xAA000000；ORN 0x2A200000/0xAA200000；EOR 0x4A000000/0xCA000000；EON 0x4A200000/0xCA200000；ANDS 0x6A000000/0xEA000000；BICS 0x6A200000/0xEA200000 |
| MOV(reg)/MVN/NEG/NEGS/TST/CMP 别名 | MOV=ORR Rd,XZR,Rm；MVN=ORN Rd,XZR,Rm；NEG=SUB Rd,XZR,Rm；NEGS=SUBS；CMP=SUBS XZR；TST=ANDS XZR | 寄存器搬运/取反/取负/比较 | ORR 0xAA000000；ORN 0xAA200000；SUB 0xCB000000 等（含 Rm<<16、Rn=31<<5） |
| LSLV/LSRV/ASRV/RORV | `sf 00 [28:21]=11010110; [20:16]=Rm(移位量寄存器,取低 5/6 位); [15:10]=op2: 001000=LSLV 001001=LSRV 001010=ASRV 001011=RORV; [9:5]=Rn; [4:0]=Rd` | 按寄存器移位 | 32 位基 0x1AC02000/24/28/2C00；64 位加 bit31=0x9AC0… |
| MADD/MSUB (+MUL/MNEG 别名) | `[31]=sf; [30:29]=00; [28:21]=11011000; [20:16]=Rm; [15]=o0=0 MADD /1 MSUB; [14:10]=Ra; [9:5]=Rn; [4:0]=Rd` | Rd = Ra ± Rn×Rm（截断）；MUL=Ra=XZR(0x…7C00)；MNEG=MSUB Ra=XZR | MADD 0x1B000000/0x9B000000；MSUB 0x1B008000/0x9B008000 |
| UDIV/SDIV | `[31]=sf; [30:29]=00; [28:21]=11010110; [20:16]=Rm(除数); [15:10]=op2: 000010=UDIV 000011=SDIV; [9:5]=Rn(被除数); [4:0]=Rd` | Rd=Rn÷Rm；除 0 得 0；SDIV INT_MIN/−1 得 INT_MIN；不置标志 | 32: 0x1AC00800/0x1AC00C00；64: 0x9AC00800/0x9AC00C00 |
| 宽乘 SMADDL/UMADDL/SMSUBL/UMSUBL 等 | 同 MADD 三源布局但固定 64 位结果（sf 恒 1，源 W 32×32） | 后置实现（本表先不做） | 0x9B200000/0x9BA00000/…（另行核对） |
| BFM/SBFM/UBFM 族（ASR/LSR/LSL/SXTB/SXTH/SXTW/UXTB/UXTH/BFI/BFXIL/SBFX/UBFX 的载体） | `[31]=sf; [30:29]=opc: 00=SBFM 01=BFM 10=UBFM; [28:23]=100110; [22]=N(64 位=1); [21:16]=immr; [15:10]=imms; [9:5]=Rn; [4:0]=Rd` | 位域搬移；别名见 §2/§10 | 32: SBFM 0x13000000 BFM 0x33000000 UBFM 0x53000000；64(N=1): 0x93400000/0xB3400000/0xD3400000 |

## 4. 条件选择 / 条件比较

| 指令 | 位布局 | 语义 | 基址(32/64) |
| --- | --- | --- | --- |
| CSEL | `[31]=sf; [30]=op=0; [29]=0; [28:23]=110101; [22:21]=00; [20:16]=Rm; [15:12]=cond; [11:10]=op2=00; [9:5]=Rn; [4:0]=Rd` | Rd = cond 真 ? Rn : Rm | 0x1A800000/0x9A800000 |
| CSINC | 同上 `[11:10]=op2=01` | Rd = cond ? Rn : Rm+1 | 0x1A800400/0x9A800400 |
| CSINV | 同上 `[30]=op=1; [11:10]=00` | Rd = cond ? Rn : ~Rm | 0x5A800000/0xDA800000 |
| CSNEG | 同上 `[30]=1; [11:10]=01` | Rd = cond ? Rn : −Rm | 0x5A800400/0xDA800400 |
| CSET/CSETM/CINC/CINV/CNEG | CSET=CSINC Rd,ZR,ZR；CSETM=CSINV Rd,ZR,ZR；CINC=CSINC Rd,Rn,Rn；CINV=CSINV Rd,Rn,Rn；CNEG=CSNEG Rd,Rn,Rn | 别名：按条件产生 1/0、全 1/0、+1、~、− | 同族基址 |
| CCMP/CCMN (register) | `[31]=sf; [30]=op: 0=CCMN 1=CCMP; [29:21]=111010010; [20:16]=Rm; [15:12]=cond; [11:10]=o2=00; [9:5]=Rn; [4]=0; [3:0]=nzcv` | cond 真→按 Rn±Rm 置 NZCV；假→NZCV=nzcv | CCMN 0x3A400000/0xBA400000；CCMP 0x7A400000/0xFA400000 |
| CCMP/CCMN (immediate) | 同上但 `[20:16]=imm5`(0..31)，`[11:10]=o2=10` | cond 真→按 Rn±imm5 置标志 | 同上基址 + 0x800（o2 位） |
| ADC/ADCS/SBC/SBCS | `[31]=sf; [30]=op: 0=ADC 1=SBC; [29]=S; [28:21]=11010000; [20:16]=Rm; [15:10]=000000; [9:5]=Rn; [4:0]=Rd` | Rd=Rn+Rm+C（SBC：Rn−Rm−1+C） | ADC 0x1A000000/0x9A000000；ADCS 0x3A/0xBA；SBC 0x5A/0xDA；SBCS 0x7A/0xFA |

条件码（cond，4 位，NZCV 组合）：EQ 0 / NE 1 / CS(HS) 2 / CC(LO) 3 / MI 4 / PL 5 / VS 6 / VC 7 / HI 8 / LS 9 / GE 10 / LT 11 / GT 12 / LE 13 / AL 14 / NV 15（NV 无新语义，行为同 AL）。

## 5. 加载/存储（ABI / 帧 / 栈槽重点；先聚焦整数、无缩放 imm 与成对）

| 指令 | 位布局 | 语义/范围 | 基址示例(64 位取 X) |
| --- | --- | --- | --- |
| LDR/STR (unsigned imm12, scaled) | `[31:30]=size: 00=B 01=H 10=W 11=X; [29:27]=111; [26:24]=001; [23:22]=opc: 00=STR 01=LDR 10=LDRSB/LDRSH/LDRSW(sign-ext) 11=PRFM; [21:10]=imm12(地址偏移=imm12<<scale: B<<0,H<<1,W<<2,X<<3); [9:5]=Rn; [4:0]=Rt` | [base,#imm] 不回写。X: ±? 0..32760(步8)，W: 0..16380，H: 0..8190，B: 0..4095（imm12 无符号，64 位范围 0–32760） | STR X 0xF9000000；LDR X 0xF9400000；LDR W 0xB9400000；LDRSW 0xB9800000；LDRB 0x39400000；LDRH 0x79400000；STRB 0x39000000；STRH 0x79000000 |
| LDR/STR (imm9: unscaled/pre/post) | `[31:30]=size; [29:27]=111; [26:24]=000; [23:22]=opc(同左,10=带符号 LDURSW 等); [21]=0; [20:12]=imm9 有符号(−256..255 字节); [11:10]=idx: 00=LDUR/STUR 01=post([Xn],#i) 10=未分配 11=pre([Xn,#i]!); [9:5]=Rn(base, 允许 SP); [4:0]=Rt` | pre/post 访问后把 Xn=addr±imm9 写回；LDUR 供非缩放偏移(汇编器在 imm 不能缩放时自动选) | LDUR X 0xF8400000；STUR X 0xF8000000；LDURSW 0xB8800000；LDR X post 基 0xF8400400、pre 基 0xF8400C00（idx 位） |
| LDP/STP (offset) | `[31:30]=opc: 00=32 位对 10=64 位对(01/11=FP/SIMD); [29:27]=101; [26:25]=00; [24:22]=index: 100=STP off 101=LDP off; [21:15]=imm7 有符号(×4 W 对 / ×8 X 对; X: −512..504); [14:10]=Rt2; [9:5]=Rn; [4:0]=Rt1` | 相邻两寄存器访问；[base,#imm] 不回写 | STP X 0xA9000000；LDP X 0xA9400000（W 对: 0x29000000/0x29400000） |
| LDP/STP (pre/post) | 同上 `[24:22]=110/010=STP pre/post; 111/011=LDP pre/post` | [Xn,#imm]! / [Xn],#imm 写回 Xn；Rt1/Rt2 不得与 Xn 相同 | STP pre 0xA9800000；LDP pre 0xA9C00000；STP post 0xA8800000；LDP post 0xA8C00000（W 对去 bit31） |
| LDR literal | `[31:30]=opc: 00=LDR W 01=LDR X 10=LDRSW 11=PRFM; [29:24]=011000; [23:5]=imm19 有符号×4(±1MB); [4:0]=Rt` | 从 PC 相对字面量加载（数据池） | 0x18000000/0x58000000/0x98000000/0xD8000000 |
| MOV (to/from SP) | ADD(imm) 别名，Rn 或 Rd=31(SP)：`mov x0,sp=add x0,sp,#0`；`mov sp,x0=add sp,x0,#0`；`add sp,sp,#i` | 栈指针搬运/加常数 | 0x91000000 族（SP=31 编码） |
| SP 使用规则 | 访存 Rn(base)、LDP/STP Rn、add/sub(imm,S=0) 的 Rn/Rd 可用 SP(31)；其余位置 31=XZR/WZR；SP 读入寄存器必须走 `mov xN,sp`(add 别名)，XZR 不能作地址 | | |
| 未列入(后置) | 寄存器偏移寻址 [Xn,Rm]{,ext}、LDTR/STTR、LDXR/STXR/LDAXR/STLXR、原子、acquire/release、浮点/SIMD 访存 | | |

## 6. 分支

| 指令 | 位布局 | 范围 | 基址 |
| --- | --- | --- | --- |
| B / BL | `[31]=op: 0=B 1=BL; [30:26]=00101; [25:0]=imm26×4` | ±128MB | 0x14000000 / 0x94000000 |
| B.cond | `[31:24]=01010100; [23:5]=imm19×4; [4]=0; [3:0]=cond` | ±1MB | 0x54000000（`b.ne`=0x54000001） |
| CBZ/CBNZ | `[31]=sf; [30:25]=011010; [24]=op: 0=CBZ 1=CBNZ; [23:5]=imm19×4; [4:0]=Rt` | ±1MB，不读标志 | CBZ 0x34000000/0xB4000000；CBNZ 0x35000000/0xB5000000 |
| TBZ/TBNZ | `[31]=b5(被测位 bit5); [30:25]=011011; [24]=op: 0=TBZ 1=TBNZ; [23:19]=b40(被测位低5位); [18:5]=imm14×4; [4:0]=Rt` | ±32KB，不读标志 | 0x36000000 / 0x37000000 |
| BR/BLR/RET | `[31:25]=1101011; [23:21]=op2: 000=BR 001=BLR 010=RET; [20:16]=11111; [15:10]=000000; [9:5]=Rn; [4:0]=00000` | 绝对跳寄存器；BLR 把返回地址存 X30；RET 同 BR+返回提示 | BR 0xD61F0000；BLR 0xD63F0000；RET 0xD65F0000，均 `Rn<<5`；`ret`(无操作数)=RET X30=0xD65F03C0 |

## 7. 系统 / 杂项（本表仅列基准，编码细节后置）

NOP=**0xD503201F**（HINT #0）；HINT 其余(ESB/PAC/BTI…)为 0xD503201F 改 [19:12]/[11:5] 域。SVC/HVC/SMC=0xD4000001/2/3；BRK=0xD4200000|imm16<<5；ERET=0xD69F03E0；MRS=0xD5300000 族、MSR(reg)=0xD5100000、MSR(imm)=0xD500401F、DMB/DSB/ISB≈0xD50330BF/9F/DF。RET 在 §6。禁止 MSR/MRS/SVC 表外细节暂不展开。

## 8. 已核实编码常量样例（golden 对照；来源标注 [Y]=YJIT 单测常量(真实 CPU 验证) [K]=Linux insn.h 基址 [A]=armgen(官方 XML 位图推得) [C]=通用编译器常量(经典)）

| 汇编 | 编码 | 来源 |
| --- | --- | --- |
| add x0,x1,x2 | 0x8B020020 | [Y]（移位寄存器, LSL#0） |
| add w0,w1,w2 / adds / sub / subs (w/x 同型) | 0x0B020020 / 0xAB020020(x) / 0xCB020020 / 0xEB020020 | [Y] |
| add x0,x1,#7 / adds x0,x1,#7 / sub x0,x1,#7 / subs x0,x1,#7 | 0x91001C20 / 0xB1001C20 / 0xD1001C20 / 0xF1001C20 | [Y] |
| cmp x0,x1 / cmp x0,#7 / cmn | 0xEB01001F / 0xF1001C1F / adds xzr(0xB1…1F) | [Y] |
| sub sp,sp,#16 / add x29,sp,#0(mov x29,sp) / mov x0,sp | 0xD10043FF / 0x910003FD / 0x910003E0 | [C] |
| movz x0,#123 / movk x0,#123 | 0xD2800F60 / 0xF2800F60 | [Y] |
| movn x0,#0（≡ mov x0,#−1）/ movn w0,#0 | 0x92800000 / 0x12800000 | [K]+推得 |
| mov x0,x1 / mov x0,x0(自) | 0xAA0103E0 / 0xAA0003E0 | [C] |
| neg x0,x1 / mvn x0,x1 | 0xCB0103E0 / 0xAA2103E0 | [C] |
| lsl x0,x1,#7 / lsr x0,x1,#7 / asr 同 lsr(换 SBFM) | 0xD379E020 / 0xD347FC20 | [Y] |
| csel x0,x1,x2,ne | 0x9A821020 | [Y] |
| cset w0,ne / cset w0,eq | 0x1A9F17E0 / 0x1A9F03E0 | [C](=CSINC wzr,wzr) |
| ccmp x0,#0,#0,eq（imm 形式） | 0xFA400800（按 [A] 位图推得；nzcv/cond 可变位另加） | [A] |
| ldr x0,[x1] / str x0,[x1] / ldr w0,[x1] | 0xF9400020 / 0xF9000020 / 0xB9400020 | [K]+公式 |
| ldr x0,[x1,#8] | 0xF9400420（imm12=8>>3=1） | 公式 |
| ldr x0,[x1],#16 (post) / ldr x0,[x1,#16]! (pre) | 0xF8410420 / 0xF8410C20 | [Y] |
| ldur x0,[x1] / ldurb w0,[x1] / ldurh / ldursw | 0xF8400020 / 0x38400020 / 0x78400020 / 0xB8800020 | [Y] |
| ldp x0,x1,[x2] / stp x0,x1,[x2] | 0xA9400440 / 0xA9000440 | [Y] |
| stp x29,x30,[sp,#−16]! / ldp x29,x30,[sp],#16 | 0xA9BF7BFD / 0xA8C17BFD | [C] |
| adr x0,#0 / adrp x0,#0x4000 / adrp x0,#0 | 0x10000000 / 0x90000020 / 0x90000000 | [Y] |
| b .(自身) / bl . / b.eq +128B(imm19=32) | 0x14000000 / 0x94000000 / 0x54000400 | [K]/[Y] |
| cbz w0,label@0 / cbnz x0,label@0 | 0x34000000 / 0xB5000000 | [K] |
| tbz x0,#0,0 / tbnz x0,#0,0 | 0x36000000 / 0x37000000 | [Y] |
| br x0 / blr x0 / ret x30 / ret x20 | 0xD61F0000 / 0xD63F0000 / 0xD65F03C0 / 0xD65F0280 | [Y] |
| nop / svc #0 / eret | 0xD503201F / 0xD4000001 / 0xD69F03E0 | [K] |
| mul x0,x1,x2 / udiv x0,x1,x2 | 0x9B027C20 / 0x9AC20820（madd Ra=XZR / [A] 布局推得） | [A]+公式 |
| and x0,x1,x2 / orr x0,x1,x2 | 0x8A020020 / 0xAA020020（shifted-reg 逻辑族） | [K]+公式 |
| ldr x0,literal(off 0) | 0x58000000 | [K] |

## 9. A64 寄存器命名与规则

- 通用寄存器 0–30：64 位名 X0–X30（W 名 W0–W30 访问低 32 位；**写 W 寄存器自动把高 32 位清零**）。
- **编号 31 依语境**：能做"栈指针操作"的位置（访存 base/Rn、LDP/STP base、add/sub(immediate) 的 Rn/Rd 且 S=0、`mov`/`add` SP 别名）＝ SP/WSP；其它位置＝ XZR/WZR（读数恒 0，写被丢弃）。移位寄存器 ADD/逻辑/乘法等一律 ZR 语义。
- x29=帧指针 FP（AAPCS 中为 callee-saved）、x30=链接寄存器 LR；`bl`/`blr` 自动写 X30。
- PC 不能直接读/写/寻址——地址仅能经 adr/adrp/literal/分支隐式获得。
- 条件标志 NZCV 由 CMP/CMN/TST/ADDS/SUBS/ANDS/CCMP/ADC/SBC 等设置；cond 4 位表见 §4。
- SP 在任何被使用时必须 16 字节对齐（AAPCS64 要求全程 SP mod 16 = 0）；栈向下增长。
- AAPCS64（官方，2025Q4 版）：参数/返回值 r0–r7（浮点 v0–v7）；r8=间接结果寄存器；r9–r15 调用者保存；r16=IP0、r17=IP1（PLT/veneers 可用，属调用者保存）；r18=平台专用；**r19–r28 被调用者保存**（x29 亦被保存）；x30=LR；帧记录=两个 8B：[FP]→上一帧 FP、[FP+8]→进入时的 LR（用指针签名时先签名再存）。

## 10. 汇编语法别名要点

- `cmp x0,#1` = SUBS XZR,X0,#1（rd 域=31）；`cmn`=ADDS XZR；`tst`=ANDS XZR。
- `mov x0,#imm`＝能 MOVZ 就 MOVZ（大值需 movz+movk 组合，汇编器自动）；`movn` 用于负数；`mov x0,x1`=ORR X0,XZR,X1；`mvn`=ORN；`neg`=SUB rd,xzr,rm；`negs`=SUBS。
- `mov x0,sp`/`mov sp,x0`＝ADD(imm) 别名（#0）；`mov x0,xzr` 用 ORR。
- `ret`(无操作数)=RET X30；`ret x5` 等价显式 Rn。
- `mul x0,x1,x2`=MADD X0,X1,X2,XZR；`mneg`=MSUB Ra=ZR；`cinc/cset/csetm/cinv/cneg` 见 §4 别名。
- `lsl/lsr/asr #s` 是 UBFM/SBFM 别名（§2/§3）；`ror`=EXTR；寄存器版本 LSLV/LSRV/ASRV/RORV 是真指令；`sxtb/sxth/sxtw/uxth/…/bfi/bfxil/sbfx/ubfx` 皆 BFM 族别名。
- 访存：`ldur`/`stur` 供汇编器在偏移无法缩放编码时回退；`ldrsw`＝word 符号扩展进 X；字节/半字载入 W 寄存器自动零扩展，符号扩展需 LDRSB/LDRSH。
- B.cond 只有 AL 之前的条件可写；`cbz` 不读 NZCV。

## 11. 资料 URL（全部为本文引用来源）

官方（网页正文需 JS，仅能确认存在与版次；内容与上表经 XML/镜像交叉验证）：

- <https://developer.arm.com/documentation/ddi0602/2025-03/Base-Instructions/ADD--immediate---Add-immediate-value->  （DDI0602「A-profile A64 ISA」指令页，2025-03；同日档 2025-06/2025-09 亦存在，见 CCMP 页）
- <https://developer.arm.com/documentation/ddi0602/2025-03/Base-Instructions/CCMP--immediate---Conditional-compare--immediate--> ；…/2025-06/…；…/2025-09/Base-Instructions/CCMP--register---Conditional-compare--register--
- <https://developer.arm.com/documentation/ddi0602/2020-12/…（旧版存档页仍被引用）>
- DDI0596「Arm A64 ISA（Base Instructions 前身）」/ DDI0597：同一内容老编号，URL 形如 developer.arm.com/documentation/ddi0596/2021-12/Base-Instructions/…

等价/派生源（实际抓取正文）：

- armgen（官方 ISA XML 2026-03_rel 生成，逐指令位图；页脚 Copyright Arm）：
  <http://www.scs.stanford.edu/~zyedidia/arm64/ccmp_imm.html> 、ccmp_reg.html 、ccmn_imm.html 、udiv.html 、madd.html （同一站点另有全部 Base/SIMD/SVE 指令与 Index-by-Encoding 页）
- Linux 内核 insn.h（注释引述 ARM ARM §C3.1 主表；含全族基址掩码/条件码/寄存器 31 语义）：
  <https://raw.githubusercontent.com/xanmod/linux/master/arch/arm64/include/asm/insn.h> （官方镜像 <https://gitlab.arm.com/linux-arm/linux-ak/-/raw/…/arch/arm64/include/asm/insn.h）>
- Ruby YJIT A64 编码器（每文件注释链官方 DDI0596/DDI0602 页，单测常量在真机验证）：
  <https://github.com/ruby/ruby/tree/master/yjit/src/asm/arm64/inst> （mov.rs / branch.rs / branch_cond.rs / conditional.rs / data_imm.rs / data_reg.rs / pc_rel.rs / load_store.rs / reg_pair.rs / shift_imm.rs / test_bit.rs 已抓取核对）
- AAPCS64 官方（2025Q4，2026-01-23 发布）：<https://raw.githubusercontent.com/ARM-software/abi-aa/main/aapcs64/aapcs64.rst>
- 中文入门佐证（非官方转译，仅概念/别名）：
  <https://armv8-doc.readthedocs.io/en/latest/06.html> （周贺贺《Armv8/Armv9 架构入门指南》）

抓取失败的通道（均已尝试）：support.arm.com SPA 正文、documentation-service.arm.com（PDF）、csci.viu.ca ARM ARM C4 章 PDF、r.jina.ai 渲染代理、web.archive.org、go.googlesource.com。

## 实现状态（2026-09，isa/arm64_v12.toml）

- 已实现：整数 ALU(imm/reg ± 与逻辑)、MOV 别名、CMP/CMN、MOVZ/MOVN(hw0)/
  MOVK(hw0) + **hw 变体（MOVZX1/2/3、MOVZW1、MOVKX1/2，clang oracle
  d2b579a0/d2d579a0/d2f579a0/52b579a0/f2b579a0/f2d579a0 逐字对照）**、
  **Iconst 全值域值分派**（attr `iconst` = 常量池解析真值——Iconst 的 `imm0`
  是 ConstId 池索引，无值语义，P3① 实证 imm0<0 规则永不命中）：16 位域内
  |v|<0x10000 → 单条 MOVZ（正）/MOVN（负，`{iconst_c16}`=~v，movn 高位全 1
  = 符号扩展）；域外 → X 恒 4 条 movz(hw3)+movk(hw2/1/0)、W 恒 2 条
  movz(hw1)+movk(hw0)（分片占位符 `{iconst_f0..f3}`，两补码位型逐片构造，
  任意 i64/i32 常量可表示）、乘除(MADD/MSUB/MUL/SDIV/UDIV)、分支
  (B/BL/BR/BLR/RET/CBZ/CBNZ)、CSEL 族、LDR/STR/LDUR/STUR、LDP/STP、
  SP/XZR(31) 语义；[abi]/[emit]/[spill]/[[lowering]] TargetMachine 全链（P2）；
  jit_matrix runner + QEMU aarch64 semihosting 真执行（P3；值域过滤放宽至
  [-128,127]，return_negative/return_minus_one 转绿，矩阵 23 passed；大常量
  e2e：return 0x12345678 / -1_000_000_007 QEMU 真执行低 8 位验证；run_qemu
  临时 ELF 文件名加进程内原子序号——修并行测试文件串扰）、**多块控制流
  （P3②）**：B 角色 jump/epilogue_jump（bare 定宽跳，仅 label 槽）、CBZX
  角色 branch（cbz cond,false → b true；终结符按指令字段形状自适应分派）、
  [emit].epilogue_label=true（return block 经 `b epilogue_label` 跳统一尾声，
  多 return block 安全）；Arm64RelocPatcher 判词改 top6（B/BL imm26 占
  [25:0]，初编码 label 占位污染 top8——word 0x17FFFFFD top8=0x17 也要识别）；
  e2e：if/else + phi merge 双翼 QEMU 真执行（40/2）。
- 已提交（P3 其余线）：reloc patcher（ddb6d8e）+ ② 多块 e2e（本系列）、
  forge-rustc aarch64 注册（d78debd，能力边界注释在 compile.rs）、CI
  qemu-system-arm 门禁（dcb42e7，Test(Linux) 装 qemu-system-arm 提供
  qemu-system-aarch64）。
- 待做（下一阶段）：Icmp lowering（cset/cond 码）使矩阵 Block 用例
  （conditional_branch/loop/if_else_chain）转正；跨函数 Call（BL）经
  reloc patcher + Call lowering 路径。
- 验证基准：本机 LLVM clang --target=aarch64-none-elf + llvm-objdump
  oracle 逐字对照 + tm 测试 + QEMU 矩阵真跑（钉版 nightly-2026-09-05）。
