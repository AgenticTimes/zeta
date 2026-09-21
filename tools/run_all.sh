#!/usr/bin/env bash
# tools/run_all.sh — Q4 (advice.md): one command → three baseline numbers as JSON.
# Usage: ./tools/run_all.sh [--json-only] [--skip-corpus] [--skip-official] [--skip-python]
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

for a in "$@"; do
  case "$a" in
    --json-only) JSON_ONLY=1 ;;
    --skip-corpus) SKIP_CORPUS=1 ;;
    --skip-official) SKIP_OFFICIAL=1 ;;
    --skip-python) SKIP_PYTHON=1 ;;
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
official_pass=0; official_total=0
py_pass=0; py_fail=0; py_known=0; py_xpass=0
corpus_ok=0; corpus_total=0

# ── 1) official (compile-only, top-level *.z; copy to /tmp like validate.md) ──
if [[ $SKIP_OFFICIAL -eq 0 ]]; then
  rm -rf /tmp/zeta_tests /tmp/zt_out
  mkdir -p /tmp/zeta_tests /tmp/zt_out
  # shellcheck disable=SC2086
  cp $ROOT/tests/unit-tests/*.z /tmp/zeta_tests/ 2>/dev/null || true
  while IFS= read -r -d '' z; do
    official_total=$((official_total + 1))
    n=$(basename "$z" .z)
    if "$ZETAC" "$z" -o "/tmp/zt_out/$n" >/dev/null 2>&1; then
      official_pass=$((official_pass + 1))
    fi
  done < <(find /tmp/zeta_tests -maxdepth 1 -name '*.z' -print0 | sort -z)
  [[ $JSON_ONLY -eq 0 ]] && echo "official: ${official_pass}/${official_total}"
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

# ── JSON summary (single source of truth) ──
python3 - <<PY
import json
doc = {
  "ts": "$ts",
  "zetac": "$ZETAC",
  "official": {"pass": $official_pass, "total": $official_total},
  "python_style": {
    "pass": $py_pass, "fail": $py_fail,
    "known_fail": $py_known, "xpass": $py_xpass,
  },
  "corpus": {"parse_ok": $corpus_ok, "total": $corpus_total},
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
if [[ $SKIP_OFFICIAL -eq 0 && $official_pass -ne $official_total ]]; then rc=1; fi
if [[ $SKIP_PYTHON -eq 0 && $py_fail -ne 0 ]]; then rc=1; fi
# corpus: parse_ok should equal total when suite is healthy; warn-only if skipped dirs empty
if [[ $SKIP_CORPUS -eq 0 && $corpus_total -gt 0 && $corpus_ok -ne $corpus_total ]]; then rc=1; fi
exit $rc
