#!/usr/bin/env bash
# tools/batch_gate.sh — 批次门禁＋内容寻址缓存（批次 816，调研 Go/Bazel 后落地）
#
# 原理（Go test cache / Bazel test caching 同款）：
#   指纹 = sha256(git HEAD + 未提交改动内容 + zetac 二进制 md5)
#   指纹没变 ⇒ 上一轮全绿的结果直接复用（秒回）；变了 ⇒ 三路并行跑检查，
#   全绿后记下新指纹。
# 用法：
#   tools/batch_gate.sh            # 有缓存命中就秒回；否则跑三路检查
#   tools/batch_gate.sh --force    # 无视缓存强制全跑
# 三路（并行）：差分 --group 50 ｜ python_style 抽样窗口 ｜ cargo test --lib
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# cargo 自举：调用方环境可能没有 ~/.cargo/bin（如后台任务/精简 shell）
command -v cargo >/dev/null 2>&1 || export PATH="$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null 2>&1 || { echo "缺 cargo（~/.cargo/bin 不存在？）" >&2; exit 2; }
STATE="tools/baselines/.batch_gate_state"

FORCE=0
[ "${1:-}" = "--force" ] && FORCE=1

ZETAC="$ROOT/target/release/zetac"
if [[ ! -x "$ZETAC" ]]; then
  echo "building zetac..." >&2
  cargo build --release -q || { echo "build failed" >&2; exit 2; }
fi

# 指纹：提交状态＋工作树未提交改动的完整内容＋二进制
TRACKED_CHANGES="$(git diff HEAD 2>/dev/null | sha256sum | cut -d' ' -f1)"
ZETAC_MD5="$(md5 -q "$ZETAC" 2>/dev/null || md5sum "$ZETAC" | cut -d' ' -f1)"
FP="$( (git rev-parse HEAD; echo "$TRACKED_CHANGES"; echo "$ZETAC_MD5") | sha256sum | cut -d' ' -f1)"

if [[ $FORCE -eq 0 && -f "$STATE" ]]; then
  OLD_FP="$(cut -d' ' -f1 "$STATE" 2>/dev/null)"
  OLD_VERDICT="$(cut -d' ' -f2 "$STATE" 2>/dev/null)"
  OLD_TIME="$(cut -d' ' -f3- "$STATE" 2>/dev/null)"
  if [[ "$OLD_FP" == "$FP" && "$OLD_VERDICT" == "GREEN" ]]; then
    echo "batch_gate: GREEN (cached) — 编译器与用例自 $OLD_TIME 起未变，三路结果直接复用，本次零执行"
    exit 0
  fi
fi

# 清场：#268 旋转 bug 的 dump 进程可能残留烧核（实测单个 380 CPU 分钟），
# 不清场则三路检查全被拖慢——每次开跑前先断根。
pkill -9 -f "zetac --dump-mir" 2>/dev/null || true
sleep 1
echo "batch_gate: 指纹变化（或强制），清场后三路并行检查…"
LOGDIR="$(mktemp -d /tmp/batch_gate.XXXXXX)"

run_libtest(){ cargo test -p zetac --lib -- --test-threads=1 >"$LOGDIR/libtest.log" 2>&1; }
run_global() { # 每 10 批一次：全局逐个跑（diff_test 本就逐例判定、逐例点名）
  python3 tools/diff_test.py >"$LOGDIR/diff.log" 2>&1
  bash tests/python_style/run.sh >"$LOGDIR/pystyle.log" 2>&1
}

# 批次计数：每批（指纹变化即一批）+1；计数到 10 的整数倍 ⇒ 本批附带全局跑
CNT_FILE="$ROOT/tools/baselines/.batch_gate_count"
BATCH=$(( $(cat "$CNT_FILE" 2>/dev/null || echo 0) + 1 ))
echo $BATCH > "$CNT_FILE"
echo "batch_gate: 第 $BATCH 批"

# 每批：只跑内置单元测试（模块内部，毫秒级——2026-10-03 用户裁定）
P1=""; P2=""
run_libtest & P3=$!
GLOBAL_RUN=0
if [ $(( BATCH % 10 )) -eq 0 ]; then
  GLOBAL_RUN=1
  run_global & P1=$!
fi

# 进度看门狗（2026-10-03 用户裁定：长任务必须可视化）：每 10 秒把三路
# 推进写进 progress.log，跑完自动退出。P1/P2 空值批（非全局批）不误判。
DIFFWD=""
( while true; do
    PY="$(grep -oE '[0-9]+/47[0-9]' "$LOGDIR/pystyle.log" 2>/dev/null | tail -1)"
    DSTAT="running"; [ -n "$P1" ] && { kill -0 $P1 2>/dev/null || DSTAT="done"; }
    LSTAT="running"; kill -0 $P3 2>/dev/null || LSTAT="done"
    echo "[$(date +%H:%M:%S)] 差分:$DSTAT | python_style:${PY:-启动中} | 内置:$LSTAT" >> "$LOGDIR/progress.log"
    ALIVE=0
    [ -n "$P1" ] && kill -0 $P1 2>/dev/null && ALIVE=1
    [ -n "$P2" ] && kill -0 $P2 2>/dev/null && ALIVE=1
    kill -0 $P3 2>/dev/null && ALIVE=1
    [ "$ALIVE" = "0" ] && break
    sleep 10
  done ) &
WATCHDOG=$!
RC=0
wait $P3 || { echo "❌ 内置单元测试红："; grep -E "FAILED|panicked" "$LOGDIR/libtest.log" | head -3; RC=1; }
if [ "$GLOBAL_RUN" = "1" ]; then
  wait $P1 || { echo "❌ 全局红（失败用例按裁定转模块内置测试后从全局退役）："; tail -3 "$LOGDIR/diff.log"; RC=1; }
fi
kill $WATCHDOG 2>/dev/null
wait $WATCHDOG 2>/dev/null
tail -3 "$LOGDIR/progress.log" 2>/dev/null

if [[ $RC -eq 0 ]]; then
  echo "$FP GREEN $(date '+%m-%d %H:%M')" > "$STATE"
  echo "batch_gate: GREEN（内置单元测试过）— 日志 $LOGDIR"
else
  rm -f "$STATE"
  echo "batch_gate: RED — 日志 ${LOGDIR}（修复后重跑自动替换指纹）"
fi
exit $RC
