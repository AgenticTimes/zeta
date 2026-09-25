#!/usr/bin/env bash
# tools/emit_stable.sh — 出码字节稳定性（批次 418）
#
# 判据（权威清单只写在这里，run_all.sh 第 16 步只认退出码）：
#   1. tests/determinism/ 下每个夹具必须存在且非空——glob 落空不等于通过。
#   2. 每个夹具用同一个 zetac 连编 EMIT_STABLE_RUNS 次（默认 3），
#      `--emit-llvm` 的 IR 必须逐字节相同；出现第 2 种字节即 FAIL。
# 两处被测的迭代序各由一个夹具盯住：fn_body_slots.z 盯函数序言里"每个局部槽一个
# alloca"的发射序，shared_field_names.z 盯按名字扫 struct 定义表得到的字段偏移。
# 必须在 $ROOT 下跑：pylib 库面是顺着 zetac 自身路径往上找的，换目录编译等于换输入
# （批次 418 实拍：把编译器拷去 /tmp 后连 `pandas` 都解析不到，虚报成"链接缺符号"）。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
RUNS="${EMIT_STABLE_RUNS:-3}"

failed=0
checked=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if ! ls tests/determinism/*.z >/dev/null 2>&1; then
  echo "  FAIL tests/determinism/ 里没有夹具 —— 空匹配不能算通过"
  exit 1
fi

for src in tests/determinism/*.z; do
  name="$(basename "$src")"
  checked=$((checked + 1))
  rm -f "$tmp"/*.ll
  compiled=1
  for i in $(seq 1 "$RUNS"); do
    if ! "$ZETAC" "$src" --emit-llvm -o "$tmp/$i.ll" >/dev/null 2>&1 \
      || [[ ! -s "$tmp/$i.ll" ]]; then
      echo "  FAIL ${name} —— 第 ${i} 次编译没产出 IR"
      failed=$((failed + 1))
      compiled=0
      break
    fi
  done
  [[ $compiled -eq 1 ]] || continue
  kinds="$(python3 -c '
import hashlib, os, sys
d = sys.argv[1]
h = {hashlib.sha256(open(os.path.join(d, f), "rb").read()).hexdigest()
     for f in os.listdir(d) if f.endswith(".ll")}
print(len(h))' "$tmp")"
  if [[ "$kinds" != "1" ]]; then
    echo "  FAIL ${name} —— ${RUNS} 次出码有 ${kinds} 种字节（期望 1）"
    failed=$((failed + 1))
  else
    echo "  ok ${name} —— ${RUNS} 次出码逐字节相同"
  fi
done

echo "emit_stable: ${checked} 个夹具，违规 ${failed}（期望 0）"
[[ $failed -eq 0 ]] || exit 1
exit 0
