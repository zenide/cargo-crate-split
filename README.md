# cargo-crate-split

Analyze a large single-crate Rust project's **module dependency graph** and propose how to split it into a **workspace of smaller crates** — in a way that is **provably free of circular crate dependencies**.

Splitting a monolithic crate is the highest-leverage way to speed up Rust compile times: the crate is rustc's unit of compilation, caching, and parallelism. But Rust forbids circular crate dependencies, so a naive split fails to compile. The hard part isn't moving the code — your IDE (via rust-analyzer) already does that well — it's deciding **what goes in which crate so the result is a DAG**. That decision is what this tool computes.

## What it does

1. Parses every `.rs` file with `syn` and builds a directed graph: node = module, edge `A → B` = "module `A` references `crate::B`".
2. Computes **strongly-connected components** (Tarjan). Modules in the same SCC are mutually recursive — they *cannot* live in separate crates without breaking an edge, so each SCC is the atomic unit of a split.
3. **Condenses the SCCs into a graph, which is mathematically guaranteed to be a DAG**, and topologically sorts it into dependency layers. Leaf (foundation) crates — those with no outgoing dependencies — fall out automatically.
4. Reports:
   - the proposed crates (modules → crate) with LOC each,
   - the crate dependency DAG as a **Mermaid diagram**,
   - for any cycle you'd like to split further, the exact **back-edges** to remove (with `file:line` and the referenced path), ranked by how few must be cut.
5. Optionally scaffolds the empty workspace skeleton (crate dirs + `Cargo.toml`s).

It is **suggest-only**: it never rewrites your source. You apply the moves in your IDE, guided by the plan.

## Usage

```sh
cargo install --path .
cargo crate-split analyze ./path/to/crate            # text report
cargo crate-split analyze ./path/to/crate --mermaid  # + Mermaid DAG
cargo crate-split analyze ./path/to/crate --json      # machine-readable
cargo crate-split analyze ./path/to/crate --granularity 2   # split at 2 module levels deep
cargo crate-split scaffold ./path/to/crate --out ./workspace  # generate skeleton
```

## Accuracy

v1 uses `syn` AST + path analysis (`use crate::…` and `crate::…::` references). This captures the large majority of module-to-module edges with no build step. It can miss edges that only appear through glob re-exports or type inference without an accompanying `use`. A future backend can swap in rust-analyzer (`ra_ap_*`) for full name-resolution accuracy behind the same graph interface.

## License

MIT OR Apache-2.0
