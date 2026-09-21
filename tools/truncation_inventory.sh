#!/usr/bin/env bash
# tools/truncation_inventory.sh — G.7a (refactor.md): the inventory behind the
# W1002 warning, i.e. WHICH files lose their tail and how much of each.
#
# The compiler already fails loudly (`ensure_fully_parsed` prints
# `[W1002] <N> line(s) at the end of the input were NOT parsed …`), but a
# warning you have to trigger one compile at a time is not a plan input. This
# walks a whole suite and lists every affected file, so G.7b (top-level
# sync recovery) can be scoped and verified file by file instead of guessed.
#
# Usage:
#   ./tools/truncation_inventory.sh                # official suite + corpus
#   ./tools/truncation_inventory.sh <path> …       # explicit files or dirs
#
# Env: MIR_CORPUS (corpus root, same discovery as tools/mir_diff.sh),
#      ZETAC (compiler binary).
#
# Read-only report: it never fails the build (exit 0) — the gate stays
# tools/run_all.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
if [[ ! -x "$ZETAC" ]]; then
  echo "building zetac..." >&2
  cargo build --release -p zetac -q
fi

FILES=()
if [[ $# -gt 0 ]]; then
  for p in "$@"; do
    if [[ -d "$p" ]]; then
      while IFS= read -r line; do FILES+=("$line"); done < <(find "$p" -type f \( -name '*.z' -o -name '*.py' \) | sort)
    else
      FILES+=("$p")
    fi
  done
else
  CORPUS_ROOT="${MIR_CORPUS:-$HOME/source/quant/REasyQuant/strategies}"
  while IFS= read -r line; do FILES+=("$line"); done < <(
    {
      find "$ROOT/tests/unit-tests" -name '*.z'
      find "$CORPUS_ROOT" -name '*.py' -not -path '*/.venv/*' 2>/dev/null
    } | sort
  )
fi

if command -v timeout >/dev/null 2>&1; then TIMEOUT="timeout 180"; else TIMEOUT=""; fi
OUT_BIN="${TMPDIR:-/tmp}/truncation_inventory_probe"

# W1002 carries the count and the (mapped) original line; the snippet is on the
# same message, so one grep per file is enough.
ROWS=""
for f in ${FILES[@]+"${FILES[@]}"}; do
  # shellcheck disable=SC2086
  log=$($TIMEOUT "$ZETAC" --dump-mir "$f" -o "$OUT_BIN" 2>&1 >/dev/null || true)
  while IFS= read -r hit; do
    [[ -n "$hit" ]] || continue
    # "warning: [W1002] <path>:<line>: <N> line(s) … First unparsed text: '<snippet>'"
    n=$(printf '%s' "$hit" | sed -n 's/.*\[W1002\] [^ ]* \([0-9][0-9]*\) line.*/\1/p')
    at=$(printf '%s' "$hit" | sed -n 's/.*\[W1002\] \([^ ]*:[0-9]*\):.*/\1/p')
    snippet=$(printf '%s' "$hit" | sed -n "s/.*First unparsed text: '\(.\{1,48\}\).*/\1/p")
    ROWS+=$(printf '%8s  %s  @%s  |%s|\n' "${n:-?}" "$f" "${at:-?}" "${snippet:-?}")
    ROWS+=$'\n'
  done < <(printf '%s\n' "$log" | grep '\[W1002\]' || true)
done

total=$(printf '%s' "$ROWS" | grep -c . || true)
echo "truncation inventory: $total W1002 hit(s) over ${#FILES[@]} file(s)"
if [[ "$total" -gt 0 ]]; then
  printf '%s' "$ROWS" | sort -rn
fi
