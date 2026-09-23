#!/usr/bin/env bash
# 忽略规则表的可信度盘点（批次 346）。
#
# 钉四翼：
#   1 规则表本身必须是**文本**（批次 345 的账：含 NUL 的文件被 git 判成二进制，
#     此后每一次规则改动在 review 里都是一行 "Binary files differ"）。
#   2 手写源树必须**不被任何正向规则命中**（批次 346 的账：裸前缀规则 `test_*` /
#     `zeta_*` / `simple_*` … 连目录名都匹配，曾把 290 个已跟踪源文件压在规则底下）。
#   3 产物与根级 scratch 必须**仍然被忽略**（收窄不能把 build/stubs 放出来）。
#   4 族不变式：落在正向规则上的已跟踪文件，路径必须全部在允许清单里。
#
# 翼 2 与翼 3 互证：2 只断言"不该忽略的没被忽略"，单独看会被"check-ignore 整个坏了"
# 这种假绿满足；3 断言"该忽略的确实被忽略"，两者同时成立才说明判据在测东西。
#
# 用法：ZETAC 不需要；只读仓库。退出码 0=全绿，1=有 FAIL。
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT" || exit 1

checked=0
failed=0

ok()   { printf '  ok   %s\n' "$1"; checked=$((checked+1)); }
bad()  { printf '  FAIL %s\n' "$1"; checked=$((checked+1)); failed=$((failed+1)); }

# 一条路径当前被哪条规则命中（无命中返回非 0）。--no-index 是必须的：默认
# check-ignore 会跳过索引里的路径，那样就测不出"已跟踪文件其实压在规则底下"。
hit_rule() { git check-ignore -v --no-index -- "$1" 2>/dev/null; }

echo "== 翼 1：规则表必须是文本 =="
gi_bytes=$(wc -c < .gitignore | tr -d ' ')
nul=$(tr -dc '\000' < .gitignore | wc -c | tr -d ' ')
cr=$(tr -dc '\r' < .gitignore | wc -c | tr -d ' ')
if [[ $nul -eq 0 && $cr -eq 0 ]]; then
  ok ".gitignore 零控制字节（${gi_bytes} 字节，NUL=0 CR=0）"
else
  bad ".gitignore 混进控制字节：NUL=${nul} CR=${cr}"
fi
if git grep -I -q -e '^# ' -- .gitignore 2>/dev/null; then
  ok "git 把 .gitignore 当文本（git grep -I 可见）"
else
  bad "git 把 .gitignore 判成二进制 —— 规则改动不可 review"
fi

echo "== 翼 2：手写源树不得被命中 =="
visible=(
  zeta_src/algorithm.z
  src/tests/test_const_parser.rs
  src/runtime/zeta_runtime.rs
  src/bit_ops_test.z
  examples/quantum.z
  tests/unit/simple_compile_test.rs
  tests/unit/murphy_bitarray_rust.rs
  tests/python_style/t124_ternary.z
  .github/automation/check_agents.ps1
  zetas/capybara/build.z
)
for p in "${visible[@]}"; do
  [[ -e $p ]] || { bad "夹具不存在：$p"; continue; }
  h=$(hit_rule "$p")
  rule=${h%%$'\t'*}
  if [[ -z $h ]]; then
    ok "可见 $p"
  elif [[ $rule == *':!'* ]]; then
    # check-ignore -v 对"被反向规则重新纳入"的路径也会输出一行；这不是命中，
    # 恰恰是可见的证明。判据只认正向规则（不带 `!`）的命中。
    ok "可见 ${p}（由反向规则 ${rule##*:} 重新纳入）"
  else
    bad "仍被吞 $p ← $h"
  fi
done

echo "== 翼 3：产物与根级 scratch 必须仍被忽略 =="
# (路径, 期望命中的规则文本)
ignored_pairs=(
  "build/stubs/std.z|/build/**/*.z"
  "build/stubs/external/serde.z|/build/**/*.z"
  "test_scratch_probe.z|/test_*"
  "simple_scratch_probe.z|/simple_*"
  "zeta_scratch_probe.o|*.o"
  "scratch_probe.ps1|/*.ps1"
)
for pair in "${ignored_pairs[@]}"; do
  p=${pair%%|*}; want=${pair##*|}
  h=$(hit_rule "$p")
  rule=${h%%$'\t'*}                      # `file:line:pattern`，制表符后才是被测路径
  if [[ -z $h ]]; then
    bad "没被忽略（期望命中 ${want}）：$p"
  elif [[ ${rule##*:} == "$want" ]]; then
    ok "仍忽略 $p ← ${want}"
  else
    bad "被另一个答案忽略：$p ← ${h}（期望 ${want}）"
  fi
done

echo "== 翼 4：正向规则底下的已跟踪文件必须在允许清单内 =="
# 允许清单（批次 346 实测留下的 28 个，逐条都是"产物 / 备份 / 别处的 exclude"）：
#   ^build/          桩与再生文件        \.o$       根级已提交的目标文件（另一笔账）
#   \.backup$        手工备份             ^\.codegraph/  .git/info/exclude 的地盘
allow='^build/|\.o$|\.backup$|^\.codegraph/'
strays=$(git ls-files | git check-ignore -v --no-index --stdin 2>/dev/null \
         | awk -F'\t' '$1 !~ /:!/{print $NF}' | grep -Ev "$allow" || true)
neg_total=0
if [[ -z $strays ]]; then
  ok "无清单外的已跟踪文件被正向规则命中"
else
  neg_total=$(printf '%s\n' "$strays" | wc -l | tr -d ' ')
  bad "${neg_total} 个已跟踪文件压在规则底下且不在允许清单里："
  printf '%s\n' "$strays" | head -5 | sed 's/^/         /'
fi
pos_total=$(git ls-files | git check-ignore -v --no-index --stdin 2>/dev/null \
            | awk -F'\t' '$1 !~ /:!/' | wc -l | tr -d ' ')
printf '  注 正向命中 %s 个已跟踪文件（其中 %s 个不在允许清单里）；本翼在上限被突破时判红\n' "$pos_total" "$neg_total"

echo "== 汇总：${checked} 条断言，FAIL ${failed} =="
[[ $failed -eq 0 ]] || exit 1
exit 0
