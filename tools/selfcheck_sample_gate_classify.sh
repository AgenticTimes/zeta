#!/bin/bash
# 批次 10030 附检：sample_gate.sh ② 步的判定归类，用文件里的真实代码跑合成 verdict。
# 归类链从被测脚本里 sed 取原文（取不到＝脚本结构被改坏，本检直接报错），
# 防止"检查脚本和被检脚本走散"。
set -u
ROOT=${PWD}
SG="$ROOT/tools/sample_gate.sh"
chain=$(sed -n '/^  if \[ ! -s "\$d\/verdict" \]; then$/,/^  fi$/p' "$SG")
n=$(printf '%s\n' "$chain" | wc -l | tr -d ' ')
[ "$n" -ge 12 ] || { echo "归类链取到 $n 行＝结构变了，本检作废"; exit 1; }

cls() { # $1 = verdict 文件内容（""＝文件不存在）
  local t=$1
  ps_ok=0; ps_bad=""; ps_missing=""; ps_known_n=0; ps_known=""; ps_xpass=""; ps_stuck=""; ps_stuck_n=0
  if [ -z "$t" ]; then
    d=/tmp/sg_align/no_such_dir
  else
    d=/tmp/sg_align/v
    mkdir -p "$d"; printf '%s\n' "$t" > "$d/verdict"
  fi
  b=case_x
  eval "$chain"
  if   [ -n "$ps_missing" ]; then echo missing
  elif [ "$ps_ok" = 1 ];        then echo ok
  elif [ "$ps_known_n" = 1 ];   then echo known-fail
  elif [ -n "$ps_xpass" ];      then echo xpass
  elif [ -n "$ps_stuck" ];      then echo stuck
  elif [ -n "$ps_bad" ];        then echo bad
  else echo none; fi
}

ok=0; bad=0
chk() { # $1 = 期望桶, $2 = verdict 内容（-＝文件缺失）
  got=$(cls "$2")
  if [ "$got" = "$1" ]; then ok=$((ok + 1)); echo "ok    期望 $1 实得 $got"
  else bad=$((bad + 1)); echo "FAILED 期望 $1 实得 $got（内容: ${2:0:40}）"; fi
}
mkdir -p /tmp/sg_align
chk missing ""
chk ok "PASS       t001"
chk known-fail "KNOWN-FAIL t001 (已知缺口)"
chk xpass "XPASS      t001 (known-fail 已达成预期，可摘除标记)"
chk stuck "STUCK      t001 (进程停在不可中断态，退出码取不到；stderr 已命中期望串（#20006）)"
chk bad "FAIL       t001 (值不符)"
chk bad "SOMETHING-ELSE t001"
[ ! -s /tmp/sg_align/v ] || cls "$(printf 'STUCK  t1 (x)\n')" >/dev/null

# 报告段（② 步末尾的 if/elif/else）：分母扣减与不计红说明，同样从脚本原文取
rpt=$(sed -n '/^if \[ "\$ps_total" -eq 0 \]; then$/,/^fi$/p' "$SG")
m=$(printf '%s\n' "$rpt" | wc -l | tr -d ' ')
[ "$m" -ge 8 ] || { echo "报告段取到 $m 行＝结构变了，本检作废"; exit 1; }
run_rpt() { # $1=stuck数, $2=stuck名单, $3=bad名单
  ps_total=41; ps_ok=39; ps_known_n=0; ps_known=""; ps_xpass=""
  ps_stuck_n=$1; ps_stuck=$2; ps_bad=$3; ps_missing=""; rc_total=0
  eval "$rpt"; echo "rc=$rc_total"
}
r=$(run_rpt 2 " t256 t405" "")
case "$r" in *"39/39 PASS"*) echo "ok    分母扣减：$(printf '%s' "$r" | head -1)" ;;
  *) bad=$((bad + 1)); echo "FAILED 分母应为 39/39，实得：$r" ;; esac
case "$r" in *"另 2 条 STUCK"*) echo "ok    不计红说明带名单"; ;;
  *) bad=$((bad + 1)); echo "FAILED 缺 STUCK 说明：$r" ;; esac
case "$r" in *"rc=0"*) echo "ok    只有 STUCK 不计红（rc_total=0）";;
  *) bad=$((bad + 1)); echo "FAILED 只有 STUCK 时 rc_total 非 0：$r" ;; esac
r2=$(run_rpt 1 " t256" " t999")
# 41 = 39 PASS + 1 STUCK + 1 FAIL ⇒ 分母扣掉 STUCK 应为 39/40
case "$r2" in *"39/40 PASS，非 PASS: t999"*) echo "ok    有真 FAIL 时仍红且分母扣 STUCK：$(printf '%s' "$r2" | head -1)" ;;
  *) bad=$((bad + 1)); echo "FAILED 真 FAIL 臂读数不对：$r2" ;; esac
case "$r2" in *"rc=1"*) echo "ok    真 FAIL 时 rc_total=1";;
  *) bad=$((bad + 1)); echo "FAILED 真 FAIL 时 rc_total 非 1：$r2" ;; esac

echo "selfcheck_sample_gate_classify: $ok ok, $bad failed"
# 真实 verdict 侧证：跑窗口 0 的两枚 expect-abort 靶夹具，确认 worker 实际判定行落进本检的桶
for t in t256_pylib_stub_abort t485_dyn_getitem_guard_stays_loud; do
  z="$ROOT/tests/python_style/$t.z"; [ -e "$z" ] || { echo "缺夹具 $t"; continue; }
  dd=/tmp/sg_align/real_$t; mkdir -p "$dd"
  timeout 200 bash "$ROOT/tests/python_style/run_one.sh" "$z" "$dd" >/dev/null 2>&1
  printf 'real  %-40s %s\n' "$t" "$(head -1 "$dd/verdict" 2>/dev/null || echo '(空)')"
done
[ "$bad" -eq 0 ]
