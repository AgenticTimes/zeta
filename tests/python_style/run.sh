#!/usr/bin/env bash
# tests/python_style/run.sh — Python 风格测试套件
# 用例格式：`// expect: <一行输出>`（按序）；`// expect-error`（编译必须失败）；
#           `// expect-abort: <stderr 子串>`（编译成功、运行必须非 0 且 stderr 含该串）
#           `// args: <argv...>`（可选，运行程序时传入的命令行参数）
#           `// env: K=V`（可选，编译/运行该用例时的环境变量）
#           `// known-fail: <原因>`（已知缺口：照常编译+运行并比对 expect，
#           值不符/编译失败/abort 记 KNOWN-FAIL 不计通过率；打印与 expect
#           逐字相同才记 XPASS，即"标记可摘除"）
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

# §5.2 (refactor.md): one verdict per case, routed through `known` so that
# `// known-fail:` means "this VALUE is known-wrong", not merely "it compiles".
# The old known-fail path only ran the compile step, so every compile-but-print-
# garbage gap reported XPASS and its marker was one "cleanup" away from being
# deleted off a case that is still broken. Reads `known`/`f` from the loop.
verdict() { # $1 = ok|bad, $2 = name, $3 = detail
    if [ "$known" != "0" ]; then
        if [ "$1" = ok ]; then
            echo "XPASS      $2 (known-fail 已达成预期，可摘除标记)"
            xpass=$((xpass + 1))
        else
            rsn=$(grep '^// known-fail:' "$f" | head -1 | sed 's|^// known-fail: ||')
            echo "KNOWN-FAIL $2 (${rsn}${3:+; $3})"
            knownfail=$((knownfail + 1))
        fi
    elif [ "$1" = ok ]; then
        echo "PASS       $2"
        pass=$((pass + 1))
    else
        echo "FAIL       $2${3:+ ($3)}"
        fail=$((fail + 1)); failed_files="$failed_files $2"
    fi
}

for f in "$ROOT"/tests/python_style/t*.z; do
    name="$(basename "$f" .z)"
    expects=()
    args=()
    envs=()
    while IFS= read -r line; do
        case "$line" in
            "// expect: "*) expects+=("${line#// expect: }") ;;
            "// expect:")    expects+=("") ;;
            "// args: "*)
                # shellcheck disable=SC2206
                args+=(${line#// args: })
                ;;
            "// env: "*)
                # shellcheck disable=SC2206
                envs+=(${line#// env: })
                ;;
        esac
    done < "$f"

    # 编译/运行时的环境变量（`// env: K=V`），空数组在 set -u 下不能直接展开
    if [ ${#envs[@]} -gt 0 ]; then
        ZC=(env "${envs[@]}" "$ZETAC")
    else
        ZC=("$ZETAC")
    fi

    known=$(grep -c '^// known-fail:' "$f" || true)
    want_error=$(grep -c '^// expect-error' "$f" || true)
    want_abort=$(grep '^// expect-abort:' "$f" | head -1 | sed 's|^// expect-abort: ||' || true)
    if [ "$known" != "0" ]; then
        # A known-fail case is judged by its expected OUTPUT only: "refuses to
        # compile" and "aborts" are the gap, not the contract.
        want_error=0
        want_abort=""
    fi

    # 负面用例：编译必须失败
    if [ "$want_error" != "0" ]; then
        if "${ZC[@]}" "$f" -o "$OUTDIR/$name" >/dev/null 2>&1; then
            verdict bad "$name" "期望编译报错，却编译成功"
        else
            verdict ok "$name" ""
        fi
        continue
    fi

    if ! "${ZC[@]}" "$f" -o "$OUTDIR/$name" >"$OUTDIR/$name.cc" 2>&1; then
        verdict bad "$name" "编译失败: $(tail -1 "$OUTDIR/$name.cc")"
        continue
    fi

    # D: 运行期响亮失败 — 必须非 0 退出且 stderr 含期望子串
    if [ -n "$want_abort" ]; then
        set +e
        if [ ${#envs[@]} -gt 0 ]; then
            err=$(timeout 20 env "${envs[@]}" "$OUTDIR/$name" 2>&1 >/dev/null)
            rc=$?
        else
            err=$(timeout 20 "$OUTDIR/$name" 2>&1 >/dev/null)
            rc=$?
        fi
        set -u
        if [ "$rc" -eq 0 ]; then
            verdict bad "$name" "期望 abort，却退出 0"
        elif printf '%s' "$err" | grep -qF "$want_abort"; then
            verdict ok "$name" ""
        else
            verdict bad "$name" "stderr 未含: $want_abort"
            echo "  stderr: $(printf '%s' "$err" | tr '\n' ' ' | head -c 200)"
        fi
        continue
    fi

    # ⚠️ `env "${envs[@]}"` with an EMPTY array expands to `env "" prog`, which
    # runs nothing at all — so branch on emptiness instead of expanding blindly.
    if [ ${#envs[@]} -gt 0 ] && [ ${#args[@]} -gt 0 ]; then
        # A hanging program must not stall the whole suite (a `for … continue`
        # hang once blocked a run for 30 minutes).
        actual=$(timeout 20 env "${envs[@]}" "$OUTDIR/$name" "${args[@]}" 2>/dev/null)
    elif [ ${#envs[@]} -gt 0 ]; then
        actual=$(timeout 20 env "${envs[@]}" "$OUTDIR/$name" 2>/dev/null)
    elif [ ${#args[@]} -gt 0 ]; then
        actual=$(timeout 20 "$OUTDIR/$name" "${args[@]}" 2>/dev/null)
    else
        actual=$(timeout 20 "$OUTDIR/$name" 2>/dev/null)
    fi
    # 逐行比对（尾随空行归一化）
    expected="$(printf '%s\n' "${expects[@]:-}" | sed -e ':a' -e '/^[[:space:]]*$/{$d;N;ba' -e '}')"
    actual_n="$(printf '%s\n' "$actual" | sed -e ':a' -e '/^[[:space:]]*$/{$d;N;ba' -e '}')"
    if [ ${#expects[@]} -gt 0 ] && [ "$expected" = "$actual_n" ]; then
        verdict ok "$name" ""
    elif [ ${#expects[@]} -eq 0 ] && [ "$known" != "0" ]; then
        # no value assertion to judge by: compile+link is the whole claim
        verdict ok "$name" ""
    else
        verdict bad "$name" ""
        echo "  expected: $(printf '%s | ' "${expects[@]:-}")"
        echo "  actual:   $(printf '%s | ' "$actual")"
    fi
done

echo "----------------------------------------"
echo "python_style: $pass passed, $fail failed, $knownfail known-fail, $xpass xpass"
[ -n "$failed_files" ] && echo "failed:$failed_files"

# 任务 #34 (docs/ABI.md 附 B#9)：编译期告警此前写进 $OUTDIR/$name.cc 就再没人读，
# 而 $OUTDIR 在 EXIT trap 里被整目录删掉 ⇒ 门禁日志收不到任何告警，"零告警"是在
# 空集上测的。必须在删目录**之前**聚合。判定不受影响：上面的比对读的是运行期
# stdout，这里只是把编译期 stderr 变成可见的数字 + 去重后的头部若干条。
# 排除 `clang: warning:`（链接器抱怨 -no-pie，工具链噪声，不排除会埋掉真信号）。
diag_py=$(grep -hv '^clang: warning' "$OUTDIR"/*.cc 2>/dev/null | grep -c 'warning:\|PY-A:' || true)
diag_py=${diag_py:-0}
diag_py_files=0
for c in "$OUTDIR"/*.cc; do
    if grep -v '^clang: warning' "$c" 2>/dev/null | grep -q 'warning:\|PY-A:'; then
        diag_py_files=$((diag_py_files + 1))
    fi
done
echo "compile-diagnostics: python_style ${diag_py} warning line(s) in ${diag_py_files} file(s)"
if [ "$diag_py" != "0" ]; then
    grep -hv '^clang: warning' "$OUTDIR"/*.cc 2>/dev/null | grep 'warning:\|PY-A:' \
        | sed -E 's/[0-9]+/N/g' | sort | uniq -c | sort -rn | head -10
fi

[ "$fail" -eq 0 ]
