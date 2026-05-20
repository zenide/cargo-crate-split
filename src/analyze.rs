//! SCC computation, condensation, layering, and back-edge (feedback-arc-set) heuristic.

use std::collections::{HashMap, HashSet};

use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use serde::Serialize;

use crate::classify::{classify, CutClassification};
use crate::fas::{min_fas_order, NodeId, WeightedEdge};
use crate::frontier::{compute_frontier, FrontierStep};
use crate::graph::{LabelEdge, ModuleGraph, Occurrence};

/// A reference occurrence rendered for output.
#[derive(Clone, Debug, Serialize)]
pub struct OccurrenceOut {
    pub file: String,
    pub line: usize,
    pub path: String,
}

impl OccurrenceOut {
    fn from(o: &Occurrence) -> Self {
        OccurrenceOut {
            file: o.file.display().to_string(),
            line: o.line,
            path: o.path.clone(),
        }
    }
}

/// A proposed crate (one SCC).
#[derive(Clone, Debug, Serialize)]
pub struct CrateProposal {
    /// Suggested crate name, e.g. `<prefix>-models`.
    pub crate_name: String,
    /// Module labels contained in this crate.
    pub modules: Vec<String>,
    /// Total LOC across contained modules.
    pub loc: usize,
    /// Total file count across contained modules.
    pub file_count: usize,
    /// Dependency layer (0 = foundation/sink crates first... see `Layer`).
    pub layer: usize,
    /// Names of other proposed crates this crate depends on.
    pub depends_on: Vec<String>,
    /// True if this SCC contains a real cycle (>1 module).
    pub is_cycle: bool,
    /// Internal SCC index (for cross-referencing back-edges).
    pub scc: usize,
}

/// A back-edge to remove to break a cycle within a multi-module SCC.
#[derive(Clone, Debug, Serialize)]
pub struct BackEdge {
    pub source_module: String,
    pub target_module: String,
    pub occurrence_count: usize,
    pub occurrences: Vec<OccurrenceOut>,
    /// The SCC (crate proposal) this cycle belongs to.
    pub scc: usize,
    /// Inferred cut kind, difficulty, and a concrete how-to-break suggestion.
    pub classification: CutClassification,
}

/// A dependency layer grouping crate proposals.
#[derive(Clone, Debug, Serialize)]
pub struct Layer {
    pub level: usize,
    pub crates: Vec<String>,
}

/// How many of the recommended cuts to actually apply when forming the
/// proposed decomposition.
#[derive(Clone, Copy, Debug, Default)]
pub enum CutBudget {
    /// Apply every recommended cut (the maximal cheap decomposition).
    #[default]
    All,
    /// Apply only the `N` cheapest cuts.
    MaxCuts(usize),
    /// Apply cheapest cuts in order until cumulative occurrences would exceed `M`.
    Refs(usize),
}

/// Full analysis result.
#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    pub package_name: String,
    pub crate_prefix: String,
    pub granularity: usize,
    /// Proposed crates = SCCs of `G − applied_cuts`.
    pub proposals: Vec<CrateProposal>,
    pub layers: Vec<Layer>,
    /// The recommended cuts (cheapest first). These are the prerequisites — the
    /// price — of the proposed decomposition.
    pub back_edges: Vec<BackEdge>,
    /// How many of `back_edges` were applied to form `proposals` (cheapest first).
    pub applied_cut_count: usize,
    /// Cumulative reference occurrences across the applied cuts.
    pub applied_refs_cut: usize,
    /// Modules that sat in a multi-module SCC in the raw graph `G` but are
    /// standalone (their own single-module crate) after the applied cuts.
    pub freed_modules: Vec<String>,
    /// Effort/payoff frontier: one row per `k = 0..=back_edges.len()` cuts.
    pub frontier: Vec<FrontierStep>,
    pub summary: Summary,
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub module_count: usize,
    pub proposed_crate_count: usize,
    pub largest_crate_loc: usize,
    /// Multi-module cycles remaining AFTER the applied cuts (0 when all cuts
    /// applied, since the full cut set makes `G` acyclic at this granularity).
    pub cycle_count: usize,
    /// Total recommended cuts available (the full cheap cut set).
    pub recommended_cut_count: usize,
}

/// Derive a crate-name prefix from the package name (sanitized, no trailing -).
fn crate_prefix(package_name: &str) -> String {
    let mut s: String = package_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .to_lowercase();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s.trim_matches('-').to_string()
}

/// Suggest a crate name for an SCC given its module labels.
fn crate_name_for(prefix: &str, modules: &[String]) -> String {
    // Use the shortest (most foundational-looking) module label as the suffix,
    // sanitized into a crate-name fragment.
    let base = modules
        .iter()
        .min_by_key(|m| (m.len(), m.as_str()))
        .cloned()
        .unwrap_or_else(|| "crate".to_string());
    let frag: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .to_lowercase();
    let frag = frag.trim_matches('-').to_string();
    if prefix.is_empty() {
        frag
    } else {
        format!("{prefix}-{frag}")
    }
}

/// Run SCC + condensation + layering + back-edge analysis.
///
/// `respect_order`, when `Some`, is a user-supplied module order (foundation
/// first) used to detect back-edges instead of the ELS-computed ordering: any
/// intra-SCC edge that points "upward" relative to this order is reported as a
/// violation. Modules not named in the spec keep their ELS-computed position
/// (placed after the named ones they depend on). When `None`, the weighted ELS
/// (GR) heuristic computes the order per SCC.
pub fn analyze(mg: &ModuleGraph, package_name: &str, respect_order: Option<&[String]>) -> Analysis {
    analyze_with_budget(mg, package_name, respect_order, CutBudget::All)
}

/// Run the full analysis, applying `budget` to choose how many of the
/// recommended cuts form the proposed decomposition.
pub fn analyze_with_budget(
    mg: &ModuleGraph,
    package_name: &str,
    respect_order: Option<&[String]>,
    budget: CutBudget,
) -> Analysis {
    let g = &mg.graph;
    let prefix = crate_prefix(package_name);

    // 1. Compute the recommended cut set from the RAW graph's SCCs. These are
    //    the light minority-direction back-edges within each multi-module SCC.
    let raw_sccs = petgraph::algo::tarjan_scc(g);
    let mut back_edges: Vec<BackEdge> = Vec::new();
    for (i, comp) in raw_sccs.iter().enumerate() {
        if comp.len() <= 1 {
            continue;
        }
        back_edges.extend(scc_back_edges(g, comp, i, respect_order));
    }
    // Rank globally by fewest occurrences first (cheapest to remove).
    back_edges.sort_by(|a, b| {
        a.occurrence_count
            .cmp(&b.occurrence_count)
            .then(a.source_module.cmp(&b.source_module))
    });

    // Modules sitting in a multi-module SCC in the raw graph (candidates to be
    // "freed" once enough cuts land).
    let mut locked_in_raw: HashSet<String> = HashSet::new();
    for comp in &raw_sccs {
        if comp.len() > 1 {
            for &n in comp {
                locked_in_raw.insert(g[n].label());
            }
        }
    }

    // 2. Decide how many cuts to apply (cheapest first) per the budget.
    let applied_cut_count = match budget {
        CutBudget::All => back_edges.len(),
        CutBudget::MaxCuts(k) => k.min(back_edges.len()),
        CutBudget::Refs(m) => {
            let mut acc = 0usize;
            let mut taken = 0usize;
            for be in &back_edges {
                if acc + be.occurrence_count > m {
                    break;
                }
                acc += be.occurrence_count;
                taken += 1;
            }
            taken
        }
    };
    let applied_refs_cut: usize = back_edges[..applied_cut_count]
        .iter()
        .map(|b| b.occurrence_count)
        .sum();

    // 3. Build the proposed decomposition from `G − applied_cuts`.
    let applied: HashSet<LabelEdge> = back_edges[..applied_cut_count]
        .iter()
        .map(|b| LabelEdge {
            source: b.source_module.clone(),
            target: b.target_module.clone(),
        })
        .collect();
    let sccs = mg.sccs_without(&applied);
    let (mut proposals, layers) = build_proposals(g, &prefix, &sccs, &applied);

    // 4. Freed modules: locked in raw G, now standalone (their own crate).
    let standalone_now: HashSet<String> = sccs
        .iter()
        .filter(|c| c.len() == 1)
        .map(|c| g[c[0]].label())
        .collect();
    let mut freed_modules: Vec<String> = locked_in_raw
        .intersection(&standalone_now)
        .cloned()
        .collect();
    freed_modules.sort();

    // 5. Effort/payoff frontier (always over the full recommended cut set).
    let frontier = compute_frontier(mg, &prefix, &back_edges, &locked_in_raw);

    // Sort proposals by layer (foundation first), then name.
    proposals.sort_by(|a, b| a.layer.cmp(&b.layer).then(a.crate_name.cmp(&b.crate_name)));

    let module_count = g.node_count();
    let largest_crate_loc = proposals.iter().map(|p| p.loc).max().unwrap_or(0);
    let cycle_count = proposals.iter().filter(|p| p.is_cycle).count();

    Analysis {
        package_name: package_name.to_string(),
        crate_prefix: prefix,
        granularity: mg.granularity,
        summary: Summary {
            module_count,
            proposed_crate_count: proposals.len(),
            largest_crate_loc,
            cycle_count,
            recommended_cut_count: back_edges.len(),
        },
        proposals,
        layers,
        back_edges,
        applied_cut_count,
        applied_refs_cut,
        freed_modules,
        frontier,
    }
}

/// Build crate proposals + layer groupings from a set of SCCs (over any edge
/// view of `g`). Returns proposals (unsorted by layer) and the layer groupings.
///
/// `depends_on` and layering are computed from the condensation of the
/// POST-CUT graph `G − applied`: an inter-SCC edge is any non-cut `g`-edge whose
/// endpoints fall in different SCCs of this view. The applied cut edges are
/// excluded so the reported DAG matches the decomposition that the SCC split was
/// computed from (otherwise a cut `a -> b` plus a kept `b -> a` would render as a
/// phantom cycle in the dependency graph).
fn build_proposals(
    g: &petgraph::graph::DiGraph<crate::graph::ModuleNode, crate::graph::EdgeWeight>,
    prefix: &str,
    sccs: &[Vec<NodeIndex>],
    applied: &HashSet<LabelEdge>,
) -> (Vec<CrateProposal>, Vec<Layer>) {
    let mut scc_of: HashMap<NodeIndex, usize> = HashMap::new();
    for (i, comp) in sccs.iter().enumerate() {
        for &n in comp {
            scc_of.insert(n, i);
        }
    }

    let scc_count = sccs.len();
    let mut cond_succ: Vec<HashSet<usize>> = vec![HashSet::new(); scc_count];
    for edge in g.edge_references() {
        let le = LabelEdge {
            source: g[edge.source()].label(),
            target: g[edge.target()].label(),
        };
        if applied.contains(&le) {
            continue;
        }
        let s = scc_of[&edge.source()];
        let t = scc_of[&edge.target()];
        if s != t {
            cond_succ[s].insert(t);
        }
    }

    let layer_of = compute_layers(&cond_succ);

    let mut proposals: Vec<CrateProposal> = Vec::with_capacity(scc_count);
    for (i, comp) in sccs.iter().enumerate() {
        let mut modules: Vec<String> = comp.iter().map(|&n| g[n].label()).collect();
        modules.sort();
        let loc: usize = comp.iter().map(|&n| g[n].loc).sum();
        let file_count: usize = comp.iter().map(|&n| g[n].file_count).sum();
        let crate_name = crate_name_for(prefix, &modules);
        proposals.push(CrateProposal {
            crate_name,
            modules,
            loc,
            file_count,
            layer: layer_of[i],
            depends_on: Vec::new(),
            is_cycle: comp.len() > 1,
            scc: i,
        });
    }

    dedup_crate_names(&mut proposals);
    let scc_to_name: Vec<String> = proposals.iter().map(|p| p.crate_name.clone()).collect();
    for p in &mut proposals {
        let mut deps: Vec<String> = cond_succ[p.scc]
            .iter()
            .map(|&t| scc_to_name[t].clone())
            .collect();
        deps.sort();
        deps.dedup();
        p.depends_on = deps;
    }

    let mut layer_map: HashMap<usize, Vec<String>> = HashMap::new();
    for p in &proposals {
        layer_map.entry(p.layer).or_default().push(p.crate_name.clone());
    }
    let mut layers: Vec<Layer> = layer_map
        .into_iter()
        .map(|(level, mut crates)| {
            crates.sort();
            Layer { level, crates }
        })
        .collect();
    layers.sort_by_key(|l| l.level);

    (proposals, layers)
}

/// Longest-downward-chain layering: sinks (no successors) = layer 0.
fn compute_layers(succ: &[HashSet<usize>]) -> Vec<usize> {
    let n = succ.len();
    let mut layer = vec![0usize; n];
    let mut memo: Vec<Option<usize>> = vec![None; n];
    // DFS with memoization for longest path to a sink.
    fn longest(
        node: usize,
        succ: &[HashSet<usize>],
        memo: &mut Vec<Option<usize>>,
        on_stack: &mut Vec<bool>,
    ) -> usize {
        if let Some(v) = memo[node] {
            return v;
        }
        if on_stack[node] {
            // Should not happen on a DAG; guard against accidental cycles.
            return 0;
        }
        on_stack[node] = true;
        let mut best = 0;
        for &s in &succ[node] {
            let d = 1 + longest(s, succ, memo, on_stack);
            if d > best {
                best = d;
            }
        }
        on_stack[node] = false;
        memo[node] = Some(best);
        best
    }
    let mut on_stack = vec![false; n];
    for (i, slot) in layer.iter_mut().enumerate() {
        *slot = longest(i, succ, &mut memo, &mut on_stack);
    }
    layer
}

/// Disambiguate duplicate crate names in place.
fn dedup_crate_names(proposals: &mut [CrateProposal]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in proposals.iter() {
        *counts.entry(p.crate_name.clone()).or_default() += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    for p in proposals.iter_mut() {
        if counts.get(&p.crate_name).copied().unwrap_or(0) > 1 {
            let n = seen.entry(p.crate_name.clone()).or_default();
            *n += 1;
            p.crate_name = format!("{}-{}", p.crate_name, *n);
        }
    }
}

/// Compute back-edges within a single SCC.
///
/// Default mode: a weighted vertex ordering (Eades–Lin–Smyth / GR heuristic,
/// weighted by occurrence count) that approximately maximizes forward-edge
/// weight. Back-edges = intra-SCC edges `u -> v` where `v` precedes `u` in the
/// order. Because the order maximizes forward weight, the cut edges are the
/// light minority-direction ones.
///
/// `respect_order` mode: use the user's pinned module order (foundation first)
/// as the vertex order. Modules not named keep their ELS-computed position. Any
/// edge that points "upward" (target later than source in the intended order)
/// is reported as a violation to cut.
fn scc_back_edges(
    g: &petgraph::graph::DiGraph<crate::graph::ModuleNode, crate::graph::EdgeWeight>,
    comp: &[NodeIndex],
    scc_index: usize,
    respect_order: Option<&[String]>,
) -> Vec<BackEdge> {
    let member: HashSet<NodeIndex> = comp.iter().copied().collect();

    // Map this SCC's NodeIndex set to contiguous FAS NodeIds.
    let mut to_id: HashMap<NodeIndex, NodeId> = HashMap::new();
    let mut to_node: Vec<NodeIndex> = Vec::with_capacity(comp.len());
    let mut sorted_comp = comp.to_vec();
    sorted_comp.sort_by_key(|&n| g[n].label());
    for &n in &sorted_comp {
        to_id.insert(n, to_node.len());
        to_node.push(n);
    }
    let nodes: Vec<NodeId> = (0..to_node.len()).collect();

    // Build weighted edges among the SCC members (collapse parallel edges by
    // summing occurrence counts).
    let mut weighted: Vec<WeightedEdge> = Vec::new();
    for edge in g.edge_references() {
        let s = edge.source();
        let t = edge.target();
        if s == t || !member.contains(&s) || !member.contains(&t) {
            continue;
        }
        weighted.push(WeightedEdge {
            from: to_id[&s],
            to: to_id[&t],
            weight: edge.weight().len() as u64,
        });
    }

    // Determine the vertex order, oriented FOUNDATION-FIRST (dependencies
    // before dependents) so both modes share one back-edge rule.
    //
    // `min_fas_order` returns a source-first order: a "forward" edge `u -> v`
    // (kept, not cut) has `u` before `v`. Since our edges mean "source depends
    // on target", source-first puts dependents first. We reverse it to get a
    // foundation-first order, matching the `--respect-order` spec orientation.
    // The pinned spec is already foundation-first.
    let order: Vec<NodeId> = match respect_order {
        None => {
            let mut o = min_fas_order(&nodes, &weighted);
            o.reverse();
            o
        }
        Some(spec) => pinned_order(g, &sorted_comp, &to_id, &nodes, &weighted, spec),
    };
    let pos: HashMap<NodeId, usize> = order.iter().enumerate().map(|(i, &n)| (n, i)).collect();

    // In a foundation-first order a healthy edge points "downward" (a dependent
    // references a more-foundational module: pos(target) < pos(source)). A
    // back-edge / violation is `u -> v` where `v` is LATER (higher) than `u` —
    // a module depending "upward".
    let mut out: Vec<BackEdge> = Vec::new();
    for edge in g.edge_references() {
        let s = edge.source();
        let t = edge.target();
        if s == t || !member.contains(&s) || !member.contains(&t) {
            continue;
        }
        let (sp, tp) = (pos[&to_id[&s]], pos[&to_id[&t]]);
        if tp > sp {
            let occs: Vec<OccurrenceOut> =
                edge.weight().iter().map(OccurrenceOut::from).collect();
            let source_module = g[s].label();
            let target_module = g[t].label();
            let classification = classify(&source_module, &target_module, &occs);
            out.push(BackEdge {
                source_module,
                target_module,
                occurrence_count: occs.len(),
                occurrences: occs,
                scc: scc_index,
                classification,
            });
        }
    }
    out.sort_by(|a, b| {
        a.occurrence_count
            .cmp(&b.occurrence_count)
            .then(a.source_module.cmp(&b.source_module))
    });
    out
}

/// Build a pinned vertex order from a user `--respect-order` spec.
///
/// Named modules take the positions given by `spec` (foundation first).
/// Unlisted modules are appended after all named ones, placed by their
/// ELS-computed relative order so they still sort after the pinned modules they
/// depend on. The result is a permutation of `nodes`.
fn pinned_order(
    g: &petgraph::graph::DiGraph<crate::graph::ModuleNode, crate::graph::EdgeWeight>,
    sorted_comp: &[NodeIndex],
    to_id: &HashMap<NodeIndex, NodeId>,
    nodes: &[NodeId],
    weighted: &[WeightedEdge],
    spec: &[String],
) -> Vec<NodeId> {
    // Label -> NodeId for members of this SCC.
    let label_to_id: HashMap<String, NodeId> = sorted_comp
        .iter()
        .map(|&n| (g[n].label(), to_id[&n]))
        .collect();

    let mut placed: HashSet<NodeId> = HashSet::new();
    let mut order: Vec<NodeId> = Vec::with_capacity(nodes.len());

    // 1. Named modules present in this SCC, in spec order.
    for label in spec {
        if let Some(&id) = label_to_id.get(label) {
            if placed.insert(id) {
                order.push(id);
            }
        }
    }

    // 2. Unlisted modules: appended after all named ones (so they sort after
    //    the pinned modules they depend on), using their ELS-relative order
    //    reversed to foundation-first to match the spec orientation.
    let mut els = min_fas_order(nodes, weighted);
    els.reverse();
    for id in els {
        if placed.insert(id) {
            order.push(id);
        }
    }

    order
}
