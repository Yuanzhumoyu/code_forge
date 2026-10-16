# 覆盖率下限棘轮判定（v26 M3）。
#
# 用法（CI 与本地同一条命令）：
#   awk -f docs/performance/coverage_floor.awk docs/performance/coverage_floor.txt lcov.info
#
# 输入：① 下限文件（首行一个数，百分数）；② `cargo llvm-cov --lcov` 产出的 lcov。
# 判据：全文件 `LF:`（可执行行）与 `LH:`（命中行）求和 ⇒ 行覆盖率 < 下限 ⇒ 退出码 1。
# 为什么用 awk 而不是 `cargo llvm-cov report --fail-under-lines`：后者要求 profdata
# 仍在（实测本地缺 profdata 时读成 0/0，判定失去意义）；直接对 lcov 求和则不依赖它。
BEGIN { floor = 0 }

# 下限文件是**第一个**输入文件：取它的第一条记录（`NR == 1`，而不是按文件名匹配——
# 按名字匹配时换一个文件名（如临时验证用的 floor95.txt）会静默读成 0，实测撞到）。
NR == 1 { floor = $1 + 0 }

FILENAME ~ /lcov\.info$/ {
  if ($0 ~ /^LF:/) { split($0, a, ":"); lf += a[2] + 0 }
  if ($0 ~ /^LH:/) { split($0, a, ":"); lh += a[2] + 0 }
}

END {
  pct = (lf == 0) ? 0 : 100 * lh / lf
  printf "行覆盖率 = %.2f%%（LF=%d LH=%d，下限 %d%%）\n", pct, lf, lh, floor
  if (lf == 0) { print "错误：lcov 里没有 LF 记录（覆盖率数据缺失）"; exit 1 }
  if (pct < floor) { print "低于下限"; exit 1 }
  print "达标"
}
