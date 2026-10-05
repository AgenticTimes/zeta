#!/usr/bin/env bash
# tests/python_style/run_one.sh — 单用例 worker（批次 461 / backlog #23 的并行化半步）。
#
# 用法：run_one.sh <t*.z 路径> <该用例的私有目录>
# 产出：
#   <dir>/verdict   — 判定输出（可多行；由 run.sh 按用例名排序后原样打印）
#   <dir>/<name>.cc — 编译 stderr 原样（供 run.sh 的 compile-diagnostics 聚合）
#   <dir>/<name>.out / <dir>/<name>.stderr — 运行期 stdout / stderr（后台跑落文件，
#                    见下面 run_capped 的说明；不进 compile-diagnostics）
#
# 判定逻辑与并行化前的 run.sh 循环体逐字等价：批次 304 的按值判定，
# expect-error / expect-abort / expect-no-compile / args / env / known-fail
# 全部保留。run.sh 只负责派发、排序聚合与总数——一个用例一个目录，互不共享
# 任何文件，因此并行安全。
# 批次 10030 起的偏离：跑被测程序改成"后台起＋输出落文件＋轮询"（见 run_capped），
# 新增判定行 STUCK（expect-abort 用例停在不可中断态、退出码取不到时）。
# 其余判定口径不变。
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

verdict() { # $1 = ok|bad|stuck, $2 = detail(可空)
    if [ "$known" != "0" ]; then
        if [ "$1" = ok ]; then
            echo "XPASS      $name (known-fail 已达成预期，可摘除标记)" >> "$V"
        else
            rsn=$(grep '^// known-fail:' "$f" | head -1 | sed 's|^// known-fail: ||')
            echo "KNOWN-FAIL $name (${rsn}${2:+; $2})" >> "$V"
        fi
    elif [ "$1" = ok ]; then
        echo "PASS       $name" >> "$V"
    elif [ "$1" = stuck ]; then
        echo "STUCK      $name${2:+ ($2)}" >> "$V"
    else
        echo "FAIL       $name${2:+ ($2)}" >> "$V"
    fi
}

# 跑被测程序：后台起＋输出落文件＋轮询，不依赖"能等到退出码"。
# 原因（#20006，2026-10-04 全量跑第②步卡 34 分钟实证）：调用 abort 的二进制在本机
# 可能停在不可中断态（ps 状态 UE），kill -9 收不掉，timeout 发完信号也要等它收尾，
# 于是同步等待的 worker 一起挂住，整池不再推进。这里把"等退出码"变成可选：
# 进程真退出了才取退出码；确认它停在不可中断态、或到了上限，就不再等。
# 写回全局：RUN_RC（取不到为 -1）、RUN_OUT、RUN_ERR、RUN_STUCK（0/1）。
run_capped() { # $1 = 上限秒, $2 = stdout 落点, $3 = stderr 落点, 其余 = 命令
    local cap=$1 outfile=$2 errfile=$3
    shift 3
    : > "$outfile"; : > "$errfile"
    "$@" >"$outfile" 2>"$errfile" &
    # 轮询间隔 1 秒：每轮要起一颗 `ps`，0.2 秒一档＝每 worker 每秒 5 次 fork，满负载的池里
    # 这份开销由全部 451 枚用例分摊（批次 10030 两遍全量各出现 1–2 枚"输出为空"的假红，
    # 同机还有另一条车道在跑自己的套件，未定量到这一层，先把能省的轮询省掉）。
    # 改成 1 秒后三枚靶夹具单用例实测 2s／4s／3s（含编译），判定语义与归类不变。
    local pid=$! polls=0 max=$cap stat=""
    while :; do
        stat=$(ps -o stat= -p "$pid" 2>/dev/null | tr -d ' ')
        [ -z "$stat" ] && break        # 已退出（下一条 wait 能取到退出码）
        case "$stat" in
            *U*) break ;;             # 不可中断：收不掉，别再等
            Z*)  break ;;             # 已退出待回收
        esac
        if [ "$polls" -ge "$max" ]; then
            kill -TERM "$pid" 2>/dev/null
            break
        fi
        sleep 1; polls=$((polls + 1))
    done
    RUN_OUT=$(cat "$outfile")
    RUN_ERR=$(cat "$errfile")
    case "$stat" in
        *U*) RUN_STUCK=1; RUN_RC=-1 ;;
        *)   RUN_STUCK=0
             if wait "$pid" 2>/dev/null; then RUN_RC=0; else RUN_RC=$?; fi ;;
    esac
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
    if [ ${#envs[@]} -gt 0 ]; then
        run_capped 20 "$wd/$name.out" "$wd/$name.stderr" env "${envs[@]}" "$wd/$name"
    else
        run_capped 20 "$wd/$name.out" "$wd/$name.stderr" "$wd/$name"
    fi
    err="$RUN_ERR"
    if [ "$RUN_STUCK" = 1 ]; then
        # 停在不可中断态＝退出码取不到。stderr 命中期望串说明响亮 abort 已发生，
        # 只是收尾收不掉（#20006）；不冒充 PASS，单独记 STUCK 交 run.sh 计数。
        if printf '%s' "$err" | grep -qF "$want_abort"; then
            verdict stuck "进程停在不可中断态，退出码取不到；stderr 已命中期望串（#20006）"
        else
            verdict bad "进程停在不可中断态且 stderr 未含: $want_abort"
            echo "  stderr: $(printf '%s' "$err" | tr '\n' ' ' | head -c 200)" >> "$V"
        fi
    elif [ "$RUN_RC" -eq 0 ]; then
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
# 上限 20 秒：挂死的程序不许拖住整个套件（曾有一次 for…continue 挂了 30 分钟）。
if [ ${#envs[@]} -gt 0 ] && [ ${#args[@]} -gt 0 ]; then
    run_capped 20 "$wd/$name.out" "$wd/$name.stderr" env "${envs[@]}" "$wd/$name" "${args[@]}"
elif [ ${#envs[@]} -gt 0 ]; then
    run_capped 20 "$wd/$name.out" "$wd/$name.stderr" env "${envs[@]}" "$wd/$name"
elif [ ${#args[@]} -gt 0 ]; then
    run_capped 20 "$wd/$name.out" "$wd/$name.stderr" "$wd/$name" "${args[@]}"
else
    run_capped 20 "$wd/$name.out" "$wd/$name.stderr" "$wd/$name"
fi
actual="$RUN_OUT"

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
