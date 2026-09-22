#!/usr/bin/env python3
"""tools/check_abi_anchors.py —— docs/ABI.md 锚点自动核对（任务 #35 / G.5d）

为什么需要它：ABI.md 的每条规则都以 `file:line` 为证据（"没有锚点就不算合同"）。
批次 319 往 codegen.rs 插了 74 行，19 个锚点当场全部指偏——靠人肉重映射 + 抽查
才没留下假证据。**下一次插行的批次未必会记得**，而漂移后的锚点比没有锚点更坏：
它会读起来像已核对过。

判据分四层，逐层严格：
  1) **可定位**：路径能唯一解析到仓内文件（裸名按 `git ls-files` 后缀匹配；
     多个候选时用车轮判据——只有"行数 ≥ 锚点行号"的候选才算数；仍剩 >1 ⇒
     报"歧义"，要求文档改成可唯一判定的路径）。
  2) **未越界**：目标文件行数 ≥ 锚点行号。
  3) **未漂移**：锚点所指那一行（区间则整段）的文本与基线快照逐字一致。
     代码被就地改写也算漂移——合同引用重读一遍是应有成本。
  4) **可归属**（任务 #52 / 批次 335）：文档里每个 `:NNNN` 形态的行号引用都必须
     绑定到一个路径，绑不上的 counted 并棘轮化——一条都不许静默飘过。

第三层必须有基线才可比。基线入库（同 dc_audit.sh 的口径）：
  ./tools/check_abi_anchors.py --bless     # 采集/刷新 tools/baselines/abi_anchors.tsv
  ./tools/check_abi_anchors.py             # 核对；漂移/待归属增加 rc=1，歧义/越界 rc=2
  ./tools/check_abi_anchors.py --list      # 打印"每个锚点现在指着哪句代码"（归属自查）

归属的两种来源（第 4 层）：
  a) **同行最近路径**——`codegen.rs:6885-7010；:3766、:4317` 里的续写绑到 codegen.rs。
     只在同一行内继承，绝不跨行。跨行"就近归属"实测会把 §3.4 的 gen.rs 行号
     （:8052 `slots[i] = Some(v)`）挂到上一段的 py_additions.c 上——挂错的锚点
     比没有锚点更坏（见文档开头那段），所以宁可要人写一遍路径。
  b) **显式作用域行**——表格/清单里重复挂路径太难读，改由一行 `> 锚点源码：<path>`
     声明"从这里到下一个标题（或下一个声明）之间的 `:NNNN` 都属于这个文件"。
     声明本身要过第 1 层核对；作用域内没有可绑路径的引用不算错。
     引用仓外证据（`/tmp` 下的 IR 转储等）时写 `> 锚点源码：外部转储（不入锚点核对）`，
     意思是"这里核不了，是人显式说的"，与"没人想过"是两件事。

不能这么引用（会进"待归属"，逼着改写）：
  - 裸区间 `6968-6982`（同行有路径也不猜：它可能是"批次 333-334"、年份、计数）；
  - ASCII 逗号列表 `:2550,2560`（与千分位 `1,719` 同形，判不出来）；
  两者都要写成 `path:2550、:2560` 这种带路径/带冒号的显式形态。

比"待归属"更坏的是**隐形引用**：形状不在任何一条正则里，既不核对也不计数。
批次 335 实测到三种，全部靠人工改写收口（脚本不猜，因为每种都有同名假阳性）：
  - `stub:339`——冒号前是单词字符，CONT 的负向前瞻（冒号前不许是单词字符或斜杠）拒掉
    ⇒ 写全 `runtime/xxx_stub.c:339`；
  - `codegen.rs:1069、1071、1078`——后两个没有冒号 ⇒ 每个数都得带 `:`；
  - `gen.rs:8797/:8647/:8666`——冒号前是斜杠，同一个负向前瞻拒掉 ⇒ 改用 `、` 分隔。
另一条同源限制：解析器只认仓库索引内的路径。根目录的活文档（`refactor.md`）
`git ls-files` 里没有 ⇒ 第 1 层就"无解"，要引用就写成"第 N 行 + 注明未入库"，
别指望它进基线。

已知边界（不是缺陷，别当"已覆盖"）：本脚本只回答"锚点还指着当初那行吗"，
不回答"那行是否仍然是该规则的实现点"——符号被整段搬走且原位置留了同样文本的
情况测不出来，仍需人工抽查。另一条：作用域声明只保证"路径解析得开"，不保证
"作者没把两个文件的行号混进同一段"——插完标记要用 `--list` 逐条读一遍目标行。
批次 335 在实现本层之前先按"跨行就近归属"试算过一张对照表（文档行 / 就近路径 /
裸引用），正是那张表暴露了 §3.4 的 :8052 会挂到 py_additions.c、§3.3 的
:4283 会挂到 py_additions.c，才把设计从"猜"改成"人声明"。
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_DOC = ROOT / "docs" / "ABI.md"
DEFAULT_BASE = ROOT / "tools" / "baselines" / "abi_anchors.tsv"

# 锚点形态：路径（可含目录，允许 .inc.c 这类双后缀）+ 行号或行号区间。
# 扩展名含 md/z：`validate.md:149`、`tests/.../t27_builtins_fmt.z:18` 都是仓内、
# 会漂移、且此前**完全核不到**的引用。（活文档例外：refactor.md 的行号会被
# 用户自己的编辑改掉，那条已在本批改写成按章节引用——锚点不指向移动中的文档。）
ANCHOR_RE = re.compile(
    r"(?<![\w./-])(?P<path>(?:[\w.-]+/)*[\w.-]+\.(?:rs|c|h|py|sh|txt|toml|md|z))"
    r":(?P<a>\d+)(?:-(?P<b>\d+))?"
)
# 续写形态：`resolver.rs:1887、:2639` 里的 `:2639` —— 只认**同一行内**最近一个
# 带路径锚点的文件，或本行所处的显式作用域（见 SCOPE_RE）。
# 批次 335 放宽了三处"人已经这么写了但工具看不见"的形态：全角括号/冒号紧跟
# （`（:6630-6690）`、`调用点：:3766`）、区间右端点带冒号（`:4-:17`）、
# 以及斜杠分隔的并列（`:1450/:1458`）。
# 反向排除 `(?<![\w/])`：`ci.yml:61`、`http://h:8080` 里的冒号属于"前一个 token
# 是名字"，那是路径没被 ANCHOR_RE 认出来（或压根不是路径），不能当续写挂到别处。
# 末尾只要求后面不是数字（不挡 `/` `，` `` ` ``），否则 `:1450/` 整条匹配失败、又飘回看不见。
CONT_RE = re.compile(r"(?<![\w/]):(?P<c>\d+)(?:-:?(?P<d>\d+))?(?!\d)")
# 显式作用域声明行：`> 锚点源码：codegen.rs` / `> 锚点源码：外部转储（不入锚点核对）`
SCOPE_RE = re.compile(r"^\s*(?:>\s*)?锚点源码[:：]\s*(?P<what>[\w./-]+)(?P<note>[（(].*)?$")
EXTERNAL_WORDS = ("外部", "不入库", "非仓内", "不入锚点")
# 能当"同行最近归属"的扩展名：代码与数据表。md/z 是"就这一个引用"的文档锚点，不参与继承。
SRC_EXT = re.compile(r"\.(rs|c|h|py|sh|txt|toml|inc)$")
# 待归属：裸区间（同行有路径也不绑）。千分位/年份/批次号都是这个形状。
BARE_RANGE_RE = re.compile(r"(?<![\w.:/-])(?P<r1>\d{3,5})-(?P<r2>\d{3,5})(?![\w./-])")
# `批次 333-334`、`#4-7` 这类不是行号；扫之前先抹掉（抹等长空格，保持列号）。
EXEMPT_RES = (
    re.compile(r"批次\s*\d+(?:-\d+)?"),
    re.compile(r"\d{4}-\d{2}-\d{2}"),  # ISO 日期
)
COMBINED = re.compile(ANCHOR_RE.pattern + "|" + CONT_RE.pattern)
# 文档里的 IR/汇编转储行也会带 `foo.c:` 形态的字符串，但不是本仓锚点。
SKIP_PREFIXES = ("/tmp/", "http:", "https:")
PENDING = "«待归属»"
EXTERNAL = "«仓外»"


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


def strip_exempt(line: str) -> str:
    """把"形状像引用但不是"的片段替换成等长空格（保持列号，便于回溯）。"""
    for rx in EXEMPT_RES:
        line = rx.sub(lambda m: " " * (m.end() - m.start()), line)
    return line


Row = tuple[int, str, str, int, int, str]  # 文档行 / 声明的归属 / 解析到的文件 / 起 / 止 / 文本


def collect(
    doc: Path, idx: Index
) -> tuple[
    list[tuple[tuple[str, int], str]],
    list[str],
    "Counter[str]",
    list[Row],
    int,
    list[tuple[int, str, str]],
]:
    snapshot: dict[tuple[str, int], str] = {}
    problems: list[str] = []
    pending: Counter[str] = Counter()
    rows: list[Row] = []
    pending_locs: list[tuple[int, str, str]] = []
    external_n = 0
    scope: str | None = None  # 已解析的显式作用域路径；None=无，EXTERNAL=声明为仓外
    doc_lines = doc.read_text(encoding="utf-8", errors="replace").splitlines()
    for docno, raw in enumerate(doc_lines, 1):
        line = raw
        if not raw.strip():  # 空行结束作用域：声明必须紧挨着它负责的那段
            scope = None
        elif re.match(r"^#{1,6} ", raw):  # 标题同样结束
            scope = None
        m = SCOPE_RE.match(raw)
        if m:
            what = m.group("what")
            if any(w in what for w in EXTERNAL_WORDS):
                scope = EXTERNAL
            else:
                rel, cands = idx.resolve(what, 1)
                if rel is None:
                    problems.append(
                        f"{doc.name}:{docno} 作用域声明 `{what}` → "
                        f"{'歧义' if cands else '无解'}: {', '.join(cands[:4]) or what}"
                    )
                    scope = None
                else:
                    scope = rel
            continue
        line = strip_exempt(raw)

        last_cited: str | None = None  # 续写锚点继承**本行**最近的路径，逐行重置
        for m in COMBINED.finditer(line):
            cited = m.group("path")
            if cited is None:
                a, b = m.group("c"), m.group("d")
                text = m.group(0)
                owner = last_cited or scope
                if owner is None:
                    pending[text] += 1
                    pending_locs.append((docno, text, raw.strip()[:66]))
                    continue
                if owner == EXTERNAL:
                    external_n += 1
                    continue
                cited, start = owner, int(a)
                end = int(b) if b else start
            else:
                a, b = m.group("a"), m.group("b")
                start = int(a)
                end = int(b) if b else start
                if cited.startswith(SKIP_PREFIXES):
                    continue
            rel, cands = idx.resolve(cited, end)
            if rel is None:
                # 三种失败要分开报：路径本身不确定、路径确定但行号超出文件长度、
                # 压根没有这个文件。"歧义"只在该说歧义的时候说。
                if cands and all(len(idx.lines(c)) < end for c in cands):
                    kind = "越界（该文件没有这么多行）"
                elif cands:
                    kind = "歧义（多解）"
                else:
                    kind = "无解"
                where = ", ".join(
                    f"{c}({len(idx.lines(c))} 行)" for c in cands[:4]
                ) or cited
                problems.append(
                    f"{doc.name}:{docno} `{cited}:{start}` → {kind}: {where}"
                )
                continue
            if SRC_EXT.search(rel):
                # 只有代码/数据文件才当"同行最近的归属"。文档锚点（`NOTES.md:25`）
                # 不设置：实测 §3.5 有一行同时含 `NOTES.md:25-28` 和三个 codegen.rs
                # 的 `:6xxx`，让 .md 参与继承会把 `:6116` 挂到一份 61 行的笔记上。
                last_cited = rel
            body = idx.lines(rel)
            snippet = normalize(" ".join(body[start - 1 : end]))
            if not snippet:
                problems.append(f"{doc.name}:{docno} `{rel}:{start}` → 该行内容为空")
                continue
            snapshot[(rel, start)] = snippet
            rows.append((docno, cited, rel, start, end, snippet))
        # 裸区间：同行有路径也不绑（见模块 docstring 第 4 层）
        for m in BARE_RANGE_RE.finditer(line):
            pending[m.group(0)] += 1
            pending_locs.append((docno, m.group(0), raw.strip()[:66]))
    return sorted(snapshot.items()), problems, pending, rows, external_n, pending_locs


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--doc", default=str(DEFAULT_DOC))
    ap.add_argument("--baseline", default=str(DEFAULT_BASE))
    ap.add_argument("--bless", action="store_true", help="采集/刷新基线快照")
    ap.add_argument("--list", action="store_true", help="打印每个锚点当前指向的代码行")
    ap.add_argument("--pending", action="store_true", help="打印待归属引用的文档行号")
    args = ap.parse_args()

    doc = Path(args.doc)
    base = Path(args.baseline)
    idx = Index(tracked_files())
    snap, problems, pending, rows, external_n, pending_locs = collect(doc, idx)

    for p in problems:
        print(f"  [定位失败] {p}")

    if args.pending:
        for docno, text, ctx in pending_locs:
            print(f"{docno:4} {text:14} | {ctx}")
        print(f"待归属 {len(pending_locs)} 处 / {len(pending)} 种")
        return 0

    if args.list:
        for docno, cited, rel, start, end, snippet in rows:
            span = f"{start}-{end}" if end != start else f"{start}"
            print(f"{docno:4} {cited}:{span} → {rel}:{span} | {snippet[:96]}")
        print(f"共 {len(rows)} 条；声明为仓外 {external_n} 条；待归属 {sum(pending.values())} 条")

    if args.bless:
        base.parent.mkdir(parents=True, exist_ok=True)
        with base.open("w", encoding="utf-8") as f:
            for (rel, line), text in snap:
                f.write(f"{rel}\t{line}\t{text}\n")
            for text, n in sorted(pending.items()):
                f.write(f"{PENDING}\t{n}\t{text}\n")
        print(
            f"基线已写入 {base}（{len(snap)} 个锚点 + "
            f"{sum(pending.values())} 条待归属 / {len(pending)} 种）"
        )
        return 2 if problems else 0

    print(f"锚点：{len(snap)} 个可解析 / {len(problems)} 个定位失败（{doc}）")
    if not base.is_file():
        print(f"[E1001] 无基线 {base}，先跑 --bless")
        return 2 if problems else 1

    old: dict[tuple[str, int], str] = {}
    old_pending: Counter[str] = Counter()
    for raw in base.read_text(encoding="utf-8").splitlines():
        parts = raw.split("\t", 2)
        if len(parts) != 3:
            continue
        if parts[0] == PENDING:
            old_pending[parts[2]] = int(parts[1])
        else:
            old[(parts[0], int(parts[1]))] = parts[2]
    new = dict(snap)

    drifted = [k for k in new if k in old and new[k] != old[k]]
    added = [k for k in new if k not in old]
    removed = [k for k in old if k not in new and k[0] != PENDING]
    for rel, line in sorted(drifted):
        print(f"  [漂移] {rel}:{line}")
        print(f"     基线: {old[(rel, line)]}")
        print(f"     现在: {new[(rel, line)]}")
    for rel, line in sorted(added):
        print(f"  [新锚点] {rel}:{line} — {new[(rel, line)]}")
    for rel, line in sorted(removed):
        print(f"  [消失] {rel}:{line} — 文档已不再引用，或行号已变")

    # 待归属棘轮：只许缩不许涨（新增形态 = 又留了一个核不到的引用）
    grew = {t: n for t, n in pending.items() if n > old_pending.get(t, 0)}
    for text, n in sorted(grew.items()):
        print(f"  [待归属+{n - old_pending.get(text, 0)}] {text}"
              f"（基线 {old_pending.get(text, 0)} → 现在 {n}）")
    for text, n in sorted(old_pending.items()):
        if pending.get(text, 0) == 0:
            print(f"  [待归属已消化] {text}（原 {n} 条）")
    print(
        f"漂移 {len(drifted)} / 新 {len(added)} / 消失 {len(removed)}"
        f"（基线 {len(old)} 条，按 文件+行号 比对）"
        f"；待归属 {sum(pending.values())} 条 / {len(pending)} 种"
        f"（基线 {sum(old_pending.values())} 条），声明为仓外 {external_n} 条"
    )
    if problems:
        return 2
    if drifted or added or removed:
        return 1
    if grew:
        return 1
    print("锚点全部对上")
    return 0


if __name__ == "__main__":
    sys.exit(main())
