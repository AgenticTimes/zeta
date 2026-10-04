# Architecture Health Report — `docs/diagrams/zeta-*.drawio`

Produced by: `architecture-visualization:architecture-health` (routed via `explore`).
Audit date: 2026-09-25. Repo HEAD at audit: `11d54204`.
Audited artifacts: `zeta-pipeline-flow.drawio`, `zeta-call-chain.drawio`, `zeta-core-classes.drawio` — all last committed `e881e516` (2026-09-24).
Scope: `src/` (172 `.rs`, the real compiler) + `runtime/` C. `zeta_src/` excluded (0 `.rs`, port corpus).
Reproduce: `python3 docs/architecture-health/check-arch-claims.py .` — exit code is the FAIL count.

## Verdict

**Trust the shapes, not the numbers.**

All three diagrams are structurally sound: every pipeline stage, every symbol, and every
call-chain edge I could check resolves in `src/`. What is not sound is the set of numeric and
field-attribution assertions in `zeta-core-classes.drawio`, which are already wrong, and the total
absence of `sourceRefs` in all three, which is what let the wrong numbers survive.

36 claims checked: **27 PASS / 9 FAIL**.

| Diagram | Nodes/Edges | Symbol & flow claims | Numeric claims | Confidence |
| --- | --- | --- | --- | --- |
| `zeta-call-chain` | 15 / 12 | all pass | none made | **high** — safe to rely on |
| `zeta-pipeline-flow` | 9 / 8 | all pass | `198` passes; `40+` understated | **high** for the flow |
| `zeta-core-classes` | 13 / 5 | methods/fields mostly pass | 5 of 7 fail | **low** — do not cite numbers from it |

## Findings

### F-1 [high] `Resolver` field list is a pre-batch-300 snapshot

`zeta-core-classes.drawio` (`resolver-b`) lists fields `fns / classes / fn_rets / func_ret_types`.
**Zero of the four are fields of `pub struct Resolver`** (`src/middle/resolver/resolver.rs:35`), which
owns 31 fields: `impls, cached_mirs, mono_mirs, borrow_checker, associated_types, ctfe_consts,
funcs, registered_funcs, module_resolver, macro_expander, type_decls, generated_closures,
nonlocal_names, module_globals, …` (resolver.rs:36-118).

This is not invented content — it is a real older snapshot. `git log -S` shows `    classes:` and
`    fn_rets:` each touched by 2 commits, the last being **`610b8db5` 2026-09-21, "batch 300"**.
The same box *does* cite `unannotated_return_ty()（批次 300）` (confirmed at resolver.rs:2192), so the
box was partially updated in the batch-300 era and the field list was left behind.

Impact: an agent reading this box will look for `resolver.fn_rets` and find a *parameter*
(`fn_rets: &HashMap<String, Type>` at resolver.rs:1783), not a field, and may "fix" code to match a
struct that does not exist.

### F-2 [high] `C 运行时（462 个函数）` is unreproducible

Two methods agreed on **816** column-0 function definitions across the two files the box names
(`runtime/tokio_runtime_stub.c` 506 + `runtime/py_additions.c` 310). A looser regex gave 502, which
was an undercount (it required the opening brace on the same line).

462 does not match any plausible narrower scope either, so the counting rule behind it is lost, not
merely stale:

| candidate scope | count |
| --- | --- |
| all definitions, 2 named files | 816 |
| `py_*` | 389 |
| `zeta_*` | 123 |
| `host_*` | 55 |
| `host_* + zeta_* + vec_*` | 184 |
| all four prefixes | 573 |

There is also a boundary ambiguity feeding this: `runtime/` holds 6 C sources
(`py_additions.c`, `aliases.inc.c`, `parquet_min.c`, `unavailable_stubs.c`, `tokio_runtime_stub.c`,
`capybara_runtime.c`) but the box names only two.

### F-3 [medium] `gen.rs 13,785 行` and `resolver.rs 4.7k 行` are stale

Actual: `src/middle/mir/gen.rs` **14,644** (claim low by 859, −5.9%); `resolver.rs` **5,121**
(claim low by ~420). `codegen.rs 7.6k` **holds** at 7,650.

Note on the moving target: my first read of `gen.rs` at ~06:20 was 14,645; at 06:27 the file's mtime
changed and three independent methods agree on 14,644. **`gen.rs` was being edited during this
audit.** Any hand-typed line count in a diagram in this repo is wrong within days — 787 commits in
the last 30 days.

### F-4 [low] `Mir` field `globals` does not exist

`src/middle/mir/mir.rs:6-26` has `name, param_indices, stmts, exprs, ctfe_consts, type_map,
global_consts, properties, generic_params, is_extern`. `stmts: Vec<MirStmt>` (:9),
`exprs: HashMap<u32, MirExpr>` (:10), `param_indices` (:8) all confirmed verbatim. `globals` is
absent; nearest is `global_consts` (:13), which is a different thing.

### F-5 [low] `Type（types/mod.rs，50+ 变体）` counts 47

`pub enum Type` spans `src/middle/types/mod.rs:94-182`; 47 variants
(`I8 … Usize, F32, F64, I32x4, V4I64, …, PyDynamic`). The `40+` claim on `AstNode` passes at **65**
(`src/frontend/ast.rs:28-367`) but is understated by 25 — both counts are one-variant-per-line
based, the Rustfmt convention here.

### F-6 [low] No `sourceRefs` anywhere; `core-classes` has no rendered export

Across all three files: 0 occurrences of `\.rs:[0-9]+`, `sourceRef`, or `src/` in 62 `mxCell`
elements. That is the root cause making F-1…F-5 undetectable by inspection — nothing binds a claim
to a file, so nothing fails loudly when the file moves.

Separately, `docs/diagrams/` has `zeta-pipeline-flow.drawio.png` and `zeta-call-chain.drawio.png`
but **no** `zeta-core-classes.drawio.png`, so the one diagram with the most broken claims is also
the one never eyeballed in review.

## What is safe to rely on right now

The call-chain diagram's semantic claim about identity conversion is the strongest single result in
this audit — `runtime/tokio_runtime_stub.c:2464` reads
`int64_t host_str_to_string(int64_t s) { return s; }`, matching the label "恒等：字符串不可变，返回同一句柄" verbatim.

Confirmed against code with concrete locations:

| Claim | Evidence |
| --- | --- |
| `indent::indent_preprocess` | `src/middle/resolver/resolver.rs:2534` |
| `parse_zeta` / `parse_top_level_entry` | `src/middle/resolver/module_resolver.rs:639`, `src/frontend/parser/top_level.rs:1608` |
| `parse_block_body` → `parse_full_expr` | `src/frontend/parser/stmt.rs:21`, `:58` |
| `W1002`丢行 / `ZETA_PARSE_TRACE` 探针 | `src/frontend/parser/parser.rs:284`, `src/frontend/parser/stmt.rs:67` |
| `MirGen::lower_to_mir` | `src/middle/mir/gen.rs:1067` |
| `str_method_symbol` 决定运行时符号 | `src/middle/mir/gen.rs:10224` |
| `Resolver::lower_to_mir`（also on Resolver） | `src/middle/resolver/resolver.rs:3235` |
| `LLVMCodegen::gen_mirs` / `get_function` | `src/lib.rs:148`, `src/backend/codegen/codegen.rs:2275` |
| `host_result_is_ok` / `zeta_call1` | `src/lib.rs:231`, `runtime/py_additions.c:3414` |
| `MirGen` fields `type_map/exprs/func_ret_types/type_decls` | `gen.rs:104 / :102 / :217 / :118` |
| 链接两个 `.o` | `tools/build_runtime.sh:3,42`；`zeta_runtime_c.o` present |
| `源码 .z（官方 198 个）` | `find tests/unit-tests -name '*.z'` = **198**; referent corroborated by `backlog.md:44` |

Caveat on 198: the *gate* denominator is 194 (`tests/unit-tests` non-recursive), and `backlog.md:44`
itself notes the paired "39 个 `.py`" is an out-of-repo directory whose count moves. `git ls-files
'*.py'` = 32, and the 39 is not verifiable from inside this repo.

## Corrections I made to my own readings during this audit

Three of my first-pass numbers were artifacts of my own tooling and are superseded above:

1. `AstNode` counted 63, then 65 — the first regex character class omitted `,`, so bare variants
   like `I8,` were silently skipped. Same class bug briefly made `Type` read 16 before it read 47.
2. `C functions` read 502 by a brace-on-same-line regex — an undercount, not an overcount.
3. I initially wrote a note claiming `zeta_runtime_c.o` did not exist. It does, at the repo root,
   built by `tools/build_runtime.sh:42`. The `traceability-index.json` note is corrected and
   `a-link` is back to `high`.

## Follow-up checks

Deterministic and re-runnable — put the first in CI alongside the existing gate:

- [ ] `python3 docs/architecture-health/check-arch-claims.py .` must exit 0.
- [ ] Replace F-1/F-2/F-3/F-4/F-5 numbers in `zeta-core-classes.drawio` with either the live value
      plus a `(checked <date>)` suffix, or a range that survives drift — do not hand-patch exact
      integers that the checker will re-break within a week.
- [ ] Add `sourceRefs` to node labels (`gen.rs:1067 lower_to_mir` style) so a moved symbol breaks
      visibly instead of quietly.
- [ ] Render `zeta-core-classes.drawio.png` so it gets eyeballed like the other two.
- [ ] Decide the `C 运行时` counting scope before restating its number.

## Open design decisions

- [ ] `needs-design-decision`: should exact line counts appear in diagrams at all, given `gen.rs`
      moved mid-audit? Candidates: drop the number, cite only `file:symbol`, or keep numbers and let
      the checker gate them.
- [ ] `needs-design-decision`: whether `runtime/`'s other 4 C sources belong inside the "C 运行时"
      boundary or are a separate node.
