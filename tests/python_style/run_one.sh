#!/usr/bin/env bash
# tests/python_style/run_one.sh — 单用例 worker（批次 461 / backlog #23 的并行化半步）。
#
# 用法：run_one.sh <t*.z 路径> <该用例的私有目录>
# 产出：
#   <dir>/verdict   — 判定输出（可多行；由 run.sh 按用例名排序后原样打印）
#   <dir>/<name>.cc — 编译 stderr 原样（供 run.sh 的 compile-diagnostics 聚合）
#
# 判定逻辑与并行化前的 run.sh 循环体逐字等价：批次 304 的按值判定，
# expect-error / expect-abort / expect-no-compile / args / env / known-fail
# 全部保留。run.sh 只负责派发、排序聚合与总数——一个用例一个目录，互不共享
# 任何文件，因此并行安全。
set -u

f="$1"; wd="$2"
mkdir -p "$wd"
ZETAC="${ZETAC:-$(cd "$(dirname "$0")/../.." && pwd)/target/release/zetac}"
name="$(basename "$f" .z)"
V="$wd/verdict"
: > "$V"

# 用例名先于一切：判定行的第二列就是它
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

if [ ${#envs[@]} -gt 0 ]; then
    ZC=(env "${envs[@]}" "$ZETAC")
else
    ZC=("$ZETAC")
fi

known=$(grep -c '^// known-fail:' "$f" || true)
want_error=$(grep -c '^// expect-error' "$f" || true)
want_abort=$(grep '^// expect-abort:' "$f" | head -1 | sed 's|^// expect-abort: ||' || true)
if [ "$known" != "0" ]; then
    # 已知失败只按值判定："拒编"与"abort"是缺口本身，不是契约（批次 304）。
    want_error=0
    want_abort=""
fi

verdict() { # $1 = ok|bad, $2 = detail(可空)
    if [ "$known" != "0" ]; then
        if [ "$1" = ok ]; then
            echo "XPASS      $name (known-fail 已达成预期，可摘除标记)" >> "$V"
        else
            rsn=$(grep '^// known-fail:' "$f" | head -1 | sed 's|^// known-fail: ||')
            echo "KNOWN-FAIL $name (${rsn}${2:+; $2})" >> "$V"
        fi
    elif [ "$1" = ok ]; then
        echo "PASS       $name" >> "$V"
    else
        echo "FAIL       $name${2:+ ($2)}" >> "$V"
    fi
}

# 负面用例：编译必须失败
if [ "$want_error" != "0" ]; then
    if "${ZC[@]}" "$f" -o "$wd/$name" >/dev/null 2>&1; then
        verdict bad "期望编译报错，却编译成功"
    else
        verdict ok
    fi
    exit 0
fi

if ! "${ZC[@]}" "$f" -o "$wd/$name" >"$wd/$name.cc" 2>&1; then
    verdict bad "编译失败: $(tail -1 "$wd/$name.cc")"
    exit 0
fi

# 批次405：`// expect-no-compile: <子串>` —— 编译 stderr 不得含该串。
want_no=$(grep '^// expect-no-compile:' "$f" | head -1 | sed 's|^// expect-no-compile: ||' || true)
if [ -n "$want_no" ] && grep -qF "$want_no" "$wd/$name.cc"; then
    verdict bad "编译 stderr 含不该出现的: $want_no"
    exit 0
fi

# expect-abort：运行必须非 0 且 stderr 含期望子串
if [ -n "$want_abort" ]; then
    set +e
    if [ ${#envs[@]} -gt 0 ]; then
        err=$(timeout 20 env "${envs[@]}" "$wd/$name" 2>&1 >/dev/null)
        rc=$?
    else
        err=$(timeout 20 "$wd/$name" 2>&1 >/dev/null)
        rc=$?
    fi
    set -u
    if [ "$rc" -eq 0 ]; then
        verdict bad "期望 abort，却退出 0"
    elif printf '%s' "$err" | grep -qF "$want_abort"; then
        verdict ok
    else
        verdict bad "stderr 未含: $want_abort"
        echo "  stderr: $(printf '%s' "$err" | tr '\n' ' ' | head -c 200)" >> "$V"
    fi
    exit 0
fi

# ⚠️ `env "${envs[@]}"` 空数组会展开成 `env "" prog`，必须按空 分支（同 run.sh 原版）。
# timeout 20：挂死的程序不许拖住整个套件（曾有一次 for…continue 挂了 30 分钟）。
if [ ${#envs[@]} -gt 0 ] && [ ${#args[@]} -gt 0 ]; then
    actual=$(timeout 20 env "${envs[@]}" "$wd/$name" "${args[@]}" 2>/dev/null)
elif [ ${#envs[@]} -gt 0 ]; then
    actual=$(timeout 20 env "${envs[@]}" "$wd/$name" 2>/dev/null)
elif [ ${#args[@]} -gt 0 ]; then
    actual=$(timeout 20 "$wd/$name" "${args[@]}" 2>/dev/null)
else
    actual=$(timeout 20 "$wd/$name" 2>/dev/null)
fi

expected="$(printf '%s\n' "${expects[@]:-}" | sed -e ':a' -e '/^[[:space:]]*$/{$d;N;ba' -e '}')"
actual_n="$(printf '%s\n' "$actual" | sed -e ':a' -e '/^[[:space:]]*$/{$d;N;ba' -e '}')"
if [ ${#expects[@]} -gt 0 ] && [ "$expected" = "$actual_n" ]; then
    verdict ok
elif [ ${#expects[@]} -eq 0 ] && [ "$known" != "0" ]; then
    # 没有值断言可判：编译+链接成功就是全部主张
    verdict ok
else
    verdict bad
    echo "  expected: $(printf '%s | ' "${expects[@]:-}")" >> "$V"
    echo "  actual:   $(printf '%s | ' "$actual")" >> "$V"
fi
