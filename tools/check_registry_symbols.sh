#!/usr/bin/env bash
# tools/check_registry_symbols.sh — Q2/A1 (advice.md)
# Extract F/W/X symbols from pylib/registry.txt and verify each exists in
# zeta_runtime_c.o and/or tokio_runtime.o (nm).
# Lines with decl=0 are skipped (intentionally undeclared).
# stub=1 with default decl still requires a C abort wrapper (task D).
# Exit 0 = all present. Exit 1 = missing symbols. Zero stdout when clean.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

REG="${REG:-pylib/registry.txt}"
OBJ_C="${OBJ_C:-zeta_runtime_c.o}"
OBJ_T="${OBJ_T:-tokio_runtime.o}"

if [[ ! -f "$REG" ]]; then
  echo "missing $REG" >&2
  exit 2
fi
if [[ ! -f "$OBJ_C" && ! -f "$OBJ_T" ]]; then
  echo "missing both $OBJ_C and $OBJ_T — rebuild runtime first (validate.md §4)" >&2
  exit 2
fi

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

{
  [[ -f "$OBJ_C" ]] && nm -gU "$OBJ_C" 2>/dev/null || true
  [[ -f "$OBJ_T" ]] && nm -gU "$OBJ_T" 2>/dev/null || true
} | awk '/ [TDB] / { s=$NF; sub(/^_/,"",s); print s }' | sort -u >"$tmpdir/defined.txt"

awk '
  /^[FWX] / && /decl=0/ { next }
  /^F / { print $4 }
  /^W / { print $4 }
  /^X / { print $2 }
' "$REG" | grep -v '^$' | sort -u >"$tmpdir/wanted.txt"

missing=0
while IFS= read -r sym; do
  [[ -z "$sym" ]] && continue
  if ! grep -qxF "$sym" "$tmpdir/defined.txt"; then
    echo "MISSING $sym"
    missing=$((missing + 1))
  fi
done <"$tmpdir/wanted.txt"

if [[ $missing -gt 0 ]]; then
  echo "check_registry_symbols: $missing missing (of $(wc -l <"$tmpdir/wanted.txt") wanted)" >&2
  exit 1
fi
exit 0
