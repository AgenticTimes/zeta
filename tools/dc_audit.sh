#!/usr/bin/env bash
# tools/dc_audit.sh —— 死代码审计（refactor 轴 A 的机器判据）
#
# 为什么需要它：src/lib.rs:7 和 src/main.rs:7 各有一行 `#![allow(dead_code)]`，
# 所以全仓的 dead_code 告警平时**一条都不报**——"没有告警"不代表"没有死代码"。
# `RUSTFLAGS=--force-warn dead_code` 在命令行上强制打开该 lint，且优先级高于
# 源码里的 allow 属性，因此**不需要改 lib.rs**（该文件由并发流程持有）。
#
# 口径（必须读，否则会把数字理解反）：
#   1) 这是**下界**。`pub` 项在 `pub mod` 里永远不会被判死（对外可达即算活）。
#      想审计某个文件的 pub 项，先把它们临时降为私有再跑：
#         python3 - <<'PY'   # 见 roadmap 批次 310 的做法
#      编译器会替你证明"没有外部引用"（E0603/E0624），比 grep 可靠。
#   2) 只统计本仓 src/ 与 tests/，跳过依赖 crate 的噪声。
#   3) 结构判据，不替代三套基线（官方 194 / python_style / corpus 39）。
#
# 用法：
#   ./tools/dc_audit.sh                 # 打印当前命中清单 + 计数
#   ./tools/dc_audit.sh --snapshot      # 另存基线到 /tmp/zeta_dc_baseline.txt
#   ./tools/dc_audit.sh --diff          # 与基线比对；有**新增**命中则 rc=1
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
BASE="${ZETA_DC_BASELINE:-/tmp/zeta_dc_baseline.txt}"
# ⚠️ 基线落在 /tmp ⇒ 重启即失，`--diff` 会退回"无基线"。固化到仓内需先定 run_all.sh 的
#    入库口径（.gitignore 的 `run_*` 会吞掉 tools/ 下的门禁产物，见任务 #15）。
OUT=/tmp/zeta_dc_audit.raw
HITS=/tmp/zeta_dc_hits.txt

# cargo 会缓存告警：命中缓存的那次运行**一条 warning 都不输出**，会被误读成
# "0 处死代码"。所以这里只碰 src/lib.rs 的 mtime（不改内容）强制重编 zetac 一个 crate。
#
# 特性口径：dead_code 是**按 cfg 配置**算的。批次 310 实测：默认特性 116 条（删掉 ctfe/
# resolver 两项后 114），`ZETA_DC_FEATS=--all-features` 118 条（同样 −2 ⇒ 恒比默认多 2 条，
# 多出的在 `src/integration/`，只有开 `integration` 特性才进编译图）。
# 该测量本身还踩过一次坑：第一次跑 --all-features 报 52 条，其实是**缓存运行**——
# 正下面那行 `Checking zetac` 自检就是为此存在。
# ⇒ 换特性配置要换基线文件名（`ZETA_DC_BASELINE=/tmp/zeta_dc_baseline_allfeat.txt`）。
FEATS=()
[ -n "${ZETA_DC_FEATS:-}" ] && FEATS=(${ZETA_DC_FEATS})
touch src/lib.rs
RUSTFLAGS="--force-warn dead_code" cargo check -p zetac --tests ${FEATS[@]+"${FEATS[@]}"} >"$OUT" 2>&1
crc=$?
if [ $crc -ne 0 ]; then
    echo "[E2001] cargo check 失败（rc=$crc），不产出命中清单："
    grep -E '^error' "$OUT" | head -10
    exit 2
fi
# 自检：没重新编译 = 缓存 = 清单不可信
grep -q 'Checking zetac' "$OUT" || { echo "[E2002] 本次为缓存运行（无 'Checking zetac'），命中清单不可信"; exit 3; }

# 每条 warning 取"消息 + 第一个 --> 位置"为主证据；只用消息措辞过滤 lint
# （不能用 `= note: requested on the command line` 做判据——实测同一批 dead_code
#   告警里只有少数带那条 note，拿它过滤会丢掉 ~80% 命中，批次 310 踩过）
awk '
  /^warning: / {
      if (loc != "") print loc "\t" msg
      msg = substr($0, 10); loc = ""; next
  }
  loc == "" && /^[[:space:]]*-->[[:space:]]+(src|tests)\// {
      sub(/^[[:space:]]*-->[[:space:]]*/, "")
      loc = $0
  }
  END { if (loc != "") print loc "\t" msg }
' "$OUT" | awk -F'\t' '$2 ~ / (is|are) never (used|read|constructed|called)$/' | sort -u > "$HITS"

n=$(wc -l < "$HITS" | tr -d ' ')
echo "dead_code 命中（本仓 src/ + tests/，下界口径见头部注释）：$n"
cut -f1 "$HITS" | cut -d: -f1 | sort | uniq -c | sort -rn | head -15

case "${1:-}" in
  --snapshot) cp "$HITS" "$BASE"; echo "基线已写入 $BASE（$n 条）"; exit 0 ;;
  --diff)
    [ -f "$BASE" ] || { echo "[E2002] 无基线 $BASE，先 --snapshot"; exit 2; }
    new=$(comm -13 <(cut -f1 "$BASE" | sort -u) <(cut -f1 "$HITS" | sort -u))
    if [ -n "$new" ]; then
        echo "新增命中（相对 $BASE）："
        echo "$new" | sed 's/^/  /'
        exit 1
    fi
    echo "无新增命中（基线 $BASE：$(wc -l < "$BASE" | tr -d ' ') 条）"
    exit 0 ;;
  *) cat "$HITS"; exit 0 ;;
esac
