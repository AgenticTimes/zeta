#!/usr/bin/env bash
# tests/python_style/run.sh — Python 风格测试套件
# 用例格式：`// expect: <一行输出>`（按序）；`// expect-error`（编译必须失败）；
#           `// known-fail: <原因>`（已知缺口，单列不计入通过率）
set -u

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ZETAC="$ROOT/target/release/zetac"
OUTDIR="$(mktemp -d /tmp/zeta_pytests.XXXXXX)"
trap 'rm -rf "$OUTDIR"' EXIT

if [ ! -x "$ZETAC" ]; then
    echo "building zetac (release)..."
    (cd "$ROOT" && cargo build --release -q) || { echo "build failed"; exit 1; }
fi

pass=0; fail=0; knownfail=0; xpass=0; failed_files=""

for f in "$ROOT"/tests/python_style/t*.z; do
    name="$(basename "$f" .z)"
    expects=()
    while IFS= read -r line; do
        case "$line" in
            "// expect: "*) expects+=("${line#// expect: }") ;;
            "// expect:")    expects+=("") ;;
        esac
    done < "$f"

    known=$(grep -c '^// known-fail:' "$f" || true)
    want_error=$(grep -c '^// expect-error' "$f" || true)

    # known-fail: 单列；若意外通过则报 XPASS
    if [ "$known" != "0" ]; then
        "$ZETAC" "$f" -o "$OUTDIR/$name" >/dev/null 2>&1
        if [ $? -eq 0 ] && [ "$want_error" = "0" ]; then
            echo "XPASS      $name (known-fail 但已能编译，可摘除标记)"
            xpass=$((xpass+1))
        else
            echo "KNOWN-FAIL $name ($(grep '^// known-fail:' "$f" | head -1 | sed 's|^// known-fail: ||'))"
            knownfail=$((knownfail+1))
        fi
        continue
    fi

    # 负面用例：编译必须失败
    if [ "$want_error" != "0" ]; then
        if "$ZETAC" "$f" -o "$OUTDIR/$name" >/dev/null 2>&1; then
            echo "FAIL       $name (期望编译报错，却编译成功)"
            fail=$((fail+1)); failed_files="$failed_files $name"
        else
            echo "PASS       $name"
            pass=$((pass+1))
        fi
        continue
    fi

    if ! "$ZETAC" "$f" -o "$OUTDIR/$name" >"$OUTDIR/$name.cc" 2>&1; then
        echo "FAIL       $name (编译失败: $(tail -1 "$OUTDIR/$name.cc"))"
        fail=$((fail+1)); failed_files="$failed_files $name"
        continue
    fi
    actual=$("$OUTDIR/$name" 2>/dev/null)
    # 逐行比对（尾随空行归一化）
    expected="$(printf '%s\n' "${expects[@]:-}" | sed -e ':a' -e '/^[[:space:]]*$/{$d;N;ba' -e '}')"
    actual_n="$(printf '%s\n' "$actual" | sed -e ':a' -e '/^[[:space:]]*$/{$d;N;ba' -e '}')"
    if [ "$expected" = "$actual_n" ] && [ ${#expects[@]} -gt 0 ]; then
        echo "PASS       $name"
        pass=$((pass+1))
    else
        echo "FAIL       $name"
        echo "  expected: $(printf '%s | ' "${expects[@]:-}")"
        echo "  actual:   $(printf '%s | ' $actual)"
        fail=$((fail+1)); failed_files="$failed_files $name"
    fi
done

echo "----------------------------------------"
echo "python_style: $pass passed, $fail failed, $knownfail known-fail, $xpass xpass"
[ -n "$failed_files" ] && echo "failed:$failed_files"
[ "$fail" -eq 0 ]
