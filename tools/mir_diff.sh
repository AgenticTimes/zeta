#!/usr/bin/env bash
# tools/mir_diff.sh — T0 (refactor.md B.5): a MIR baseline that can prove a
# refactor was pure code motion.
#
# Usage:
#   ./tools/mir_diff.sh snapshot [dir]   compile each corpus file twice, store the
#                                        canonical MIR dump as the baseline
#   ./tools/mir_diff.sh diff     [dir]   recompile and diff against the baseline
#
# `dir` defaults to $MIR_DIFF_DIR or /tmp/zeta_mir_baseline (kept out of the
# repo: the dumps are ~300k lines per file).
#
# Corpus = the same discovery as tools/corpus_baseline.py
# (~/source/quant/REasyQuant/strategies/**/*.py, minus .venv). Extra paths may be
# appended as `--file <path>` (repeatable) — used by tests of this tool.
#
# Exit status: 0 when nothing CHANGED. Files added/removed since the snapshot and
# files whose two snapshot compiles disagreed (UNSTABLE) are reported but do not
# fail — the mainline agent swaps its driver in and out of the corpus, and
# instability is a finding to fix, not a reason to blind the diff.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
DIR="${MIR_DIFF_DIR:-/tmp/zeta_mir_baseline}"
SNAPSHOT_RUNS="${MIR_SNAPSHOT_RUNS:-2}"
EXTRA_FILES=()

MODE="${1:-}"
case "$MODE" in
  snapshot|diff) shift ;;
  *) sed -n '2,20p' "$0"; exit 2 ;;
esac
while [[ $# -gt 0 ]]; do
  case "$1" in
    --file) EXTRA_FILES+=("$2"); shift 2 ;;
    --runs) SNAPSHOT_RUNS="$2"; shift 2 ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) DIR="$1"; shift ;;
  esac
done

if [[ ! -x "$ZETAC" ]]; then
  echo "building zetac..." >&2
  cargo build --release -p zetac -q
fi

CORPUS_ROOT="${MIR_CORPUS:-$HOME/source/quant/REasyQuant/strategies}"
# bash 3.2 (macOS default) has no `mapfile`.
FILES=()
while IFS= read -r line; do FILES+=("$line"); done < <(
  {
    # `|| true`: under `set -e` a missing CORPUS_ROOT made find abort the whole
    # grouping, so `--file` inputs vanished silently and the run reported
    # "no input files" instead of scanning what it was given.
    find "$CORPUS_ROOT" -name '*.py' -not -path '*/.venv/*' 2>/dev/null || true
    for f in ${EXTRA_FILES[@]+"${EXTRA_FILES[@]}"}; do echo "$f"; done
  } | sort
)
if [[ ${#FILES[@]} -eq 0 ]]; then
  echo "mir_diff: no input files (CORPUS_ROOT=$CORPUS_ROOT)" >&2
  exit 2
fi

key() { echo "$1" | tr '/ ' '__' | tr -c 'A-Za-z0-9_.-' '_'; }
SHOW="${MIR_DIFF_SHOW:-0}"

OUT_BIN="${TMPDIR:-/tmp}/mir_diff_probe"
# `timeout` is a coreutils binary — optional; without it a hung compile hangs
# the snapshot instead of failing it.
if command -v timeout >/dev/null 2>&1; then TIMEOUT="timeout 180"; else TIMEOUT=""; fi

# One `--dump-mir` run → canonical text on stdout. A non-zero exit is normal
# here (link failures are the next stage's problem); the dump is already written.
dump() {
  local src="$1" dst="$2"
  # shellcheck disable=SC2086
  $TIMEOUT "$ZETAC" --dump-mir "$src" -o "$OUT_BIN" >"$dst" 2>/dev/null || true
}

mkdir -p "$DIR"

total=0; stable=0; unstable=0; changed=0; added=0; removed=0
unstable_list=(); changed_list=()

if [[ "$MODE" == "snapshot" ]]; then
  for f in "${FILES[@]}"; do
    total=$((total + 1))
    k=$(key "$f")
    echo "$f" > "$DIR/$k.src"
    first="$DIR/$k.dump"
    dump "$f" "$first"
    ok=1
    if [[ "$SNAPSHOT_RUNS" -gt 1 ]]; then
      tmp=$(mktemp)
      for ((i = 2; i <= SNAPSHOT_RUNS; i++)); do
        dump "$f" "$tmp"
        if ! cmp -s "$first" "$tmp"; then ok=0; break; fi
      done
      rm -f "$tmp"
    fi
    if [[ $ok -eq 1 ]]; then
      stable=$((stable + 1))
    else
      unstable=$((unstable + 1))
      unstable_list+=("$(basename "$f")")
      # Keep the baseline anyway: `diff` then compares against one arbitrary
      # member of the unstable set, and the file stays flagged.
    fi
  done
  printf 'mir_diff snapshot: total=%d stable=%d unstable=%d dir=%s\n' \
    "$total" "$stable" "$unstable" "$DIR"
  if [[ ${#unstable_list[@]} -gt 0 ]]; then
    printf '  UNSTABLE: %s\n' "${unstable_list[*]}"
  fi
  # An unstable file cannot certify "pure code motion" — fail the snapshot so it
  # gets fixed instead of being tolerated silently.
  if [[ $unstable -gt 0 ]]; then exit 1; fi
  exit 0
fi

# ── diff ──
for f in "${FILES[@]}"; do
  total=$((total + 1))
  k=$(key "$f")
  base="$DIR/$k.dump"
  cur=$(mktemp)
  dump "$f" "$cur"
  if [[ ! -f "$base" ]]; then
    added=$((added + 1))
  elif cmp -s "$base" "$cur"; then
    stable=$((stable + 1))
  else
    changed=$((changed + 1))
    changed_list+=("$(basename "$f")")
    if [[ "$SHOW" == "1" ]]; then
      echo "--- $f" >&2
      diff -u "$base" "$cur" | head -60 >&2 || true
    fi
  fi
  rm -f "$cur"
done
for base in "$DIR"/*.dump; do
  [[ -e "$base" ]] || continue
  k=${base##*/}; k=${k%.dump}
  [[ -f "$DIR/$k.src" ]] || continue
  if ! printf '%s\n' "${FILES[@]}" | grep -qxF "$(cat "$DIR/$k.src")"; then
    removed=$((removed + 1))
  fi
done

printf 'mir_diff diff: total=%d same=%d changed=%d added=%d removed=%d dir=%s\n' \
  "$total" "$stable" "$changed" "$added" "$removed" "$DIR"
if [[ ${#changed_list[@]} -gt 0 ]]; then
  printf '  CHANGED: %s\n' "${changed_list[*]}"
fi
if [[ "$SHOW" != "1" && $changed -gt 0 ]]; then
  echo '  (re-run with MIR_DIFF_SHOW=1 for the diff hunks)' >&2
fi

if [[ $changed -gt 0 ]]; then exit 1; fi
exit 0
