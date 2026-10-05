#!/usr/bin/env bash
# tests/python_style/selfcheck_run_capped.sh — 单用例 worker 里 run_capped 的自证脚本（批次 10030）。
#
# 跑法：bash tests/python_style/selfcheck_run_capped.sh   （秒级，不编译任何 zeta 用例）
# 取的是同目录 run_one.sh 里 run_capped 的原文函数体，所以这段守卫不会和 worker 走散。
#
# 覆盖四种进程形状（都用工子脚本现场造，不依赖被测编译器）：
#   ① 正常退出 0 且 stdout/stderr 各归各位；② 非 0 退出码取到；
#   ③ 真 SIGABRT 的进程要么给退出码、要么给"停在不可中断态"（#20006 那一形），
#      两条路都得在 20 秒上限内落地；④ 慢进程到上限即收手，不许无限等。
# 第④项是这条改动存在的理由：旧口径 `timeout 20` 发完信号还要等子进程收尾，
# 收尾收不掉的进程（ps 状态 UE）会把整个并行池钉住（2026-10-04 全量跑第②步实证）。
set -u
ONE="$(cd "$(dirname "$0")" && pwd)/run_one.sh"
tmp="$(mktemp -d /tmp/zeta_run_capped.XXXXXX)"
trap 'rm -rf "$tmp"' EXIT

sed -n '/^run_capped() {/,/^}/p' "$ONE" > "$tmp/fn.sh"
[ "$(wc -l < "$tmp/fn.sh" | tr -d ' ')" -gt 10 ] || { echo "取不到 run_capped 函数体（$ONE 的函数名或括号形状变了）"; exit 1; }
# shellcheck disable=SC1091
. "$tmp/fn.sh"

ok=0; bad=0
chk() { if [ "$2" = "$3" ]; then echo "  ok   $1"; ok=$((ok + 1)); else echo "  FAIL ${1}：期望 [$3] 实得 [$2]"; bad=$((bad + 1)); fi; }

mk() { printf '#!/bin/sh\n%s\n' "$2" > "$tmp/$1"; chmod +x "$tmp/$1"; }
mk normal  'echo hello-stdout; echo note-stderr >&2; exit 0'
mk nonzero 'echo "stub not implemented: foo" >&2; exit 134'
mk abrt    'echo "stub not implemented: bar" >&2; kill -ABRT $$'
mk slow    'sleep 30'

run_capped 3 "$tmp/1.out" "$tmp/1.err" "$tmp/normal"
chk "① 正常退出取到 rc=0"        "$RUN_RC"      "0"
chk "① 正常退出不算停住"          "$RUN_STUCK"   "0"
chk "① stdout 落进 RUN_OUT"      "$RUN_OUT"     "hello-stdout"
chk "① stderr 落进 RUN_ERR"      "$RUN_ERR"     "note-stderr"

run_capped 3 "$tmp/2.out" "$tmp/2.err" "$tmp/nonzero"
chk "② 非 0 退出码取到"          "$RUN_RC"      "134"
chk "② 非 0 不算停住"            "$RUN_STUCK"   "0"
chk "② stderr 命中期望子串"      "$(printf '%s' "$RUN_ERR" | grep -c 'stub not implemented')" "1"

run_capped 3 "$tmp/3.out" "$tmp/3.err" "$tmp/abrt"
# 本机负载决定这颗会不会停在不可中断态：取到码或确认停住，二者之一即算落地
if [ "$RUN_STUCK" = 1 ]; then
    chk "③ SIGABRT 形：停住时 rc 记 -1" "$RUN_RC" "-1"
else
    chk "③ SIGABRT 形取到退出码 134"    "$RUN_RC" "134"
fi
chk "③ SIGABRT 形 stderr 命中"   "$(printf '%s' "$RUN_ERR" | grep -c 'stub not implemented')" "1"

t0=$(date +%s)
run_capped 2 "$tmp/4.out" "$tmp/4.err" "$tmp/slow"
el=$(( $(date +%s) - t0 ))
chk "④ 慢进程到上限即收手（≤8 秒）" "$([ "$el" -le 8 ] && echo yes || echo no)" "yes"
chk "④ 慢进程不发停住标记"          "$RUN_STUCK" "0"
pgrep -f "$tmp/slow" >/dev/null 2>&1 && { echo "  FAIL ④ 慢进程到点后还活着"; bad=$((bad + 1)); } \
    || { echo "  ok   ④ 慢进程已收尾"; ok=$((ok + 1)); }

echo "run_capped 自证：ok=$ok 失败=$bad"
[ "$bad" = 0 ]
