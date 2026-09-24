#!/bin/bash
# 全语料扫一条警告行：$1 = 匹配串（grep -c 的 -e），输出每个文件的命中数（只列 >0）
ZETAC="${ZETAC:-./target/release/zetac}"
PAT="${1:?pattern}"
OUTDIR="${2:-/tmp/warn_sweep}"
mkdir -p "$OUTDIR"; : > "$OUTDIR/hits.txt"
export ZETAC PAT OUTDIR
find tests examples pylib -name '*.z' -not -path '*/target/*' -print0 \
  | xargs -P 8 -0 bash -c '
      for z in "$@"; do
        log="$OUTDIR/$(echo "$z" | tr / _).log"
        "$ZETAC" "$z" --no-link -o "$OUTDIR/$(echo "$z" | tr / _).out" >/dev/null 2>"$log"
        c=$(grep -c -e "$PAT" "$log")
        [ "$c" -gt 0 ] && printf "%s\t%s\n" "$c" "$z" >> "$OUTDIR/hits.txt"
      done ' _
sort -rn "$OUTDIR/hits.txt" -o "$OUTDIR/hits.txt" 2>/dev/null || true
printf '总命中 %s 行 / %s 文件\n' "$(awk -F'\t' '{s+=$1} END{print s+0}' "$OUTDIR/hits.txt")" "$(wc -l < "$OUTDIR/hits.txt" | tr -d ' ')"
