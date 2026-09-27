#!/bin/bash
# 批次 459 判据台（任务 #167②）：锚点快照的键必须是 (文件, 起始行, 终点行)。
#
# 旧版键只有起始行 ⇒ 同一行号被文档引成两种长度时，两条引用塌进同一条槽位、
# 后写的顶掉先写的。后果是四类读数失真（下面 E1–E5 各拦一类）：
#   E1 一份两条引用的文档只配出一行基线；
#   E2 把其中一条引用整条删掉，核对器打印"锚点全部对上"且 rc=0；
#   E3 把多行区间缩短一行（内容跟着变），核对器看不见"换了区间"；
#   E4 --bless-only 无法点名多行锚点（带区间的写法被 E1003 拒收）；
#   E5 行号整体位移后 --rebind 拒改那一条（"同一键有不同长度"），搬家关不掉。
#
# 用法：bash tools/abi_anchors_check.sh [被检的 check_abi_anchors.py 路径]
# 退出码：0=五条全过；1=有失败。E5 需要一个 detached worktree，脚本自己建自己删。
# 仓库文件一个字都不改（E1–E4 用合成文档 + 真实只读源，E5 在 worktree 副本里改）。
set -u
cd "$(dirname "$0")/.." || exit 2
REPO=$PWD
CHK="${1:-tools/check_abi_anchors.py}"
W=$(mktemp -d)
SRC=runtime/py_additions.c
fails=0

ok()   { printf '  [过] %s\n' "$1"; }
bad()  { printf '  [败] %s\n' "$1"; fails=$((fails + 1)); }

# 选锚点：不写死行号 —— 源文件一改，写死的行号会让台自己变红（假失败）。
# 条件：起点不落在文件头 20 行内（E5 会在第 1 行插占位行，太靠前的锚点会被占位行顶掉）、
# 起点的上一行与后两行都非空（位移后落在有内容的行上才报"漂移"而非"定位失败"）、
# 单行内容全文件唯一（rebind 的"唯一命中"判据成立）、单行与四行内容不同。
pick_anchor() {
python3 - "$1" <<'PY'
import sys, pathlib
body = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()
norm = lambda lines: " ".join(" ".join(lines).split())[:160]
lines = [norm([b]) for b in body]
for p in range(20, len(body) - 4):
    if not all(lines[q] for q in (p - 1, p, p + 1, p + 2)):
        continue
    if lines.count(lines[p]) != 1:            # rebind 的"全文件唯一命中"判据
        continue
    if lines[p] == norm(body[p:p + 4]):
        continue
    print(p + 1)                              # 1 起
    break
PY
}
S=$(pick_anchor "$SRC")
[ -n "$S" ] || { echo "选不出锚点，台无法运行"; exit 2; }
echo "锚点起点选定 $S（台自算：该行及其后两行非空、且单行内容全文件唯一），三行区间 $S-$((S + 2))"
E=$((S + 2))

printf '# 台：同一行号被引成两种区间\n- `%s:%s` = 声明 A（单行）\n- `%s:%s-%s` = 声明 B（三行）\n' \
  "$SRC" "$S" "$SRC" "$S" "$E" > "$W/doc_ab.md"
printf '# 台：只留 B\n- `%s:%s-%s` = 声明 B（三行）\n' "$SRC" "$S" "$E" > "$W/doc_b.md"
printf '# 台：B 的区间缩短\n- `%s:%s` = 声明 A（单行）\n- `%s:%s-%s` = 声明 B（两行）\n' \
  "$SRC" "$S" "$SRC" "$S" "$((S + 1))" > "$W/doc_short.md"
base="$W/base.tsv"

echo "=== E1 塌键：两条引用配出几行基线 ==="
python3 "$CHK" --doc "$W/doc_ab.md" --baseline "$base" --bless >"$W/e1.log" 2>&1
n=$(grep -vc '^«待归属»' "$base" 2>/dev/null); n=${n:-0}
if [ "$n" = "2" ]; then ok "基线 $n 行（旧版只写 1 行：A 的内容被 B 顶掉）"; else bad "基线 $n 行，期望 2 —— $(tail -1 "$W/e1.log")"; fi

echo "=== E2 删掉 A 这条引用，核对器要出声 ==="
python3 "$CHK" --doc "$W/doc_b.md" --baseline "$base" >"$W/e2.log" 2>&1; rc=$?
if [ "$rc" != "0" ] && grep -q '\[消失\]' "$W/e2.log"; then
  ok "rc=$rc 且有 [消失]（旧版 rc=0 并打印\"锚点全部对上\"）"
else
  bad "rc=$rc／消失行 $(grep -c '\[消失\]' "$W/e2.log") 条，期望 rc≠0 且有 [消失]"
fi

echo "=== E3 B 的区间从 $S-$E 改成 $S-$((S + 1)) ==="
python3 "$CHK" --doc "$W/doc_short.md" --baseline "$base" >"$W/e3.log" 2>&1; rc=$?
if [ "$rc" != "0" ] && grep -q '\[新锚点\]' "$W/e3.log" && grep -q '\[消失\]' "$W/e3.log"; then
  ok "rc=$rc：换区间被当成新锚点 + 消失各一条（旧版两条塌在一起 ⇒ 看不见）"
else
  bad "rc=$rc，期望 rc≠0 且同时有 [新锚点] 与 [消失]"
fi

echo "=== E4 --bless-only 点名多行锚点 ==="
python3 "$CHK" --doc "$W/doc_ab.md" --baseline "$base" --bless-only "$SRC:$S-$E" >"$W/e4.log" 2>&1; rc=$?
if [ "$rc" = "0" ] && grep -q '\[刷\]' "$W/e4.log"; then
  ok "rc=$rc 且刷到了 $S-$E（旧版只认 \`路径:行号\` ⇒ E1003 拒收）"
else
  bad "rc=$rc，期望 0 —— $(head -2 "$W/e4.log" | tr '\n' ' ')"
fi

echo "=== E5 行号整体位移 +1 后 --rebind 要把两条都改掉 ==="
WT="$W/wt"
SRC_MD5_BEFORE=$(md5 -q "$SRC")
if git worktree add --detach "$WT" HEAD >"$W/wt.log" 2>&1; then
  # E5 必须改一份源文件才能造出"搬家"。只允许改 worktree 副本：锚点、路径、恢复
  # 全部带 $WT 前缀，另外在收尾时核对主树那颗文件逐字未动（本机踩过一次路径漏前缀
  # ⇒ 占位行打进主树，故这条断言写死在台里）。
  S2=$(pick_anchor "$WT/$SRC"); E2=$((S2 + 2))
  D5="$WT/e5doc.md"; B5="$WT/e5base.tsv"; C5="$WT/tools/_abi_under_test.py"
  cp "$REPO/$CHK" "$C5"
  printf -- '- `%s:%s` = 单行锚点 A\n- `%s:%s-%s` = 多行锚点 B（同起不同止＝旧键会塌的那一对）\n' \
    "$SRC" "$S2" "$SRC" "$S2" "$E2" > "$D5"
  python3 "$C5" --doc "$D5" --baseline "$B5" --bless >"$W/e5a.log" 2>&1
  printf '%s\n' '// 判据台 E5 占位行：把整份文件往下推一行' \
    | cat - "$WT/$SRC" > "$WT/$SRC.tmp" && mv "$WT/$SRC.tmp" "$WT/$SRC"
  python3 "$C5" --doc "$D5" --baseline "$B5" >"$W/e5b.log" 2>&1
  d=$(grep -c '\[漂移\]' "$W/e5b.log")
  python3 "$C5" --doc "$D5" --baseline "$B5" --rebind >"$W/e5c.log" 2>&1; rc=$?
  python3 "$C5" --doc "$D5" --baseline "$B5" >"$W/e5d.log" 2>&1; rc2=$?
  n5=$(grep -vc '^«待归属»' "$B5" 2>/dev/null)
  BT='`'   # 反引号在双引号里是命令替换，先取成变量再拼
  if [ "$d" = "2" ] && [ "$rc" = "0" ] && [ "$rc2" = "0" ] \
     && grep -q "$BT$SRC:$((S2 + 1))-$((E2 + 1))$BT" "$D5" && [ "$n5" = "2" ]; then
    ok "位移后报 $d 条漂移、基线 $n5 行、rebind rc=0、文档两个号都改写、复跑 rc=0（旧版只报 1 条且 rebind 拒改 rc=1）"
  else
    bad "漂移 $d 条（期望 2）／基线 $n5 行／rebind rc=$rc／复跑 rc=$rc2 —— $(sed -n '2,4p' "$W/e5c.log" | tr '\n' ' ')"
  fi
  git -C "$WT" checkout -- "$SRC" 2>/dev/null
  git worktree remove --force "$WT" >/dev/null 2>&1 || rm -rf "$WT"
else
  bad "worktree 建不起来：$(tail -2 "$W/wt.log" | tr '\n' ' ')"
fi
[ "$(md5 -q "$SRC")" = "$SRC_MD5_BEFORE" ] \
  && ok "主树 $SRC 逐字未动（E5 只在 worktree 副本里改文件）" \
  || bad "主树 $SRC 被本台改动了 —— 立即 git checkout -- $SRC"

rm -rf "$W"
echo "判据台结论：$([ "$fails" = 0 ] && echo 'E1–E5 全过' || echo "$fails 条失败")"
[ "$fails" = 0 ]
