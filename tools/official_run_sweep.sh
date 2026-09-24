#!/bin/bash
# 官方 tests/unit-tests 编译 + 链接 + **真运行**（门禁只看 compile/link，不看运行）。
# 用法: bash tools/official_run_sweep.sh [zettac 路径] [输出目录]
set -u
ZETAC="${1:-target/release/zetac}"
OUT="${2:-/tmp/zeta_official_run}"
SRC="tests/unit-tests"
mkdir -p "$OUT"
: > "$OUT/table.tsv"
export ZETAC OUT SRC
find "$SRC" -maxdepth 1 -name '*.z' -print0 \
  | xargs -P 8 -0 bash -c '
      for z in "$@"; do
        n=$(basename "$z" .z)
        cerr="$OUT/$n.cerr"
        if ! "$ZETAC" "$z" -o "$OUT/$n.bin" >"$cerr" 2>&1; then
          if grep -q "Linking failed" "$cerr"; then printf "%s\tLINK-FAIL\t-\t-\n" "$n" >> "$OUT/table.tsv"
          else printf "%s\tCOMPILE-FAIL\t-\t-\n" "$n" >> "$OUT/table.tsv"; fi
          continue
        fi
        o=$(cd "$OUT" && timeout 5 "./$n.bin" 2>/dev/null); rc=$?
        lines=$(printf "%s" "$o" | grep -c .)
        printf "%s\tOK\t%s\t%s\n" "$n" "$rc" "$lines" >> "$OUT/table.tsv"
      done
      ' _
sort "$OUT/table.tsv" -o "$OUT/table.tsv"
