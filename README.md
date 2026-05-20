# cargo-crate-split

Analyze a large single-crate Rust project's **module dependency graph** and propose how to split it into a **workspace of smaller crates** — in a way that is **provably free of circular crate dependencies**.

Splitting a monolithic crate is the highest-leverage way to speed up Rust compile times: the crate is rustc's unit of compilation, caching, and parallelism. But Rust forbids circular crate dependencies, so a naive split fails to compile. The hard part isn't moving the code — your IDE (via rust-analyzer) already does that well — it's deciding **what goes in which crate so the result is a DAG**. That decision is what this tool computes.

## What it does

1. Parses every `.rs` file with `syn` and builds a directed graph: node = module, edge `A → B` = "module `A` references `crate::B`".
2. Computes **strongly-connected components** (Tarjan). Modules in the same SCC are mutually recursive in the raw graph.
3. Finds the cheap **cut set** that breaks those cycles (weighted ELS feedback-arc-set, below): the light minority-direction edges.
4. **Removes the cut set and recomputes SCCs on `G − cuts`.** The proposed crates are the SCCs of that *reduced* graph — i.e. the split you'd actually get after making the suggested edits. The condensation of `G − cuts` is mathematically guaranteed to be a DAG and is topologically sorted into dependency layers.
5. Reports:
   - the **proposed split — achievable after N cuts**, with the modules each cut **unlocks** marked `★`,
   - the crate dependency DAG (post-cut) as a **Mermaid diagram**,
   - the exact **required cuts** to remove (with `file:line` and the referenced path), ranked cheapest-first — these are the *price* of the proposed split,
   - an **effort/payoff frontier** table showing, for each prefix of the cut set, how the decomposition improves.
6. Optionally scaffolds the empty workspace skeleton (crate dirs + `Cargo.toml`s).

### Why the post-cut proposal matters

Reporting the SCCs of the *raw* graph is pessimistically useless. A single 3-reference import (`queries → ai`) can collapse nine modules into one 130k-LOC "inseparable" crate — making a codebase look unsplittable when it's actually one cheap edit away from decomposing. This tool instead proposes the split **achievable after a small, cheap set of edits**, and lists those edits as the price.

On a real backend, the difference is stark:

```
EFFORT / PAYOFF FRONTIER
   cuts   refs cut  #crates   largest LOC   modules newly freed
      0          0       27        132029   —
      1          1       28        132029   models, services
      2          3       28        132029   —
      3          6       33        113464   auth, email, import, push, queries
      4         12       34        112567   scheduler
      5         28       35        109089   skills
      6         51       36         72904   ai, handlers
```

0 cuts → largest crate 132,029 LOC (useless). All 6 cuts (51 references) → largest crate 72,904 LOC and every module in its own crate. Note the **unlock points**: the largest crate doesn't shrink until a *key* edge goes (rows 3, 5, 6) — removing one edge at a time often leaves the big SCC intact until the last cycle-closing reference is cut.

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

## Budgeting the cuts: `--max-cuts` / `--budget`

Cuts have a cost (references to refactor) and a payoff (a finer split, faster compiles). By default the tool proposes applying **all** recommended cuts — the maximal cheap decomposition. To pick an effort budget instead, constrain which cuts are applied:

```sh
cargo crate-split analyze ./crate --max-cuts 3   # apply only the 3 cheapest cuts
cargo crate-split analyze ./crate --budget 6     # apply cheapest cuts until cumulative refs would exceed 6
```

The proposed split (and its DAG / layers / LOC) is then the decomposition of `G − (those cuts)`, and the report says how many cycles remain. `--max-cuts` and `--budget` are mutually exclusive; both compose with `--respect-order` (the "cuts" then become the order-violating edges, budgeted cheapest-first). The frontier table always covers the full recommended cut set regardless of the chosen budget, so you can see what the next cut would buy.

## Usage

```sh
cargo install --path .
cargo crate-split analyze ./path/to/crate            # text report
cargo crate-split analyze ./path/to/crate --mermaid  # + Mermaid DAG
cargo crate-split analyze ./path/to/crate --json      # machine-readable
cargo crate-split analyze ./path/to/crate --granularity 2   # split at 2 module levels deep
cargo crate-split analyze ./path/to/crate --respect-order "models,queries,handlers"  # report order violations
cargo crate-split analyze ./path/to/crate --max-cuts 3   # propose the split after only the 3 cheapest cuts
cargo crate-split analyze ./path/to/crate --budget 6     # ...or cap total references cut at 6
cargo crate-split scaffold ./path/to/crate --out ./workspace  # generate skeleton
```

## Accuracy

v1 uses `syn` AST + path analysis (`use crate::…` and `crate::…::` references). This captures the large majority of module-to-module edges with no build step. It can miss edges that only appear through glob re-exports or type inference without an accompanying `use`. A future backend can swap in rust-analyzer (`ra_ap_*`) for full name-resolution accuracy behind the same graph interface.

## License

MIT OR Apache-2.0
