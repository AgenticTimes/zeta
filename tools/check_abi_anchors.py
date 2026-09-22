#!/usr/bin/env python3
"""tools/check_abi_anchors.py —— docs/ABI.md 锚点自动核对（任务 #35 / G.5d）

为什么需要它：ABI.md 的每条规则都以 `file:line` 为证据（"没有锚点就不算合同"）。
批次 319 往 codegen.rs 插了 74 行，19 个锚点当场全部指偏——靠人肉重映射 + 抽查
才没留下假证据。**下一次插行的批次未必会记得**，而漂移后的锚点比没有锚点更坏：
它会读起来像已核对过。

判据分三层，逐层严格：
  1) **可定位**：路径能唯一解析到仓内文件（裸名按 `git ls-files` 后缀匹配；
     多个候选时用车轮判据——只有"行数 ≥ 锚点行号"的候选才算数；仍剩 >1 ⇒
     报"歧义"，要求文档改成可唯一判定的路径）。
  2) **未越界**：目标文件行数 ≥ 锚点行号。
  3) **未漂移**：锚点所指那一行（区间则整段）的文本与基线快照逐字一致。
     代码被就地改写也算漂移——合同引用重读一遍是应有成本。

第三层必须有基线才可比。基线入库（同 dc_audit.sh 的口径）：
  ./tools/check_abi_anchors.py --bless     # 采集/刷新 tools/baselines/abi_anchors.tsv
  ./tools/check_abi_anchors.py             # 核对；漂移 rc=1，歧义/越界 rc=2

已知边界（不是缺陷，别当"已覆盖"）：本脚本只回答"锚点还指着当初那行吗"，
不回答"那行是否仍然是该规则的实现点"——符号被整段搬走且原位置留了同样文本的
情况测不出来，仍需人工抽查。
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_DOC = ROOT / "docs" / "ABI.md"
DEFAULT_BASE = ROOT / "tools" / "baselines" / "abi_anchors.tsv"

# 锚点形态：路径（可含目录，允许 .inc.c 这类双后缀）+ 行号或行号区间。
ANCHOR_RE = re.compile(
    r"(?<![\w./-])(?P<path>(?:[\w.-]+/)*[\w.-]+\.(?:rs|c|h|py|sh|txt|toml))"
    r":(?P<a>\d+)(?:-(?P<b>\d+))?"
)
# 续写形态：`resolver.rs:1887、:2639` 里的 `:2639` —— 只认**同一行内**最近一个
# 带路径锚点的文件。不跨行继承（否则 IR 转储里的 `:1068` 会被绑到上一段的真路径上）。
CONT_RE = re.compile(r"(?<=[、，,\s`]):(?P<c>\d+)(?:-(?P<d>\d+))?(?![\w./-])")
COMBINED = re.compile(ANCHOR_RE.pattern + "|" + CONT_RE.pattern)
# 文档里的 IR/汇编转储行也会带 `foo.c:` 形态的字符串，但不是本仓锚点。
SKIP_PREFIXES = ("/tmp/", "http:", "https:")


def tracked_files() -> list[str]:
    out = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout
    paths = out.splitlines()
    # 生成物（runtime/*.inc.c、src/backend/codegen/runtime_decls_*.rs 等）未入库，
    # 但确实是锚点目标，所以补一次工作树扫描。排除构建目录，避免把 target/ 拉进来。
    for extra in ("runtime", "pylib", "tools", "src", "tests", "docs"):
        base = ROOT / extra
        if not base.is_dir():
            continue
        for p in base.rglob("*"):
            if p.is_file():
                rel = str(p.relative_to(ROOT))
                if rel not in paths:
                    paths.append(rel)
    return paths


class Index:
    def __init__(self, paths: list[str]) -> None:
        self.by_suffix: dict[str, list[str]] = defaultdict(list)
        for rel in paths:
            parts = rel.split("/")
            for i in range(len(parts)):
                self.by_suffix["/".join(parts[i:])].append(rel)
        self._lines: dict[str, list[str]] = {}

    def lines(self, rel: str) -> list[str]:
        if rel not in self._lines:
            try:
                self._lines[rel] = (ROOT / rel).read_text(
                    encoding="utf-8", errors="replace"
                ).splitlines()
            except OSError:
                self._lines[rel] = []
        return self._lines[rel]

    def resolve(self, cited: str, line: int) -> tuple[str | None, list[str]]:
        """返回 (唯一路径 | None, 候选列表)。"""
        key = cited.lstrip("./")
        candidates = self.by_suffix.get(key, [])
        if not candidates:
            candidates = self.by_suffix.get(key.split("/")[-1], [])
        fits = [c for c in candidates if len(self.lines(c)) >= line]
        if len(fits) == 1:
            return fits[0], candidates
        if len(fits) > 1:
            return None, fits
        return None, candidates


def normalize(text: str) -> str:
    return " ".join(text.split())[:160]


def collect(doc: Path, idx: Index) -> tuple[list[tuple[tuple[str, int], str]], list[str]]:
    snapshot: dict[tuple[str, int], str] = {}
    problems: list[str] = []
    doc_lines = doc.read_text(encoding="utf-8", errors="replace").splitlines()
    for docno, line in enumerate(doc_lines, 1):
        last_cited: str | None = None  # 续写锚点继承**本行**最近的路径，逐行重置
        for m in COMBINED.finditer(line):
            cited = m.group("path")
            if cited is None:
                if last_cited is None:
                    continue
                cited, a, b = last_cited, m.group("c"), m.group("d")
            else:
                a, b = m.group("a"), m.group("b")
            start = int(a)
            end = int(b) if b else start
            if cited.startswith(SKIP_PREFIXES):
                continue
            rel, cands = idx.resolve(cited, end)
            if rel is None:
                kind = "歧义（多解）" if cands else "无解"
                where = ", ".join(cands[:4]) if cands else cited
                problems.append(
                    f"{doc.name}:{docno} `{cited}:{start}` → {kind}: {where}"
                )
                continue
            last_cited = cited
            body = idx.lines(rel)
            snippet = normalize(" ".join(body[start - 1 : end]))
            if not snippet:
                problems.append(f"{doc.name}:{docno} `{rel}:{start}` → 该行内容为空")
                continue
            snapshot[(rel, start)] = snippet
    return sorted(snapshot.items()), problems


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--doc", default=str(DEFAULT_DOC))
    ap.add_argument("--baseline", default=str(DEFAULT_BASE))
    ap.add_argument("--bless", action="store_true", help="采集/刷新基线快照")
    args = ap.parse_args()

    doc = Path(args.doc)
    base = Path(args.baseline)
    idx = Index(tracked_files())
    snap, problems = collect(doc, idx)

    for p in problems:
        print(f"  [定位失败] {p}")

    if args.bless:
        base.parent.mkdir(parents=True, exist_ok=True)
        with base.open("w", encoding="utf-8") as f:
            for (rel, line), text in snap:
                f.write(f"{rel}\t{line}\t{text}\n")
        print(f"基线已写入 {base}（{len(snap)} 个锚点）")
        return 2 if problems else 0

    print(f"锚点：{len(snap)} 个可解析 / {len(problems)} 个定位失败（{doc}）")
    if not base.is_file():
        print(f"[E1001] 无基线 {base}，先跑 --bless")
        return 2 if problems else 1

    old = {}
    for raw in base.read_text(encoding="utf-8").splitlines():
        parts = raw.split("\t", 2)
        if len(parts) == 3:
            old[(parts[0], int(parts[1]))] = parts[2]
    new = dict(snap)

    drifted = [k for k in new if k in old and new[k] != old[k]]
    added = [k for k in new if k not in old]
    removed = [k for k in old if k not in new]
    for rel, line in sorted(drifted):
        print(f"  [漂移] {rel}:{line}")
        print(f"     基线: {old[(rel, line)]}")
        print(f"     现在: {new[(rel, line)]}")
    for rel, line in sorted(added):
        print(f"  [新锚点] {rel}:{line} — {new[(rel, line)]}")
    for rel, line in sorted(removed):
        print(f"  [消失] {rel}:{line} — 文档已不再引用，或行号已变")

    print(
        f"漂移 {len(drifted)} / 新 {len(added)} / 消失 {len(removed)}"
        f"（基线 {len(old)} 条，按 文件+行号 比对）"
    )
    if problems:
        return 2
    if drifted or added or removed:
        return 1
    print("锚点全部对上")
    return 0


if __name__ == "__main__":
    sys.exit(main())
