#!/usr/bin/env bash
# tests/python_style/run.sh — Python 风格测试套件
# 用例格式：`// expect: <一行输出>`（按序）；`// expect-error`（编译必须失败）；
#           `// expect-no-compile: <子串>`（编译必须成功，且 stderr 不含该串 —— 锁误报）；
#           `// expect-abort: <stderr 子串>`（编译成功、运行必须非 0 且 stderr 含该串）
#           `// args: <argv...>`（可选，运行程序时传入的命令行参数）
#           `// env: K=V`（可选，编译/运行该用例时的环境变量）
#           `// known-fail: <原因>`（已知缺口：照常编译+运行并比对 expect，
#           值不符/编译失败/abort 记 KNOWN-FAIL 不计通过率；打印与 expect
#           逐字相同才记 XPASS，即"标记可摘除"）
#
# 批次 461（backlog #23 的并行化半步）：单用例逻辑抽到同目录 run_one.sh，
# 本脚本按 PY_JOBS 分片并行派发、按用例名排序聚合——判定行的内容与串行版
# 逐字相同，只有输出顺序从"文件序"变成"用例名序"（本来也是同一顺序）。
# PY_JOBS=1 回到纯串行；默认 PY_JOBS=核数（上限 8）。每用例一个私有目录，
# 互不共享文件，并行安全。
set -u

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ZETAC="$ROOT/target/release/zetac"
ONE="$ROOT/tests/python_style/run_one.sh"

if [ ! -x "$ZETAC" ]; then
    echo "building zetac (release)..."
    (cd "$ROOT" && cargo build --release -q) || { echo "build failed"; exit 1; }
fi

JOBS="${PY_JOBS:-$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)}"
case "$JOBS" in ''|*[!0-9]*) JOBS=4 ;; esac
[ "$JOBS" -lt 1 ] && JOBS=1
[ "$JOBS" -gt 24 ] && JOBS=24

CASES=("$ROOT"/tests/python_style/t*.z)
WORK="$(mktemp -d /tmp/zeta_pytests.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

# 派发：worker w 处理下标 w, w+JOBS, w+2JOBS, …（各自写各自的用例目录）
w=0
while [ "$w" -lt "$JOBS" ]; do
    (
        i=$w
        n=${#CASES[@]}
        while [ "$i" -lt "$n" ]; do
            f="${CASES[$i]}"
            cname="$(basename "$f" .z)"
            mkdir -p "$WORK/$cname"
            "$ONE" "$f" "$WORK/$cname" || echo "FAIL       $cname (worker 内部错误)" > "$WORK/$cname/verdict"
            i=$((i + JOBS))
        done
    ) &
    w=$((w + 1))
done
wait

# 聚合：按用例名排序（与串行版 glob 序一致），判定行与详情行原样打印
pass=0; fail=0; knownfail=0; xpass=0; failed_files=""
for f in "${CASES[@]}"; do
    name="$(basename "$f" .z)"
    v="$WORK/$name/verdict"
    [ -f "$v" ] || { echo "FAIL       $name (无判定输出)"; fail=$((fail + 1)); failed_files="$failed_files $name"; continue; }
    while IFS= read -r line; do echo "$line"; done < "$v"
    cls="$(head -1 "$v" | awk '{print $1}')"
    case "$cls" in
        PASS)       pass=$((pass + 1)) ;;
        XPASS)      xpass=$((xpass + 1)) ;;
        KNOWN-FAIL) knownfail=$((knownfail + 1)) ;;
        *)          fail=$((fail + 1)); failed_files="$failed_files $name" ;;
    esac
done

echo "----------------------------------------"
echo "python_style: $pass passed, $fail failed, $knownfail known-fail, $xpass xpass"
[ -n "$failed_files" ] && echo "failed:$failed_files"

# 任务 #34 (docs/ABI.md 附 B#9)：编译期告警必须在删目录**之前**聚合。判定不受
# 影响：比对读的是运行期 stdout，这里只是把编译期 stderr 变成可见的数字 +
# 去重后的头部若干条。排除 `clang: warning:`（链接器 -no-pie 噪声）。
diag_py=$(cat "$WORK"/*/*.cc 2>/dev/null | grep -v '^clang: warning' | grep -c 'warning:\|PY-A:' || true)
diag_py=${diag_py:-0}
diag_py_files=0
for c in "$WORK"/*/*.cc; do
    [ -f "$c" ] || continue
    if grep -v '^clang: warning' "$c" 2>/dev/null | grep -q 'warning:\|PY-A:'; then
        diag_py_files=$((diag_py_files + 1))
    fi
done
echo "compile-diagnostics: python_style ${diag_py} warning line(s) in ${diag_py_files} file(s)"
if [ "$diag_py" != "0" ]; then
    cat "$WORK"/*/*.cc 2>/dev/null | grep -v '^clang: warning' | grep 'warning:\|PY-A:' \
        | sed -E 's/[0-9]+/N/g' | sort | uniq -c | sort -rn | head -10
fi

[ "$fail" -eq 0 ]
