#!/usr/bin/env bash
# tools/run_all.sh — Q4 (advice.md): one command → three baseline numbers as JSON.
# Usage: ./tools/run_all.sh [--json-only] [--skip-corpus] [--skip-official] [--skip-python] [--skip-jit] [--skip-diff] [--skip-knob] [--skip-swallow] [--skip-import] [--skip-empty] [--skip-clean] [--skip-pysrc] [--skip-sem] [--skip-ignore] [--skip-mbvar]
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
SKIP_IMPORT=0
SKIP_EMPTY=0
SKIP_CLEAN=0
SKIP_PYSRC=0
SKIP_SEM=0
SKIP_IGNORE=0
SKIP_MBVAR=0

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
    --skip-import) SKIP_IMPORT=1 ;;
    --skip-empty) SKIP_EMPTY=1 ;;
    --skip-clean) SKIP_CLEAN=1 ;;
    --skip-pysrc) SKIP_PYSRC=1 ;;
    --skip-sem) SKIP_SEM=1 ;;
    --skip-ignore) SKIP_IGNORE=1 ;;
    --skip-mbvar) SKIP_MBVAR=1 ;;
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
    echo "knob: ${knob_checked} 条断言，FAIL ${knob_failed}（rc=${knob_rc}）"
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
    echo "swallow: ${swallow_checked} 条断言，FAIL ${swallow_failed}（rc=${swallow_rc}）"
  fi
  if [[ $swallow_rc -ne 0 ]]; then
    tail -30 "$swallow_log" >&2
  fi
  rm -f "$swallow_log"
fi

# ── 8) `import` 的形状值域（批次 338）──
# `import` 与 `use` 共用同一份 `::` 文法，且结尾 `;` 不改变形状。两翼各锁一条：
# 等值翼（每种 :: 形状的 MIR 必须与 use 逐字节相同）+ 中立翼（带不带分号必须相同、
# 不许截断文件）。这一步跑的是断言，不是抽样。
import_rc=0; import_failed=0; import_checked=0
if [[ $SKIP_IMPORT -eq 0 ]]; then
  import_log=$(mktemp)
  set +e
  ZETAC="$ZETAC" "$ROOT/tools/import_form_inventory.sh" >"$import_log" 2>&1
  import_rc=$?
  set -e
  import_failed=$(grep -c '  FAIL ' "$import_log" || true); import_failed=${import_failed:-0}
  import_checked=$(grep -cE '  (ok|FAIL) ' "$import_log" || true); import_checked=${import_checked:-0}
  if [[ $JSON_ONLY -eq 0 ]]; then
    echo "import: ${import_checked} 条断言，FAIL ${import_failed}（rc=${import_rc}）"
  fi
  if [[ $import_rc -ne 0 ]]; then
    tail -30 "$import_log" >&2
  fi
  rm -f "$import_log"
fi

# ── 9) 裸 `;` 的形状值域（批次 339）──
# 顶层循环是 many0(顶层项)，规则不认的形状不是"报错"而是"文件余部整段丢弃"。
# `;` 是**顶层空项**：与块内空语句同一语义，实现只有一处（parse_top_level_entry）。
# 四翼：加不加 ; 逐字节同一份 MIR（中立）+ 尾巴必须还在（截断）+ 默认路径与
# ZETA_PARSE_RECOVER=1 同音同调（两路）+ 吞词判据不许被顺手放宽（负控制）。
empty_rc=0; empty_failed=0; empty_checked=0
if [[ $SKIP_EMPTY -eq 0 ]]; then
  empty_log=$(mktemp)
  set +e
  ZETAC="$ZETAC" "$ROOT/tools/empty_stmt_inventory.sh" >"$empty_log" 2>&1
  empty_rc=$?
  set -e
  empty_failed=$(grep -c '  FAIL ' "$empty_log" || true); empty_failed=${empty_failed:-0}
  empty_checked=$(grep -cE '  (ok|FAIL) ' "$empty_log" || true); empty_checked=${empty_checked:-0}
  if [[ $JSON_ONLY -eq 0 ]]; then
    echo "empty_stmt: ${empty_checked} 条断言，FAIL ${empty_failed}（rc=${empty_rc}）"
  fi
  if [[ $empty_rc -ne 0 ]]; then
    tail -30 "$empty_log" >&2
  fi
  rm -f "$empty_log"
fi

# ── 10) 干净检出可编译（批次 342）──
# 判的是"克隆 HEAD 就能编"，不是"我这台机器能编"：批次 340 的 `include_str!` 指向未入库
# 文件时，主树（带着那个未跟踪文件）前九步全绿，而干净检出 rc=101 —— 主树永远看不见。
# 用 detached worktree、不用临时目录：主树常有未提交改动，本步骤必须一个字都不碰它。
# target 共享主树（实测冷 4 s / 热 1 s；主树缓存没被改脏，改前改后 check 都是 0.1 s 级）。
# 选址必须在仓库外：PY-A 从被编译文件往上找 6 级祖先（340 OPEN 2），放在 $ROOT 之下会让
# "干净检出"借到主树的 .z，读数就不再是干净检出。
# 破坏性命令为零：只 checkout，不 clean；同名但未登记的目录不删、不改、也不测。
clean_rc=0; clean_secs=0; clean_rev=""
if [[ $SKIP_CLEAN -eq 0 ]]; then
  WT="${ZETA_CLEAN_WT:-$HOME/zeta-clean-checkout}"
  CT="${ZETA_CLEAN_TARGET:-$ROOT/target}"
  REF="${ZETA_CLEAN_REF:-HEAD}"
  clean_log=$(mktemp)
  case "$WT" in
    /*) ;;
    *) echo "clean_checkout: ZETA_CLEAN_WT 必须是绝对路径（当前：${WT}）" >&2; clean_rc=99 ;;
  esac
  if [[ $clean_rc -eq 0 ]]; then
    case "$WT/" in
      "$ROOT"/*) echo "clean_checkout: $WT 在仓库内，检出读数会借用主树文件" >&2; clean_rc=98 ;;
    esac
  fi
  # 提交必须在**主树**里解析：`git -C "$WT" checkout HEAD` 解的是工作树自己的 HEAD，
  # 复用时它会原地不动，于是"干净检出"永远在重复测上一次那个提交（实测判绿而 rev 陈旧）。
  if [[ $clean_rc -eq 0 ]]; then
    clean_rev=$(git -C "$ROOT" rev-parse --verify --quiet "${REF}^{commit}") || clean_rev=""
    if [[ -z "$clean_rev" ]]; then
      echo "clean_checkout: $REF 在 $ROOT 里解不出提交，读数无效" >&2; clean_rc=93
    fi
  fi
  if [[ $clean_rc -eq 0 && ! -e "$WT" ]]; then
    git worktree add --detach "$WT" "$clean_rev" >"$clean_log" 2>&1 || clean_rc=97
  fi
  # 复用的前提：路径登记在本仓的 worktree 表里（git 自己说的算）。
  if [[ $clean_rc -eq 0 ]] && ! git worktree list --porcelain | grep -qx "worktree $WT"; then
    echo "clean_checkout: $WT 存在但未登记为本仓 worktree —— 拒绝复用（换 ZETA_CLEAN_WT 或 --skip-clean）" >&2
    clean_rc=96
  fi
  if [[ $clean_rc -eq 0 ]]; then
    git -C "$WT" checkout --detach "$clean_rev" >"$clean_log" 2>&1 || clean_rc=95
  fi
  if [[ $clean_rc -eq 0 && -n "$(git -C "$WT" status --porcelain)" ]]; then
    echo "clean_checkout: $WT 检出 ${clean_rev:0:8} 后仍不干净，读数无效（本步骤只 checkout，不 clean）" >&2
    clean_rc=94
  fi
  if [[ $clean_rc -eq 0 ]]; then
    t0=$(date +%s)
    set +e
    ( cd "$WT" && CARGO_TARGET_DIR="$CT" cargo check --offline --locked -q ) >"$clean_log" 2>&1
    clean_rc=$?
    set -e
    clean_secs=$(( $(date +%s) - t0 ))
  fi
  if [[ $JSON_ONLY -eq 0 ]]; then
    echo "clean_checkout: rc=${clean_rc}（${clean_secs}s，rev=${clean_rev:0:8}，worktree=${WT}）"
  fi
  if [[ $clean_rc -ne 0 ]]; then
    # cargo 的根因在**开头**（后面全是它引发的 E0282 级联），所以这里先头后尾，不像
    # 其它步骤只 tail：实测只 tail 时看得见 4 条 type annotations needed、看不见那句
    # 真正没读到的文件。
    head -20 "$clean_log" >&2
    tail -5 "$clean_log" >&2
  fi
  rm -f "$clean_log"
fi

# ── 11) PY-A 模块解析的落点（批次 343）──
# 被编译文件所在目录**往上 5 级**都在搜索基里，所以门禁读数会随检出位置变化：家目录躺
# 一个同名 .z 就能压过 pylib。本步骤钉的是"越界必须出声（W1005）、就地/注册表/相对基不
# 许出声、越界与否给出的 MIR 逐字节相同"——六翼，判据在脚本内部，这里只认退出码。
pysrc_rc=0; pysrc_failed=0; pysrc_checked=0
if [[ $SKIP_PYSRC -eq 0 ]]; then
  pysrc_log=$(mktemp)
  set +e
  ZETAC="$ZETAC" "$ROOT/tools/py_module_search_inventory.sh" >"$pysrc_log" 2>&1
  pysrc_rc=$?
  set -e
  pysrc_failed=$(grep -c '  FAIL ' "$pysrc_log" || true); pysrc_failed=${pysrc_failed:-0}
  pysrc_checked=$(grep -cE '  (ok|FAIL) ' "$pysrc_log" || true); pysrc_checked=${pysrc_checked:-0}
  if [[ $JSON_ONLY -eq 0 ]]; then
    echo "pysrc: ${pysrc_checked} 条断言，FAIL ${pysrc_failed}（rc=${pysrc_rc}）"
  fi
  if [[ $pysrc_rc -ne 0 ]]; then
    tail -30 "$pysrc_log" >&2
  fi
  rm -f "$pysrc_log"
fi

# ── 12) 只读入口不许执行程序（批次 344）──
# 修复前"要不要执行被编译的程序"只看有没有 `-o` ⇒ --dump-mir / --emit-llvm /
# --report-stubs / --report-untyped / ZETA_DUMP_IR=1 全都边 dump 边跑 main。
# 本步骤钉五翼（344 三翼 + 348 两翼）：裸跑必须执行（探测器阳性对照）、五个只读入口必须不执行且 rc=0、
# `-o` 那条路逐字不变。判据在 tools/cli_semantics_check.sh 内部，这里只认退出码。
sem_rc=0; sem_failed=0; sem_checked=0
if [[ $SKIP_SEM -eq 0 ]]; then
  sem_log=$(mktemp)
  set +e
  ZETAC="$ZETAC" "$ROOT/tools/cli_semantics_check.sh" >"$sem_log" 2>&1
  sem_rc=$?
  set -e
  sem_failed=$(grep -c '  FAIL ' "$sem_log" || true); sem_failed=${sem_failed:-0}
  sem_checked=$(grep -cE '  (ok|FAIL) ' "$sem_log" || true); sem_checked=${sem_checked:-0}
  if [[ $JSON_ONLY -eq 0 ]]; then
    echo "cli_semantics: ${sem_checked} 条断言，FAIL ${sem_failed}（rc=${sem_rc}）"
  fi
  if [[ $sem_rc -ne 0 ]]; then
    tail -30 "$sem_log" >&2
  fi
  rm -f "$sem_log"
fi

# ── 13) 忽略规则表不许再吞手写源（批次 346）──
# 批次 345 清了 `.gitignore` 里的 7 个 NUL（含 NUL 的文件被 git 判成二进制 ⇒ 此后每次
# 规则改动在 review 里都是一行 "Binary files differ"），批次 346 把 10 条裸前缀规则
# （`test_*` / `zeta_*` / `simple_*` / 裸 `*.z` …—— 不带斜杠的规则连目录名都匹配）
# 收窄成根锚定或 `/build/**`，于是压在规则底下的已跟踪源文件从 290 个降到 28 个。
# 本步骤钉四翼：规则表必须是文本、手写源树必须不被正向规则命中、产物与根级 scratch
# 必须仍被忽略、清单外的已跟踪文件数必须为 0。判据在 tools/ignore_rule_inventory.sh
# 内部，这里只认退出码。
ignore_rc=0; ignore_failed=0; ignore_checked=0
if [[ $SKIP_IGNORE -eq 0 ]]; then
  ignore_log=$(mktemp)
  set +e
  "$ROOT/tools/ignore_rule_inventory.sh" >"$ignore_log" 2>&1
  ignore_rc=$?
  set -e
  ignore_failed=$(grep -c '  FAIL ' "$ignore_log" || true); ignore_failed=${ignore_failed:-0}
  ignore_checked=$(grep -cE '  (ok|FAIL) ' "$ignore_log" || true); ignore_checked=${ignore_checked:-0}
  if [[ $JSON_ONLY -eq 0 ]]; then
    echo "ignore_rules: ${ignore_checked} 条断言，FAIL ${ignore_failed}（rc=${ignore_rc}）"
  fi
  if [[ $ignore_rc -ne 0 ]]; then
    tail -30 "$ignore_log" >&2
  fi
  rm -f "$ignore_log"
fi

# ── 14) 脚本里的 `$VAR` 不许紧跟多字节字符（批次 346）──
# 实测缺陷：bash 3.2 把 `$p（…` 里多字节字符的首字节并进变量名（实得 `p\xef`），
# `set -u` 当场报 `line N: p?: unbound variable` 并**杀掉整个脚本**。批次 346 前的
# 步骤 13 就死在这一行上——"整步崩掉"与"整步通过"在汇总里都只剩一句 rc，看不出来。
# 全仓实测 40 处、10 个脚本；中文文案 + `set -u` 是本仓默认风格 ⇒ 必然复发。
# 判据在 tools/mbvar_lint.sh 内部（注释与单引号字面量不算），这里只认退出码。
mbvar_rc=0; mbvar_failed=0; mbvar_checked=0
if [[ $SKIP_MBVAR -eq 0 ]]; then
  mbvar_log=$(mktemp)
  set +e
  "$ROOT/tools/mbvar_lint.sh" >"$mbvar_log" 2>&1
  mbvar_rc=$?
  set -e
  mbvar_failed=$(grep -c '  FAIL ' "$mbvar_log" || true); mbvar_failed=${mbvar_failed:-0}
  mbvar_checked=$(grep -cE '^  (ok|FAIL) ' "$mbvar_log" || true); mbvar_checked=${mbvar_checked:-0}
  if [[ $JSON_ONLY -eq 0 ]]; then
    echo "mbvar: ${mbvar_checked} 个脚本，违规 ${mbvar_failed}（rc=${mbvar_rc}）"
  fi
  if [[ $mbvar_rc -ne 0 ]]; then
    tail -30 "$mbvar_log" >&2
  fi
  rm -f "$mbvar_log"
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
  "import_form": {"checked": $import_checked, "failed": $import_failed,
                  "skipped": $SKIP_IMPORT},
  "empty_stmt": {"checked": $empty_checked, "failed": $empty_failed,
                 "skipped": $SKIP_EMPTY},
  "pysrc": {"checked": $pysrc_checked, "failed": $pysrc_failed,
            "rc": $pysrc_rc, "skipped": $SKIP_PYSRC},
  "cli_semantics": {"checked": $sem_checked, "failed": $sem_failed,
                    "rc": $sem_rc, "skipped": $SKIP_SEM},
  "ignore_rules": {"checked": $ignore_checked, "failed": $ignore_failed,
                   "rc": $ignore_rc, "skipped": $SKIP_IGNORE},
  "mbvar": {"checked": $mbvar_checked, "failed": $mbvar_failed,
            "rc": $mbvar_rc, "skipped": $SKIP_MBVAR},
  "clean_checkout": {"rc": $clean_rc, "secs": $clean_secs, "rev": "${clean_rev:0:8}",
                     "skipped": $SKIP_CLEAN},
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
if [[ $SKIP_IMPORT -eq 0 && $import_rc -ne 0 ]]; then rc=1; fi
# empty_stmt: 判据在 tools/empty_stmt_inventory.sh 内部（四翼断言），这里只认退出码。
if [[ $SKIP_EMPTY -eq 0 && $empty_rc -ne 0 ]]; then rc=1; fi
# pysrc: 判据在 tools/py_module_search_inventory.sh 内部（六翼——批次 343 补的 A2 翼，
# 这行当时漏改，批次 344 顺手更正），这里只认退出码。
if [[ $SKIP_PYSRC -eq 0 && $pysrc_rc -ne 0 ]]; then rc=1; fi
# cli_semantics: 判据在 tools/cli_semantics_check.sh 内部（五翼），这里只认退出码。
if [[ $SKIP_SEM -eq 0 && $sem_rc -ne 0 ]]; then rc=1; fi
# ignore_rules: 判据在 tools/ignore_rule_inventory.sh 内部（四翼），这里只认退出码。
if [[ $SKIP_IGNORE -eq 0 && $ignore_rc -ne 0 ]]; then rc=1; fi
# mbvar: 判据在 tools/mbvar_lint.sh 内部（注释与单引号字面量不计），这里只认退出码。
if [[ $SKIP_MBVAR -eq 0 && $mbvar_rc -ne 0 ]]; then rc=1; fi
# clean_checkout: 判据在步骤 10 内部（rc=0 才算"检出即可编译"）；93~99 是选址/登记/提交解析
# 本身不合法，同样判红——静默跳过等于这一步不存在。
if [[ $SKIP_CLEAN -eq 0 && $clean_rc -ne 0 ]]; then rc=1; fi
if [[ $SKIP_DIFF -eq 0 && $diff_rc -eq 2 ]]; then
  echo "[G.3] 差分有 $diff_bad 条坏用例（参考侧跑不出真值）——不参与判定，但必须修用例" >&2
fi
exit $rc
