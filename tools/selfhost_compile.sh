#!/usr/bin/env bash
# tools/selfhost_compile.sh — zeta_src/ 全量编译判据（批次 347 / backlog #29）
#
# 为什么需要它（原 CI 这一步是假绿）：
#   ci.yml 的 `zeta-selfhost` job 里有两个说谎的步骤——
#   ① `Compile all zeta_src/ files` 用的是 `for f in zeta_src/*.z`：zeta_src 共 51 个
#      `.z`（其余在 middle/ frontend/ runtime/ 等子目录），这个 glob 只覆盖**顶层 5 个**，
#      却在步骤名里写 "all"。
#   ② `Run known-good tests` 引用 `tests/test_hello.z` / `tests/test_values.z` /
#      `tests/test_basic.z` —— **三个文件都不存在**；命令还用 `--jit`，而 `main.rs` 里
#      根本没有这个标志。外面套 `if [ -f "$f" ]` ⇒ 循环体一次都不执行、静默跳过，
#      末行却无条件打印 "Self-host compilation verified: zeta_src/ compiles cleanly"。
#      即：这一步什么都没验证，却给出已验证的结论。
#
# 本脚本把这一步变成真判据：
#   - 覆盖 `zeta_src/**/*.z` 全部文件（不靠 glob 的层级假设）
#   - 用 `--no-link -o` 只编译不执行（CLI 不带 `-o` 会 JIT **执行**被编译的程序，
#     批次 344 已把这条收窄；判据不该有副作用）
#   - 已知失败**逐条钉住并写明原因**（不是"允许失败"，是"登记在案"）
#   - 新失败 ⇒ 判红；钉住项变通过 ⇒ 也判红并提示把该行删掉（清单只能缩，不能烂）
#
# 用法：tools/selfhost_compile.sh [--full]（--full 打印每个文件的完整 stderr）
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
FULL=0
[[ "${1:-}" == "--full" ]] && FULL=1

if [[ ! -x "$ZETAC" ]]; then
  echo "selfhost: 找不到可执行的 zetac（${ZETAC}）——先 cargo build --release -p zetac" >&2
  exit 2
fi

# 已知失败（键 = 相对仓库根的路径；值 = 一句话原因）。
# 加行必须同时写明原因与来源批次；条目变通过时要删行（下面有断言盯着）。
KNOWN_FAIL="
zeta_src/runtime/array.z|解析已修（旁路批次 460：模式解析器补 &/&mut 引用模式臂）但暴露下一层：codegen panic codegen.rs:6759 exprs 索引（SemiringFold 操作数 id 未注册，MIR 实测 12 节点）；移交主线（gen/codegen 车道）
zeta_src/runtime/actor/map.z|LLVM verifier：返回类型不匹配（ret i64 vs ptr）
zeta_src/runtime/actor/result.z|同上 + 调用参数类型不匹配（host_result_free_1）
"

known_hit=0
ok=0; pinned=0; newfail=0; fixed=0
fail_list=""
fixed_list=""

is_known() { printf '%s\n' "$KNOWN_FAIL" | awk -F'|' -v k="$1" '$1==k{print $2}' | head -1; }

while IFS= read -r f; do
  rel="${f#./}"
  reason=$(is_known "$rel")
  out=/tmp/zsh_out_$$
  diag=$(timeout 120 "$ZETAC" "$f" --no-link -o "$out" 2>&1); rc=$?
  rm -f "$out"
  if [[ $rc -eq 0 ]]; then
    if [[ -n "$reason" ]]; then
      fixed=$((fixed+1)); fixed_list="$fixed_list $rel"
      echo "  FIXED ${rel}  —— 已知失败已转通过，请从清单删行（原记：${reason}）"
    else
      ok=$((ok+1))
    fi
  else
    if [[ -n "$reason" ]]; then
      pinned=$((pinned+1)); known_hit=$((known_hit+1))
    else
      newfail=$((newfail+1))
      first=$(printf '%s' "$diag" | grep -viE '^clang:|^warning: PY-A' | head -1 | cut -c1-110)
      echo "  FAIL ${rel}  rc=${rc}  << ${first}"
      fail_list="$fail_list $rel"
    fi
    if [[ $FULL -eq 1 ]]; then
      printf '%s\n' "$diag" | sed 's/^/       /' | head -20
    fi
  fi
done < <(find zeta_src -name '*.z' | sort)

total=$((ok + pinned + newfail))
printf 'selfhost: 共 %s 个文件 —— 通过 %s，已登记失败 %s，新失败 %s，已修复待摘 %s\n' \
  "$total" "$ok" "$pinned" "$newfail" "$fixed"
[[ $newfail -gt 0 ]] && echo "新失败清单：${fail_list}" >&2
[[ $fixed -gt 0 ]] && echo "已修复待摘清单：${fixed_list}" >&2

# 清单只能缩：出现"已修复"时判红是故意的——否则清单会烂成永久豁免。
[[ $newfail -eq 0 && $fixed -eq 0 ]]
