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
     这一层有两种"对不上"，形状不同、修法也不同：
       · **搬家**＝代码插了行、文档还没跟着改号 ⇒ 基线里的 (文件,行号) 还在文档里，
         只是那一行的内容换了 ⇒ `--rebind` 按"内容逐字相同 + 全文件唯一命中"自动改文档。
       · **改号**＝人已经把文档里的号改对了、基线还是旧号 ⇒ 同一文件里出现一对
         `[消失] :旧` + `[新锚点] :新`，而两者内容逐字相同。此前这两笔各说各话，
         唯一的关闭手段 `--bless` 会**无条件重采全部锚点**（把别处刚漂的、刚被改写的
         一起洗成"对上"）。批次 354 起 `pair_renumber()` 把这种成对形态判出来：
         互为唯一候选才配，报告里点名，`--rebind` 只刷基线、不动文档。
  4) **可归属**（任务 #52 / 批次 335）：文档里每个 `:NNNN` 形态的行号引用都必须
     绑定到一个路径，绑不上的 counted 并棘轮化——一条都不许静默飘过。

第三层必须有基线才可比。基线入库（同 dc_audit.sh 的口径）：
  ./tools/check_abi_anchors.py --bless     # 全量重采基线；在场"漂移/消失"会先拦一次（见下）
  ./tools/check_abi_anchors.py --bless-only 路径:行号[,…]  # 只刷点名的几条，其余逐字不动
  ./tools/check_abi_anchors.py             # 核对；漂移/待归属增加 rc=1，歧义/越界 rc=2
  ./tools/check_abi_anchors.py --list      # 打印"每个锚点现在指着哪句代码"（归属自查）
  ./tools/check_abi_anchors.py --rebind    # 自动收尾两类行号错位：搬家（改文档）+ 改号（刷基线）；--dry 只看不动

`--bless` 的护栏（批次 355）：全量重采会覆盖核对器**此刻正判为假**的条目，批次 355 实测
"装一条改号 + 一条漂移 → --bless → rc=0 且打印'锚点全部对上'"，那条假引用从此带着工具自己
盖章的证据。所以现在要先拦一次：被抹平的条目逐条点名、整次拒收（rc=2）；确实要覆盖得加
`--force`（仍打印覆盖清单），只想刷个别几条用 `--bless-only`。纯新增（文档刚加了一批锚点）
不在射程内——它不覆盖任何东西。

`--rebind` 治的是这一件事：往 `tools/run_all.sh` 之类被引用的文件里插行，锚点行的**内容
一字未动、行号全漂**。批次 336/337/338/339 连续四批都是这个形状，每批手工重绑 3~4 条。
判据是**唯一命中**：拿基线里那段内容在目标文件里逐行搜（同 `normalize`、同区间长度），
命中一处才改文档，零命中（内容被就地改写过）与多命中一律拒改并原样报出来——
宁可让人再来一遍，也不猜。详见 `rebind()` 的 docstring。

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
# 位置信息：文档行号 + 解析到的文件 + 起止行 + 两组数字在**原始文档行**里的列区间。
# 列区间来自正则的 group span，`strip_exempt` 抹豁免片段时抹的是等长空格，
# 所以列号与原文一一对得上——改写时按列区间切片替换，不用再做一次文本匹配。
Pos = tuple[int, str, int, int, tuple[int, int], tuple[int, int] | None]


def collect(
    doc: Path, idx: Index
) -> tuple[
    list[tuple[tuple[str, int], str]],
    list[str],
    "Counter[str]",
    list[Row],
    int,
    list[tuple[int, str, str]],
    list[Pos],
]:
    snapshot: dict[tuple[str, int], str] = {}
    problems: list[str] = []
    pending: Counter[str] = Counter()
    rows: list[Row] = []
    positions: list[Pos] = []
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
                span_a, span_b = m.span("c"), (m.span("d") if b else None)
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
                span_a, span_b = m.span("a"), (m.span("b") if b else None)
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
            positions.append((docno, rel, start, end, span_a, span_b))
        # 裸区间：同行有路径也不绑（见模块 docstring 第 4 层）
        for m in BARE_RANGE_RE.finditer(line):
            pending[m.group(0)] += 1
            pending_locs.append((docno, m.group(0), raw.strip()[:66]))
    return (
        sorted(snapshot.items()),
        problems,
        pending,
        rows,
        external_n,
        pending_locs,
        positions,
    )


def find_snippet_lines(body: list[str], snippet: str, span: int) -> list[int]:
    """在文件里按**整段内容**找锚点现在的位置（1 起，可能多个）。

    判据与 `collect` 完全同源（同一个 `normalize`、同样按 span 行长取段），所以
    "命中"的含义就是"这一行的内容一字未变"——搬家可以自动改，就地改写不行。
    """
    hits: list[int] = []
    for p in range(1, max(1, len(body) - span + 2)):
        if normalize(" ".join(body[p - 1 : p - 1 + span])) == snippet:
            hits.append(p)
    return hits


def pair_renumber(
    old: dict[tuple[str, int], str],
    new: dict[tuple[str, int], str],
    gone: list[tuple[str, int]],
    added: list[tuple[str, int]],
) -> tuple[list[tuple[str, int, int]], list[tuple[str, int]], list[tuple[str, int]]]:
    """把"消失 + 新"配成一次改号，返回 (配对, 落单消失, 落单新)。

    存在的理由（任务 #52 第 3 层 / 批次 354）：文档里手工把 `:543` 改成 `:574` 时，核对器
    看到的是**两笔互不相干的账**——`[消失] f:543` 加 `[新锚点] f:574`，rc=1。当时唯一的
    关闭手段是 `--bless`，而它无条件重采**全部**锚点：为了收掉这一对，会把同一时刻真实
    存在的其它问题（别处刚漂、刚被就地改写）一起洗成"对上"。也就是说，这条路径要求人
    用一个破坏性的动作去修一个机械可判的动作。

    配对判据与 `--rebind` 的"搬家"同源，同样不猜：
      1) 同一目标文件；
      2) 内容**逐字相同**（同一个 `normalize`、同一段长度，因为基线存的就是那段文本）；
      3) **两边都只有这一个候选**——一条消失对应多条新（或反之）时全部拒配。
         第 3 条是本函数存在的全部意义：配对错了，等于给一条合同引用换了一条
         逐字核对过的假证据（批次 341 反证 B 逼出来的同一条教训）。
    """
    added_by: dict[tuple[str, str], list[int]] = defaultdict(list)
    for rel, line in added:
        added_by[(rel, new[(rel, line)])].append(line)
    gone_by: dict[tuple[str, str], list[int]] = defaultdict(list)
    for rel, line in gone:
        gone_by[(rel, old[(rel, line)])].append(line)

    pairs: list[tuple[str, int, int]] = []
    unmatched_gone: list[tuple[str, int]] = []
    for key, glines in sorted(gone_by.items()):
        alines = added_by.get(key, [])
        if len(glines) == 1 and len(alines) == 1:
            pairs.append((key[0], glines[0], alines[0]))
        else:
            unmatched_gone += [(key[0], g) for g in glines]
    paired_new = {(rel, n) for rel, _, n in pairs}
    unmatched_added = [k for k in sorted(added) if k not in paired_new]
    return pairs, sorted(unmatched_gone), unmatched_added


def rebind(
    doc: Path,
    idx: Index,
    old: dict[tuple[str, int], str],
    new: dict[tuple[str, int], str],
    positions: list[Pos],
    base: Path,
    dry: bool,
) -> int:
    """把"内容一字未变、只是行号搬家"的锚点改回文档。

    存在的理由：批次 336/337/338/339 **连续四批**都是同一个形状——往 `tools/run_all.sh`
    插一段，`docs/ABI.md` 里 3~4 条锚点当场漂，每次都靠人读核对器打印的"基线内容 vs
    现在内容"再手工改行号。既然核对器已经打出了那段内容，"这只是搬家"就是**可判定**的，
    不必也不可能靠记性。

    唯一性硬要求：那段内容在目标文件里**恰好命中一处**才改；零命中（内容被就地改写过）
    和多命中（同形文本不止一处）一律拒改并原样报出来——宁可让人再来一遍，也不猜。

    拒改的条目在刷新基线时**原样保留**（批次 341 反证控制 B 逼出来的规则）：第一版无条件
    重采，于是"文档里的 :15 已经指向别的行"这件事被 --rebind 自己洗成了"锚点全部对上"——
    rc 从 1 变 0，而那条合同引用比之前更坏（它现在带着一条逐字核对过的假证据）。
    现在搬家过的重采，拒改的留在原地继续报错，直到有人真的重读它。

    批次 354 把同一条纪律延伸到另外两类：改号配对只刷基线、不动文档（文档写的已经是对的），
    而**落单的新锚点一律不写入基线**——它没有配对的消失项，等于一条没被任何人对过的引用，
    收进去就是上面那个事故的另一种犯法方式。
    """
    drifted = [k for k in sorted(new) if k in old and new[k] != old[k]]
    gone = [k for k in sorted(old) if k not in new]
    added_keys = [k for k in sorted(new) if k not in old]
    pairs, unmatched_gone, unmatched_added = pair_renumber(old, new, gone, added_keys)
    edits: list[tuple[int, int, int, str]] = []  # 文档行 / 列起 / 列止 / 新文本
    moved: list[tuple[str, int, int, int]] = []  # 文件 / 旧行 / 新行 / 段长
    refused: list[str] = []
    refused_keys: list[tuple[str, int]] = []

    def refuse(key: tuple[str, int], msg: str) -> None:
        # 键也要记下来：刷新基线时原样保留，见函数末尾那段。
        refused_keys.append(key)
        refused.append(msg)

    for rel, line in drifted:
        refs = [p for p in positions if p[1] == rel and p[2] == line]
        if not refs:
            refuse((rel, line), f"{rel}:{line} → 文档里已找不到这条引用（或它已解析失败）")
            continue
        span = refs[0][3] - line + 1
        if any(r[3] - r[2] + 1 != span for r in refs):
            refuse(
                (rel, line),
                f"{rel}:{line} → 同一 (文件,行号) 在文档里有**不同长度**的区间引用，"
                f"不自动改（改了会让另一条继续漂）"
            )
            continue
        hits = find_snippet_lines(idx.lines(rel), old[(rel, line)], span)
        if not hits:
            refuse(
                (rel, line),
                f"{rel}:{line} → 零命中：那段内容已不在 {rel} 里（就地改写？必须人工重读合同）"
            )
            continue
        if len(hits) > 1:
            refuse(
                (rel, line),
                f"{rel}:{line} → {len(hits)} 处命中（{', '.join(map(str, hits[:5]))}"
                f"{'…' if len(hits) > 5 else ''}）：唯一性不成立，不猜"
            )
            continue
        dst = hits[0]
        if dst == line:
            refuse((rel, line), f"{rel}:{line} → 内容命中自身所在行，判据自相矛盾（工具缺陷）")
            continue
        moved.append((rel, line, dst, span))
        for docno, _, _, _, span_a, span_b in refs:
            edits.append((docno, span_a[0], span_a[1], str(dst)))
            if span_b is not None:
                edits.append((docno, span_b[0], span_b[1], str(dst + span - 1)))

    for key in unmatched_gone:
        refuse(
            key,
            f"{key[0]}:{key[1]} → 文档不再产生这条锚点（消失），且**没有唯一配对的新锚点**，"
            f"不自动改（这条合同引用现在是悬空的）",
        )

    print(
        f"rebind：漂移 {len(drifted)} 条 → 判定搬家 {len(moved)} 条 / 拒改 {len(refused)} 条"
        f"；改号配对 {len(pairs)} 对 / 落单消失 {len(unmatched_gone)} 条"
        f" / 落单新锚点 {len(unmatched_added)} 条"
    )
    for rel, line, dst, span in moved:
        rng = f"-{dst + span - 1}" if span > 1 else ""
        old_rng = f"-{line + span - 1}" if span > 1 else ""
        print(f"  [搬家] {rel}:{line}{old_rng} → :{dst}{rng}"
              f"（{span} 行内容逐字相同，全文件唯一命中）")
    for rel, line, dst in pairs:
        print(f"  [改号] {rel}:{line} → :{dst}"
              f"（文档写的已是新号；两边内容逐字相同、互为唯一候选 ⇒ 只需刷新基线）")
    for rel, line in unmatched_added:
        print(f"  [不纳新] {rel}:{line} → 落单的新锚点**不写进基线**"
              f"（它没有配对的消失项，等于一条没人核过的引用；要收它得由人显式 --bless）")
    for r in refused:
        print(f"  [拒改] {r}")

    if dry:
        if edits:
            print("  （--dry：文档未改动）")
        return 1 if (refused or unmatched_added) else 0

    if edits:
        raw_lines = doc.read_text(encoding="utf-8").splitlines()
        by_doc: dict[int, list[tuple[int, int, int, str]]] = defaultdict(list)
        for e in edits:
            by_doc[e[0]].append(e)
        for docno, group in by_doc.items():
            text = raw_lines[docno - 1]
            # 同一行内从右往左替换，前面的列号才不会被后面的改写顶掉。
            for _, c0, c1, new_text in sorted(group, key=lambda e: -e[1]):
                text = text[:c0] + new_text + text[c1:]
            raw_lines[docno - 1] = text
        doc.write_text("\n".join(raw_lines) + "\n", encoding="utf-8")
        print(f"  已改写 {doc}（{len(by_doc)} 行 / {len(edits)} 个数字）")

    # 文档一动，基线里那批"旧行号"就成了假漂移；立刻重采，让"搬家"是一次动作而不是两次。
    # 但**只重采被改过的那部分**：拒改的条目原样留着，核对器会继续报它漂/消失。
    snap2, problems2, pending2, _, _, _, _ = collect(doc, idx)
    merged = dict(snap2)
    kept = 0
    for key in refused_keys:
        if key in old and merged.get(key) != old[key]:
            merged[key] = old[key]
            kept += 1
    dropped = 0
    for key in unmatched_added:
        if merged.pop(key, None) is not None:
            dropped += 1
    base.parent.mkdir(parents=True, exist_ok=True)
    with base.open("w", encoding="utf-8") as f:
        for (rel, line), text in sorted(merged.items()):
            f.write(f"{rel}\t{line}\t{text}\n")
        for text, n in sorted(pending2.items()):
            f.write(f"{PENDING}\t{n}\t{text}\n")
    print(
        f"  基线已随之刷新 {base}（{len(merged)} 个锚点；定位失败 {len(problems2)} 条"
        + (f"；拒改的 {kept} 条原样保留 → 核对器会继续报错" if kept else "")
        + (f"；落单新锚点 {dropped} 条未写入 → 核对器会继续报错" if dropped else "")
        + "）"
    )
    return 1 if (refused or unmatched_added or problems2) else 0


def load_base(base: Path) -> tuple[dict[tuple[str, int], str], Counter[str]]:
    """读基线快照：{(相对路径, 行号): 内容} 与待归属计数。文件不在就是两份空表。"""
    old: dict[tuple[str, int], str] = {}
    old_pending: Counter[str] = Counter()
    if not base.is_file():
        return old, old_pending
    for raw in base.read_text(encoding="utf-8").splitlines():
        parts = raw.split("\t", 2)
        if len(parts) != 3:
            continue
        if parts[0] == PENDING:
            old_pending[parts[2]] = int(parts[1])
        else:
            old[(parts[0], int(parts[1]))] = parts[2]
    return old, old_pending


def write_base(
    base: Path, items, pending
) -> None:
    """按传入顺序写基线（不重排）：重排会让"只刷一条"看起来像整份文件都动了。

    先把整份文本在内存里拼好再落盘 —— 批次 355 的第一版直接 `open("w")` 后逐行写，
    循环里抛异常就留下一份**截断的基线**（0 行），比不写更坏：核对器从此看谁都像新锚点。
    """
    buf = []
    for (rel, line), text in items:
        buf.append(f"{rel}\t{line}\t{text}\n")
    for text, n in sorted(pending.items()):
        buf.append(f"{PENDING}\t{n}\t{text}\n")
    base.parent.mkdir(parents=True, exist_ok=True)
    base.write_text("".join(buf), encoding="utf-8")


def bless_only(
    base: Path, spec: str, snap: dict[tuple[str, int], str], old: dict, old_pending: Counter[str]
) -> int:
    """只重采**点名**的锚点，其余基线行逐字不动。

    存在的理由：`--bless` 是全量重采，而它是"改号"这件事在过去唯一的收尾手段
    （批次 337/338 就这么用）。全量重采会把同一时刻**另一条真问题**的证据一起抹掉
    ——批次 355 实测：副本上同时装一条改号和一条漂移，`--bless` 之后核对 rc=0、
    "锚点全部对上"，而那条漂移对应的文档引用从此带着一条工具自己盖章的假证据。
    点名重采把"我只核了这一条"这件事写进了动作本身。
    """
    named: list[tuple[str, int]] = []
    for chunk in spec.split(","):
        chunk = chunk.strip()
        if not chunk:
            continue
        rel, _, num = chunk.rpartition(":")
        if not rel or not num.isdigit():
            print(f"[E1003] --bless-only 要的是 `路径:行号`（逗号分隔），收到 {chunk!r} ⇒ 拒收")
            return 2
        named.append((rel, int(num)))
    if not named:
        print("[E1003] --bless-only 后面一个键都没解析出来 ⇒ 拒收")
        return 2
    missing = [k for k in named if k not in snap]
    if missing:
        # 点名点不到 = 文档里根本没有这条引用（或路径/行号打错了）。静默跳过就成了
        # "以为刷了新号、其实基线还是旧的"，比不刷更坏。
        for rel, line in missing:
            print(f"  [点名不中] {rel}:{line} — 当前文档里没有这条锚点，什么都没刷")
        print(f"[E1004] --bless-only 的 {len(missing)}/{len(named)} 条点不到 ⇒ 整次拒收（不部分生效）")
        return 2
    # 已有行按基线原顺序逐字沿用（只换被点名那条的内容），新键**按 (路径, 行号) 插进应有的
    # 位置**。直接 append 实测会把基线排乱（第 158 行起逆序），而下一次全量重采又会把它排回
    # 去 —— 于是一行真改动穿上一身假 diff，"只刷一条"这件事本身被抹掉了。
    items = [(k, snap[k] if k in named else v) for k, v in old.items()]
    fresh = sorted(k for k in named if k not in old)
    for key in fresh:
        pos = next((i for i, (k, _) in enumerate(items) if k > key), len(items))
        items.insert(pos, (key, snap[key]))
    write_base(base, items, old_pending)
    print(
        f"只刷点名的 {len(named)} 条锚点 → {base}（基线共 {len(items)} 条；"
        f"其中新入表 {len(fresh)} 条）；待归属沿用基线旧值"
        f"（{sum(old_pending.values())} 条 / {len(old_pending)} 种，本档不动）"
    )
    for rel, line in named:
        mark = "新" if (rel, line) not in old else "刷"
        print(f"  [{mark}] {rel}:{line} → {snap[(rel, line)][:80]}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--doc", default=str(DEFAULT_DOC))
    ap.add_argument("--baseline", default=str(DEFAULT_BASE))
    ap.add_argument(
        "--bless",
        action="store_true",
        help="全量重采基线快照（在场问题会先拦一次；只想刷个别几条用 --bless-only）",
    )
    ap.add_argument(
        "--bless-only",
        metavar="路径:行号[,路径:行号…]",
        help="只重采点名的锚点，其余基线行逐字不动",
    )
    ap.add_argument(
        "--force",
        action="store_true",
        help="与 --bless 同用：明知在覆盖被判假的内容仍要全量重采（会打印被覆盖清单）",
    )
    ap.add_argument("--list", action="store_true", help="打印每个锚点当前指向的代码行")
    ap.add_argument("--pending", action="store_true", help="打印待归属引用的文档行号")
    ap.add_argument(
        "--rebind",
        action="store_true",
        help="把“内容逐字未变、只是行号搬家”的锚点自动改回文档（唯一命中才改）",
    )
    ap.add_argument("--dry", action="store_true", help="与 --rebind 同用：只打印判定，不动文件")
    args = ap.parse_args()
    if args.dry and not args.rebind:
        # 单挂一个不生效的开关 = 任务 #63 那一类缺陷（参数收下即弃）。当场拒收，不静默。
        print("[E1002] --dry 只对 --rebind 有意义，单独使用什么都不做 ⇒ 拒收")
        return 2
    if args.force and not args.bless:
        print("[E1002] --force 只对 --bless 有意义（--bless-only 本来就只刷点名的那条）⇒ 拒收")
        return 2
    if args.bless_only and (args.bless or args.rebind):
        print("[E1002] --bless-only 是独立动作，不与 --bless/--rebind 同用 ⇒ 拒收")
        return 2

    doc = Path(args.doc)
    base = Path(args.baseline)
    idx = Index(tracked_files())
    snap, problems, pending, rows, external_n, pending_locs, positions = collect(doc, idx)

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

    old, old_pending = load_base(base)
    new = dict(snap)
    drifted = [k for k in new if k in old and new[k] != old[k]]
    added = [k for k in new if k not in old]
    removed = [k for k in old if k not in new and k[0] != PENDING]
    pairs, unmatched_gone, unmatched_added = pair_renumber(old, new, removed, added)

    if args.bless_only:
        if not base.is_file():
            print(f"[E1001] 无基线 {base} ⇒ --bless-only 不能凭空建表（那样其余锚点会整批消失），先用 --bless")
            return 2
        return bless_only(base, args.bless_only, new, old, old_pending)

    if args.bless:
        # 全量重采会**覆盖**核对器此刻判为假的条目。批次 355 实测：副本上装一条改号 +
        # 一条漂移，`--bless` 之后核对 rc=0、打印"锚点全部对上"——那条被判定"文档引用的
        # 内容已不是它说的内容"的合同，从此带着一份工具自己盖章的证据。所以先拦一次：
        # 要覆盖就得点名（--bless-only）或明说（--force）。纯新增不在此列，它不覆盖任何东西。
        washed = sorted(drifted) + sorted(unmatched_gone)
        if washed and not args.force:
            for rel, line in washed:
                why = "内容已变（漂移）" if (rel, line) in drifted else "文档已不再引用（消失）"
                print(f"  [会被抹平] {rel}:{line} — {why}")
            print(
                f"[E1005] --bless 会把上面 {len(washed)} 条**已被判为假**的引用一并抹平 ⇒ 拒收。"
                f"搬家/改号用 --rebind，确实只重采个别几条用 --bless-only 点名，"
                f"都要覆盖才加 --force"
            )
            return 2
        write_base(base, snap, pending)
        if washed:
            print(f"  （--force：明知故犯，本次覆盖了 {len(washed)} 条被判为假的引用）")
        print(
            f"基线已写入 {base}（{len(snap)} 个锚点 + "
            f"{sum(pending.values())} 条待归属 / {len(pending)} 种）"
        )
        return 2 if problems else 0

    print(f"锚点：{len(snap)} 个可解析 / {len(problems)} 个定位失败（{doc}）")
    if not base.is_file():
        print(f"[E1001] 无基线 {base}，先跑 --bless")
        return 2 if problems else 1

    if args.rebind:
        return rebind(doc, idx, old, new, positions, base, args.dry)

    for rel, line in sorted(drifted):
        print(f"  [漂移] {rel}:{line}")
        print(f"     基线: {old[(rel, line)]}")
        print(f"     现在: {new[(rel, line)]}")
    for rel, line, dst in pairs:
        print(f"  [改号] {rel}:{line} → :{dst} — 内容逐字相同、互为唯一候选"
              f"（文档已是新号，只差刷新基线；./tools/check_abi_anchors.py --rebind 可机械关闭）")
    for rel, line in sorted(unmatched_added):
        print(f"  [新锚点] {rel}:{line} — {new[(rel, line)]}"
              f"（文档新增了一条基线里没有的引用；--rebind 不会替你收它）")
    for rel, line in sorted(unmatched_gone):
        print(f"  [消失] {rel}:{line} — 文档已不再引用（且无唯一配对的新锚点）")

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
        f"；其中改号配对 {len(pairs)} 对 ⇒ 落单新 {len(unmatched_added)} / 落单消失 {len(unmatched_gone)}"
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
