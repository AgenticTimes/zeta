#!/usr/bin/env bash
# tools/knob_probe.sh —— 布尔旋钮的"值域"断言 + ZETA_PARSE_RECOVER 的全量影响读数
#
# 两件事分开，因为判定强度不同：
#   A) 断言：`ZETA_*=0` 必须是**关**。批次 336 实测，27 处旋钮读法全是
#      `std::env::var(..).is_ok()` ⇒ "存在即开" ⇒ 写 `0` 的人得到的是"开"：
#        ZETA_PARSE_RECOVER=0 → 打出 W1003（恢复被启用）
#        ZETA_STRICT_PARSE=0  → E1002 致命（rc=1）
#        ZETA_STRICT_ABI=0    → ABI 强转告警升级为失败（rc=1）
#      现在都收进 `zetac::diagnostics::env_flag`，本段就是它的回归网。
#      两处**故意**留在 is_ok：src/diagnostics.rs 的 NO_COLOR（外部约定就是"存在即生效"）、
#      src/std/env/mod.rs 的 `env::var(name).is_ok()`（那是**被编译语言**的"变量是否存在" API，
#      不是编译器旋钮）——A4 点名它们，防止"全仓已收敛"被误读成"这三处也该改"。
#   B) 读数：解析恢复（`ZETA_PARSE_RECOVER=1`，top_level.rs 的 skip_to_top_level_sync）
#      对 official 194 文件的退出码影响。**只报告，不判定**——它默认关着，
#      这段是"若要默认打开，先还掉多少账"的凭证（批次 336 的凭证在 roadmap）。
#
# 用法：
#   ./tools/knob_probe.sh          # A 断言 + B 全量（约 2-4 分钟）
#   ./tools/knob_probe.sh --assert-only
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
[[ -x "$ZETAC" ]] || { echo "building zetac..." >&2; cargo build --release -p zetac -q; }
unset ZETA_STRICT_PARSE ZETA_STRICT_ABI ZETA_PARSE_RECOVER

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
rc=0

# ---- A) 值域断言 ----------------------------------------------------------
# 夹具：体内裸 `mut counter: i64 = 0` 解析不了 ⇒ 关态丢尾(W1002)、开态跳过(W1003)。
# 批次 384 之前这里放的是 `static mut counter: i64 = 0`；那条现在能解析了（parse_static
# + 提升到模块级），留在原地会让 A1/A2 十四断言全部空转。换成它的邻居拼法——同一个
# 位置、同样整项解析失败，且正好锁住"批 384 只接住 `static`，没有顺手放宽裸 `mut`"。
cat > "$TMP/recover.z" <<'EOF'
fn f() -> i64 {
    mut counter: i64 = 0
    return counter
}
print(f())
EOF
# 夹具：`.sqrt()` 的实参是 f64、形参落定成 i64 ⇒ 真的走 fptosi ⇒ coerce 告警 + 汇总行。
cat > "$TMP/abi.z" <<'EOF'
fn f() -> i64 {
    let x: f64 = 4.0
    let y = x.sqrt() as u64
    return y as i64
}
print(f())
EOF

want() { # $1=描述 $2=期望 $3=实际
  if [[ "$2" == "$3" ]]; then echo "  ok   $1 → $3"; else echo "  FAIL $1 → 期望 ${2}，实得 $3"; rc=1; fi
}
# sig <env赋值|''> <文件> → 该程序在这组旋钮下的诊断文本
sig() {
  local kv=${1:-} f=$2
  ( if [[ -n "$kv" ]]; then export "$kv"; fi
    "$ZETAC" "$f" -o "$TMP/o.o" 2>&1 | grep -v 'clang:' )
}
# rc_of <env赋值|''> <文件> → zetac 退出码。**不许经管道**：管道取到的是 grep 的 rc，
# 会把"rc=1 的致命"和"什么都没输出"混成一类（首版就栽过）。
rc_of() {
  local kv=${1:-} f=$2
  ( if [[ -n "$kv" ]]; then export "$kv"; fi
    "$ZETAC" "$f" -o "$TMP/o.o" >/dev/null 2>&1 )
  echo $?
}

echo "== A1 ZETA_PARSE_RECOVER（关=W1002 丢尾，开=W1003 跳过）"
# 假的**整个值域**都要测，不是测一个特例形状就算闭合：off-list 的每一档 + 大小写 + 首尾空白。
for kv in 'ZETA_PARSE_RECOVER=0' 'ZETA_PARSE_RECOVER=off' 'ZETA_PARSE_RECOVER=OFF' \
          'ZETA_PARSE_RECOVER= false ' 'ZETA_PARSE_RECOVER=no' 'ZETA_PARSE_RECOVER=NO' \
          'ZETA_PARSE_RECOVER=' 'ZETA_PARSE_RECOVER=  '; do
  want "$kv 关" W1002 "$(sig "$kv" "$TMP/recover.z" | grep -o 'W100[23]' | sort -u | tr '\n' ',' | sed 's/,$//')"
done
want "未设置 关" W1002 "$(sig '' "$TMP/recover.z" | grep -o 'W100[23]' | sort -u | tr '\n' ',' | sed 's/,$//')"
# 真值侧同样不是一个点：`1` 与"其余任意非空值"都必须开（合同是"除假值拼写外都算开"）。
for kv in 'ZETA_PARSE_RECOVER=1' 'ZETA_PARSE_RECOVER=true' 'ZETA_PARSE_RECOVER=on' 'ZETA_PARSE_RECOVER=2'; do
  want "$kv 开" W1003 "$(sig "$kv" "$TMP/recover.z" | grep -o 'W100[23]' | sort -u | tr '\n' ',' | sed 's/,$//')"
done

echo "== A2 ZETA_STRICT_PARSE（=0 不致命 / =1 致命）"
want "=0 rc" 0 "$(rc_of ZETA_STRICT_PARSE=0 "$TMP/recover.z")"
want "=1 rc" 1 "$(rc_of ZETA_STRICT_PARSE=1 "$TMP/recover.z")"
want "未设置 rc" 0 "$(rc_of '' "$TMP/recover.z")"

echo "== A3 ZETA_STRICT_ABI（=0 不拦 / =1 拦）"
if ! sig '' "$TMP/abi.z" | grep -q "non-allowlisted cast"; then
  want "夹具产出 coerce 告警" "yes" "no（判据无法测，别当已覆盖）"
else
  want "coerce 夹具成立" yes yes
  want "=0 rc" 0 "$(rc_of ZETA_STRICT_ABI=0 "$TMP/abi.z")"
  want "=1 rc" 1 "$(rc_of ZETA_STRICT_ABI=1 "$TMP/abi.z")"
  want "未设置 rc" 0 "$(rc_of '' "$TMP/abi.z")"
fi

echo "== A4 仍留 is_ok 的三处（点名，防止把'旋钮侧收敛'读成'全仓再无 is_ok'）"
want 'diagnostics.rs NO_COLOR 仍按存在判定' 1 "$(grep -c 'env::var("NO_COLOR").is_ok()' src/diagnostics.rs)"
want 'std/env/mod.rs 变量存在性 API 保留' 1 "$(grep -c 'env::var(name.as_ref()).is_ok()' src/std/env/mod.rs)"
want 'ZETA_* 旋钮侧残留 is_ok 站点' 0 \
  "$(grep -rn 'env::var("ZETA_[A-Z_]*")\.is_ok()' src/ | wc -l | tr -d ' ')"

if [[ "${1:-}" == --assert-only ]]; then
  echo "knob_probe: A 段 rc=${rc}（B 段未跑）"; exit $rc
fi

# ---- B) 恢复态对 official 的退出码影响 ------------------------------------
echo "== B official 194 文件：ZETA_PARSE_RECOVER 关/开 的退出码对照"
for f in tests/unit-tests/*.z; do
  b=$(basename "$f" .z)
  env -u ZETA_PARSE_RECOVER -u ZETA_STRICT_PARSE -u ZETA_STRICT_ABI \
    "$ZETAC" "$f" -o "$TMP/$b.off.o" >/dev/null 2>&1; echo "$b $?"
done > "$TMP/off.txt"
for f in tests/unit-tests/*.z; do
  b=$(basename "$f" .z)
  ZETA_PARSE_RECOVER=1 "$ZETAC" "$f" -o "$TMP/$b.on.o" >/dev/null 2>&1; echo "$b $?"
done > "$TMP/on.txt"
join "$TMP/off.txt" "$TMP/on.txt" > "$TMP/j.txt"
echo "  文件数: $(wc -l < "$TMP/off.txt" | tr -d ' ')"
echo "  关态 rc 分布: $(awk '{print $2}' "$TMP/off.txt" | sort | uniq -c | tr '\n' ' ')"
echo "  开态 rc 分布: $(awk '{print $2}' "$TMP/on.txt"  | sort | uniq -c | tr '\n' ' ')"
echo "  0→非0（恢复引入的失败，默认打开前必须逐个消化）: $(awk '$2==0&&$3!=0' "$TMP/j.txt" | wc -l | tr -d ' ')"
awk '$2==0&&$3!=0 {print "    " $1}' "$TMP/j.txt"
echo "  非0→0（恢复修好的）: $(awk '$2!=0&&$3==0' "$TMP/j.txt" | wc -l | tr -d ' ')"
echo "knob_probe: A 段 rc=${rc}（B 段只读数，不参与判定）"
exit $rc
