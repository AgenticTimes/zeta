#!/usr/bin/env bash
# tools/build_runtime.sh — advice.md 任务 M / validate.md §4
# Rebuild tokio_runtime.o and zeta_runtime_c.o from C sources.
# Pass --gen to also regenerate registry decls + LLVM .N aliases.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

GEN=0
for a in "$@"; do
  case "$a" in --gen) GEN=1 ;; esac
done

if [[ $GEN -eq 1 ]]; then
  python3 tools/gen_from_registry.py --emit --emit-core --emit-jit --emit-aliases
fi

INC=(-I/opt/homebrew/include -Iruntime)
clang -c -O2 "${INC[@]}" -DZT_REAL_ASYNC runtime/tokio_runtime_stub.c -o /tmp/zt_stub.o

# batch 156: loud-abort scaffolding for symbols that are undefined at link time
# when compiling the local backtest entry — lets the binary LINK so the run can
# report which paths are actually reached.
if [[ -f runtime/unavailable_stubs.c ]]; then
  clang -c -O1 "${INC[@]}" runtime/unavailable_stubs.c -o /tmp/zt_unavail.o
  UNAVAIL=(/tmp/zt_unavail.o)
else
  UNAVAIL=()
fi

if [[ -f tokio_runtime.c ]]; then
  if clang -c -O2 "${INC[@]}" -DZT_REAL_ASYNC tokio_runtime.c -o /tmp/zt_async.o 2>/tmp/zt_async.err; then
    ld -r /tmp/zt_async.o /tmp/zt_stub.o "${UNAVAIL[@]}" -o tokio_runtime.o
  else
    echo "note: tokio_runtime.c skipped; stub-only tokio_runtime.o" >&2
    ld -r /tmp/zt_stub.o "${UNAVAIL[@]}" -o tokio_runtime.o
  fi
else
  ld -r /tmp/zt_stub.o "${UNAVAIL[@]}" -o tokio_runtime.o
fi

clang -c -O2 "${INC[@]}" runtime/py_additions.c -o zeta_runtime_c.o
clang -c -O2 "${INC[@]}" runtime/parquet_min.c -o /tmp/zt_pq.o
ld -r zeta_runtime_c.o /tmp/zt_pq.o -o zeta_runtime_c.o
echo "ok: tokio_runtime.o ($(nm tokio_runtime.o | rg -c ' T ' || true) T) + zeta_runtime_c.o"
