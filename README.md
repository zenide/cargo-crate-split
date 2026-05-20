# cargo-crate-split

[![crates.io](https://img.shields.io/crates/v/cargo-crate-split.svg?logo=rust&color=fc8d62)](https://crates.io/crates/cargo-crate-split)
[![downloads](https://img.shields.io/crates/d/cargo-crate-split.svg?color=66c2a5)](https://crates.io/crates/cargo-crate-split)
[![license](https://img.shields.io/crates/l/cargo-crate-split.svg?color=8da0cb)](#license)
[![cargo install](https://img.shields.io/badge/install-cargo%20install%20cargo--crate--split-dca060?logo=rust)](https://crates.io/crates/cargo-crate-split)

Analyze a large single-crate Rust project's **module dependency graph** and propose how to split it into a **workspace of smaller crates** — in a way that is **provably free of circular crate dependencies**, ranked cheapest-first, with a concrete refactor instruction for every edit it asks you to make.

Splitting a monolithic crate is the highest-leverage way to speed up Rust compile times: the crate is rustc's unit of compilation, caching, and parallelism. But Rust forbids circular crate dependencies, so a naive split fails to compile. The hard part isn't moving the code — your IDE already does that. It's deciding **what goes in which crate so the result is a DAG**, and **which few edges are in the way**. That decision is what this tool computes.

It is **suggest-only**: it never rewrites your source. You (or a coding agent) apply the moves, guided by the plan.

---

## Quick install

```sh
# From crates.io (recommended)
cargo install cargo-crate-split

# Then run it as a cargo subcommand:
cargo crate-split analyze ./path/to/crate
```

### Install the coding-agent skill

This repo ships a [`SKILL.md`](./SKILL.md) so Claude Code (and compatible agents) know when and how to drive the tool. Drop it into your project:

```sh
mkdir -p .claude/skills/cargo-crate-split
curl -fsSL https://raw.githubusercontent.com/zenide/cargo-crate-split/main/SKILL.md \
  -o .claude/skills/cargo-crate-split/SKILL.md
```

Now an agent can answer "can we split this crate to speed up builds?" by running the tool and acting on the ranked, classified suggestions — including the exact type to move for each cut.

---

## What it found on real projects

The most important question about a tool like this is: **does it produce something actually useful on real code?** Here is the unedited verdict on four well-known Rust binaries, with an honest read on each.

| Project | LOC analyzed | Raw graph | After cheap cuts | Cuts needed |
|---|---|---|---|---|
| **nu-command** (Nushell) | 92k | 30 modules, **already a DAG** | 30 crates, largest 14k LOC | **0** |
| **starship** (`--granularity 2`) | 50k | 238 modules, **already a DAG** | 238 crates, largest 2.3k LOC | **0** |
| **starship** (`--granularity 1`) | 50k | 1 blob, 48.8k LOC | 15 crates, largest 37k LOC | 11 (20 refs) |
| **alacritty** | 21k | 8 SCCs, 19.4k LOC | 18 crates, largest 4.5k LOC | 13 (39 refs) |
| **bat** | 11k | 25 crates, 2.3k LOC | 26 crates | 1 (2 refs) |

Two findings are worth dwelling on, because they're the kind of thing you can't eyeball:

**1. Many "monoliths" are already splittable today, for free.** `nu-command` is 92,000 lines in one crate — and at the top module level it is *already an acyclic DAG*. Zero cycle-breaking edits required: 30 of its modules could each become their own crate this afternoon, and the workspace would compile. Same story for starship's giant `modules/` directory at `--granularity 2`: 238 prompt-segment modules, **zero** circular dependencies, largest piece 2.3k LOC. The perception of "untangleable monolith" was psychological, not structural. The tool proves it mechanically.

**2. When cuts *are* needed, they're shockingly cheap and precisely located.** alacritty looks like an 8-way tangle (largest SCC 19.4k LOC). The tool shows the entire thing decomposes into 18 crates after cutting just **39 reference occurrences across 13 edges**, ranked so you do the trivial ones first. starship's `--granularity 1` blob (48.8k LOC in one SCC) breaks into 15 crates for **20 reference edits** — most of them one-line type moves like:

```
✂ cut  segment -> config  (1 occurrence, 1 file)
    kind: move-type-to-foundation  |  difficulty: trivial
    fix: Move `Style` out of `config` into a shared foundation crate that both
         `segment` and `config` depend on. The upward edge then disappears.
    src/segment.rs:2  crate::config::Style
```

### The effort/payoff frontier (alacritty)

Removing one edge usually does **nothing** until a *key* cycle-closing edge goes — so the largest crate shrinks in **steps**, not smoothly. The frontier makes those unlock points visible:

```
   cuts   refs cut  #crates   largest LOC   modules newly freed
  ------------------------------------------------------------
      0          0        8         19415   —
      4          4       10         18737   scheduler
      9         13       13         17676   logging, message_bar
     10         16       14         15399   input
     11         20       16         10673   cli, config
     12         27       17          8580   event
     13         39       18          4471   display, renderer    ← the heavy graphics
```

You can stop anywhere on this curve with `--budget`/`--max-cuts` and take the payoff you've paid for.

### Honest limitations (read before trusting a "0")

- **The syn backend can miss edges.** It resolves explicit `use crate::…` and `crate::…::` paths. It does **not** see couplings that exist only through glob re-exports (`pub use foo::*`) or fully type-inferred cross-module calls with no path written. So a reported cut set is a **lower bound**: you need *at least* those cuts; a glob-heavy crate may hide a few more. The cuts it *does* report are real (they point at real references). False negatives are possible; false positives are not. (A rust-analyzer backend for full name resolution is on the [roadmap](https://github.com/zenide/cargo-crate-split/issues/1).)
- **LOC is a crude compile-cost proxy.** A 4k-LOC macro-heavy crate (sqlx `query!`, big derives) can cost more than 30k LOC of plain code. The "largest LOC" column approximates, it doesn't measure, compile time.
- **The hard floor.** At `--granularity 1` the biggest module often stays huge (starship's `modules` = 37k LOC in one node). The fix is to re-run that level at `--granularity 2` — which, for starship, shattered it into 238 free-standing crates. The tool won't auto-recurse for you (yet).

The takeaway: treat it as a fast, mechanical **second opinion on splittability** — strongest at saying "this is already a DAG, just split it" and "here are the N cheapest, exact edits in your way," weakest where globs hide edges or where LOC misrepresents cost.

---

## How it works

1. Parses every `.rs` file with `syn` and builds a directed graph: node = module, edge `A → B` = "module `A` references `crate::B`".
2. Computes **strongly-connected components** (Tarjan). Modules in the same SCC are mutually recursive in the raw graph.
3. Finds the cheap **cut set** that breaks those cycles (weighted Eades–Lin–Smyth feedback-arc-set): the light, minority-direction edges.
4. **Removes the cut set and recomputes SCCs on `G − cuts`.** The proposed crates are the SCCs of that *reduced* graph — i.e. the split you'd actually get after the suggested edits. Its condensation is mathematically guaranteed to be a DAG and is topologically sorted into dependency layers.
5. **Classifies each cut** and emits a concrete instruction + difficulty (`move-type-to-foundation`, `dependency-inversion`, `inspect-glob-reexport`, `extract-shared`).
6. Reports the proposed split, the crate DAG (text + Mermaid), the ranked required cuts, and the effort/payoff frontier.
7. Optionally scaffolds the empty workspace skeleton.

### Why the post-cut proposal matters

Reporting the SCCs of the *raw* graph is pessimistically useless: a single 3-reference import can collapse nine modules into one "inseparable" crate, making a codebase look unsplittable when it's one cheap edit away from decomposing. This tool proposes the split **achievable after a small, cheap set of edits**, and lists those edits as the price.

### How back-edges are chosen (weighted ELS feedback-arc-set)

The wrong edge to cut is the heavy, semantically-healthy one (`handlers → queries`, hundreds of references — the main data-flow direction). The right edge is the **light, minority-direction** one going the wrong way (a single `queries → ai` reference a foundation module shouldn't have). The tool computes a vertex ordering that approximately **maximizes the weight of forward edges** (Eades–Lin–Smyth "GR" heuristic, weighted by occurrence count); the **back-edges** are the light edges pointing upward against that order. On a 2-node cycle `x ↔ y` with `x→y` weight 10 and `y→x` weight 2, it cuts the 2.

---

## CLI reference

```sh
cargo crate-split analyze ./crate                    # text report
cargo crate-split analyze ./crate --mermaid          # + Mermaid DAG
cargo crate-split analyze ./crate --json             # machine-readable (for agents/CI)
cargo crate-split analyze ./crate --granularity 2    # split 2 module levels deep
cargo crate-split analyze ./crate --max-cuts 3       # propose the split after only the 3 cheapest cuts
cargo crate-split analyze ./crate --budget 6         # ...or cap total references cut at 6
cargo crate-split analyze ./crate --respect-order "models,queries,handlers"   # report order violations
cargo crate-split analyze ./crate --respect-order "models,queries" --check    # CI fitness fn (exit 1 on violation)
cargo crate-split analyze ./crate --check            # CI guard: exit 1 if any cycle exists
cargo crate-split scaffold ./crate --out ./workspace # generate empty skeleton
```

### `--check`: an architecture-fitness function for CI

`--check` exits non-zero when the module graph isn't clean, printing a concise, greppable list. Two modes:

- **Plain `--check`** fails when any cycle exists — a "no new circular dependencies" guard.
- **`--check --respect-order "<foundation,…,top>"`** pins your intended layering and fails when a module depends *upward* on something that should sit above it. Pin your architecture once; the build breaks the day someone introduces a violating dependency.

```sh
$ cargo crate-split analyze ./src --check
cargo-crate-split: architecture check of `bat` (granularity 1)
  mode: acyclic (cycle-closing edges)
  FAIL — 1 violation(s):
    decorations -> printer  (2 ref(s), 1 file(s), move-type-to-foundation, trivial)
        first at src/decorations.rs:3  crate::printer::Colors
$ echo $?
1
```

### `--respect-order`: "I know my intended architecture; show me what breaks it"

A module order from **foundation to top**, separated by `,` or `<` (whitespace ignored). A reported violation is an edge `u → v` where `v` is **later** than `u` — a module depending upward. Modules you don't name keep their computed position.

### `--budget` / `--max-cuts`: pick an effort budget

By default the tool proposes applying **all** recommended cuts. Constrain it to a prefix of the cheapest cuts with `--max-cuts N` or `--budget M` (cap on cumulative references cut). The frontier table always covers the full cut set, so you can see what the next cut would buy.

---

## For coding agents

This tool is built to be driven by an agent. Tips:

- **Start with `--json`.** Every struct is typed; the `back_edges[]` array carries `source_module`, `target_module`, `occurrence_count`, `classification.kind`, `classification.difficulty`, `classification.distinct_symbols`, `classification.suggestion`, and exact `occurrences[]` with `file`/`line`/`path`. That is a ready-made, ranked work queue.
- **Do the `trivial` `move-type-to-foundation` cuts first.** They're one-line type relocations into a shared crate and unblock the most structure for the least risk.
- **Use the frontier to decide where to stop.** If the largest crate doesn't drop until cut #11, there's no payoff in stopping at #5 — but #1–#4 might already free several leaf modules. Match effort to the unlock points.
- **If the biggest proposed crate is still huge, re-run it at `--granularity 2`.** That's where the real parallelism often hides (see starship above).
- **Wire `--check` into CI** once a target architecture is agreed, so regressions fail fast.
- **It never edits source.** The agent does the moves; the tool only plans and verifies.

See [`SKILL.md`](./SKILL.md) for the full agent playbook.

---

## License

MIT OR Apache-2.0
