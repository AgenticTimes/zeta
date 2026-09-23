#!/usr/bin/env bash
# tools/asan_run.sh — G.2（refactor.md）：把 C 运行期换成 ASan 插桩版本，编译并运行
# 目标程序，输出一份"命中清单"。只读工具：不改源文件、不覆盖仓库根的 .o。
#
# 用法：
#   tools/asan_run.sh                    扫 tests/python_style/t*.z
#   tools/asan_run.sh a.z b.py ...       只扫给定文件
#   tools/asan_run.sh --rebuild ...      强制重编 ASan 目标文件
#   tools/asan_run.sh --strict ...       有 ASan 命中则退出码 1（nightly 用）
#   tools/asan_run.sh --selftest         自检：证明工具能看见已知越界 + 复现 GC 盲区
#
# 覆盖范围（诚实口径，别把"零命中"读成"没有内存错误"）：
#   * 插桩的是 runtime/*.c； zetac 生成的目标代码**不**插桩（需要 LLVM 的 ASan
#     pass 且 IR 里指针/i64 不混用——那是轴 B/F 的活，不是本脚本的）。
#   * 容器（vec/map/struct）走 Boehm GC：GC 自己只向系统要大块，块内相邻分配
#     之间**没有 redzone**，所以"写过了 vec 头 8 字节"这类越界 ASan 看不见；
#     ASan 看得见的是：运行期 C 的栈/全局越界、wild pointer 解引用、
#     非 GC 堆（strdup/malloc）的 overflow/use-after-free、以及把整数当指针
#     解引用（正好是批次 291/297 那一类）。
#   * ASAN_OPTIONS=detect_leaks=0 是必须的：GC 保留的块会被泄漏检查全部误报。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 1
ZETAC="$ROOT/target/release/zetac"
OBJ="${ZETA_ASAN_OBJDIR:-/tmp/zeta_asan_rt}"
LOG="${ZETA_ASAN_LOGDIR:-/tmp/zeta_asan_logs}"

REBUILD=0
STRICT=0
SELFTEST=0
FILES=()
for a in "$@"; do
    case "$a" in
        --rebuild)  REBUILD=1 ;;
        --strict)   STRICT=1 ;;
        --selftest) SELFTEST=1 ;;
        *)          FILES+=("$a") ;;
    esac
done

[ -x "$ZETAC" ] || { echo "需要 ${ZETAC}（先 cargo build --release -p zetac）" >&2; exit 2; }

INC=(-I/opt/homebrew/include -I"$ROOT/runtime")
# 必须用**和 zetac 链接器同一个** clang 编译插桩对象：zetac 走 `gcc`（Apple clang 17），
# 而 PATH 里的 `clang` 是 Homebrew clang 21。混用的后果不是警告而是拒链——
# 每个 ASan 对象都引一个发行版专属符号
# `___asan_version_mismatch_check_apple_clang_1700` vs `..._v8`，实测 Homebrew 编、
# gcc 链 = "Undefined symbols: ___asan_version_mismatch_check_v8"。
ASANC="${ZETA_ASAN_CC:-gcc}"
# -g + 帧指针：报告可读。注意 -O2 会让部分被检访存消失（实测 -O1 下
# malloc(8) 后写 p[9] 会被 dead-store 消除而完全不过 shadow memory）。
ASAN_FLAGS=(-fsanitize=address -O1 -g -fno-omit-frame-pointer)

if [ "$REBUILD" = 1 ] || [ ! -f "$OBJ/zeta_runtime_c.o" ] || [ ! -f "$OBJ/tokio_runtime.o" ]; then
    mkdir -p "$OBJ" || exit 2
    echo "building ASan runtime objects into $OBJ (compiler: $ASANC) ..."
    "$ASANC" -c "${ASAN_FLAGS[@]}" "${INC[@]}" "$ROOT/runtime/py_additions.c" \
        -o "$OBJ/zt_pyadd.o" || { echo "ASan: py_additions.c 编译失败" >&2; exit 2; }
    "$ASANC" -c "${ASAN_FLAGS[@]}" "${INC[@]}" "$ROOT/runtime/parquet_min.c" \
        -o "$OBJ/zt_pq.o" || { echo "ASan: parquet_min.c 编译失败" >&2; exit 2; }
    ld -r "$OBJ/zt_pyadd.o" "$OBJ/zt_pq.o" -o "$OBJ/zeta_runtime_c.o" || exit 2
    "$ASANC" -c "${ASAN_FLAGS[@]}" "${INC[@]}" -DZT_REAL_ASYNC \
        "$ROOT/runtime/tokio_runtime_stub.c" -o "$OBJ/zt_stub.o" \
        || { echo "ASan: tokio_runtime_stub.c 编译失败" >&2; exit 2; }
    EXTRA=()
    if [ -f "$ROOT/runtime/unavailable_stubs.c" ]; then
        "$ASANC" -c "${ASAN_FLAGS[@]}" "${INC[@]}" "$ROOT/runtime/unavailable_stubs.c" \
            -o "$OBJ/zt_unavail.o" && EXTRA=("$OBJ/zt_unavail.o")
    fi
    ld -r "$OBJ/zt_stub.o" "${EXTRA[@]}" -o "$OBJ/tokio_runtime.o" || exit 2
    echo "ok: ASan 目标文件已生成"
fi

if [ "${SELFTEST:-0}" = 1 ]; then
    # 自检：证明"本工具能看见一个已知越界"，并复现 GC 盲区（0 命中因此不能读成干净）。
    mkdir -p "$LOG"
    cat > "$LOG/asan_ctl.c" <<'CEOF'
#include <gc.h>
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char** argv) {
    if (argc > 1 && argv[1][0] == 'g') {
        GC_init();
        char* q = (char*)GC_malloc(64);
        q[72] = 1;
        printf("GC case wrote %d\n", (int)q[72]);
    } else {
        char* p = (char*)malloc(64);
        p[72] = 1;
        printf("malloc case wrote %d\n", (int)p[72]);
    }
    return 0;
}
CEOF
    "$ASANC" -fsanitize=address -O0 -g -I/opt/homebrew/include "$LOG/asan_ctl.c" \
        -o "$LOG/asan_ctl" -L/opt/homebrew/opt/bdw-gc/lib -lgc || exit 2
    # malloc 用例是被 ASan 以 SIGABRT 杀掉的，bash 会替我们打一行"Abort trap: 6"。
    # 与主循环同一处理：脚本自身 stderr 收进 notices 文件，之后只回显真正的异常行。
    { ( "$LOG/asan_ctl" m >"$LOG/asan_ctl.m.out" 2>&1 ); mrc=$?
      ( "$LOG/asan_ctl" g >"$LOG/asan_ctl.g.out" 2>&1 ); grc=$?
    } 2>"$LOG/selftest.notices"
    grep -q "ERROR: AddressSanitizer" "$LOG/asan_ctl.m.out" && m=detected || m=MISSED
    grep -q "ERROR: AddressSanitizer" "$LOG/asan_ctl.g.out" && g=detected || g=silent
    printf 'selftest: malloc-overflow=%s (rc=%d)  GC-overflow=%s (rc=%d)\n' "$m" "$mrc" "$g" "$grc"
    echo "期望：malloc-overflow=detected、GC-overflow=silent（后者即 GC 盲区，见头部口径）"
    [ "$m" = detected ] && [ "$g" = silent ] && exit 0
    exit 1
fi

if [ ${#FILES[@]} -eq 0 ]; then
    FILES=("$ROOT"/tests/python_style/t*.z)
fi

mkdir -p "$LOG" && rm -f "$LOG"/*.log "$LOG"/*.hit 2>/dev/null
BIN="$LOG/bin"
mkdir -p "$BIN"

export ZETA_RUNTIME_DIR="$OBJ"
export ZETA_STRICT_RUNTIME_DIR=1
export ZETA_EXTRA_LDFLAGS="-fsanitize=address"
export ASAN_OPTIONS="detect_leaks=0:print_stacktrace=1:abort_on_error=0"
export DYLD_LIBRARY_PATH="/opt/homebrew/opt/bdw-gc/lib${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}"

hits=0; crashes=0; oks=0; cfails=0; exits=0; rows=""
for f in "${FILES[@]}"; do
    [ -f "$f" ] || continue
    name=$(basename "$f"); name="${name%.*}"
    log="$LOG/$name.log"
    # 与 run.sh 同口径：认 `// env:` 与 `// args:`；`// expect-error`（拒编即契约）
    # 和 `// expect-abort`（非零退出即契约）不参与崩溃判定。
    envs=(); args=(); want_error=0; want_abort=0
    while IFS= read -r line; do
        case "$line" in
            "// env: "*)  envs+=(${line#// env: }) ;;
            "// args: "*) args+=(${line#// args: }) ;;
            "// expect-error"*) want_error=1 ;;
            "// expect-abort:"*) want_abort=1 ;;
        esac
    done < "$f"

    compiled=1
    if [ ${#envs[@]} -gt 0 ]; then
        env "${envs[@]}" "$ZETAC" "$f" -o "$BIN/$name" >"$log.cc" 2>&1 || compiled=0
    else
        "$ZETAC" "$f" -o "$BIN/$name" >"$log.cc" 2>&1 || compiled=0
    fi
    if [ "$compiled" != 1 ]; then
        if [ "$want_error" = 1 ]; then oks=$((oks+1)); continue; fi
        cfails=$((cfails+1)); printf 'CFAIL %s  %s\n' "$name" "$(tail -1 "$log.cc")"
        continue
    fi
    [ "$want_error" = 1 ] && { cfails=$((cfails+1)); printf 'CFAIL %s  期望拒编却编过了\n' "$name"; continue; }

    # bash 3.2 + `set -u`：空数组不能直接展开（实测 "args[@]: unbound variable"）
    ARGV=(${args[@]+"${args[@]}"})
    # 子进程死于信号时 bash 自己会打一行作业提示（"Abort trap: 6"），换 shell/子 shell
    # 都躲不掉，所以把脚本自身 stderr 收进 notices 文件、循环后只回显异常行。
    if [ ${#envs[@]} -gt 0 ]; then
        env "${envs[@]}" timeout 30 "$BIN/$name" "${ARGV[@]+"${ARGV[@]}"}" 2>>"$log" >/dev/null
    else
        timeout 30 "$BIN/$name" "${ARGV[@]+"${ARGV[@]}"}" 2>>"$log" >/dev/null
    fi
    rc=$?

    kind=$(grep -m1 -o 'ERROR: AddressSanitizer: [a-z-]*' "$log" 2>/dev/null | sed 's/ERROR: AddressSanitizer: //')
    if [ -n "$kind" ]; then
        hits=$((hits+1))
        fn=$(grep -m2 -oE '#[0-9]+ 0x[0-9a-f]+ in [a-zA-Z_][a-zA-Z0-9_]*' "$log" \
             | sed -n '2p' | sed 's/.* in //')
        printf 'ASAN  %s  %s  (%s)\n' "$kind" "$name" "${fn:-?}"
        rows="$rows$kind	$name	${fn:-?}
"
        : > "$log.hit"
        continue
    fi
    # 注意：zetac 生成的 main 直接把表达式的 i64 值当退出码（实测 t48 正常输出却
    # rc=5），所以"非零退出"不是崩溃信号——单列 EXIT，run.sh 也只比 stdout。
    case "$rc" in
        0)   oks=$((oks+1)); continue;;
        124) why="timeout";;
        139) why="SEGV";;
        134) if [ "$want_abort" = 1 ]; then oks=$((oks+1)); continue; fi
             why="abort: $(grep -m1 'zeta: ' "$log" | cut -c1-70)";;
        *)   if [ "$want_abort" = 1 ]; then oks=$((oks+1)); continue; fi
             exits=$((exits+1)); printf 'EXIT  %s  rc=%s\n' "$name" "$rc"; continue;;
    esac
    crashes=$((crashes+1))
    printf 'CRASH %s  %s\n' "$name" "$why"
    rows="$rows$why	$name	-
"
done 2>"$LOG/notices.log"

echo "----------------------------------------"
printf 'asan sweep: %d program(s)  %d ASan-hit  %d crash  %d ok  %d exit!=0  %d compile-fail\n' \
    "$(printf '%s\n' "${FILES[@]}" | grep -c .)" "$hits" "$crashes" "$oks" "$exits" "$cfails"
if [ "$hits" != 0 ]; then
    echo "ASan 命中分类："
    printf '%s' "$rows" | awk -F'\t' '$1 !~ /^rc=|^SEGV$|^abort|^timeout|^CRASH/ {print $1}' \
        | sort | uniq -c | sort -rn
else
    echo "注：0 命中 ≠ 没有内存错误。容器全在 Boehm GC 堆里，块间无 redzone"
    echo "    （实测：GC_malloc(64) 后写 q[72] → ASan 静默、rc=0；同样越界写在 malloc(64) 上 → rc=134 报 heap-buffer-overflow）"
    echo "    要覆盖这一类需要 G.2b：给 vec/map/struct 分配加自带 canary（ZT_CONTAINER_GUARDS）。"
fi
echo "日志：$LOG/<name>.log（.hit 标记 = 该用例有 ASan 命中）"
# 作业提示是预期的（形如 "line 87: 12345 Abort trap: 6 …"）；其余（unbound variable
# 等脚本自身故障）必须露出来，所以只按死法字样过滤。
grep -vE "Abort trap: 6|Segmentation fault: 11|Killed: 9|^\s*$" "$LOG/notices.log" 2>/dev/null || true

if [ "$STRICT" = 1 ] && [ "$hits" != 0 ]; then exit 1; fi
exit 0
