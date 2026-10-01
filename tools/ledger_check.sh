#!/usr/bin/env bash
# tools/ledger_check.sh — 批次 759（#196）：合并协议的台账保全硬核对。
#
# 背景（451 收尾合并 `2feff1de` 实拍）：旁路纯文档提交 `89a5df8a` 从陈旧快照
# 整文件重写 worktree.md，把 §4 主线台账 449/449b/450 三行整段删除——当时是
# 恰好 451 行插在同一位置才让 git 报冲突"被撞上"的；无冲突时这些行会一声
# 不出地消失，而台账是两班互读状态的唯一面。
#
# 用法：
#   tools/ledger_check.sh [PRE_REF] [POST_REF]
#     PRE_REF  合并前的引用（默认 HEAD＝合并提交前的bootstrap 顶；合并中可用 HEAD）
#     POST_REF 合并后的引用（默认工作树的 worktree.md；传 ref 则取该树版本）
#   核对：PRE 侧 worktree.md 里每一行 `| <批次号> | <车道> |`（bootstrap 与
#   cleanup 两边都查），逐号在 POST 侧 grep 必须仍存在；缺一行即 rc=1，
#   修法一律"两侧都留"（§3 协议）。
#
# 阳性对照（本批实测）：
#   tools/ledger_check.sh 89a5df8a^ 89a5df8a   ⇒ 红（449/449b/450 缺失）
#   tools/ledger_check.sh ae372218 cd35c276    ⇒ 绿（757 合并无删行）
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PRE=${1:-HEAD}
POST=${2:-}

pre_file=$(git -C "$ROOT" show "$PRE:worktree.md" 2>/dev/null) || {
    echo "ledger_check: 取不到 $PRE:worktree.md"; exit 2; }

if [ -n "$POST" ]; then
    post_file=$(git -C "$ROOT" show "$POST:worktree.md" 2>/dev/null) || {
        echo "ledger_check: 取不到 $POST:worktree.md"; exit 2; }
else
    post_file=$(cat "$ROOT/worktree.md")
fi

# 收集 PRE 侧台账行（§4 bootstrap 与 §5 cleanup 都算）：行首 `| <号> | <车道> |`
# bash 3.2（macOS 自带）没有 mapfile，用 herestring 循环，计数器不被子壳吞。
row_list=$(printf '%s\n' "$pre_file" | grep -oE '^\| [0-9]+[A-Za-z]* \| (bootstrap|cleanup) \|' | sort -u)

total=0; missing=0
while IFS= read -r row; do
    [ -z "$row" ] && continue
    total=$((total + 1))
    # 不用 -q：-q 首中即退会触发 SIGPIPE，pipefail 把整条管道判非零
    # （rc=141 实拍）→ 每行都误报缺失。读全量就没有这个坑。
    if ! printf '%s\n' "$post_file" | grep -F "$row" >/dev/null; then
        missing=$((missing + 1))
        echo "ledger_check: 台账行在合并后缺失 → $row"
    fi
done <<< "$row_list"

echo "ledger_check: PRE=$PRE rows=$total missing=$missing"
[ "$missing" -eq 0 ]
