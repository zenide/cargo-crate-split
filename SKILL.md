---
name: cargo-crate-split
description: Use when a Rust project's compile times are slow and the cause is a large monolithic crate, or when the user asks how to split a crate into a workspace, break a dependency cycle, decide what goes in which crate, or enforce an architecture layering in CI. Plans a provably-acyclic split — it does not move code.
---

# cargo-crate-split

Analyze a Rust crate's module dependency graph and produce a **provably-acyclic** plan for splitting it into smaller workspace crates, with the cheapest cycle-breaking edits ranked first and a concrete refactor instruction for each. Suggest-only: it never rewrites source — you apply the moves.

## When to use this

- "Our Rust build is slow / the crate is too big — can we split it?"
- "What would a workspace decomposition of this crate look like?"
- "Why can't I split this crate — what's the circular dependency?"
- "Enforce that `domain` never depends on `http` (architecture fitness in CI)."

## Install

```sh
cargo install cargo-crate-split
```

Runs as a cargo subcommand: `cargo crate-split analyze ./path/to/crate` (the dir containing `Cargo.toml`).

## The workflow

1. **Get the machine-readable plan.** Always start here — the JSON is a ranked work queue.
   ```sh
   cargo crate-split analyze ./crate --json
   ```
   Key fields:
   - `summary.recommended_cut_count` — total cycle-breaking edits needed. **If `0`, the crate is already an acyclic DAG at this granularity — it can be split into `proposals[]` crates with no refactoring.** That's often the answer.
   - `proposals[]` — the proposed crates, each with `crate_name`, `modules`, `loc`, `layer`, `depends_on`. Already topologically layered (layer 0 = foundation).
   - `back_edges[]` — the cuts, cheapest first. Each has `source_module`, `target_module`, `occurrence_count`, `occurrences[]` (`file`/`line`/`path`), and a `classification`:
     - `kind`: `move-type-to-foundation` | `dependency-inversion` | `inspect-glob-reexport` | `extract-shared`
     - `difficulty`: `trivial` | `moderate` | `hard`
     - `distinct_symbols`: the leaf symbols crossing the edge
     - `suggestion`: a concrete, imperative instruction for breaking it
   - `frontier[]` — for each prefix of `k` cheapest cuts: `refs_cut`, `crate_count`, `largest_loc`, `freed_modules`. Shows where the payoff actually lands.

2. **Read the human report when you need the reasoning** (`analyze ./crate`, add `--mermaid` for a DAG diagram).

3. **Execute cuts cheapest-first.** Do every `trivial` `move-type-to-foundation` cut first: relocate the named type(s) into a new shared foundation crate that both sides depend on, then update imports. These are low-risk and unblock the most structure. Re-run after each batch to confirm the cycle count drops.

4. **Stop where the payoff is.** Use `frontier[]`: if `largest_loc` doesn't drop until cut #11, stopping at #5 buys nothing structural — but check `freed_modules` to see if early cuts already peel off useful leaf crates. Constrain with `--max-cuts N` or `--budget M`.

5. **Crack the hard floor.** If the largest proposed crate is still huge, re-run *that crate's directory* (or the whole crate) at `--granularity 2`. Deep module trees often decompose for free one level down.

## CI architecture-fitness guard

```sh
# Fail if any cycle exists at all:
cargo crate-split analyze ./crate --check

# Fail if a dependency violates a pinned foundation→top layering:
cargo crate-split analyze ./crate --respect-order "models,queries,db,handlers,app" --check
```

Exit code `1` + a greppable violation list when dirty, `0` when clean. Pin the agreed architecture once and the build breaks the day a violating upward dependency is introduced.

## Important constraints

- **Suggest-only.** The tool plans; you make the edits. `scaffold` writes only to a fresh `--out` dir, never to the source crate.
- **The cut set is a lower bound.** The `syn` backend resolves explicit `crate::…` paths; it can miss couplings hidden behind glob re-exports (`pub use foo::*`) or fully type-inferred calls. You may need *at least* the reported cuts, occasionally a few more. Cuts it reports are always real.
- **LOC ≈ compile cost, not = it.** Macro/derive-heavy modules cost more per line. Treat `largest_loc` as a proxy.

## Quick command reference

| Goal | Command |
|---|---|
| Full plan, machine-readable | `cargo crate-split analyze ./crate --json` |
| Human report + DAG diagram | `cargo crate-split analyze ./crate --mermaid` |
| Split deeper into the module tree | `cargo crate-split analyze ./crate --granularity 2` |
| Only the cheapest N edits | `cargo crate-split analyze ./crate --max-cuts N` |
| Cap total references touched | `cargo crate-split analyze ./crate --budget M` |
| Report layering violations | `cargo crate-split analyze ./crate --respect-order "a,b,c"` |
| CI guard (exit 1 if dirty) | `cargo crate-split analyze ./crate --check` |
| Generate empty workspace skeleton | `cargo crate-split scaffold ./crate --out ./ws` |
