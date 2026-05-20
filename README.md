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

## How back-edges are chosen (weighted ELS feedback-arc-set)

When an SCC contains a real cycle, breaking it means cutting at least one edge. The wrong edge to cut is the heavy, semantically-healthy one (e.g. `handlers → queries` with hundreds of references — that's the main data-flow direction). The right edge to cut is the **light, minority-direction** one going the "wrong way" (e.g. a single `queries → ai` reference that a foundation module shouldn't have).

The tool picks the cut set with the **Eades–Lin–Smyth greedy heuristic (the "GR" feedback-arc-set algorithm), weighted by occurrence count**:

- Each intra-SCC edge is weighted by how many `crate::…` references produced it.
- The algorithm computes a vertex ordering that approximately **maximizes the total weight of forward edges** (equivalently, minimizes the weight of edges it must cut): it repeatedly peels sinks to the end and sources to the start, and when neither exists removes the vertex maximizing `weighted_out_degree − weighted_in_degree`.
- The **back-edges** are then the edges that point "upward" against this order. Because the order kept the heavy edges forward, the cut set is the light minority-direction edges.

On a 2-node cycle `x ↔ y` where `x → y` has 10 references and `y → x` has 2, it cuts `y → x` (the 2). On a real codebase this drops the total cut weight by roughly an order of magnitude versus a naive DFS-finishing-order heuristic.

## `--respect-order`: "I know my intended architecture; show me what breaks it"

If you already have an intended layering in mind, pass it explicitly and the tool reports exactly the edges that **violate** it instead of computing its own order:

```sh
cargo crate-split analyze ./crate --respect-order "models,queries,db,ai,handlers,app"
# `<` works too, and whitespace is ignored:
cargo crate-split analyze ./crate --respect-order "models < queries < db < ai < handlers < app"
```

**SPEC format:** a module order from **foundation to top**, separated by `,` or `<`. Earlier = more foundational; later = higher in the stack. A reported violation is an edge `u → v` where `v` is **later** than `u` in your order — i.e. a module depending "upward" on something that should sit above it.

Modules you don't name keep their computed position (placed after all the named modules, by their ELS-relative order). The flag is also accepted by `scaffold`. Omit it and the weighted ELS heuristic above is used.

It is **suggest-only**: it never rewrites your source. You apply the moves in your IDE, guided by the plan.

## Usage

```sh
cargo install --path .
cargo crate-split analyze ./path/to/crate            # text report
cargo crate-split analyze ./path/to/crate --mermaid  # + Mermaid DAG
cargo crate-split analyze ./path/to/crate --json      # machine-readable
cargo crate-split analyze ./path/to/crate --granularity 2   # split at 2 module levels deep
cargo crate-split analyze ./path/to/crate --respect-order "models,queries,handlers"  # report order violations
cargo crate-split scaffold ./path/to/crate --out ./workspace  # generate skeleton
```

## Accuracy

v1 uses `syn` AST + path analysis (`use crate::…` and `crate::…::` references). This captures the large majority of module-to-module edges with no build step. It can miss edges that only appear through glob re-exports or type inference without an accompanying `use`. A future backend can swap in rust-analyzer (`ra_ap_*`) for full name-resolution accuracy behind the same graph interface.

## License

MIT OR Apache-2.0
