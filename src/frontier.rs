//! Effort/payoff frontier: for each prefix of the cheapest-first cut set,
//! recompute the decomposition of `G − (first k cuts)` and record the tradeoff
//! between refactoring effort (occurrences cut) and payoff (finer crates,
//! smaller largest crate).
//!
//! Removing a single edge often does NOT shrink the big SCC until a *key* edge
//! goes; the `largest_loc` column makes those "unlock points" visible (it drops
//! in steps, not smoothly).

use std::collections::HashSet;

use serde::Serialize;

use crate::analyze::BackEdge;
use crate::graph::{LabelEdge, ModuleGraph};

/// One row of the frontier: the decomposition after applying the `k` cheapest
/// cuts.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FrontierStep {
    /// Number of cuts applied (`k`).
    pub cuts_applied: usize,
    /// Cumulative reference occurrences cut across those `k` cuts.
    pub refs_cut: usize,
    /// Number of proposed crates (SCCs) at this step.
    pub crate_count: usize,
    /// LOC of the largest proposed crate at this step.
    pub largest_loc: usize,
    /// Modules that became standalone (their own crate) AT THIS step — i.e. were
    /// still locked in a multi-module SCC at step `k-1` and are standalone now.
    pub freed_modules: Vec<String>,
}

/// Compute the frontier over the full (cheapest-first) `back_edges` cut set.
///
/// `locked_in_raw` is the set of module labels that sit in a multi-module SCC of
/// the raw graph `G` (the candidates that can be "freed"). We track which of
/// them become standalone at each step so the report can show unlock points.
pub fn compute_frontier(
    mg: &ModuleGraph,
    _prefix: &str,
    back_edges: &[BackEdge],
    locked_in_raw: &HashSet<String>,
) -> Vec<FrontierStep> {
    let g = &mg.graph;
    let mut steps: Vec<FrontierStep> = Vec::with_capacity(back_edges.len() + 1);

    // Track which originally-locked modules were already standalone at the
    // previous step, so each row reports only the *newly* freed ones.
    let mut already_free: HashSet<String> = HashSet::new();

    for k in 0..=back_edges.len() {
        let applied: HashSet<LabelEdge> = back_edges[..k]
            .iter()
            .map(|b| LabelEdge {
                source: b.source_module.clone(),
                target: b.target_module.clone(),
            })
            .collect();
        let refs_cut: usize = back_edges[..k].iter().map(|b| b.occurrence_count).sum();

        let sccs = mg.sccs_without(&applied);
        let crate_count = sccs.len();
        let largest_loc = sccs
            .iter()
            .map(|comp| comp.iter().map(|&n| g[n].loc).sum::<usize>())
            .max()
            .unwrap_or(0);

        // Standalone (single-module SCC) labels at this step.
        let standalone_now: HashSet<String> = sccs
            .iter()
            .filter(|c| c.len() == 1)
            .map(|c| g[c[0]].label())
            .collect();

        // Newly freed = locked-in-raw ∩ standalone-now − already-free.
        let mut freed_modules: Vec<String> = standalone_now
            .iter()
            .filter(|m| locked_in_raw.contains(*m) && !already_free.contains(*m))
            .cloned()
            .collect();
        freed_modules.sort();

        for m in &freed_modules {
            already_free.insert(m.clone());
        }

        steps.push(FrontierStep {
            cuts_applied: k,
            refs_cut,
            crate_count,
            largest_loc,
            freed_modules,
        });
    }

    steps
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use petgraph::graph::DiGraph;

    use crate::analyze::{analyze_with_budget, CutBudget};
    use crate::graph::{EdgeWeight, ModuleGraph, ModuleNode, Occurrence};
    use crate::module_path::ModulePath;

    fn node(label: &str, loc: usize) -> ModuleNode {
        ModuleNode {
            module: ModulePath {
                segments: vec![label.to_string()],
            },
            loc,
            file_count: 1,
            bin: None,
        }
    }

    fn occ(n: usize) -> EdgeWeight {
        (0..n)
            .map(|i| Occurrence {
                file: PathBuf::from("x.rs"),
                line: i + 1,
                path: "crate::x".to_string(),
            })
            .collect()
    }

    /// Build a graph that is ONE light edge away from splitting into 3 crates.
    ///
    /// Heavy forward chain a -> b -> c (each a strong dependency). One light
    /// back-edge c -> a closes the cycle, collapsing {a,b,c} into a single SCC.
    /// Removing that single cut yields three standalone SCCs.
    fn one_edge_from_three() -> ModuleGraph {
        let mut g: DiGraph<ModuleNode, EdgeWeight> = DiGraph::new();
        let a = g.add_node(node("a", 100));
        let b = g.add_node(node("b", 100));
        let c = g.add_node(node("c", 100));
        g.add_edge(a, b, occ(20)); // heavy forward
        g.add_edge(b, c, occ(20)); // heavy forward
        g.add_edge(c, a, occ(1)); // light back-edge (the one cut)
        ModuleGraph {
            graph: g,
            granularity: 1,
        }
    }

    #[test]
    fn one_cut_yields_three_sccs() {
        let mg = one_edge_from_three();
        // Raw graph: single 3-module SCC.
        let raw = petgraph::algo::tarjan_scc(&mg.graph);
        assert_eq!(raw.len(), 1, "raw graph must be one big SCC, got {raw:?}");
        assert_eq!(raw[0].len(), 3);

        // Applying ALL cuts (the default) must free all three into their own
        // crates: 3 SCCs, no cycles.
        let a = analyze_with_budget(&mg, "demo", None, CutBudget::All);
        assert_eq!(a.proposals.len(), 3, "expected 3 crates, got {:?}", a.proposals);
        assert_eq!(a.summary.cycle_count, 0);
        assert_eq!(a.applied_cut_count, 1, "exactly one cut breaks the cycle");
        assert_eq!(a.applied_refs_cut, 1);
        let mut freed = a.freed_modules.clone();
        freed.sort();
        assert_eq!(freed, vec!["a", "b", "c"]);
    }

    #[test]
    fn frontier_shows_unlock_at_the_one_cut() {
        let mg = one_edge_from_three();
        let a = analyze_with_budget(&mg, "demo", None, CutBudget::All);
        // One recommended cut -> frontier has rows for k = 0 and k = 1.
        assert_eq!(a.frontier.len(), 2);

        let s0 = &a.frontier[0];
        assert_eq!(s0.cuts_applied, 0);
        assert_eq!(s0.refs_cut, 0);
        assert_eq!(s0.crate_count, 1, "0 cuts: one big crate");
        assert_eq!(s0.largest_loc, 300);
        assert!(s0.freed_modules.is_empty());

        let s1 = &a.frontier[1];
        assert_eq!(s1.cuts_applied, 1);
        assert_eq!(s1.refs_cut, 1);
        assert_eq!(s1.crate_count, 3, "1 cut: three crates");
        assert_eq!(s1.largest_loc, 100, "largest crate drops 300 -> 100 at the unlock");
        assert_eq!(s1.freed_modules, vec!["a", "b", "c"]);
    }

    #[test]
    fn max_cuts_zero_keeps_the_blob() {
        let mg = one_edge_from_three();
        let a = analyze_with_budget(&mg, "demo", None, CutBudget::MaxCuts(0));
        assert_eq!(a.applied_cut_count, 0);
        assert_eq!(a.proposals.len(), 1, "no cuts: still one blob");
        assert_eq!(a.summary.cycle_count, 1, "the cycle remains");
        assert!(a.freed_modules.is_empty());
    }

    #[test]
    fn budget_refs_applies_cheapest_within_limit() {
        let mg = one_edge_from_three();
        // Budget of 0 refs cannot afford the weight-1 cut.
        let a0 = analyze_with_budget(&mg, "demo", None, CutBudget::Refs(0));
        assert_eq!(a0.applied_cut_count, 0);
        // Budget of 1 ref can afford exactly the single weight-1 cut.
        let a1 = analyze_with_budget(&mg, "demo", None, CutBudget::Refs(1));
        assert_eq!(a1.applied_cut_count, 1);
        assert_eq!(a1.proposals.len(), 3);
    }
}
