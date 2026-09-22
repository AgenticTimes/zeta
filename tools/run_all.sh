#!/usr/bin/env bash
# tools/run_all.sh — Q4 (advice.md): one command → three baseline numbers as JSON.
# Usage: ./tools/run_all.sh [--json-only] [--skip-corpus] [--skip-official] [--skip-python] [--skip-jit] [--skip-diff] [--skip-knob] [--skip-swallow]
# Exit 0 if all enabled suites pass their green criteria; else 1.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
OUT_JSON="${OUT_JSON:-/tmp/zeta_baseline.json}"
JSON_ONLY=0
SKIP_CORPUS=0
SKIP_OFFICIAL=0
SKIP_PYTHON=0
SKIP_JIT=0
SKIP_DIFF=0
SKIP_KNOB=0
SKIP_SWALLOW=0

for a in "$@"; do
  case "$a" in
    --json-only) JSON_ONLY=1 ;;
    --skip-corpus) SKIP_CORPUS=1 ;;
    --skip-official) SKIP_OFFICIAL=1 ;;
    --skip-python) SKIP_PYTHON=1 ;;
    --skip-jit) SKIP_JIT=1 ;;
    --skip-diff) SKIP_DIFF=1 ;;
    --skip-knob) SKIP_KNOB=1 ;;
    --skip-swallow) SKIP_SWALLOW=1 ;;
    -h|--help)
      sed -n '2,6p' "$0"
      exit 0
      ;;
  esac
done

if [[ ! -x "$ZETAC" ]]; then
  echo "building zetac..." >&2
  cargo build --release -p zetac -q
fi

# G.2: this gate never rebuilds the C runtime, so an edited runtime/*.c silently
# puts the whole suite through a stale .o. Warn loudly, don't fail: the objects
# may be pinned on purpose while another workflow owns the C side.
zt_stale_check() { # $1 = linked object, rest = its sources
  local obj="$1" s; shift
  [[ -f "$obj" ]] || return 0
  for s in "$@"; do
    if [[ -f "$s" && "$s" -nt "$obj" ]]; then
      echo "[W2003] $s 比 $obj 新 —— 先 ./tools/build_runtime.sh，否则本次基线跑的是旧运行期" >&2
    fi
  done
  return 0
}
zt_stale_check zeta_runtime_c.o runtime/py_additions.c runtime/parquet_min.c runtime/unavailable_stubs.c
zt_stale_check tokio_runtime.o runtime/tokio_runtime_stub.c runtime/unavailable_stubs.c

ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
official_pass=0; official_total=0; official_diag_files=0; official_diag_lines=0; official_compile=0
py_pass=0; py_fail=0; py_known=0; py_xpass=0
corpus_ok=0; corpus_total=0
py_diag_lines=0; py_diag_files=0

# ── 1) official (compile-only, top-level *.z; copy to /tmp like validate.md) ──
# 任务 #34（docs/ABI.md 附 B#9）：此前编译 stderr 被 `>/dev/null 2>&1` 全丢 ⇒ 门禁日志里
# 一条编译期告警都收不到，任何"零告警"结论都是在空集上测的（批次 319 就这么错过一次）。
# 现在逐文件留档再聚合。**判定不变**：本步的口径仍是"编译成功数"，告警只出声不改 pass/fail。
# 聚合时排除 `clang: warning:`（链接器抱怨 `-no-pie`，实测 194/194 全有）——不排除的话，
# 真信号会被 194 条同名工具链噪声埋掉，聚合结果等于没聚合。
OFFICIAL_DIAG="${OFFICIAL_DIAG:-/tmp/zeta_official_diag.txt}"
OFFICIAL_LINK_DIAG="${OFFICIAL_LINK_DIAG:-/tmp/zeta_official_link.txt}"
if [[ $SKIP_OFFICIAL -eq 0 ]]; then
  rm -rf /tmp/zeta_tests /tmp/zt_out
  mkdir -p /tmp/zeta_tests /tmp/zt_out
  : > "$OFFICIAL_DIAG"
  : > "$OFFICIAL_LINK_DIAG"
  # shellcheck disable=SC2086
  cp $ROOT/tests/unit-tests/*.z /tmp/zeta_tests/ 2>/dev/null || true
  while IFS= read -r -d '' z; do
    official_total=$((official_total + 1))
    n=$(basename "$z" .z)
    d="/tmp/zt_out/$n.diag"
    if "$ZETAC" "$z" -o "/tmp/zt_out/$n" >/dev/null 2>"$d"; then
      official_pass=$((official_pass + 1))
      official_compile=$((official_compile + 1))
    elif "$ZETAC" "$z" --no-link -o "/tmp/zt_out/$n" >/dev/null 2>/dev/null; then
      # 批次 323：编译/降级全过、只有链接缺运行时绑定。这类失败**不是**编译器缺陷，
      # 也不许悄悄算成通过 —— 逐个登记文件名和缺的符号名。
      official_compile=$((official_compile + 1))
      miss=$(grep -oE '^  "_[A-Za-z0-9_]+"' "$d" | tr -d ' "' | sort -u | paste -sd, -)
      printf '### %s — 缺运行时绑定: %s\n' "$n" "${miss:-（链接器未列出符号名）}" >> "$OFFICIAL_LINK_DIAG"
    fi
    if grep -v '^clang: warning' "$d" 2>/dev/null | grep -q 'warning:\|PY-A:'; then
      official_diag_files=$((official_diag_files + 1))
      { printf '### %s\n' "$n"; grep -v '^clang: warning' "$d"; } >> "$OFFICIAL_DIAG"
    fi
  done < <(find /tmp/zeta_tests -maxdepth 1 -name '*.z' -print0 | sort -z)
  official_diag_lines=$(grep -c 'warning:\|PY-A:' "$OFFICIAL_DIAG" || true)
  official_diag_lines=${official_diag_lines:-0}
  link_only=$(wc -l < "$OFFICIAL_LINK_DIAG" | tr -d ' ')
  [[ $JSON_ONLY -eq 0 ]] && echo "official: compile ${official_compile}/${official_total}, compile+link ${official_pass}/${official_total}"
  if [[ $link_only -gt 0 ]]; then
    echo "link-only failures (编译通过、缺运行时绑定) ${link_only} —— 明细 $OFFICIAL_LINK_DIAG"
    cat "$OFFICIAL_LINK_DIAG"
  fi
  echo "compile-diagnostics: official ${official_diag_files}/${official_total} file(s) with compiler warnings, ${official_diag_lines} line(s) — 明细 $OFFICIAL_DIAG"
fi

# ── 2) python_style ──
if [[ $SKIP_PYTHON -eq 0 ]]; then
  py_log=$(mktemp)
  set +e
  "$ROOT/tests/python_style/run.sh" >"$py_log" 2>&1
  py_rc=$?
  set -e
  # Summary line: python_style: N passed, M failed, K known-fail, X xpass
  summary=$(rg -n '^python_style:' "$py_log" | tail -1 || true)
  if [[ -n "$summary" ]]; then
    py_pass=$(echo "$summary" | sed -E 's/.*: ([0-9]+) passed.*/\1/')
    py_fail=$(echo "$summary" | sed -E 's/.*passed, ([0-9]+) failed.*/\1/')
    py_known=$(echo "$summary" | sed -E 's/.*failed, ([0-9]+) known-fail.*/\1/')
    py_xpass=$(echo "$summary" | sed -E 's/.*known-fail, ([0-9]+) xpass.*/\1/')
  fi
  # 任务 #34：python_style 的编译告警由 run.sh 自己聚合（它的 OUTDIR 在 EXIT trap
  # 里就被删掉，外部再也捞不回来），这里只把它的计数行转成 JSON 字段。
  diagline=$(grep '^compile-diagnostics:' "$py_log" | tail -1 || true)
  if [[ -n "$diagline" ]]; then
    py_diag_lines=$(echo "$diagline" | sed -E 's/.*python_style ([0-9]+).*/\1/')
    py_diag_files=$(echo "$diagline" | sed -E 's/.*in ([0-9]+) file.*/\1/')
    # 只在 python_style **通过**时自己打印：它失败时下面那条 `tail -20 "$py_log"`
    # 本来就会把同一份聚合块吐出来（聚合块是 run.sh 的最后若干行），两处都印是纯噪声。
    if [[ $JSON_ONLY -eq 0 && $py_rc -eq 0 ]]; then
      awk '/^compile-diagnostics:/{p=1} p && c++<11' "$py_log"
    fi
  fi
  if [[ $JSON_ONLY -eq 0 ]]; then
    echo "$summary"
    [[ $py_rc -ne 0 ]] && tail -20 "$py_log" >&2
  fi
  rm -f "$py_log"
fi

# ── 3) corpus parse baseline ──
if [[ $SKIP_CORPUS -eq 0 ]]; then
  corp_log=$(mktemp)
  set +e
  python3 "$ROOT/tools/corpus_baseline.py" >"$corp_log" 2>&1
  set -e
  # 解析通过: ok/total
  line=$(rg '解析通过:' "$corp_log" | tail -1 || true)
  if [[ -n "$line" ]]; then
    corpus_ok=$(echo "$line" | sed -E 's/.*: ([0-9]+)\/([0-9]+).*/\1/')
    corpus_total=$(echo "$line" | sed -E 's/.*: ([0-9]+)\/([0-9]+).*/\2/')
  fi
  [[ $JSON_ONLY -eq 0 ]] && cat "$corp_log"
  rm -f "$corp_log"
fi

# ── 4) JIT（无 -o）静默崩溃门禁（批次 315 / 任务 #26）──
# 上面三套全走 `-o`（AOT），JIT 路径一行都没覆盖 —— `zetac f.z` 曾经当场 SIGSEGV、
# 零诊断，而三套基线全绿。此步把"JIT 不许静默崩溃、跑通数不许回退"变成判据。
# 缺 coreutils `timeout` 时**喊话跳过**，不静默跳过（静默跳过就是批次 314 记的幽灵门禁）。
jit_rc=0; jit_ok=0; jit_segv=0; jit_total=0; jit_skipped=0
if [[ $SKIP_JIT -eq 0 ]]; then
  jit_log=$(mktemp)
  set +e
  "$ROOT/tools/jit_sweep.sh" >"$jit_log" 2>&1
  jit_rc=$?
  set -e
  if [[ $jit_rc -eq 2 ]]; then
    jit_skipped=1
    echo "SKIP jit: tools/jit_sweep.sh 需要 coreutils timeout" >&2
  fi
  line=$(grep '^jit sweep:' "$jit_log" | tail -1 || true)
  # `ok=` must be anchored to the front of the line: the sweep line ends with
  # the *threshold* (`…，最小 ok=163`), and a greedy `.*ok=` skips past the
  # measurement and captures that floor instead. Measured: the unanchored form
  # returned 163 while the real reading on the same line was 170 — so the
  # baseline JSON recorded a constant, and any cross-batch diff of it was
  # structurally incapable of showing a regression.
  jit_ok=$(echo "$line" | sed -nE 's/^jit sweep: ok=([0-9]+) .*/\1/p'); jit_ok=${jit_ok:-0}
  jit_segv=$(echo "$line" | sed -nE 's/.*segv=([0-9]+).*/\1/p'); jit_segv=${jit_segv:-0}
  jit_total=$(echo "$line" | sed -nE 's/.*total ([0-9]+).*/\1/p'); jit_total=${jit_total:-0}
  [[ $JSON_ONLY -eq 0 ]] && cat "$jit_log"
  rm -f "$jit_log"
fi

# ── 5) CPython 差分一致率（refactor.md G.3 起步 / 立即档 ⑩，批次 326）──
# 前四步全部只测"自洽"：`// expect:` 的期望值是人照着自己对 zeta 的想象手写的，
# 所以它能量出回归，永远量不出"语义与 Python 不一致"。这一步把期望值交给参考
# 实现（python3 现场跑同一段语义），首次让"还有多少语义是错的"变成一个数字。
# 判据在 tools/diff_test.py 内部：基线里 match 的用例不许变差、match 绝对数不许
# 低于 match_min。rc=2 是"参考侧自己跑不出真值"（坏用例/跨实现差异），
# **只喊话不判红**——那类失败要修的是用例或环境，不是编译器；静默忽略才是问题。
diff_rc=0; diff_match=0; diff_judged=0; diff_rate=0; diff_bad=0; diff_skipped=0
if [[ $SKIP_DIFF -eq 0 ]]; then
  diff_log=$(mktemp)
  set +e
  python3 "$ROOT/tools/diff_test.py" >"$diff_log" 2>&1
  diff_rc=$?
  set -e
  line=$(grep '^diff test:' "$diff_log" | tail -1 || true)
  diff_match=$(echo "$line" | sed -nE 's/^diff test: match=([0-9]+).*/\1/p'); diff_match=${diff_match:-0}
  diff_judged=$(echo "$line" | sed -nE 's/.* judged=([0-9]+).*/\1/p'); diff_judged=${diff_judged:-0}
  diff_rate=$(echo "$line" | sed -nE 's/.* rate=([0-9.]+)%.*/\1/p'); diff_rate=${diff_rate:-0}
  diff_bad=$(echo "$line" | sed -nE 's/.* bad_case=([0-9]+).*/\1/p'); diff_bad=${diff_bad:-0}
  [[ $JSON_ONLY -eq 0 ]] && grep -E '^(  [a-z]+ |[a-z]+ +[0-9]+/|diff test:)' "$diff_log" | tail -12
  if [[ $diff_rc -ne 0 ]]; then
    # 判定行（回归清单/坏用例）不在上面的抽样里，红的时候必须全文出声
    tail -40 "$diff_log" >&2
  fi
  rm -f "$diff_log"
fi

# ── 6) 布尔旋钮的值域断言（批次 336）──
# 27 个 Rust 侧旋钮以前全是 `env::var(…).is_ok()`＝"存在即开"，于是 `ZETA_NO_OPT=0`、
# `ZETA_STRICT_PARSE=0` 这类"关"的写法得到的都是"开"。判据不是"某条命令恰好没用到它"，
# 而是**每个受影响旋钮 × 整个假值域**都要测——所以这一步跑的是断言，不是抽样。
# 只跑 A 段（`--assert-only`，秒级）；B 段是全语料读数，不参与判定。
knob_rc=0; knob_failed=0; knob_checked=0
if [[ $SKIP_KNOB -eq 0 ]]; then
  knob_log=$(mktemp)
  set +e
  ZETAC="$ZETAC" "$ROOT/tools/knob_probe.sh" --assert-only >"$knob_log" 2>&1
  knob_rc=$?
  set -e
  knob_failed=$(grep -c '  FAIL ' "$knob_log" || true); knob_failed=${knob_failed:-0}
  knob_checked=$(grep -cE '  (ok|FAIL|skip) ' "$knob_log" || true); knob_checked=${knob_checked:-0}
  if [[ $JSON_ONLY -eq 0 ]]; then
    echo "knob: ${knob_checked} 条断言，FAIL ${knob_failed}（rc=$knob_rc）"
  fi
  if [[ $knob_rc -ne 0 ]]; then
    tail -30 "$knob_log" >&2
  fi
  rm -f "$knob_log"
fi

# ── 7) 前导词静默吞掉的判据两翼断言（批次 337）──
# 语句解析的兜底是"一个表达式＝一条语句"，于是解析器不认识的前导词会**无声消失**
# （`static mut c = 0` 丢 `static`、`import std::memory;` 绑成 `std` 并丢掉 `memory`）。
# W1004 在吞掉那一刻出声；它的排除项（函数尾隐式返回长得一样）同样必须被测住，
# 否则这个判据可以在"永远不响"和"到处乱响"之间任意翻车而门禁全绿。
swallow_rc=0; swallow_failed=0; swallow_checked=0
if [[ $SKIP_SWALLOW -eq 0 ]]; then
  swallow_log=$(mktemp)
  set +e
  ZETAC="$ZETAC" "$ROOT/tools/junk_swallow_inventory.sh" >"$swallow_log" 2>&1
  swallow_rc=$?
  set -e
  swallow_failed=$(grep -c '  FAIL ' "$swallow_log" || true); swallow_failed=${swallow_failed:-0}
  swallow_checked=$(grep -cE '  (ok|FAIL) ' "$swallow_log" || true); swallow_checked=${swallow_checked:-0}
  if [[ $JSON_ONLY -eq 0 ]]; then
    echo "swallow: ${swallow_checked} 条断言，FAIL ${swallow_failed}（rc=$swallow_rc）"
  fi
  if [[ $swallow_rc -ne 0 ]]; then
    tail -30 "$swallow_log" >&2
  fi
  rm -f "$swallow_log"
fi

# ── JSON summary (single source of truth) ──
python3 - <<PY
import json
doc = {
  "ts": "$ts",
  "zetac": "$ZETAC",
  "official": {"pass": $official_pass, "compile": $official_compile, "total": $official_total},
  "python_style": {
    "pass": $py_pass, "fail": $py_fail,
    "known_fail": $py_known, "xpass": $py_xpass,
  },
  "corpus": {"parse_ok": $corpus_ok, "total": $corpus_total},
  "jit": {"ok": $jit_ok, "segv": $jit_segv, "total": $jit_total,
          "skipped": $jit_skipped},
  "diff": {"match": $diff_match, "judged": $diff_judged, "rate_pct": $diff_rate,
           "bad_case": $diff_bad, "skipped": $SKIP_DIFF},
  "knob": {"checked": $knob_checked, "failed": $knob_failed,
           "skipped": $SKIP_KNOB},
  "swallow": {"checked": $swallow_checked, "failed": $swallow_failed,
              "skipped": $SKIP_SWALLOW},
  # 只出声、不参与退出码（附 B#9 的护栏：判定看运行期 stdout，告警不改判定）
  "compile_diagnostics": {
    "official_files_with_warnings": $official_diag_files,
    "official_warning_lines": $official_diag_lines,
    "official_not_measured": $SKIP_OFFICIAL,
    "python_style_files_with_warnings": $py_diag_files,
    "python_style_warning_lines": $py_diag_lines
  },
}
path = "$OUT_JSON"
with open(path, "w") as f:
    json.dump(doc, f, indent=2)
    f.write("\n")
print(path)
print(json.dumps(doc, indent=2))
PY

# Green criteria
rc=0
# 批次 323 口径变更（附 B#10）：判据从"编译+链接全过"改为"**编译**全过"。
# 理由不是"把它绿过去"——这两件事此前被同一个数字混在一起，而 self-host 语料的
# 解析行数一旦恢复，就会引用还没绑定的 std 方法，于是"缺运行时绑定"以"编译器不支持
# 这段语法"的形式报出来，把解析工作直接封死在截断点上。现在 compile+link 照样打印、
# 缺绑定的文件与符号名逐条登记到 $OFFICIAL_LINK_DIAG 并打进日志，只是不再冒充编译缺陷。
if [[ $SKIP_OFFICIAL -eq 0 && $official_compile -ne $official_total ]]; then rc=1; fi
if [[ $SKIP_PYTHON -eq 0 && $py_fail -ne 0 ]]; then rc=1; fi
# corpus: parse_ok should equal total when suite is healthy; warn-only if skipped dirs empty
if [[ $SKIP_CORPUS -eq 0 && $corpus_total -gt 0 && $corpus_ok -ne $corpus_total ]]; then rc=1; fi
# jit: 判据在 tools/jit_sweep.sh 内部（segv==0 且 ok>=基线），这里只认它的退出码
if [[ $SKIP_JIT -eq 0 && $jit_skipped -eq 0 && $jit_rc -ne 0 ]]; then rc=1; fi
# diff: rc=1=一致用例回归/match 数回退（判红）；rc=2=坏用例（只喊话，见步骤 5 注释）
if [[ $SKIP_DIFF -eq 0 && $diff_rc -eq 1 ]]; then rc=1; fi
# knob: 判据在 tools/knob_probe.sh 的 A 段内部（假值拼写必须关、留白名单站点必须还在），
# 这里只认它的退出码。
if [[ $SKIP_KNOB -eq 0 && $knob_rc -ne 0 ]]; then rc=1; fi
# swallow: 判据同上，跑的是 tools/junk_swallow_inventory.sh 的夹具段（不含全语料计数）。
if [[ $SKIP_SWALLOW -eq 0 && $swallow_rc -ne 0 ]]; then rc=1; fi
if [[ $SKIP_DIFF -eq 0 && $diff_rc -eq 2 ]]; then
  echo "[G.3] 差分有 $diff_bad 条坏用例（参考侧跑不出真值）——不参与判定，但必须修用例" >&2
fi
exit $rc
